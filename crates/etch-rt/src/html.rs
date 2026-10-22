//! Escaping and text formatting, done once when a fragment is built (never per request).

/// Writes s escaped for HTML text and attribute values: & < > " '.
pub fn esc(b: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &c) in bytes.iter().enumerate() {
        let r: &[u8] = match c {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&#39;",
            _ => continue,
        };
        b.extend_from_slice(&bytes[start..i]);
        b.extend_from_slice(r);
        start = i + 1;
    }
    b.extend_from_slice(&bytes[start..]);
}

pub fn esc_string(s: &str) -> String {
    let mut b = Vec::with_capacity(s.len());
    esc(&mut b, s);
    String::from_utf8(b).unwrap()
}

/// s as a JSON (and JavaScript) string literal, quotes included, as Go's encoding/json writes it
/// with HTML escaping off: ", \ and control characters escaped; < > & as they are.
pub fn json_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Links in already-escaped text become anchors, and newlines `<br>` (crudspec's autolink).
pub fn autolink(escaped: &str) -> String {
    let mut o = String::with_capacity(escaped.len());
    let mut rest = escaped;
    loop {
        let at = [rest.find("http://"), rest.find("https://")].into_iter().flatten().min();
        match at {
            None => break,
            Some(i) => {
                o.push_str(&rest[..i]);
                let tail = &rest[i..];
                let scheme = if tail.starts_with("https://") { 8 } else { 7 };
                let end = tail[scheme..].find(|c: char| c.is_whitespace() || c == '<').map(|e| e + scheme).unwrap_or(tail.len());
                if end == scheme {
                    // "http://" alone isn't a link
                    o.push_str(&tail[..scheme]);
                    rest = &tail[scheme..];
                    continue;
                }
                let url = &tail[..end];
                o.push_str("<a href=\"");
                o.push_str(url);
                o.push_str("\" target=\"_blank\" rel=\"noopener\">");
                o.push_str(url);
                o.push_str("</a>");
                rest = &tail[end..];
            }
        }
    }
    o.push_str(rest);
    o.replace('\n', "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(esc_string(r#"I'm <b>"x"</b> & y"#), "I&#39;m &lt;b&gt;&quot;x&quot;&lt;/b&gt; &amp; y");
        assert_eq!(json_string("> a\n\n"), "\"> a\\n\\n\"");
        assert_eq!(autolink("see https://x.com/a?b=1 now\nok"), "see <a href=\"https://x.com/a?b=1\" target=\"_blank\" rel=\"noopener\">https://x.com/a?b=1</a> now<br>ok");
    }
}
