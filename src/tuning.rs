//! Movement constants recovered from AssassinsCreed_Dx9.exe (v1.02 b86610).
//!
//! Every value cites the function it was read from (see RE/*.md). Values the game takes from
//! animation root motion (not yet decoded) are marked PLACEHOLDER and must be replaced once
//! RE/09 (animation format) exists.
//!
//! Coordinate note: the game is Z-up, Bevy is Y-up. "Height" below means game Z = Bevy Y.

/// PORT comparison switch for the RE/15 §6.3 visual-root and camera pass; the ankle bug fix is unconditional.
pub const GAME_SMOOTHING: bool = true;
/// PORT comparison switch for the verified fall scalar/input corrections.
pub const GAME_FALLS: bool = true;
/// PORT comparison switch for the verified air-catch rules and beam contact detector.
pub const AIR_CATCHES: bool = true;
/// PORT comparison switch for the verified Ground extras state/contact rules (RE/02 §9).
pub const GAME_GROUND_EXTRAS: bool = true;

// ---------------------------------------------------------------- input (RE/01 §6)
/// Stick dead-zone; speed = (|stick| - 0.35) / 0.65.  GoAssassinActionInterpreter 0xEE65A0
pub const STICK_DEADZONE: f32 = 0.35;
/// "Legs" (jump) button buffer, seconds.  0xEEDE60
pub const JUMP_BUFFER: f32 = 0.3;
/// Turn attenuation: beyond 45° from facing, speed scales down to 0 at 90°; floor 0.1. 0xEE65A0
pub const TURN_ATTEN_START: f32 = std::f32::consts::FRAC_PI_4;
pub const TURN_ATTEN_RANGE: f32 = std::f32::consts::FRAC_PI_4;
pub const TURN_ATTEN_UP_RATE: f32 = 5.0;
pub const TURN_ATTEN_DOWN_RATE: f32 = 10.0;
pub const TURN_ATTEN_FLOOR: f32 = 0.1;

// ---------------------------------------------------------------- ground (RE/02)
/// Speed parameter bands (0..1): Walk <= 0.25 < Jog <= 0.5 < Run <= 0.75 < Sprint. GetSpeedBand 0xD807B0
pub const BAND_WALK: f32 = 0.25;
pub const BAND_JOG: f32 = 0.5;
pub const BAND_RUN: f32 = 0.75;
/// Target speed parameter = base + 0.25 * stick; base 0 / 0.5 / 0.75. RE/02 §1
pub const BASE_LOW_PROFILE: f32 = 0.0;
pub const BASE_HIGH_PROFILE: f32 = 0.5;
pub const BASE_SPRINT: f32 = 0.75;
pub const STICK_SPAN: f32 = 0.25;
/// Speed parameter: rises at 1.0/s, falls through the deceleration curve; blend weights and root motion
/// of the locomotion clips: see `player::move_blend` (HumanGround__UpdateMoveBlend 0xDA0810).

/// Player turn rate (rad/s): min 270°/s, max 360°/s; thresholds are 0 so effectively 360°/s.
/// HumanGround__UpdateHeading 0xD95290 → RotateTowards 0xD94F30
pub const PLAYER_TURN_RATE: f32 = std::f32::consts::TAU;

/// Ground loss → InAir fall types by fall height 1 / 2 / 8 m and horizontal speed 2.5 m/s. 0xD8C380
pub const FALL_TYPE_HEIGHTS: [f32; 3] = [1.0, 2.0, 8.0];

// ---------------------------------------------------------------- in air (RE/04)
/// Hard-coded gravity, m/s². 0xDFFC90
pub const GRAVITY: f32 = 9.8;
/// Max horizontal speed while steering a free-fall onto the real target, m/s. 0xE0DEF0
pub const FREEFALL_MAX_HORIZONTAL: f32 = 15.0;
/// Non-target drift: decays at 4 m/s², capped at 5 m/s. 0x19BA434 / 0x19BA438
pub const DRIFT_DECEL: f32 = 4.0;
pub const DRIFT_MAX: f32 = 5.0;
/// Jump arrives when the clip ends within 0.01 m of the target. CheckJumpTargetArrival 0xE07D00
pub const ARRIVAL_TOLERANCE: f32 = 0.01;
/// Over-drop rule: if the target is > 5 m below (3 m for Leap of Faith) aim 5 m down, then free-fall. 0xB1B8C0
pub const OVERDROP: f32 = 5.0;
/// Non-player constructor defaults (0xE0FE80); player enables the fixed table (0xED6280).
/// Retained only for the GAME_FALLS comparison path.
pub const FALL_HEAVY: f32 = 6.3;
pub const FALL_FATAL: f32 = 7.0;
/// Total drop > 3 m plays the damage / damage-roll landing + camera shake (drop − 3)/7, else the soft/hard
/// landing blend. SetupToGround_Landing 0xE05940 (`player::jump_blend::landing`)
pub const ROLL_DROP: f32 = 3.0;

/// PORT: jump distance when no target is in range (the game always jumps to a target; vt28 resolves one,
/// 0xD832F0). The jump itself uses the game's free-step blend.
pub const FREE_JUMP_DISTANCE: f32 = 2.5;

// ---------------------------------------------------------------- jump targets (RE/01 §7b)
/// Candidate must lie within a 45° cone of the wanted direction and no lower than -3 m. 0xE96BF0
pub const TARGET_CONE: f32 = std::f32::consts::FRAC_PI_4;
pub const TARGET_MIN_DZ: f32 = -3.0;
/// Ground-type target bands (max up, near, mid, far). Human__ComputeJumpAnimBlend 0xB1EC40
pub const GROUND_MAX_UP: f32 = 1.3;
pub const GROUND_FAR: f32 = 7.0;
/// Landing point is placed this far inside a roof edge. PLACEHOLDER (game uses the guidance contact).
/// Ground loss → fall type 0–6 (0xD8C380): height < 1 (or < 2 with probe type 1) → 0/1, 2..8 with probe
/// type 1 → 2/3, else 4/6; the faster type at a horizontal speed ≥ 2.5 m/s.
pub const LAND_INSET: f32 = 0.45;

// ---------------------------------------------------------------- ledge (RE/03 §7)
/// Wall hang: root (feet) 1.1 m below the hands and 0.5 m out from the wall. 0xDD6730 / 0xDE1060
pub const WALL_HANG_DROP: f32 = 1.1;
pub const WALL_HANG_OUT: f32 = 0.5;
/// Free hang: root 2.4 m below the hands. 0xDD6730
pub const FREE_HANG_DROP: f32 = 2.4;
/// Free-hang root offset from the edge, from xx_h_hangfree_wait (wrists 0.04 m behind the root; RE/11 §5.1).
pub const FREE_HANG_OUT: f32 = 0.01;
/// WallFree hang (free hang with a wall under it): root 0.5 m out, from xx_h_hangwallfree_wait (hands 0.46 m
/// in front of the root) and the 0xB21DA0 band offset (+0.5·n).
pub const WALLFREE_HANG_OUT: f32 = 0.5;
/// Shimmy free-space sweep: sphere r 0.15, length 1.55, origin 0.75 out + 0.2 up. ProbeLateral 0xDD9640
pub const SHIMMY_SWEEP_R: f32 = 0.15;
pub const SHIMMY_SWEEP_LEN: f32 = 1.55;
pub const SHIMMY_SWEEP_OUT: f32 = 0.75;
pub const SHIMMY_SWEEP_UP: f32 = 0.2;
/// Blocked if free distance − 0.15 < 0.7; step = min(d − 0.4, 1.0 − |handL − handR|); min step 0.15.
pub const SHIMMY_MIN_FREE: f32 = 0.7;
pub const SHIMMY_WALL_MARGIN: f32 = 0.4;
pub const SHIMMY_MAX_SPACING: f32 = 1.0;
pub const SHIMMY_MIN_STEP: f32 = 0.15;
/// PLACEHOLDER: hand spacing after a closing step (the game re-snaps the trailing hand to the chain).
pub const HAND_SPACING: f32 = 0.4;
/// Lost-ledge check radius = 0.25 + half the hand spacing. HasLostLedge 0xDD20D0
pub const LOST_LEDGE_R: f32 = 0.25;
/// Vertical hand-over-hand step to a ledge 0.6–1.2 m away (StartHandStep 0xDDE0C0).
pub const VSTEP_MIN: f32 = 0.45;
pub const VSTEP_MAX: f32 = 1.25;
/// Pull-up: root ends 0.5 m inside the ledge (Pullup_Start 0xDDBE80).
pub const PULLUP_IN: f32 = 0.5;
/// Move durations = the game clips' lengths (RE/10): xx_h_hangwall_strafe_*_050cm_open 0.533 s /
/// _close 0.600 s, xx_h_hangwall_u_climb_1m 0.600 s, _d_climb_1m 0.533 s.
pub const SHIMMY_OPEN_TIME: f32 = 0.533;
pub const SHIMMY_CLOSE_TIME: f32 = 0.600;
/// Vertical hand step halves = the game's step clips (ledge tables 0x1A2C4C0 / 0x1A2C4F0, RE/13):
/// wall first hand up xx_h_hangwall_climb_1m_u_1l?u 0.6 s, down _1m_d_ 0.4 s; every second half and every
/// free-hang half 0.533 s.
pub const VSTEP_TIME: f32 = 0.6;
pub const VSTEP_DOWN_TIME: f32 = 0.4;
pub const VSTEP_SECOND_TIME: f32 = 0.533;
pub const JUMP_UP_TIME: f32 = 0.6;
/// PORT: corner / side-jump duration only when the clip table lacks the action (jump_clips.rs).
pub const CORNER_FALLBACK_TIME: f32 = 0.8;
pub const SIDE_JUMP_FALLBACK_TIME: f32 = 1.0;
/// `HumanLedge__TryJumpUpToClimb` 0xDD5E10: hold rows 1.2 m above the hands and 1.2 m below that, sub_11713A0 radius
/// 0.4 with 0.5 (PORT: used as the vertical tolerance).
pub const JUMP_TO_CLIMB_UP: f32 = 1.2;
pub const JUMP_TO_CLIMB_R: f32 = 0.4;
pub const JUMP_TO_CLIMB_VTOL: f32 = 0.5;
/// Pull-up = the game's clip chains (RE/11 §5): wall hang → hangknee → stand = 0.333+0.4+0.467+0.4 s;
/// free hang → hangwaist → hangknee → stand = 0.8+0.2+0.733+0.467+0.4 s.
pub const PULLUP_WALL_TIME: f32 = 1.6;
pub const PULLUP_FREE_TIME: f32 = 2.6;
/// Catching a ledge: the reception clip (xx_fall_tr_hangwall_straight_min a+b = 0.667 s).
pub const GRAB_TIME: f32 = 0.667;

// ---------------------------------------------------------------- climb (RE/03 §4)
/// Hold grid: columns 0.75 m, rows 0.6 m; hands sit 2 rows above their foot cell. BuildHoldGrid 0xDF6A40
pub const CLIMB_COL: f32 = 0.75;
pub const CLIMB_ROW: f32 = 0.6;
pub const CLIMB_HAND_ROWS: i32 = 2;
/// Probe: radius 0.375, vertical tolerance 0.3, max edge angle 45°.
pub const CLIMB_PROBE_R: f32 = 0.375;
pub const CLIMB_PROBE_VTOL: f32 = 0.3;
/// The grid cell probe's depth: the box reaches 1.0 m both ways along the facing from the grid plane, and the
/// reference point sits 0.2 m out of it (sub_11713A0 args 1.0 and (0, −0.2, 0), BuildHoldGrid 0xDF6A40).
pub const CLIMB_PROBE_DEPTH: f32 = 1.0;
pub const CLIMB_PROBE_BACK: f32 = 0.2;
/// The cell probe shortens each edge piece by this at both ends (sub_116D3C0 step 0.1).
pub const CLIMB_PROBE_STEP: f32 = 0.1;
/// PORT (lost-grip check, 0xDFE7F0 → sub_E55A10): a climb hold counts as present while a grab edge lies within this.
pub const CLIMB_HOLD_TOL: f32 = 0.1;
/// Stick magnitude above which the LONG move table is tried first. ChooseMove 0xDFDE90
pub const CLIMB_LONG_STICK: f32 = 0.5;
/// Climb move duration = the game move clips xx_l_climb_* (0.533 s; StartMove 0xDFA0C0 uses the clip length, 0.5 s without one).
pub const CLIMB_MOVE_TIME: f32 = 0.533;
/// Root distance behind the feet holds' midpoint along the climb facing (sub_B1CC20: 0.5·F). The clips agree:
/// xx_climb_wait_* put it 0.49 m out (RE/11 §5.1).
pub const CLIMB_ROOT_OUT: f32 = 0.5;

// ---------------------------------------------------------------- air catch (RE/04)
/// Ledge catch: hand box 0.4 x 0.3, edges within 70°, reach +1.4 (ledge) / +1.95 (wall). 0xE0A990 / 0xE0AC70
pub const CATCH_BOX_R: f32 = 0.4;
pub const CATCH_BOX_V: f32 = 0.3;
pub const CATCH_MAX_ANGLE: f32 = 70.0 * std::f32::consts::PI / 180.0;
pub const CATCH_REACH_LEDGE: f32 = 1.4;
pub const CATCH_REACH_WALL: f32 = 1.95;
/// Ledge-type jump targets: max up 3.0 m, distance bands 2.5 / 6 / 8 m. 0xB1EC40
pub const LEDGE_MAX_UP: f32 = 3.0;
/// Highest hand target of the standing straight jump: its top band blends the 250 / 300 cm clips over
/// 2.5–3.0 m (0xB21DA0). (hypothesis) the target finder's own limit is not traced.
pub const STRAIGHT_JUMP_MAX: f32 = 3.0;
#[allow(dead_code)]
pub const LEDGE_FAR: f32 = 8.0;
/// PORT: how far a running jump reaches for a ledge (hang target). The game only jumps along the world's precomputed
/// jump links (`MetaLinkTypeID_JumpLink`, `WorldArea::JumpLinkRange` Normal / Extended), so its reach is level data;
/// 0xB1EC40's 8 m is only the animation band. The port uses the 5.5 m float of the table at 0x1A2EA50 (hypothesis:
/// the extended link range) and the 2.5 m near band for ledges the root has to rise more than 1 m to.
/// LIVE: read JumpLinkRange_Normal / _Extended from a loaded WorldArea.
pub const LEDGE_JUMP_FAR: f32 = 5.5;
pub const LEDGE_JUMP_FAR_UP: f32 = 2.5;
pub const LEDGE_JUMP_UP_RISE: f32 = 1.0;

// ---------------------------------------------------------------- character proxy (RE/01 §7, `collision`)
/// Capsule radius: shape 0.35 + keep distance 0.05 (`HumanGround__OnEnterInit` 0xDA7D20 → 0x52ED40(0.4), 0x52E9C0;
/// keep distance 0.05 at controller +204, ctor 0x57B4F0).
pub const CAPSULE_RADIUS: f32 = 0.4;
/// Capsule height (0xDA7D20 → 0x52ED20(1.8); `HumanInAir__Cleanup` 0xE03DE0 restores it).
pub const CAPSULE_HEIGHT: f32 = 1.8;
/// Step offset on the ground: the capsule floats this high above the feet (controller +100, set by
/// `HumanGround__OnActivateSetup` 0xDAE6E0 → 0x578290(0.37), applied by 0x579500).
pub const STEP_HEIGHT: f32 = 0.37;
/// Stick-to-ground reach below the feet (controller +96, 0xDAE6E0 → 0x5782C0(0.58); cast by 0x57D240).
pub const SNAP_DOWN: f32 = 0.58;

// ---------------------------------------------------------------- limb IK (RE/11, LimbIK__SolveEffectors 0xE57570)
/// Limb weight rises at 4/s while the limb has a contact (0.25 s fade-in).
pub const IK_WEIGHT_IN_RATE: f32 = 4.0;
/// …and falls at 5/s once released (0.2 s fade-out).
pub const IK_WEIGHT_OUT_RATE: f32 = 5.0;
/// PORT: a contact moving further than this counts as a new hold (limb travels to it).
pub const IK_RETARGET_DIST: f32 = 0.02;
/// Wrist relative to the edge point it grips. Game data: the wall hang root is 1.1 m below / 0.5 m out
/// from the hands (0xDD6730) while xx_h_hangwall_wait puts the wrists 1.0 m above / 0.45 m in front of
/// its root (→ wrist ~0.1 m below / 0.05 m out), and every hang/climb clip curls the middle finger 0.07 m
/// forward / 0.12 m up from the wrist. PORT: 0.08 / 0.04 so the curled fingers end over the lip.
pub const IK_HAND_DROP: f32 = -0.08;
pub const IK_HAND_OUT: f32 = 0.04;
/// Ankle relative to the foot hold: level with it, 0.17 m out (xx_climb_wait_*: ankles 0.32 m in front
/// of the root, wrists 0.44 m → the wall plane is ~0.49 m in front).
pub const IK_FOOT_UP: f32 = 0.0;
pub const IK_FOOT_OUT: f32 = 0.17;
/// PORT: a hold moved while the clip keeps that limb in contact and never releases it: settle after this long.
pub const IK_CONTACT_WAIT: f32 = 0.3;
