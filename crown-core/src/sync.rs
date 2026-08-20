use std::sync::{Mutex, MutexGuard};

/// Locks `m`, recovering from a poisoned lock instead of panicking.
///
/// A poisoned `Mutex` only means some *earlier* holder of the lock panicked
/// while holding it — the data itself is still structurally valid, just
/// possibly mid-update. Every lock in this codebase guards plain data (no
/// invariant that spans multiple fields and can be left half-written by a
/// panic), so recovering and carrying on is strictly better than a second,
/// unrelated panic here — especially for the `live` lock, which is taken
/// from both the GUI thread (every tick) and the BLE thread (up to 256
/// Hz): an unwind out of either across the cxx-qt FFI boundary is not a
/// clean crash.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
