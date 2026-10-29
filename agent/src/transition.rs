//! Network mode serialization. Order: transition -> WireGuard -> Clash.
//! Public backend mutations take this once; inner mutations must not reenter it.
use std::sync::{Mutex, MutexGuard};
static LOCK: Mutex<()> = Mutex::new(());
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap()
}
#[cfg(test)]
mod tests {
    #[test]
    fn transitions_are_exclusive() {
        let guard = super::lock();
        assert!(super::LOCK.try_lock().is_err());
        drop(guard);
        assert!(super::LOCK.try_lock().is_ok());
    }
}
