//! Tiny audio helpers for the TV Assistant voice channel: decode a WAV blob
//! (what the TTS endpoint returns) and resample it to the exact shape the
//! Android TV Remote voice protocol wants — **8 kHz, mono, 16-bit LE PCM**.
//!
//! Deliberately dependency-free (no `hound`/`symphonia`): the TTS server emits
//! plain PCM WAV, so a minimal RIFF reader + a linear resampler is enough, and
//! it stays trivially unit-testable.

use anyhow::{Result, bail};

/// The sample rate the ATV `RemoteVoicePayload` protocol specifies.
pub const ATV_VOICE_RATE: u32 = 8000;

/// A decoded PCM buffer: interleaved `i16` samples plus its format.
struct Pcm {
    samples: Vec<i16>,
    rate: u32,
    channels: u16,
}

/// The parsed `fmt ` chunk fields we care about.
struct WavFormat {
    audio_format: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

/// Read a little-endian `u32` at `off`.
fn u32_le(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn u16_le(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

/// Parse a PCM WAV blob into interleaved `i16` samples. Handles 16-bit integer
/// PCM (format 1) and 32-bit float (format 3) — the two shapes TTS servers
/// emit — and walks the chunk list rather than assuming a fixed 44-byte header
/// (some encoders insert `LIST`/`fact` chunks before `data`).
fn parse_wav(bytes: &[u8]) -> Result<Pcm> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut fmt: Option<WavFormat> = None;
    let mut data: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_le(bytes, pos + 4).unwrap_or(0) as usize;
        let body_at = pos + 8;
        let body = bytes.get(body_at..body_at + size);
        match (id, body) {
            (b"fmt ", Some(b)) if b.len() >= 16 => {
                fmt = Some(WavFormat {
                    audio_format: u16_le(b, 0).unwrap_or(1),
                    channels: u16_le(b, 2).unwrap_or(1),
                    rate: u32_le(b, 4).unwrap_or(0),
                    bits: u16_le(b, 14).unwrap_or(0),
                });
            }
            (b"data", Some(b)) => data = Some(b),
            _ => {}
        }
        // Chunks are word-aligned: an odd size carries a pad byte.
        pos = body_at + size + (size & 1);
    }
    let WavFormat {
        audio_format,
        channels,
        rate,
        bits,
    } = fmt.ok_or_else(|| anyhow::anyhow!("no fmt chunk"))?;
    let data = data.ok_or_else(|| anyhow::anyhow!("no data chunk"))?;
    if channels == 0 || rate == 0 {
        bail!("degenerate WAV format ({channels} ch @ {rate} Hz)");
    }
    let samples: Vec<i16> = match (audio_format, bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| {
                let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect(),
        (fmt, bits) => bail!("unsupported WAV sample format (audio_format={fmt}, bits={bits})"),
    };
    Ok(Pcm {
        samples,
        rate,
        channels,
    })
}

/// Downmix interleaved samples to mono by averaging channels.
fn to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    samples
        .chunks_exact(ch)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / ch as i32) as i16
        })
        .collect()
}

/// Linear-interpolation resample of a mono signal from `from_rate` to `to_rate`.
/// Good enough for speech into a voice-assistant (which re-features the audio
/// anyway); avoids pulling in a resampling crate.
fn resample_mono(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let j = src.floor() as usize;
        let frac = src - j as f64;
        let a = input[j] as f64;
        let b = *input.get(j + 1).unwrap_or(&input[j]) as f64;
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

/// Decode a TTS WAV blob and return **8 kHz mono 16-bit LE PCM bytes** ready
/// for `RemoteVoicePayload`. The one public seam.
pub fn wav_to_atv_voice_pcm(wav: &[u8]) -> Result<Vec<u8>> {
    let pcm = parse_wav(wav)?;
    let mono = to_mono(&pcm.samples, pcm.channels);
    let resampled = resample_mono(&mono, pcm.rate, ATV_VOICE_RATE);
    let mut bytes = Vec::with_capacity(resampled.len() * 2);
    for s in resampled {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 16-bit PCM WAV for `samples` at `rate`/`channels`.
    fn wav16(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let byte_rate = rate * channels as u32 * 2;
        let block_align = channels * 2;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&byte_rate.to_le_bytes());
        w.extend_from_slice(&block_align.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    #[test]
    fn passthrough_at_target_rate_is_lossless() {
        let samples = [0i16, 1000, -1000, 32000, -32000];
        let wav = wav16(&samples, 8000, 1);
        let pcm = wav_to_atv_voice_pcm(&wav).unwrap();
        let back: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(back, samples);
    }

    #[test]
    fn downsamples_24k_to_8k_by_a_third() {
        // 2400 samples @ 24kHz → ~800 @ 8kHz.
        let samples: Vec<i16> = (0..2400).map(|i| (i % 100) as i16).collect();
        let wav = wav16(&samples, 24000, 1);
        let pcm = wav_to_atv_voice_pcm(&wav).unwrap();
        let out_samples = pcm.len() / 2;
        assert!(
            (out_samples as i32 - 800).abs() <= 1,
            "expected ~800 samples, got {out_samples}"
        );
    }

    #[test]
    fn stereo_is_downmixed_to_mono() {
        // L=1000, R=-1000 per frame → mono 0.
        let stereo: Vec<i16> = (0..10).flat_map(|_| [1000i16, -1000]).collect();
        let wav = wav16(&stereo, 8000, 2);
        let pcm = wav_to_atv_voice_pcm(&wav).unwrap();
        let back: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(back.len(), 10, "stereo frames collapse to mono samples");
        assert!(back.iter().all(|&s| s == 0), "L+R averaged to silence");
    }

    #[test]
    fn float32_wav_is_decoded() {
        let mut w = Vec::new();
        let data: Vec<u8> = [0.0f32, 0.5, -0.5, 1.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&8000u32.to_le_bytes());
        w.extend_from_slice(&32000u32.to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes());
        w.extend_from_slice(&32u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        let pcm = wav_to_atv_voice_pcm(&w).unwrap();
        let back: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(back[0], 0);
        assert!(
            (back[1] - 16383).abs() <= 1,
            "0.5 → ~half scale: {}",
            back[1]
        );
        assert_eq!(back[3], i16::MAX);
    }

    #[test]
    fn a_non_wav_blob_errors() {
        assert!(wav_to_atv_voice_pcm(b"this is not audio").is_err());
    }

    #[test]
    fn skips_extra_chunks_before_data() {
        let samples = [5i16, 6, 7, 8];
        let mut wav = wav16(&samples, 8000, 1);
        // Splice a LIST chunk after the header (before data): find "data".
        let data_pos = wav.windows(4).position(|w| w == b"data").unwrap();
        let mut list = Vec::new();
        list.extend_from_slice(b"LIST");
        list.extend_from_slice(&4u32.to_le_bytes());
        list.extend_from_slice(b"INFO");
        wav.splice(data_pos..data_pos, list);
        let pcm = wav_to_atv_voice_pcm(&wav).unwrap();
        let back: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(back, samples, "the data chunk is found past the LIST chunk");
    }
}
