use thiserror::Error;

const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BOXES: usize = 16_384;
const MAX_DEPTH: usize = 8;
const MAX_TABLE_ENTRIES: u32 = 2_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoMetadata {
    pub container: &'static str,
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub frame_count: u64,
    pub codec: Option<String>,
    pub validation_level: &'static str,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("invalid MP4: {0}")]
    Invalid(String),
}

#[derive(Default)]
struct Track {
    video: bool,
    width: u32,
    height: u32,
    timescale: u32,
    duration: u64,
    samples: u64,
    sample_bytes: u64,
    timing_samples: u64,
    timing_duration: u64,
    max_sample_delta: u64,
    codec: Option<String>,
    sample_width: u32,
    sample_height: u32,
    sample_sizes: Vec<u32>,
    sample_to_chunks: Vec<SampleToChunk>,
    chunk_offsets: Vec<u64>,
}

#[derive(Clone, Copy, Default)]
struct SampleToChunk {
    first_chunk: u32,
    samples_per_chunk: u32,
}

#[derive(Default)]
struct Scan {
    boxes: usize,
    ftyp: bool,
    moov: bool,
    mdat_extents: Vec<(u64, u64)>,
    tracks: Vec<Track>,
}

pub fn validate_mp4(bytes: &[u8]) -> Result<VideoMetadata, ValidationError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ValidationError::Invalid(
            "file is empty or exceeds 256 MiB".into(),
        ));
    }
    let mut scan = Scan::default();
    parse_boxes(bytes, 0, 0, &mut scan, None)?;
    if !scan.ftyp || !scan.moov || scan.mdat_extents.is_empty() {
        return Err(ValidationError::Invalid(
            "ftyp, moov, and non-empty mdat are required".into(),
        ));
    }
    let track = scan
        .tracks
        .into_iter()
        .find(|track| track.video)
        .ok_or_else(|| ValidationError::Invalid("declared video track is required".into()))?;
    if track.width == 0
        || track.height == 0
        || track.timescale == 0
        || track.duration == 0
        || track.samples == 0
        || track.sample_bytes == 0
        || track.timing_samples != track.samples
        || track.timing_duration == 0
        || track.codec.is_none()
        || track.sample_width != track.width
        || track.sample_height != track.height
    {
        return Err(ValidationError::Invalid(
            "video metadata or sample tables are incomplete".into(),
        ));
    }
    validate_chunks(&track, &scan.mdat_extents, bytes.len() as u64)?;
    let duration_difference = track.duration.abs_diff(track.timing_duration);
    let duration_tolerance = track.max_sample_delta.max(track.duration / 100).max(1);
    if duration_difference > duration_tolerance {
        return Err(ValidationError::Invalid(
            "mdhd and sample timing durations disagree".into(),
        ));
    }
    let duration_ms = track
        .duration
        .checked_mul(1000)
        .map(|value| value / u64::from(track.timescale))
        .filter(|value| *value > 0)
        .ok_or_else(|| ValidationError::Invalid("invalid video duration".into()))?;
    let fps_num64 = track
        .timing_samples
        .checked_mul(u64::from(track.timescale))
        .ok_or_else(|| ValidationError::Invalid("frame timing overflow".into()))?;
    let divisor = gcd(fps_num64, track.timing_duration);
    let fps_numerator = u32::try_from(fps_num64 / divisor)
        .map_err(|_| ValidationError::Invalid("frame rate is out of range".into()))?;
    let fps_denominator = u32::try_from(track.timing_duration / divisor)
        .map_err(|_| ValidationError::Invalid("frame rate is out of range".into()))?;
    if fps_numerator == 0
        || fps_denominator == 0
        || u64::from(fps_numerator) > 240 * u64::from(fps_denominator)
    {
        return Err(ValidationError::Invalid(
            "frame rate is out of range".into(),
        ));
    }
    Ok(VideoMetadata {
        container: "MP4",
        format: "MP4",
        width: track.width,
        height: track.height,
        duration_ms,
        fps_numerator,
        fps_denominator,
        frame_count: track.samples,
        codec: track.codec,
        validation_level: "structural",
    })
}

fn parse_boxes(
    data: &[u8],
    depth: usize,
    base_offset: u64,
    scan: &mut Scan,
    mut track: Option<&mut Track>,
) -> Result<(), ValidationError> {
    if depth > MAX_DEPTH {
        return Err(ValidationError::Invalid("box nesting is too deep".into()));
    }
    let mut offset = 0usize;
    while offset < data.len() {
        scan.boxes += 1;
        if scan.boxes > MAX_BOXES || data.len() - offset < 8 {
            return Err(ValidationError::Invalid(
                "truncated or excessive boxes".into(),
            ));
        }
        let size32 = be_u32(&data[offset..offset + 4]) as u64;
        let kind = &data[offset + 4..offset + 8];
        let (size, header) = if size32 == 1 {
            if data.len() - offset < 16 {
                return Err(ValidationError::Invalid("truncated extended box".into()));
            }
            (be_u64(&data[offset + 8..offset + 16]), 16usize)
        } else if size32 == 0 {
            ((data.len() - offset) as u64, 8usize)
        } else {
            (size32, 8usize)
        };
        if size < header as u64 || size > (data.len() - offset) as u64 {
            return Err(ValidationError::Invalid("box bounds exceed file".into()));
        }
        let end = offset + size as usize;
        let content = &data[offset + header..end];
        match kind {
            b"ftyp" if depth == 0 => {
                if content.len() < 8 {
                    return Err(ValidationError::Invalid("invalid ftyp".into()));
                }
                scan.ftyp = true;
            }
            b"moov" if depth == 0 => {
                scan.moov = true;
                parse_boxes(
                    content,
                    depth + 1,
                    base_offset + offset as u64 + header as u64,
                    scan,
                    None,
                )?;
            }
            b"trak" => {
                let mut value = Track::default();
                parse_boxes(
                    content,
                    depth + 1,
                    base_offset + offset as u64 + header as u64,
                    scan,
                    Some(&mut value),
                )?;
                scan.tracks.push(value);
            }
            b"mdia" | b"minf" | b"stbl" => parse_boxes(
                content,
                depth + 1,
                base_offset + offset as u64 + header as u64,
                scan,
                track.as_deref_mut(),
            )?,
            b"mdat" if depth == 0 => {
                let start = base_offset
                    .checked_add(offset as u64)
                    .and_then(|value| value.checked_add(header as u64))
                    .ok_or_else(|| ValidationError::Invalid("media offset overflow".into()))?;
                let end = start
                    .checked_add(content.len() as u64)
                    .ok_or_else(|| ValidationError::Invalid("media offset overflow".into()))?;
                if start == end {
                    return Err(ValidationError::Invalid("empty mdat".into()));
                }
                scan.mdat_extents.push((start, end));
            }
            b"tkhd" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_tkhd(content, value)?;
                }
            }
            b"mdhd" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_mdhd(content, value)?;
                }
            }
            b"hdlr" => {
                if let Some(value) = track.as_deref_mut()
                    && content.len() >= 12
                    && &content[8..12] == b"vide"
                {
                    value.video = true;
                }
            }
            b"stts" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_stts(content, value)?;
                }
            }
            b"stsz" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_stsz(content, value)?;
                }
            }
            b"stsd" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_stsd(content, value)?;
                }
            }
            b"stsc" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_stsc(content, value)?;
                }
            }
            b"stco" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_chunk_offsets(content, value, false)?;
                }
            }
            b"co64" => {
                if let Some(value) = track.as_deref_mut() {
                    parse_chunk_offsets(content, value, true)?;
                }
            }
            _ => {}
        }
        offset = end;
    }
    Ok(())
}

fn parse_tkhd(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    let offset = if data.first() == Some(&1) { 88 } else { 76 };
    if data.len() < offset + 8 {
        return Err(ValidationError::Invalid("truncated tkhd".into()));
    }
    track.width = be_u32(&data[offset..offset + 4]) >> 16;
    track.height = be_u32(&data[offset + 4..offset + 8]) >> 16;
    Ok(())
}

fn parse_mdhd(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    let (scale, duration) = if data.first() == Some(&1) {
        if data.len() < 32 {
            return Err(ValidationError::Invalid("truncated mdhd".into()));
        }
        (be_u32(&data[20..24]), be_u64(&data[24..32]))
    } else {
        if data.len() < 20 {
            return Err(ValidationError::Invalid("truncated mdhd".into()));
        }
        (be_u32(&data[12..16]), u64::from(be_u32(&data[16..20])))
    };
    track.timescale = scale;
    track.duration = duration;
    Ok(())
}

fn parse_stts(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    if data.len() < 8 {
        return Err(ValidationError::Invalid("truncated stts".into()));
    }
    let count = be_u32(&data[4..8]);
    if count == 0 || count > MAX_TABLE_ENTRIES || data.len() < 8 + count as usize * 8 {
        return Err(ValidationError::Invalid("invalid stts table".into()));
    }
    for entry in data[8..8 + count as usize * 8].chunks_exact(8) {
        let samples = u64::from(be_u32(&entry[..4]));
        let delta = u64::from(be_u32(&entry[4..]));
        if samples == 0 || delta == 0 {
            return Err(ValidationError::Invalid("zero frame timing".into()));
        }
        track.timing_samples = track
            .timing_samples
            .checked_add(samples)
            .ok_or_else(|| ValidationError::Invalid("timing overflow".into()))?;
        track.timing_duration = track
            .timing_duration
            .checked_add(
                samples
                    .checked_mul(delta)
                    .ok_or_else(|| ValidationError::Invalid("timing overflow".into()))?,
            )
            .ok_or_else(|| ValidationError::Invalid("timing overflow".into()))?;
        track.max_sample_delta = track.max_sample_delta.max(delta);
    }
    Ok(())
}

fn parse_stsz(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    if data.len() < 12 {
        return Err(ValidationError::Invalid("truncated stsz".into()));
    }
    let default_size = be_u32(&data[4..8]);
    let count = be_u32(&data[8..12]);
    if count == 0 || count > MAX_TABLE_ENTRIES {
        return Err(ValidationError::Invalid("invalid sample count".into()));
    }
    track.samples = u64::from(count);
    track.sample_sizes = if default_size != 0 {
        vec![default_size; count as usize]
    } else {
        if data.len() < 12 + count as usize * 4 {
            return Err(ValidationError::Invalid("truncated sample sizes".into()));
        }
        data[12..12 + count as usize * 4]
            .chunks_exact(4)
            .map(be_u32)
            .collect()
    };
    if track.sample_sizes.contains(&0) {
        return Err(ValidationError::Invalid("zero video sample".into()));
    }
    track.sample_bytes = track.sample_sizes.iter().try_fold(0u64, |sum, size| {
        sum.checked_add(u64::from(*size))
            .ok_or_else(|| ValidationError::Invalid("sample size overflow".into()))
    })?;
    Ok(())
}

fn parse_stsd(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    if data.len() < 94 || be_u32(&data[4..8]) != 1 {
        return Err(ValidationError::Invalid("invalid stsd".into()));
    }
    let size = be_u32(&data[8..12]) as usize;
    if size < 86 || size > data.len() - 8 {
        return Err(ValidationError::Invalid(
            "invalid sample description".into(),
        ));
    }
    let codec = &data[12..16];
    if !matches!(
        codec,
        b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"av01" | b"vp09" | b"mp4v"
    ) {
        return Err(ValidationError::Invalid(
            "unrecognized visual sample entry".into(),
        ));
    }
    let entry_width = u32::from(u16::from_be_bytes(data[40..42].try_into().unwrap()));
    let entry_height = u32::from(u16::from_be_bytes(data[42..44].try_into().unwrap()));
    if entry_width == 0 || entry_height == 0 {
        return Err(ValidationError::Invalid(
            "visual sample entry dimensions are invalid".into(),
        ));
    }
    track.codec = Some(String::from_utf8_lossy(codec).into_owned());
    track.sample_width = entry_width;
    track.sample_height = entry_height;
    Ok(())
}

fn parse_stsc(data: &[u8], track: &mut Track) -> Result<(), ValidationError> {
    if data.len() < 8 {
        return Err(ValidationError::Invalid("truncated stsc".into()));
    }
    let count = be_u32(&data[4..8]);
    let table_bytes = (count as usize)
        .checked_mul(12)
        .and_then(|value| value.checked_add(8))
        .ok_or_else(|| ValidationError::Invalid("stsc size overflow".into()))?;
    if count == 0 || count > MAX_TABLE_ENTRIES || data.len() < table_bytes {
        return Err(ValidationError::Invalid("invalid stsc table".into()));
    }
    let mut previous = 0;
    for entry in data[8..table_bytes].chunks_exact(12) {
        let value = SampleToChunk {
            first_chunk: be_u32(&entry[..4]),
            samples_per_chunk: be_u32(&entry[4..8]),
        };
        if value.first_chunk <= previous
            || value.samples_per_chunk == 0
            || be_u32(&entry[8..12]) != 1
        {
            return Err(ValidationError::Invalid("invalid stsc mapping".into()));
        }
        previous = value.first_chunk;
        track.sample_to_chunks.push(value);
    }
    if track.sample_to_chunks[0].first_chunk != 1 {
        return Err(ValidationError::Invalid(
            "stsc must begin at chunk one".into(),
        ));
    }
    Ok(())
}

fn parse_chunk_offsets(data: &[u8], track: &mut Track, wide: bool) -> Result<(), ValidationError> {
    if data.len() < 8 || !track.chunk_offsets.is_empty() {
        return Err(ValidationError::Invalid(
            "missing or duplicate chunk offsets".into(),
        ));
    }
    let count = be_u32(&data[4..8]);
    let width = if wide { 8 } else { 4 };
    let table_bytes = (count as usize)
        .checked_mul(width)
        .and_then(|value| value.checked_add(8))
        .ok_or_else(|| ValidationError::Invalid("chunk table size overflow".into()))?;
    if count == 0 || count > MAX_TABLE_ENTRIES || data.len() < table_bytes {
        return Err(ValidationError::Invalid(
            "invalid chunk offset table".into(),
        ));
    }
    track.chunk_offsets = data[8..table_bytes]
        .chunks_exact(width)
        .map(|entry| {
            if wide {
                be_u64(entry)
            } else {
                u64::from(be_u32(entry))
            }
        })
        .collect();
    Ok(())
}

fn validate_chunks(
    track: &Track,
    mdats: &[(u64, u64)],
    file_len: u64,
) -> Result<(), ValidationError> {
    if track.sample_to_chunks.is_empty() || track.chunk_offsets.is_empty() {
        return Err(ValidationError::Invalid(
            "stsc and stco or co64 are required".into(),
        ));
    }
    let mut sample = 0usize;
    let mut ranges = Vec::with_capacity(track.chunk_offsets.len());
    for (chunk_index, offset) in track.chunk_offsets.iter().enumerate() {
        let number = u32::try_from(chunk_index + 1)
            .map_err(|_| ValidationError::Invalid("too many chunks".into()))?;
        let mapping = track
            .sample_to_chunks
            .iter()
            .rev()
            .find(|entry| entry.first_chunk <= number)
            .ok_or_else(|| ValidationError::Invalid("unmapped chunk".into()))?;
        let end_sample = sample
            .checked_add(mapping.samples_per_chunk as usize)
            .ok_or_else(|| ValidationError::Invalid("sample mapping overflow".into()))?;
        if end_sample > track.sample_sizes.len() {
            return Err(ValidationError::Invalid(
                "chunk mapping exceeds samples".into(),
            ));
        }
        let chunk_bytes =
            track.sample_sizes[sample..end_sample]
                .iter()
                .try_fold(0u64, |sum, size| {
                    sum.checked_add(u64::from(*size))
                        .ok_or_else(|| ValidationError::Invalid("chunk size overflow".into()))
                })?;
        let end = offset
            .checked_add(chunk_bytes)
            .ok_or_else(|| ValidationError::Invalid("chunk range overflow".into()))?;
        if end > file_len
            || !mdats
                .iter()
                .any(|(start, mdat_end)| *offset >= *start && end <= *mdat_end)
        {
            return Err(ValidationError::Invalid(
                "chunk bytes are outside mdat".into(),
            ));
        }
        ranges.push((*offset, end));
        sample = end_sample;
    }
    if sample != track.sample_sizes.len() {
        return Err(ValidationError::Invalid(
            "not all samples are mapped to chunks".into(),
        ));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(ValidationError::Invalid("chunk byte ranges overlap".into()));
    }
    Ok(())
}

fn be_u32(value: &[u8]) -> u32 {
    u32::from_be_bytes(value[..4].try_into().unwrap())
}
fn be_u64(value: &[u8]) -> u64 {
    u64::from_be_bytes(value[..8].try_into().unwrap())
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a.max(1)
}

#[cfg(test)]
pub(crate) fn structural_fixture() -> Vec<u8> {
    structural_fixture_for(320, 192, 9, 8)
}

#[cfg(test)]
pub(crate) fn structural_fixture_for(width: u16, height: u16, frames: u32, fps: u32) -> Vec<u8> {
    fn bx(kind: &[u8; 4], content: Vec<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&((content.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend(content);
        out
    }
    let ftyp = bx(
        b"ftyp",
        [b"isom".as_slice(), &[0, 0, 0, 0], b"isom"].concat(),
    );
    let mut tkhd = vec![0; 84];
    tkhd[76..80].copy_from_slice(&(u32::from(width) << 16).to_be_bytes());
    tkhd[80..84].copy_from_slice(&(u32::from(height) << 16).to_be_bytes());
    let mut mdhd = vec![0; 20];
    let timescale = fps * 1000;
    let duration = frames * 1000;
    mdhd[12..16].copy_from_slice(&timescale.to_be_bytes());
    mdhd[16..20].copy_from_slice(&duration.to_be_bytes());
    let mut hdlr = vec![0; 12];
    hdlr[8..12].copy_from_slice(b"vide");
    let stts = bx(
        b"stts",
        [
            vec![0; 4],
            1u32.to_be_bytes().to_vec(),
            frames.to_be_bytes().to_vec(),
            1000u32.to_be_bytes().to_vec(),
        ]
        .concat(),
    );
    let stsz = bx(
        b"stsz",
        [
            vec![0; 4],
            1u32.to_be_bytes().to_vec(),
            frames.to_be_bytes().to_vec(),
        ]
        .concat(),
    );
    let mut visual_entry = vec![0; 86];
    visual_entry[..4].copy_from_slice(&86u32.to_be_bytes());
    visual_entry[4..8].copy_from_slice(b"avc1");
    visual_entry[14..16].copy_from_slice(&1u16.to_be_bytes());
    visual_entry[32..34].copy_from_slice(&width.to_be_bytes());
    visual_entry[34..36].copy_from_slice(&height.to_be_bytes());
    let stsd = bx(
        b"stsd",
        [vec![0; 4], 1u32.to_be_bytes().to_vec(), visual_entry].concat(),
    );
    let stsc = bx(
        b"stsc",
        [
            vec![0; 4],
            1u32.to_be_bytes().to_vec(),
            1u32.to_be_bytes().to_vec(),
            frames.to_be_bytes().to_vec(),
            1u32.to_be_bytes().to_vec(),
        ]
        .concat(),
    );
    let make_moov = |chunk_offset: u32| {
        let stco = bx(
            b"stco",
            [
                vec![0; 4],
                1u32.to_be_bytes().to_vec(),
                chunk_offset.to_be_bytes().to_vec(),
            ]
            .concat(),
        );
        let stbl = bx(
            b"stbl",
            [stts.clone(), stsz.clone(), stsd.clone(), stsc.clone(), stco].concat(),
        );
        let mdia = bx(
            b"mdia",
            [
                bx(b"mdhd", mdhd.clone()),
                bx(b"hdlr", hdlr.clone()),
                bx(b"minf", stbl),
            ]
            .concat(),
        );
        bx(
            b"moov",
            bx(b"trak", [bx(b"tkhd", tkhd.clone()), mdia].concat()),
        )
    };
    let placeholder = make_moov(0);
    let mdat_offset = u32::try_from(ftyp.len() + placeholder.len() + 8).unwrap();
    let moov = make_moov(mdat_offset);
    [ftyp, moov, bx(b"mdat", vec![1; frames as usize])].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_structural_fixture_without_claiming_decode() {
        let value = validate_mp4(&structural_fixture()).unwrap();
        assert_eq!(
            (
                value.width,
                value.height,
                value.frame_count,
                value.fps_numerator,
                value.fps_denominator,
                value.validation_level
            ),
            (320, 192, 9, 8, 1, "structural")
        );
    }
    #[test]
    fn rejects_empty_corrupt_and_truncated() {
        for value in [
            Vec::new(),
            b"not mp4".to_vec(),
            structural_fixture()[..40].to_vec(),
        ] {
            assert!(validate_mp4(&value).is_err());
        }
    }
    #[test]
    fn rejects_chunk_offset_outside_mdat_and_missing_chunk_mapping() {
        let mut outside = structural_fixture();
        let position = outside
            .windows(4)
            .position(|value| value == b"stco")
            .unwrap();
        outside[position + 12..position + 16].copy_from_slice(&1u32.to_be_bytes());
        assert!(validate_mp4(&outside).is_err());

        let mut missing = structural_fixture();
        let position = missing
            .windows(4)
            .position(|value| value == b"stsc")
            .unwrap();
        missing[position..position + 4].copy_from_slice(b"free");
        assert!(validate_mp4(&missing).is_err());
    }
}
