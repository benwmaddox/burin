//! Passwords: argon2id hashes in PHC text (from the RustCrypto `argon2` crate; nothing hand-made).
//! Verified on the request's thread, never on the writer: a check takes tens of milliseconds.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use std::sync::OnceLock;

/// A new hash of `password`, with a random salt.
pub fn hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).expect("random");
    let salt = SaltString::encode_b64(&salt).expect("salt");
    Argon2::default().hash_password(password.as_bytes(), &salt).expect("hash").to_string()
}

/// Whether `password` matches `hash`. With no hash (no such user), a hash is checked anyway, so
/// the answer takes as long: how fast it comes doesn't tell which accounts exist.
pub fn verify(hash: Option<&str>, password: &str) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    match hash.and_then(|h| PasswordHash::new(h).ok()) {
        Some(h) => Argon2::default().verify_password(password.as_bytes(), &h).is_ok(),
        None => {
            let d = DUMMY.get_or_init(|| self::hash("not anyone's password"));
            let _ = Argon2::default().verify_password(password.as_bytes(), &PasswordHash::new(d).expect("dummy"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_hash_verifies_only_its_password() {
        let h = super::hash("correct horse");
        assert!(h.starts_with("$argon2id$"));
        assert!(super::verify(Some(&h), "correct horse"));
        assert!(!super::verify(Some(&h), "wrong"));
        assert!(!super::verify(None, "correct horse"));
        assert!(!super::verify(Some("not a hash"), "x"));
    }
}
