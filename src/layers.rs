//! Collision layers (`CollisionFilterInfo::Layer`, 50 values) and the layer pairs that collide.
//!
//! GENERATED from the exe by the private RE tools (CollisionFilterHook setup 0x4DC8A0: every pair collides
//! except the 721 pairs disabled through 0x4DBD20). A collidable's filter word (0x4DC2B0): bits 0-5 layer,
//! 6-9 body part, 10-15 subsystem group (equal and non-zero: no collision), 16-31 system group (equal: the
//! part matrix decides). Do not edit by hand.
#![allow(dead_code)]

pub const LAYER_NONE: u8 = 0;
pub const STATIC: u8 = 1;
pub const DYNAMIC: u8 = 2;
pub const DEBRIS: u8 = 3;
pub const DETECTION_ZONE: u8 = 4;
pub const CHARACTER: u8 = 5;
pub const RAGDOLL: u8 = 6;
pub const RAYCAST: u8 = 7;
pub const NO_CHARACTER_COLLISION: u8 = 8;
pub const RAYCAST_ONLY_CHARACTERS: u8 = 9;
pub const NOTHING: u8 = 10;
pub const INTERACTIVE: u8 = 11;
pub const CHARACTER_NO_INTERACTIVE: u8 = 12;
pub const ONLY_STATIC: u8 = 13;
pub const RAYCAST_NO_CHARACTERS: u8 = 14;
pub const HORSE: u8 = 15;
pub const HORSE_BARRIER: u8 = 16;
pub const CAMERA: u8 = 17;
pub const CAMERA_BARRIER: u8 = 18;
pub const STATIC_NO_CAMERA: u8 = 19;
pub const CHARACTER_NO_STATIC: u8 = 20;
pub const MAIN_CHARACTER: u8 = 21;
pub const MAIN_CHARACTER_NO_INTERACTIVE: u8 = 22;
pub const MAIN_CHARACTER_NO_STATIC: u8 = 23;
pub const FUZZY_ZONE: u8 = 24;
pub const FUZZY_ZONE_NO_CAMERA: u8 = 25;
pub const FUZZY_ZONE_ALL_CHARACTER: u8 = 26;
pub const ONLY_STATIC_AND_CAMERA_RAYCAST: u8 = 27;
pub const HOLLYWOOD_MODE: u8 = 28;
pub const STATIC_NO_LOS: u8 = 29;
pub const STATIC_NO_CAMERA_NO_LOS: u8 = 30;
pub const STATIC_NO_HORSE: u8 = 31;
pub const ONLY_LOS: u8 = 32;
pub const FUZZY_ZONE_ALL_CHARACTER_NO_CAMERA: u8 = 33;
pub const NPC_BARRIER: u8 = 34;
pub const OCCLUSION_HEIGHT_TEST: u8 = 35;
pub const CAMERA_BARRIER_PLUS_LOS: u8 = 36;
pub const PROJECTILE: u8 = 37;
pub const PROJECTILE_BARRIER: u8 = 38;
pub const PROJECTILE_CAMERA_BARRIER: u8 = 39;
pub const ONLY_LOS_AND_FUZZY: u8 = 40;
pub const FUZZY_ZONE_NO_CAMERA_ALLOW_ASSASSINATION: u8 = 41;
pub const HORSE_NO_MAIN_CHARACTER_COLLISION: u8 = 42;
pub const RAYCAST_NO_CHARACTERS_FUZZY_ZONE: u8 = 43;
pub const ONLY_DYNAMIC: u8 = 44;
pub const ONLY_FUZZY: u8 = 45;
pub const NPC_SPAWNING: u8 = 46;
pub const RAYCAST_FUZZY_ZONE: u8 = 47;
pub const NO_HORSE_COLLISION: u8 = 48;
pub const RAGDOLL_PENETRATION_SOLVER: u8 = 49;

/// Names, by layer.
pub const NAMES: [&str; 50] = ["LayerNone", "Static", "Dynamic", "Debris", "DetectionZone", "Character", "Ragdoll", "Raycast", "NoCharacterCollision", "RaycastOnlyCharacters", "Nothing", "Interactive", "CharacterNoInteractive", "OnlyStatic", "RaycastNoCharacters", "Horse", "HorseBarrier", "Camera", "CameraBarrier", "StaticNoCamera", "CharacterNoStatic", "MainCharacter", "MainCharacterNoInteractive", "MainCharacterNoStatic", "FuzzyZone", "FuzzyZoneNoCamera", "FuzzyZoneAllCharacter", "OnlyStaticAndCameraRaycast", "HollywoodMode", "StaticNoLOS", "StaticNoCameraNoLOS", "StaticNoHorse", "OnlyLOS", "FuzzyZoneAllCharacterNoCamera", "NPCBarrier", "OcclusionHeightTest", "CameraBarrierPlusLOS", "Projectile", "ProjectileBarrier", "ProjectileCameraBarrier", "OnlyLOSAndFuzzy", "FuzzyZoneNoCameraAllowAssassination", "HorseNoMainCharacterCollision", "RaycastNoCharactersFuzzyZone", "OnlyDynamic", "OnlyFuzzy", "NPCSpawning", "RaycastFuzzyZone", "NoHorseCollision", "RagdollPenetrationSolver"];

/// For each layer, the bit set of layers it collides with.
pub const COLLIDES: [u64; 50] = [
    0x03cffffffffbff, // LayerNone
    0x03cd2de86af9ef, // Static
    0x03dc2ce8f8d9ff, // Dynamic
    0x030424e8f89967, // Debris
    0x01142018f09925, // DetectionZone
    0x01c426e4f89abf, // Character
    0x004404e80808cf, // Ragdoll
    0x030424f8f89967, // Raycast
    0x014004e908099f, // NoCharacterCollision
    0x008c2018f2d021, // RaycastOnlyCharacters
    0x00980800000000, // Nothing
    0x03cc24e8bac9ff, // Interactive
    0x014425e0f892bf, // CharacterNoInteractive
    0x00040ce0080003, // OnlyStatic
    0x008d01f8080a07, // RaycastNoCharacters
    0x0046366f6f9abf, // Horse
    0x000cd000058001, // HorseBarrier
    0x008c90a5048a03, // Camera
    0x008cd000078001, // CameraBarrier
    0x03cd25e8f8f9ef, // StaticNoCamera
    0x01442050f81abd, // CharacterNoStatic
    0x01c222e7f89abf, // MainCharacter
    0x014626e7f892bf, // MainCharacterNoInteractive
    0x01462054f81abd, // MainCharacterNoStatic
    0x01bf6207628101, // FuzzyZone
    0x01bf6207608001, // FuzzyZoneNoCamera
    0x01ff6207e28021, // FuzzyZoneAllCharacter
    0x03cc04f808cbdf, // OnlyStaticAndCameraRaycast
    0x008c0808904291, // HollywoodMode
    0x03cc2ce86af9ef, // StaticNoLOS
    0x03cc2ce8f8f9ef, // StaticNoCameraNoLOS
    0x03cd2de86a79ef, // StaticNoHorse
    0x009d1d80085003, // OnlyLOS
    0x01ff6207608021, // FuzzyZoneAllCharacterNoCamera
    0x034505e848b9ef, // NPCBarrier
    0x009f49f0002407, // OcclusionHeightTest
    0x008dd100078001, // CameraBarrierPlusLOS
    0x0146e2e7f89abf, // Projectile
    0x001efa07050001, // ProjectileBarrier
    0x000cf000070001, // ProjectileCameraBarrier
    0x009d1f87084003, // OnlyLOSAndFuzzy
    0x01be6a07e08001, // FuzzyZoneNoCameraAllowAssassination
    0x02cfffffdffaff, // HorseNoMainCharacterCollision
    0x00b7dbff0f4e07, // RaycastNoCharactersFuzzyZone
    0x00bb4b07000414, // OnlyDynamic
    0x009a0207000000, // OnlyFuzzy
    0x030426ecf89967, // NPCSpawning
    0x003f1bff2e4e27, // RaycastFuzzyZone
    0x014226eff819bf, // NoHorseCollision
    0x024404e808088f, // RagdollPenetrationSolver
];

/// Do layers `a` and `b` collide?
pub fn collides(a: u8, b: u8) -> bool {
    (a as usize) < 50 && (b as usize) < 50 && COLLIDES[a as usize] >> b & 1 != 0
}
