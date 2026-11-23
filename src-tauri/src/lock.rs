//! The optional app PIN: asked for every time Budget4 starts, before any budget is listed or
//! opened. It locks the app only — budget files stay in YNAB 4's unencrypted format.
//!
//! The PIN is stored as an Argon2id hash (PHC string) in config.json. A forgotten PIN is
//! reset by deleting the "pinHash" line from that file.

use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

pub const MIN_LEN: usize = 4;
pub const MAX_LEN: usize = 128;
/// Wrong tries allowed before each pause.
const FREE_TRIES: u32 = 5;

pub fn hash_pin(pin: &str) -> Result<String, String> {
    check_new_pin(pin)?;
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes()).map_err(|e| e.to_string())?;
    Argon2::default().hash_password(pin.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

pub fn verify_pin(pin: &str, stored: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|h| Argon2::default().verify_password(pin.as_bytes(), &h).is_ok())
}

pub fn check_new_pin(pin: &str) -> Result<(), String> {
    let n = pin.chars().count();
    if n < MIN_LEN {
        return Err(format!("Use at least {MIN_LEN} characters"));
    }
    if n > MAX_LEN {
        return Err(format!("Use at most {MAX_LEN} characters"));
    }
    Ok(())
}

/// Counts wrong PINs and slows down guessing: after every 5 wrong tries the next one has to
/// wait 30 s, doubling each time (capped at 15 minutes).
#[derive(Default)]
pub struct Throttle {
    failures: u32,
    wait_until: Option<Instant>,
}

impl Throttle {
    /// Err with the seconds left while a pause is running.
    pub fn check(&self) -> Result<(), String> {
        match self.wait_until.map(|t| t.saturating_duration_since(Instant::now())) {
            Some(left) if !left.is_zero() => Err(format!("Too many wrong tries. Try again in {} s.", left.as_secs() + 1)),
            _ => Ok(()),
        }
    }

    pub fn failed(&mut self) {
        self.failures += 1;
        if self.failures % FREE_TRIES == 0 {
            let rounds = self.failures / FREE_TRIES - 1;
            let secs = (30u64 << rounds.min(5)).min(900);
            self.wait_until = Some(Instant::now() + Duration::from_secs(secs));
        }
    }

    pub fn succeeded(&mut self) {
        *self = Throttle::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_pin("2468").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_pin("2468", &h));
        assert!(!verify_pin("2469", &h));
        assert!(!verify_pin("2468", "not a hash"));
        assert_ne!(h, hash_pin("2468").unwrap(), "salted");
        assert!(hash_pin("123").is_err());
    }

    #[test]
    fn throttle_pauses_after_five_wrong_tries() {
        let mut t = Throttle::default();
        for _ in 0..4 {
            t.failed();
            assert!(t.check().is_ok());
        }
        t.failed();
        assert!(t.check().is_err());
        t.succeeded();
        assert!(t.check().is_ok());
    }
}
