#include <stdio.h>
#include <zlib.h>

const char *greet(void);

int main(void) {
    char line[128] = "(no greeting file)";
    FILE *f = fopen(DATADIR "/greeting.txt", "r");
    if (f) {
        if (fgets(line, sizeof line, f)) {
            for (char *p = line; *p; p++)
                if (*p == '\n') *p = 0;
        }
        fclose(f);
    }
    printf("cmake-hello %s: %s, %s, zlib %s\n", VERSION, line, greet(), zlibVersion());
    return f ? 0 : 1;
}
