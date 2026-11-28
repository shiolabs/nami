#include <stdio.h>
#include <zlib.h>

int main(void) {
    printf("hello-meson %s: hello, zlib %s\n", VERSION, zlibVersion());
    return 0;
}
