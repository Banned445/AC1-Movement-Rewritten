//! Authored wind: the game's global force source (scimitar::Wind, RE/09 §8.16).
//! Read by the robe (SoftBody__ComputeAcceleration 0x4D0EC0) and the skeleton's hinges (0x4E7550).
use bevy::prelude::*;

// PORT: the greybox has no authored Wind; use Masyaf's so the robe moves as it does in the game.
const GREYBOX_AUTHORED_WIND: bool = true;

/// Knuth's subtractive generator as seeded by Random__Seed 0x7BFD40 (modulus 10⁹).
struct Subtractive { ma: [i32; 56], inext: usize, inextp: usize }

impl Subtractive {
    fn new(seed: i32) -> Self {
        const MBIG: i32 = 1_000_000_000;
        let mut ma = [0; 56];
        let mut mj = (161_803_398 - seed.abs()).abs() % MBIG;
        ma[55] = mj;
        let mut mk = 1;
        for i in 1..55 {
            let ii = 21 * i % 55;
            ma[ii] = mk;
            mk = mj - mk;
            if mk < 0 { mk += MBIG; }
            mj = ma[ii];
        }
        for _ in 0..4 {
            for i in 1..56 {
                ma[i] -= ma[1 + (i + 30) % 55];
                if ma[i] < 0 { ma[i] += MBIG; }
            }
        }
        Self { ma, inext: 0, inextp: 31 }
    }

    /// One draw in [0, 1): f32(f64(f32(value)) × 1e-9) (0x973920 / 0x973A4F).
    fn next(&mut self) -> f32 {
        self.inext = if self.inext == 55 { 1 } else { self.inext + 1 };
        self.inextp = if self.inextp == 55 { 1 } else { self.inextp + 1 };
        let mut mj = self.ma[self.inext] - self.ma[self.inextp];
        if mj < 0 { mj += 1_000_000_000; }
        self.ma[self.inext] = mj;
        (mj as f32 as f64 * 9.999999717180685e-10) as f32
    }
}

/// The 1D gradient-noise tables built at startup (Noise__InitPermutation 0x973850, Noise__InitGradients 0x9739B0).
pub struct NoiseTables { perm: [u8; 256], grad: [f32; 256] }

impl NoiseTables {
    pub fn new() -> Self {
        let mut perm: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut rng = Subtractive::new(84);
        for v in 0..256 {
            let k = ((rng.next() as f64 * 256.0) as f32) as i32 as u8 as usize;
            perm[v] = perm[k];
            perm[k] = v as u8;
        }
        let mut rng = Subtractive::new(42);
        let grad = std::array::from_fn(|_| (rng.next() as f64 * 2.0 - 1.0) as f32);
        Self { perm, grad }
    }

    /// Noise__Perlin1D 0x971B10.
    pub fn noise(&self, x: f32) -> f32 {
        let x = (x as f64 + 4096.0) as f32;
        let i = x as i32;
        let f = (x as f64 - i as f32 as f64) as f32;
        let a = self.grad[self.perm[(i & 255) as usize] as usize] * f;
        let b = self.grad[self.perm[((i + 1) & 255) as usize] as usize] * (f - 1.0);
        a + (b - a) * ((3.0 - (f + f)) * (f * f))
    }
}

/// One serialized Wind (Wind__Read 0x5C7F20) placed by its owner entity.
#[derive(Clone, Debug)]
pub struct WindSource {
    pub strength: f32,
    pub noise: bool,
    /// Noise percentages +132 / +136 / +140 / +144.
    pub percent: [f32; 4],
    /// Owner entity X axis × local matrix (ForceSource__Direction 0x5C79C0, mode 0), native Z-up.
    pub direction: Vec3,
}

/// Decode a Wind component from its owner Entity resource; `transform` is the entity's native placement.
/// PORT: only the authored subset is accepted (no regions, mode 0 without oscillation, kind 0).
pub fn decode_wind(payload: &[u8], transform: Mat4) -> Result<Option<WindSource>, String> {
    let class = crate::assets::forge::crc32("Wind");
    let word = |p: usize| payload.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let Some(p) = (5..payload.len().saturating_sub(4)).find(|&p| word(p) == Some(class)) else { return Ok(None); };
    let decode = || -> Option<WindSource> {
        let b = p + 4;
        let float = |o: usize| word(b + o).map(f32::from_bits).filter(|v| v.is_finite());
        if *payload.get(b)? != 1 { return None; } // component active byte (0x5271C0)
        let strength = float(1)?;
        let noise = match *payload.get(b + 5)? { 0 => false, 1 => true, _ => return None };
        let percent = [float(6)?, float(10)?, float(14)?, float(18)?];
        if word(b + 22)? != 0 { return None; } // regions (0x5C7620)
        let mode = word(b + 26)?;
        let oscillation = [float(30)?, float(34)?, float(38)?];
        let kind = word(b + 42)?;
        if mode != 0 || oscillation != [0.0; 3] || kind != 0 { return None; }
        let local: [f32; 16] = std::array::from_fn(|k| float(86 + 4 * k).unwrap_or(f32::NAN));
        if local.iter().any(|v| !v.is_finite()) { return None; }
        let direction = (transform * Mat4::from_cols_array(&local)).transform_vector3(Vec3::X).try_normalize()?;
        Some(WindSource { strength, noise, percent, direction })
    };
    decode().map(Some).ok_or_else(|| "unsupported wind layout".into())
}

impl WindSource {
    /// ForceSource__Evaluate 0x5C8900 at native position `p` and wind time `t` (seconds since creation).
    pub fn force(&self, tables: &NoiseTables, p: Vec3, t: f32) -> Vec3 {
        let mut direction = self.direction;
        if direction.abs().max_element() <= 0.001 { return Vec3::ZERO; }
        if self.noise {
            // Wind__NoiseVector 0x5C66D0 / Wind__NoiseMagnitude 0x5C67D0; .data constants 3, 2, 1, 6.
            let a = 3.0 * (self.percent[0] / 100.0);
            let b = 2.0 * (self.percent[1] / 100.0) * t;
            let n = Vec3::new(tables.noise(b + p.x), tables.noise(b + p.y), tables.noise(b + p.z));
            direction = (direction + n * a).normalize_or_zero();
            let magnitude = tables.noise(6.0 * (self.percent[3] / 100.0) * t + (p.y + p.x + p.z)).abs() + self.percent[2] / 100.0;
            direction *= magnitude;
        }
        direction * self.strength
    }
}

/// Global force sources (ForceSources__SumGlobal 0x483E60) and their clock.
#[derive(Resource)]
pub struct WindField {
    pub tables: NoiseTables,
    pub sources: Vec<WindSource>,
    /// Game time when the sources were created (map load); Timer__ElapsedSeconds 0x5C6100.
    pub created: f32,
    pub time: f32,
}

impl Default for WindField {
    fn default() -> Self { Self { tables: NoiseTables::new(), sources: Vec::new(), created: 0.0, time: 0.0 } }
}

fn native(p: Vec3) -> Vec3 { Vec3::new(p.x, -p.z, p.y) }
fn bevy(v: Vec3) -> Vec3 { Vec3::new(v.x, v.z, -v.y) }

impl WindField {
    /// Sum of global sources at a Bevy-space position, in Bevy space (ForceField__SampleForce 0x5C8A60 with a
    /// config of scale `scale`; the authored robe and skeleton configs use scale 1 and global sources only).
    pub fn sample(&self, position: Vec3, scale: f32) -> Vec3 {
        let p = native(position);
        bevy(self.sources.iter().map(|s| s.force(&self.tables, p, self.time)).sum::<Vec3>() * scale)
    }

    pub fn replace(&mut self, sources: Vec<WindSource>, now: f32) {
        self.sources = sources;
        self.created = now;
        self.time = 0.0;
    }
}

/// Each playable area authors one Wind with the same settings and its own placement, e.g. `Wind_Masyaf_001`
/// (DataPC_Masyaf.forge / Cell05460_DataBlock) or `Wind_Damascus_001` (DataPC_Damascus.forge / Cell01364_DataBlock).
pub const MASYAF_WIND: (&str, &str) = ("DataPC_Masyaf.forge", "Cell05460_DataBlock");

pub fn load_wind(game: &std::path::Path, (archive, cell): (&str, &str)) -> Result<Vec<WindSource>, String> {
    let mut forge = crate::assets::forge::Forge::open(&game.join(archive)).map_err(|e| e.to_string())?;
    let entry = forge.find(cell).cloned().ok_or_else(|| format!("missing wind cell {cell}"))?;
    let mut sources = Vec::new();
    for r in forge.resources(&entry).map_err(|e| e.to_string())? {
        if r.class_hash != crate::assets::forge::crc32("Entity") { continue; }
        let transform = crate::assets::world::entity_transform(&r.payload).ok();
        let Some(transform) = transform else { continue; };
        if let Some(source) = decode_wind(&r.payload, transform)? { sources.push(source); }
    }
    Ok(sources)
}

fn startup_wind(mut wind: ResMut<WindField>, time: Res<Time>) {
    if !GREYBOX_AUTHORED_WIND || std::env::var("AC_GREYBOX_WIND").is_ok_and(|v| v == "0") { return; }
    match load_wind(&crate::assets::game_dir(), MASYAF_WIND) {
        Ok(sources) => wind.replace(sources, time.elapsed_secs()),
        Err(e) => warn!("authored wind not loaded: {e}"),
    }
}

/// Wind__UpdateTime 0x5C7600: seconds since the sources were created, in game time.
fn advance_wind(mut wind: ResMut<WindField>, time: Res<Time>) {
    wind.time = time.elapsed_secs() - wind.created;
}

pub struct WindPlugin;

impl Plugin for WindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindField>().add_systems(Startup, startup_wind).add_systems(First, advance_wind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_a_permutation_and_unit_gradients() {
        let mut rng = Subtractive::new(1);
        assert!((0..1000).map(|_| rng.next()).all(|v| (0.0..1.0).contains(&v)));
        // 0x973850 writes P[v] = P[k]; P[k] = v, which is not a strict shuffle: values may repeat.
        let tables = NoiseTables::new();
        assert!(tables.grad.iter().all(|g| (-1.0..1.0).contains(g)));
    }

    #[test]
    fn authored_wind_reproduces_a_live_sample() {
        let game = crate::assets::game_dir();
        if !game.join("DataPC_Masyaf.forge").is_file() { eprintln!("skipped: Masyaf archive absent"); return; }
        assert!(load_wind(&game, MASYAF_WIND).unwrap()[0].direction.distance(Vec3::new(-0.8836, 0.4683, 0.0)) < 1e-3);
        if !game.join("DataPC_Damascus.forge").is_file() { eprintln!("skipped: Damascus archive absent"); return; }
        // The live session ran in Damascus; its Wind matched the running entity's placement.
        let sources = load_wind(&game, ("DataPC_Damascus.forge", "Cell01364_DataBlock")).unwrap();
        assert_eq!(sources.len(), 1);
        let s = &sources[0];
        assert_eq!(s.strength, 10.0);
        assert!(s.noise);
        assert!((s.percent[0] - 61.3793).abs() < 1e-3 && (s.percent[3] - 11.7241).abs() < 1e-3);
        assert!(s.direction.distance(Vec3::new(0.852, -0.523, 0.0)) < 2e-3);
        // Cheat Engine read at the robe's entity, t = 217.3512 s: stored Cloth +32 (RE/09 §8.16).
        let field = WindField { sources, time: 217.3512, ..default() };
        let f = field.sample(bevy(Vec3::new(54.472, 7.846, 11.452)), 1.0);
        assert!(native(f).distance(Vec3::new(3.2603, -4.2908, -0.4926)) < 2e-3, "{:?}", native(f));
    }

    #[test]
    fn noise_is_zero_on_integers_and_continuous() {
        let t = NoiseTables::new();
        for i in -5..5 { assert!(t.noise(i as f32).abs() < 1e-6); }
        let a = t.noise(3.4999);
        let b = t.noise(3.5001);
        assert!((a - b).abs() < 1e-3);
    }

    #[test]
    fn noiseless_wind_blows_along_its_entity_axis() {
        let source = WindSource { strength: 10.0, noise: false, percent: [0.0; 4], direction: Vec3::new(0.6, -0.8, 0.0) };
        let field = WindField { sources: vec![source], ..default() };
        let f = field.sample(Vec3::ZERO, 1.0);
        assert!(f.distance(Vec3::new(6.0, 0.0, 8.0)) < 1e-5, "native (x, y, z) maps to Bevy (x, z, -y)");
    }

    /// The live tables dumped from the running game (RE/09 §8.16); run with AC_NOISE_TABLES=<file>.
    #[test]
    #[ignore]
    fn tables_match_the_running_game() {
        let path = std::env::var("AC_NOISE_TABLES").expect("AC_NOISE_TABLES");
        let text = std::fs::read_to_string(path).unwrap();
        let mut lines = text.lines();
        let perm: Vec<u8> = lines.next().unwrap().split_whitespace().map(|v| v.parse().unwrap()).collect();
        let grad: Vec<u32> = lines.next().unwrap().split_whitespace().map(|v| u32::from_str_radix(v, 16).unwrap()).collect();
        let t = NoiseTables::new();
        assert_eq!(&perm[..256], &t.perm[..]);
        assert_eq!(&perm[256..], &t.perm[..], "second copy (0x1A225C8)");
        for (i, (&live, &ours)) in grad.iter().zip(&t.grad).enumerate() { assert_eq!(live, ours.to_bits(), "gradient {i}"); }
    }
}
