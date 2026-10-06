# AC1 Movement Rewritten

A clean-room recreation of Altaïr's movement from Assassin's Creed (2008) in Rust and Bevy: ground locomotion,
jumps and falls, ledges, climbing, beams, wall runs, ladders and swing bars, on a greybox test level or an
experimental imported Masyaf environment.

> **Status: early and unfinished.** This is a work in progress, not a playable game. Many moves are missing or only
> partly done, animation switches can still look choppy, and some behaviour is a placeholder (marked `PORT:` in the
> code) until the game's own rule is understood. Expect bugs.

No game files are included. The program reads Altaïr's model, animations and optional map assets from **your own
copy** of the game (the Steam PC version, `DataPC.forge` and `DataPC_Masyaf.forge`). Educational and personal use only.

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

An experimental Masyaf rooftop slice can be loaded from your install instead of the greybox:

```powershell
$env:AC_NATIVE_MAP = "masyaf-roofs"
cargo run --release
```

It reads three source houses, their placements, textures, triangle collision and authored climbing edges,
plus a ladder between the roofs. Clear `AC_NATIVE_MAP` to return to the greybox. This is a bounded crop:
surrounding terrain and props are absent, and native material blending, lighting and some guidance-filter
semantics remain unfinished. It is not yet an exact recreation of the original scene.

Press **F2** in-game to open the debug map menu. Click a map or press **1** (greybox) / **2** (Masyaf roofs) / **3** (Masyaf village)
to switch without restarting. Switching resets the player and respawn position. Movement pauses while
the menu is open; **F2** or **Esc** closes it. If a map cannot load, the current scene stays available and
the menu shows the error.

**Masyaf village** (**3** in the menu, or `AC_NATIVE_MAP=masyaf-village`) expands the sample into a connected
street and rooftop test area. It loads four neighbouring village cells and the shared native ground:
145 static placements, including houses, ladders, walls, rocks, stairs and props, with original collision
and authored climbing edges. You spawn on a street and can climb to the roofs. Camera obstruction
avoidance keeps nearby buildings from blocking the view. Source street walking and a street-to-roof
ladder route are tested. This is a village test area, with surrounding ground extending beyond the
imported buildings; other city cells, animated vegetation, NPCs and original material blending remain absent.

## Controls

WASD move, left mouse button capture the mouse (Esc releases it), right mouse button high profile, Space legs
(with the right button held: sprint / free-run; into a wall: climb or grab), E empty hand (drop off a ladder), G show the climbable edges, F9 save a bug
report, F1 hide the help. The full list is on screen.

## Progress

Movement only: there is no combat or NPCs. The default scene is a greybox test level; the optional Masyaf
slice imports a small section of the original environment.

| Area | State |
|---|---|
| Imported environments | Experimental Masyaf roofs and connected village, with source textures, collision and climbing edges; exact materials, surrounding city cells and vegetation remain unfinished |
| Ground: walk, jog, run, sprint, stops, pivots | Working. The full ground state machine is not done yet, so some transitions into and out of the run are simplified |
| Jumps, falls and landings | Mostly working. Some fall and damage cases are missing |
| Ledges: hang, shimmy, corners, pull-up, drops | Mostly working. Hand spacing after a shimmy is a placeholder |
| Climbing on walls, climb jumps | Working, with placeholders in hold choice and some jump paths |
| Wall runs | Working |
| Beams: walking, 90° turns, pull-down, corner hops, bends, support loss and climb starts | Mostly working. Entry and step-off use locomotion; bends steer gradually and hop landings move. Impulsion uses the native 0.2 s input delay; jump/hop completion waits until past item end. Native animation queue scheduling and obstacle target selection still need comparison |
| Ladders | Mostly working. The ladder turn and the hang's side jump are missing |
| Swing bars | Working |
| Animation: blending, transitions, foot and hand IK | Partial. Transitions into the ground run are skipped for now, which is the main source of choppy switches |
| Not started | Crouch, crowds, swimming, hay and kiosk hiding, slope slides, combat |

## Tests

```
cargo test --release
```

Tests that need game data skip themselves when the game isn't found.

To reproduce the native-map walk-to-freerun transition, set `AC_NATIVE_MAP=masyaf-village` and
`AC_AUTOPILOT=native-freerun`. It walks from the map spawn and presses Legs in high profile after two seconds.
Native query bounds filtering and cached static jump candidates are enabled by default;
`AC_NATIVE_QUERY_CULLING=0` disables them for comparison. A manual CPU timing check is available with
`cargo test --release profile_native_freerun_transition -- --ignored --nocapture`.
