// The Rust port's optimized variant, checked against itself without caches: random writes go to
// the cached server (A); after each, pages are fetched from it and from the same build with its
// caches off (B), on the same database, as the same people, and must be the same bytes. A stale
// page (a write whose version bump was missed) shows as a difference.
//   node rustopt-verify.mjs <A url> <B url> <cookies.json> <people.json> <db> <steps> <seed>
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";

const [A, B, cookiesPath, peoplePath, dbPath, stepsArg, seedArg] = process.argv.slice(2);
const cookies = JSON.parse(readFileSync(cookiesPath, "utf8"));
const people = JSON.parse(readFileSync(peoplePath, "utf8")).people.filter((p) => cookies[p.id]);
const db = new DatabaseSync(dbPath, { readOnly: true });
const steps = Number(stepsArg || 200);
let seed = BigInt(seedArg || 1) * 2n + 1n;
const rand = () => {
  seed ^= seed << 13n; seed &= (1n << 64n) - 1n;
  seed ^= seed >> 7n;
  seed ^= seed << 17n; seed &= (1n << 64n) - 1n;
  return Number(seed % 1000000n) / 1000000;
};
const pick = (xs) => xs[Math.floor(rand() * xs.length)];

const headers = (p, extra = {}) => ({ cookie: cookies[p.id].cookie, "sec-fetch-site": "same-origin", ...extra });
async function write(p, method, path, form) {
  const body = new URLSearchParams({ _method: method, ...form }).toString();
  const r = await fetch(A + path, { method: "POST", redirect: "manual", body,
    headers: headers(p, { "content-type": "application/x-www-form-urlencoded", accept: "text/vnd.turbo-stream.html, text/html" }) });
  await r.arrayBuffer();
  return r.status;
}
async function page(base, p, path, frame) {
  const r = await fetch(base + path, { redirect: "manual", headers: headers(p, frame ? { "turbo-frame": frame } : {}) });
  return { status: r.status, body: await r.text() };
}

const roomsOf = (p) => p.rooms;
const one = (sql, ...args) => db.prepare(sql).get(...args);
const writes = {
  post: async (p) => { const r = pick(roomsOf(p)); return [r, await write(p, "post", `/rooms/${r}/messages`, { "message[body]": `verify ${rand()}`, "message[client_message_id]": crypto.randomUUID() })]; },
  edit: async (p) => {
    const m = one(`SELECT id, room_id FROM messages WHERE creator_id = ? ORDER BY id DESC LIMIT 1`, p.id);
    if (!m) return [null, 0];
    return [m.room_id, await write(p, "patch", `/rooms/${m.room_id}/messages/${m.id}`, { "message[body]": `edited ${rand()}` })];
  },
  delete: async (p) => {
    const m = one(`SELECT id, room_id FROM messages WHERE creator_id = ? ORDER BY id DESC LIMIT 1`, p.id);
    if (!m) return [null, 0];
    return [m.room_id, await write(p, "delete", `/rooms/${m.room_id}/messages/${m.id}`, {})];
  },
  boost: async (p) => {
    const r = pick(roomsOf(p));
    const m = one(`SELECT id FROM messages WHERE room_id = ? ORDER BY created_at DESC LIMIT 1 OFFSET ?`, r, Math.floor(rand() * 5));
    if (!m) return [r, 0];
    return [r, await write(p, "post", `/messages/${m.id}/boosts`, { "boost[content]": pick(["👍", "🎉", "ok"]) })];
  },
  unboost: async (p) => {
    const b = one(`SELECT boosts.id, boosts.message_id, messages.room_id FROM boosts JOIN messages ON messages.id = boosts.message_id WHERE booster_id = ? ORDER BY boosts.id DESC LIMIT 1`, p.id);
    if (!b) return [null, 0];
    return [b.room_id, await write(p, "delete", `/messages/${b.message_id}/boosts/${b.id}`, {})];
  },
  rename: async (p) => {
    const r = one(`SELECT id FROM rooms WHERE creator_id = ? AND type = 'Rooms::Open' LIMIT 1`, p.id);
    if (!r) return [null, 0];
    return [r.id, await write(p, "patch", `/rooms/opens/${r.id}`, { "room[name]": `Room ${Math.floor(rand() * 1000)}` })];
  },
  profile: async (p) => [null, await write(p, "patch", `/users/me/profile`, { "user[name]": `${p.name ?? "Person"} ${Math.floor(rand() * 100)}` })],
  involvement: async (p) => { const r = pick(roomsOf(p)); return [r, await write(p, "patch", `/rooms/${r}/involvement`, { involvement: pick(["everything", "mentions", "nothing"]) })]; },
};
const weights = [["post", 6], ["edit", 2], ["delete", 1], ["boost", 3], ["unboost", 1], ["rename", 1], ["profile", 1], ["involvement", 1]];
const total = weights.reduce((s, [, w]) => s + w, 0);
const chooseWrite = () => { let x = rand() * total; for (const [k, w] of weights) { if ((x -= w) < 0) return k; } return "post"; };

let compared = 0, differed = 0;
const done = {};
// each server writes its own address into some pages
// (also inside the join link's QR code address, which is the link in base64)
const normalize = (s) =>
  s.replaceAll(new URL(B).host, new URL(A).host).replace(/\/qr_code\/([A-Za-z0-9_-]+=*)/g, (_, code) =>
    "/qr_code/" + Buffer.from(Buffer.from(code, "base64url").toString("utf8").replaceAll(new URL(B).host, new URL(A).host)).toString("base64url"));
async function compare(p, path, frame) {
  // the cached server first: a stale entry is what's being looked for
  const a = await page(A, p, path, frame);
  const b = await page(B, p, path, frame);
  compared++;
  const [na, nb] = [normalize(a.body), normalize(b.body)];
  if (a.status !== b.status || na !== nb) {
    differed++;
    if (differed <= 5) {
      let i = 0;
      while (i < na.length && na[i] === nb[i]) i++;
      console.log(`DIFFERS ${path}${frame ? " (frame)" : ""} as ${p.id}: status ${a.status}/${b.status} at ${i}\n  cached:   ${JSON.stringify(na.slice(Math.max(0, i - 200), i + 200))}\n  uncached: ${JSON.stringify(nb.slice(Math.max(0, i - 200), i + 200))}`);
    }
  }
}
// everyone once first, on both servers (a first request may refresh the session, a write)
for (const p of people) {
  await page(A, p, `/rooms/${p.rooms[0]}`);
  await page(B, p, `/rooms/${p.rooms[0]}`);
}
for (let step = 0; step < steps; step++) {
  const who = pick(people);
  const kind = chooseWrite();
  const [room, status] = await writes[kind](who);
  done[`${kind} ${status}`] = (done[`${kind} ${status}`] || 0) + 1;
  // the room's page for a few of its members, someone's sidebar, and a page elsewhere
  const members = room ? db.prepare(`SELECT user_id FROM memberships WHERE room_id = ?`).all(room).map((m) => people.find((p) => p.id === m.user_id)).filter(Boolean) : [];
  for (let k = 0; k < 3 && members.length; k++) await compare(pick(members), `/rooms/${room}`);
  const someone = room && members.length ? pick(members) : who;
  await compare(someone, `/users/me/sidebar`, "user_sidebar");
  const other = pick(people);
  await compare(other, `/rooms/${pick(roomsOf(other))}`);
}
console.log(JSON.stringify({ steps, compared, differed, writes: done }));
console.log(differed === 0 ? "PASS" : "FAIL");
process.exitCode = differed === 0 ? 0 : 1;
