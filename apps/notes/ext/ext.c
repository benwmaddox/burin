/* Notes' C functions (apps/notes/notes.etch): pure, as `extern fn` must be. */
#include "etch.h"

/* A URL slug: ASCII letters and digits kept (lowercased), any run of others one '-', at most
 * `max` bytes, no '-' at either end. */
void slug(const uint8_t *t, int64_t n, int64_t max, etch_out *out) {
    uint8_t buf[256];
    int64_t k = 0;
    int dash = 0;
    if (max > (int64_t)sizeof buf) max = sizeof buf;
    for (int64_t i = 0; i < n && k < max; i++) {
        uint8_t c = t[i];
        if (c >= 'A' && c <= 'Z') c = c - 'A' + 'a';
        if ((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')) {
            if (dash && k > 0 && k < max - 1) buf[k++] = '-';
            dash = 0;
            buf[k++] = c;
        } else {
            dash = 1;
        }
    }
    etch_ext_write(out, buf, k);
}

/* Words: runs of bytes that aren't ASCII whitespace. */
int64_t word_count(const uint8_t *t, int64_t n) {
    int64_t words = 0;
    int in = 0;
    for (int64_t i = 0; i < n; i++) {
        int space = t[i] == ' ' || t[i] == '\n' || t[i] == '\t' || t[i] == '\r';
        if (!space && !in) words++;
        in = !space;
    }
    return words;
}

/* More than `words` words. */
int64_t is_long(const uint8_t *t, int64_t n, int64_t words) {
    return word_count(t, n) > words;
}
