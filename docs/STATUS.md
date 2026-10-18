# Status

Updated 2026-10-07 (mods round).

## Project direction

- **The goal:** a clean, dependency-light, Rust-native Doom engine. It should be broadly faithful to classic Doom and serve as a foundation for new rendering features, gameplay systems, weapons, monsters, maps, effects and scripting.
- **Chocolate Doom is an oracle, not a spec.** Use it to catch major correctness bugs. Minor visual differences are acceptable.
- **Vanilla quirks:** reproduce them only when they materially affect gameplay or compatibility. Prefer clear, extensible code over copying how the C engine was implemented.
- **Done means playable and extensible,** not bit-exact.

## Works

- **Content:** WAD loading with PWAD overrides; palettes, colormaps, patches, flats, textures, sprites, sounds, music, maps (vanilla format); IWAD identification.
- **Mod formats (2026-10-07):**
  - **PK3:** zip archives with stored/deflate entries and zip64. They mount with GZDoom's directory rules (root lumps, namespace directories, `graphics/sounds/music`, `maps/*.wad`, root WADs, `filter/<game>/`, full-path lookup). Entries are decompressed on first use. Deflate is our own decoder.
  - **UDMF:** all common namespaces. Doom-namespace specials are kept; Hexen-style (`zdoom`) specials are translated to Doom ones where an equivalent exists (doors incl. locked, lifts, floors, ceilings, crushers, stairs, exits).
  - **Map data kept from UDMF:** per-part sidedef offsets, side/plane light, flat panning/scale/rotation (perspective renderer), thing heights, skill/ambush/single-player flags.
  - **Nodes:** XNOD/ZNOD/XGLN/ZGLN/XGL2/ZGL2/XGL3/ZGL3. GL minisegs are dropped, seg angles and offsets computed. Maps without BLOCKMAP get one built; a missing REJECT rejects nothing.
  - **Level limits:** indices are 32-bit throughout, so there is no 32K/64K limit.
  - **Graphics:** PNG decoding (all colour types, Adam7, `grAb` offsets) mapped to the palette and converted to Doom pictures, so both renderers draw them. Standalone textures from TX_START/`textures/`. A `TEXTURES` lump subset including X/YScale (the perspective renderer honours scale). Wall textures on floors and flats on walls work, as in ZDoom.
  - **DECORATE subset:** new actors and replacements, inheritance from Doom classes, basic properties and flags, states with Doom action functions, gotos, `#include`.
  - **MAPINFO/ZMAPINFO/UMAPINFO subset:** level name, sky, music, next/secret next.
  - **Tolerant loading:** unknown thing types are skipped with a warning.
  - **Verified:** MyHouse.wad (UDMF, XGL3 nodes, PNG textures, DECORATE) loads and renders on Doom II. The same content repacked as a PK3 renders identically (`tests/mods.rs`).
- **Playsim:** complete vanilla single-player gameplay. The 4 IWAD demos and 72 synthetic demos stay in sync with Chocolate Doom.
- **Classic renderer:** the vanilla column/span renderer. It matches Chocolate Doom frame for frame on the IWAD demos at the default view size.
- **Perspective renderer (default):** a polygon renderer through a free camera with yaw and **true pitch** (freelook).
  - Walls, flats, sky, sprites, masked midtextures are all perspective correct; nothing is stretched.
  - It uses the vanilla pegging rules, light tables, sprite rotations, fuzz and translations.
  - At pitch 0 it lines up with the classic renderer.
  - Sky follows vanilla's rules (sky ceilings are open to the sky; the both-sky upper-wall rule; missing upper/lower textures show the neighbouring flat as in vanilla). Above and below the sky texture it fades into a solid colour.
  - ~2–4 ms per frame at 320×200 on the IWAD maps (release build, one core).
- **Sound:** sound effects with vanilla `s_sound.c` logic (8 channels, priorities, distance attenuation and stereo separation); OPL2 music from MUS + GENMIDI, emulating the DMX driver.
- **Presentation:** title pages and the demo loop, menus (new game, episode, skill, options, sound volume, read-this, quit/end-game prompts), status bar and face, full-screen HUD, view border, HUD messages, automap (with `iddt`), intermission tally and episode maps, finales (text, art, bunny scroll, cast call), pause, palette flashes. All drawn in screen space from WAD graphics.
- **Bevy host:** window, keyboard/mouse at 35 Hz, auto mouse capture, mouse/keyboard freelook, audio output, 4:3 presentation, fullscreen, screenshots.
  - The previous host version compiled against real Bevy 0.16.1 on Windows. This version (audio, freelook, key routing) was type-checked against a stub here, because crates.io is unreachable from this container; it still needs a real build.

## Verified against Chocolate Doom (2026-10-07)

- **Lighting.** `examples/light_compare.rs` renders demo frames with both renderers through special colormap tables that reveal, per pixel, the texel and the light level (colormap) used. Where both sample the same texel, the perspective renderer picks the same colormap as vanilla for 98.4–98.7% of pixels (mean difference 0.01 levels) at every view size and detail level.
  - Fixed: in low detail the perspective renderer drew walls at half height and lit everything with a 4× scale error (far too bright).
  - Fixed: pixel sample positions now follow vanilla's `centerx`/`centery`, so the same texels are sampled (72% vs 55%).
  - Fixed: the sky always uses colormap 0, as in vanilla.
  - Added: vanilla gamma correction (F11, `gammatable`).
  - Added: Chocolate Doom's presentation scaling. The 200 rows are scaled by 1.2 for the 4:3 display, and plain nearest scaling (the previous host) makes rows uneven, which breaks up the light bands. The host now upscales by an integer factor, then resizes smoothly.
  - Regression test: `crates/doom_game/tests/lighting.rs`.
- **Music.** `tools/refopl` builds Chocolate Doom's `i_oplmusic.c` + `mus2mid.c` + `midifile.c` + Nuked OPL3 into a deterministic renderer that logs every OPL register write.
  - Our MUS player issues the **same register writes in the same order** for all 32 songs in the IWAD (120 s each, at volumes 64 and 127): notes, instruments, volumes, pitch bends.
  - Replaying the reference's writes through our `Opl` reproduces its PCM **bit for bit**. This needed two fixes: Nuked's buffered register writes (2 native samples apart) and the chip's one-sample channel pipeline delay. Before these, there was a −17 dB error signal: inaudible phase differences, not wrong notes.
  - Timing: we tick at exactly 140 Hz like DMX. Chocolate drifts about 0.06% slow, because each event is scheduled from when the previous callback ran.
  - Regression test: `crates/doom_audio/tests/opl_reference.rs` with fixtures from `tools/refopl/gen_fixtures.sh`.
  - What the OPL path cannot do: sound like General MIDI / SoundFont playback (Roland SC-55, GZDoom's FluidSynth default). That needs a separate synthesizer; see the roadmap.

## Corpses at map start

- The corpses at map start are **authored map things**, not engine-created.
  - E1M1 has dead marines (doomednum 15), bloody messes (10/12) and pools of blood (24).
- All monsters are alive at spawn and still alive after 10 seconds (checked on E1M1–3 and E2M1).
- Demo state hashes from tic 0 match Chocolate Doom.
- `crates/doom_game/examples/corpse_probe.rs` lists them per map.

## Known limitations

- **Freelook is presentation only.** Hitscan and projectiles still aim with vanilla autoaim, not along the view pitch.
- **Perspective renderer:**
  - Sprites are clipped by the depth buffer, so a sprite partly inside a wall is cut where vanilla would draw it whole.
  - There is no occlusion culling beyond the view cone, so very large maps may be slower.
- **Audio:**
  - OPL rhythm mode and 4-op voices are not emulated (DMX doesn't use them).
  - There is no PC speaker, MIDI-file, General MIDI/SoundFont, or OGG/MP3/MOD music.
  - Sound lumps are resampled linearly.
  - The mixer runs at 44.1 kHz like Chocolate Doom. If the output device runs at another rate, Bevy's audio backend (rodio) resamples with plain linear interpolation.
- **Not implemented:** savegames (the save/load menus say so), multiplayer, DEHACKED.
- **Mods:**
  - Not supported yet: ZScript, ACS (scripted events, doors and lights do nothing), Hexen-style specials without a Doom equivalent (teleports by TID, Line_Horizon, mirrors, Transfer_Heights, polyobjects), 3D floors, slopes, portals, translucency (`alpha`), dynamic lights, OGG/MP3/WAV/FLAC sounds and music, MIDI music.
  - A UDMF map without nodes cannot load: there is no node builder yet.
  - Long texture names (more than 8 characters) are truncated.
  - Texture scale is ignored by the classic renderer.
  - All PNG sprites and textures are decoded at startup, so very large PK3s take a few seconds to load.
- **IWAD quirk:** the bundled `doom1.wad` is Ultimate Doom but lacks `M_EPI4`, so the menu draws "Thy Flesh Consumed" in the HUD font.

## Roadmap

Agreed order: baseline authenticity (lighting, music: done above), then the formats needed to load `MyHouse.pk3`, then settings, then advanced rendering.

1. **Mod formats, part 2 (next):**
   - an ACS interpreter (BEHAVIOR) with the Hexen special set, which most UDMF maps need for interaction;
   - OGG/WAV sound and music, plus MIDI on the OPL player;
   - a node builder for UDMF maps shipped without nodes;
   - DEHACKED;
   - ZScript is a large separate project (a language VM).
2. **Settings / CVars:** a typed CVar registry with config file persistence. First users: freelook on/off (pitch locked to the horizon), mouse look invert, gamma, renderer choice, scaling mode, music device.
3. **Optional General MIDI music:** a SoundFont (SF2) synthesizer as an alternative to OPL, chosen by a CVar.
4. **Advanced rendering (perspective renderer):** voxel models (KVX, then MagicaVoxel `.vox`) as a sprite replacement; MD3 / glTF models tied to actor definitions; later dynamic lights, slopes and 3D floors.
5. **Also outstanding:** savegames, optional vertical aiming along the view pitch.

## Tooling

- **Frame dumps:** `cargo run -p doom_render --release --example demo_frames -- <iwad> DEMO1 <out> 11 0 70 400` writes PNGs.
- **Pitch shots:** `cargo run -p doom_game --release --example pitch_shots -- <iwad> <out> E1M1` renders classic vs perspective at several pitches, plus sky views.
- **UI shots:** `cargo run -p doom_game --release --example ui_shots -- <iwad> <out>` saves every presentation state.
- **Reference oracle:** see `tools/refdoom`. Fixtures cover demo sync and frame hashes.
- **Music reference:** `tools/refopl` (`build.sh <chocolate checkout>`, then `compare_logs.py` against `cargo run -p doom_audio --release --example music_trace`).
- **Mod probe:** `cargo run -p doom_game --release --example mod_probe -- <out> <iwad> [mods...] [--map MAP01] [--walk TICS]` reports content, warnings and screenshots.
- **Lighting comparison:** `cargo run -p doom_game --release --example light_compare -- <iwad> [out dir]` (`LOW_DETAIL=1`, `BLOCKS=n`).
- **Open renderer item, parked:** a 1-pixel low-detail difference in two frames of `full_low` DEMO2/DEMO3.
