// Reference OPL music renderer built from Chocolate Doom's i_oplmusic.c,
// mus2mid.c, midifile.c and Nuked OPL3. Writes the music lump's register
// writes (sample position, register, value) and a 16-bit stereo WAV.
//
//   refopl <iwad> <music lump> <seconds> <out.wav> <out.log> [rate] [volume 0-127]

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#include "doomtype.h"
#include "i_sound.h"
#include "opl.h"

extern const music_module_t music_opl_module;
extern FILE *refopl_log;
void refopl_render(int16_t *out, unsigned int buffer_samples);
uint64_t refopl_samples(void);

int snd_samplerate = 44100;
int snd_musicdevice = SNDDEVICE_SB;

static uint8_t *wad;
static long wad_len;

static uint8_t *find_lump(const char *name, int *len)
{
    int n = *(int32_t *)(wad + 4), dir = *(int32_t *)(wad + 8);
    for (int i = n - 1; i >= 0; --i)
    {
        uint8_t *e = wad + dir + 16 * i;
        if (strncasecmp((char *)e + 8, name, 8) == 0)
        {
            *len = *(int32_t *)(e + 4);
            return wad + *(int32_t *)e;
        }
    }
    return NULL;
}

// --- engine services i_oplmusic.c needs ------------------------------------

void *W_CacheLumpName(const char *name, int tag)
{
    int len;
    void *p = find_lump(name, &len);
    (void)tag;
    if (!p) { fprintf(stderr, "no lump %s\n", name); exit(1); }
    return p;
}
const char *DEH_String(const char *s) { return s; }
void I_Error(const char *error, ...) { fprintf(stderr, "I_Error: %s\n", error); exit(1); }
void *Z_Malloc(int size, int tag, void *user) { (void)tag; (void)user; return malloc(size); }
void Z_Free(void *p) { free(p); }
void I_PrintDivider(void) {}
void W_ReleaseLumpName(const char *name) { (void)name; }
void *I_Realloc(void *ptr, size_t size) { void *p = realloc(ptr, size); if (!p && size) I_Error("I_Realloc"); return p; }
uint16_t SDL_SwapBE16(uint16_t x) { return (uint16_t)((x >> 8) | (x << 8)); }
uint32_t SDL_SwapBE32(uint32_t x) { return __builtin_bswap32(x); }
void I_PrintBanner(const char *s) { (void)s; }

int main(int argc, char **argv)
{
    if (argc < 6) { fprintf(stderr, "usage: refopl <iwad> <lump> <seconds> <out.wav> <out.log> [rate]\n"); return 2; }
    int volume = 127;
    if (argc > 6) snd_samplerate = atoi(argv[6]);
    if (argc > 7) volume = atoi(argv[7]);
    FILE *f = fopen(argv[1], "rb");
    fseek(f, 0, SEEK_END); wad_len = ftell(f); fseek(f, 0, SEEK_SET);
    wad = malloc(wad_len);
    if (fread(wad, 1, wad_len, f) != (size_t)wad_len) return 1;
    fclose(f);

    int mlen;
    uint8_t *mus = find_lump(argv[2], &mlen);
    if (!mus) { fprintf(stderr, "no lump %s\n", argv[2]); return 1; }

    // The log covers everything from chip reset, so it can be replayed;
    // "# start" marks where the song (and the WAV) begins.
    refopl_log = fopen(argv[5], "w");
    if (!music_opl_module.Init()) { fprintf(stderr, "init failed\n"); return 1; }
    music_opl_module.SetMusicVolume(volume);
    void *song = music_opl_module.RegisterSong(mus, mlen);
    if (!song) return 1;

    uint64_t start = refopl_samples();
    fprintf(refopl_log, "# start %llu rate %d\n", (unsigned long long)start, snd_samplerate);
    music_opl_module.PlaySong(song, true);

    unsigned int total = (unsigned int)(atof(argv[3]) * snd_samplerate);
    int16_t *pcm = malloc(total * 4);
    for (unsigned int done = 0; done < total; done += 512)
    {
        unsigned int n = total - done < 512 ? total - done : 512;
        refopl_render(pcm + done * 2, n);
    }
    fclose(refopl_log);

    FILE *w = fopen(argv[4], "wb");
    uint32_t data_len = total * 4, riff = 36 + data_len, fmt_len = 16, rate = snd_samplerate, brate = rate * 4;
    uint16_t pcm_fmt = 1, ch = 2, align = 4, bits = 16;
    fwrite("RIFF", 1, 4, w); fwrite(&riff, 4, 1, w); fwrite("WAVEfmt ", 1, 8, w);
    fwrite(&fmt_len, 4, 1, w); fwrite(&pcm_fmt, 2, 1, w); fwrite(&ch, 2, 1, w);
    fwrite(&rate, 4, 1, w); fwrite(&brate, 4, 1, w); fwrite(&align, 2, 1, w); fwrite(&bits, 2, 1, w);
    fwrite("data", 1, 4, w); fwrite(&data_len, 4, 1, w); fwrite(pcm, 4, total, w);
    fclose(w);
    printf("rendered %s: %u samples\n", argv[2], total);
    return 0;
}
