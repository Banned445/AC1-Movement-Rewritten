# AC1 Movement Rewritten

A clean-room recreation of Altaïr's movement from Assassin's Creed (2008), written in Rust with Bevy.

> **Status: early and unfinished.** Not a playable game. Many moves are missing or partial, animation
> transitions can still look choppy, and placeholders are marked `PORT:` in the code. Expect bugs.

## What it covers

- Ground locomotion: walk, jog, run, sprint, stops and pivots
- Jumps, falls and landings
- Ledges: hang, shimmy, pull-up and drops
- Climbing walls and climb jumps
- Wall runs, beams, ladders and swing bars

No game files are included. The program reads Altaïr's models, animations and map data from **your own copy**
of the Steam PC version. Educational and personal use only.

## Requirements

- Rust (<https://rustup.rs>)
- Assassin's Creed (2008), PC (Steam)

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

| Input | Action |
|---|---|
| WASD | Move |
| Left mouse | Capture mouse (Esc releases) |
| Right mouse (hold) | High profile |
| Space | Legs. Into a wall: climb or grab. With right mouse held: sprint / free-run |
| E | Empty hand (drop off a ladder) |
| G | Show climbable edges |
| F1 | Toggle help |
| F2 | Debug map menu |
| F3 | City streaming counters (Damascus) |
| F5 | Debug fly / noclip, the game's own debug flight (Debug context, "Ghost mode" animation): WASD along the view, Space / Ctrl up / down, 5 m/s, Shift × 3, Alt × 10. F5 again lands on the floor below |
| F9 | Save a bug report |

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
gate (the arrival from the Kingdom). NPCs, crowds, sounds, missions and particles are not included, the lighting is a
fixed sun and haze, and materials use a standard PBR shader rather than the game's.

To start on a native map from the command line:

```powershell
$env:AC_NATIVE_MAP = "damascus"   # or "masyaf-village" / "masyaf-roofs"; unset to use the greybox
$env:AC_CITY_SPAWN = "Souk"       # Damascus only, optional: Bureau, Academy, Palace or Souk
$env:AC_LOD_FADE = "1"            # optional: cross-fade between LODs (the retail game switches them)
cargo run --release
```

## Progress

| Area | State |
|---|---|
| Ground | Working. Some transitions into and out of the run are simplified |
| Jumps and falls | Mostly working. Some fall and damage cases missing |
| Ledges | Mostly working. Hand spacing after a shimmy is a placeholder |
| Climbing and wall runs | Working |
| Beams | Mostly working. Some bends and hop landings still approximate |
| Ladders | Mostly working. Ladder turn and side jump missing |
| Swing bars | Working |
| Animation and IK | Partial. Transitions into the ground run are skipped for now |
| Character | All Rank 9 parts load. Robe folds, weapon offsets and facial expressions unfinished |
| Not started | Crouch, crowds, swimming, hay and kiosk hiding, slope slides, combat |

## Tests

```
cargo test --release
```

Tests that need game data skip themselves when the game isn't found.
