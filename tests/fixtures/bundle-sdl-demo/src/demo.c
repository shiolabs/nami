/* Opens a window, draws in it with OpenGL for a while, and says which
 * video driver and which OpenGL drew: `sdl-demo [frames]` (0: until closed). */
#include <SDL.h>
#include <stdio.h>
#include <stdlib.h>

typedef const unsigned char *(*get_string_fn)(unsigned int);
typedef void (*clear_color_fn)(float, float, float, float);
typedef void (*clear_fn)(unsigned int);
#define GL_VENDOR 0x1F00
#define GL_RENDERER 0x1F01
#define GL_VERSION 0x1F02
#define GL_COLOR_BUFFER_BIT 0x4000

int main(int argc, char **argv) {
    int frames = argc > 1 ? atoi(argv[1]) : 0;
    if (SDL_Init(SDL_INIT_VIDEO) != 0) {
        fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
        return 1;
    }
    SDL_version v;
    SDL_GetVersion(&v);
    printf("SDL %d.%d.%d, video driver: %s\n", v.major, v.minor, v.patch, SDL_GetCurrentVideoDriver());
    SDL_Window *w = SDL_CreateWindow("nami bundle: SDL demo", SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED, 640, 400, SDL_WINDOW_OPENGL | SDL_WINDOW_SHOWN);
    if (!w) {
        fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError());
        return 1;
    }
    SDL_GLContext ctx = SDL_GL_CreateContext(w);
    if (!ctx) {
        fprintf(stderr, "SDL_GL_CreateContext: %s\n", SDL_GetError());
        return 1;
    }
    get_string_fn get_string = (get_string_fn)SDL_GL_GetProcAddress("glGetString");
    clear_color_fn clear_color = (clear_color_fn)SDL_GL_GetProcAddress("glClearColor");
    clear_fn clear = (clear_fn)SDL_GL_GetProcAddress("glClear");
    printf("GL_VENDOR: %s\nGL_RENDERER: %s\nGL_VERSION: %s\n", get_string(GL_VENDOR), get_string(GL_RENDERER), get_string(GL_VERSION));
    fflush(stdout);
    int n = 0;
    for (int running = 1; running && (frames == 0 || n < frames); n++) {
        SDL_Event e;
        while (SDL_PollEvent(&e))
            if (e.type == SDL_QUIT || (e.type == SDL_KEYDOWN && e.key.keysym.sym == SDLK_ESCAPE))
                running = 0;
        float t = (n % 240) / 240.0f;
        clear_color(0.1f + 0.5f * t, 0.3f, 0.6f - 0.4f * t, 1.0f);
        clear(GL_COLOR_BUFFER_BIT);
        SDL_GL_SwapWindow(w);
        SDL_Delay(16);
    }
    printf("drew %d frames\n", n);
    SDL_GL_DeleteContext(ctx);
    SDL_DestroyWindow(w);
    SDL_Quit();
    return 0;
}
