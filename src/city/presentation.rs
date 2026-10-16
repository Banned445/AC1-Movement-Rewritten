//! The city's presentation besides its meshes: the local lights, the particle effects and the ambient sound emitters
//! placed in the cells (RE/17 §11).
//!
//! A cell entity with a `LightInstance` component (class 0x554C614C: a `Component` base, then `Light`, an owned
//! reference, and the instance's `LightFlare`) points at an `OmniLight` resource stored with the cells (Damascus: 64
//! of them shared by 463 instances, no `SpotLight`). The light sits at the entity's placement.

use std::{collections::HashMap, sync::Arc};

use bevy::{asset::RenderAssetUsages, camera::visibility::NoFrustumCulling, light::{NotShadowCaster, NotShadowReceiver}, mesh::PrimitiveTopology,
    prelude::*};

use super::material::TextureData;
use crate::assets::{forge::crc32, particles::{parse_particle_system, BlendMode, ParticleSystemDef}, static_mesh::word};
use crate::particles::{EmitterSim, Frame, Lcg};

/// A local light of a loaded cell (Bevy space).
#[derive(Clone, Copy, Debug)]
pub struct CityLight {
    pub position: Vec3,
    /// `LightColor` (sRGB, 0..1).
    pub color: Vec3,
    pub intensity: f32,
    pub near: f32,
    pub far: f32,
    /// The light's world flag (serialised flag byte 3): set on the world lights, clear on the `_Char` copies that only
    /// light characters (byte 2) and on the candle / fire lights of the effects (byte 9 or none).
    pub world: bool,
}

/// The `OmniLight` resource layout (101 bytes in Damascus):
/// `[id][class]`, `GraphicObject` u32, `LightColor` as an embedded `Color` (`[id][class 0xA79767ED]` + 4 floats RGBA),
/// ten flag bytes, `Intensity` f32, the light's `LightFlare` (embedded, variable), then `OmniLight`'s `Near`, `Far`.
/// Names by CRC32 of the reflected property hashes (`Light` 0xAADB73F3, `OmniLight` 0x344780D6).
pub fn omni_light(payload: &[u8]) -> Option<(Vec3, [u8; 10], f32, f32, f32)> {
    if word(payload, 4).ok()? != crc32("OmniLight") || word(payload, 16).ok()? != 0xA797_67ED || payload.len() < 58 { return None; }
    let f = |o: usize| word(payload, o).ok().map(f32::from_bits);
    let color = Vec3::new(f(20)?, f(24)?, f(28)?);
    let flags: [u8; 10] = payload[36..46].try_into().ok()?;
    let intensity = f(46)?;
    let n = payload.len();
    Some((color, flags, intensity, f(n - 8)?, f(n - 4)?))
}

/// The lights of one entity body placed at `position` (Bevy space). `resolve` reads a resource by id.
pub fn body_lights(b: &[u8], position: Vec3, resolve: impl Fn(u32) -> Option<Vec<u8>>) -> Vec<CityLight> {
    let class = crc32("LightInstance").to_le_bytes();
    let mut out = Vec::new();
    for p in b.windows(4).enumerate().filter(|(_, w)| *w == class).map(|(p, _)| p) {
        // the component's u8 (1) and the owned `Light`'s reference
        if b.get(p + 4) != Some(&1) { continue; }
        let Ok(id) = word(b, p + 5) else { continue };
        let Some((color, flags, intensity, near, far)) = resolve(id).as_deref().and_then(omni_light) else { continue };
        out.push(CityLight { position, color, intensity, near, far, world: flags[3] != 0 });
    }
    out
}

/// The Bevy point light for a city light. PORT: the game's omni-light pixel shaders sit in the data's shader cache
/// (not the exe); the live constants (RE/17 §11) hold each light as (position, Far²) and (colour · Intensity, 1 / Far²).
/// Bevy's inverse-square falloff (windowed at `range`) is matched to a `1 − d² / Far²` falloff at half of `Far`,
/// under the city's exposure (`ambiance::exposure`: `SUN_LUX / π` is white).
pub fn point_light(l: &CityLight) -> PointLight {
    let at = l.far * 0.5;
    let lux = super::ambiance::SUN_LUX * l.intensity * (1.0 - 0.25);
    PointLight {
        color: Color::srgb(l.color.x, l.color.y, l.color.z),
        intensity: lux * 4.0 * std::f32::consts::PI * at * at,
        range: l.far.max(0.1),
        radius: 0.05,
        shadow_maps_enabled: false,
        ..default()
    }
}

/// Spawn a cell's lights; returns their entities (the cell despawns them when it unloads).
pub fn spawn_lights(lights: &[CityLight], commands: &mut Commands) -> Vec<Entity> {
    lights.iter().filter(|l| l.world && l.intensity > 0.0 && l.far > 0.0)
        .map(|l| commands.spawn((crate::map_menu::MapEntity, point_light(l), Transform::from_translation(l.position))).id())
        .collect()
}

/// A particle system placed in a cell: an entity whose Visual draws a `ParticleSystem` (Damascus: 809 of them, the
/// fires, smoke, dust, birds …).
#[derive(Clone)]
pub struct CityEffect {
    /// The entity's frame in game space (unit axes) and its scale.
    pub frame: Frame,
    pub scale: f32,
    pub system: Arc<ParticleSystemDef>,
    /// The emitters' TextureMaps.
    pub textures: Vec<(u32, Arc<TextureData>)>,
}

/// The effect an entity body draws, when its Visual's drawable is a `ParticleSystem` (`placement`: the game matrix).
pub fn body_effect(data: &super::decode::CityData, b: &[u8], placement: Mat4, systems: &mut HashMap<u32, Option<Arc<ParticleSystemDef>>>)
    -> Option<Result<CityEffect, String>> {
    let v = *super::decode::markers(b, "Visual").first()?;
    if b.get(v + 4) != Some(&1) { return None; }
    let drawable = word(b, v + 5).ok().filter(|&d| d != 0)?;
    let is_system = |id: u32| data.archives.get(id).ok().filter(|r| r.class_hash == crc32("ParticleSystem"));
    let system = systems.entry(drawable).or_insert_with(|| is_system(drawable).and_then(|r| parse_particle_system(&r.payload).ok()).map(Arc::new)).clone();
    let system = match system {
        Some(s) => s,
        None => return is_system(drawable).map(|_| Err("particle system: not parsed".to_string())),
    };
    let mut textures = Vec::new();
    for e in &system.emitters {
        if e.texture == 0 || textures.iter().any(|(id, _)| *id == e.texture) { continue; }
        match data.materials.texture(&data.archives, e.texture, false, data.bc) {
            Ok(t) => textures.push((e.texture, t)),
            Err(e) => return Some(Err(format!("particle texture: {e}"))),
        }
    }
    let axis = |c: Vec4| c.truncate();
    let scale = axis(placement.x_axis).length().max(1e-4);
    let frame = Frame { x: axis(placement.x_axis) / scale, y: axis(placement.y_axis).normalize_or(Vec3::Y), z: axis(placement.z_axis).normalize_or(Vec3::Z),
        t: axis(placement.w_axis) };
    Some(Ok(CityEffect { frame, scale, system, textures }))
}

/// Effects run within this distance of the camera (PORT: the game's own particle culling, `CameraRelativeSpawn` and
/// the LOD of the particle systems are not traced).
pub const EFFECT_DISTANCE: f32 = 60.0;

struct RunningEffect {
    cell: usize,
    effect: CityEffect,
    /// Per emitter: the simulation and its mesh entity, while within `EFFECT_DISTANCE`.
    running: Option<Vec<(EmitterSim, Entity)>>,
}

/// The placed effects of the loaded cells (filled from the streamer's hand-off, `CityState::effects_out`).
#[derive(Resource, Default)]
pub struct CityEffects {
    effects: Vec<RunningEffect>,
    images: HashMap<u32, Handle<Image>>,
    materials: HashMap<(u32, u8), Handle<StandardMaterial>>,
    rng: Option<Lcg>,
}

impl CityEffects {
    /// (placed effects of the loaded cells, effects running near the camera) for the overlay.
    pub fn counts(&self) -> (usize, usize) { (self.effects.len(), self.effects.iter().filter(|e| e.running.is_some()).count()) }
    fn stop(r: &mut RunningEffect, commands: &mut Commands) {
        for (_, e) in r.running.take().into_iter().flatten() { commands.entity(e).despawn(); }
    }
}

fn effect_material(fx: &mut CityEffects, textures: &[(u32, Arc<TextureData>)], texture: u32, blend: BlendMode, images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    let key = (texture, blend as u8);
    if let Some(h) = fx.materials.get(&key) { return h.clone(); }
    let image = textures.iter().find(|(id, _)| *id == texture).map(|(id, t)| {
        fx.images.entry(*id).or_insert_with(|| images.add(super::stream::image(t, false))).clone()
    });
    // BlendMode (+80 bits 21-23), as the contact FX (`particles::material_for`)
    let alpha_mode = match blend {
        BlendMode::Additive | BlendMode::Dodge => AlphaMode::Add,
        BlendMode::Modulate => AlphaMode::Multiply,
        BlendMode::Copy => AlphaMode::Opaque,
        BlendMode::Subtractive | BlendMode::Alpha => AlphaMode::Blend,
    };
    let h = materials.add(StandardMaterial { base_color_texture: image, unlit: true, alpha_mode, cull_mode: None, double_sided: true, ..default() });
    fx.materials.insert(key, h.clone());
    h
}

/// Take the streamer's new and unloaded cells' effects, start / stop them by the camera distance and run them (the
/// contact FX's emitter code, `particles::EmitterSim`).
#[allow(clippy::too_many_arguments)]
pub fn update_effects(mut city: ResMut<super::stream::City>, mut fx: ResMut<CityEffects>, contact: Res<crate::particles::ContactFx>, time: Res<Time>,
    wind: Res<crate::wind::WindField>, camera: Query<&GlobalTransform, With<crate::camera::MainCamera>>, mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<StandardMaterial>>, mut handles: Query<&mut Mesh3d>) {
    let fx = &mut *fx;
    let Some(state) = city.0.as_mut() else {
        for mut r in fx.effects.drain(..) { CityEffects::stop(&mut r, &mut commands); }
        return;
    };
    for c in state.effects_unloaded.drain(..) {
        for r in fx.effects.iter_mut().filter(|r| r.cell == c) { CityEffects::stop(r, &mut commands); }
        fx.effects.retain(|r| r.cell != c);
    }
    for (cell, effect) in state.effects_out.drain(..) { fx.effects.push(RunningEffect { cell, effect, running: None }); }
    let Ok(cam) = camera.single() else { return };
    let eye = to_game(cam.translation());
    let (right, up) = (cam.right().as_vec3(), cam.up().as_vec3());
    let dt = time.delta_secs().min(0.1);
    let mut rng = fx.rng.take().unwrap_or_else(|| Lcg::new(0x2008_0DA5));
    let mut effects = std::mem::take(&mut fx.effects);
    for r in &mut effects {
        if r.effect.frame.t.distance(eye) >= EFFECT_DISTANCE { CityEffects::stop(r, &mut commands); continue; }
        if r.running.is_none() {
            let mut list = Vec::new();
            for e in &r.effect.system.emitters {
                let frame = Frame::from_rows(&e.local_matrix).then(&r.effect.frame);
                let material = effect_material(fx, &r.effect.textures, e.texture, e.blend, &mut images, &mut materials);
                let sim = EmitterSim::new(frame, r.effect.scale);
                // the mesh has its full vertex layout from the start (the pipeline is keyed on it)
                let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
                crate::particles::build_quads(&mut mesh, &sim, &r.effect.system, e, right, up);
                let entity = commands.spawn((crate::map_menu::MapEntity, Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::IDENTITY,
                    NoFrustumCulling, NotShadowCaster, NotShadowReceiver)).id();
                list.push((sim, entity));
            }
            r.running = Some(list);
        }
        let sys = &r.effect.system;
        for ((sim, entity), e) in r.running.as_mut().unwrap().iter_mut().zip(&sys.emitters) {
            let w = to_game(wind.sample(super::decode::to_bevy(sim.frame.t), 1.0));
            sim.update(sys, e, dt, w, &mut rng, &contact.hash);
            if let Ok(mut m) = handles.get_mut(*entity) {
                if let Some(mut mesh) = meshes.get_mut(&mut m.0) { crate::particles::build_quads(&mut mesh, sim, sys, e, right, up); }
            }
        }
    }
    fx.effects = effects;
    fx.rng = Some(rng);
}

fn to_game(v: Vec3) -> Vec3 { Vec3::new(v.x, -v.z, v.y) }

/// An ambient sound emitter placed in a cell: an entity's `Ambient3dSoundComponent` (Damascus: 1547; palms, gardens,
/// fountains, lanterns, kiosks, braziers …). Its first `SoundDescriptor` is the start event (the second, when there is
/// one, its stop event), `[class][u32 DARE event][u32 SoundBao resource]`.
#[derive(Clone, Copy, Debug)]
pub struct CitySound {
    /// Bevy space.
    pub position: Vec3,
    pub event: u32,
    /// The event's SoundBao forge resource (its wrapper lists the sound and audio resources it needs).
    pub resource: u32,
}

/// The ambient emitters of one entity body placed at `position` (Bevy space).
pub fn body_sounds(b: &[u8], position: Vec3) -> Vec<CitySound> {
    let emitters = super::decode::markers(b, "Ambient3dSoundComponent");
    let descriptors = super::decode::markers(b, "SoundDescriptor");
    let mut out = Vec::new();
    for (k, &p) in emitters.iter().enumerate() {
        let end = emitters.get(k + 1).copied().unwrap_or(b.len());
        let Some(&q) = descriptors.iter().find(|&&q| q > p && q < end) else { continue };
        let (Ok(event), Ok(resource)) = (word(b, q + 4), word(b, q + 8)) else { continue };
        if event >> 28 == 1 && resource != 0 { out.push(CitySound { position, event, resource }); }
    }
    out
}

/// PORT: one-shot voices of an ambient emitter (the birds and leaves; sounds with a frame count) start again after a
/// random pause in this range (seconds); the game's ambient scheduling is not traced. Voices whose sound stores no
/// frame count (fountains, fires, lanterns) loop.
const RETRIGGER: (f32, f32) = (2.0, 9.0);

struct RunningSound {
    cell: usize,
    sound: CitySound,
    /// (full, silent) distances of the start event.
    range: (f32, f32),
    /// Per voice: its entity while playing, the loop flag, the gain, the time to the next start of a one-shot.
    voices: Option<Vec<(Option<Entity>, crate::assets::dare::Voice, bool, f32)>>,
}

/// The ambient emitters of the loaded cells and the sound objects they use.
#[derive(Resource, Default)]
pub struct CityAmbience {
    sounds: Vec<RunningSound>,
    bank: crate::assets::dare::SoundBank,
    loaded: std::collections::HashSet<u32>,
    clips: HashMap<u32, Option<Handle<crate::sound::PcmClip>>>,
    picker: Option<crate::assets::dare::Picker>,
    listener: Option<Entity>,
    beds: Option<Beds>,
}

impl CityAmbience {
    /// (emitters of the loaded cells, emitters playing) for the overlay.
    pub fn counts(&self) -> (usize, usize) { (self.sounds.len(), self.sounds.iter().filter(|s| s.voices.is_some()).count()) }
    /// The ambience sets playing (names), for the overlay.
    pub fn beds(&self) -> Vec<&str> { self.beds.iter().flat_map(|b| b.playing.iter().filter(|p| p.wanted).map(|p| p.set.name.as_str())).collect() }
    fn stop(s: &mut RunningSound, commands: &mut Commands) {
        for (e, ..) in s.voices.take().into_iter().flatten() {
            if let Some(e) = e { commands.entity(e).try_despawn(); }
        }
    }
    /// The SoundBao resource and every SoundBao its wrapper lists, into the bank.
    fn load(&mut self, archives: &super::archive::Archives, resource: u32) {
        let mut todo = vec![resource];
        while let Some(id) = todo.pop() {
            if !self.loaded.insert(id) { continue; }
            let Ok(r) = archives.get(id) else { continue };
            if r.class_hash != crate::assets::dare::CLASS_SOUND_BAO { continue; }
            self.bank.insert(r.id, &r.name, &r.payload);
            let wrapper = r.payload.len() - crate::assets::dare::bao_object(&r.payload).map_or(r.payload.len(), |o| o.len());
            todo.extend((0..wrapper.saturating_sub(3)).filter_map(|p| word(&r.payload, p).ok()).filter(|&d| d != id && archives.contains(d)));
        }
    }
    #[cfg(test)]
    pub fn load_for_test(&mut self, archives: &super::archive::Archives, resource: u32) { self.load(archives, resource); }
    #[cfg(test)]
    pub fn bank_for_test(&self, event: u32) -> (&crate::assets::dare::SoundBank, Option<(f32, f32)>) { (&self.bank, self.bank.event_distances(event)) }
    fn clip(&mut self, v: &crate::assets::dare::Voice, clips: &mut Assets<crate::sound::PcmClip>) -> Option<Handle<crate::sound::PcmClip>> {
        let bank = &self.bank;
        self.clips.entry(v.res).or_insert_with(|| {
            let pcm = bank.object(v.res).and_then(|o| crate::assets::dare::decode_ima(o, v.channels, v.frames))?;
            Some(clips.add(crate::sound::PcmClip { channels: v.channels, rate: v.rate, data: pcm.into() }))
        }).clone()
    }
}

/// The gain at a distance: full inside the event's first distance, silent past its second (PORT, hypothesis: a linear
/// fade between them; the DARE 3D rolloff is not traced).
fn falloff(range: (f32, f32), d: f32) -> f32 {
    if d <= range.0 { 1.0 } else if d >= range.1 { 0.0 } else { 1.0 - (d - range.0) / (range.1 - range.0).max(1e-3) }
}

fn start_voice(commands: &mut Commands, handle: Handle<crate::sound::PcmClip>, v: &crate::assets::dare::Voice, looped: bool, at: Vec3, gain: f32) -> Entity {
    let mode = if looped { bevy::audio::PlaybackMode::Loop } else { bevy::audio::PlaybackMode::Despawn };
    commands.spawn((crate::map_menu::MapEntity, AudioPlayer::<crate::sound::PcmClip>(handle), Transform::from_translation(at),
        PlaybackSettings { mode, volume: bevy::audio::Volume::Linear(gain), speed: v.speed, spatial: true,
            // the distance gain is applied here (`falloff`); the spatial sink only places the voice left / right
            spatial_scale: Some(bevy::audio::SpatialScale::new(0.02)), ..default() })).id()
}

/// Take the streamer's new and unloaded cells' emitters, start the ones in hearing range and keep their gain to the
/// listener's distance.
#[allow(clippy::too_many_arguments)]
pub fn update_ambience(mut city: ResMut<super::stream::City>, mut amb: ResMut<CityAmbience>, sounds: Res<crate::sound::Sounds>, time: Res<Time>,
    camera: Query<(Entity, &GlobalTransform), With<crate::camera::MainCamera>>, mut commands: Commands, mut clips: ResMut<Assets<crate::sound::PcmClip>>,
    mut sinks: Query<&mut bevy::audio::SpatialAudioSink>, mut bed_sinks: Query<&mut AudioSink>) {
    use bevy::audio::AudioSinkPlayback;
    let amb = &mut *amb;
    let Some(state) = city.0.as_mut() else {
        for mut s in amb.sounds.drain(..) { CityAmbience::stop(&mut s, &mut commands); }
        for p in amb.beds.take().into_iter().flat_map(|b| b.playing) { for (e, _) in p.pads { commands.entity(e).try_despawn(); } }
        if let Some(l) = amb.listener.take() { commands.entity(l).try_remove::<SpatialListener>(); }
        return;
    };
    let mut beds = amb.beds.take().unwrap_or_default();
    for (cell, sets) in state.ambience_out.drain(..) { beds.by_cell.insert(cell, sets); }
    for &c in &state.sounds_unloaded { beds.by_cell.remove(&c); }
    for c in state.sounds_unloaded.drain(..) {
        for s in amb.sounds.iter_mut().filter(|s| s.cell == c) { CityAmbience::stop(s, &mut commands); }
        amb.sounds.retain(|s| s.cell != c);
    }
    let archives = state.data.archives.clone();
    for (cell, sound) in state.sounds_out.drain(..) {
        amb.load(&archives, sound.resource);
        let range = amb.bank.event_distances(sound.event).unwrap_or((2.0, 10.0));
        amb.sounds.push(RunningSound { cell, sound, range, voices: None });
    }
    let Ok((cam, at)) = camera.single() else { amb.beds = Some(beds); return };
    if amb.listener != Some(cam) {
        commands.entity(cam).insert(SpatialListener::new(0.3));
        amb.listener = Some(cam);
    }
    let ear = at.translation();
    let dt = time.delta_secs();
    let enabled = sounds.enabled && std::env::var_os("AC_NO_SOUND").is_none();
    let mut picker = amb.picker.take().unwrap_or_else(|| crate::assets::dare::Picker::new(0x2008_0DA5));
    let mut list = std::mem::take(&mut amb.sounds);
    for s in &mut list {
        let d = s.sound.position.distance(ear);
        if !enabled || d > s.range.1 + 2.0 { CityAmbience::stop(s, &mut commands); continue; }
        if s.voices.is_none() {
            let voices = amb.bank.resolve(s.sound.event, &|_| None, &mut picker);
            s.voices = Some(voices.into_iter().map(|v| {
                let looped = v.frames == 0;
                // a one-shot starts after a random part of the pause, so neighbours do not start together
                let wait = if looped { 0.0 } else { picker_range(&mut picker, 0.0, RETRIGGER.1) };
                (None, v, looped, wait)
            }).collect());
        }
        let falloff = falloff(s.range, d);
        let mut voices = s.voices.take().unwrap();
        for (entity, v, looped, wait) in &mut voices {
            let gain = bevy::audio::Volume::Decibels(v.gain_db).to_linear() * falloff;
            match entity {
                Some(e) if sinks.get(*e).is_ok() => {
                    if let Ok(mut sink) = sinks.get_mut(*e) { sink.set_volume(bevy::audio::Volume::Linear(gain)); }
                }
                Some(e) if *looped => { let _ = e; }
                _ => {
                    *wait -= dt;
                    if *wait > 0.0 { continue; }
                    *wait = if *looped { f32::INFINITY } else { picker_range(&mut picker, RETRIGGER.0, RETRIGGER.1) };
                    // a one-shot picks again from its containers each time
                    let v = if *looped { v.clone() } else {
                        amb.bank.resolve(s.sound.event, &|_| None, &mut picker).into_iter().next().unwrap_or_else(|| v.clone())
                    };
                    let Some(handle) = amb.clip(&v, &mut clips) else { continue };
                    *entity = Some(start_voice(&mut commands, handle, &v, *looped, s.sound.position, gain));
                }
            }
        }
        s.voices = Some(voices);
    }
    amb.sounds = list;
    update_beds(&mut beds, amb, &archives, &state.data.grid, ear, dt, enabled, &mut picker, &mut commands, &mut clips, &mut bed_sinks);
    amb.beds = Some(beds);
    amb.picker = Some(picker);
}

fn picker_range(p: &mut crate::assets::dare::Picker, a: f32, b: f32) -> f32 {
    a + (b - a) * p.next01()
}

/// A `SoundAmbienceSet` stored in a cell file (Damascus: the wind, the rich / poor quarters, the markets, the crowd
/// "walla" sets, the Bureau's room …): looping pads and random one-shots. Layout: `[id][class]`, the pad list
/// (`SoundAmbiencePad` … `SoundInstance` → `SoundDescriptor` `[class][u32 event][u32 SoundBao]`), then the random FX
/// (`SoundAmbienceRandomFX` 0xAE188AFB: floats at +4, +8, two bytes, floats at +14, +18, the pause range at +22 / +26,
/// the count at +30, then its descriptors).
#[derive(Clone, Debug)]
pub struct AmbienceSet {
    pub id: u32,
    pub name: String,
    /// (event, SoundBao resource) per pad.
    pub pads: Vec<(u32, u32)>,
    /// Per random FX: the pause range (its last two floats, seconds; PORT hypothesis: the wind gusts 20–120, a
    /// quarter's calls 1–30) and its sounds.
    pub fx: Vec<((f32, f32), Vec<(u32, u32)>)>,
}

pub fn ambience_set(r: &crate::assets::forge::Resource) -> AmbienceSet {
    let b = &r.payload;
    let descriptors = super::decode::markers(b, "SoundDescriptor");
    let fx_at = super::decode::markers(b, "SoundAmbienceRandomFX");
    let pair = |q: usize| Some((word(b, q + 4).ok()?, word(b, q + 8).ok()?)).filter(|(e, _)| e >> 28 == 1);
    let first_fx = fx_at.first().copied().unwrap_or(b.len());
    let pads = descriptors.iter().filter(|&&q| q < first_fx).filter_map(|&q| pair(q)).collect();
    let fx = fx_at.iter().enumerate().map(|(k, &p)| {
        let end = fx_at.get(k + 1).copied().unwrap_or(b.len());
        let f = |o: usize| word(b, p + o).map(f32::from_bits).unwrap_or(0.0);
        // +4, +8 floats, two bytes, two floats, then the pause range at +22 / +26 and the count at +30
        let (lo, hi) = (f(22), f(26));
        let range = if lo.is_finite() && hi.is_finite() && hi >= lo && hi > 0.0 { (lo.max(0.5), hi) } else { (5.0, 30.0) };
        (range, descriptors.iter().filter(|&&q| q > p && q < end).filter_map(|&q| pair(q)).collect())
    }).collect();
    AmbienceSet { id: r.id, name: r.name.clone(), pads, fx }
}

/// The sets of a cell file.
pub fn cell_ambience(resources: &HashMap<u32, Arc<crate::assets::forge::Resource>>) -> Vec<AmbienceSet> {
    let class = crc32("SoundAmbienceSet");
    let mut v: Vec<AmbienceSet> = resources.values().filter(|r| r.class_hash == class).map(|r| ambience_set(r)).collect();
    v.sort_by_key(|s| s.id);
    v
}

/// Pads fade in and out over this time when the sets under the player change (PORT).
const PAD_FADE: f32 = 2.0;

struct PlayingSet {
    set: AmbienceSet,
    /// 0..1, toward 1 while the set is wanted.
    level: f32,
    wanted: bool,
    /// Per pad: its voices' entities and gains.
    pads: Vec<(Entity, f32)>,
    /// Per random FX: the time to its next sound.
    timers: Vec<f32>,
    started: bool,
}

/// The ambience beds: the sets of the loaded cells and the ones playing.
#[derive(Default)]
struct Beds {
    by_cell: HashMap<usize, Vec<AmbienceSet>>,
    playing: Vec<PlayingSet>,
}

fn update_beds(beds: &mut Beds, amb_bank: &mut CityAmbience, archives: &super::archive::Archives, grid: &super::grid::Grid, player: Vec3,
    dt: f32, enabled: bool, picker: &mut crate::assets::dare::Picker, commands: &mut Commands, clips: &mut Assets<crate::sound::PcmClip>,
    sinks: &mut Query<&mut AudioSink>) {
    use bevy::audio::AudioSinkPlayback;
    // the sets of the loaded cells under the player (PORT: the game picks them by its sound regions, not traced)
    let at = Vec2::new(player.x, -player.z);
    // (the finest cell under the player that stores sets: the coarse cells also carry the countryside's)
    let under = |c: usize| {
        let cell = &grid.cells[c];
        let (lo, hi) = grid.cell_rect(cell);
        cell.level != super::grid::DATABLOCK_LEVEL && at.x >= lo.x && at.y >= lo.y && at.x < hi.x && at.y < hi.y
    };
    let finest = beds.by_cell.keys().copied().filter(|&c| under(c)).map(|c| grid.cells[c].level).min();
    let mut wanted: Vec<&AmbienceSet> = Vec::new();
    for (&c, sets) in &beds.by_cell {
        if !under(c) || Some(grid.cells[c].level) != finest { continue; }
        for s in sets {
            // the crowd murmur ("walla") comes in _big / _small / _nothing sets the game picks by the crowd around the
            // player (PORT: no NPCs in the port, so the _nothing set)
            let n = s.name.to_ascii_lowercase();
            if n.contains("walla") && (n.contains("_big") || n.contains("_small")) { continue; }
            if !wanted.iter().any(|w| w.id == s.id) { wanted.push(s); }
        }
    }
    for p in &mut beds.playing { p.wanted = enabled && wanted.iter().any(|w| w.id == p.set.id); }
    if enabled {
        for w in wanted {
            if beds.playing.iter().any(|p| p.set.id == w.id) { continue; }
            beds.playing.push(PlayingSet { set: w.clone(), level: 0.0, wanted: true, pads: Vec::new(), timers: Vec::new(), started: false });
        }
    }
    for p in &mut beds.playing {
        let step = (if p.wanted { dt } else { -dt }) / PAD_FADE;
        p.level = (p.level + step).clamp(0.0, 1.0);
        if !p.started {
            p.started = true;
            for &(event, res) in &p.set.pads {
                amb_bank.load(archives, res);
                for v in amb_bank.bank.resolve(event, &|_| None, picker) {
                    let Some(h) = amb_bank.clip(&v, clips) else { continue };
                    let gain = bevy::audio::Volume::Decibels(v.gain_db).to_linear();
                    let e = commands.spawn((crate::map_menu::MapEntity, AudioPlayer::<crate::sound::PcmClip>(h),
                        PlaybackSettings { mode: bevy::audio::PlaybackMode::Loop, volume: bevy::audio::Volume::Linear(0.0), speed: v.speed, ..default() })).id();
                    p.pads.push((e, gain));
                }
            }
            for (_, sounds) in &p.set.fx { for &(_, res) in sounds { amb_bank.load(archives, res); } }
            p.timers = p.set.fx.iter().map(|((lo, hi), _)| lo + (hi - lo) * picker.next01()).collect();
        }
        for &(e, gain) in &p.pads {
            if let Ok(mut s) = sinks.get_mut(e) { s.set_volume(bevy::audio::Volume::Linear(gain * p.level)); }
        }
        if !p.wanted { continue; }
        for (k, ((lo, hi), sounds)) in p.set.fx.iter().enumerate() {
            p.timers[k] -= dt;
            if p.timers[k] > 0.0 || sounds.is_empty() { continue; }
            p.timers[k] = lo + (hi - lo) * picker.next01();
            let (event, _) = sounds[((picker.next01() * sounds.len() as f32) as usize).min(sounds.len() - 1)];
            for v in amb_bank.bank.resolve(event, &|_| None, picker) {
                let Some(h) = amb_bank.clip(&v, clips) else { continue };
                let gain = bevy::audio::Volume::Decibels(v.gain_db).to_linear() * p.level;
                commands.spawn((crate::map_menu::MapEntity, AudioPlayer::<crate::sound::PcmClip>(h),
                    PlaybackSettings { mode: bevy::audio::PlaybackMode::Despawn, volume: bevy::audio::Volume::Linear(gain), speed: v.speed, ..default() }));
            }
        }
    }
    beds.playing.retain(|p| {
        let keep = p.wanted || p.level > 0.0;
        if !keep { for &(e, _) in &p.pads { commands.entity(e).try_despawn(); } }
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Bureau-street lantern light as stored (`OmniLight` 0x2A15AA10, Damascus, 101 bytes).
    #[test]
    fn omni_light_layout() {
        let hex = "10aa152ad68047340000000000000000ed6797a7e1e0603fdcdb5b3fc4c3433f0000803f000000010000000000000000803f18aa152ab6968faa\n                   00000000000000803f0000803fcdcccc3dcdcccc3d0000803f0001cdcc4c3e010000000000000000001041";
        let hex: String = hex.split_whitespace().collect();
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        assert_eq!(b.len(), 101);
        let (color, flags, intensity, near, far) = omni_light(&b).unwrap();
        assert!((color - Vec3::new(0.8784, 0.8588, 0.7647)).length() < 1e-3, "{color}");
        assert_eq!(flags, [0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!((intensity, near, far), (1.0, 0.0, 9.0));
    }
}
