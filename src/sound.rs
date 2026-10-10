//! The player's sounds: the anim event keys (AudioEvents, and ContactEvents through the ContactTable) start DARE
//! events from the install (`assets::dare`), decoded to PCM and played with Bevy audio.

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::audio::{AddAudioSource, ChannelCount, Decodable, PlaybackSettings, Sample, SampleRate, Source, Volume};
use bevy::prelude::*;

use crate::anim::AnimPlayer;
use crate::assets::ac_anim::EventKind;
use crate::assets::dare::{decode_ima, load_sound_bank, Picker, SoundBank, Voice};
use crate::assets::game_dir;

/// Decoded PCM, interleaved.
#[derive(Asset, TypePath, Clone)]
pub struct PcmClip {
    pub channels: u16,
    pub rate: u32,
    pub data: Arc<[i16]>,
}

pub struct PcmDecoder {
    clip: PcmClip,
    pos: usize,
}

impl Iterator for PcmDecoder {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let s = *self.clip.data.get(self.pos)?;
        self.pos += 1;
        Some(s as Sample / 32768.0)
    }
}

impl Source for PcmDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.clip.data.len() - self.pos)
    }
    fn channels(&self) -> ChannelCount {
        NonZero::new(self.clip.channels.max(1)).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        NonZero::new(self.clip.rate.max(1)).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        let frames = self.clip.data.len() / self.clip.channels.max(1) as usize;
        Some(Duration::from_secs_f64(frames as f64 / self.clip.rate.max(1) as f64))
    }
}

impl Decodable for PcmClip {
    type Decoder = PcmDecoder;
    fn decoder(&self) -> PcmDecoder {
        PcmDecoder { clip: self.clone(), pos: 0 }
    }
}

/// The bank (loaded on a thread at startup), the random state and the decoded resources.
#[derive(Resource)]
pub struct Sounds {
    bank: Option<Arc<SoundBank>>,
    loading: Option<Arc<Mutex<Option<Result<SoundBank, String>>>>>,
    picker: Picker,
    clips: HashMap<u32, Option<Handle<PcmClip>>>,
    pub enabled: bool,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds { bank: None, loading: None, picker: Picker::new(0x2008_1113), clips: HashMap::new(), enabled: true }
    }
}

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<PcmClip>()
            .init_resource::<Sounds>()
            .add_systems(Startup, start_loading)
            .add_systems(Update, play_anim_sounds.after(crate::anim::apply_clip));
    }
}

fn start_loading(mut s: ResMut<Sounds>) {
    if std::env::var_os("AC_NO_MODEL").is_some() || std::env::var_os("AC_NO_SOUND").is_some() {
        return;
    }
    let slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    std::thread::spawn(move || {
        let r = load_sound_bank(&game_dir());
        *out.lock().unwrap() = Some(r);
    });
    s.loading = Some(slot);
}

/// The switch values the port knows.
/// PORT: none yet. Switch 0 is the surface material (ContactTable_AlTair groups the contact sounds by the same 0..22
/// material ids the footstep events switch on); the others (1, 3, 4, 5, 10..16) are not identified. The game reads
/// them from the sound object type's callback (`dword_1A16DD8 + 188 * type + 16`), registered at runtime.
/// LIVE: read that table in the running game to find the callback and the values it returns.
fn switch_value(_var: u32) -> Option<u32> {
    None
}

fn play_anim_sounds(
    mut commands: Commands,
    mut s: ResMut<Sounds>,
    mut clips: ResMut<Assets<PcmClip>>,
    players: Query<&AnimPlayer>,
) {
    if let Some(slot) = s.loading.clone() {
        let done = slot.lock().unwrap().take();
        match done {
            Some(Ok(bank)) => {
                info!("sounds: {} SoundBaos from your install", bank.len());
                s.bank = Some(Arc::new(bank));
                s.loading = None;
            }
            Some(Err(e)) => {
                warn!("sounds not loaded: {e}");
                s.loading = None;
            }
            None => {}
        }
    }
    let Some(bank) = s.bank.clone() else { return };
    if !s.enabled {
        return;
    }
    for p in &players {
        for e in &p.events {
            let event = match e.kind {
                EventKind::Audio { sound } => bank.event_for_resource(sound),
                EventKind::Contact { ty, .. } => bank.contact.get(ty as usize).copied().flatten(),
                EventKind::Other(_) => None,
            };
            let Some(event) = event else { continue };
            let voices = bank.resolve(event, &switch_value, &mut s.picker);
            debug!("sound: {} t={:.3} event {event:08x} -> {} voice(s)", e.clip, e.time, voices.len());
            for v in voices {
                let Some(handle) = clip_for(&mut s, &bank, &mut clips, &v) else { continue };
                commands.spawn((
                    AudioPlayer::<PcmClip>(handle),
                    PlaybackSettings::DESPAWN.with_volume(Volume::Decibels(v.gain_db)).with_speed(v.speed),
                ));
            }
        }
    }
}

/// The decoded resource (decoded once; `None` when it is not IMA-ADPCM).
fn clip_for(s: &mut Sounds, bank: &SoundBank, clips: &mut Assets<PcmClip>, v: &Voice) -> Option<Handle<PcmClip>> {
    s.clips
        .entry(v.res)
        .or_insert_with(|| {
            let pcm = bank.object(v.res).and_then(|o| decode_ima(o, v.channels, v.frames));
            if pcm.is_none() {
                // PORT: the Ogg Vorbis / PCM resources are not decoded (the player's events use IMA-ADPCM only)
                debug!("sound resource {:08x}: not IMA-ADPCM", v.res);
            }
            pcm.map(|d| clips.add(PcmClip { channels: v.channels, rate: v.rate, data: d.into() }))
        })
        .clone()
}
