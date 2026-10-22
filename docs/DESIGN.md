# etch: a web app language designed like hardware

*Started 22 Oct 2026. An experiment, after crudspec and AgentLang (Flow).*

Taalas made an LLM chip by fixing one model's weights into the silicon. Nothing is fetched, so
the chip runs about 10× faster than a GPU (the company's figures). FPGAs and fixed-function chips
are fast for the same reasons: everything is decided when the chip is built, data stays next to
the logic that uses it, and work flows through fixed stages instead of being looked up and
dispatched at run time.

etch asks whether a web app can be built that way. The compiler sees the whole app (as crudspec's
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
| Spec to binary, after a one-line change | ≤ 15 s | > 60 s |
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

Rust helps here: a per-request arena with a lifetime lets the borrow checker prove that nothing
outlives the request, which crudspec's Go runtime can't prove (its bench README: "nothing proves
it here").

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

## Lessons kept from crudspec and AgentLang

- **The whole app is one program, checked as a whole** (crudspec): names, types, permissions,
  links, forms. etch adds the dependency analysis of principle 1 to the same pass.
- **Rules carry through every layer** (crudspec): `max 80` is a database CHECK, a form's
  `maxlength`, and here also a buffer size.
- **Data lives in SQLite.** All precomputed state is derived and can be rebuilt from SQLite at
  any time. It is never the source of truth (the owner's rule for crudspec).
- **Typed columns and typed rows, no JSON columns, no boxing** (crudspec).
- **Holes are checked for their HTML context at compile time** (AgentLang item 3).
- **No hand-written crypto** (AgentLang): HMAC from the `hmac`/`sha2` crates.
- **Use built libraries for CPU-heavy pieces:** `httparse` for HTTP parsing, `rusqlite` (bundled
  SQLite), `libdeflate` if compression is added.
- **A small language, not Turing-complete** (crudspec's lesson from AgentLang): every list is
  bounded, and there's no recursion.
- **Kill criteria in writing, and differential checks mandatory** (both).
- **A fast edit loop from day one** (AgentLang's 5–10 minute builds shaped most of its work).
- **Compare against a strong baseline:** crudspec's Go build and the recorded Rust port, with
  the fair protocol.

## Plan

**Phase 1a: the datapath, placed by hand.** Hardware designers often hand-place a critical
datapath before trusting a tool to generate it. Write the runtime crate, then by hand the Rust
the compiler should generate for the room page and `post_message`. Measure it against the
targets. If it misses them, the compiler isn't worth building, and that is the result.

**Phase 1b: the compiler.** A Rust compiler for a crudspec-like syntax, covering what the room
page and `post_message` need. It must generate code equivalent to 1a's (the same page bytes and
the same measurements).

**Phase 2** (only if phase 1 passes): the rest of Campfire, live updates (the writer already
knows what changed), the agent API, migrations, and an LSP.

## Reference page

`D:\code\crudspec-hw\golden\room.html` is crudspec's room page for the benchmark user on
`bench-snap-nc5` (366,229 bytes, the same on every request, valid against the contract). etch's
page must equal it once crudspec's live-update bookkeeping is removed: the `<!--f:…-->` markers
and the `crudspec-page` data script. etch will need its own live updates in phase 2.
