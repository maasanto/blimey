//! Markers in a WAV file, as the DAW renders them: `cue ` points, named by the `labl` entries
//! of a `LIST`/`adtl` chunk, and regions, an `ltxt` entry on the cue point they start at.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use crate::song::Marker;

/// More than any song carries; a count past it is a damaged file, not a long list to allocate.
const MAX_CUE_POINTS: u32 = 10_000;
/// The most read of one `fmt ` or `cue ` chunk, and of all `adtl` lists together; the audio and
/// other lists are skipped, never read.
const MAX_METADATA_CHUNK: u32 = 1 << 20;
const CUE_POINT_SIZE: usize = 24;
/// `ltxt`: cue point id, sample length, purpose, country, language, dialect, code page.
const LTXT_HEADER_SIZE: usize = 20;

/// The file's markers in time order. A WAV without cue points has none; anything that isn't a
/// whole WAV file is an error naming what is wrong.
pub fn markers(file: &mut (impl Read + Seek)) -> Result<Vec<Marker>, String> {
    let length = file.seek(SeekFrom::End(0)).map_err(damaged)?;
    file.seek(SeekFrom::Start(0)).map_err(damaged)?;
    let mut riff = [0; 12];
    if file.read_exact(&mut riff).is_err() || &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
        return Err("not a WAV file: it doesn't start with a RIFF/WAVE header".into());
    }
    let riff_end = 8 + u64::from(u32_at(&riff, 4));
    if riff_end > length {
        return Err(format!(
            "damaged WAV file: it should hold {riff_end} bytes but holds {length}; was it cut short?"
        ));
    }
    let length = riff_end;
    let mut sample_rate = None;
    let mut cue_points = Vec::new();
    let mut labels = Labels::default();
    // Across every adtl list, so the names kept stay within MAX_METADATA_CHUNK however many lists.
    let mut adtl_bytes = 0u64;
    let mut position = 12;
    while position + 8 <= length {
        file.seek(SeekFrom::Start(position)).map_err(damaged)?;
        let mut header = [0; 8];
        file.read_exact(&mut header).map_err(damaged)?;
        let (id, size) = (&header[..4], u32_at(&header, 4));
        let end = position + 8 + u64::from(size);
        if end > length {
            return Err(format!(
                "damaged WAV file: its {:?} chunk runs past the end of the file; was it cut short?",
                String::from_utf8_lossy(id)
            ));
        }
        match id {
            b"fmt " => sample_rate = Some(format_sample_rate(&chunk(file, id, size)?)?),
            b"cue " => cue_points = parse_cue_points(&chunk(file, id, size)?)?,
            b"LIST" if size >= 4 => {
                let mut list_type = [0; 4];
                file.read_exact(&mut list_type).map_err(damaged)?;
                // Other lists, such as INFO, may be large and say nothing of markers.
                if &list_type == b"adtl" {
                    adtl_bytes += u64::from(size - 4);
                    if adtl_bytes > u64::from(MAX_METADATA_CHUNK) {
                        return Err("damaged WAV file: its adtl lists hold over 1 MB, more than markers ever take".into());
                    }
                    labels.read_entries(&chunk(file, b"adtl", size - 4)?);
                }
            }
            _ => {}
        }
        // Chunks start on an even byte: an odd-sized one is followed by a padding byte.
        position = end + u64::from(size & 1);
    }
    if cue_points.is_empty() {
        return Ok(Vec::new());
    }
    let sample_rate = sample_rate
        .ok_or("damaged WAV file: it has cue points but no fmt chunk giving its sample rate")?;
    Ok(labels.markers(&cue_points, sample_rate))
}

fn damaged(error: io::Error) -> String {
    format!("damaged WAV file: {error}")
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn chunk(file: &mut impl Read, id: &[u8], size: u32) -> Result<Vec<u8>, String> {
    if size > MAX_METADATA_CHUNK {
        return Err(format!(
            "damaged WAV file: its {:?} chunk claims {size} bytes, more than markers ever take",
            String::from_utf8_lossy(id)
        ));
    }
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes).map_err(damaged)?;
    Ok(bytes)
}

fn format_sample_rate(format: &[u8]) -> Result<u32, String> {
    match format.get(4..8).map(|_| u32_at(format, 4)) {
        Some(rate) if rate > 0 => Ok(rate),
        _ => Err("damaged WAV file: its fmt chunk gives no sample rate".into()),
    }
}

/// Cue point id → its sample position in the audio.
fn parse_cue_points(cue: &[u8]) -> Result<Vec<(u32, u32)>, String> {
    let count = cue.get(..4).map_or(0, |_| u32_at(cue, 0));
    if count > MAX_CUE_POINTS || cue.len() < 4 + count as usize * CUE_POINT_SIZE {
        return Err(format!(
            "damaged WAV file: its cue chunk lists {count} cue points but doesn't hold them"
        ));
    }
    Ok(cue[4..]
        .chunks_exact(CUE_POINT_SIZE)
        .take(count as usize)
        .map(|point| (u32_at(point, 0), u32_at(point, 20)))
        .collect())
}

/// The names `adtl` gives cue points: a `labl`, else a region's `ltxt` text.
#[derive(Default)]
struct Labels {
    labels: BTreeMap<u32, String>,
    region_texts: BTreeMap<u32, String>,
}

impl Labels {
    /// The entries of an `adtl` list, after its type.
    fn read_entries(&mut self, mut entries: &[u8]) {
        while entries.len() >= 8 {
            let size = u32_at(entries, 4) as usize;
            let Some(entry) = entries.get(8..8 + size) else {
                return;
            };
            match &entries[..4] {
                b"labl" if size >= 4 => {
                    self.labels.insert(u32_at(entry, 0), text(&entry[4..]));
                }
                b"ltxt" if size >= LTXT_HEADER_SIZE => {
                    let text = text(&entry[LTXT_HEADER_SIZE..]);
                    self.region_texts.insert(u32_at(entry, 0), text);
                }
                _ => {}
            }
            entries = entries.get(8 + size + (size & 1)..).unwrap_or_default();
        }
    }

    fn markers(&self, cue_points: &[(u32, u32)], sample_rate: u32) -> Vec<Marker> {
        let mut points = cue_points.to_vec();
        points.sort_by_key(|&(id, sample)| (sample, id));
        points
            .iter()
            .enumerate()
            .map(|(index, (id, sample))| {
                let name = [self.labels.get(id), self.region_texts.get(id)]
                    .into_iter()
                    .flatten()
                    .find(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Marker {}", index + 1));
                let at = Duration::from_secs_f64(f64::from(*sample) / f64::from(sample_rate));
                Marker { name, at }
            })
            .collect()
    }
}

/// A zero-terminated string, as RIFF writes them.
fn text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

/// WAV files with markers, built byte by byte for tests here and in the API's.
#[doc(hidden)]
pub mod build {
    /// A RIFF chunk, with its padding byte when `body` is odd-sized.
    pub fn riff_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut chunk = id.to_vec();
        chunk.extend((body.len() as u32).to_le_bytes());
        chunk.extend(body);
        if body.len() % 2 == 1 {
            chunk.push(0);
        }
        chunk
    }

    /// A mono 16-bit WAV at `sample_rate`: `chunks` after `fmt `, ahead of a short `data`.
    pub fn wav(sample_rate: u32, chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut format = vec![1, 0, 1, 0];
        format.extend(sample_rate.to_le_bytes());
        format.extend((sample_rate * 2).to_le_bytes());
        format.extend([2, 0, 16, 0]);
        let mut body = b"WAVE".to_vec();
        body.extend(riff_chunk(b"fmt ", &format));
        body.extend(riff_chunk(b"data", &[0; 64]));
        for chunk in chunks {
            body.extend(chunk);
        }
        riff_chunk(b"RIFF", &body)
    }

    /// A `cue ` chunk of `(id, sample)` points.
    pub fn cue_points(points: &[(u32, u32)]) -> Vec<u8> {
        let mut body = (points.len() as u32).to_le_bytes().to_vec();
        for (id, sample) in points {
            body.extend(id.to_le_bytes());
            body.extend(sample.to_le_bytes());
            body.extend(b"data");
            body.extend([0; 8]);
            body.extend(sample.to_le_bytes());
        }
        riff_chunk(b"cue ", &body)
    }

    pub fn adtl(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"adtl".to_vec();
        for entry in entries {
            body.extend(entry);
        }
        riff_chunk(b"LIST", &body)
    }

    pub fn label(id: u32, name: &str) -> Vec<u8> {
        let mut body = id.to_le_bytes().to_vec();
        body.extend(name.as_bytes());
        body.push(0);
        riff_chunk(b"labl", &body)
    }

    pub fn region(id: u32, samples: u32, text: &str) -> Vec<u8> {
        let mut body = id.to_le_bytes().to_vec();
        body.extend(samples.to_le_bytes());
        body.extend(b"rgn ");
        body.extend([0; 8]);
        body.extend(text.as_bytes());
        riff_chunk(b"ltxt", &body)
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;
    use std::io::Cursor;

    fn read(bytes: Vec<u8>) -> Result<Vec<(String, Duration)>, String> {
        markers(&mut Cursor::new(bytes))
            .map(|markers| markers.into_iter().map(|m| (m.name, m.at)).collect())
    }

    fn at(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn named_and_unnamed_cue_points_and_a_region_read_in_time_order() {
        let file = wav(
            44_100,
            &[
                cue_points(&[(1, 88_200), (2, 22_050), (3, 441_000), (4, 132_300)]),
                adtl(&[
                    label(1, "Verse"),
                    region(3, 44_100, ""),
                    label(3, "Chorus"),
                    region(4, 44_100, "Bridge"),
                ]),
            ],
        );
        assert_eq!(
            read(file),
            Ok(vec![
                ("Marker 1".into(), at(0.5)),
                ("Verse".into(), at(2.0)),
                ("Bridge".into(), at(3.0)),
                ("Chorus".into(), at(10.0)),
            ])
        );
    }

    #[test]
    fn odd_sized_chunks_are_padded_and_the_sample_rate_is_the_files() {
        // A 3-byte unknown chunk and an odd-length label both carry a padding byte.
        let file = wav(
            48_000,
            &[
                riff_chunk(b"junk", b"abc"),
                cue_points(&[(7, 96_000)]),
                adtl(&[label(7, "Outro"), label(8, "ab")]),
            ],
        );
        assert_eq!(read(file), Ok(vec![("Outro".into(), at(2.0))]));
    }

    #[test]
    fn a_large_info_list_is_skipped_but_adtl_lists_are_capped_together() {
        let mut info = b"INFO".to_vec();
        info.resize(2 << 20, b'x');
        let file = wav(
            44_100,
            &[
                riff_chunk(b"LIST", &info),
                cue_points(&[(1, 44_100)]),
                adtl(&[label(1, "Verse")]),
            ],
        );
        assert_eq!(read(file), Ok(vec![("Verse".into(), at(1.0))]));

        let long_name = "x".repeat(600 << 10);
        let file = wav(
            44_100,
            &[
                cue_points(&[(1, 0)]),
                adtl(&[label(1, &long_name)]),
                adtl(&[label(1, &long_name)]),
            ],
        );
        assert_eq!(
            read(file).unwrap_err(),
            "damaged WAV file: its adtl lists hold over 1 MB, more than markers ever take"
        );
    }

    #[test]
    fn a_wav_without_cue_points_has_no_markers() {
        assert_eq!(read(wav(44_100, &[])), Ok(vec![]));
    }

    #[test]
    fn a_truncated_or_damaged_file_is_an_error_never_a_panic() {
        let whole = wav(44_100, &[cue_points(&[(1, 0)]), adtl(&[label(1, "A")])]);
        for length in 0..whole.len() {
            let error = read(whole[..length].to_vec()).unwrap_err();
            assert!(
                error.starts_with("not a WAV file") || error.starts_with("damaged WAV file"),
                "{length} bytes: {error}"
            );
        }
        let mut list_too_long = whole.clone();
        let list = whole.windows(4).position(|id| id == b"LIST").unwrap();
        list_too_long[list + 4..list + 8].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(
            read(list_too_long).unwrap_err(),
            "damaged WAV file: its \"LIST\" chunk runs past the end of the file; was it cut short?"
        );
        let huge_count = wav(44_100, &[riff_chunk(b"cue ", &u32::MAX.to_le_bytes())]);
        assert_eq!(
            read(huge_count).unwrap_err(),
            "damaged WAV file: its cue chunk lists 4294967295 cue points but doesn't hold them"
        );
        let mut no_format = whole.clone();
        no_format[12..16].copy_from_slice(b"fmtx");
        assert_eq!(
            read(no_format).unwrap_err(),
            "damaged WAV file: it has cue points but no fmt chunk giving its sample rate"
        );
        let mut broken_label = adtl(&[label(1, "A")]);
        // The labl's size, past the end of its list.
        broken_label[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        let file = wav(44_100, &[cue_points(&[(1, 0)]), broken_label]);
        assert_eq!(read(file), Ok(vec![("Marker 1".into(), at(0.0))]));
    }

    #[test]
    fn a_file_that_isnt_a_wav_is_refused() {
        for bytes in [&b"ID3\x04\0\0\0\0\0\0"[..], b"fLaC\0\0\0\x22", b""] {
            assert_eq!(
                read(bytes.to_vec()).unwrap_err(),
                "not a WAV file: it doesn't start with a RIFF/WAVE header"
            );
        }
    }
}
