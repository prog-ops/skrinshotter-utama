use crate::settings::SaveFormat;
use chrono::{DateTime, Local};
use image::{codecs::jpeg::JpegEncoder, RgbaImage};
use std::fs;
use std::path::Path;

const SOFTWARE_NAME: &str = "Skrinshotter";

#[derive(Debug, Clone)]
pub struct ScreenshotMetadata {
    pub title: String,
    pub description: String,
    pub author: String,
    pub tags_raw: String,
    pub captured_at: DateTime<Local>,
}

impl ScreenshotMetadata {
    pub fn new(default_author: &str) -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            author: default_author.to_string(),
            tags_raw: String::new(),
            captured_at: Local::now(),
        }
    }

    /// Parses comma-separated tags, trimming whitespace and deduplicating case-insensitively
    /// while preserving original casing.
    pub fn parsed_tags(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for part in self.tags_raw.split(',') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !result
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
            {
                result.push(trimmed.to_string());
            }
        }
        result
    }

    pub fn suggested_filename(&self, format: SaveFormat) -> String {
        let ts = self.captured_at.format("%Y%m%d_%H%M%S");
        let ext = format.extension();
        let slug: String = self
            .title
            .trim()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else if c == ' ' || c == '-' || c == '_' {
                    '-'
                } else {
                    '\0'
                }
            })
            .filter(|&c| c != '\0')
            .collect();

        let clean_slug = slug
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");

        if clean_slug.is_empty() {
            format!("screenshot_{ts}.{ext}")
        } else {
            let truncated: String = clean_slug.chars().take(40).collect();
            format!("{truncated}_{ts}.{ext}")
        }
    }
}

/// Saves an RGBA image in either PNG or JPEG format with embedded metadata
/// (PNG `iTXt` + XMP + `eXIf`, or JPEG `APP1` EXIF + Windows XP UTF-16LE tags + XMP).
pub fn save_image_with_metadata(
    path: &Path,
    image: &RgbaImage,
    format: SaveFormat,
    jpeg_quality: u8,
    meta: &ScreenshotMetadata,
) -> Result<(), String> {
    let bytes = match format {
        SaveFormat::Png => encode_png_with_metadata(image, meta)?,
        SaveFormat::Jpeg => encode_jpeg_with_metadata(image, jpeg_quality, meta)?,
    };
    fs::write(path, bytes).map_err(|e| format!("Failed to write image file: {e}"))
}

pub fn encode_png_with_metadata(
    image: &RgbaImage,
    meta: &ScreenshotMetadata,
) -> Result<Vec<u8>, String> {
    let mut raw_png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut raw_png, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        let title = meta.title.trim();
        let description = meta.description.trim();
        let author = meta.author.trim();
        let tags = meta.parsed_tags();

        if !title.is_empty() {
            let _ = encoder.add_itxt_chunk("Title".to_string(), title.to_string());
        }
        if !description.is_empty() {
            let _ = encoder.add_itxt_chunk("Description".to_string(), description.to_string());
            let _ = encoder.add_itxt_chunk("Comment".to_string(), description.to_string());
        }
        if !author.is_empty() {
            let _ = encoder.add_itxt_chunk("Author".to_string(), author.to_string());
        }
        if !tags.is_empty() {
            let _ = encoder.add_itxt_chunk("Keywords".to_string(), tags.join(", "));
        }
        let _ = encoder.add_itxt_chunk("Software".to_string(), SOFTWARE_NAME.to_string());
        let _ = encoder.add_itxt_chunk("Creation Time".to_string(), meta.captured_at.to_rfc2822());

        let xmp = build_xmp_packet(meta);
        let _ = encoder.add_itxt_chunk("XML:com.adobe.xmp".to_string(), xmp);

        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("Failed to write PNG header: {e}"))?;
        writer
            .write_image_data(image.as_raw())
            .map_err(|e| format!("Failed to encode PNG pixels: {e}"))?;
    }

    // Inject PNG `eXIf` chunk right before `IEND` so EXIF-centric readers also see full metadata.
    let exif_tiff = build_tiff_exif_payload(meta);
    inject_png_chunk_before_iend(&mut raw_png, *b"eXIf", &exif_tiff);

    Ok(raw_png)
}

pub fn encode_jpeg_with_metadata(
    image: &RgbaImage,
    jpeg_quality: u8,
    meta: &ScreenshotMetadata,
) -> Result<Vec<u8>, String> {
    // Composite transparent regions (e.g. outside a Freeform selection) smoothly over white
    // because JPEG does not have an alpha channel.
    let mut rgb = image::RgbImage::new(image.width(), image.height());
    for (x, y, px) in image.enumerate_pixels() {
        let a = u16::from(px.0[3]);
        let blend_white = |c: u8| -> u8 {
            ((u16::from(c) * a + 255 * (255 - a) + 127) / 255) as u8
        };
        rgb.put_pixel(
            x,
            y,
            image::Rgb([
                blend_white(px.0[0]),
                blend_white(px.0[1]),
                blend_white(px.0[2]),
            ]),
        );
    }

    let mut raw_jpeg = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut raw_jpeg, jpeg_quality.clamp(10, 100));
        encoder
            .encode_image(&rgb)
            .map_err(|e| format!("Failed to encode JPEG: {e}"))?;
    }

    if raw_jpeg.len() < 4 || raw_jpeg[0] != 0xFF || raw_jpeg[1] != 0xD8 {
        return Err("Invalid JPEG stream.".to_string());
    }

    // Build APP1 EXIF segment ("Exif\0\0" + TIFF payload)
    let tiff_payload = build_tiff_exif_payload(meta);
    let mut exif_app1_data = Vec::with_capacity(6 + tiff_payload.len());
    exif_app1_data.extend_from_slice(b"Exif\0\0");
    exif_app1_data.extend_from_slice(&tiff_payload);

    // Build APP1 XMP segment ("http://ns.adobe.com/xap/1.0/\0" + XMP UTF-8 XML)
    let xmp_xml = build_xmp_packet(meta);
    let xmp_header = b"http://ns.adobe.com/xap/1.0/\0";
    let mut xmp_app1_data = Vec::with_capacity(xmp_header.len() + xmp_xml.len());
    xmp_app1_data.extend_from_slice(xmp_header);
    xmp_app1_data.extend_from_slice(xmp_xml.as_bytes());

    // Find insertion point right after SOI (0xFFD8) and optional APP0 (0xFFE0 JFIF)
    let mut insert_pos = 2usize;
    if raw_jpeg.len() >= 6 && raw_jpeg[2] == 0xFF && raw_jpeg[3] == 0xE0 {
        let seg_len = u16::from_be_bytes([raw_jpeg[4], raw_jpeg[5]]) as usize;
        if 4 + seg_len <= raw_jpeg.len() {
            insert_pos = 2 + 2 + seg_len;
        }
    }

    let mut output =
        Vec::with_capacity(raw_jpeg.len() + exif_app1_data.len() + xmp_app1_data.len() + 16);
    output.extend_from_slice(&raw_jpeg[..insert_pos]);
    append_jpeg_app1_segment(&mut output, &exif_app1_data);
    append_jpeg_app1_segment(&mut output, &xmp_app1_data);
    output.extend_from_slice(&raw_jpeg[insert_pos..]);

    Ok(output)
}

fn append_jpeg_app1_segment(out: &mut Vec<u8>, payload: &[u8]) {
    let total_len = payload.len() + 2;
    if total_len > u16::MAX as usize {
        return;
    }
    out.push(0xFF);
    out.push(0xE1); // APP1 marker
    out.extend_from_slice(&(total_len as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

fn inject_png_chunk_before_iend(png_bytes: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
    // IEND chunk is always the final 12 bytes: [0,0,0,0, 'I','E','N','D', crc(4)]
    if png_bytes.len() < 12 || data.len() > u32::MAX as usize {
        return;
    }
    let iend_pos = png_bytes.len() - 12;
    if &png_bytes[iend_pos + 4..iend_pos + 8] != b"IEND" {
        return;
    }

    let iend_tail = png_bytes.split_off(iend_pos);
    png_bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png_bytes.extend_from_slice(&chunk_type);
    png_bytes.extend_from_slice(data);
    let crc = png_crc32(&chunk_type, data);
    png_bytes.extend_from_slice(&crc.to_be_bytes());
    png_bytes.extend_from_slice(&iend_tail);
}

fn png_crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

struct IfdRawEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    data: Vec<u8>,
}

/// Builds a Little-Endian TIFF/EXIF structure containing standard EXIF tags
/// as well as Windows Explorer UTF-16LE `XPTitle`, `XPComment`, `XPAuthor`, and `XPKeywords`.
fn build_tiff_exif_payload(meta: &ScreenshotMetadata) -> Vec<u8> {
    let title = meta.title.trim();
    let description = meta.description.trim();
    let author = meta.author.trim();
    let tags = meta.parsed_tags();
    let exif_dt = format!("{}\0", meta.captured_at.format("%Y:%m:%d %H:%M:%S"));

    let mut ifd0_entries: Vec<IfdRawEntry> = Vec::new();

    // 0x010E ImageDescription (ASCII / UTF-8)
    let desc_or_title = if !description.is_empty() {
        description
    } else {
        title
    };
    if !desc_or_title.is_empty() {
        let mut bytes = desc_or_title.as_bytes().to_vec();
        bytes.push(0);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x010E,
            field_type: 2, // ASCII
            count: bytes.len() as u32,
            data: bytes,
        });
    }

    // 0x0131 Software
    {
        let mut bytes = SOFTWARE_NAME.as_bytes().to_vec();
        bytes.push(0);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x0131,
            field_type: 2,
            count: bytes.len() as u32,
            data: bytes,
        });
    }

    // 0x0132 DateTime
    ifd0_entries.push(IfdRawEntry {
        tag: 0x0132,
        field_type: 2,
        count: exif_dt.len() as u32,
        data: exif_dt.as_bytes().to_vec(),
    });

    // 0x013B Artist
    if !author.is_empty() {
        let mut bytes = author.as_bytes().to_vec();
        bytes.push(0);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x013B,
            field_type: 2,
            count: bytes.len() as u32,
            data: bytes,
        });
    }

    // Placeholder for 0x8769 ExifIFDPointer (will be filled once IFD0 layout size is known)
    let exif_ptr_index = ifd0_entries.len();
    ifd0_entries.push(IfdRawEntry {
        tag: 0x8769,
        field_type: 4, // LONG
        count: 1,
        data: vec![0, 0, 0, 0],
    });

    // Windows Explorer Details UTF-16LE tags (type = 1 BYTE)
    if !title.is_empty() {
        let utf16 = encode_utf16le_null_terminated(title);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x9C9B, // XPTitle
            field_type: 1,
            count: utf16.len() as u32,
            data: utf16,
        });
    }
    if !description.is_empty() {
        let utf16 = encode_utf16le_null_terminated(description);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x9C9C, // XPComment
            field_type: 1,
            count: utf16.len() as u32,
            data: utf16,
        });
    }
    if !author.is_empty() {
        let utf16 = encode_utf16le_null_terminated(author);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x9C9D, // XPAuthor
            field_type: 1,
            count: utf16.len() as u32,
            data: utf16,
        });
    }
    if !tags.is_empty() {
        let joined = tags.join(";");
        let utf16 = encode_utf16le_null_terminated(&joined);
        ifd0_entries.push(IfdRawEntry {
            tag: 0x9C9E, // XPKeywords
            field_type: 1,
            count: utf16.len() as u32,
            data: utf16,
        });
    }

    // Build Exif SubIFD entries (sorted ascending by tag)
    let mut sub_entries: Vec<IfdRawEntry> = vec![
        IfdRawEntry {
            tag: 0x9003, // DateTimeOriginal
            field_type: 2,
            count: exif_dt.len() as u32,
            data: exif_dt.as_bytes().to_vec(),
        },
        IfdRawEntry {
            tag: 0x9004, // DateTimeDigitized
            field_type: 2,
            count: exif_dt.len() as u32,
            data: exif_dt.as_bytes().to_vec(),
        },
    ];

    if !description.is_empty() {
        let mut uc = b"UNICODE\0".to_vec();
        for unit in description.encode_utf16() {
            uc.extend_from_slice(&unit.to_le_bytes());
        }
        sub_entries.push(IfdRawEntry {
            tag: 0x9286, // UserComment
            field_type: 7, // UNDEFINED
            count: uc.len() as u32,
            data: uc,
        });
    }

    // Serialize TIFF header (8 bytes)
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    buf.extend_from_slice(b"II"); // Little-endian
    buf.extend_from_slice(&0x002Au16.to_le_bytes());
    buf.extend_from_slice(&8u32.to_le_bytes()); // IFD0 starts at byte 8

    // Compute IFD0 size & external value pool
    let ifd0_table_size = 2 + ifd0_entries.len() * 12 + 4;
    let mut ifd0_data_offset = 8 + ifd0_table_size;
    let mut ifd0_extra = Vec::new();

    for entry in &ifd0_entries {
        if entry.data.len() > 4 {
            ifd0_extra.extend_from_slice(&entry.data);
            if ifd0_extra.len() % 2 != 0 {
                ifd0_extra.push(0);
            }
        }
    }

    let sub_ifd_offset = (ifd0_data_offset + ifd0_extra.len()) as u32;
    ifd0_entries[exif_ptr_index].data = sub_ifd_offset.to_le_bytes().to_vec();

    // Write IFD0 table
    buf.extend_from_slice(&(ifd0_entries.len() as u16).to_le_bytes());
    for entry in &ifd0_entries {
        buf.extend_from_slice(&entry.tag.to_le_bytes());
        buf.extend_from_slice(&entry.field_type.to_le_bytes());
        buf.extend_from_slice(&entry.count.to_le_bytes());
        if entry.data.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..entry.data.len()].copy_from_slice(&entry.data);
            buf.extend_from_slice(&inline);
        } else {
            buf.extend_from_slice(&(ifd0_data_offset as u32).to_le_bytes());
            ifd0_data_offset += entry.data.len();
            if entry.data.len() % 2 != 0 {
                ifd0_data_offset += 1;
            }
        }
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // next IFD offset = 0
    buf.extend_from_slice(&ifd0_extra);

    // Write Exif SubIFD table + extra pool
    debug_assert_eq!(buf.len() as u32, sub_ifd_offset);
    let sub_table_size = 2 + sub_entries.len() * 12 + 4;
    let mut sub_data_offset = buf.len() + sub_table_size;
    let mut sub_extra = Vec::new();

    buf.extend_from_slice(&(sub_entries.len() as u16).to_le_bytes());
    for entry in &sub_entries {
        buf.extend_from_slice(&entry.tag.to_le_bytes());
        buf.extend_from_slice(&entry.field_type.to_le_bytes());
        buf.extend_from_slice(&entry.count.to_le_bytes());
        if entry.data.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..entry.data.len()].copy_from_slice(&entry.data);
            buf.extend_from_slice(&inline);
        } else {
            buf.extend_from_slice(&(sub_data_offset as u32).to_le_bytes());
            sub_extra.extend_from_slice(&entry.data);
            sub_data_offset += entry.data.len();
            if entry.data.len() % 2 != 0 {
                sub_extra.push(0);
                sub_data_offset += 1;
            }
        }
    }
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&sub_extra);

    buf
}

fn encode_utf16le_null_terminated(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity((text.len() + 1) * 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&[0x00, 0x00]);
    out
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn build_xmp_packet(meta: &ScreenshotMetadata) -> String {
    let title = meta.title.trim();
    let description = meta.description.trim();
    let author = meta.author.trim();
    let tags = meta.parsed_tags();
    let create_date = meta.captured_at.to_rfc3339();

    let mut rdf_body = String::new();
    rdf_body.push_str(&format!(
        "      <xmp:CreatorTool>{}</xmp:CreatorTool>\n",
        xml_escape(SOFTWARE_NAME)
    ));
    rdf_body.push_str(&format!(
        "      <xmp:CreateDate>{}</xmp:CreateDate>\n",
        xml_escape(&create_date)
    ));

    if !title.is_empty() {
        rdf_body.push_str(&format!(
            "      <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n",
            xml_escape(title)
        ));
    }

    if !description.is_empty() {
        rdf_body.push_str(&format!(
            "      <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n",
            xml_escape(description)
        ));
    }

    if !author.is_empty() {
        rdf_body.push_str(&format!(
            "      <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n",
            xml_escape(author)
        ));
    }

    if !tags.is_empty() {
        rdf_body.push_str("      <dc:subject><rdf:Bag>\n");
        for tag in &tags {
            rdf_body.push_str(&format!("        <rdf:li>{}</rdf:li>\n", xml_escape(tag)));
        }
        rdf_body.push_str("      </rdf:Bag></dc:subject>\n");
    }

    format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
    <rdf:Description rdf:about=\"\"\n\
      xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n\
{rdf_body}    </rdf:Description>\n\
  </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn sample_meta() -> ScreenshotMetadata {
        let mut m = ScreenshotMetadata::new("Ferris Rustacean");
        m.title = "Checkout Button & Discount Bug".to_string();
        m.description = "Button does not respond when <PROMO> coupon is applied.".to_string();
        m.tags_raw = "bug, checkout, ui, Bug,  promo ".to_string();
        m
    }

    #[test]
    fn test_parsed_tags_and_filename() {
        let m = sample_meta();
        assert_eq!(m.parsed_tags(), vec!["bug", "checkout", "ui", "promo"]);
        let fname = m.suggested_filename(SaveFormat::Png);
        assert!(fname.starts_with("checkout-button-discount-bug_"));
        assert!(fname.ends_with(".png"));
    }

    #[test]
    fn test_encode_png_with_metadata_valid_and_contains_chunks() {
        let img = RgbaImage::from_pixel(32, 24, Rgba([20, 120, 220, 255]));
        let meta = sample_meta();
        let bytes = encode_png_with_metadata(&img, &meta).expect("PNG encoding should succeed");

        // Must decode cleanly back into a 32x24 image
        let decoded = image::load_from_memory(&bytes).expect("Decoded PNG should be valid");
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 24);

        // Verify presence of iTXt, XML:com.adobe.xmp, and eXIf chunks
        let text_lossy = String::from_utf8_lossy(&bytes);
        assert!(text_lossy.contains("XML:com.adobe.xmp"));
        assert!(text_lossy.contains("Ferris Rustacean"));
        assert!(text_lossy.contains("&lt;PROMO&gt;"));
        assert!(bytes.windows(4).any(|w| w == b"eXIf"));
    }

    #[test]
    fn test_encode_jpeg_with_metadata_valid_and_contains_exif_xmp() {
        let img = RgbaImage::from_pixel(40, 30, Rgba([240, 80, 40, 255]));
        let meta = sample_meta();
        let bytes =
            encode_jpeg_with_metadata(&img, 92, &meta).expect("JPEG encoding should succeed");

        let decoded = image::load_from_memory(&bytes).expect("Decoded JPEG should be valid");
        assert_eq!(decoded.width(), 40);
        assert_eq!(decoded.height(), 30);

        assert!(bytes.windows(6).any(|w| w == b"Exif\0\0"));
        assert!(bytes
            .windows(29)
            .any(|w| w == b"http://ns.adobe.com/xap/1.0/\0"));
        let utf16_author = encode_utf16le_null_terminated("Ferris Rustacean");
        assert!(bytes
            .windows(utf16_author.len())
            .any(|w| w == utf16_author.as_slice()));
    }

    #[test]
    fn test_png_all_chunk_crcs_and_tiff_ifd_integrity() {
        let img = RgbaImage::from_pixel(16, 16, Rgba([10, 20, 30, 255]));
        let meta = sample_meta();
        let png_bytes = encode_png_with_metadata(&img, &meta).unwrap();

        // 1. Validate PNG signature & walk every chunk verifying CRC32
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
        let mut pos = 8usize;
        let mut saw_itxt = false;
        let mut saw_exif = false;
        let mut saw_iend = false;

        while pos < png_bytes.len() {
            assert!(pos + 12 <= png_bytes.len());
            let len = u32::from_be_bytes(png_bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let ctype: [u8; 4] = png_bytes[pos + 4..pos + 8].try_into().unwrap();
            let data = &png_bytes[pos + 8..pos + 8 + len];
            let stored_crc =
                u32::from_be_bytes(png_bytes[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            assert_eq!(stored_crc, png_crc32(&ctype, data));

            if &ctype == b"iTXt" {
                saw_itxt = true;
            } else if &ctype == b"eXIf" {
                saw_exif = true;
                // 2. Verify TIFF header & strictly ascending IFD0 tags inside eXIf
                assert_eq!(&data[0..4], b"II\x2a\x00");
                let ifd0_off = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
                let count =
                    u16::from_le_bytes(data[ifd0_off..ifd0_off + 2].try_into().unwrap()) as usize;
                let mut prev_tag = 0u16;
                for i in 0..count {
                    let e = ifd0_off + 2 + i * 12;
                    let tag = u16::from_le_bytes(data[e..e + 2].try_into().unwrap());
                    assert!(tag > prev_tag, "TIFF IFD tags must be strictly ascending");
                    prev_tag = tag;
                }
            } else if &ctype == b"IEND" {
                saw_iend = true;
            }
            pos += 12 + len;
        }

        assert!(saw_itxt && saw_exif && saw_iend);
    }
}
