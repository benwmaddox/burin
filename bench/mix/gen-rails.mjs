// A larger Campfire for the mixed benchmark, in Rails' own format (every implementation loads it:
// Rails and the Rust port directly, Flow from the Rails database, crudspec through its importer,
// etch from crudspec's copy). It starts from the verification fixture and adds people, rooms
// (open, closed, direct), memberships, messages, boosts and a session for everyone.
// Deterministic for a seed.
//   node bench/mix/gen-rails.mjs <fixture production.sqlite3> <out.sqlite3> <bcrypt digest> [seed]
// Everyone added signs in as person<N>@mix.example with the bench password (its digest given).
import { DatabaseSync } from "node:sqlite";
import { copyFileSync } from "node:fs";
process.removeAllListeners("warning");

const [fixture, out, digest, seedArg] = process.argv.slice(2);
if (!fixture || !out || !digest) { console.error("usage: gen-rails.mjs <fixture> <out> <bcrypt digest> [seed]"); process.exit(64); }
let seed = Math.imul((Number(seedArg ?? 1) ^ 0x9e3779b9) >>> 0, 0x85ebca6b) >>> 0;
const rnd = () => { seed = (seed + 0x6d2b79f5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const int = (a, b) => a + Math.floor(rnd() * (b - a + 1));
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const shuffle = (xs) => { const a = xs.slice(); for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; } return a; };
const hex = (n) => Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(rnd() * 16)]).join("");

copyFileSync(fixture, out);
const db = new DatabaseSync(out);
db.exec("PRAGMA foreign_keys = OFF; BEGIN");

// times: the last 60 days, up to an hour ago (Rails' datetime(6) text)
const NOW = Date.parse("2026-10-20T12:00:00Z");
const ts = (ms) => new Date(ms).toISOString().replace("T", " ").replace("Z", "").slice(0, 26);

const FIRST = ["Ada", "Ben", "Cleo", "Dev", "Eli", "Fay", "Gus", "Hana", "Ivo", "Jo", "Kai", "Lena", "Max", "Nia", "Omar", "Pia", "Quin", "Rosa", "Sam", "Tess", "Uma", "Vik", "Wren", "Xia", "Yuri", "Zoe"];
const LAST = ["Park", "Diaz", "Okafor", "Novak", "Ito", "Silva", "Berg", "Khan", "Moreau", "Rossi", "Haas", "Lund", "Costa", "Weber", "Reyes", "Sato"];
const WORDS = ("the a we it this that ship deploy review fix test plan idea launch design copy page bug merge call meeting lunch today tomorrow later soon " +
  "great thanks sure agreed looks good nice question about with for on in by after before update draft notes customer support release " +
  "server client cache build metrics latency write read room thread pager rollout").split(" ");
const sentence = () => {
  const n = int(4, 28);
  const w = Array.from({ length: n }, () => pick(WORDS));
  w[0] = w[0][0].toUpperCase() + w[0].slice(1);
  return w.join(" ") + pick([".", ".", ".", "?", "!"]);
};

const one = (sql, ...a) => db.prepare(sql).get(...a);
const run = (sql, ...a) => db.prepare(sql).run(...a);
const maxId = (t) => Number(one(`SELECT max(id) m FROM ${t}`).m ?? 0);

// people: the fixture's, and 112 more (one in ten an administrator)
const existing = db.prepare("SELECT id FROM users WHERE status = 0 AND role != 2").all().map((r) => Number(r.id));
let uid = Math.max(maxId("users"), 900000000);
const added = [];
for (let i = 1; i <= 112; i++) {
  const id = ++uid, t = NOW - int(30, 400) * 86400000;
  run(`INSERT INTO users (id, bio, created_at, email_address, name, password_digest, role, status, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)`,
    id, rnd() < 0.4 ? pick(["Support", "Design", "Programmer", "Ops", "Sales", "Writer"]) : null, ts(t), `person${i}@mix.example`,
    `${pick(FIRST)} ${pick(LAST)}`, digest, rnd() < 0.1 ? 1 : 0, ts(t));
  added.push(id);
}
const people = [...existing, ...added];

// rooms: 20 open (everyone), 19 closed (some), 10 direct (two or three)
let rid = Math.max(maxId("rooms"), 900000000), mid = Math.max(maxId("memberships"), 900000000);
const rooms = [];
const member = (room, user, involvement, created) => run(
  `INSERT INTO memberships (id, connected_at, connections, created_at, involvement, room_id, unread_at, updated_at, user_id) VALUES (?, NULL, 0, ?, ?, ?, NULL, ?, ?)`,
  ++mid, ts(created), involvement, room, ts(created), user);
const NAMES = ["Ops", "Design", "Launch", "Support", "Sales", "Writers", "Mobile", "Infra", "Data", "Security", "Hiring", "Docs", "Billing", "Growth", "QA", "Platform", "Events", "Partners", "Research", "Random"];
for (let i = 0; i < 49; i++) {
  const type = i < 20 ? "Rooms::Open" : i < 39 ? "Rooms::Closed" : "Rooms::Direct";
  const created = NOW - int(60, 200) * 86400000, creator = pick(people);
  const name = type === "Rooms::Direct" ? null : `${pick(NAMES)} ${i + 1}`;
  const id = ++rid;
  run(`INSERT INTO rooms (id, created_at, creator_id, name, type, updated_at) VALUES (?, ?, ?, ?, ?, ?)`, id, ts(created), creator, name, type, ts(created));
  const members = type === "Rooms::Open" ? people : type === "Rooms::Closed" ? [creator, ...shuffle(people.filter((p) => p !== creator)).slice(0, int(8, 40))] : [creator, ...shuffle(people.filter((p) => p !== creator)).slice(0, int(1, 2))];
  for (const u of members) member(id, u, type === "Rooms::Direct" ? "everything" : "mentions", created);
  rooms.push({ id, type, members, created });
}

// messages, oldest first, and a few boosts
let msg = Math.max(maxId("messages"), 900000000), rt = Math.max(maxId("action_text_rich_texts"), 900000000), bst = Math.max(maxId("boosts"), 900000000);
let messages = 0, boosts = 0;
for (const r of rooms) {
  const n = r.type === "Rooms::Open" ? int(150, 600) : r.type === "Rooms::Closed" ? int(50, 300) : int(20, 150);
  let t = r.created;
  const step = (NOW - 3600000 - r.created) / n;
  for (let k = 0; k < n; k++) {
    t += step * (0.5 + rnd());
    const id = ++msg, creator = pick(r.members), at = ts(Math.min(t, NOW - 3600000));
    run(`INSERT INTO messages (id, client_message_id, created_at, creator_id, room_id, updated_at) VALUES (?, ?, ?, ?, ?, ?)`, id, `${hex(8)}-${hex(4)}-${hex(4)}-${hex(4)}-${hex(12)}`, at, creator, r.id, at);
    run(`INSERT INTO action_text_rich_texts (id, body, created_at, name, record_id, record_type, updated_at) VALUES (?, ?, ?, 'body', ?, 'Message', ?)`, ++rt, sentence(), at, id, at);
    messages++;
    if (rnd() < 0.08) {
      for (const b of shuffle(r.members).slice(0, int(1, 3))) {
        run(`INSERT INTO boosts (id, booster_id, content, created_at, message_id, updated_at) VALUES (?, ?, ?, ?, ?, ?)`, ++bst, b, pick(["👍", "🎉", "❤️", "😂", "🔥", "nice"]), at, id, at);
        boosts++;
      }
    }
  }
  run(`UPDATE rooms SET updated_at = ? WHERE id = ?`, ts(Math.min(t, NOW - 3600000)), r.id);
}

// everyone can sign in with the bench password (the fixture's people too: the same people sign
// in on every implementation)
run(`UPDATE users SET password_digest = ? WHERE status = 0 AND role != 2`, digest);

// a session for everyone active (the cookie etch and crudspec read; Rails and Rust sign in)
let sid = Math.max(maxId("sessions"), 900000000);
for (const u of db.prepare("SELECT id FROM users WHERE status = 0 AND role != 2").all()) {
  if (one("SELECT 1 FROM sessions WHERE user_id = ?", u.id)) continue;
  run(`INSERT INTO sessions (id, created_at, ip_address, last_active_at, token, updated_at, user_agent, user_id) VALUES (?, ?, '127.0.0.1', ?, ?, ?, 'mixload', ?)`,
    ++sid, ts(NOW), ts(NOW), `mix${hex(29)}`, ts(NOW), u.id);
}
db.exec("COMMIT");
const count = (t) => one(`SELECT count(*) n FROM ${t}`).n;
console.log(JSON.stringify({ users: count("users"), rooms: count("rooms"), memberships: count("memberships"), messages: count("messages"), boosts: count("boosts"), sessions: count("sessions"), added_messages: messages, added_boosts: boosts }));
