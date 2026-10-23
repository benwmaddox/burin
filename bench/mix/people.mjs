// people.json for the mixed load: from the Rails-format database, everyone active who can sign in
// (their email, session token, the rooms they read), how active each is (a Zipf weight, in a
// random order), and how popular each room is (Zipf by rank; open rooms tend to rank higher).
//   node bench/mix/people.mjs <rails.sqlite3> <password> [seed] > people.json
import { DatabaseSync } from "node:sqlite";
process.removeAllListeners("warning");

const [path, password, seedArg] = process.argv.slice(2);
let seed = Math.imul((Number(seedArg ?? 7) ^ 0x9e3779b9) >>> 0, 0x85ebca6b) >>> 0;
const rnd = () => { seed = (seed + 0x6d2b79f5) >>> 0; let t = seed; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const shuffle = (xs) => { const a = xs.slice(); for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; } return a; };
const db = new DatabaseSync(path, { readOnly: true });

// rooms by popularity: open rooms first in a random order, then the rest; weight 1/rank
const rooms = db.prepare("SELECT id, type FROM rooms").all();
const ranked = [...shuffle(rooms.filter((r) => r.type === "Rooms::Open")), ...shuffle(rooms.filter((r) => r.type !== "Rooms::Open"))];
const roomWeight = Object.fromEntries(ranked.map((r, i) => [String(r.id), +(1 / (i + 1)).toFixed(5)]));

// people: everyone signs in with the bench password (gen-rails.mjs gives it to all); burin and
// crudspec use the session token, Rails, Rust and Flow sign in
const users = shuffle(db.prepare("SELECT u.id, u.email_address email, s.token FROM users u JOIN sessions s ON s.user_id = u.id WHERE u.status = 0 AND u.role != 2 GROUP BY u.id").all());
const people = users.map((u, i) => ({
  id: Number(u.id),
  email: u.email,
  password,
  token: u.token,
  weight: +(1 / Math.pow(i + 1, 0.8)).toFixed(5),
  rooms: db.prepare("SELECT room_id FROM memberships WHERE user_id = ? AND involvement != 'invisible'").all(u.id).map((r) => Number(r.room_id)),
})).filter((p) => p.rooms.length > 0);
console.log(JSON.stringify({ people, rooms: roomWeight }));
