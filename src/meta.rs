//! What a saved photo keeps from the original: its EXIF (camera, lens, date)
//! and, for a wide-colour photo, its Display P3 profile.
//!
//! The EXIF block is edited in place: orientation becomes "upright" (the
//! pixels are already turned), the image size is updated, the old embedded
//! thumbnail is dropped, and unless asked to keep it, the location is wiped,
//! values included. A block that can't be read safely is dropped whole.

/// Metadata found at the start of a JPEG file.
#[derive(Clone, Debug, Default)]
pub struct SourceMeta {
    /// The APP1 payload, from "Exif\0\0" on.
    pub exif: Option<Vec<u8>>,
    /// The ICC profile, reassembled from its APP2 chunks.
    pub icc: Option<Vec<u8>>,
}

/// Reads the metadata segments of a JPEG file, stopping at the image data.
/// `bytes` may be just the start of the file. Anything else gives nothing.
pub fn read_jpeg(bytes: &[u8]) -> SourceMeta {
    let mut meta = SourceMeta::default();
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return meta;
    }
    let mut icc_chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut icc_count = 0;
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xff {
            break;
        }
        let marker = bytes[i + 1];
        if marker == 0xff {
            i += 1; // fill byte
            continue;
        }
        if marker == 0xda || marker == 0xd9 {
            break; // start of scan, end of image
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        let body = &bytes[i + 4..i + 2 + len];
        if marker == 0xe1 && body.starts_with(b"Exif\0\0") && meta.exif.is_none() {
            meta.exif = Some(body.to_vec());
        } else if marker == 0xe2 && body.starts_with(b"ICC_PROFILE\0") && body.len() > 14 {
            icc_count = body[13];
            icc_chunks.push((body[12], &body[14..]));
        }
        i += 2 + len;
    }
    if icc_count > 0 && icc_chunks.len() == icc_count as usize {
        icc_chunks.sort_by_key(|&(seq, _)| seq);
        if icc_chunks.iter().enumerate().all(|(k, &(seq, _))| seq as usize == k + 1) {
            meta.icc = Some(icc_chunks.iter().flat_map(|&(_, d)| d.iter().copied()).collect());
        }
    }
    meta
}

/// Whether an ICC profile has Display P3's primaries (as Apple's cameras
/// write it), so its pixels can be read and saved in that colour space.
pub fn is_display_p3(icc: &[u8]) -> bool {
    // D50-adapted XYZ of the red, green and blue primaries.
    const P3: [[f64; 3]; 3] = [[0.5151, 0.2412, -0.0011], [0.2920, 0.6922, 0.0419], [0.1571, 0.0666, 0.7841]];
    let Some(count) = be32(icc, 128) else { return false };
    let mut found: [Option<[f64; 3]>; 3] = [None; 3];
    for k in 0..count.min(64) as usize {
        let at = 132 + k * 12;
        let (Some(sig), Some(off), Some(len)) = (icc.get(at..at + 4), be32(icc, at + 4), be32(icc, at + 8)) else {
            return false;
        };
        let slot = match sig {
            b"rXYZ" => 0,
            b"gXYZ" => 1,
            b"bXYZ" => 2,
            _ => continue,
        };
        let off = off as usize;
        if len < 20 || icc.get(off..off + 4) != Some(b"XYZ ") {
            return false;
        }
        let xyz = |c: usize| be32(icc, off + 8 + c * 4).map(|v| v as i32 as f64 / 65536.0);
        let (Some(x), Some(y), Some(z)) = (xyz(0), xyz(1), xyz(2)) else { return false };
        found[slot] = Some([x, y, z]);
    }
    found.iter().zip(&P3).all(|(got, want)| {
        got.is_some_and(|g| g.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01))
    })
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// The EXIF block (APP1 payload) to save with a developed photo of
/// `width`×`height`, or None if it can't be edited safely.
pub fn clean_exif(app1: &[u8], keep_location: bool, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut out = app1.to_vec();
    let tiff = out.get_mut(6..)?;
    let mut t = Tiff::new(tiff)?;
    let ifd0 = t.u32(4)? as usize;
    let mut gps = None;
    let mut exif_ifd = None;
    for e in t.entries(ifd0)? {
        match e.tag {
            0x0112 => t.set_value(&e, 1)?, // orientation: upright
            0x8825 => gps = Some(e.offset_value(&t)? as usize),
            0x8769 => exif_ifd = Some(e.offset_value(&t)? as usize),
            0x0100 => t.set_value(&e, width)?,
            0x0101 => t.set_value(&e, height)?,
            _ => {}
        }
    }
    if let Some(at) = exif_ifd {
        for e in t.entries(at)? {
            match e.tag {
                0xa002 => t.set_value(&e, width)?,
                0xa003 => t.set_value(&e, height)?,
                _ => {}
            }
        }
    }
    if let (Some(at), false) = (gps, keep_location) {
        t.wipe_ifd(at)?;
    }
    // The thumbnail (IFD1) still shows the photo as it was: unlink and wipe it.
    let next_at = ifd0 + 2 + 12 * t.u16(ifd0)? as usize;
    let ifd1 = t.u32(next_at)? as usize;
    if ifd1 != 0 {
        let entries = t.entries(ifd1)?;
        let (mut start, mut len) = (None, None);
        for e in &entries {
            match e.tag {
                0x0201 => start = Some(e.offset_value(&t)? as usize),
                0x0202 => len = Some(e.offset_value(&t)? as usize),
                _ => {}
            }
        }
        if let (Some(s), Some(l)) = (start, len) {
            t.zero(s, l)?;
        }
        t.wipe_ifd(ifd1)?;
        t.put_u32(next_at, 0)?;
    }
    Some(out)
}

/// Byte order and access for a TIFF structure (the body of an EXIF block).
struct Tiff<'a> {
    b: &'a mut [u8],
    le: bool,
}

#[derive(Clone, Debug)]
struct Entry {
    /// Where the entry starts.
    at: usize,
    tag: u16,
    kind: u16,
    count: u32,
}

impl Entry {
    /// Bytes per value of this entry's type.
    fn unit(&self) -> Option<usize> {
        Some(match self.kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        })
    }

    /// Where the values are: in the entry itself when they fit in 4 bytes.
    fn data(&self, t: &Tiff) -> Option<(usize, usize)> {
        let len = self.unit()?.checked_mul(self.count as usize)?;
        let at = if len <= 4 { self.at + 8 } else { t.u32(self.at + 8)? as usize };
        (at.checked_add(len)? <= t.b.len()).then_some((at, len))
    }

    /// A single LONG or SHORT value, such as an offset.
    fn offset_value(&self, t: &Tiff) -> Option<u32> {
        match (self.kind, self.count) {
            (4 | 13, 1) => t.u32(self.at + 8),
            (3, 1) => t.u16(self.at + 8).map(u32::from),
            _ => None,
        }
    }
}

impl<'a> Tiff<'a> {
    fn new(b: &'a mut [u8]) -> Option<Self> {
        let le = match b.get(..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Tiff { b, le })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let s = self.b.get(at..at + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }

    fn put_u16(&mut self, at: usize, v: u16) -> Option<()> {
        let bytes = if self.le { v.to_le_bytes() } else { v.to_be_bytes() };
        self.b.get_mut(at..at + 2)?.copy_from_slice(&bytes);
        Some(())
    }

    fn put_u32(&mut self, at: usize, v: u32) -> Option<()> {
        let bytes = if self.le { v.to_le_bytes() } else { v.to_be_bytes() };
        self.b.get_mut(at..at + 4)?.copy_from_slice(&bytes);
        Some(())
    }

    fn zero(&mut self, at: usize, len: usize) -> Option<()> {
        self.b.get_mut(at..at.checked_add(len)?)?.fill(0);
        Some(())
    }

    fn entries(&self, ifd: usize) -> Option<Vec<Entry>> {
        let n = self.u16(ifd)? as usize;
        (0..n)
            .map(|k| {
                let at = ifd + 2 + 12 * k;
                Some(Entry { at, tag: self.u16(at)?, kind: self.u16(at + 2)?, count: self.u32(at + 4)? })
            })
            .collect()
    }

    /// Sets a single SHORT or LONG value.
    fn set_value(&mut self, e: &Entry, v: u32) -> Option<()> {
        match (e.kind, e.count) {
            (3, 1) => self.put_u16(e.at + 8, u16::try_from(v).ok()?),
            (4, 1) => self.put_u32(e.at + 8, v),
            _ => Some(()),
        }
    }

    /// Zeroes an IFD's values and entries, leaving an empty IFD in its place.
    fn wipe_ifd(&mut self, ifd: usize) -> Option<()> {
        let entries = self.entries(ifd)?;
        for e in &entries {
            if let Some((at, len)) = e.data(self) {
                self.zero(at, len)?;
            }
        }
        // The count, every entry and the link to a next IFD.
        self.zero(ifd, 2 + 12 * entries.len() + 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian EXIF block: IFD0 with orientation 6, a GPS IFD with a
    /// latitude (3 rationals), and IFD1 with a 4-byte thumbnail.
    fn sample_exif() -> Vec<u8> {
        let mut t: Vec<u8> = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
            t.extend(tag.to_le_bytes());
            t.extend(kind.to_le_bytes());
            t.extend(count.to_le_bytes());
            t.extend(value.to_le_bytes());
        };
        // IFD0 at 8: 2 entries, then the next-IFD link at 8 + 2 + 24 = 34.
        t.extend(2u16.to_le_bytes());
        entry(&mut t, 0x0112, 3, 1, 6);
        entry(&mut t, 0x8825, 4, 1, 38);
        t.extend(80u32.to_le_bytes()); // IFD1 at 80
        // GPS IFD at 38: one entry, latitude (3 RATIONALs at 56), link 0.
        t.extend(1u16.to_le_bytes());
        entry(&mut t, 0x0002, 5, 3, 56);
        t.extend(0u32.to_le_bytes());
        assert_eq!(t.len(), 56);
        for v in [52u32, 1, 22, 1, 3333, 100] {
            t.extend(v.to_le_bytes());
        }
        assert_eq!(t.len(), 80);
        // IFD1 at 80: thumbnail at 110, 4 bytes long.
        t.extend(2u16.to_le_bytes());
        entry(&mut t, 0x0201, 4, 1, 110);
        entry(&mut t, 0x0202, 4, 1, 4);
        t.extend(0u32.to_le_bytes());
        assert_eq!(t.len(), 110);
        t.extend([0xff, 0xd8, 0xff, 0xd9]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(t);
        app1
    }

    #[test]
    fn exif_is_cleaned_in_place() {
        let src = sample_exif();
        let out = clean_exif(&src, false, 10, 10).unwrap();
        assert_eq!(out.len(), src.len());
        let tiff = &out[6..];
        assert_eq!(u16::from_le_bytes([tiff[18], tiff[19]]), 1, "orientation reset");
        assert!(tiff[38..80].iter().all(|&b| b == 0), "location wiped, values included");
        assert_eq!(&tiff[34..38], &[0, 0, 0, 0], "thumbnail unlinked");
        assert!(tiff[80..114].iter().all(|&b| b == 0), "thumbnail wiped");
    }

    #[test]
    fn location_can_be_kept() {
        let src = sample_exif();
        let out = clean_exif(&src, true, 10, 10).unwrap();
        assert_eq!(&out[6 + 56..6 + 60], &52u32.to_le_bytes());
    }

    #[test]
    fn broken_exif_is_dropped() {
        let mut src = sample_exif();
        src[6 + 30] = 0xff; // the GPS pointer now points past the end
        src[6 + 31] = 0xff;
        assert!(clean_exif(&src, false, 10, 10).is_none());
        assert!(clean_exif(b"Exif\0\0nonsense", false, 10, 10).is_none());
    }

    #[test]
    fn reads_segments_until_the_image_data() {
        let exif = sample_exif();
        let mut jpeg = vec![0xff, 0xd8];
        let mut segment = |marker: u8, body: &[u8]| {
            jpeg.extend([0xff, marker]);
            jpeg.extend(((body.len() + 2) as u16).to_be_bytes());
            jpeg.extend(body);
        };
        segment(0xe1, &exif);
        segment(0xe2, b"ICC_PROFILE\0\x02\x02world");
        segment(0xe2, b"ICC_PROFILE\0\x01\x02hello ");
        segment(0xda, b"scan");
        let meta = read_jpeg(&jpeg);
        assert_eq!(meta.exif.as_deref(), Some(&exif[..]));
        assert_eq!(meta.icc.as_deref(), Some(&b"hello world"[..]));
        assert!(read_jpeg(b"\x89PNG").exif.is_none());
    }
}
