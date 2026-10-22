//! The data directory's keys, made on first start as crudspec makes them: `secret` (32 random
//! bytes, as hex text, used as the HMAC key as it is) and `vapid.json` (a P-256 key pair for web
//! push, from the p256 crate).

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use std::path::Path;

pub fn secret(data: &Path) -> Vec<u8> {
    let p = data.join("secret");
    if let Ok(s) = std::fs::read(&p) {
        return s;
    }
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).expect("random");
    let hex: String = k.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&p, &hex).expect("write the secret");
    hex.into_bytes()
}

pub fn vapid_public(data: &Path) -> String {
    let p = data.join("vapid.json");
    if let Ok(v) = std::fs::read_to_string(&p) {
        if let Some(k) = v.split("\"publicKey\":\"").nth(1).and_then(|r| r.split('"').next()) {
            return k.to_string();
        }
    }
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let public = B64.encode(key.public_key().to_sec1_bytes());
    let private = B64.encode(key.to_bytes());
    std::fs::write(&p, format!(r#"{{"publicKey":"{public}","privateKey":"{private}"}}"#)).expect("write vapid.json");
    public
}
