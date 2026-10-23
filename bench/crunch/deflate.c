// apps/crunch/deflate.burin's algorithm in C, line for line: greedy LZ77 over a 32 KB window,
// hash chains of 3-byte prefixes (CHAIN long at most), one block of the fixed Huffman codes. Its
// output must equal burin's byte for byte (bench/crunch/run.sh checks). Also CRC-32 a byte at a
// time, as apps/crunch's crc32, and zlib and libdeflate for context (other algorithms).
//   cc -O2 deflate.c -lz -ldeflate -o deflate     (plain C)
//   … -fsanitize=signed-integer-overflow,bounds -fsanitize-trap=all   (checked, near burin's)
//   ./deflate <input> [out] [runs]
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <zlib.h>
#include <libdeflate.h>

#define CHAIN 32

static int64_t min64(int64_t a, int64_t b) { return a < b ? a : b; }

// the compressed bytes into out (room for n + n / 8 + 64); their count
static size_t deflate_fixed(const uint8_t *t, int64_t n, uint8_t *out) {
    static int32_t head[32768], prev[32768];
    memset(head, 0, sizeof head);
    memset(prev, 0, sizeof prev);
    int64_t lit_code[288], lit_bits[288];
    for (int64_t s = 0; s < 288; s++) {
        int64_t code, bits;
        if (s < 144) { code = 48 + s; bits = 8; }
        else if (s < 256) { code = 400 + s - 144; bits = 9; }
        else if (s < 280) { code = s - 256; bits = 7; }
        else { code = 192 + s - 280; bits = 8; }
        int64_t r = 0;
        for (int64_t k = 0; k < bits; k++) r = (r << 1) | ((code >> k) & 1);
        lit_code[s] = r;
        lit_bits[s] = bits;
    }
    int64_t len_extra[29], len_base[29], base = 3;
    for (int64_t c = 0; c < 29; c++) {
        int64_t x = (c >= 8 && c < 28) ? (c - 4) / 4 : 0;
        len_extra[c] = x;
        len_base[c] = base;
        base += (int64_t)1 << x;
    }
    len_base[28] = 258;
    int64_t len_sym[259] = {0};
    for (int64_t c = 0; c < 29; c++) {
        int64_t hi = c < 28 ? len_base[c + 1] : 259;
        for (int64_t l = len_base[c]; l < min64(hi, 259); l++) len_sym[l] = c;
    }
    int64_t dist_extra[30], dist_base[30];
    base = 1;
    for (int64_t c = 0; c < 30; c++) {
        int64_t x = c >= 4 ? (c - 2) / 2 : 0;
        dist_extra[c] = x;
        dist_base[c] = base;
        base += (int64_t)1 << x;
    }
    uint8_t dist_lo[256] = {0}, dist_hi[256] = {0};
    for (int64_t c = 0; c < 30; c++)
        for (int64_t d = dist_base[c] - 1; d < min64(dist_base[c] - 1 + ((int64_t)1 << dist_extra[c]), 32768); d++) {
            if (d < 256) dist_lo[d] = (uint8_t)c;
            dist_hi[d >> 7] = (uint8_t)c;
        }
    int64_t dist_rev[30];
    for (int64_t c = 0; c < 30; c++) {
        int64_t r = 0;
        for (int64_t k = 0; k < 5; k++) r = (r << 1) | ((c >> k) & 1);
        dist_rev[c] = r;
    }

    size_t o = 0;
    int64_t acc = 0, have = 0, q_code[4], q_bits[4], q = 0;
    q_code[0] = 1; q_bits[0] = 1; q_code[1] = 1; q_bits[1] = 2; q = 2;
    int64_t i = 0;
    int finished = 0;
    for (;;) {
        for (int64_t k = 0; k < q; k++) {
            acc |= q_code[k] << have;
            have += q_bits[k];
            while (have >= 8) {
                out[o++] = (uint8_t)(acc & 255);
                acc >>= 8;
                have -= 8;
            }
        }
        q = 0;
        if (i >= n) {
            if (finished) break;
            q_code[0] = lit_code[256];
            q_bits[0] = lit_bits[256];
            q = 1;
            finished = 1;
            continue;
        }
        int64_t best = 0, best_d = 0;
        if (i + 2 < n) {
            int64_t h = (((int64_t)t[i] << 10) ^ ((int64_t)t[i + 1] << 5) ^ t[i + 2]) & 32767;
            int64_t cand = head[h], chain = 0, most = min64(258, n - i);
            while (cand > 0 && chain < CHAIN) {
                int64_t p = cand - 1, d = i - p;
                if (d > 32768) break;
                int64_t l = 0;
                while (l < most && t[p + l] == t[i + l]) l++;
                if (l > best) {
                    best = l;
                    best_d = d;
                    if (l == most) break;
                }
                cand = prev[p & 32767];
                chain++;
            }
        }
        int64_t step = 1;
        if (best >= 3) {
            int64_t c = len_sym[best];
            q_code[0] = lit_code[257 + c];
            q_bits[0] = lit_bits[257 + c];
            q_code[1] = best - len_base[c];
            q_bits[1] = len_extra[c];
            int64_t dc = best_d - 1 < 256 ? dist_lo[best_d - 1] : dist_hi[(best_d - 1) >> 7];
            q_code[2] = dist_rev[dc];
            q_bits[2] = 5;
            q_code[3] = best_d - dist_base[dc];
            q_bits[3] = dist_extra[dc];
            q = 4;
            step = best;
        } else {
            int64_t b = t[i];
            q_code[0] = lit_code[b];
            q_bits[0] = lit_bits[b];
            q = 1;
        }
        for (int64_t j = i; j < i + step; j++)
            if (j + 2 < n) {
                int64_t h2 = (((int64_t)t[j] << 10) ^ ((int64_t)t[j + 1] << 5) ^ t[j + 2]) & 32767;
                prev[j & 32767] = head[h2];
                head[h2] = (int32_t)(j + 1);
            }
        i += step;
    }
    if (have > 0) out[o++] = (uint8_t)(acc & 255);
    return o;
}

static int64_t crc32_bytes(const uint8_t *t, int64_t n) {
    int64_t table[256];
    for (int64_t i = 0; i < 256; i++) {
        int64_t c = i;
        for (int k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >> 1)) : (c >> 1);
        table[i] = c;
    }
    int64_t crc = 0xFFFFFFFF;
    for (int64_t i = 0; i < n; i++) crc = table[(crc ^ t[i]) & 255] ^ (crc >> 8);
    return crc ^ 0xFFFFFFFF;
}

static double now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

static int cmp(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return x < y ? -1 : x > y;
}

// the median of `runs` timings of `body`
#define MEDIAN(runs, out_s, body) do { \
    double ts_[64]; \
    for (int r_ = 0; r_ < (runs); r_++) { double t0_ = now(); body; ts_[r_] = now() - t0_; } \
    qsort(ts_, (runs), sizeof(double), cmp); \
    (out_s) = ts_[(runs) / 2]; \
} while (0)

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: deflate <input> [out] [runs]\n"); return 64; }
    FILE *f = fopen(argv[1], "rb");
    if (!f) { perror(argv[1]); return 1; }
    fseek(f, 0, SEEK_END);
    int64_t n = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *t = malloc(n), *out = malloc(n + n / 8 + 64), *zo = malloc(compressBound(n));
    if (fread(t, 1, n, f) != (size_t)n) return 1;
    fclose(f);
    int runs = argc > 3 ? atoi(argv[3]) : 9;
    if (runs > 64) runs = 64;
    double mb = n / 1e6, s;
    size_t m = 0;
    MEDIAN(runs, s, m = deflate_fixed(t, n, out));
    if (argc > 2 && argv[2][0]) { FILE *o = fopen(argv[2], "wb"); fwrite(out, 1, m, o); fclose(o); }
    double crc_s;
    int64_t crc = 0;
    MEDIAN(runs, crc_s, crc = crc32_bytes(t, n));
    // small inputs: a call's own cost (its tables, built each call as burin's source does)
    int64_t sink = 0, small = n < 64 ? n : 64, kb = n < 1024 ? n : 1024;
    double crc64_s, d1k_s;
    MEDIAN(runs, crc64_s, for (int k = 0; k < 100000; k++) { sink += crc32_bytes(t, small); __asm__ volatile("" : : "r"(sink) : "memory"); });
    MEDIAN(runs, d1k_s, for (int k = 0; k < 2000; k++) { sink += (int64_t)deflate_fixed(t, kb, out); __asm__ volatile("" : : "r"(sink) : "memory"); });
    printf("{\"impl\":\"c\",\"bytes\":%lld,\"deflate_mb_s\":%.1f,\"compressed\":%zu,\"crc32_mb_s\":%.1f,\"crc32\":%lld,\"crc32_64b_ns\":%.0f,\"deflate_1k_ns\":%.0f", (long long)n, mb / s, m, mb / crc_s, (long long)crc, crc64_s * 1e9 / 100000, d1k_s * 1e9 / 2000);
    // context: zlib (levels 1 and 6) and libdeflate (1 and 6), raw DEFLATE; zlib's crc32
    for (int level = 1; level <= 6; level += 5) {
        size_t zn = 0;
        MEDIAN(runs, s, {
            z_stream z = {0};
            deflateInit2(&z, level, Z_DEFLATED, -15, 8, Z_DEFAULT_STRATEGY);
            z.next_in = t; z.avail_in = n; z.next_out = zo; z.avail_out = compressBound(n);
            deflate(&z, Z_FINISH);
            zn = z.total_out;
            deflateEnd(&z);
        });
        printf(",\"zlib%d_mb_s\":%.1f,\"zlib%d_compressed\":%zu", level, mb / s, level, zn);
        struct libdeflate_compressor *c = libdeflate_alloc_compressor(level);
        size_t ln = 0;
        MEDIAN(runs, s, ln = libdeflate_deflate_compress(c, t, n, zo, compressBound(n)));
        libdeflate_free_compressor(c);
        printf(",\"libdeflate%d_mb_s\":%.1f,\"libdeflate%d_compressed\":%zu", level, mb / s, level, ln);
    }
    uLong zc = 0;
    MEDIAN(runs, s, zc = crc32(0L, t, n));
    printf(",\"zlib_crc32_mb_s\":%.1f}\n", mb / s);
    (void)zc;
    return 0;
}
