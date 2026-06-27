/* examples/export/digest_driver.c
 *
 * A C program calling INTO a Sentinel library that returns OWNED byte buffers
 * (ADR 0059 Phase 1b, A7). It #includes the snc-generated header and links the
 * snc-built `.a`. The harness test (crates/sentinel-driver/tests/export.rs)
 * builds the library + header from digest_lib.sentinel, compiles this driver
 * against them, runs it, and asserts exit 42 — proving a foreign caller gets a
 * Sentinel verified-constant-time SHA-256 over a plain C buffer API, and that
 * the owned-`[u8]` return ABI (out-params + sentinel_free_bytes) round-trips.
 *
 * The owned-return contract: a `[u8]`-returning export takes two trailing
 * out-params `(uint8_t** out_data, int64_t* out_len)` and writes a heap buffer
 * the caller OWNS; the caller releases it with sentinel_free_bytes.
 */
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include "sentineldigest.h"

int main(void) {
    /* SHA-256("abc") = the NIST FIPS 180-4 sample vector. */
    const unsigned char msg[] = "abc";
    static const unsigned char want[32] = {
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
        0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad
    };

    uint8_t *digest = NULL;
    int64_t dlen = 0;
    sha256_oneshot(msg, 3, &digest, &dlen);
    int digest_ok = (dlen == 32) && (memcmp(digest, want, 32) == 0);
    sentinel_free_bytes(digest); /* C owns the buffer; release it */

    /* A variable-length owned return: 5 copies of 'Z'. */
    uint8_t *rep = NULL;
    int64_t rlen = 0;
    repeat_byte((int64_t)'Z', 5, &rep, &rlen);
    int rep_ok = (rlen == 5) && rep[0] == 'Z' && rep[4] == 'Z';
    sentinel_free_bytes(rep);

    printf("sha256(\"abc\") len=%lld ok=%d\n", (long long)dlen, digest_ok);
    printf("repeat_byte('Z',5) len=%lld ok=%d\n", (long long)rlen, rep_ok);

    if (digest_ok && rep_ok) {
        return 42;
    }
    return 1;
}
