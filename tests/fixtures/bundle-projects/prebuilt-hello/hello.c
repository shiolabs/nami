#include <stdio.h>
const char *greet(void);
int main(void) {
    printf("prebuilt-hello: hello, %s\n", greet());
    return 0;
}
