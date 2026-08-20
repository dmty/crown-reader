use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use uuid::Uuid;

use crate::auth::{token, Credentials, TokenStore};
use crate::raw::RawDecoder;
use crate::state::{ConnectionState, Live};
use crate::stitch::Stitcher;
use crate::streams::{Awareness, DeviceInfo, PowerByBand, SignalQuality};

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x00001803_0000_1000_8000_00805f9b34fb);
pub const CHAR_AUTH: Uuid = Uuid::from_u128(0x7f9f1a35_9816_471b_bf67_2ec6f2295a1d);
pub const CHAR_DEVICE_INFO: Uuid = Uuid::from_u128(0x97b81f68_04cf_4650_a044_14924f11b9ee);
pub const CHAR_RAW: Uuid = Uuid::from_u128(0x009cf0bb_b68d_4af1_a0e5_625f2eb964a6);
pub const CHAR_POWER_BY_BAND: Uuid = Uuid::from_u128(0x2f6236dd_215a_427f_b94c_ab5df71937af);
pub const CHAR_FOCUS: Uuid = Uuid::from_u128(0x8e12baf1_81bb_4a1b_8948_9e68a4457d2a);
pub const CHAR_CALM: Uuid = Uuid::from_u128(0x7d47617d_a60a_41d1_8df6_cfb78d02ffeb);
pub const CHAR_SIGNAL_QUALITY: Uuid = Uuid::from_u128(0xcf28ed0c_20cd_48ed_93c5_ee2fb265099a);

const NAME_PREFIXES: [&str; 2] = ["Crown-", "Notion-"];

/// Scans for a headset by advertised name. The service UUID is not used as a
/// scan filter because the device does not reliably advertise it.
pub async fn find_crown(adapter: &Adapter) -> Result<Peripheral> {
    adapter.start_scan(ScanFilter::default()).await?;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            let name = match p.properties().await? {
                Some(props) => props.local_name.unwrap_or_default(),
                None => continue,
            };
            if NAME_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
                adapter.stop_scan().await?;
                return Ok(p);
            }
        }
    }
    adapter.stop_scan().await?;
    Err(anyhow!("no Crown or Notion device found within 10 seconds"))
}

async fn characteristic(
    p: &Peripheral,
    uuid: Uuid,
) -> Result<btleplug::api::Characteristic> {
    p.characteristics()
        .into_iter()
        .find(|c| c.uuid == uuid)
        .ok_or_else(|| anyhow!("characteristic {uuid} not found on device"))
}

/// Writes `jwt` to the auth characteristic.
///
/// A single write is tried first. If it fails with what looks like a length
/// or MTU error, we fall back to writing MTU-sized chunks with-response,
/// since CoreBluetooth enforces a hard per-write length cap and doesn't
/// always perform a queued long write the way Web Bluetooth does. btleplug's
/// CoreBluetooth backend reports any write failure as `Error::RuntimeError`
/// carrying whatever text the OS gave it, so detection here is a
/// best-effort keyword match rather than a typed error — only real hardware
/// (Task 8) can confirm which path this device actually needs.
async fn write_jwt(p: &Peripheral, auth: &btleplug::api::Characteristic, jwt: &str) -> Result<()> {
    let bytes = jwt.as_bytes();
    if let Err(e) = p.write(auth, bytes, WriteType::WithResponse).await {
        let msg = e.to_string().to_lowercase();
        let looks_like_a_length_error =
            ["length", "mtu", "too long", "exceed"].iter().any(|kw| msg.contains(kw));
        if !looks_like_a_length_error {
            return Err(e.into());
        }
        let mtu = match p.mtu() {
            0 => 512,
            n => n as usize,
        };
        let chunk_size = mtu.saturating_sub(3).max(1);
        for chunk in bytes.chunks(chunk_size) {
            p.write(auth, chunk, WriteType::WithResponse).await?;
        }
    }
    Ok(())
}

/// Writes the minted JWT and reads back `[isAuthenticated, expiresIn]`.
pub async fn authenticate(p: &Peripheral, jwt: &str) -> Result<(bool, Option<f64>)> {
    let auth = characteristic(p, CHAR_AUTH).await?;
    write_jwt(p, &auth, jwt).await?;
    let raw = p.read(&auth).await?;
    let parsed: serde_json::Value = serde_json::from_slice(&raw)?;
    let ok = parsed
        .get(0)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow!("auth response missing boolean: {parsed}"))?;
    let expires_in = parsed.get(1).and_then(|v| v.as_f64());
    Ok((ok, expires_in))
}

/// Connects, authenticates, subscribes, and pumps notifications into `live`
/// until the connection drops. Returns Ok(()) on a clean disconnect.
pub async fn run(
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    raw_enabled: bool,
) -> Result<()> {
    let set = |s: ConnectionState| {
        let mut l = live.lock().unwrap();
        l.connection = s;
        l.touch();
    };

    set(ConnectionState::Scanning);
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter available"))?;
    let peripheral = find_crown(&adapter).await?;

    set(ConnectionState::Connecting);
    peripheral.connect().await?;
    peripheral.discover_services().await?;

    set(ConnectionState::Authenticating);
    let mut jwt = token(&creds, store.as_ref(), false).await?;
    let (mut ok, _) = authenticate(&peripheral, &jwt).await?;
    if !ok {
        // A cached token can outlive its validity; mint once more, then give up.
        store.clear();
        jwt = token(&creds, store.as_ref(), true).await?;
        (ok, _) = authenticate(&peripheral, &jwt).await?;
    }
    if !ok {
        set(ConnectionState::Failed);
        return Err(anyhow!("device rejected the Bluetooth token twice"));
    }

    // The notification stream must exist before we subscribe: btleplug only
    // delivers notifications that arrive after this call, and deviceInfo may
    // be sent only once, right on subscribe. Creating it late risks losing
    // that one notification and, under the deviceInfo-gated raw subscribe
    // below, never subscribing to raw at all for the life of the connection.
    let mut notifications = peripheral.notifications().await?;

    // `raw` is deliberately not in this initial batch. It has no delimiter or
    // checksum, so decoding it requires knowing the channel count from
    // deviceInfo first; it is subscribed lazily below once that arrives.
    for uuid in [CHAR_DEVICE_INFO, CHAR_POWER_BY_BAND, CHAR_CALM, CHAR_FOCUS, CHAR_SIGNAL_QUALITY] {
        peripheral.subscribe(&characteristic(&peripheral, uuid).await?).await?;
    }

    {
        let mut l = live.lock().unwrap();
        l.raw_enabled = raw_enabled;
        l.touch();
    }
    set(ConnectionState::Streaming);

    let mut stitchers: std::collections::HashMap<Uuid, Stitcher> = Default::default();
    let mut raw_decoder = RawDecoder::default();
    let mut raw_subscribed = false;

    while let Some(n) = notifications.next().await {
        if n.uuid == CHAR_RAW {
            let channels = {
                let l = live.lock().unwrap();
                l.device.as_ref().map(|d| d.channels).unwrap_or(0)
            };
            if channels == 0 {
                // Belt-and-braces: raw is never subscribed before deviceInfo
                // arrives, so this should be unreachable in practice.
                continue;
            }
            let samples = raw_decoder.push(&n.value, channels);
            let mut l = live.lock().unwrap();
            for s in &samples {
                l.push_raw(s);
            }
            continue;
        }

        let lines = stitchers.entry(n.uuid).or_default().push(&n.value);
        for line in lines {
            let mut device_info_configured = false;
            {
                let mut l = live.lock().unwrap();
                let parsed = match n.uuid {
                    CHAR_DEVICE_INFO => serde_json::from_str::<DeviceInfo>(&line)
                        .map(|d| {
                            l.configure(d);
                            device_info_configured = true;
                        })
                        .is_ok(),
                    CHAR_POWER_BY_BAND => serde_json::from_str::<PowerByBand>(&line)
                        .map(|b| {
                            l.bands = Some(b);
                            l.touch();
                        })
                        .is_ok(),
                    CHAR_CALM => serde_json::from_str::<Awareness>(&line)
                        .map(|a| {
                            l.calm = a.probability as f32;
                            l.touch();
                        })
                        .is_ok(),
                    CHAR_FOCUS => serde_json::from_str::<Awareness>(&line)
                        .map(|a| {
                            l.focus = a.probability as f32;
                            l.touch();
                        })
                        .is_ok(),
                    CHAR_SIGNAL_QUALITY => serde_json::from_str::<SignalQuality>(&line)
                        .map(|q| {
                            l.quality = q;
                            l.touch();
                        })
                        .is_ok(),
                    _ => true,
                };
                if !parsed {
                    l.dropped_frames += 1;
                    l.touch();
                }
            }

            if device_info_configured && raw_enabled && !raw_subscribed {
                peripheral.subscribe(&characteristic(&peripheral, CHAR_RAW).await?).await?;
                raw_subscribed = true;
            }
        }
    }

    set(ConnectionState::Disconnected);
    Ok(())
}
