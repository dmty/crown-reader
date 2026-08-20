use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::raw::RawSample;
use crate::state::MAX_CHANNELS;
use crate::streams::DeviceInfo;

/// Writes one session to disk: raw samples as CSV, derived metrics as JSON
/// lines, and a metadata file. Recording is secondary to streaming, so a
/// disk write failing here must not take the BLE loop down with it — every
/// write method returns `io::Result` and the caller decides how loud to be
/// about a failure rather than this type panicking or silently swallowing
/// one itself.
#[derive(Debug)]
pub struct Recorder {
    dir: PathBuf,
    raw: BufWriter<File>,
    derived: BufWriter<File>,
    /// The reconciled channel count `raw.csv`'s header (and `meta.json`'s
    /// `channels`) were built against — see `start`'s doc comment. Every
    /// row `write_raw` accepts must have exactly this many values.
    columns: usize,
    /// Set once the first raw sample's device timestamp has been paired
    /// with a host timestamp in `derived.jsonl` — see `write_raw`.
    clock_anchor_written: bool,
}

impl Recorder {
    /// A directory name safe on every filesystem: no colons, sortable.
    pub fn session_name() -> String {
        chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string()
    }

    /// Starts a new session under `root/name`.
    ///
    /// `name` must be a single path component: no separators, and not `.`
    /// or `..`. It is rejected outright otherwise rather than joined onto
    /// `root` unchecked, since an absolute path or a `..` component would
    /// let it write outside `root`. The only caller today is
    /// `session_name()`, which always satisfies this.
    ///
    /// All three files are created with `create_new` semantics: a session
    /// directory that already holds any of them fails with
    /// `io::ErrorKind::AlreadyExists` rather than silently truncating a
    /// prior recording. This matters because `session_name()`'s resolution
    /// is one second — a stop/start double-tap within the same second would
    /// otherwise collide and destroy the earlier session's files.
    ///
    /// `info` is reconciled, not trusted as-is: `channels` is capped by
    /// both `channel_names.len()` and `MAX_CHANNELS`, and a report with
    /// zero usable channels after that is rejected with
    /// `io::ErrorKind::InvalidData` before any file is created, rather than
    /// producing a recorder whose header can never match a real sample.
    pub fn start(root: &Path, info: &DeviceInfo, name: &str) -> io::Result<Self> {
        if name.is_empty() || name == "." || name == ".." || name.contains(std::path::is_separator)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid session name: {name:?}"),
            ));
        }

        // Reconciled the same way `Live::configure` reconciles a device's
        // self-reported `DeviceInfo`: the wire format does not guarantee
        // `channels == channel_names.len()`, and does not bound either
        // against a corrupt or hostile report, but the header written
        // below, `meta.json`'s `channels`, and every row's width all must
        // agree with each other or the CSV is malformed. Doing that
        // reconciling here — capped at `MAX_CHANNELS` exactly as
        // `Live::configure` caps it, rather than trusting the caller to
        // have already done it — means the three agree by construction no
        // matter what `info` was built from.
        let columns = info.channels.min(info.channel_names.len()).min(MAX_CHANNELS);
        if columns == 0 {
            // `Live::configure` makes the matching call for the same
            // reason: applying a zero-channel report leaves nothing usable
            // configured. A caller finding out here, before any file is
            // created, is far better than discovering it when the first
            // real sample latches recording off.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DeviceInfo has zero usable channels after reconciliation",
            ));
        }
        let mut channel_names = info.channel_names.clone();
        channel_names.truncate(columns);

        let dir = root.join(name);
        fs::create_dir_all(&dir)?;

        let mut raw = BufWriter::new(new_file(&dir.join("raw.csv"))?);
        writeln!(raw, "timestamp,{}", channel_names.join(","))?;

        let derived = BufWriter::new(new_file(&dir.join("derived.jsonl"))?);

        let meta = serde_json::json!({
            "deviceId": info.device_id,
            "deviceNickname": info.device_nickname,
            "channelNames": channel_names,
            "channels": columns,
            "samplingRate": info.sampling_rate,
            "appVersion": env!("CARGO_PKG_VERSION"),
            "startedAt": chrono::Local::now().to_rfc3339(),
            // `Live::push_raw` drops a sample containing a non-finite value
            // before it ever reaches the waveform; `write_raw` below does
            // not — it writes whatever the device sent. The two are
            // supposed to disagree here: this is the ground-truth file.
            "rawCsvIsVerbatim": true,
        });
        new_file(&dir.join("meta.json"))?.write_all(&serde_json::to_vec_pretty(&meta)?)?;

        Ok(Self { dir, raw, derived, columns, clock_anchor_written: false })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes one raw sample as a CSV row.
    ///
    /// The first successful call also anchors the clocks: it writes one
    /// `derived.jsonl` line (`"stream":"clockAnchor"`) pairing that
    /// sample's device-clock `timestamp` with the host-clock timestamp
    /// `write_derived` stamps every line with, so `raw.csv`'s device time
    /// and `derived.jsonl`'s host time can be aligned after the fact.
    ///
    /// Rejects — without writing anything — a sample whose channel count
    /// disagrees with the header `start` wrote. `Live::configure` can
    /// change the live channel count mid-session on a `deviceInfo`
    /// re-notification (which is not itself written to `derived.jsonl`, so
    /// there would be no record of why the row width changed); writing a
    /// row against a stale header would corrupt `raw.csv` with no way to
    /// recover alignment afterward, so this reports the mismatch as an
    /// ordinary `io::Result` error instead, indistinguishable to the
    /// caller from a disk failure — a short, honest recording beats a
    /// corrupt one.
    pub fn write_raw(&mut self, s: &RawSample) -> io::Result<()> {
        if s.data.len() != self.columns {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "sample has {} channel value(s), recorder was started with {}",
                    s.data.len(),
                    self.columns
                ),
            ));
        }

        if !self.clock_anchor_written {
            self.write_derived("clockAnchor", &serde_json::json!(s.timestamp))?;
            self.clock_anchor_written = true;
        }

        // Formatted as one `String` and written in a single call rather
        // than one `write!` per value: with the latter, a failure partway
        // through a row left whatever columns had already reached the
        // `BufWriter`'s buffer sitting there with no newline, which
        // `Drop`'s flush would later write out as a truncated final line.
        // A row is now either fully formatted before it ever reaches
        // `self.raw`, or not written at all.
        let mut row = String::new();
        write!(row, "{}", s.timestamp).unwrap();
        for v in &s.data {
            write!(row, ",{v}").unwrap();
        }
        row.push('\n');
        self.raw.write_all(row.as_bytes())
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

/// Creates `path`, failing with `io::ErrorKind::AlreadyExists` rather than
/// truncating if something is already there — see `Recorder::start`'s doc
/// comment on why a session never clobbers a prior one.
fn new_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
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
    fn write_raw_rejects_a_width_mismatched_sample_without_corrupting_the_file() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut rec = Recorder::start(&root, &info(), "session-width").unwrap();

        let err = rec
            .write_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, 2.0, 3.0] })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(rec);

        // The rejected sample must not have left a malformed row behind.
        let dir = root.join("session-width");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().count(), 1, "only the header, no row from the rejected sample");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn start_reconciles_channels_and_channel_names_like_live_configure_does() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut mismatched = info();
        mismatched.channels = 6; // claims 6 channels but only reports 2 names
        let mut rec = Recorder::start(&root, &mismatched, "session-reconcile").unwrap();

        // The header and meta.json must agree on the reconciled count (2),
        // not the claimed one (6) — so a 2-value sample is accepted...
        rec.write_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, 2.0] }).unwrap();
        // ...and a sample sized to the claimed-but-unreconciled count is not.
        let err = rec
            .write_raw(&RawSample { timestamp: 2, marker: 0, data: vec![1.0; 6] })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(rec);

        let dir = root.join("session-reconcile");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().next().unwrap(), "timestamp,CP3,C3");

        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["channels"], 2);
        assert_eq!(meta["channelNames"], serde_json::json!(["CP3", "C3"]));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn start_refuses_to_clobber_an_existing_session() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut first = Recorder::start(&root, &info(), "session-noclobber").unwrap();
        first.write_raw(&RawSample { timestamp: 10, marker: 0, data: vec![1.5, -2.5] }).unwrap();
        drop(first);

        // A second `start` for the same root/name — e.g. a same-second
        // stop/start double-tap — must fail rather than truncate the first
        // session's files.
        let err = Recorder::start(&root, &info(), "session-noclobber").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        let dir = root.join("session-noclobber");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().nth(1).unwrap(), "10,1.5,-2.5", "first session's data must survive");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn start_rejects_a_session_name_that_could_escape_root() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        for bad in ["..", "../elsewhere", "nested/path", ""] {
            let err = Recorder::start(&root, &info(), bad).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "name {bad:?} should be rejected");
        }
    }

    #[test]
    fn start_caps_channels_at_max_channels_like_live_configure_does() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut huge = info();
        huge.channels = 1000;
        huge.channel_names = (0..1000).map(|i| format!("CH{i}")).collect();
        let mut rec = Recorder::start(&root, &huge, "session-cap").unwrap();

        // Reconciliation must stop at MAX_CHANNELS (64), the same bound
        // `Live::configure` applies — not at `channel_names.len()` (1000),
        // which would produce a header wider than `Live`'s rings ever emit.
        rec.write_raw(&RawSample { timestamp: 1, marker: 0, data: vec![0.0; 64] }).unwrap();
        let err = rec
            .write_raw(&RawSample { timestamp: 2, marker: 0, data: vec![0.0; 1000] })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(rec);

        let dir = root.join("session-cap");
        let raw = std::fs::read_to_string(dir.join("raw.csv")).unwrap();
        assert_eq!(raw.lines().next().unwrap().matches(',').count(), 64);

        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["channels"], 64);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn start_rejects_a_device_info_with_zero_usable_channels() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));

        let mut empty_names = info();
        empty_names.channel_names = vec![];
        let err = Recorder::start(&root, &empty_names, "session-zero-a").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let mut zero_channels = info();
        zero_channels.channels = 0;
        let err = Recorder::start(&root, &zero_channels, "session-zero-b").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        // Neither attempt should have left a session directory behind.
        assert!(!root.join("session-zero-a").exists());
        assert!(!root.join("session-zero-b").exists());
    }

    #[test]
    fn first_raw_write_anchors_the_clocks_in_derived_jsonl() {
        let root = std::env::temp_dir().join(format!("crown-test-{}", std::process::id()));
        let mut rec = Recorder::start(&root, &info(), "session-anchor").unwrap();

        rec.write_raw(&RawSample { timestamp: 987_654, marker: 0, data: vec![1.0, 2.0] }).unwrap();
        rec.write_raw(&RawSample { timestamp: 987_655, marker: 0, data: vec![1.0, 2.0] }).unwrap();
        drop(rec);

        let dir = root.join("session-anchor");
        let derived = std::fs::read_to_string(dir.join("derived.jsonl")).unwrap();
        let lines: Vec<&str> = derived.lines().collect();
        assert_eq!(lines.len(), 1, "the anchor is written once, on the first raw sample only");
        assert!(lines[0].contains("\"stream\":\"clockAnchor\""));
        assert!(lines[0].contains("\"value\":987654"), "anchor must carry the device timestamp: {}", lines[0]);

        std::fs::remove_dir_all(&root).unwrap();
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
