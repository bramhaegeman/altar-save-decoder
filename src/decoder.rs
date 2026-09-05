use std::fmt::Write as _;
use std::path::Path;

const BLOCK_SIZE: usize = 131_072; // 128 KiB — Oblivion Remastered's Oodle chunk size.
const HEADER_SCAN_WINDOW: usize = 512; // bytes to search forward for the next chunk header.
const MAX_COMPRESSED_SEARCH: usize = 300_000; // generous upper bound for one chunk's compressed span.
const SHRINK_SIZES: [usize; 9] = [65536, 32768, 16384, 8192, 4096, 2048, 1024, 512, 256];

pub struct DecodeReport {
    pub chunks_decoded: usize,
    pub decoded_bytes: usize,
    pub source_bytes: usize,
    #[allow(dead_code)] // kept for diagnostics / future use
    pub bytes_consumed: usize,
    pub tail_bytes_left_raw: usize,
}

fn try_decode(input: &[u8], out_size: usize) -> Option<usize> {
    let input = input.to_vec();
    let result = std::panic::catch_unwind(move || {
        let mut extractor = oozextract::Extractor::new();
        let mut output = vec![0u8; out_size];
        extractor.read_from_slice(&input, &mut output).ok().map(|_| ())
    });
    result.ok().flatten().map(|_| out_size)
}

fn find_min_compressed_len(data: &[u8], start: usize, out_size: usize, max_len: usize) -> Option<usize> {
    let upper_bound = max_len.min(data.len().saturating_sub(start));
    if upper_bound == 0 {
        return None;
    }
    try_decode(&data[start..start + upper_bound], out_size)?;

    let mut lo = 1usize;
    let mut hi = upper_bound;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if try_decode(&data[start..start + mid], out_size).is_some() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

fn decode_exact(input: &[u8], out_size: usize) -> Option<Vec<u8>> {
    let input = input.to_vec();
    std::panic::catch_unwind(move || {
        let mut extractor = oozextract::Extractor::new();
        let mut output = vec![0u8; out_size];
        extractor.read_from_slice(&input, &mut output).ok().map(|_| output)
    })
    .ok()
    .flatten()
}

/// Walks the Oodle chunk chain inside a raw byte blob (e.g. the GVAS `AltarSaveData`
/// property's bytes, already extracted from the outer save file), decoding every
/// 128KB-block chunk it can find — however many hours a save has racked up, this loops
/// until the data runs out rather than assuming a fixed chunk count.
pub fn decode_chunk_chain(data: &[u8], progress: &mut impl FnMut(usize, usize)) -> (Vec<u8>, DecodeReport) {
    std::panic::set_hook(Box::new(|_| {}));

    let mut offset = 0usize;
    let mut chunk_idx = 0usize;
    let mut output = Vec::with_capacity(data.len() * 3);

    // The very first chunk's header starts at a small fixed offset in this game's format.
    if data.len() > 49 {
        offset = 49;
    }

    loop {
        if offset >= data.len() {
            break;
        }
        let remaining = data.len() - offset;
        let max_search = MAX_COMPRESSED_SEARCH.min(remaining);

        let mut found: Option<(usize, usize, usize)> = None;
        'search: for probe_offset in offset..(offset + HEADER_SCAN_WINDOW).min(data.len()) {
            if let Some(n) = find_min_compressed_len(data, probe_offset, BLOCK_SIZE, max_search) {
                found = Some((probe_offset, n, BLOCK_SIZE));
                break 'search;
            }
        }

        if found.is_none() {
            'outer: for probe_offset in offset..(offset + HEADER_SCAN_WINDOW).min(data.len()) {
                for &shrink in &SHRINK_SIZES {
                    if let Some(n) = find_min_compressed_len(data, probe_offset, shrink, max_search) {
                        found = Some((probe_offset, n, shrink));
                        break 'outer;
                    }
                }
            }
        }

        let (actual_offset, compressed_len, out_size) = match found {
            Some(v) => v,
            None => break,
        };

        let decoded = match decode_exact(&data[actual_offset..actual_offset + compressed_len], out_size) {
            Some(d) => d,
            None => break,
        };

        output.extend_from_slice(&decoded);
        offset = actual_offset + compressed_len;
        chunk_idx += 1;
        progress(offset, data.len());
    }

    let report = DecodeReport {
        chunks_decoded: chunk_idx,
        decoded_bytes: output.len(),
        source_bytes: data.len(),
        bytes_consumed: offset,
        tail_bytes_left_raw: data.len().saturating_sub(offset),
    };
    (output, report)
}

/// Formats bytes as a classic `offset | hex | ascii` hex dump, the way modders expect to
/// read raw structures (16 bytes per row).
pub fn to_hex_dump(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 4);
    for (row, chunk) in data.chunks(16).enumerate() {
        let offset = row * 16;
        let _ = write!(out, "{:08X}  ", offset);
        for i in 0..16 {
            if i < chunk.len() {
                let _ = write!(out, "{:02X} ", chunk[i]);
            } else {
                out.push_str("   ");
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for &b in chunk {
            out.push(if (32..=126).contains(&b) { b as char } else { '.' });
        }
        out.push_str("|\n");
    }
    out
}

/// Extracts the raw `AltarSaveData` bytes from the outer GVAS save file using the `gvas`
/// crate's proper Unreal tagged-property parser (robust to whatever else the wrapper
/// contains, rather than assuming AltarSaveData is the last thing before EOF).
pub fn extract_altar_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    use gvas::game_version::GameVersion;
    use gvas::properties::array_property::ArrayProperty;
    use gvas::properties::Property;
    use gvas::GvasFile;

    let mut f = std::fs::File::open(path)?;
    let gvas_file = GvasFile::read(&mut f, GameVersion::Default)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e)))?;

    for (_, prop) in gvas_file.properties.iter() {
        if let Property::ArrayProperty(ArrayProperty::Bytes { bytes }) = prop {
            return Ok(bytes.clone());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "No byte-array property found in this save's GVAS wrapper — is this an Oblivion Remastered .sav file?",
    ))
}
