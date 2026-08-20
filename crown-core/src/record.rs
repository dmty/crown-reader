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
}
