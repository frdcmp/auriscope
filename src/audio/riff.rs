//! RIFF/WAVE header walk, independent of decoding.
//!
//! Symphonia gives us the audio; this gives us what the file *says* about
//! itself: the chunk list, the raw `fmt ` fields, Broadcast Wave metadata
//! (`bext`), `LIST INFO` tags and cue markers. Only chunk headers and the
//! few chunks of interest are read, so this costs a handful of small reads
//! no matter how large the file is.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Largest metadata chunk we will read into memory.
const CHUNK_CAP: u64 = 1 << 20;

#[derive(Debug, Clone, Default)]
pub struct WavHeader {
    /// `RF64` rather than `RIFF`: 64-bit sizes via a `ds64` chunk.
    pub rf64: bool,
    /// Declared size of the RIFF body, in bytes.
    pub riff_size: u64,
    pub chunks: Vec<Chunk>,
    pub fmt: Option<Fmt>,
    pub bext: Option<Bext>,
    /// `LIST INFO` fields, as (human name, value), in file order.
    pub info: Vec<(String, String)>,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    /// Four-character code, trailing spaces trimmed.
    pub id: String,
    /// Payload size in bytes.
    pub size: u64,
    /// Offset of the chunk header from the start of the file.
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct Fmt {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// WAVE_FORMAT_EXTENSIBLE only.
    pub valid_bits: Option<u16>,
    pub channel_mask: Option<u32>,
    /// First two bytes of the sub-format GUID, which carry the real tag.
    pub subformat: Option<u16>,
}

impl Fmt {
    pub fn is_extensible(&self) -> bool {
        self.format_tag == 0xFFFE
    }

    /// The effective format tag, looking through the extensible wrapper.
    pub fn effective_tag(&self) -> u16 {
        self.subformat.unwrap_or(self.format_tag)
    }

    pub fn format_name(&self) -> String {
        let name = match self.effective_tag() {
            0x0001 => "PCM".to_string(),
            0x0003 => "IEEE float".into(),
            0x0006 => "A-law".into(),
            0x0007 => "µ-law".into(),
            0x0011 => "IMA ADPCM".into(),
            0x0050 => "MPEG".into(),
            0x0055 => "MP3".into(),
            0x0092 => "Dolby AC-3".into(),
            other => format!("0x{other:04X}"),
        };
        if self.is_extensible() {
            format!("{name} (extensible)")
        } else {
            name
        }
    }
}

/// Broadcast Wave Format metadata (EBU Tech 3285).
#[derive(Debug, Clone, Default)]
pub struct Bext {
    pub description: String,
    pub originator: String,
    pub originator_reference: String,
    pub origination_date: String,
    pub origination_time: String,
    /// First sample of the file since midnight, in samples.
    pub time_reference: u64,
    pub version: u16,
    /// SMPTE UMID as hex, when not all zero.
    pub umid: Option<String>,
    /// Version 2 loudness fields, when present.
    pub loudness: Option<BextLoudness>,
    pub coding_history: String,
}

#[derive(Debug, Clone, Copy)]
pub struct BextLoudness {
    pub integrated_lufs: f32,
    pub range_lu: f32,
    pub max_true_peak_dbtp: f32,
    pub max_momentary_lufs: f32,
    pub max_short_term_lufs: f32,
}

#[derive(Debug, Clone)]
pub struct Cue {
    pub id: u32,
    /// Sample offset into the data chunk.
    pub position: u64,
    pub label: Option<String>,
}

/// Read the header of a RIFF/RF64 WAVE file. `None` if the file is not one.
pub fn read_wav_header(path: &Path) -> Option<WavHeader> {
    let mut f = File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).ok()?;
    let rf64 = match &hdr[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if &hdr[8..12] != b"WAVE" {
        return None;
    }
    let mut out = WavHeader {
        rf64,
        riff_size: u32_at(&hdr, 4) as u64,
        ..Default::default()
    };
    let mut ds64_data: Option<u64> = None;
    let mut labels: Vec<(u32, String)> = Vec::new();

    let mut pos = 12u64;
    while pos + 8 <= file_len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut ch = [0u8; 8];
        if f.read_exact(&mut ch).is_err() {
            break;
        }
        let id_raw = &ch[0..4];
        let id = String::from_utf8_lossy(id_raw).trim_end().to_string();
        let mut size = u32_at(&ch, 4) as u64;
        if rf64 && size == 0xFFFF_FFFF && id == "data" {
            size = ds64_data.unwrap_or(size);
        }
        let body = pos + 8;
        let read_body = |f: &mut File| -> Option<Vec<u8>> {
            let n = size.min(CHUNK_CAP).min(file_len.saturating_sub(body));
            let mut buf = vec![0u8; n as usize];
            f.seek(SeekFrom::Start(body)).ok()?;
            f.read_exact(&mut buf).ok()?;
            Some(buf)
        };
        match id_raw {
            b"ds64" => {
                if let Some(b) = read_body(&mut f)
                    && b.len() >= 16
                {
                    out.riff_size = u64_at(&b, 0);
                    ds64_data = Some(u64_at(&b, 8));
                }
            }
            b"fmt " => {
                if let Some(b) = read_body(&mut f) {
                    out.fmt = parse_fmt(&b);
                }
            }
            b"bext" => {
                if let Some(b) = read_body(&mut f) {
                    out.bext = parse_bext(&b);
                }
            }
            b"LIST" => {
                if let Some(b) = read_body(&mut f) {
                    parse_list(&b, &mut out.info, &mut labels);
                }
            }
            b"cue " => {
                if let Some(b) = read_body(&mut f) {
                    out.cues = parse_cues(&b);
                }
            }
            _ => {}
        }
        out.chunks.push(Chunk {
            id,
            size,
            offset: pos,
        });
        // Chunks are word-aligned.
        pos = body.saturating_add(size).saturating_add(size & 1);
    }

    for cue in &mut out.cues {
        cue.label = labels
            .iter()
            .find(|(id, _)| *id == cue.id)
            .map(|(_, l)| l.clone());
    }
    Some(out)
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u32_at(b, i) as u64 | (u32_at(b, i + 4) as u64) << 32
}

/// Fixed-width, NUL-padded ASCII field.
fn field(b: &[u8], range: std::ops::Range<usize>) -> String {
    let s = &b[range.start.min(b.len())..range.end.min(b.len())];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).trim().to_string()
}

fn parse_fmt(b: &[u8]) -> Option<Fmt> {
    if b.len() < 16 {
        return None;
    }
    let mut fmt = Fmt {
        format_tag: u16_at(b, 0),
        channels: u16_at(b, 2),
        sample_rate: u32_at(b, 4),
        byte_rate: u32_at(b, 8),
        block_align: u16_at(b, 12),
        bits_per_sample: u16_at(b, 14),
        valid_bits: None,
        channel_mask: None,
        subformat: None,
    };
    if fmt.is_extensible() && b.len() >= 40 {
        fmt.valid_bits = Some(u16_at(b, 18));
        fmt.channel_mask = Some(u32_at(b, 20));
        fmt.subformat = Some(u16_at(b, 24));
    }
    Some(fmt)
}

fn parse_bext(b: &[u8]) -> Option<Bext> {
    if b.len() < 602 {
        return None;
    }
    let version = u16_at(b, 346);
    let umid = &b[348..412];
    let umid = umid
        .iter()
        .any(|&c| c != 0)
        .then(|| umid.iter().map(|c| format!("{c:02X}")).collect::<String>());
    let loud = |i: usize| u16_at(b, i) as i16 as f32 / 100.0;
    let loudness = (version >= 2 && u16_at(b, 412) as i16 != 0x7FFF).then(|| BextLoudness {
        integrated_lufs: loud(412),
        range_lu: loud(414),
        max_true_peak_dbtp: loud(416),
        max_momentary_lufs: loud(418),
        max_short_term_lufs: loud(420),
    });
    Some(Bext {
        description: field(b, 0..256),
        originator: field(b, 256..288),
        originator_reference: field(b, 288..320),
        origination_date: field(b, 320..330),
        origination_time: field(b, 330..338),
        time_reference: u64_at(b, 338),
        version,
        umid,
        loudness,
        coding_history: field(b, 602..b.len()),
    })
}

fn info_name(id: &[u8]) -> String {
    match id {
        b"INAM" => "Title",
        b"IART" => "Artist",
        b"IPRD" => "Product",
        b"IALB" => "Album",
        b"ICRD" => "Date",
        b"IGNR" => "Genre",
        b"ICMT" => "Comment",
        b"ISFT" => "Software",
        b"ICOP" => "Copyright",
        b"IENG" => "Engineer",
        b"ITRK" => "Track",
        b"ISBJ" => "Subject",
        b"IKEY" => "Keywords",
        b"ITCH" => "Technician",
        b"ISRC" => "Source",
        b"IMED" => "Medium",
        b"ICMS" => "Commissioned",
        other => return String::from_utf8_lossy(other).trim().to_string(),
    }
    .to_string()
}

fn parse_list(b: &[u8], info: &mut Vec<(String, String)>, labels: &mut Vec<(u32, String)>) {
    if b.len() < 4 {
        return;
    }
    let kind = &b[0..4];
    let mut pos = 4usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(b, pos + 4) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(b.len());
        let data = &b[body.min(end)..end];
        match kind {
            b"INFO" => {
                let v = field(data, 0..data.len());
                if !v.is_empty() {
                    info.push((info_name(id), v));
                }
            }
            b"adtl" if id == b"labl" && data.len() >= 4 => {
                labels.push((u32_at(data, 0), field(data, 4..data.len())));
            }
            _ => {}
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
}

fn parse_cues(b: &[u8]) -> Vec<Cue> {
    if b.len() < 4 {
        return Vec::new();
    }
    let n = u32_at(b, 0) as usize;
    (0..n)
        .map_while(|i| {
            let at = 4 + i * 24;
            (at + 24 <= b.len()).then(|| Cue {
                id: u32_at(b, at),
                position: u32_at(b, at + 20) as u64,
                label: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn fmt_pcm(channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * bits as u32 / 8).to_le_bytes());
        b.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b
    }

    fn write_riff(path: &Path, chunks: &[Vec<u8>]) {
        let body: Vec<u8> = chunks.concat();
        let mut f = File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&((body.len() + 4) as u32).to_le_bytes())
            .unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(&body).unwrap();
    }

    #[test]
    fn plain_wav_lists_fmt_and_data() {
        let dir = std::env::temp_dir().join(format!("auriscope-riff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.wav");
        let sig: Vec<f32> = (0..480).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        crate::audio::decoder::write_wav_i16(&path, 48_000, &[sig]).unwrap();
        let h = read_wav_header(&path).unwrap();
        assert!(!h.rf64);
        let ids: Vec<&str> = h.chunks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["fmt", "data"]);
        assert_eq!(h.chunks[1].size, 480 * 2);
        let fmt = h.fmt.unwrap();
        assert_eq!(fmt.sample_rate, 48_000);
        assert_eq!(fmt.bits_per_sample, 16);
        assert_eq!(fmt.format_name(), "PCM");
        assert!(h.bext.is_none() && h.info.is_empty() && h.cues.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bwf_metadata_info_tags_and_labelled_cues() {
        let dir = std::env::temp_dir().join(format!("auriscope-riff-bwf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bwf.wav");

        let mut bext = vec![0u8; 602];
        bext[..11].copy_from_slice(b"Interview 3");
        bext[256..261].copy_from_slice(b"Zoom ");
        bext[288..297].copy_from_slice(b"REF-00042");
        bext[320..330].copy_from_slice(b"2026-09-16");
        bext[330..338].copy_from_slice(b"10:22:00");
        bext[338..346].copy_from_slice(&(48_000u64 * 3600 * 10).to_le_bytes()); // 10:00:00
        bext[346..348].copy_from_slice(&2u16.to_le_bytes());
        bext[412..414].copy_from_slice(&(-2310i16).to_le_bytes()); // -23.10 LUFS
        bext[414..416].copy_from_slice(&(650i16).to_le_bytes());
        bext[416..418].copy_from_slice(&(-120i16).to_le_bytes());
        bext.extend_from_slice(b"A=PCM,F=48000,W=24,M=mono,T=Zoom F6\r\n");

        let mut info = Vec::new();
        info.extend_from_slice(b"INFO");
        info.extend(chunk(b"INAM", b"Take 7\0"));
        info.extend(chunk(b"ISFT", b"Auriscope\0"));

        let mut cue = Vec::new();
        cue.extend_from_slice(&2u32.to_le_bytes());
        for (id, pos) in [(1u32, 24_000u32), (2, 96_000)] {
            cue.extend_from_slice(&id.to_le_bytes());
            cue.extend_from_slice(&pos.to_le_bytes());
            cue.extend_from_slice(b"data");
            cue.extend_from_slice(&0u32.to_le_bytes());
            cue.extend_from_slice(&0u32.to_le_bytes());
            cue.extend_from_slice(&pos.to_le_bytes());
        }
        let mut adtl = Vec::new();
        adtl.extend_from_slice(b"adtl");
        let mut labl = 2u32.to_le_bytes().to_vec();
        labl.extend_from_slice(b"chorus\0");
        adtl.extend(chunk(b"labl", &labl));

        write_riff(
            &path,
            &[
                chunk(b"fmt ", &fmt_pcm(1, 48_000, 24)),
                chunk(b"bext", &bext),
                chunk(b"LIST", &info),
                chunk(b"cue ", &cue),
                chunk(b"LIST", &adtl),
                chunk(b"data", &[0u8; 6]),
            ],
        );

        let h = read_wav_header(&path).unwrap();
        let ids: Vec<&str> = h.chunks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["fmt", "bext", "LIST", "cue", "LIST", "data"]);
        let b = h.bext.unwrap();
        assert_eq!(b.description, "Interview 3");
        assert_eq!(b.originator, "Zoom");
        assert_eq!(b.originator_reference, "REF-00042");
        assert_eq!(b.origination_date, "2026-09-16");
        assert_eq!(b.time_reference, 48_000 * 36_000);
        assert_eq!(b.version, 2);
        assert!(b.umid.is_none());
        let l = b.loudness.unwrap();
        assert!((l.integrated_lufs + 23.1).abs() < 1e-3);
        assert!((l.range_lu - 6.5).abs() < 1e-3);
        assert!(b.coding_history.starts_with("A=PCM"));
        assert_eq!(
            h.info,
            vec![
                ("Title".to_string(), "Take 7".to_string()),
                ("Software".to_string(), "Auriscope".to_string())
            ]
        );
        assert_eq!(h.cues.len(), 2);
        assert_eq!(h.cues[0].position, 24_000);
        assert_eq!(h.cues[0].label, None);
        assert_eq!(h.cues[1].label.as_deref(), Some("chorus"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_wave_files_are_rejected() {
        let dir = std::env::temp_dir().join(format!("auriscope-riff-no-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.bin");
        std::fs::write(&path, b"fLaC\0\0\0\"random bytes that are not riff").unwrap();
        assert!(read_wav_header(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
