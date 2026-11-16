//! PDFium bindings for rendering a PDF document page to RGBA pixels. Port of
//! `src/data/pdf_render.lua`: `pdfium.dll` (the prebuilt "pdfium-win-x64" release from
//! bblanchon/pdfium-binaries, the same file the original ships) is loaded at runtime, and its
//! absence is a soft failure with the original's message.
//!
//! PDFium isn't safe to call from several threads at once, so every call goes through one lock
//! (the original kept each document on its own thread and never had two open at once in
//! practice).

use std::ffi::{c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::graphics::image_data::RgbaImage;

type Document = *mut c_void;
type Page = *mut c_void;
type Bitmap = *mut c_void;

/// `FPDFBitmap_BGRA`.
const BITMAP_FORMAT_BGRA: c_int = 4;

struct PdfiumLibrary {
    load_memory_document: unsafe extern "C" fn(*const c_void, c_int, *const i8) -> Document,
    close_document: unsafe extern "C" fn(Document),
    page_count: unsafe extern "C" fn(Document) -> c_int,
    load_page: unsafe extern "C" fn(Document, c_int) -> Page,
    close_page: unsafe extern "C" fn(Page),
    page_width: unsafe extern "C" fn(Page) -> f64,
    page_height: unsafe extern "C" fn(Page) -> f64,
    create_bitmap: unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> Bitmap,
    fill_rect: unsafe extern "C" fn(Bitmap, c_int, c_int, c_int, c_int, c_ulong),
    render_page_bitmap: unsafe extern "C" fn(Bitmap, Page, c_int, c_int, c_int, c_int, c_int, c_int),
    bitmap_buffer: unsafe extern "C" fn(Bitmap) -> *mut u8,
    bitmap_stride: unsafe extern "C" fn(Bitmap) -> c_int,
    destroy_bitmap: unsafe extern "C" fn(Bitmap),
    _library: libloading::Library,
}

// The function pointers are plain C entry points; every call is made under `PDFIUM_LOCK`.
unsafe impl Send for PdfiumLibrary {}
unsafe impl Sync for PdfiumLibrary {}

static PDFIUM_LOCK: Mutex<()> = Mutex::new(());

/// Folders searched for `pdfium.dll`: the executable's folder, and in development the
/// workspace's `runtime/windows` (as for libwebp).
fn search_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        folders.push(folder);
    }
    if cfg!(debug_assertions) {
        folders.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/windows"));
    }
    folders
}

fn bind(library: libloading::Library) -> Option<PdfiumLibrary> {
    unsafe {
        let pdfium = PdfiumLibrary {
            load_memory_document: *library.get(b"FPDF_LoadMemDocument\0").ok()?,
            close_document: *library.get(b"FPDF_CloseDocument\0").ok()?,
            page_count: *library.get(b"FPDF_GetPageCount\0").ok()?,
            load_page: *library.get(b"FPDF_LoadPage\0").ok()?,
            close_page: *library.get(b"FPDF_ClosePage\0").ok()?,
            page_width: *library.get(b"FPDF_GetPageWidth\0").ok()?,
            page_height: *library.get(b"FPDF_GetPageHeight\0").ok()?,
            create_bitmap: *library.get(b"FPDFBitmap_CreateEx\0").ok()?,
            fill_rect: *library.get(b"FPDFBitmap_FillRect\0").ok()?,
            render_page_bitmap: *library.get(b"FPDF_RenderPageBitmap\0").ok()?,
            bitmap_buffer: *library.get(b"FPDFBitmap_GetBuffer\0").ok()?,
            bitmap_stride: *library.get(b"FPDFBitmap_GetStride\0").ok()?,
            destroy_bitmap: *library.get(b"FPDFBitmap_Destroy\0").ok()?,
            _library: library,
        };
        let initialize: libloading::Symbol<unsafe extern "C" fn()> = pdfium._library.get(b"FPDF_InitLibrary\0").ok()?;
        initialize();
        Some(pdfium)
    }
}

fn load_library() -> Option<PdfiumLibrary> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for folder in search_folders() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.filter_map(Result::ok) {
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case("pdfium.dll") {
                candidates.push(entry.path());
            }
        }
    }
    candidates.push(PathBuf::from(libloading::library_filename("pdfium")));
    candidates.into_iter().find_map(|candidate| unsafe { libloading::Library::new(&candidate) }.ok().and_then(bind))
}

fn library() -> Option<&'static PdfiumLibrary> {
    static LIBRARY: OnceLock<Option<PdfiumLibrary>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            load_library()
        })
        .as_ref()
}

/// `P.available()`.
pub fn is_available() -> bool {
    library().is_some()
}

/// An open document. Keeps the file bytes alive: PDFium reads from them while rendering.
pub struct PdfDocument {
    library: &'static PdfiumLibrary,
    document: Document,
    _bytes: Vec<u8>,
}

// The handle is only used through `PDFIUM_LOCK`.
unsafe impl Send for PdfDocument {}

impl PdfDocument {
    /// `P.openDocument(bytes)`.
    pub fn open(bytes: Vec<u8>) -> Result<PdfDocument, String> {
        let library = library().ok_or("PDF rulebooks need pdfium.dll next to the app")?;
        let length = c_int::try_from(bytes.len()).map_err(|_| "invalid or unsupported PDF".to_string())?;
        let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let document = unsafe { (library.load_memory_document)(bytes.as_ptr().cast(), length, std::ptr::null()) };
        if document.is_null() {
            return Err("invalid or unsupported PDF".to_string());
        }
        Ok(PdfDocument { library, document, _bytes: bytes })
    }

    /// `P.pageCount(handle)`.
    pub fn page_count(&self) -> usize {
        let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        usize::try_from(unsafe { (self.library.page_count)(self.document) }).unwrap_or(0)
    }

    /// `P.pageSize(handle, index)`: the page size in points (1/72 inch), without rendering.
    pub fn page_size(&self, index: usize) -> Option<(f64, f64)> {
        let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            let page = (self.library.load_page)(self.document, index as c_int);
            if page.is_null() {
                return None;
            }
            let size = ((self.library.page_width)(page), (self.library.page_height)(page));
            (self.library.close_page)(page);
            Some(size)
        }
    }

    /// `P.renderPage(handle, index, pixelWidth)`: the page `pixel_width` wide (the height follows
    /// its aspect ratio) on white, as RGBA.
    pub fn render_page(&self, index: usize, pixel_width: u32) -> Result<RgbaImage, String> {
        let library = self.library;
        let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            let page = (library.load_page)(self.document, index as c_int);
            if page.is_null() {
                return Err("could not load page".to_string());
            }
            let (page_width, page_height) = ((library.page_width)(page), (library.page_height)(page));
            if page_width <= 0.0 || page_height <= 0.0 {
                (library.close_page)(page);
                return Err("invalid page size".to_string());
            }
            let pixel_height = ((pixel_width as f64 * page_height / page_width + 0.5).floor() as i64).max(1) as u32;
            // PDFium owns the buffer and picks the stride.
            let bitmap = (library.create_bitmap)(
                pixel_width as c_int,
                pixel_height as c_int,
                BITMAP_FORMAT_BGRA,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() {
                (library.close_page)(page);
                return Err("could not create bitmap".to_string());
            }
            // Pages have no background of their own: fill white so text-only pages aren't clear.
            (library.fill_rect)(bitmap, 0, 0, pixel_width as c_int, pixel_height as c_int, 0xFFFF_FFFF);
            (library.render_page_bitmap)(bitmap, page, 0, 0, pixel_width as c_int, pixel_height as c_int, 0, 0);
            let buffer = (library.bitmap_buffer)(bitmap);
            let stride = (library.bitmap_stride)(bitmap) as usize;
            let result = if buffer.is_null() {
                Err("could not render page".to_string())
            } else {
                let (width, height) = (pixel_width as usize, pixel_height as usize);
                let source = std::slice::from_raw_parts(buffer, stride * height);
                let mut pixels = vec![0u8; width * height * 4];
                for (row, destination_row) in pixels.chunks_exact_mut(width * 4).enumerate() {
                    let source_row = &source[row * stride..row * stride + width * 4];
                    for (destination, bgra) in destination_row.chunks_exact_mut(4).zip(source_row.chunks_exact(4)) {
                        destination.copy_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
                    }
                }
                Ok(RgbaImage { width: pixel_width, height: pixel_height, pixels })
            };
            (library.destroy_bitmap)(bitmap);
            (library.close_page)(page);
            result
        }
    }
}

impl Drop for PdfDocument {
    /// `P.closeDocument(handle)`.
    fn drop(&mut self) {
        let _guard = PDFIUM_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { (self.library.close_document)(self.document) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page 200×100-point PDF with a black rectangle in the left half.
    pub(crate) fn tiny_pdf() -> Vec<u8> {
        let content = "0 0 0 rg 0 0 100 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref_offset = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    #[test]
    fn renders_a_page_with_the_bundled_library() {
        if !is_available() {
            eprintln!("pdfium not found; skipping");
            return;
        }
        assert!(PdfDocument::open(b"not a pdf".to_vec()).is_err());
        let document = PdfDocument::open(tiny_pdf()).unwrap();
        assert_eq!(document.page_count(), 1);
        assert_eq!(document.page_size(0), Some((200.0, 100.0)));
        assert!(document.page_size(1).is_none());
        let image = document.render_page(0, 40).unwrap();
        assert_eq!((image.width, image.height), (40, 20));
        let pixel = |x: usize, y: usize| &image.pixels[(y * 40 + x) * 4..(y * 40 + x) * 4 + 4];
        assert_eq!(pixel(5, 10), &[0, 0, 0, 255]);
        assert_eq!(pixel(35, 10), &[255, 255, 255, 255]);
        assert!(document.render_page(3, 40).is_err());
    }
}
