//! Image header sniffing: format, size and file extension without decoding pixels.

/// Returns `(mime, extension, width, height)` for PNG, JPEG, GIF and WebP data.
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, &'static str, u32, u32)> {
    let (mime, ext, (w, h)) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ("image/png", "png", png(bytes)?)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        ("image/jpeg", "jpg", jpeg(bytes)?)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ("image/gif", "gif", gif(bytes)?)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ("image/webp", "webp", webp(bytes)?)
    } else {
        return None;
    };
    if w == 0 || h == 0 {
        return None;
    }
    Some((mime, ext, w, h))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32)
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32)
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16)
}

fn png(b: &[u8]) -> Option<(u32, u32)> {
    // Signature, then the IHDR chunk: length(4) "IHDR" width(4) height(4).
    if b.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(b, 16)?, be32(b, 20)?))
}

fn gif(b: &[u8]) -> Option<(u32, u32)> {
    Some((le16(b, 6)?, le16(b, 8)?))
}

fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    loop {
        // Find the next marker: 0xFF followed by a non-fill, non-zero byte.
        while *b.get(i)? != 0xFF {
            i += 1;
        }
        while *b.get(i)? == 0xFF {
            i += 1;
        }
        let m = *b.get(i)?;
        i += 1;
        match m {
            // Standalone markers without a length.
            0x00 | 0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return None,
            // SOFn (not DHT 0xC4, JPG 0xC8, DAC 0xCC).
            0xC0..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                // length(2) precision(1) height(2) width(2)
                return Some((be16(b, i + 5)?, be16(b, i + 3)?));
            }
            _ => {
                let len = be16(b, i)? as usize;
                if len < 2 {
                    return None;
                }
                i += len;
            }
        }
    }
}

fn webp(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        // Lossy: 3-byte frame tag, start code 9d 01 2a, then 14-bit width/height.
        b"VP8 " => {
            if b.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF))
        }
        // Lossless: signature 0x2f, then 14 bits width-1 and 14 bits height-1.
        b"VP8L" => {
            if *b.get(20)? != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        // Extended: flags(4) then 24-bit canvas width-1 and height-1.
        b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_header() {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&1280u32.to_be_bytes());
        b.extend_from_slice(&800u32.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(sniff(&b), Some(("image/png", "png", 1280, 800)));
        assert_eq!(sniff(&b[..20]), None);
    }

    #[test]
    fn jpeg_scans_markers() {
        let mut b = vec![0xFF, 0xD8];
        // APP0 with 16 bytes of payload (length includes itself).
        b.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        b.extend_from_slice(&[0; 14]);
        // DHT, skipped even though it is in the SOF range.
        b.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x04, 0, 0]);
        // SOF2 (progressive): length 11, precision 8, height 620, width 980.
        b.extend_from_slice(&[0xFF, 0xC2, 0x00, 0x0B, 0x08]);
        b.extend_from_slice(&620u16.to_be_bytes());
        b.extend_from_slice(&980u16.to_be_bytes());
        b.extend_from_slice(&[3, 0, 0, 0]);
        assert_eq!(sniff(&b), Some(("image/jpeg", "jpg", 980, 620)));
        // Truncated before the SOF.
        assert_eq!(sniff(&b[..20]), None);
    }

    #[test]
    fn gif_header() {
        let mut b = b"GIF89a".to_vec();
        b.extend_from_slice(&64u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        assert_eq!(sniff(&b), Some(("image/gif", "gif", 64, 32)));
    }

    fn riff(fourcc: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&((payload.len() + 12) as u32).to_le_bytes());
        b.extend_from_slice(b"WEBP");
        b.extend_from_slice(fourcc);
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
        b
    }

    #[test]
    fn webp_lossy() {
        let mut p = vec![0, 0, 0, 0x9D, 0x01, 0x2A];
        p.extend_from_slice(&300u16.to_le_bytes());
        p.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(
            sniff(&riff(b"VP8 ", &p)),
            Some(("image/webp", "webp", 300, 200))
        );
    }

    #[test]
    fn webp_lossless() {
        let bits: u32 = (640 - 1) | ((480 - 1) << 14);
        let mut p = vec![0x2F];
        p.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            sniff(&riff(b"VP8L", &p)),
            Some(("image/webp", "webp", 640, 480))
        );
    }

    #[test]
    fn webp_extended() {
        let mut p = vec![0, 0, 0, 0];
        p.extend_from_slice(&(1920u32 - 1).to_le_bytes()[..3]);
        p.extend_from_slice(&(1080u32 - 1).to_le_bytes()[..3]);
        assert_eq!(
            sniff(&riff(b"VP8X", &p)),
            Some(("image/webp", "webp", 1920, 1080))
        );
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"hello world, not an image"), None);
        assert_eq!(sniff(&[0xFF, 0xD8]), None);
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
    }
}
