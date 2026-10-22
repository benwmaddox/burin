#!/bin/bash
# The bundled SQLite's compile flags (libsqlite3-sys's build.rs).
f=$(ls -d ~/.cargo/registry/src/*/libsqlite3-sys-0.30*/ | head -1)build.rs
echo "$f"
grep -n 'MEMORY_MANAGEMENT\|LIBSQLITE3_FLAGS\|"-U\|THREADSAFE\|MEMSTATUS' "$f" | head -20
