# etch's IR: an app as a dataflow netlist

*22 Oct 2026.* How the compiler organizes an app before LLVM sees it. The model is a hardware
netlist: a graph of sources, operators and registers, where every node belongs to one clock
domain (stage), and values cross between stages only at named places.

## Words used here

- **Node:** one value the app computes: a query's rows, a field, a formatted time, a piece of
  HTML.
- **Stage:** when a node is computed. Like a clock domain in a chip, a stage runs on its own
  schedule, and values cross into another stage only through a register.
- **Register (materialized node):** a node's value kept in the snapshot, under a key, between
  writes. The registers are what "precompute is the normal path" means.
- **Key:** what a register's value depends on that changes per instance: a room, a message, a
  viewer.
- **Audience:** the viewers a register's variants are built for, derived from the screen's guard.
- **Variant bit:** a `Bool` a node depends on that comes from the viewer. Each one doubles the
  number of possible variants, the way each input of a lookup table doubles its size.
- **Splice:** a place in a register's bytes filled at read time from another register, because
  the value depends on something the first register isn't keyed by.
- **Delta edge:** what an action's write does to a register: which keys it affects, and how the
  register is brought up to date.

## The stages

| Stage | Runs | Its nodes | Crosses into the next stage through |
| --- | --- | --- | --- |
| **compile** | once, in the compiler | constant HTML, SQL text, row layouts | static bytes in the binary (`.rodata`) |
| **start** | once, when the server starts | the secret, the assets' hashed addresses, the configured host | start-time constants the builders read |
| **write** | in the writer, after each commit | queries, formatting, escaping, signing, fragment HTML, variants | the snapshot swap: the only write→read crossing |
| **read** | per request, on the fast path | key lookups, variant choice, splices, the list of byte ranges | the socket (`writev`) |

A node is placed in the earliest stage whose inputs are all available there. The **read stage
may contain only** lookups by key, choices by variant index, and concatenation. A route whose
read stage would need anything else (a query, formatting, an allocation) can't use the fast
path. The compiler reports which node forced that, and the route goes to the cold path.

## Inputs and their classes

Every node gets the set of inputs it depends on, worked out by propagating from the leaves:

| Input | Example | Stage it allows |
| --- | --- | --- |
| constant | a literal | compile |
| config | `config.host`, the secret, an asset's hash | start |
| route parameter | `room` in `/rooms/{room}` | write (as a key) |
| row | `m.body` for `m` in a query | write (as a key, or inside a keyed list) |
| viewer, through a `Bool` | `m.creator == viewer` | write (as a variant bit) |
| viewer, otherwise | the viewer's sidebar | write (keyed by the viewer) |
| request, otherwise | a query string, a cookie's value | cold path only |
| clock | `now` in a template | cold path only |

## Registers

A register is a node whose value is kept under a key. The compiler makes one wherever a node
can't be compile- or start-time but all of its inputs are keys, rows or variant bits:

- the **page head** of a room screen, keyed by `room`;
- each **message fragment**, keyed by the message, with variants for its viewer bits;
- the room's **message window** (`room_messages(room)`: window 40 last), keyed by `room`: a list
  of message registers;
- the **sidebar**, keyed by the viewer;
- the **actor index** (session token → user) and the **users** the fragments read. These are
  registers like any other, with delta edges, so a revoked session or a renamed user takes
  effect at the next commit. (Phase 1a loaded them once at start; that was a shortcut.)

### Audiences and variants

A register with variant bits needs to know whose variants to build. The compiler derives its
**audience** from the screen's guard. `require member(viewer, room)` means the audience of
everything keyed by `room` is `Membership where .room == room` → `.user`. For each member, the
writer evaluates the register's variant bits (the bits inside loops too, such as "is this boost
the viewer's"), deduplicates the bit vectors, and builds one variant per distinct vector. A
read picks the variant by the viewer's place in the audience. The compiler caps variants per
register (default 256) and reports the worst case.

### Splices

When a hole inside a register depends on an input the register isn't keyed by, it becomes a
splice. Example: the sidebar is keyed by the viewer, but its forms carry the page's token,
which is keyed by the room. The read stage joins the two registers' bytes: no copy, just one
more byte range.

## Delta edges: what each action changes

For every action, the compiler pairs each write (table, columns) with every register that reads
those columns, and derives an **affected-keys query**: the keys whose values may have changed.
The always-correct default is to rebuild those keys from SQL after the commit. Examples from
`post_message`:

| Write | Register | Affected keys | Update |
| --- | --- | --- | --- |
| insert `Message` (room = r) | message window of `room` | `r` | append: `created = now` is the window's order, so the new row is last; the oldest drops (proved monotone) |
| insert `Message` | message fragment | the new row | build it |
| set `Room.updated` (r) | sidebars (direct rooms are ordered by it) | `Membership where .room == r` → `.user`, if `r` is direct | rebuild |
| set `Membership.unread_since` | sidebars | each changed row's `.user` | rebuild |

An incremental update (the append) is used only where the compiler can prove it equals the
rebuild. Everything else is rebuilt. In test mode, `--verify` compares every fast page with a
fresh render after every write (the golden model).

## Lowering to LLVM

The compiler emits one LLVM IR module per app, linked with the prebuilt runtime crate (the base
die) under cross-language LTO. rustc's own LLVM tools and `rust-lld` are used, so the generated IR
and the runtime's bitcode share one LLVM version, and runtime helpers inline into generated
code. The spike in `scripts/spike-lto.sh` shows a Rust helper inlined into a generated function,
leaving neither symbol behind.

| IR element | LLVM |
| --- | --- |
| a row layout | a struct type; fields at fixed offsets; text as (pointer, length) |
| a query | a constant SQL string, and a decode loop that calls the runtime's statement helpers and fills structs |
| a fragment builder (write stage) | a function: straight-line appends of constant byte ranges and escaped holes, a branch per variant bit, a loop per list |
| the read stage of a route | a function: lookups, the variant index, and a push of each byte range; no calls that allocate |
| an action | a function: the guards, then the statements; it returns the deltas |
| a delta edge | a function: the affected-keys query and the rebuild or the incremental update |

## The compile report (the "timing report")

For each route, the compiler prints:
- each node's stage;
- each register's key, audience and variant bits;
- the splices;
- the number of byte ranges a page has;
- its worst-case size from the spec's bounds;
- the memory a register costs per key.

A route whose read stage isn't clean names the node that forced it to the cold path.

## Compatibility with crudspec

To compare byte for byte, phase 1 reproduces crudspec's HTML, including details that are
crudspec's implementation rather than etch's design: the handler ids 88, 137 and 85 in the
signed form tokens, and those tokens' JSON shapes. These live in one labelled file in the app's
spec (`apps/campfire/crudspec-compat.etch`) and don't shape the language.
