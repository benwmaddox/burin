// Rails signed cookies for existing sessions (the mixed benchmark signs everyone in this way:
// Campfire limits sign-ins to 10 per 3 minutes from one address). Standard primitives from
// Node's crypto (PBKDF2, HMAC); the format is Rails' MessageVerifier: base64(JSON) "--" HMAC.
//
//   node rails-cookie.mjs probe <reference.env> <cookies.json> <rails.sqlite3>
//     finds which key derivation matches a cookie the server issued (prints only which one)
//   node rails-cookie.mjs make <reference.env> <people.json> <rails.sqlite3> <digest> <iters> <keylen> <hmac>
//     prints {"<user id>": {"cookie": "session_token=…"}} for everyone's session
import { readFileSync } from "node:fs";
import { pbkdf2Sync, createHmac, timingSafeEqual } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
process.removeAllListeners("warning");

const [mode, envPath, a, dbPath, ...rest] = process.argv.slice(2);
const secret = readFileSync(envPath, "utf8").split("\n").find((l) => l.startsWith("SECRET_KEY_BASE="))?.slice(16).trim();
if (!secret) throw new Error("no SECRET_KEY_BASE");
const sign = (data, key, hmac) => createHmac(hmac, key).update(data).digest("hex");

if (mode === "probe") {
  const cookies = JSON.parse(readFileSync(a, "utf8"));
  const raw = decodeURIComponent(Object.values(cookies)[0].cookie.split(";")[0].split("=").slice(1).join("="));
  const [data, sig] = raw.split("--");
  const payload = JSON.parse(Buffer.from(data, "base64").toString());
  console.log("payload keys:", JSON.stringify(Object.keys(payload._rails ?? payload)), "purpose:", payload._rails?.pur ?? null);
  const inner = Buffer.from(payload._rails.message, "base64").toString();
  console.log("message shape:", inner.replace(/[A-Za-z0-9]+/g, (m) => `<${m.length}>`), "exp:", payload._rails.exp);
  for (const digest of ["sha1", "sha256"]) for (const iters of [1000, 65536]) for (const len of [64, 32]) for (const hmac of ["sha1", "sha256"]) {
    const key = pbkdf2Sync(secret, "signed cookie", iters, len, digest);
    const s = sign(data, key, hmac);
    if (s.length === sig.length && timingSafeEqual(Buffer.from(s), Buffer.from(sig))) console.log("match:", digest, iters, len, hmac);
  }
} else if (mode === "make") {
  const [digest, iters, len, hmac] = rest;
  const key = pbkdf2Sync(secret, "signed cookie", Number(iters), Number(len), digest);
  const people = JSON.parse(readFileSync(a, "utf8")).people;
  const db = new DatabaseSync(dbPath, { readOnly: true });
  const out = {};
  for (const p of people) {
    const s = db.prepare("SELECT token FROM sessions WHERE user_id = ? ORDER BY id DESC LIMIT 1").get(p.id);
    if (!s) continue;
    // Rails 7's envelope: the value (JSON) in base64, an expiry (a permanent cookie: 20 years), the purpose
    const message = Buffer.from(JSON.stringify(s.token)).toString("base64");
    const data = Buffer.from(JSON.stringify({ _rails: { message, exp: "2046-10-22T00:00:00.000Z", pur: "cookie.session_token" } })).toString("base64");
    out[p.id] = { cookie: `session_token=${encodeURIComponent(`${data}--${sign(data, key, hmac)}`)}` };
  }
  console.log(JSON.stringify(out));
}
