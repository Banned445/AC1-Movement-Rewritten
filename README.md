# AC1 Movement Rewritten

A clean-room recreation of Altaïr's movement from Assassin's Creed (2008) in Rust and Bevy: ground locomotion,
jumps and falls, ledges, climbing, beams, wall runs, ladders and swing bars, on a greybox test level.

> **Status: early and unfinished.** This is a work in progress, not a playable game. Many moves are missing or only
> partly done, animation switches can still look choppy, and some behaviour is a placeholder (marked `PORT:` in the
> code) until the game's own rule is understood. Expect bugs.

No game files are included. At startup the program reads Altaïr's model and animations from **your own copy** of
the game (the Steam PC version, `DataPC.forge`). Educational and personal use only.

## Setup

1. Install Rust: <https://rustup.rs>
2. Build and run:

   ```
   cargo run --release
   ```

3. Point it at your game the first time, if it isn't found automatically:

   ```
   cargo run --release -- --game-dir "C:\Program Files (x86)\Steam\steamapps\common\Assassin's Creed"
   ```

   Use the folder that contains `AssassinsCreed_Dx9.exe` and `DataPC.forge`. The path is saved to `game_dir.txt`
   next to the executable, so you only do this once.

The game folder is found in this order: the `AC_GAME_DIR` environment variable, `game_dir.txt`, a folder named
`Assassin's Creed` beside the project, then your Steam libraries. Without the game the level still runs, with a
capsule in place of Altaïr and no animations.

## Controls

WASD move, left mouse button capture the mouse (Esc releases it), right mouse button high profile, Space legs
(with the right button held: sprint / free-run; into a wall: climb or grab), E empty hand (drop off a ladder), G show the climbable edges, F9 save a bug
report, F1 hide the help. The full list is on screen.

## Progress

Movement only: there is no combat, no NPCs and no real game level, only a greybox test level.

| Area | State |
|---|---|
| Ground: walk, jog, run, sprint, stops, pivots | Working. The full ground state machine is not done yet, so some transitions into and out of the run are simplified |
| Jumps, falls and landings | Mostly working. Some fall and damage cases are missing |
| Ledges: hang, shimmy, corners, pull-up, drops | Mostly working. Hand spacing after a shimmy is a placeholder |
| Climbing on walls, climb jumps | Working, with placeholders in hold choice and some jump paths |
| Wall runs | Working |
| Beams: walking, 90° turns, pull-down to a hang, corner hops, bends | Mostly working. Falling off a beam is missing |
| Ladders | Mostly working. The ladder turn and the hang's side jump are missing |
| Swing bars | Working |
| Animation: blending, transitions, foot and hand IK | Partial. Transitions into the ground run are skipped for now, which is the main source of choppy switches |
| Not started | Crouch, crowds, swimming, hay and kiosk hiding, slope slides, combat |

## Tests

```
cargo test --release
```

Tests that need game data skip themselves when the game isn't found.
