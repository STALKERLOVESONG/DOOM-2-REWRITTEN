# Developer console and skateboard mode

## Console

Press <kbd>`</kbd> (backtick / tilde) to open it. It covers the top half of the screen and lists matching commands and variables under the input line as you type.

| Key | Action |
| --- | --- |
| Enter | run the line (`;` separates commands, `"..."` groups words) |
| Tab | complete; when several match, extend to the common prefix and list them; Tab again cycles |
| Up / Down | history |
| PgUp / PgDn | scroll back |
| Left / Right / Home / End / Backspace / Delete | edit |
| Esc or ` | close |

The game keeps running while the console is open, but the player gets no input. Use `pause` to stop it.

A word that is not a command reads or sets the console variable of that name, for example `skate_max_speed 900` or `cl_freelook`. A transient boolean variable, such as `show_collision`, toggles when it is typed on its own.

Code and tests run commands through `Session::exec("give all; god 1")`, which returns the output or an error message. New commands can be registered with `session.commands_mut().add(name, usage, help, |session, args| ...)` and given a completer with `set_completer`. The console lives in `crates/doom_game/src/console.rs` and the built-in commands in `crates/doom_game/src/session/commands.rs`.

### Commands

| Group | Commands |
| --- | --- |
| Console | `help [name]`, `cmdlist [prefix]`, `cvarlist [prefix]`, `echo`, `clear`, `toggle <cvar>`, `reset <cvar>`, `quit` |
| Items | `give all \| weapons \| ammo [n] \| health [n] \| armor [n] \| backpack \| keys \| key <blue\|yellow\|red>[card\|skull] \| <bullets\|shells\|rockets\|cells> [n] \| <weapon> \| <item or mod class> [count]` |
| Player | `god [0\|1]`, `noclip [0\|1]`, `notarget [0\|1]`, `kill [monsters\|<class>]`, `suicide`, `respawn`, `where` |
| World | `map <map>` (new game), `warp <x> <y> [z]` or `warp <map>` (keeps inventory), `nextmap`, `restart` (pistol start), `killall [class]`, `spawn` / `summon <class\|doomednum> [count]` |
| Debug | `pause`, `unpause`, `timescale <v>`, `show_player`, `show_collision`, `show_velocity`, `show_grindables`, `show_skate_state`, `skate_debug`, `r_wireframe` |
| Skate | `skate [0\|1]`, `skate_grind [radius]`, `rebuild_grindables` |
| Skate actions | `jump` / `ollie`, `push`, `brake [tics]`, `bail`, `grind`, `drop`, `spin <degrees>`, `set_speed <units/s>`, `set_velocity <x> <y> <z>` |
| Weapons | `slot <0-9>` (as pressing that slot's key) |

The skate actions are queued for the controller's next tic as `doom_sim::skate::SkateAction` values. Keyboard input becomes the same actions every tic, so the console, scripts and keys all go through `apply_action` and the controller's own state changes. An action that can't apply in the current state fails with the reason, for example `spin` on the ground or `drop` when not grinding.

## Skateboard mode

Turn it on with `skate 1` or `player_controller Skate`. The view switches to a third-person chase camera, and Doomguy rides the skateboard model (see below).

| Input | Action |
| --- | --- |
| W | push (Shift pushes harder) |
| S | brake (foot drag, or a powerslide at speed) |
| A / D | carve (lean into the turn) |
| Arrows / mouse | steer on the ground, spin in the air |
| hold Space, then release | crouch and charge, then ollie (on a rail: pop off) |
| W / S while grinding | nosegrind / 5-0 |
| A / D while grinding | keep your balance |
| E | use (Space is the ollie while skating) |

Grinds start by themselves when the board comes down onto an edge while travelling roughly along it. The board's angle picks the style:

- along the edge: 50-50;
- at an angle: crooked grind;
- across the edge: boardslide.

### What you see

The board is the real model listed in `RESOURCES/skateboard.model`: `Skateboard.obj` with its `BaseColor_1` texture. That small config file names the mesh and texture and says how the asset maps onto the board: nose and up axes, length and ground placement. The source files are only read. The mesh is loaded once and drawn as perspective-correct textured triangles on the board pose from the skate physics. The texture is quantised to the palette and mip-mapped. The pose covers:

- yaw;
- pitch from the wheels on stairs, the ollie snap and the flight path;
- roll when carving;
- nose or tail tilt in nosegrinds and 5-0s.

Without the file, a plain box board stands in.

The rider is the game's Doomguy sprite (`PLAY`) drawn as a billboard in the world. It stands sideways on the deck with the lowest opaque row of each frame on the deck top, and picks rotation frames like any actor. It uses:

- run frames through a push stroke;
- a squash while crouching or charging;
- a stretch on the pop;
- attack and pain frames when shooting or hit;
- death frames during a bail.

The board and rider are depth tested against the level. In the host-drawn mode they are also part of `render_overlay_rgba`, and `Session::scene()` exposes them for a truecolor renderer.

### Camera

The game runs at 35 Hz. The host calls `Session::set_frame_time(alpha, dt)` every frame, so the board, rider and camera are drawn from a pose interpolated between the last two tics. The camera smooths with the real frame time. It follows behind the direction of travel, looks slightly above the board, and backs off with speed between its minimum and maximum distance. Walls pull it in at once and it eases back out. It never ends up behind a wall, and it is clamped between floor and ceiling.

| Cvar | Default | Meaning |
| --- | --- | --- |
| `skate_camera_distance` | 110 | distance at rest |
| `skate_camera_height` | 46 | height above the board |
| `skate_camera_look_height` | 24 | look-at point above the board |
| `skate_camera_follow` | 9 | position smoothing (per second) |
| `skate_camera_rotation` | 5 | turning smoothing (per second) |
| `skate_camera_radius` | 8 | clearance from walls |
| `skate_camera_min_distance` | 64 | closest distance |
| `skate_camera_max_distance` | 190 | farthest distance, at speed |

### How it fits together

- `doom_sim::skate` holds the controller: `Skater` on the player, the `SkateState` machine, physics, grinding and bails. It runs from `player_think` in place of `P_MovePlayer`/`P_CalcHeight`. The body is still the player mobj and moves through `P_TryMove`, so pickups, line triggers, teleporters and damage all work as before.
- `doom_sim::grind` builds the grind graph when a level loads. It finds candidate edges, filters them, links them into nodes and connections, and files them in a lookup grid. The session caches graphs per map.
- `doom_game::session::skate_view` draws the chase camera, the board model, the Doomguy billboard and the debug views.
- `doom_render::DebugScene` carries the extra triangles and lines, which `PerspectiveRenderer::render_scene` depth-tests against the level.

Demos never skate. The controller only switches on for a game the user is playing.

## Mod weapons

Weapons are classes. Doom's nine weapons are `DoomWeapon` classes (`Fist`, `Pistol`, `Shotgun`, ...) whose states are `weaponinfo`'s, so a mod can inherit from them or replace them. A vanilla weapon whose class a mod replaced (`ReskinPistol : Pistol replaces Pistol`) runs the replacement's states, ammo, attack sound and puffs. Inherited states, such as `Ready`, still come from the original. Sprites and sounds in a later PWAD replace the IWAD's lumps of the same name.

| Feature | How |
| --- | --- |
| Custom puffs | Hitscan attacks carry their puff class (`A_FireBullets(..., "MyPuff")`, `A_CustomPunch`, `LineAttack`). The puff spawns with its `VSpeed` and flags (`+PUFFONACTORS`, `+ALWAYSPUFF`, `+PUFFGETSOWNER`, `+HITTRACER`, `+RANDOMIZE`) and runs its states: `Crash` on walls, `XDeath` on bleeding actors, `Melee` for melee. |
| AltFire | Right mouse or Z (`ext::ALTATTACK`) enters `AltFire`, with `AltHold` on refire. It uses `Weapon.AmmoType2`/`AmmoUse2`/`AmmoGive2` (`Player::fire_mode` picks the ammo) and respects `A_WeaponReady` flags (`WRF_NOPRIMARY`/`WRF_NOSECONDARY`/`WRF_NOSWITCH`/`WRF_NOBOB`). |
| Slots 0-9 | `TicCmd::weapon_slot: Option<u8>` holds slots 0-9 or no selection. Keys 1-9 and 0 send it. Slots 1-7 without mod weapons follow Doom's key rules; otherwise a slot cycles through its owned weapons. When a class weapon runs dry, the switch goes by `SelectionOrder`. Demos keep vanilla's 3-bit field. |
| ZScript methods | Methods with bodies are stored per class and called by name from state calls, code blocks and other methods, through the parent chain. Parameters are bound and values returned. `invoker` is the weapon, with its fields, `ammo1/2.amount` and `ammouse1/2`. Natives such as `CheckAmmo`, `DepleteAmmo`, `SpawnPlayerMissile`, `LineAttack` and `TakeInventory`/`GiveInventory` are available, along with all weapon and actor actions. |
