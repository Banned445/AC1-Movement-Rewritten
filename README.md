# AC1 Movement Rewritten

A clean-room recreation of Altaïr's movement from Assassin's Creed (2008), written in Rust with Bevy.

> **Status: early and unfinished.** Not a playable game. Many moves are missing or partial, animation
> transitions can still look choppy, and placeholders are marked `PORT:` in the code. Expect bugs.

## What it covers

- Ground locomotion: walk, jog, run, sprint, stops and pivots
- Jumps, falls and landings, including leaps into haystacks and jumps through kiosks
- Ledges: hang, shimmy, pull-up, drops, pass-overs and swing bars
- Climbing walls and climb jumps
- Wall runs, beams, pilotis and ladders
- Damascus, the whole city, streamed from your install
- The character: Rank 9 body parts, limb IK, cloth, skirt hinges and wind

No game files are included. The program reads Altaïr's models, animations and map data from **your own copy**
of the Steam PC version. Educational and personal use only.

## Requirements

- Rust, edition 2024 (1.85 or newer) (<https://rustup.rs>)
- Assassin's Creed (2008), PC (Steam)
- Optional: ffmpeg, for recording replay videos (see Debugging and replays)

## Run

```
cargo run --release
```

The game folder is found automatically when it sits beside this project or in your Steam libraries. To set it
yourself, point to the folder that contains `AssassinsCreed_Dx9.exe` and `DataPC.forge`:

```
cargo run --release -- --game-dir "C:\Program Files (x86)\Steam\steamapps\common\Assassin's Creed"
```

The path is saved to `game_dir.txt`, so you only do this once. You can also set the `AC_GAME_DIR`
environment variable. Without the game, the level still runs with a capsule in place of Altaïr.

## Controls

The keys below are the game's built-in keyboard profile. If your install has its own `DefaultBindings.map`, the
game reads its bindings from there instead, as the original does.

| Input | Action |
|---|---|
| WASD | Move |
| Arrow keys | Camera (the right stick). The mouse also moves the camera |
| Left mouse | Capture mouse (Esc releases) |
| Right mouse (hold) | High profile |
| Space | Legs. Into a wall: climb or grab. With right mouse held: sprint / free-run |
| Left Shift | Empty hand. On a ladder: drop off. Held in a fall: falling grab |
| Left Alt (hold) | Walk slow (keyboard stick × 0.6) |
| G | Show climbable edges |
| F1 | Toggle help |
| F2 | Debug map menu |
| F3 | City streaming counters (Damascus) |
| F5 | **Ghost mode**, the game's unused development flight (the Debug context, "Ghost mode" animation). WASD on the ground plane, Space / Ctrl up / down, 5 m/s, Shift × 3, Alt × 10, Q held = strafe, F6 = debug camera (holds still). F5 again drops out into a fall; hold Shift (the empty hand) with it to keep the flight's speed. Gamepad: L3 + R3 (B with it to keep the speed), left stick, right stick Y up / down (LB holds the height), RT × 3, RB × 10, X strafe, R3 camera |
| F9 | Save a bug report to `bugs/` in the project folder (see Debugging and replays) |

Not used by movement yet: E (the Head button), and the left mouse's weapon-hand slot. They are bound, but nothing reads them.

A gamepad (Xbox layout) fills the same pad: left stick moves, A is Legs, B is the empty hand, LB is the left
shoulder, and RT is high profile.

## Maps

Press **F2** to open the map menu, then click a map or press a number key. Switching resets the player.

| Key | Map |
|---|---|
| 1 | Greybox test level (default) |
| 2 | Masyaf roofs |
| 3 | Masyaf village |
| 4 | Damascus (the whole city) |

Masyaf maps are imported from your install. They are bounded crops: surrounding terrain, props, NPCs and
vegetation are absent, and materials are not exact.

**Damascus** is the whole World from your install: every district, the walls, the countryside around the city,
props, vegetation, collision and the climbable edges. It streams like the game does: the city's grid cells load
around you on background threads, objects switch between their level-of-detail meshes with distance, and the
merged low-detail "fake" meshes the game ships draw the rest of the city on the horizon. You start at the north
gate (the arrival from the Kingdom). The city's mood comes from the World file too: the sun, the ambient light, the
distance fog, the sky and the colour grading of the game's post effects, plus the lanterns' lights, the fires, smoke
and other particle systems placed around the city, the ambient sounds of fountains, trees, kiosks and braziers, and the
quarters' ambience (wind, district beds and their random calls). NPCs, crowds and missions are not included, and
materials use a standard PBR shader rather than the game's.

The game loads extra mission and memory-block DataBlocks depending on story progress (for example the props of an
opened assassination). The port loads the city grid alone unless `AC_CITY_DATABLOCKS` names some of them; a save in
Memory Block 2 during Tamir's investigation has `Memory Block 2`, `MB02 - Side Quest`, `M13_Tamir` and
`M13_Tamir_Investigation`, and the opened assassinations add `M13_Tamir_MissionOpened`, `M33_Vizier_MissionOpened` or
`M58_Jubair_MissionOpened`.

To start on a native map from the command line:

```powershell
$env:AC_NATIVE_MAP = "damascus"   # or "masyaf-village" / "masyaf-roofs"; unset to use the greybox
$env:AC_CITY_SPAWN = "Souk"       # Damascus only, optional: Bureau, Academy, Palace or Souk
$env:AC_CITY_DATABLOCKS = "M13_Tamir_MissionOpened"  # Damascus only, optional: mission / memory-block content to load, comma-separated
$env:AC_LOD_FADE = "1"            # optional: cross-fade between LODs (the retail game switches them)
cargo run --release
```

## Progress

| Area | State |
|---|---|
| Ground | Working. Some transitions into and out of the run are simplified |
| Crouch | The state machine and its clearance checks run in the simulation. Nothing in the game starts a crouch yet: the request comes from world scripts, which are not imported |
| Jumps and falls | Mostly working. Some fall and damage cases missing |
| Ledges | Mostly working. Hand spacing after a shimmy is a placeholder |
| Climbing and wall runs | Working |
| Beams | Mostly working. Beam ends, junctions and mounts follow the game's rules. Some bends and hop landings still approximate |
| Ladders | Mostly working. Turns and the side jump are in. Ladder attacks are not ported (there is no combat) |
| Haystacks and kiosks | Landing in a haystack and jumping through kiosks work. Hiding in them is not reachable yet |
| Swing bars | Working |
| Animation and IK | Partial. Transitions into the ground run are skipped for now |
| Character | All Rank 9 parts load. Cloth, skirt hinges and wind run from the game's data. Robe folds, weapon offsets and facial expressions unfinished |
| Not started | Swimming, combat, pole and rope contexts. NPC crowds: the avoidance rules are written, but the level has no NPCs |

## Debugging and replays

**F9** writes `bugs/bug-<unix time>/` in the project folder. It holds `input.txt` (every frame since launch, which
can be replayed), `state.txt` (the last ~10 seconds of state), `screenshot.png`, and `note.txt` for a line about
what looked wrong.

Set these environment variables before `cargo run --release`:

| Variable | What it does |
|---|---|
| `AC_REPLAY=<bug folder>` | Replays a bug report in the full game, with the same timesteps, controls and camera. Writes `replay_state.txt` next to it, then exits |
| `AC_VIDEO=<out.mp4>` | With `AC_REPLAY`: pipes every replayed frame to ffmpeg. `AC_FFMPEG` sets the ffmpeg path, `AC_VIDEO_CRF` the quality |
| `AC_WINDOW=<w>x<h>` | Window size. The default is 1600 x 900 |
| `AC_NO_HUD=1` | Hides the debug HUD |
| `AC_NO_GUIDANCE=1` | Starts with the guidance lines hidden (G shows them) |
| `AC_DEMO_COURSE=1` | Adds a parkour course on the greybox's empty north side. Its scripted run is written as a recording by `cargo test --release demo_course_run -- --ignored --nocapture` (output in `target/demo-course/`) |
| `AC_AUTOPILOT=roofs`, `climb` or `ledge` | Scripted input for that scenario |
| `AC_SCREENSHOT=<file.png>` | Saves a screenshot and exits. `AC_SCREENSHOT_AT=<seconds>` sets when (default 3) |
| `AC_GHOST=1` | Turns Ghost mode on after one second |
| `AC_CITY_TOUR=<m/s>` | Flies across Damascus at that speed, to stress the streaming |
| `AC_CITY_AT="x y z yaw"` | Starts Damascus at a game position (yaw in degrees), to compare a spot with the game |
| `AC_SHOTS=t1,t2,…` | Freezes at each time and saves a screenshot per view (`AC_VIEWS=back,left,…`, `AC_SHOT_DIR`). `AC_SHOT_CAMS=<file>` places the cameras exactly instead, one line `name x y z qx qy qz qw` per view in game space |
| `AC_CITY_OVERLAY=1` | Shows the F3 city counters from the start |
| `AC_NO_SOUND=1` | Silences the sounds |
| `AC_SND_LOG=<seconds>` | Logs the playing voices and the mix level at that interval |
| `AC_NO_MODEL=1` | Runs without Altaïr's model, with the capsule |

## Tests

```
cargo test --release
```

About 575 tests run by default. About 75 more are `#[ignore]`d: long simulations and bug-report replays, which the next section runs. Tests that need game data skip themselves when the game isn't found.

To replay the saved bug reports through the headless simulation and report the first frame that differs:

```
cargo test --release replay_bug_recordings -- --ignored --nocapture
```

Set `AC_BUG=<folder name>` to replay one report.
