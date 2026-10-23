# Burin: a web app language designed like hardware

*Started 22 Oct 2026. An experiment, after crudspec and AgentLang (Flow).*

Taalas made an LLM chip by fixing one model's weights into the silicon. Nothing is fetched, so
the chip runs about 10× faster than a GPU (the company's figures). FPGAs and fixed-function chips
are fast for the same reasons: everything is decided when the chip is built, data stays next to
the logic that uses it, and work flows through fixed stages instead of being looked up and
dispatched at run time.

burin asks whether a web app can be built that way. The compiler sees the whole app (as crudspec's
does) and bakes in what doesn't change between requests. What a request does at run time is
reduced to looking up already-built bytes and writing them to the socket.

## The question, and when to stop

**Question:** does an app compiled this way serve Campfire's room page with clearly less CPU per
read than crudspec, *including while messages are being posted*, without a slower edit loop?

Measured with crudspec's fair protocol (`bench/crudspec-lifetime.sh` and
`crudspec-writes.sh`: server pinned to 4 vCPUs, its own network namespace, the same snapshot
`bench-snap-nc5`, the same contract `room_show.json`):

| | Target | Kill (stop and write up) |
| --- | --- | --- |
| CPU per read, no writes | ≤ 0.06 ms (half crudspec's full-page cache, 0.12 ms) | > 0.12 ms |
| Reads/s with a writer posting 5 msg/s into the same room | ≥ 3× crudspec | < 1.5× crudspec |
| Post latency (p99) at 50 msg/s | ≤ crudspec's | > 2× crudspec's |
| Memory (PSS) after load | ≤ crudspec's | > 2× crudspec's |
| Spec to binary, after a one-line change | < 2 min (the owner, 22 Oct 2026; was 15 s) | > 5 min |
| Every maintained fragment equals a fresh render (test mode, after every write) | always | any mismatch that isn't fixed |

Phase 1 is Campfire's room page plus posting a message, nothing else. The rest of the language
is built only if phase 1 meets its targets.

## Words used here

- **Precompute:** building output before anyone asks for it, when the data changes.
- **Fragment:** a piece of a page, such as one message's HTML.
- **Hole:** a place in a template where a value goes (`{m.body}`).
- **Variant:** one prebuilt copy of a fragment for one value of a small input. "Is this message
  mine?" has two values, so a message has at most two variants for it.
- **Snapshot:** an immutable, complete set of precomputed fragments. Readers use one, and the
  writer publishes the next one.
- **Scatter write:** sending many separate byte ranges to the socket in one system call
  (`writev`), without first copying them into one buffer.
- **Hot path:** the code a typical request runs. **Cold path:** everything else.

## Principles

Each principle names the hardware idea it comes from, and what it means here.

### 1. The precomputed page is the normal path, not a cache

*Hardware: a fixed-function chip computes as data arrives; Taalas's weights are already in place.*
*Software precedent: Noria (OSDI 2018), DBSP (VLDB 2023): views are maintained as data changes.*

The compiler classifies every hole by what it depends on:

| Class | Example | When it's built |
| --- | --- | --- |
| Constant | the stylesheet links, labels, icons | at compile time, as `static` bytes |
| Row | a message's body, author, times | when the row is written (by the writer) |
| Finite per-user | "mine", "mentioned", "can edit" (a `Bool` or enum) | when the row is written, as variants, built lazily per value |
| Per-user | the user's name in the sidebar | when that user's data changes |
| Per-request | none allowed on the hot path, except a few named ones (the CSRF token, the configured origin) | at request time, into a fixed buffer |

A room page is then a list of byte ranges: constants, the room's shared message fragments, the
right variant of each, and a few per-request bytes. Posting a message builds that one message's
fragment, adds it to the room's list, and drops the oldest. That is incremental view maintenance
in its simplest form.

The cache-invalidation problem disappears instead of being solved: nothing is invalidated,
because the writer builds the new state as part of the commit.

### 2. Allocation, freeing, copying and deep branching don't happen on the hot path

*Hardware: every buffer and wire is placed when the chip is made, and the datapath has no
branches it can mispredict.* (The owner's rule, 22 Oct 2026.)

- **Allocation and freeing:** every list in a spec has a bound (`window 40`, `limit 4`,
  `max 80`). The compiler works out each route's largest response and the largest number of byte
  ranges it can have, so each connection gets fixed buffers once. The compiler reports each hot
  route's allocations, and **the build fails if one allocates**.
- **Copying:** fragments are shared, never copied into a response. A cached response is the
  response: status line, headers and body, encoded in advance, sent as one scatter write.
- **Branching:** decisions are made when fragments are built (which variant), not when they are
  sent. Routing is a generated match on the path's bytes. The compiler reports each hot route's
  branch depth.
- **Escaping** happens once, when a value is written to a fragment, never per request.
  Holes are checked for HTML context at compile time (AgentLang's item 3).
- **Formatting** happens once too: a message's time ("1:44 PM") is formatted when the message is
  stored in a fragment. The time zone is configuration, fixed at start.

**Zero-copy sends: measured, not adopted (22 Oct 2026).** The one copy left is the kernel's
copy of the page into the socket. `crates/sendlab` (`scripts/sendlab.sh`) times the sender's
CPU per page over loopback, with page shapes like the room page:

| method | raw 345 KB, 53 ranges | gzip 74 KB, 57 ranges |
|---|---|---|
| writev (what burin does) | 42 µs | 13.8 µs |
| sendmsg MSG_ZEROCOPY | 58 µs (the kernel copied every send anyway) | 27.5 µs |
| sendfile from a memfd, one call per range | 147 µs (67 µs corked) | 147 µs (39 µs corked) |
| vmsplice + splice | 33 µs | 23 µs |
| sendfile, page as 5 ranges | 25 µs | 12.1 µs |
| sendfile, page as 1 range (unreachable best case) | 20–22 µs | 7.8–8.4 µs |
| headers + sendfile of a shared prefix + writev of the rest | 26 µs | 12.4 µs, but the reader +2.8 µs |

- sendfile costs per call, not per byte. So the lever is contiguous layout, not the send
  primitive.
- Layout can't be made contiguous here. A message's bytes depend on the viewer (`message--me`,
  edit and boost controls). A contiguous block would be needed per viewer pattern, rebuilt on
  every post.
- Even the best case saves the sender 1.4 µs per gzip page and costs the receiver more.
- MSG_ZEROCOPY can't be judged on loopback or veth, which always copy. It may pay on a real NIC
  for large raw bodies; retest there.

Rust helps here: a per-request arena with a lifetime lets the borrow checker prove that nothing
outlives the request, which crudspec's Go runtime can't prove (its bench README: "nothing proves
it here").

### Cached pieces never depend on the viewer (22 Oct 2026)

The first design also kept per-viewer pieces (each person's sidebar) and shared pieces with
variants by viewer (a message "mine" or not). The mixed benchmark (117 people, 60 rooms) showed
the cost: one post marked a room unread for everyone in it, and the writer rebuilt and
recompressed every member's sidebar (posts went from 2 ms to 60 ms), and memory grew with people.
The rule now (the owner's decision):

- **A cached fragment is the same for everyone.** It reads its row (or nothing), never the
  viewer; the compiler refuses one that does. It is built once at write time, precompressed, and
  borrowed into every response. Its cache key is its row.
- **A fragment that reads the viewer is `per viewer`: rendered per request**, never kept. It is
  straight-line code over prepared queries, into reused per-thread buffers (the fast path still
  allocates nothing). It merges the cached fragments it calls: each call is a reference to the
  cached piece, which goes into the response as it is (no copy).
- **Merging** is a scatter list: cached pieces borrowed, per-request bytes written once. In a
  gzip response the per-request bytes are stored deflate blocks (no compression work; the CRC
  continues over them).
- What only decorates a page per viewer (their own messages marked, the edit controls theirs or
  an administrator's) is shown by the page's script from the viewer's id and role; the server
  still decides every action. With the script applied, the pages equal the former server-built
  ones byte for byte (`scripts/golden-viewer-js.py`, in the checks).

So a write's work no longer grows with the people who can see what it changed, and the writer
keeps no per-viewer state at all.

### Keeping precomputed pages right (22 Oct 2026)

A kept page must always equal a fresh render. Time limits on cached pages were considered and
rejected: they hide a missed dependency instead of fixing it, and break that guarantee for up to
the limit. Instead there are four layers:

1. **Precise delta edges** from the spec: which kept pieces a write changes, and how (forwarded,
   patched, appended, rebuilt at a key). This is the fast path for writes.
2. **Read sets, and a fail-safe.** While the compiler lowers a register's loader or a screen
   list's query, it records every (entity, column) it turns into SQL: after functions and views
   are expanded and references followed, so nothing a page reads can be missed. A write that
   touches a register's read set without a precise edge gets a fallback edge: every kept instance
   is rebuilt. A missing rule costs speed, never correctness, and the build reports it
   (`FALLBACK` in `emit`'s edges). Campfire has none.
3. **A scrubber.** While the writer is idle (`--scrub-ms`, default 100), it rebuilds one kept
   page for one audience member at a time from SQLite and compares it with what readers get. A
   difference is logged, counted (`scrub_fixed`) and rebuilt. This is the hardware idea of
   memory scrubbing: it limits the damage from a bug that gets past the first two layers.
4. **Random-action testing** (`scripts/fuzz.sh`): seeded runs of every action by every person,
   with page reads after each and `--verify` comparing every fast page with a fresh build; it
   passes only with no mismatch, no full clear, and nothing for the scrubber to fix.

The writer is preferred over readers: read workers run at nice 10, so under contention the
writer gets about nine times a reader's CPU share, and reads still use every idle cycle.

### 3. Actions are fixed rules, run by one writer

*Hardware: Bluespec's guarded atomic actions; a rule fires only when its guard holds, and the
compiler schedules rules that can't conflict.* *Software precedent: H-Store/VoltDB and "OLTP
Through the Looking Glass" (SIGMOD 2008): only about 4% of a database's time was useful work; the
rest was locking, latching, logging and buffer management.*

- Actions are the only way to change data. `require` is the guard.
- Each action's reads and writes are known at compile time, so the compiler knows which
  fragments each action can change. It doesn't parse SQL at run time to find out (crudspec's
  `livekeys.ts` does).
- One writer thread takes actions from a bounded queue, runs a batch in one SQLite transaction
  (group commit), builds the changed fragments, and publishes a new snapshot by swapping one
  pointer. Readers never take a lock.
- A queue that is full refuses work (503) instead of growing: backpressure, as in a hardware
  pipeline.

### 4. The fast path is split from everything else

*Hardware: Catapult v2 put the FPGA on the network path; KV-Direct served 180M key-value
operations/s from a network card. Both handle only a tiny, fixed hot path.*

The fast path parses only the request line and the cookie, finds the actor from a precomputed
table, and writes the snapshot's byte ranges. It is a small, separately optimized piece of the
runtime. Everything else (forms, sign-in, first renders of cold rooms, the agent API) goes to the
cold path, which renders from SQLite with generated code, as crudspec does.

A room nobody has read yet isn't precomputed; the first read renders it on the cold path and
hands it to the writer to keep. Precomputed state has a memory budget, like crudspec's caches,
and rooms not read lately are let go.

### 5. Queries are straight-line code that fetch only what is used

*Software precedent: Neumann, VLDB 2011: compile each query to code that pushes rows through
every step without generic operators.*

Each query becomes prepared SQL selecting only the columns its uses read (crudspec's pruning),
and generated code that decodes each row straight into a fixed struct, with no boxed values
(crudspec's typed DTOs, Flow's `db.rows<Row>`).

### 6. More from hardware

- **Lookup tables for small inputs.** Any hole whose value comes from a `Bool` or a small enum
  is precomputed for each value (the variants above), the way an FPGA lookup table holds every
  output of a few input bits. The compiler limits variants per fragment (default 8).
- **A base die and a few custom layers.** Taalas changes only a few mask layers per model. Here,
  the runtime crate is prebuilt and shared; the compiler generates a small crate. Constants are
  `static` byte arrays, so a change rebuilds only that crate. Spec-to-binary time is measured
  from the first slice, because rustc time is the main risk to the 15 s budget.
- **Verify before tape-out, against a golden model.** Hardware is checked against a simple
  reference model before it is made. In test mode, after every write, each maintained fragment is
  compared with a fresh render; this check is always on in tests (crudspec's lesson: its
  incremental-vs-full check found a real bug but was opt-in). The room page is compared with
  crudspec's, as the reference.
- **Worst cases known at build time.** Each route's largest response, rows read and byte ranges
  are reported by the compiler, like a chip's timing report.
- **Clock-domain crossing.** Readers and the writer run separately and meet only at the snapshot
  swap, like two clock domains joined by a double-buffered register.
- **Area is a budget.** Precomputing spends memory to save CPU. The budget is explicit, and the
  compiler reports what each fragment kind can cost.

## The language leans toward efficient apps

burin is for many apps; Campfire is the first. The compiler and runtime know nothing about any one
app. The language is shaped so that the efficient design is the default and the short thing to
write, and anything that would cost the read path is either refused or written explicitly (and
then shown in the compile report). The checker enforces these rules:

| Rule | What it buys | What you write when you need the exception |
| --- | --- | --- |
| **Every list a page shows is bounded**: a view used by a screen has `limit N` or `window N`; text fields have `max N` | worst-case page size and memory per register known at compile time; fixed buffers | nothing: unbounded lists can't be shown (page them) |
| **What a page shows per viewer is a `Bool` or an enum** (a variant bit) | one prebuilt variant per kind of viewer, chosen at read time without branching | a fragment keyed by the viewer (the sidebar): the report names its memory cost per viewer |
| **No clock, randomness or request data in templates** | fragments stay valid until a write changes them | `cold` on the screen: rendered per request, outside the fast path |
| **A screen's read stage must be clean** (lookups, variant choice and concatenation only) | the fast path: no queries, formatting, escaping or allocation per request | `cold` on the screen; otherwise the build fails and names the node that forced it |
| **Actions are the only writes, with statically known write sets** | the compiler derives which registers each action changes; no cache invalidation to get wrong | nothing: there is no other way to write |
| **Queries name their order and go through indexes** | write-time rebuilds are index lookups; incremental appends where the order is proved monotone | nothing: a scan of a table on a hot rebuild is reported as an error |
| **Holes are escaped for where they are**, at write time | safety without per-request work; `on…=` holes take only `json(…)` | `raw(…)` for `Html` values (autolink's output), which the checker types |

## Numbers and time (22 Oct 2026)

Three number types, each a 64-bit integer in an `INTEGER` column (no floating point, so SQLite
and the fragments' code compute the same bits):

- **`Int`**: a whole number.
- **`Time`**: an instant, in milliseconds since 1970 (UTC). `now` is the action's time (one value
  for the whole action); every row has `created` and `updated`.
- **`Duration`**: a length of time, in milliseconds. Literals take a unit: `250ms`, `30s`, `5min`,
  `2h`, `3d`; `-30s` is negative.

What arithmetic means, by type (anything else is refused, naming the two types):

| | gives |
| --- | --- |
| `Int + - * / % Int` | `Int` |
| `Time + Duration`, `Duration + Time`, `Time - Duration` | `Time` |
| `Time - Time` | `Duration` (how long between) |
| `Duration + - % Duration`, `Duration * Int`, `Int * Duration`, `Duration / Int` | `Duration` |
| `Duration / Duration` | `Int`: how many fit (`elapsed / 1h` is whole hours) |

So `now + p.took * (p.level + 1)` is a Time, and a rate per hour is `p.rate * (now - p.updated) / 1h`
(multiply first: whole hours would drop the minutes). `*`, `/` and `%` bind tighter than `+` and
`-`. Functions: `min`, `max` and `clamp(x, lo, hi)` (= `max(lo, min(x, hi))`) on Ints, Times or
Durations of one kind; `abs` on an Int or a Duration. Comparisons order Times and Durations.

**The rules are SQLite's, in both places.** An action computes in SQL (`require`, `set`, `let`, and
values in `create`); a fragment computes in its own LLVM code, at write time. They must agree, or a
page could say "can build" while the action refuses. So the fragments' code copies SQLite's integer
rules:

- `/` truncates toward 0 (`-7 / 2` is `-3`) and `%` takes the dividend's sign (`-7 % 3` is `-1`).
- **none in, none out**: `p.bonus + 1` is none when `bonus` is (the checker types it `Int?`).
- **none orders with nothing**: `x < 10` is false when x is none, in a page as in a `require`
  (SQL's NULL), and `not (x < 10)` is true in both (SQL's NOT of it is made two-valued with
  `coalesce`). `==` and `!=` see none as a value: `x == none` is true when x is none.
- **dividing by 0 gives none** (SQLite's NULL). So `a / b` is `Int?`, unless `b` is a nonzero
  literal (`a / 4`, `d / 1h`).
- **overflow gives none.** A result outside 64 bits is none in a page, through LLVM's
  overflow-checked add, subtract and multiply. In SQL, SQLite makes it a REAL; an action reads any
  computed value that isn't an INTEGER as none (`burin_col_int`), and a STRICT table refuses to
  store a REAL, so the write fails and the action rolls back. One gap remains: an overflow *inside*
  a `require`'s comparison compares as a REAL in SQL, and as none in a page.

A `set` whose value is arithmetic comes back with `RETURNING`, so a forwarded value is the one
stored (computing `p.n - 1` again after the UPDATE would read the new `p.n`).

Shown in a page: `{{n}}` or `text(n)` for an Int's digits, `grouped(n)` with thousands grouped
("1,234,567"), `text(d)` for a Duration in its two largest units ("2h 5m", "45s", "-1m 30s"),
and `iso(t)`, `date(t)` and `clock(t)` for a Time. A page can't read `now`, because a cached
fragment must stay right until a write changes it. To show a countdown, write the end time with
`iso(…)` into a `<time datetime>`, and count in the browser.

`apps/numbers` (a tiny build queue) and `scripts/numbers-check.sh` check all of this end to end.
Each page value is compared with the same value computed by an action in SQL, and by Python, on
rows made to hit the edges: dividing by 0, none, negatives, and overflow at 64 bits.

**Not yet:** fractional numbers (one design: `Decimal`, an Int scaled by 10⁴), scheduled actions
(run at a time), and `now` in `cold` screens.

## External functions (23 Oct 2026)

When a spec can't say something, it calls code outside burin through the C ABI: C, or Rust's
`extern "C"`, in `apps/<app>/ext/` (C sources are compiled into the app; `ext.rs` goes into its
crate). `apps/notes` and `scripts/notes-check.sh` use every form.

    extern fn slug(title: Text, max: Int): Text      // pure: pages may call it
    extern effect fn notify(text: Text): Int         // effects: only actions call it

- **What crosses:** Int, Bool (0 or 1), Time and Duration (ms) as 64-bit integers; Text as its
  UTF-8 bytes and length, borrowed for the call only. A Text result is written with
  `burin_ext_write` (`include/burin.h`; `burin_rt::ext` in Rust), so burin never takes memory it
  would have to free; what isn't UTF-8 is replaced.
- **Pure, or effects:** a cached piece calls a pure function once, when it's built, and serves
  that result until its inputs change. burin can't check purity: a function that reads the
  clock, a file or its own state shows a stale result. `--verify` and the scrubber compare kept
  pages with fresh renders, so this usually shows in tests. `extern effect fn` runs only in
  actions, on the writer, inside the action's transaction: its effect isn't undone if the action
  then fails, and a slow one delays every write. (Email, webhooks and other services want an
  outbox and jobs instead: not built yet.)
- **No data access:** an external function gets no database: what it needs is passed in, which
  keeps burin's read tracking exact. It can't run inside a query (SQLite runs those); an action
  may compute one from its arguments and store the result in a field.
- **Costs:** a text argument is written into a buffer first; C is a separate native library, so
  it isn't inlined. It runs in the server's process: memory errors in C can corrupt what every
  reader shares (Rust is the safer choice). A function that allocates breaks the fast path's
  zero allocations for its pages (C's allocations aren't even counted by the hot-path test).
- **Not yet:** linking system libraries (`-lfoo`), Lists and records across the ABI.

## The standard library (23 Oct 2026)

Every app has `lang/std.burin`: text (`length`, `contains`, `starts_with`, `ends_with`,
`index_of`, `trim`, `truncate`, `slice`, `replace_all`, `repeat`, `pad_start`, `pad_end`,
`capitalize`, `plural`), numbers (`decimal`, `percent`, `round_to`) and time (`format_time`,
`weekday`). Each is Rust in the runtime (`crates/burin-rt/src/stdlib.rs`), called as an external
function, so adding one is a declaration and a Rust function, not compiler work. All are pure.

- **The same answer everywhere:** text is counted in characters, as SQLite counts it. The ten
  with a SQL form (`lang/std_sql.rs`, included by the compiler and by the runtime's test) also
  run inside queries (`Note where length(.body) > 30`); a test checks each form against its Rust
  on thousands of inputs (multi-byte text, emoji, tabs, empty text, negative counts). A function
  without a SQL form can't run on a query's rows.
- **A template's own names:** `{{let t = trim(n.title)}}` names a value for the rest of its block
  (and blocks inside it); the parser puts the value in place, so nothing after it changes.
- **Not yet:** lists as values (`split`, `join` with a separator, sums over a list), records,
  Decimal as a type.

## Live updates (23 Oct 2026)

Open pages follow writes the way Rails Campfire's do: Turbo Streams. Here they come over one
server-sent events stream per page (`<turbo-stream-source>`, `/_burin/live`), not Action Cable.
Each event is built once, by the writer, from what it already knows changed. It is sent to every
stream on its topic as the same bytes. The runtime's side is `crates/burin-rt/src/live.rs`.

- **Live lists:** `{{for m in room_messages(room) live}}`. The list's element gets an id, and its
  changes go out as `append`, `prepend`, `replace` and `remove`, keyed by each item's element id.
  When a change can't be said item by item, the event is `refresh` and Turbo reloads the page.
- **Numbered and replayed:** a page carries the number of the last event its snapshot includes. A
  stream that connects later, or reconnects with Last-Event-ID, first gets what it missed from its
  topics' rings (256 events each). Past a ring, a list topic sends `refresh`.
- **Audiences:** a stream opens only for a viewer in the screen's audience. When a write changes
  the audience, the streams of viewers who left are closed.
- **Presence:** `watch read_room` runs the action when a stream opens and again when it closes, so
  a room someone sat in while posts arrived isn't unread on their next sidebar.
- **Signals:** an action says `signal unread(room) to room(room)` (or `to viewer`, or
  `to everyone`). After the action commits, open pages get a Turbo Stream action named `unread`
  whose target is the argument's id, and the page's script decides what it means. Campfire marks
  sidebar links unread or read, and reloads the sidebar's frame for room changes. A signal is one
  event on one topic every stream is on, tagged with its audience (a screen instance's, from the
  snapshot). Each worker checks its own streams' viewers, so a post to a 1,000-member room is
  still one event. A failed action sends none. Signals a stream missed past the ring become
  `missed`, and the page reloads what signals keep up to date.
- **Departures from Rails, for the benchmark's fairness:**
  - Rails broadcasts `unread` to each member's own stream (one broadcast per member). Burin sends
    one event and filters per stream: fewer server operations per post, the same bytes per client.
  - For room changes (create, rename, members, involvement, delete), Rails pushes the rendered
    sidebar row. Burin reloads the frame: one more request per affected sidebar, for writes that
    are rare.
  - Rails reorders direct rooms on unread (its sorted-list controller). Burin doesn't yet.
  - Like Rails, members dropped from a closed room get nothing on their sidebar.
- **Checks:**
  - `scripts/live-check.sh`: events in order, presence, replay, refusal.
  - `scripts/revoke-check.sh`: a dropped member's stream closes.
  - `scripts/live-fuzz.sh`: random writes while streams are held. Each watcher applies its events
    as Turbo does, and must end with the same list as a fresh page.
  - `scripts/signals-check.sh`: who gets which signal.
  - `scripts/browser-live.sh`: two Chromium pages.

**Targets, measured with live clients in the mixed benchmark (proposed; not yet measured):** at
50 posts/s with 1,000 open streams:
- Delivery latency, from the post's reply to the event's bytes at a client: p50 ≤ 5 ms, p99 ≤ 50 ms.
- Delivered share (the events clients got ÷ the events due them, counted by item id): ≥ 99.9%.
- Holding the streams costs at most 15% of page throughput.

The Rails port and the Rust port get the same clients over Action Cable.

**Measured (s7, 23 Oct 2026, `bench/runs/mix-s7-*`; streams reopen every 30 s on average):**

| posts/s | no streams | 117 streams | 1,000 streams |
|---|---|---|---|
| Pages/s at 0 | 47.9k | 48.6k | 45.4k |
| Pages/s at 10 | 39.8k | 39.5k | 35.9k |
| Pages/s at 50 | 28.9k | 28.2k | 23.1k |
| Delivery p50 / p99 at 50 | – | 6.1 / 9.8 ms | 7.2 / 12.6 ms |

- 117 streams is one per person in the data; 1,000 is about 8.5 tabs each.
- Every due post was delivered in every run (85k due at 50 posts/s with 1,000 streams).
- One stream per person costs nothing measurable. At 1,000 streams, 50 posts/s costs 20% of page
  throughput, under the 30% kill line but over the 15% target. Nearly all of that cost is the
  kernel's socket writes: each post's `unread` goes to nearly every stream, and each read signal
  to all of the reader's tabs, about 90k writes a second. The server's user-space work fell.
- Each worker writes a wake's events to a stream at once, and finds a signal's few viewers
  through an index rather than checking every stream. Neither moved the 1,000-stream number.
- Delivery p50 (about 6 ms) misses the 5 ms target. It is the writer's batch time: a post's own
  answer takes as long.

**Kill criteria:** p99 over 250 ms, a delivered share under 99%, or more than 30% of page
throughput lost. Any one of these means rethinking where events are built: the single writer
building every event, and the workers' inboxes.

## Helpers (23 Oct 2026)

Computing that expressions can't say (a checksum, a compressor, a game's rules) is written in
burin as a helper: a `fn` with a body of statements.

    fn crc32(t: Text): Int {
      var table = ints(256)
      for i in 0..256 { … }
      var crc = 0xFFFFFFFF
      for i in 0..byte_length(t) {
        crc = table[(crc ^ byte_at(t, i)) & 255] ^ (crc >> 8)
      }
      return crc ^ 0xFFFFFFFF
    }

- **What it has:**
  - `var`, assignment, `if`/`else`, `while`, `for i in a..b`, `break`, `continue`, `return`.
  - Arrays: `ints(n)`, `int32s(n)` and `bytes(n)`, zeroed, any size.
  - Bit operations (`& | ^ << >>`) and wrapping arithmetic (`+% -% *%`).
  - Text read as bytes (`byte_at`, `byte_length`). A Text helper writes its result with
    `out_byte`.
  - It may call other helpers and expression `fn`s.
- **Kept apart from the data:**
  - Parameters are Ints, Bools, Times, Durations and Text, never rows. No queries, `viewer` or
    `now`. So a call's reads are its arguments', and burin's read tracking is unchanged.
  - A helper is declared as an external function of its own (`burin_fn_f`, the same C ABI). It's
    placed and checked like one: pure, so cached pieces may call it; actions bind its result
    into their SQL; queries can't run it on their rows (store what it computes in a field).
- **Ending:**
  - No recursion: the compiler checks the calls, including through expression `fn`s.
  - Each `while` turn spends a step of the thread's budget, unless the loop is counted: its
    condition is `x < y` (first in an `and`), x changes only by one top-level `x = x + c`, y reads
    nothing the loop changes, and nothing `continue`s it. That ends by itself, as a `for` does,
    and spends none.
  - The writer's budget is about 50 ms of turns, since every write waits for the pieces it
    builds; a request worker's is about half a second.
  - Heavy computing belongs in background work (the outbox and jobs, not built yet), not in a
    cached piece.
- **Failing:** an overflow, a divisor of 0, an index outside its array, a shift outside 0 to 63,
  a byte outside its text or the budget spent each make the result none. An Int is
  `i64::MIN`, as burin's none is everywhere; a Bool is false; a Text helper's writing is
  undone.
- **Memory (time over memory):**
  - Arrays of a size written out, up to 4 KB, are on the stack, where LLVM keeps them in
    registers.
  - The rest come from a bump arena per thread, marked on entry and released on every way out.
    The memory is kept for the next call; a call can't take more than 256 MB.
  - Arena arrays are `noalias` (as malloc's), so LLVM knows they don't overlap the text.
- **Measured** (`bench/crunch/run.sh`, Silesia's dickens, 10.2 MB, one core, median of 9):
  DEFLATE (greedy LZ77 over a 32 KB window, fixed Huffman codes) in burin and the same
  algorithm in C give the same bytes, which inflate back.

  | | DEFLATE MB/s | vs clang -O2 | CRC-32 MB/s |
  |---|---|---|---|
  | burin | 24.8 | 0.70 | 579 |
  | burin, no step budget | 27.0 | 0.76 | 582 |
  | C, clang -O2 | 35.6 | 1.00 | 564 |
  | C, gcc -O2 | 35.4 | 0.99 | 575 |
  | C, clang with overflow and bounds traps | 28.0 | 0.79 | 579 |
  | C, gcc with overflow and bounds traps | 33.7 | 0.95 | 581 |

  - burin is about as fast as C with checks.
  - Its gap to plain C is the step budget (about 8%, all on the inner byte-matching `while`)
    plus checks C doesn't make: every `byte_at` and index is bounds-checked, every `+` is
    overflow-checked.
  - CRC-32 matches C.
  - For context, other algorithms: libdeflate level 1 runs at 240 MB/s to 41% of the size (this
    algorithm gets 49.6%).
- **Next:**
  - Fewer checks where the compiler can prove a range (hoisting, as Roc does).
  - Tables built at compile time.
  - Arrays passed between helpers.
  - Text built from pieces.
  - A `Bytes` type for results that aren't UTF-8.

## Lessons kept from crudspec and AgentLang

- **The whole app is one program, checked as a whole** (crudspec): names, types, permissions,
  links, forms. burin adds the dependency analysis of principle 1 to the same pass.
- **Rules carry through every layer** (crudspec): `max 80` is a database CHECK, a form's
  `maxlength`, and here also a buffer size.
- **Data lives in SQLite.** All precomputed state is derived and can be rebuilt from SQLite at
  any time. It is never the source of truth (the owner's rule for crudspec).
- **Typed columns and typed rows, no JSON columns, no boxing** (crudspec).
- **Holes are checked for their HTML context at compile time** (AgentLang item 3).
- **No hand-written crypto** (AgentLang): HMAC from the `hmac`/`sha2` crates.
- **Use built libraries for CPU-heavy pieces:** `httparse` for HTTP parsing, `rusqlite` (bundled
  SQLite), `libdeflate` if compression is added.
- **A small language where the data is** (crudspec's lesson from AgentLang): every list a page
  shows is bounded, and nothing recurses. Since 23 Oct 2026 helpers add loops and mutation for
  computing. They're Turing-complete within a step budget, and kept apart from the data: they see
  only their arguments (see Helpers).
- **Time over memory** (the owner's rule, 23 Oct 2026): with arenas, memory is the cheap
  resource. Spend it to save time (tables, buffers, arenas kept for reuse), and budget runaway
  work by time, not bytes.
- **Kill criteria in writing, and differential checks mandatory** (both).
- **A fast edit loop from day one** (AgentLang's 5–10 minute builds shaped most of its work).
- **Compare against a strong baseline:** crudspec's Go build and the recorded Rust port, with
  the fair protocol.

## Plan

**Phase 1a: the datapath, placed by hand.** Hardware designers often hand-place a critical
datapath before trusting a tool to generate it. Write the runtime crate, then by hand the Rust
the compiler should generate for the room page and `post_message`. Measure it against the
targets. If it misses them, the compiler isn't worth building, and that is the result.

**Phase 1b: the compiler.** A Rust compiler that turns an app's spec into the dataflow IR of
`docs/IR.md` and lowers it straight to LLVM IR, linked with the runtime under cross-language
LTO (the owner's choice, 22 Oct 2026). Campfire's room page and `post_message` come first; it
must produce the same page bytes as 1a, and match or beat 1a's measurements. Built as a series
of swaps, each checked against the golden pages. Before it counts as done, a second, small app
compiles without changes to the compiler.

**Phase 2** (only if phase 1 passes): the rest of Campfire, live updates (the writer already
knows what changed), the agent API, migrations, and an LSP.

## Reference page

`D:\code\crudspec-hw\golden\room.html` is crudspec's room page for the benchmark user on
`bench-snap-nc5` (366,229 bytes, the same on every request, valid against the contract). burin's
page must equal it once crudspec's live-update bookkeeping is removed: the `<!--f:…-->` markers
and the `crudspec-page` data script. burin will need its own live updates in phase 2.
