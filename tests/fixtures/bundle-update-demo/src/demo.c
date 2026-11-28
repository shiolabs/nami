/* update-demo: says its version (the test changes it) and zlib's. */
#include <stdio.h>
#include <zlib.h>

#define VERSION "1.0"

int main(void) {
    printf("update-demo %s (zlib %s)\n", VERSION, zlibVersion());
    return 0;
}
