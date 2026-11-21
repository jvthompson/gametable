//! One open PDF and its dedicated worker thread. Port of `src/data/pdf_doc_cache.lua` and
//! `pdf_worker.lua`: the worker opens the document, reports its page sizes, and renders pages on
//! request; finished pages are uploaded to the image cache on the main thread.
//!
//! ```text
//! let mut cache = PdfDocCache::new(path);
//! cache.update(images);            // once per frame while in use
//! cache.point_sizes                // None until the document has opened
//! cache.open_error                 // set if it failed to open
//! cache.request_page(index, width) // 0-based; no-op once pending or ready
//! cache.page_texture(images, index)
//! ```

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::data::pdf_render::PdfDocument;
use crate::graphics::draw_list::TextureInfo;
use crate::graphics::image_cache::{ImageCache, ImageStatus};
use crate::graphics::image_data::RgbaImage;

enum WorkerRequest {
    Render { index: usize, pixel_width: u32 },
    Quit,
}

enum WorkerReply {
    Opened(Result<Vec<(f64, f64)>, String>),
    Page { index: usize, rendered: Result<RgbaImage, String> },
}

/// Where one page is.
#[derive(Debug, Clone, PartialEq)]
pub enum PageState {
    Pending,
    /// Uploaded (or uploading) under this image cache key.
    Ready(String),
    Error(String),
}

pub struct PdfDocCache {
    path: String,
    pub page_count: Option<usize>,
    /// Each page's size in points, once opened.
    pub point_sizes: Option<Vec<(f64, f64)>>,
    pub open_error: Option<String>,
    pub pages: HashMap<usize, PageState>,
    requests: Sender<WorkerRequest>,
    replies: Receiver<WorkerReply>,
    thread: Option<JoinHandle<()>>,
}

/// The worker: owns the document for its whole life.
fn run_worker(path: String, requests: Receiver<WorkerRequest>, replies: Sender<WorkerReply>) {
    let opened = crate::data::game_files::read(&path).map_err(|error| error.to_string()).and_then(PdfDocument::open);
    let document = match opened {
        Ok(document) => {
            let sizes =
                (0..document.page_count()).map(|index| document.page_size(index).unwrap_or((1.0, 1.0))).collect();
            if replies.send(WorkerReply::Opened(Ok(sizes))).is_err() {
                return;
            }
            Some(document)
        }
        Err(error) => {
            if replies.send(WorkerReply::Opened(Err(error))).is_err() {
                return;
            }
            None
        }
    };
    while let Ok(WorkerRequest::Render { index, pixel_width }) = requests.recv() {
        let rendered = match &document {
            Some(document) => document.render_page(index, pixel_width),
            None => Err("document not open".to_string()),
        };
        if replies.send(WorkerReply::Page { index, rendered }).is_err() {
            return;
        }
    }
}

impl PdfDocCache {
    /// `C.new(path)`: starts the worker, which opens the document straight away.
    pub fn new(path: &str) -> PdfDocCache {
        let (requests, worker_requests) = mpsc::channel();
        let (worker_replies, replies) = mpsc::channel();
        let worker_path = path.to_string();
        let thread = std::thread::Builder::new()
            .name("pdf-render".to_string())
            .spawn(move || run_worker(worker_path, worker_requests, worker_replies))
            .ok();
        PdfDocCache {
            path: path.to_string(),
            page_count: None,
            point_sizes: None,
            open_error: None,
            pages: HashMap::new(),
            requests,
            replies,
            thread,
        }
    }

    fn page_key(&self, index: usize) -> String {
        format!("pdf:{}:{index}", self.path)
    }

    /// `C:requestPage(index, pixelWidth)`: asks for a page unless it is pending or ready.
    pub fn request_page(&mut self, index: usize, pixel_width: u32) {
        if matches!(self.pages.get(&index), Some(PageState::Pending | PageState::Ready(_))) {
            return;
        }
        self.pages.insert(index, PageState::Pending);
        let _ = self.requests.send(WorkerRequest::Render { index, pixel_width });
    }

    /// `C:update()`: takes the worker's replies and hands finished pages to the image cache.
    pub fn update(&mut self, images: &mut ImageCache) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                WorkerReply::Opened(Ok(sizes)) => {
                    self.page_count = Some(sizes.len());
                    self.point_sizes = Some(sizes);
                }
                WorkerReply::Opened(Err(error)) => self.open_error = Some(error),
                WorkerReply::Page { index, rendered } => {
                    if !self.pages.contains_key(&index) {
                        continue;
                    }
                    let state = match rendered {
                        Ok(image) => {
                            let key = self.page_key(index);
                            images.put_rgba(&key, image);
                            PageState::Ready(key)
                        }
                        Err(error) => PageState::Error(error),
                    };
                    self.pages.insert(index, state);
                }
            }
        }
    }

    /// The page's texture once it is uploaded. A page the image cache has since evicted goes
    /// back to unrequested, so the next `request_page` renders it again.
    pub fn page_texture(&mut self, images: &mut ImageCache, index: usize) -> Option<TextureInfo> {
        let Some(PageState::Ready(key)) = self.pages.get(&index) else { return None };
        let key = key.clone();
        if let Some(texture) = images.uploaded(&key) {
            return Some(texture);
        }
        if images.status(&key) != Some(ImageStatus::Loading) {
            self.pages.remove(&index);
        }
        None
    }

    pub fn page_failed(&self, index: usize) -> bool {
        matches!(self.pages.get(&index), Some(PageState::Error(_)))
    }
}

impl Drop for PdfDocCache {
    /// `C:destroy()`: tells the worker to quit.
    fn drop(&mut self) {
        let _ = self.requests.send(WorkerRequest::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
