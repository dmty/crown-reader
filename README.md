# Crown Reader

Desktop app that connects to a [Neurosity Crown](https://neurosity.co) over Bluetooth LE, displays live EEG and the metrics the headset computes, and records sessions to disk. Rust throughout.

## Layout

| Crate | Contents |
|---|---|
| `crown-core` | Auth, BLE transport, stream decoding, recording. No UI dependency. Also builds `crown-cli`, a headless diagnostic client. |
| `crown-qt` | The app: [CXX-Qt](https://github.com/kdab/cxx-qt) QObjects with a QML front end. |

Core knows nothing about Qt, so the UI layer is replaceable.

## Requirements

- Rust (stable)
- Qt 6 on `PATH` — `crown-qt` only; `crown-core` builds without it
- A Neurosity account and a paired headset

The GUI opens Settings when no auth profile exists in the keyring, and stores
the profile there. After email and password, **Load devices** lists claimed
headsets; pick one instead of pasting a device ID. `crown-cli` still reads
`NEUROSITY_EMAIL`, `NEUROSITY_PASSWORD`, and `NEUROSITY_DEVICE_ID` from the
environment or a `.env` file (working directory or any parent; exported shell
variables win; `.env` is gitignored):

```bash
cp .env.example .env   # then fill in NEUROSITY_EMAIL, NEUROSITY_PASSWORD, NEUROSITY_DEVICE_ID
```

The minted BLE token is cached separately in the keyring, so the app runs offline between mints. The headset accepts one connection at a time — disconnect the Neurosity app first.

## Running

```bash
cargo run -p crown-qt                        # the app
cargo run -p crown-core --bin crown-cli      # headless, add --raw for the EEG stream
cargo test -p crown-core                     # unit tests, no hardware needed
```

Recordings are written to `~/CrownSessions/<session>/` by the app's Record button.
