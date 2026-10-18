# refopl: reference OPL music renderer

Builds Chocolate Doom's OPL music path (`src/i_oplmusic.c`, `mus2mid.c`,
`midifile.c`, `opl/opl3.c` and the callback queue) with a deterministic,
single-threaded driver (`refopl_driver.c`, modelled on `opl/opl_sdl.c`) that
logs every register write with the output sample it lands at.

```
tools/refopl/build.sh <chocolate-doom checkout> refopl
refopl <iwad> D_E1M1 30 ref.wav ref.log [rate] [volume]
cargo run -p doom_audio --release --example music_trace -- <iwad> D_E1M1 30 ours.wav ours.log 44100 127
python3 tools/refopl/compare_logs.py ref.log ours.log        # music logic
cargo run -p doom_audio --release --example music_trace -- replay ref.log ref.wav   # emulator
tools/refopl/gen_fixtures.sh refopl <iwad>                   # test fixtures
```
