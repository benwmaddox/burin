//! Signed tokens (HMAC-SHA256 from the `hmac` and `sha2` crates; no hand-written crypto).
//! A token is `base64url(body) "." base64url(hmac(secret, base64url(body)))`, as crudspec's.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub struct Signer {
    secret: Vec<u8>,
}

impl Signer {
    pub fn new(secret: &[u8]) -> Self {
        Signer { secret: secret.to_vec() }
    }

    pub fn mac(&self, body: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).expect("any key length");
        m.update(body.as_bytes());
        B64.encode(m.finalize().into_bytes())
    }

    /// The token for this JSON.
    pub fn token(&self, json: &str) -> String {
        let body = B64.encode(json.as_bytes());
        let mac = self.mac(&body);
        body + "." + &mac
    }

    /// The token for this JSON, written into `out` (no allocation once `out` has room: a page
    /// rendered per request signs its forms' tokens this way).
    pub fn token_into(&self, json: &str, out: &mut Vec<u8>) {
        let start = out.len();
        let n = base64::encoded_len(json.len(), false).expect("a token's length");
        out.resize(start + n, 0);
        B64.encode_slice(json.as_bytes(), &mut out[start..]).expect("room for the body");
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).expect("any key length");
        m.update(&out[start..]);
        let mut mac = [0u8; 64];
        let k = B64.encode_slice(m.finalize().into_bytes(), &mut mac).expect("room for the MAC");
        out.push(b'.');
        out.extend_from_slice(&mac[..k]);
    }

    /// The JSON a token carries, if its signature holds.
    pub fn verify(&self, token: &str) -> Option<Vec<u8>> {
        let (body, mac) = token.split_once('.')?;
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).ok()?;
        m.update(body.as_bytes());
        let want = B64.decode(mac).ok()?;
        m.verify_slice(&want).ok()?;
        B64.decode(body).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Signer::new(b"k");
        let t = s.token(r#"{"e":[],"p":"/rooms/1","v":[]}"#);
        assert!(t.starts_with("eyJlIjpbXSwicCI6Ii9yb29tcy8xIiwidiI6W119."));
        assert_eq!(s.verify(&t).unwrap(), br#"{"e":[],"p":"/rooms/1","v":[]}"#);
        assert!(s.verify(&(t.clone() + "x")).is_none());
    }
}
