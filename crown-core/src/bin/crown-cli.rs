use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crown_core::auth::{Credentials, KeyringStore};
use crown_core::backoff;
use crown_core::state::Live;

#[tokio::main]
async fn main() {
    let raw_enabled = std::env::args().any(|a| a == "--raw");
    let creds = match Credentials::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("session failed: {e:#}");
            std::process::exit(1);
        }
    };
    let store = Arc::new(KeyringStore {
        account: creds.email.clone(),
    });
    let live = Arc::new(Mutex::new(Live::new()));
    let recorder = Arc::new(Mutex::new(None));

    let worker = tokio::spawn({
        let live = live.clone();
        let store = store.clone();
        async move {
            backoff::supervise(live, creds, store, raw_enabled, recorder).await;
            Ok::<(), anyhow::Error>(())
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
        let snap = live.lock().unwrap_or_else(|e| e.into_inner()).snapshot(40);
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
        for (name, q) in &snap.quality {
            print!("  {name}:{:?}", q.status);
        }
        if !snap.quality.is_empty() {
            println!();
        }
        last_dropped = snap.dropped_frames;

        if raw_enabled {
            // `raw` is the accepted-sample rate: compare it to the device's
            // reported sampling rate directly — a healthy Crown reads ~256/s,
            // and a stalled stream reads 0 even while calm/focus keep moving.
            //
            // A healthy ch0 extent stays bounded and roughly stable tick to
            // tick; a misaligned decode makes it jump by orders of magnitude
            // between ticks. It's a min/max over a rolling 10s ring, so it's a
            // screen, not a verdict — one blink latches it high for 10s.
            //
            // A climbing `dropped` has three possible causes: a channel-count/
            // sample-size mismatch (the likely signature of misaligned raw
            // decoding), a non-finite sample value, or configure() rejecting a
            // bad deviceInfo report.
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
