use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crown_core::auth::{Credentials, KeyringStore};
use crown_core::ble;
use crown_core::state::Live;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw_enabled = std::env::args().any(|a| a == "--raw");
    let creds = Credentials::from_env()?;
    let store = Arc::new(KeyringStore {
        account: creds.email.clone(),
    });
    let live = Arc::new(Mutex::new(Live::new()));

    let worker = tokio::spawn({
        let live = live.clone();
        let store = store.clone();
        async move { ble::run(live, creds, store, raw_enabled).await }
    });

    let started = Instant::now();
    let mut last_dropped = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let snap = live.lock().unwrap().snapshot(40);
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
            // Raw alignment check: the raw characteristic is packed binary with no
            // delimiter or checksum, so a decoder reading it at the wrong byte
            // offset produces plausible-looking garbage instead of an error.
            // Snapshot doesn't expose raw timestamps, so watch channel 0's
            // decimated extent instead: real EEG sits in a small, stable
            // microvolt-scale range. A wild (e.g. ~1e300) or wildly asymmetric
            // min/max means the decoder has lost alignment.
            let extent = snap.waveform.first().and_then(|cols| {
                cols.iter()
                    .copied()
                    .reduce(|(alo, ahi), (lo, hi)| (alo.min(lo), ahi.max(hi)))
            });
            match extent {
                Some((lo, hi)) => println!("  raw: cols={samples} ch0_extent=[{lo:.1}, {hi:.1}]"),
                None => println!("  raw: cols={samples} ch0_extent=[no data yet]"),
            }
        }

        if worker.is_finished() {
            break;
        }
    }

    worker.await??;
    Ok(())
}
