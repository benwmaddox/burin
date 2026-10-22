// Random actions by random people, each followed by page reads; the server runs with --verify
// (every fast page compared with a fresh build) and the scrubber on. Reproducible from its seed.
//   node scripts/fuzz.mjs <base url> <app.sqlite> <steps> <seed>
// Prints the outcomes seen, then the server's stats.
import { DatabaseSync } from "node:sqlite";
process.removeAllListeners("warning");
process.on("warning", () => {});

const [base, dbPath, stepsArg, seedArg] = process.argv.slice(2);
const steps = Number(stepsArg ?? 400);
// the seed mixed first (nearby seeds would otherwise give the same stream, shifted)
let seed = Math.imul((Number(seedArg ?? 1) ^ 0x9e3779b9) >>> 0, 0x85ebca6b) >>> 0;
const rnd = () => {
  // mulberry32
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const db = new DatabaseSync(dbPath, { readOnly: true });
const all = (sql, ...a) => db.prepare(sql).all(...a);
const one = (sql, ...a) => db.prepare(sql).get(...a);

const people = all("select s.token, s.user from Session s join User u on u.id = s.user where u.status = 'active'");
const outcomes = {};
const note = (k) => (outcomes[k] = (outcomes[k] ?? 0) + 1);

async function act(who, name, fields) {
  const body = new URLSearchParams();
  for (const [k, v] of fields) body.append(k, String(v));
  const r = await fetch(`${base}/_do/${name}`, { method: "POST", redirect: "manual", headers: { Cookie: `session_token=${who.token}` }, body });
  note(`${name}: ${r.headers.get("x-etch-outcome") ?? r.status}`);
}
async function read(who, room) {
  const r = await fetch(`${base}/rooms/${room}`, { redirect: "manual", headers: { Cookie: `session_token=${who.token}`, ...(rnd() < 0.5 ? { "Accept-Encoding": "gzip" } : {}) } });
  await r.arrayBuffer();
}

const roomsOf = (u) => all("select room from Membership where user = ?", u).map((r) => r.room);
const anyRoom = () => one("select id from Room order by random() limit 1")?.id;
const usersSome = () => all("select id from User where status = 'active' order by random() limit ?", 1 + Math.floor(rnd() * 3)).map((r) => r.id);

const actions = [
  [30, async (w) => act(w, "post_message", [["room", pick(roomsOf(w.user)) ?? anyRoom()], ["body", `fuzz ${Math.floor(rnd() * 1e6)} <i>x</i> & "q"`]])],
  [8, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by random() limit 1", w.user);
    if (m) await act(w, "edit_message", [["__arg_message", `#${m.id}`], ["body", `edited ${Math.floor(rnd() * 1e6)}`]]);
  }],
  [4, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by random() limit 1", w.user);
    if (m) await act(w, "delete_message", [["__arg_message", `#${m.id}`]]);
  }],
  [15, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by created desc limit 1 offset ?", w.user, Math.floor(rnd() * 45));
    if (m) await act(w, "boost", [["__arg_message", `#${m.id}`], ["__arg_content", pick(["👍", "🎉", "ok", "🔥"])]]);
  }],
  [5, async (w) => {
    const b = one("select id from Boost order by random() limit 1");
    if (b) await act(w, "unboost", [["__arg_boost", `#${b.id}`]]);
  }],
  [2, async (w) => act(w, "create_open_room", [["name", `open ${Math.floor(rnd() * 1e4)}`]])],
  [3, async (w) => act(w, "create_closed_room", [["name", `closed ${Math.floor(rnd() * 1e4)}`], ...usersSome().map((u) => ["users", u])])],
  [4, async (w) => act(w, "create_direct_room", usersSome().map((u) => ["__arg_users", `@${u}`]))],
  [4, async (w) => {
    const r = pick(roomsOf(w.user));
    if (!r) return;
    if (rnd() < 0.5) await act(w, "update_open_room", [["room", r], ["name", `reopened ${Math.floor(rnd() * 1e4)}`]]);
    else await act(w, "update_closed_room", [["room", r], ["name", `reclosed ${Math.floor(rnd() * 1e4)}`], ...usersSome().map((u) => ["users", u])]);
  }],
  [1, async (w) => {
    const r = pick(roomsOf(w.user));
    if (r && r !== 486777696) await act(w, "delete_room", [["room", r]]);
  }],
  [5, async (w) => act(w, "set_involvement", [["room", pick(roomsOf(w.user)) ?? anyRoom()], ["involvement", pick(["everything", "mentions", "nothing", "invisible"])]])],
];
const total = actions.reduce((s, [w]) => s + w, 0);

for (let i = 0; i < steps; i++) {
  const who = pick(people);
  let x = rnd() * total;
  for (const [w, f] of actions) {
    if ((x -= w) < 0) {
      await f(who);
      break;
    }
  }
  // reads: the actor's rooms and someone else's
  for (let k = 0; k < 3; k++) {
    const r = pick(people);
    const room = pick(roomsOf(r.user));
    if (room) await read(r, room);
  }
}
// a sweep: every room for every person in it, twice (the first warms it)
for (const p of people) for (const room of roomsOf(p.user)) await read(p, room);
await new Promise((r) => setTimeout(r, 400));
for (const p of people) for (const room of roomsOf(p.user)) await read(p, room);
console.log(JSON.stringify(Object.fromEntries(Object.entries(outcomes).sort())));
