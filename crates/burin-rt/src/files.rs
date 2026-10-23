//! An app's static files (`--assets DIR`): stylesheets, scripts, images, sounds. Read once at
//! start, text compressed once; served at `/assets/<name>` (pages link them with `?v=<hash>`, so
//! a browser keeps them for a year and asks again only when the file changed).

use std::collections::HashMap;
use std::io::Write;

pub struct File {
    pub ctype: &'static str,
    pub raw: Vec<u8>,
    /// gzip, for text (absent: sent as it is)
    pub gz: Option<Vec<u8>>,
}

pub struct Files(HashMap<String, File>);

fn ctype(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "mp3" => "audio/mpeg",
        "woff2" => "font/woff2",
        "html" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl Files {
    pub fn empty() -> Files {
        Files(HashMap::new())
    }

    /// Every file under `dir`, by file name (the first found wins, as the pages' links do).
    pub fn load(dir: &std::path::Path) -> Files {
        let mut m = HashMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let (Some(name), Ok(raw)) = (p.file_name().and_then(|n| n.to_str()), std::fs::read(&p)) else { continue };
                if m.contains_key(name) {
                    continue;
                }
                let ct = ctype(name);
                let text = ct.starts_with("text/") || ct == "application/json" || ct == "image/svg+xml";
                let gz = text.then(|| {
                    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                    e.write_all(&raw).expect("compress");
                    e.finish().expect("compress")
                });
                m.insert(name.to_string(), File { ctype: ct, raw, gz });
            }
        }
        Files(m)
    }

    /// The file a path names (`/assets/<name>`, any query ignored).
    pub fn get(&self, path: &[u8]) -> Option<&File> {
        let name = path.strip_prefix(b"/assets/")?;
        let name = std::str::from_utf8(name).ok()?;
        self.0.get(name)
    }
}
