# doom_bevy

A Rust-native Doom engine with a Bevy front end. It aims to be broadly faithful to classic Doom while staying easy to read, modify and extend into a Doom-style engine and modding platform.

## Run

```
cargo run --release                                   # title screen, demo loop, menus (finds WADS/doom1.wad)
cargo run --release -- --iwad WADS/doom1.wad --map E2M1 --skill 4
cargo run --release -- --iwad WADS/doom1.wad --pwad mymap.wad --map E1M1
cargo run --release -- --iwad WADS/doom2.wad --pwad WADS/myhouse.wad --map MAP01   # UDMF map
cargo run --release -- --iwad WADS/doom2.wad --pwad WADS/mymod.pk3                  # PK3 archive
cargo run --release -- --iwad WADS/doom1.wad --playdemo DEMO1
cargo run --release -- --classic                      # vanilla renderer, no freelook
```

Without `--map` the game starts at the title screen with the demo loop, like vanilla. Press any key for the menu and pick New Game.

**Mods.** `--pwad` takes WADs and PK3s (zip) in load order. Supported:
- UDMF maps (with ZDoom/GL nodes) and ZDoom extended nodes;
- PNG graphics and TX_START/`textures/` standalone textures;
- the `TEXTURES` lump (subset);
- DECORATE actors built from Doom's behaviour;
- MAPINFO/ZMAPINFO/UMAPINFO (name, sky, music, next map).

ZScript, ACS scripts, Hexen-style specials without a Doom equivalent, 3D floors, slopes, portals, OGG/MP3 audio and MIDI music are not supported yet. Things and specials the engine does not know are skipped with a warning instead of stopping the game.

To see what the engine makes of a mod:

```
cargo run -p doom_game --release --example mod_probe -- probe_out WADS/doom2.wad WADS/mymod.pk3
```

This prints a content inventory (archive directories and file formats, definition lumps, maps and their formats), every load warning, and saves screenshots from the first map's start to `probe_out/`.

Other options:

- `--nomonsters`, `--fast`, `--respawn`
- `--scale N` (window = N × 320×240), `--fullscreen`, `--sharp` (plain nearest-neighbour scaling instead of Chocolate Doom's integer-prescale + smooth 4:3 resize)
- `--sensitivity F` (multiplier on the menu's mouse sensitivity), `--invert-mouse`, `--mouse-move` (vertical mouse walks instead of looking)
- `--screen-size 3..11`, `--low-detail`, `--nosound`

Run with `--help` for the full list.

**Controls**

| Action | Keys |
|---|---|
| Move / strafe | W/S or ↑/↓; A/D or , and . |
| Turn | ←/→ or the mouse (captured automatically while playing) |
| Look up/down | Mouse, PageUp/PageDown; End or middle mouse centres |
| Run | Shift |
| Strafe modifier | Alt + turn keys |
| Fire | Ctrl or left mouse |
| Use | Space, E, or right mouse |
| Weapons | 1–7 |
| Menu | Esc (F1 help, F4 sound, F5 detail, F7 end game, F8 messages, F10 quit, F11 gamma) |
| Automap | Tab (F follow, G grid, M mark, C clear, −/= zoom) |
| View size | − and = |
| Pause | Pause |
| Fullscreen | Alt+Enter |
| Screenshot | F12 or PrintScreen (to `screenshots/`) |
| Classic ↔ perspective renderer | \ |

Vanilla cheat codes work: `iddqd`, `idkfa`, `idfa`, `idclip`/`idspispopd`, `idclevXY`, `iddt` (in the automap).

## Layout

| Crate | Role |
|---|---|
| `crates/doom_core` | Fixed point, angles, trig tables, RNG. No dependencies. |
| `crates/doom_content` | WAD and PK3 (zip) loading, resource lookup with overrides, graphics (patches, PNG), textures (TEXTURE1/2, TX_START, TEXTURES), maps (Doom, UDMF, ZDoom nodes), MAPINFO, deflate. |
| `crates/doom_sim` | The deterministic playsim: thinkers, mobjs, movement, AI, weapons, sector specials, DECORATE actor definitions. It is driven one tic at a time and knows nothing about rendering or the host. |
| `crates/doom_render` | Software renderers producing a 320×200 palette-indexed frame: the vanilla column renderer (`Renderer`) and a perspective renderer with a free camera (`PerspectiveRenderer`, true pitch). |
| `crates/doom_audio` | Sound: DMX sfx decoding, an OPL2 emulator (Nuked OPL3 port), the DMX MUS/GENMIDI music player, and a mixer. No dependencies. |
| `crates/doom_game` | Host-independent session: title/demo loop, menus, levels, status bar, automap, intermission, finale, sound logic (`s_sound.c`), cheats, camera pitch. Tools and tests use it headless. |
| `src/main.rs` | Bevy host: window, input, audio output, framebuffer upload. No game logic. |

The simulation is plain Rust rather than Bevy ECS on purpose. Doom's behaviour depends on the order thinkers run and objects are linked, and Bevy queries don't guarantee order. Bevy owns presentation; the engine crates own the game.

## Tests

```
cargo test --release -p doom_core -p doom_content -p doom_sim -p doom_render -p doom_audio -p doom_game
```

The tests use `WADS/doom1.wad` and skip when it is absent. Simulation and renderer tests replay demos and compare against traces and frames recorded from Chocolate Doom (`tools/refdoom`). Chocolate Doom is a regression oracle for catching real bugs, not a specification: deviations are fine when they are deliberate and documented.

See `docs/STATUS.md` for what works and what's next.

License: GPL-2.0-or-later, because the gameplay code is derived from the GPL Doom source.
