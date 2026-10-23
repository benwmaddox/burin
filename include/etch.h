/* etch's external functions (`extern fn` in a spec), called through the C ABI.
 *
 * A spec declares    extern fn slug(title: Text, max: Int): Text
 * and apps/<app>/ext/ defines it (in C, or in Rust as `#[no_mangle] extern "C"`):
 *
 *   void slug(const uint8_t *title, int64_t title_len, int64_t max, etch_out *out);
 *
 * - Int, Bool (0 or 1), Time (ms since 1970) and Duration (ms) are int64_t.
 * - Text arguments are UTF-8 bytes and their length (not NUL-terminated), valid only during the call.
 * - A Text result is written with etch_ext_write into `out`, passed last; others are returned.
 * - `extern fn` must be pure: the same result for the same arguments, no effects (pages keep
 *   what it returned). `extern effect fn` may have effects; only actions call it, on the
 *   writer's thread, and its effects aren't undone if the action fails.
 * - It runs on a request or the writer's thread: it must not block for long, and must be safe
 *   to call from several threads at once.
 */
#ifndef ETCH_H
#define ETCH_H
#include <stdint.h>

typedef struct etch_out etch_out;

/* Appends `n` bytes of UTF-8 to a Text result (bytes that aren't UTF-8 are replaced). */
void etch_ext_write(etch_out *out, const uint8_t *p, int64_t n);

#endif
