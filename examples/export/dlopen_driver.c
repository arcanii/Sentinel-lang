/* examples/export/dlopen_driver.c
 *
 * A C program that loads a Sentinel SHARED library at RUNTIME via dlopen +
 * dlsym (ADR 0059 A9) — exactly what Python's ctypes (`CDLL(path)`) and any
 * language's dynamic-FFI do. It dlopens the snc-built `.dylib` (path in argv[1]),
 * resolves the exported `sha256_oneshot` + `sentinel_free_bytes`, calls a
 * verified-constant-time SHA-256 over a C buffer, checks the NIST "abc" vector,
 * and frees the owned result. Exercises the whole shared-library path: dynamic
 * load, the owned-[u8] return ABI, and the bundled runtime (alloc/free) all
 * through the `.dylib`. The harness test asserts exit 42.
 */
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <dlfcn.h>

typedef void (*sha256_oneshot_fn)(const uint8_t *, int64_t, uint8_t **, int64_t *);
typedef void (*free_bytes_fn)(uint8_t *);

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <path-to-dylib>\n", argv[0]);
        return 1;
    }
    void *h = dlopen(argv[1], RTLD_NOW);
    if (!h) {
        fprintf(stderr, "dlopen failed: %s\n", dlerror());
        return 1;
    }
    sha256_oneshot_fn sha256_oneshot = (sha256_oneshot_fn)dlsym(h, "sha256_oneshot");
    free_bytes_fn sentinel_free_bytes = (free_bytes_fn)dlsym(h, "sentinel_free_bytes");
    if (!sha256_oneshot || !sentinel_free_bytes) {
        fprintf(stderr, "dlsym failed: %s\n", dlerror());
        return 2;
    }

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
    int ok = (dlen == 32) && (memcmp(digest, want, 32) == 0);
    sentinel_free_bytes(digest);

    printf("dlopen sha256(\"abc\") len=%lld ok=%d\n", (long long)dlen, ok);
    return ok ? 42 : 3;
}
