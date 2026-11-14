//! Recordings as WAV bytes: what a measurement writes down and reads again.
//!
//! Written is always 32-bit float, the format pult mixes in. Read are 32-bit float and 16, 24 and
//! 32-bit integers (plain and `WAVE_FORMAT_EXTENSIBLE`), so a recording made with another program
//! can be judged too. Nothing here touches a file: bytes in, bytes out.

/// A recording: interleaved samples at one rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels per frame.
    pub channels: u16,
    /// The samples, interleaved, full scale at 1.
    pub samples: Vec<f32>,
}

const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xFFFE;

impl Wav {
    /// Its frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Its length in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate.max(1))
    }

    /// The recording as the bytes of a WAV file in 32-bit float.
    ///
    /// # Errors
    ///
    /// When it is longer than a WAV file can say (4 GiB).
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let data = self.samples.len() * 4;
        let too_long = || "the recording is too long for a WAV file".to_owned();
        let data_len = u32::try_from(data).map_err(|_| too_long())?;
        let riff_len = data_len.checked_add(36).ok_or_else(too_long)?;
        let block = self.channels.checked_mul(4).ok_or_else(too_long)?;
        let mut bytes = Vec::with_capacity(data + 44);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&riff_len.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&FLOAT.to_le_bytes());
        bytes.extend_from_slice(&self.channels.to_le_bytes());
        bytes.extend_from_slice(&self.sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(self.sample_rate.saturating_mul(u32::from(block))).to_le_bytes());
        bytes.extend_from_slice(&block.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in &self.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(bytes)
    }

    /// Reads the bytes of a WAV file.
    ///
    /// # Errors
    ///
    /// Says what about the bytes is no WAV file this reads: no RIFF/WAVE, no format or no data,
    /// a format other than float 32 or integer 16, 24 or 32, no channels or no rate.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err("not a WAV file (no RIFF/WAVE at its start)".to_owned());
        }
        let mut format: Option<(u16, u16, u32, u16)> = None;
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = u32_at(bytes, at + 4) as usize;
            let body = at + 8;
            // A data chunk a recorder did not close says more than there is: take what is there.
            let end = body.saturating_add(len).min(bytes.len());
            let chunk = &bytes[body..end];
            if id == b"fmt " {
                if chunk.len() < 16 {
                    return Err("the format chunk is too short".to_owned());
                }
                let mut tag = u16_at(chunk, 0);
                if tag == EXTENSIBLE && chunk.len() >= 26 {
                    // The real format is the start of the sub-format GUID.
                    tag = u16_at(chunk, 24);
                }
                format = Some((tag, u16_at(chunk, 2), u32_at(chunk, 4), u16_at(chunk, 14)));
            } else if id == b"data" {
                let (tag, channels, sample_rate, bits) =
                    format.ok_or("the data comes before the format")?;
                if channels == 0 || sample_rate == 0 {
                    return Err("no channels or no rate".to_owned());
                }
                let samples = samples_of(chunk, tag, bits)?;
                let whole = samples.len() - samples.len() % usize::from(channels);
                return Ok(Self {
                    sample_rate,
                    channels,
                    samples: samples[..whole].to_vec(),
                });
            }
            // Chunks are padded to an even length.
            at = body.saturating_add(len).saturating_add(len % 2);
        }
        Err("no data in the WAV file".to_owned())
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The samples of a data chunk in the format `tag` with `bits` per sample.
fn samples_of(chunk: &[u8], tag: u16, bits: u16) -> Result<Vec<f32>, String> {
    #[allow(clippy::cast_precision_loss)]
    let scaled = |value: i32, full: f32| value as f32 / full;
    match (tag, bits) {
        (FLOAT, 32) => Ok(chunk
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect()),
        (PCM, 16) => Ok(chunk
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| scaled(i32::from(i16::from_le_bytes(b)), 32_768.0))
            .collect()),
        (PCM, 24) => Ok(chunk
            .as_chunks::<3>()
            .0
            .iter()
            // Shifted up and down again, so the sign of the top byte carries.
            .map(|b| scaled(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8, 8_388_608.0))
            .collect()),
        (PCM, 32) => Ok(chunk
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| scaled(i32::from_le_bytes(b), 2_147_483_648.0))
            .collect()),
        _ => Err(format!(
            "format {tag} with {bits} bits is not read (float 32, or integer 16, 24, 32)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_reads_back_as_it_was_written() {
        let wav = Wav {
            sample_rate: 44_100,
            channels: 2,
            samples: vec![0.0, 1.0, -1.0, 0.25, 1e-6, -0.5],
        };
        let bytes = wav.encode().expect("fits");
        assert_eq!(bytes.len(), 44 + 6 * 4);
        assert_eq!(Wav::decode(&bytes), Ok(wav.clone()));
        assert_eq!(wav.frames(), 3);
        assert!((wav.seconds() - 3.0 / 44_100.0).abs() < 1e-12);
    }

    /// A WAV file with one format chunk of `fmt` and the data `data`, with a chunk between them
    /// that a reader has to step over (of odd length, so it is padded).
    fn file(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        bytes.extend_from_slice(&u32::try_from(fmt.len()).expect("short").to_le_bytes());
        bytes.extend_from_slice(fmt);
        bytes.extend_from_slice(b"LIST\x03\0\0\0abc\0data");
        bytes.extend_from_slice(&u32::try_from(data.len()).expect("short").to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    fn fmt(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes
    }

    #[test]
    fn integers_read_at_their_scale_with_their_sign() {
        let pcm16 = file(&fmt(PCM, 1, 48_000, 16), &[0, 0x40, 0, 0xC0, 0xFF, 0x7F]);
        let wav = Wav::decode(&pcm16).expect("reads");
        assert_eq!((wav.sample_rate, wav.channels), (48_000, 1));
        assert_eq!(wav.samples[..2], [0.5, -0.5]);
        assert!((wav.samples[2] - 1.0).abs() < 1e-4);
        let pcm24 = file(&fmt(PCM, 1, 48_000, 24), &[0, 0, 0x40, 0, 0, 0xC0]);
        assert_eq!(Wav::decode(&pcm24).expect("reads").samples, [0.5, -0.5]);
        let pcm32 = file(&fmt(PCM, 1, 48_000, 32), &[0, 0, 0, 0x40, 0, 0, 0, 0xC0]);
        assert_eq!(Wav::decode(&pcm32).expect("reads").samples, [0.5, -0.5]);
    }

    #[test]
    fn an_extensible_format_reads_by_its_sub_format() {
        let mut extensible = fmt(EXTENSIBLE, 2, 48_000, 32);
        // cbSize, valid bits, channel mask, then the GUID that starts with the real format.
        extensible.extend_from_slice(&22_u16.to_le_bytes());
        extensible.extend_from_slice(&32_u16.to_le_bytes());
        extensible.extend_from_slice(&3_u32.to_le_bytes());
        extensible.extend_from_slice(&FLOAT.to_le_bytes());
        extensible.extend_from_slice(&[0; 14]);
        let mut data = Vec::new();
        for sample in [0.5_f32, -0.25, 0.125] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        // The third sample is half a frame: dropped.
        let wav = Wav::decode(&file(&extensible, &data)).expect("reads");
        assert_eq!(wav.samples, [0.5, -0.25]);
    }

    #[test]
    fn what_is_no_recording_is_refused_with_a_reason() {
        assert!(Wav::decode(b"nothing").unwrap_err().contains("RIFF"));
        let mut no_data = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        assert!(Wav::decode(&no_data).unwrap_err().contains("too short"));
        no_data.extend_from_slice(&fmt(FLOAT, 1, 48_000, 32));
        assert!(Wav::decode(&no_data).unwrap_err().contains("no data"));
        let alaw = file(&fmt(6, 1, 8000, 8), &[1, 2, 3]);
        assert!(Wav::decode(&alaw).unwrap_err().contains("format 6"));
        let no_rate = file(&fmt(FLOAT, 1, 0, 32), &[0; 4]);
        assert!(Wav::decode(&no_rate).unwrap_err().contains("rate"));
        let early = b"RIFF\0\0\0\0WAVEdata\x04\0\0\0\0\0\0\0".to_vec();
        assert!(Wav::decode(&early).unwrap_err().contains("before"));
        // A data chunk that says more than there is reads as what is there.
        let mut open = file(&fmt(FLOAT, 1, 48_000, 32), &0.5_f32.to_le_bytes());
        let len = open.len();
        open[len - 8..len - 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Wav::decode(&open).expect("reads").samples, [0.5]);
    }
}
