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
| F9 | Save a bug report |

## Maps

Press **F2** to open the map menu, then click a map or press a number key. Switching resets the player.

| Key | Map |
|---|---|
| 1 | Greybox test level (default) |
| 2 | Masyaf roofs |
| 3 | Masyaf village |

Masyaf maps are imported from your install. They are bounded crops: surrounding terrain, props, NPCs and
vegetation are absent, and materials are not exact.

To start on a native map from the command line:

```powershell
$env:AC_NATIVE_MAP = "masyaf-village"   # or "masyaf-roofs"; unset to use the greybox
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
