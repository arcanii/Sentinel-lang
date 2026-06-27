/* examples/export/crypto_driver.c
 *
 * A C program calling INTO a MULTI-MODULE Sentinel crypto library (ADR 0059
 * A8). The library entry (crypto_lib.sentinel) `use`s the real std/security
 * SHA-256 + HMAC modules; `snc build --lib` merges the `use` graph into one
 * archive. This driver #includes the generated header, links the `.a`, calls
 * both exports over plain C buffers, checks them against canonical vectors, and
 * frees each owned result with sentinel_free_bytes — a foreign caller getting
 * the whole verified-constant-time crypto suite over a plain C ABI. The harness
 * test (crates/sentinel-driver/tests/export.rs) asserts exit 42.
 */
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include "sentinelcrypto.h"

int main(void) {
    /* SHA-256("abc") — NIST FIPS 180-4. */
    const unsigned char abc[] = "abc";
    static const unsigned char sha_want[32] = {
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
        0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad
    };
    uint8_t *digest = NULL;
    int64_t dlen = 0;
    sha256_oneshot(abc, 3, &digest, &dlen);
    int sha_ok = (dlen == 32) && (memcmp(digest, sha_want, 32) == 0);
    sentinel_free_bytes(digest);

    /* HMAC-SHA256 — RFC 4231 Test Case 1: key = 0x0b*20, data = "Hi There". */
    unsigned char key[20];
    memset(key, 0x0b, sizeof key);
    const unsigned char data[] = "Hi There";
    static const unsigned char hmac_want[32] = {
        0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53,
        0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1, 0x2b,
        0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7,
        0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32, 0xcf, 0xf7
    };
    uint8_t *tag = NULL;
    int64_t tlen = 0;
    hmac_sha256_oneshot(key, 20, data, 8, &tag, &tlen);
    int hmac_ok = (tlen == 32) && (memcmp(tag, hmac_want, 32) == 0);
    sentinel_free_bytes(tag);

    printf("sha256(\"abc\")  len=%lld ok=%d\n", (long long)dlen, sha_ok);
    printf("hmac_sha256 TC1 len=%lld ok=%d\n", (long long)tlen, hmac_ok);

    if (sha_ok && hmac_ok) {
        return 42;
    }
    return 1;
}
