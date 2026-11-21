//! An image file's pixel size, read from its header without decoding the pixels (a GameTable
//! addition, for the Card View preview). Knows PNG, JPEG, GIF, BMP and WebP, the formats card art
//! comes in.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// (width, height) in pixels, or `None` when the file can't be read or isn't a known format.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut start = [0u8; 30];
    let length = read_up_to(&mut reader, &mut start)?;
    let start = &start[..length];
    if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_dimensions(start)
    } else if start.starts_with(&[0xFF, 0xD8]) {
        reader.seek(SeekFrom::Start(2)).ok()?;
        jpeg_dimensions(&mut reader)
    } else if start.starts_with(b"GIF87a") || start.starts_with(b"GIF89a") {
        gif_dimensions(start)
    } else if start.starts_with(b"BM") {
        bmp_dimensions(start)
    } else if start.starts_with(b"RIFF") && start.get(8..12) == Some(b"WEBP") {
        webp_dimensions(start)
    } else {
        None
    }
}

/// Fills as much of `buffer` as the file has; returns how many bytes that was.
fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> Option<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]).ok()? {
            0 => break,
            count => filled += count,
        }
    }
    Some(filled)
}

fn big_endian_u16(bytes: &[u8], offset: usize) -> Option<u32> {
    let pair = bytes.get(offset..offset + 2)?;
    Some(u32::from(u16::from_be_bytes([pair[0], pair[1]])))
}

fn big_endian_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn little_endian_u16(bytes: &[u8], offset: usize) -> Option<u32> {
    let pair = bytes.get(offset..offset + 2)?;
    Some(u32::from(u16::from_le_bytes([pair[0], pair[1]])))
}

fn little_endian_u24(bytes: &[u8], offset: usize) -> Option<u32> {
    let triple = bytes.get(offset..offset + 3)?;
    Some(u32::from_le_bytes([triple[0], triple[1], triple[2], 0]))
}

fn little_endian_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

/// The IHDR chunk always comes first: width and height at bytes 16 and 20.
fn png_dimensions(start: &[u8]) -> Option<(u32, u32)> {
    (start.get(12..16) == Some(b"IHDR")).then_some(())?;
    Some((big_endian_u32(start, 16)?, big_endian_u32(start, 20)?))
}

/// Logical screen size at bytes 6 and 8.
fn gif_dimensions(start: &[u8]) -> Option<(u32, u32)> {
    Some((little_endian_u16(start, 6)?, little_endian_u16(start, 8)?))
}

/// BITMAPINFOHEADER size at bytes 18 and 22 (a negative height means top-down rows).
fn bmp_dimensions(start: &[u8]) -> Option<(u32, u32)> {
    let width = little_endian_i32(start, 18)?;
    let height = little_endian_i32(start, 22)?;
    Some((width.unsigned_abs(), height.unsigned_abs()))
}

/// The first chunk after the RIFF header says which WebP flavour it is.
fn webp_dimensions(start: &[u8]) -> Option<(u32, u32)> {
    match start.get(12..16)? {
        // Lossy: the key frame header holds 14-bit sizes after the start code.
        b"VP8 " => Some((little_endian_u16(start, 26)? & 0x3FFF, little_endian_u16(start, 28)? & 0x3FFF)),
        // Lossless: 14-bit width - 1 and height - 1 packed after the signature byte.
        b"VP8L" => {
            let bits = u32::from_le_bytes(start.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        // Extended: 24-bit canvas width - 1 and height - 1.
        b"VP8X" => Some((little_endian_u24(start, 24)? + 1, little_endian_u24(start, 27)? + 1)),
        _ => None,
    }
}

/// Walks the marker segments (after SOI) to the first start-of-frame, skipping metadata such as
/// EXIF thumbnails, however large.
fn jpeg_dimensions(reader: &mut (impl Read + Seek)) -> Option<(u32, u32)> {
    let mut byte = [0u8; 1];
    loop {
        // Find the next marker: 0xFF (possibly repeated as padding) then a marker code.
        reader.read_exact(&mut byte).ok()?;
        if byte[0] != 0xFF {
            return None;
        }
        let mut marker = 0xFF;
        while marker == 0xFF {
            reader.read_exact(&mut byte).ok()?;
            marker = byte[0];
        }
        // Markers without a length: TEM and the restart markers.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let mut length_bytes = [0u8; 2];
        reader.read_exact(&mut length_bytes).ok()?;
        let segment_length = u16::from_be_bytes(length_bytes);
        if segment_length < 2 {
            return None;
        }
        // SOF0-SOF15, except DHT (C4), JPG (C8) and DAC (CC), which share the range.
        let is_start_of_frame = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_start_of_frame {
            // Precision (1 byte), then height and width.
            let mut frame_header = [0u8; 5];
            reader.read_exact(&mut frame_header).ok()?;
            return Some((big_endian_u16(&frame_header, 3)?, big_endian_u16(&frame_header, 1)?));
        }
        if marker == 0xDA || marker == 0xD9 {
            // Start of scan or end of image before any frame header.
            return None;
        }
        reader.seek(SeekFrom::Current(i64::from(segment_length) - 2)).ok()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::image_data::RgbaImage;

    fn image(width: u32, height: u32) -> RgbaImage {
        RgbaImage { width, height, pixels: vec![128u8; (width * height * 4) as usize] }
    }

    #[test]
    fn png_and_jpeg_sizes_come_from_their_headers() {
        let folder = std::env::temp_dir().join(format!("gametable_dimensions_{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let png = folder.join("card.png");
        image(37, 52).save_png(&png).unwrap();
        let jpeg = folder.join("card.jpg");
        image(61, 23).save_jpeg(&jpeg, 90).unwrap();
        let text = folder.join("notes.png");
        std::fs::write(&text, b"not an image at all").unwrap();
        assert_eq!(image_dimensions(&png), Some((37, 52)));
        assert_eq!(image_dimensions(&jpeg), Some((61, 23)));
        assert_eq!(image_dimensions(&text), None);
        assert_eq!(image_dimensions(&folder.join("missing.png")), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn jpeg_frame_header_is_found_past_large_metadata() {
        let mut bytes = vec![0xFF, 0xD8];
        // An APP1 segment bigger than any fixed-size header read.
        bytes.extend_from_slice(&[0xFF, 0xE1, 0xFF, 0xFF]);
        bytes.extend(std::iter::repeat_n(0u8, 0xFFFF - 2));
        // Padding 0xFF bytes, then SOF2 (progressive): precision 8, height 1050, width 750.
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xC2, 0x00, 0x11, 0x08, 0x04, 0x1A, 0x02, 0xEE]);
        let mut cursor = std::io::Cursor::new(bytes);
        cursor.seek(SeekFrom::Start(2)).unwrap();
        assert_eq!(jpeg_dimensions(&mut cursor), Some((750, 1050)));
    }

    #[test]
    fn gif_bmp_and_webp_headers() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x2C, 0x01, 0xC8, 0x00]);
        assert_eq!(gif_dimensions(&gif), Some((300, 200)));

        let mut bmp = vec![0u8; 26];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[18..22].copy_from_slice(&640i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&(-480i32).to_le_bytes());
        assert_eq!(bmp_dimensions(&bmp), Some((640, 480)));

        let mut extended = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        extended.extend_from_slice(&[0u8; 8]);
        extended.extend_from_slice(&[0xED, 0x02, 0x00, 0x19, 0x04, 0x00]); // 749 + 1, 1049 + 1
        assert_eq!(webp_dimensions(&extended), Some((750, 1050)));

        let mut lossless = b"RIFF\0\0\0\0WEBPVP8L".to_vec();
        lossless.extend_from_slice(&[0u8; 4]);
        lossless.push(0x2F);
        let bits: u32 = (99) | (199 << 14);
        lossless.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(webp_dimensions(&lossless), Some((100, 200)));

        let mut lossy = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        lossy.extend_from_slice(&[0u8; 4]);
        lossy.extend_from_slice(&[0u8, 0, 0, 0x9D, 0x01, 0x2A]);
        lossy.extend_from_slice(&320u16.to_le_bytes());
        lossy.extend_from_slice(&240u16.to_le_bytes());
        assert_eq!(webp_dimensions(&lossy), Some((320, 240)));
    }
}
