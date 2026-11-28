#include <stdio.h>
#include <zlib.h>

/* Decompress gzip from stdin to stdout. */
int main(int argc, char **argv) {
    if (argc > 1) { printf("zcat-lite for %s, zlib %s\n", GREETING, zlibVersion()); return 0; }
    gzFile in = gzdopen(0, "rb");
    char buf[65536];
    int n;
    while ((n = gzread(in, buf, sizeof buf)) > 0) fwrite(buf, 1, n, stdout);
    return gzclose(in) == Z_OK ? 0 : 1;
}
