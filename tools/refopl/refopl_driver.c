// Deterministic OPL driver for the reference harness: the same timing and
// emulator calls as Chocolate Doom's opl/opl_sdl.c (OPL3_Reset at the mix
// rate, buffered register writes, callbacks fired between sample blocks),
// but pulled synchronously by refopl_render() instead of an audio thread.
// Every register write is logged with the sample position it lands at.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "SDL.h"
#include "opl.h"
#include "opl_internal.h"
#include "opl_queue.h"
#include "opl3.h"

typedef struct { unsigned int rate; unsigned int enabled; unsigned int value; uint64_t expire_time; } opl_timer_t;

static opl_callback_queue_t *callback_queue;
static uint64_t current_time;
static uint64_t samples_done;
static int paused;
static uint64_t pause_offset;
static opl3_chip opl_chip;
static int register_num;
static opl_timer_t timer1 = { 12500, 0, 0, 0 };
static opl_timer_t timer2 = { 3125, 0, 0, 0 };
static unsigned int mixing_freq;

FILE *refopl_log;

SDL_mutex *SDL_CreateMutex(void) { static SDL_mutex m; return &m; }
void SDL_DestroyMutex(SDL_mutex *m) { (void)m; }
int SDL_LockMutex(SDL_mutex *m) { (void)m; return 0; }
int SDL_UnlockMutex(SDL_mutex *m) { (void)m; return 0; }
SDL_cond *SDL_CreateCond(void) { static SDL_cond c; return &c; }
void SDL_DestroyCond(SDL_cond *c) { (void)c; }
int SDL_CondSignal(SDL_cond *c) { (void)c; return 0; }
void SDL_Delay(Uint32 ms) { (void)ms; }
Uint32 SDL_GetTicks(void) { return (Uint32)(current_time / 1000); }

void refopl_render(int16_t *out, unsigned int buffer_samples);

int SDL_CondWait(SDL_cond *c, SDL_mutex *m)
{
    // OPL_Delay waits for a callback: run the "audio thread" a little.
    int16_t scratch[64 * 2];
    (void)c; (void)m;
    refopl_render(scratch, 64);
    return 0;
}

static void AdvanceTime(unsigned int nsamples)
{
    opl_callback_t callback;
    void *callback_data;
    uint64_t us = ((uint64_t) nsamples * OPL_SECOND) / mixing_freq;
    current_time += us;
    samples_done += nsamples;
    if (paused)
        pause_offset += us;
    while (!OPL_Queue_IsEmpty(callback_queue)
        && current_time >= OPL_Queue_Peek(callback_queue) + pause_offset)
    {
        if (!OPL_Queue_Pop(callback_queue, &callback, &callback_data))
            break;
        callback(callback_data);
    }
}

// OPL_Mix_Callback without SDL: fills `buffer_samples` stereo frames.
void refopl_render(int16_t *out, unsigned int buffer_samples)
{
    unsigned int filled = 0;
    while (filled < buffer_samples)
    {
        uint64_t nsamples;
        if (paused || OPL_Queue_IsEmpty(callback_queue))
        {
            nsamples = buffer_samples - filled;
        }
        else
        {
            uint64_t next_callback_time = OPL_Queue_Peek(callback_queue) + pause_offset;
            nsamples = (next_callback_time - current_time) * mixing_freq;
            nsamples = (nsamples + OPL_SECOND - 1) / OPL_SECOND;
            if (nsamples > buffer_samples - filled)
                nsamples = buffer_samples - filled;
        }
        OPL3_GenerateStream(&opl_chip, out + filled * 2, nsamples);
        filled += nsamples;
        AdvanceTime(nsamples);
    }
}

uint64_t refopl_samples(void) { return samples_done; }

static int Init(unsigned int port_base)
{
    (void)port_base;
    paused = 0;
    pause_offset = 0;
    callback_queue = OPL_Queue_Create();
    current_time = 0;
    samples_done = 0;
    mixing_freq = opl_sample_rate;
    OPL3_Reset(&opl_chip, mixing_freq);
    return 1;
}

static void Shutdown(void) { OPL_Queue_Destroy(callback_queue); }

static unsigned int PortRead(opl_port_t port)
{
    unsigned int result = 0;
    if (port == OPL_REGISTER_PORT_OPL3)
        return 0xff;
    if (timer1.enabled && current_time > timer1.expire_time)
        result |= 0x80 | 0x40;
    if (timer2.enabled && current_time > timer2.expire_time)
        result |= 0x80 | 0x20;
    return result;
}

static void CalculateEndTime(opl_timer_t *timer)
{
    if (timer->enabled)
    {
        int tics = 0x100 - timer->value;
        timer->expire_time = current_time + ((uint64_t) tics * OPL_SECOND) / timer->rate;
    }
}

static void WriteRegister(unsigned int reg_num, unsigned int value)
{
    if (refopl_log)
        fprintf(refopl_log, "%llu %03x %02x\n", (unsigned long long) samples_done, reg_num, value);
    switch (reg_num)
    {
        case OPL_REG_TIMER1: timer1.value = value; CalculateEndTime(&timer1); break;
        case OPL_REG_TIMER2: timer2.value = value; CalculateEndTime(&timer2); break;
        case OPL_REG_TIMER_CTRL:
            if (value & 0x80) { timer1.enabled = 0; timer2.enabled = 0; }
            else
            {
                if ((value & 0x40) == 0) { timer1.enabled = (value & 0x01) != 0; CalculateEndTime(&timer1); }
                if ((value & 0x20) == 0) { timer2.enabled = (value & 0x02) != 0; CalculateEndTime(&timer2); }
            }
            break;
        default:
            OPL3_WriteRegBuffered(&opl_chip, reg_num, value);
            break;
    }
}

static void PortWrite(opl_port_t port, unsigned int value)
{
    if (port == OPL_REGISTER_PORT) register_num = value;
    else if (port == OPL_REGISTER_PORT_OPL3) register_num = value | 0x100;
    else if (port == OPL_DATA_PORT) WriteRegister(register_num, value);
}

static void SetCallback(uint64_t us, opl_callback_t callback, void *data)
{
    OPL_Queue_Push(callback_queue, callback, data, current_time - pause_offset + us);
}
static void ClearCallbacks(void) { OPL_Queue_Clear(callback_queue); }
static void Lock(void) {}
static void Unlock(void) {}
static void SetPaused(int p) { paused = p; }
static void AdjustCallbacks(float factor) { OPL_Queue_AdjustCallbacks(callback_queue, current_time, factor); }

opl_driver_t opl_sdl_driver =
{
    "refopl", Init, Shutdown, PortRead, PortWrite, SetCallback,
    ClearCallbacks, Lock, Unlock, SetPaused, AdjustCallbacks,
};
