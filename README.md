# AC1 Movement Rewritten

A clean-room recreation of Altaïr's movement from Assassin's Creed (2008) in Rust and Bevy: ground locomotion,
jumps and falls, ledges, climbing, beams, wall runs, ladders and swing bars, on a greybox test level.

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

## Tests

```
cargo test --release
```

Tests that need game data skip themselves when the game isn't found.
