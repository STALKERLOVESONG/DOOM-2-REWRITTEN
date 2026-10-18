/* Minimal single-threaded SDL stand-in for the reference OPL harness. */
#ifndef REFOPL_SDL_H
#define REFOPL_SDL_H
#include <stdint.h>
#include <stddef.h>
typedef uint8_t Uint8;
typedef uint16_t Uint16;
typedef uint32_t Uint32;
typedef int16_t Sint16;
typedef struct SDL_mutex { int dummy; } SDL_mutex;
typedef struct SDL_cond { int dummy; } SDL_cond;
SDL_mutex *SDL_CreateMutex(void);
void SDL_DestroyMutex(SDL_mutex *m);
int SDL_LockMutex(SDL_mutex *m);
int SDL_UnlockMutex(SDL_mutex *m);
SDL_cond *SDL_CreateCond(void);
void SDL_DestroyCond(SDL_cond *c);
int SDL_CondSignal(SDL_cond *c);
/* Single-threaded: waiting runs the emulator forward instead. */
int SDL_CondWait(SDL_cond *c, SDL_mutex *m);
void SDL_Delay(Uint32 ms);
Uint32 SDL_GetTicks(void);
#endif
