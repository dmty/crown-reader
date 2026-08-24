use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crown_core::auth::{AuthProfile, Credentials, TokenCoordinator};
use crown_core::backoff;
use crown_core::state::Live;

#[tokio::main]
async fn main() {
    // Load a `.env` from the working directory or any parent, if one exists.
    // Absent file is the normal case, not an error — already-exported variables
    // take precedence, so a real environment always wins over the file.
    let _ = dotenvy::dotenv();

    let raw_enabled = std::env::args().any(|a| a == "--raw");
    let creds = match Credentials::from_env() {
        Ok(credentials) => credentials,
        Err(error) => {
            eprintln!("session failed: {error:#}");
            std::process::exit(1);
        }
    };
    let profile = match AuthProfile::password(creds.email, creds.password, creds.device_id) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("session failed: {error}");
            std::process::exit(1);
        }
    };
    let auth = Arc::new(TokenCoordinator::from_profile(profile));
    let live = Arc::new(Mutex::new(Live::new()));
    let recorder = Arc::new(Mutex::new(None));

    let (stop, rx) = crown_core::ble::Stop::pair();
    let worker = tokio::spawn({
        let live = live.clone();
        async move {
            let _stop = stop;
            backoff::supervise(live, auth, raw_enabled, recorder, rx).await
        }
    });

    let started = Instant::now();
    let mut last_dropped = 0;
    let mut last_raw_samples: u64 = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        // A poisoned lock only means a prior holder panicked; recover the state
        // instead of panicking here too, so the `Err(join_err)` arm below can
        // report the actual cause instead of an unrelated `PoisonError`.
        let snap = crown_core::sync::lock(&live).snapshot(40);
        let samples: usize = snap.waveform.first().map(|w| w.len()).unwrap_or(0);
        println!(
            "[{:>4}s] {:?} device={:?} channels={} calm={:.2} focus={:.2} cols={} dropped={} (+{})",
            started.elapsed().as_secs(),
            snap.connection,
            snap.device_name.as_deref().unwrap_or("-"),
            snap.channel_names.len(),
            snap.calm,
            snap.focus,
            samples,
            snap.dropped_frames,
            snap.dropped_frames - last_dropped,
        );
        // Only when it matters: a healthy stream sits at zero, and a line
        // that always prints "stale=0ms" trains the eye to skip it.
        if let Some(ms) = snap.metric_staleness_ms.filter(|ms| *ms > 2_000) {
            println!("  WARNING: metrics are {:.0}s behind", ms as f64 / 1000.0);
        }
        for (name, q) in &snap.quality {
            print!("  {name}:{:?}", q.status);
        }
        if !snap.quality.is_empty() {
            println!();
        }
        last_dropped = snap.dropped_frames;

        if raw_enabled {
            // `raw` is the accepted-sample rate over OSC: a healthy stream
            // reads ~250/s against the device's 256 Hz, the shortfall being
            // the ~2% UDP loss measured on this link. A steady 0 means the
            // device is not broadcasting — check that OSC is enabled in its
            // settings and that it shares this subnet.
            //
            // A healthy ch0 extent stays bounded and roughly stable tick to
            // tick; a misaligned decode makes it jump by orders of magnitude
            // between ticks. It's a min/max over a rolling 10s ring, so it's a
            // screen, not a verdict — one blink latches it high for 10s.
            //
            // A slowly climbing `dropped` is that same ~2% loss and expected;
            // a fast climb instead points at a decode or channel-count fault
            // — or a real network stall, which climbs exactly the same way.
            // `raw_enabled` starts false and is only set once `supervise` is
            // past adapter setup (backoff.rs), so gate on the session being
            // active too — otherwise this fires during that startup window,
            // and on an adapter failure, and blames UDP 9000 for either.
            if snap.connection.is_active() && !snap.raw_enabled {
                println!("  raw listener is not running — check for another process already holding UDP 9000");
            }

            let raw_rate = snap.raw_samples - last_raw_samples;
            last_raw_samples = snap.raw_samples;
            let extent = snap.waveform.first().and_then(|cols| {
                cols.iter()
                    .copied()
                    .reduce(|(alo, ahi), (lo, hi)| (alo.min(lo), ahi.max(hi)))
            });
            match extent {
                Some((lo, hi)) => {
                    println!("  raw={raw_rate}/s cols={samples} ch0_extent=[{lo:.1}, {hi:.1}]")
                }
                None => println!("  raw={raw_rate}/s cols={samples} ch0_extent=[no data yet]"),
            }
        }

        if worker.is_finished() {
            break;
        }
    }

    // Every exit path names what happened on stderr: this tool's whole job is
    // to tell a human wearing the headset whether the Bluetooth path works,
    // so a silent stop is indistinguishable from a hang.
    match worker.await {
        Ok(Ok(())) => eprintln!("session ended: device disconnected"),
        Ok(Err(e)) => {
            eprintln!("session failed: {e:#}");
            std::process::exit(1);
        }
        Err(join_err) => {
            eprintln!("session failed: worker task panicked: {join_err}");
            std::process::exit(1);
        }
    }
}
