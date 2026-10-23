// The live path checked like the pages are: watchers hold rooms' streams while random people
// post, edit, delete, boost and unboost; each watcher applies what its stream sends to its copy
// of the room's list, as Turbo does (append, prepend, replace, remove, refresh). After the
// writes, every watcher's list must be the same items, in the same order and the same bytes, as
// a fresh page's. A change the writer didn't send (or sent wrong) shows as a difference.
//   node scripts/live-fuzz.mjs <base url> <app.sqlite> <steps> <seed> [watchers]
import { DatabaseSync } from "node:sqlite";
process.removeAllListeners("warning");
process.on("warning", () => {});

const [base, dbPath, stepsArg, seedArg, watchersArg] = process.argv.slice(2);
const steps = Number(stepsArg ?? 300);
let seed = Math.imul((Number(seedArg ?? 1) ^ 0x9e3779b9) >>> 0, 0x85ebca6b) >>> 0;
const rnd = () => {
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

// a page's live list: its items, each (id, bytes), in order
const START = 'id="live_room_messages">';
const END = '</div><button type="button" class="message-area__return-to-latest';
function listOf(page) {
  const a = page.indexOf(START);
  const b = page.indexOf(END, a);
  if (a < 0 || b < 0) return null;
  const body = page.slice(a + START.length, b);
  const items = [];
  const re = /<div id="(message_[^"]+)"/g;
  const starts = [];
  let m;
  while ((m = re.exec(body))) starts.push([m.index, m[1]]);
  starts.forEach(([at, id], i) => items.push([id, body.slice(at, i + 1 < starts.length ? starts[i + 1][0] : body.length)]));
  return items;
}

async function page(who, room) {
  const r = await fetch(`${base}/rooms/${room}`, { redirect: "manual", headers: { Cookie: `session_token=${who.token}` } });
  return r.status === 200 ? await r.text() : null;
}

// a watcher: its list, kept up to date from its stream
async function watch(who, room) {
  const w = { who, room, items: [], events: 0, refreshes: 0, ctrl: new AbortController() };
  const load = async () => {
    const p = await page(who, room);
    const src = p && p.match(/<turbo-stream-source src="([^"]*)"/)?.[1]?.replaceAll("&amp;", "&");
    w.items = (p && listOf(p)) || [];
    return src;
  };
  const src = await load();
  if (!src) return null;
  const r = await fetch(base + src, { headers: { Cookie: `session_token=${who.token}` }, signal: w.ctrl.signal });
  if (r.status !== 200) return null;
  w.done = (async () => {
    const dec = new TextDecoder();
    let buf = "";
    try {
      for await (const chunk of r.body) {
        buf += dec.decode(chunk, { stream: true });
        let at;
        while ((at = buf.indexOf("\n\n")) >= 0) {
          const ev = buf.slice(0, at);
          buf = buf.slice(at + 2);
          const data = ev.split("\n").filter((l) => l.startsWith("data: ")).map((l) => l.slice(6)).join("\n");
          if (data) await apply(w, data, load);
        }
      }
    } catch (e) {
      if (e.name !== "AbortError") w.error = String(e);
    }
  })();
  return w;
}

async function apply(w, stream, load) {
  w.events++;
  const m = stream.match(/^<turbo-stream action="([a-z]+)"(?: target="([^"]*)")?>(?:<template>([\s\S]*)<\/template>)?<\/turbo-stream>$/);
  if (!m) throw new Error(`not a turbo stream: ${stream.slice(0, 120)}`);
  const [, action, target, content] = m;
  const id = content?.match(/^<div id="([^"]+)"/)?.[1];
  const at = (k) => w.items.findIndex(([i]) => i === k);
  switch (action) {
    case "append":
    case "prepend": {
      // as Turbo does: an element with the same id goes first
      if (at(id) >= 0) w.items.splice(at(id), 1);
      if (action === "append") w.items.push([id, content]);
      else w.items.unshift([id, content]);
      break;
    }
    case "replace":
      if (at(target) >= 0) w.items[at(target)] = [target, content];
      break;
    case "remove":
      if (at(target) >= 0) w.items.splice(at(target), 1);
      break;
    case "refresh":
      w.refreshes++;
      await load();
      break;
    default:
      throw new Error(`an unknown action ${action}`);
  }
}

async function act(who, name, fields) {
  const body = new URLSearchParams();
  for (const [k, v] of fields) body.append(k, String(v));
  const r = await fetch(`${base}/_do/${name}`, { method: "POST", redirect: "manual", headers: { Cookie: `session_token=${who.token}` }, body });
  await r.arrayBuffer();
}
const roomsOf = (u) => all("select room from Membership where user = ?", u).map((r) => r.room);
const inRoom = (room) => people.filter((p) => roomsOf(p.user).includes(room));

const actions = [
  [30, async (w) => act(w, "post_message", [["room", pick(roomsOf(w.user))], ["body", `live fuzz ${Math.floor(rnd() * 1e6)} <i>x</i> & "q"`]])],
  [10, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by created desc limit 1 offset ?", w.user, Math.floor(rnd() * 40));
    if (m) await act(w, "edit_message", [["__arg_message", `#${m.id}`], ["body", `edited ${Math.floor(rnd() * 1e6)}`]]);
  }],
  [5, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by created desc limit 1 offset ?", w.user, Math.floor(rnd() * 45));
    if (m) await act(w, "delete_message", [["__arg_message", `#${m.id}`]]);
  }],
  [12, async (w) => {
    const m = one("select id from Message where room in (select room from Membership where user = ?) order by created desc limit 1 offset ?", w.user, Math.floor(rnd() * 45));
    if (m) await act(w, "boost", [["__arg_message", `#${m.id}`], ["__arg_content", pick(["👍", "🎉", "ok"])]]);
  }],
  [4, async (w) => {
    const b = one("select id from Boost where booster = ? order by random() limit 1", w.user);
    if (b) await act(w, "unboost", [["__arg_boost", `#${b.id}`]]);
  }],
];
const total = actions.reduce((s, [n]) => s + n, 0);

// watchers on the busiest rooms (where writes land)
const rooms = all("select room, count(*) n from Membership group by room order by n desc limit 4").map((r) => r.room);
const watchers = [];
for (let i = 0; i < Number(watchersArg ?? 6); i++) {
  const room = rooms[i % rooms.length];
  const w = await watch(pick(inRoom(room)), room);
  if (w) watchers.push(w);
}
for (let i = 0; i < steps; i++) {
  const room = pick(rooms);
  const who = rnd() < 0.8 ? pick(inRoom(room)) : pick(people);
  let x = rnd() * total;
  for (const [n, f] of actions) {
    if ((x -= n) < 0) {
      await f(who);
      break;
    }
  }
}
await new Promise((r) => setTimeout(r, 800));
let ok = true;
for (const w of watchers) {
  const fresh = listOf(await page(w.who, w.room)) || [];
  const same = fresh.length === w.items.length && fresh.every(([id, html], k) => w.items[k][0] === id && w.items[k][1] === html);
  if (w.error) {
    ok = false;
    console.log(`STREAM FAILED: room ${w.room} as ${w.who.user}: ${w.error}`);
  } else if (!same) {
    ok = false;
    const k = fresh.findIndex(([id, html], j) => !w.items[j] || w.items[j][0] !== id || w.items[j][1] !== html);
    console.log(`DIFFERS: room ${w.room} as ${w.who.user}: ${w.items.length} items kept, ${fresh.length} fresh; first difference at ${k}: kept ${w.items[k]?.[0]}, fresh ${fresh[k]?.[0]}`);
  }
  w.ctrl.abort();
}
const events = watchers.reduce((s, w) => s + w.events, 0);
const refreshes = watchers.reduce((s, w) => s + w.refreshes, 0);
console.log(JSON.stringify({ watchers: watchers.length, steps, events, refreshes }));
console.log(ok && events > 0 ? "PASS" : "FAIL");
process.exit(ok && events > 0 ? 0 : 1);
