use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::raw::RawSample;
use crate::streams::DeviceInfo;

/// Writes one session to disk: raw samples as CSV, derived metrics as JSON
/// lines, and a metadata file. Recording is secondary to streaming, so a
/// disk write failing here must not take the BLE loop down with it — every
/// write method returns `io::Result` and the caller decides how loud to be
/// about a failure rather than this type panicking or silently swallowing
/// one itself.
pub struct Recorder {
    dir: PathBuf,
    raw: BufWriter<File>,
    derived: BufWriter<File>,
}

impl Recorder {
    /// A directory name safe on every filesystem: no colons, sortable.
    pub fn session_name() -> String {
        chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string()
    }

    pub fn start(root: &Path, info: &DeviceInfo, name: &str) -> io::Result<Self> {
        let dir = root.join(name);
        fs::create_dir_all(&dir)?;

        let mut raw = BufWriter::new(File::create(dir.join("raw.csv"))?);
        writeln!(raw, "timestamp,{}", info.channel_names.join(","))?;

        let derived = BufWriter::new(File::create(dir.join("derived.jsonl"))?);

        let meta = serde_json::json!({
            "deviceId": info.device_id,
            "deviceNickname": info.device_nickname,
            "channelNames": info.channel_names,
            "channels": info.channels,
            "samplingRate": info.sampling_rate,
            "appVersion": env!("CARGO_PKG_VERSION"),
            "startedAt": chrono::Local::now().to_rfc3339(),
        });
        fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;

        Ok(Self { dir, raw, derived })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn write_raw(&mut self, s: &RawSample) -> io::Result<()> {
        write!(self.raw, "{}", s.timestamp)?;
        for v in &s.data {
            write!(self.raw, ",{v}")?;
        }
        writeln!(self.raw)
    }

    pub fn write_derived(&mut self, stream: &str, value: &serde_json::Value) -> io::Result<()> {
        let line = serde_json::json!({
            "t": chrono::Local::now().timestamp_millis(),
            "stream": stream,
            "value": value,
        });
        writeln!(self.derived, "{line}")
    }
}

/// `BufWriter`'s own `Drop` flushes but has nowhere to send a failure — the
/// stdlib docs are explicit that a flush error at that point is discarded.
/// For most `BufWriter` users that is an acceptable trade (best-effort
/// cleanup), but here it would mean the last (up to) buffer's worth of
/// recording — as much as several hundred milliseconds of raw samples —
/// can vanish with no signal at all, on exactly the path (dropping the
/// `Recorder`) that Task 14 uses to stop a session. So `Recorder` flushes
/// explicitly first and reports a failure the same way every other
/// recorder write failure is reported in this codebase; the field-level
/// `BufWriter`s still flush again right after, harmlessly, since a second
/// flush of an already-empty buffer is a no-op.
impl Drop for Recorder {
    fn drop(&mut self) {
        if let Err(e) = self.raw.flush() {
            eprintln!("warning: recorder failed to flush raw.csv on drop: {e}");
        }
        if let Err(e) = self.derived.flush() {
            eprintln!("warning: recorder failed to flush derived.jsonl on drop: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::RawSample;
    use crate::streams::DeviceInfo;

    fn info() -> DeviceInfo {
        DeviceInfo {
            device_id: "abc".into(),
            device_nickname: "Crown-1234".into(),
            channel_names: vec!["CP3".into(), "C3".into()],
            channels: 2,
            sampling_rate: 256.0,
        }
    }

    #[test]
    fn writes_meta_raw_header_and_rows() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut rec = Recorder::start(&root, &info(), "session-a").unwrap();
        rec.write_raw(&RawSample { timestamp: 10, marker: 0, data: vec![1.5, -2.5] }).unwrap();
        rec.write_derived("calm", &serde_json::json!({"probability": 0.5})).unwrap();
        drop(rec);

        let dir = root.join("session-a");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().next().unwrap(), "timestamp,CP3,C3");
        assert_eq!(raw.lines().nth(1).unwrap(), "10,1.5,-2.5");

        let derived = std::fs::read_to_string(dir.join("derived.jsonl")).unwrap();
        assert!(derived.contains("\"stream\":\"calm\""));

        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["channels"], 2);
        assert_eq!(meta["samplingRate"], 256.0);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn session_names_are_filesystem_safe() {
        let name = Recorder::session_name();
        assert!(!name.contains(':'), "colons break on some filesystems: {name}");
        assert!(name.len() >= 10);
    }

    #[test]
    fn dropped_recorder_flushes_all_buffered_data_without_an_explicit_flush() {
        let root = std::env::temp_dir().join(format!("crown-test-flush-{}", std::process::id()));
        let mut rec = Recorder::start(&root, &info(), "session-b").unwrap();

        // Comfortably exceeds BufWriter's default 8 KiB buffer, so part of
        // this is still sitting unflushed in memory the moment `rec` drops
        // below — nothing here ever calls flush() explicitly.
        let n: u64 = 2000;
        for i in 0..n {
            rec.write_raw(&RawSample { timestamp: i, marker: 0, data: vec![1.5, -2.5] }).unwrap();
        }
        drop(rec);

        let dir = root.join("session-b");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().count(), n as usize + 1, "header plus every row must survive drop");
        assert_eq!(raw.lines().last().unwrap(), format!("{},1.5,-2.5", n - 1));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
