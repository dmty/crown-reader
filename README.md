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

The GUI opens Settings on first launch and stores the auth profile in the native keyring. `crown-cli` alone reads `NEUROSITY_EMAIL`, `NEUROSITY_PASSWORD`, and `NEUROSITY_DEVICE_ID` from the environment or a `.env` file:

```bash
cp .env.example .env   # then fill in NEUROSITY_EMAIL, NEUROSITY_PASSWORD, NEUROSITY_DEVICE_ID
```

`crown-cli` reads `.env` from the working directory or any parent. Variables already exported in your shell win, and `.env` is gitignored.

The minted BLE token remains separately cached in the keyring, so the app runs offline between mints. The headset accepts one connection at a time — disconnect the Neurosity app first.

## Running

```bash
cargo run -p crown-qt                        # the app
cargo run -p crown-core --bin crown-cli      # headless, add --raw for the EEG stream
cargo test -p crown-core                     # unit tests, no hardware needed
```

Recordings are written to `~/CrownSessions/<session>/` by the app's Record button.
