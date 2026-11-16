//! Fire-and-forget sound effects with up to four overlapping voices each. Port of
//! `src/game/sound.lua`: playback failures are silent, and a file that fails to load is logged
//! once and never retried.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sdl3::AudioSubsystem;
use sdl3::audio::{AudioSpec, AudioSpecWAV, AudioStreamOwner};

/// Voices per effect.
const VOICES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundEffect {
    Draw,
    PlayCard,
    Shuffle,
    Tap,
    Untap,
    Error,
}

impl SoundEffect {
    pub fn file_name(self) -> &'static str {
        match self {
            SoundEffect::Draw => "draw.wav",
            SoundEffect::PlayCard => "playcard.wav",
            SoundEffect::Shuffle => "shuffle.wav",
            SoundEffect::Tap => "tap.wav",
            SoundEffect::Untap => "untap.wav",
            SoundEffect::Error => "error.wav",
        }
    }
}

/// One effect's decoded samples and its voices, used round-robin.
struct VoicePool {
    samples: Vec<u8>,
    voices: Vec<AudioStreamOwner>,
    next: usize,
}

pub struct SoundPlayer {
    audio: Option<AudioSubsystem>,
    audio_folder: PathBuf,
    /// `None` once an effect failed to load.
    pools: HashMap<SoundEffect, Option<VoicePool>>,
}

impl SoundPlayer {
    /// `audio` is `None` when the audio subsystem isn't available (sounds are then skipped).
    pub fn new(audio: Option<AudioSubsystem>, audio_folder: &Path) -> SoundPlayer {
        SoundPlayer { audio, audio_folder: audio_folder.to_path_buf(), pools: HashMap::new() }
    }

    /// Plays an effect. Returns false when it couldn't be played.
    pub fn play(&mut self, effect: SoundEffect) -> bool {
        let Some(audio) = &self.audio else { return false };
        if !self.pools.contains_key(&effect) {
            let path = self.audio_folder.join(effect.file_name());
            let pool = load_pool(audio, &path);
            if let Err(error) = &pool {
                eprintln!("sound: couldn't load {effect:?} ({}): {error}", path.display());
            }
            self.pools.insert(effect, pool.ok());
        }
        let Some(Some(pool)) = self.pools.get_mut(&effect) else { return false };
        let voice = &pool.voices[pool.next];
        pool.next = (pool.next + 1) % pool.voices.len();
        voice.clear().is_ok() && voice.put_data(&pool.samples).is_ok() && voice.resume().is_ok()
    }
}

fn load_pool(audio: &AudioSubsystem, path: &Path) -> Result<VoicePool, String> {
    let wav = AudioSpecWAV::load_wav(path).map_err(|error| error.to_string())?;
    let spec = AudioSpec::new(Some(wav.freq), Some(i32::from(wav.channels)), Some(wav.format));
    let voices = (0..VOICES)
        .map(|_| audio.default_playback_device().open_device_stream(Some(&spec)).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(VoicePool { samples: wav.buffer().to_vec(), voices, next: 0 })
}
