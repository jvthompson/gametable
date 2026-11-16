//! Lazy, threaded image cache for card art, backs, logos and avatars. Port of
//! `src/data/image_cache.lua` and its worker.
//!
//! ```text
//! cache.get(path)       // the texture, or None while loading / failed
//! cache.status(path)    // Ready | Loading | Error
//! cache.update(gpu)     // once per frame: uploads finished decodes, frees, evicts
//! ```
//!
//! Decoding runs on worker threads; uploads happen on the main thread in `update`. Images are
//! downscaled so their longer side is at most `max_dimension`, uploaded with mipmaps, and the
//! least-recently-used ones are released when the estimated video memory passes `budget_bytes`.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::draw_list::{TextureId, TextureInfo};
use super::gpu_renderer::GpuRenderer;
use super::image_data::RgbaImage;

/// Where an image is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStatus {
    Loading,
    Ready,
    Error,
}

#[derive(Debug, Clone)]
enum EntryState {
    Loading,
    Ready(TextureInfo),
    Error(String),
}

#[derive(Debug, Clone)]
struct Entry {
    state: EntryState,
    /// Estimated video memory (pixels × 4 plus the mip chain).
    bytes: u64,
    last_used: u64,
}

enum WorkerRequest {
    Decode { key: String, path: String, max_dimension: u32 },
    Quit,
}

type WorkerResult = (String, Result<RgbaImage, String>);

struct Workers {
    requests: Sender<WorkerRequest>,
    results: Receiver<WorkerResult>,
    threads: Vec<JoinHandle<()>>,
}

impl Workers {
    fn start() -> Workers {
        let (request_sender, request_receiver) = mpsc::channel::<WorkerRequest>();
        let (result_sender, results) = mpsc::channel::<WorkerResult>();
        let shared_requests = Arc::new(Mutex::new(request_receiver));
        let processors = std::thread::available_parallelism().map_or(2, |count| count.get());
        let count = (processors.saturating_sub(1)).clamp(1, 4);
        let threads = (0..count)
            .map(|index| {
                let requests = shared_requests.clone();
                let results = result_sender.clone();
                std::thread::Builder::new()
                    .name(format!("image-decode-{index}"))
                    .spawn(move || run_worker(&requests, &results))
                    .expect("spawn image decoder")
            })
            .collect();
        Workers { requests: request_sender, results, threads }
    }
}

/// Decodes and downscales until told to quit.
fn run_worker(requests: &Mutex<Receiver<WorkerRequest>>, results: &Sender<WorkerResult>) {
    loop {
        let request = match requests.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => return,
        };
        match request {
            Ok(WorkerRequest::Decode { key, path, max_dimension }) => {
                let decoded =
                    RgbaImage::load(std::path::Path::new(&path)).map(|image| image.downscaled_to_fit(max_dimension));
                if results.send((key, decoded)).is_err() {
                    return;
                }
            }
            Ok(WorkerRequest::Quit) | Err(_) => return,
        }
    }
}

/// `w × h × 4 × 1.34`: RGBA8 plus the mip chain.
fn estimated_bytes(texture: &TextureInfo) -> u64 {
    (texture.width as f64 * texture.height as f64 * 4.0 * 1.34).floor() as u64
}

pub struct ImageCache {
    pub max_dimension: u32,
    pub budget_bytes: u64,
    pub max_uploads_per_frame: usize,
    entries: HashMap<String, Entry>,
    frame: u64,
    used_bytes: u64,
    workers: Option<Workers>,
    /// Released textures, freed on the GPU at the next `update`.
    textures_to_free: Vec<TextureId>,
    /// Decoded images waiting for upload (from `put_bytes`).
    ready_to_upload: Vec<(String, RgbaImage)>,
}

impl Default for ImageCache {
    fn default() -> Self {
        ImageCache::new()
    }
}

impl ImageCache {
    pub fn new() -> ImageCache {
        ImageCache {
            max_dimension: 1024,
            budget_bytes: 768 * 1024 * 1024,
            max_uploads_per_frame: 8,
            entries: HashMap::new(),
            frame: 0,
            used_bytes: 0,
            workers: None,
            textures_to_free: Vec::new(),
            ready_to_upload: Vec::new(),
        }
    }

    /// The image's texture, or `None` while it loads (the first call starts loading) or when it
    /// failed.
    pub fn get(&mut self, path: &str) -> Option<TextureInfo> {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.last_used = self.frame;
            return match &entry.state {
                EntryState::Ready(texture) => Some(*texture),
                _ => None,
            };
        }
        let workers = self.workers.get_or_insert_with(Workers::start);
        self.entries.insert(path.to_string(), Entry { state: EntryState::Loading, bytes: 0, last_used: self.frame });
        let request =
            WorkerRequest::Decode { key: path.to_string(), path: path.to_string(), max_dimension: self.max_dimension };
        let _ = workers.requests.send(request);
        None
    }

    pub fn status(&self, path: &str) -> Option<ImageStatus> {
        self.entries.get(path).map(|entry| match entry.state {
            EntryState::Loading => ImageStatus::Loading,
            EntryState::Ready(_) => ImageStatus::Ready,
            EntryState::Error(_) => ImageStatus::Error,
        })
    }

    pub fn error_for(&self, path: &str) -> Option<&str> {
        match &self.entries.get(path)?.state {
            EntryState::Error(message) => Some(message),
            _ => None,
        }
    }

    /// Registers an image decoded from in-memory bytes (a network avatar, say). Decodes now and
    /// uploads at the next `update`; the key should be unique per content (`avatar:<id>`).
    /// Returns the decode error, if any.
    pub fn put_bytes(&mut self, key: &str, bytes: &[u8]) -> Result<(), String> {
        self.invalidate(key);
        match RgbaImage::decode(bytes) {
            Ok(image) => {
                self.entries
                    .insert(key.to_string(), Entry { state: EntryState::Loading, bytes: 0, last_used: self.frame });
                self.ready_to_upload.push((key.to_string(), image.downscaled_to_fit(self.max_dimension)));
                Ok(())
            }
            Err(message) => {
                self.entries.insert(
                    key.to_string(),
                    Entry { state: EntryState::Error(message.clone()), bytes: 0, last_used: self.frame },
                );
                Err(message)
            }
        }
    }

    /// Drops an entry so the next `get` reloads it.
    pub fn invalidate(&mut self, key: &str) {
        let Some(entry) = self.entries.remove(key) else { return };
        if let EntryState::Ready(texture) = entry.state {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            self.textures_to_free.push(texture.id);
        }
        self.ready_to_upload.retain(|(pending_key, _)| pending_key != key);
    }

    pub fn clear(&mut self) {
        let keys: Vec<String> = self.entries.keys().cloned().collect();
        for key in keys {
            self.invalidate(&key);
        }
    }

    /// Once per frame: uploads up to `max_uploads_per_frame` finished decodes, frees released
    /// textures and evicts least-recently-used images over budget.
    pub fn update(&mut self, renderer: &mut GpuRenderer) {
        self.frame += 1;
        for texture in self.textures_to_free.drain(..) {
            renderer.destroy_texture(texture);
        }
        for (key, image) in std::mem::take(&mut self.ready_to_upload) {
            self.finish_decode(renderer, key, Ok(image));
        }
        for _ in 0..self.max_uploads_per_frame {
            let Some((key, decoded)) = self.workers.as_ref().and_then(|workers| workers.results.try_recv().ok()) else {
                break;
            };
            self.finish_decode(renderer, key, decoded);
        }
        self.evict();
        for texture in self.textures_to_free.drain(..) {
            renderer.destroy_texture(texture);
        }
    }

    fn finish_decode(&mut self, renderer: &mut GpuRenderer, key: String, decoded: Result<RgbaImage, String>) {
        let Some(entry) = self.entries.get_mut(&key) else { return };
        if !matches!(entry.state, EntryState::Loading) {
            return;
        }
        match decoded
            .and_then(|image| renderer.create_texture_from_rgba(image.width, image.height, &image.pixels, true))
        {
            Ok(texture) => {
                entry.bytes = estimated_bytes(&texture);
                entry.state = EntryState::Ready(texture);
                self.used_bytes += entry.bytes;
            }
            Err(message) => entry.state = EntryState::Error(message),
        }
    }

    fn evict(&mut self) {
        if self.used_bytes <= self.budget_bytes {
            return;
        }
        let mut ready: Vec<(String, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, EntryState::Ready(_)) && entry.last_used < self.frame)
            .map(|(key, entry)| (key.clone(), entry.last_used))
            .collect();
        ready.sort_by_key(|(_, last_used)| *last_used);
        for (key, _) in ready {
            if self.used_bytes as f64 <= self.budget_bytes as f64 * 0.8 {
                break;
            }
            self.invalidate(&key);
        }
    }

    /// Stops the decoder threads.
    pub fn shutdown(&mut self) {
        let Some(workers) = self.workers.take() else { return };
        for _ in &workers.threads {
            let _ = workers.requests.send(WorkerRequest::Quit);
        }
        for thread in workers.threads {
            let _ = thread.join();
        }
    }

    /// `(entries, estimated bytes in use)`.
    pub fn stats(&self) -> (usize, u64) {
        (self.entries.len(), self.used_bytes)
    }
}

impl Drop for ImageCache {
    fn drop(&mut self) {
        self.shutdown();
    }
}
