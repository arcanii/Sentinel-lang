/* examples/export/driver.c
 *
 * A C program calling INTO a Sentinel library (ADR 0059). It #includes the
 * snc-generated header and calls the exported functions; it links against the
 * snc-built `.a`. The harness test (crates/sentinel-driver/tests/export.rs)
 * builds the library + header from ct_select.sentinel, compiles this driver
 * against them, runs it, and asserts the exit code is 42 — proving a foreign
 * caller gets Sentinel's verified-constant-time primitive over a plain C ABI.
 */
#include <stdio.h>
#include "sentinelct.h"

int main(void) {
    long pick_a = ct_choose(1, 111, 222); /* cond=1 -> a = 111 */
    long pick_b = ct_choose(0, 111, 222); /* cond=0 -> b = 222 */
    long sum = sentinel_add(40, 2);       /* 42 */

    printf("ct_choose(1,111,222) = %ld\n", pick_a);
    printf("ct_choose(0,111,222) = %ld\n", pick_b);
    printf("sentinel_add(40,2)   = %ld\n", sum);

    if (pick_a == 111 && pick_b == 222 && sum == 42) {
        return 42;
    }
    return 1;
}
