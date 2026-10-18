//! Sound decoders: every format a mod can ship a sound in, to one PCM
//! layout ([`Sound`]: signed 16-bit, mono or stereo, any rate).
//!
//! - DMX (Doom's sound lumps: format 3, unsigned 8-bit);
//! - RIFF WAVE: integer PCM of 8/16/24/32 bits, 32/64-bit float, and the
//!   `WAVE_FORMAT_EXTENSIBLE` wrapper of those;
//! - Ogg Vorbis, FLAC and MP3, through `symphonia`.
//!
//! The format is recognised from the data itself, never from a file name:
//! mods put Ogg files in extension-less lumps and WAVs named `.ogg`.
//! Decoding happens once per sound; callers cache the result.

use std::io::Cursor;

use crate::mixer::Sound;

/// What a sound resource is, judged from its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundFormat {
    Dmx,
    Wav,
    Ogg,
    Flac,
    Mp3,
    Unknown,
}

impl SoundFormat {
    pub fn name(self) -> &'static str {
        match self {
            SoundFormat::Dmx => "DMX",
            SoundFormat::Wav => "WAV",
            SoundFormat::Ogg => "Ogg Vorbis",
            SoundFormat::Flac => "FLAC",
            SoundFormat::Mp3 => "MP3",
            SoundFormat::Unknown => "unknown",
        }
    }
}

/// Recognise a sound's format from its header.
pub fn sniff(data: &[u8]) -> SoundFormat {
    if data.len() >= 8 && data[0] == 3 && data[1] == 0 {
        return SoundFormat::Dmx;
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        return SoundFormat::Wav;
    }
    if data.starts_with(b"OggS") {
        return SoundFormat::Ogg;
    }
    if data.starts_with(b"fLaC") {
        return SoundFormat::Flac;
    }
    if data.starts_with(b"ID3") || (data.len() >= 2 && data[0] == 0xFF && data[1] & 0xE0 == 0xE0) {
        return SoundFormat::Mp3;
    }
    SoundFormat::Unknown
}

/// Decode a sound resource of any supported format.
pub fn decode(data: &[u8]) -> Result<Sound, String> {
    match sniff(data) {
        SoundFormat::Dmx => Sound::parse_dmx(data).ok_or_else(|| "bad DMX sound".to_string()),
        SoundFormat::Wav => decode_wav(data),
        f @ (SoundFormat::Ogg | SoundFormat::Flac | SoundFormat::Mp3) => decode_symphonia(data, f),
        SoundFormat::Unknown => Err("not a recognised sound format (DMX, WAV, Ogg, FLAC, MP3)".into()),
    }
}

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// RIFF WAVE with integer or float PCM.
pub fn decode_wav(data: &[u8]) -> Result<Sound, String> {
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    let mut pcm: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32le(data, pos + 4) as usize;
        let body = &data[pos + 8..(pos + 8 + len).min(data.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16le(body, 0);
                if tag == 0xFFFE && body.len() >= 26 {
                    // WAVE_FORMAT_EXTENSIBLE: the real tag opens the GUID.
                    tag = u16le(body, 24);
                }
                fmt = Some((tag, u16le(body, 2), u32le(body, 4), u16le(body, 14)));
            }
            b"data" => pcm = Some(body),
            _ => {}
        }
        // Chunks are padded to even sizes.
        pos += 8 + len + (len & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or("WAV without a fmt chunk")?;
    let pcm = pcm.ok_or("WAV without a data chunk")?;
    if channels == 0 || rate == 0 {
        return Err("WAV with no channels or rate".into());
    }
    let samples: Vec<i16> = match (tag, bits) {
        (1, 8) => pcm.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        (1, 16) => pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
        (1, 24) => pcm.chunks_exact(3).map(|c| i16::from_le_bytes([c[1], c[2]])).collect(),
        (1, 32) => pcm.chunks_exact(4).map(|c| (i32::from_le_bytes([c[0], c[1], c[2], c[3]]) >> 16) as i16).collect(),
        (3, 32) => pcm.chunks_exact(4).map(|c| to_i16(f32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect(),
        (3, 64) => pcm.chunks_exact(8).map(|c| to_i16(f64::from_le_bytes(c.try_into().unwrap()) as f32)).collect(),
        (t, b) => return Err(format!("unsupported WAV encoding (format {t:#x}, {b} bits)")),
    };
    Ok(to_stereo_or_mono(samples, channels as usize, rate))
}

#[inline]
fn to_i16(f: f32) -> i16 {
    (f.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Keep mono and stereo; mix more channels down to stereo.
fn to_stereo_or_mono(samples: Vec<i16>, channels: usize, rate: u32) -> Sound {
    match channels {
        1 | 2 => Sound { rate, channels: channels as u16, samples },
        n => {
            let frames = samples.len() / n;
            let mut out = Vec::with_capacity(frames * 2);
            for f in samples.chunks_exact(n) {
                // Even channels left, odd right (the usual front pairs first).
                let (mut l, mut r, mut nl, mut nr) = (0i32, 0i32, 0, 0);
                for (i, &s) in f.iter().enumerate() {
                    if i % 2 == 0 {
                        l += s as i32;
                        nl += 1;
                    } else {
                        r += s as i32;
                        nr += 1;
                    }
                }
                out.push((l / nl.max(1)) as i16);
                out.push((r / nr.max(1)) as i16);
            }
            Sound { rate, channels: 2, samples: out }
        }
    }
}

/// Ogg Vorbis, FLAC and MP3.
fn decode_symphonia(data: &[u8], format: SoundFormat) -> Result<Sound, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let stream = MediaSourceStream::new(Box::new(Cursor::new(data.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(match format {
        SoundFormat::Ogg => "ogg",
        SoundFormat::Flac => "flac",
        _ => "mp3",
    });
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("{}: {e}", format.name()))?;
    let mut reader = probed.format;
    let track = reader.default_track().ok_or_else(|| format!("{}: no audio track", format.name()))?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.ok_or_else(|| format!("{}: no sample rate", format.name()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("{}: {e}", format.name()))?;
    let mut samples: Vec<i16> = Vec::new();
    let mut channels = 0usize;
    let mut buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => {
                if samples.is_empty() {
                    return Err(format!("{}: {e}", format.name()));
                }
                break; // keep what decoded before a damaged tail
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                let spec = *audio.spec();
                channels = spec.channels.count();
                let need = audio.capacity() as u64;
                if buf.as_ref().is_none_or(|b| (b.capacity() as u64) < need * channels as u64) {
                    buf = Some(SampleBuffer::new(need, spec));
                }
                let b = buf.as_mut().expect("just made");
                b.copy_interleaved_ref(audio);
                samples.extend_from_slice(b.samples());
            }
            // A corrupt frame: skip it, as players do.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("{}: {e}", format.name())),
        }
    }
    if channels == 0 || samples.is_empty() {
        return Err(format!("{}: no samples decoded", format.name()));
    }
    Ok(to_stereo_or_mono(samples, channels, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file with the given format tag, channels, rate and bits.
    pub(crate) fn wav(tag: u16, channels: u16, rate: u32, bits: u16, pcm: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        let align = channels * bits / 8;
        fmt.extend_from_slice(&(rate * align as u32).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((4 + 8 + fmt.len() + 8 + pcm.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        // An unknown chunk first, odd sized (padding must be honoured).
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&[1, 2, 3, 0]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }

    #[test]
    fn wav_layouts() {
        let s = decode(&wav(1, 1, 22050, 8, &[128, 255, 0])).unwrap();
        assert_eq!((s.rate, s.channels), (22050, 1));
        assert_eq!(s.samples, vec![0, 127 << 8, -128 << 8]);
        let pcm16: Vec<u8> = [1000i16, -1000, 2000, -2000].iter().flat_map(|v| v.to_le_bytes()).collect();
        let s = decode(&wav(1, 2, 44100, 16, &pcm16)).unwrap();
        assert_eq!((s.channels, s.frames()), (2, 2));
        assert_eq!(s.samples, vec![1000, -1000, 2000, -2000]);
        let f32s: Vec<u8> = [0.5f32, -1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let s = decode(&wav(3, 1, 8000, 32, &f32s)).unwrap();
        assert_eq!(s.samples, vec![16383, -32767]);
        let s24: Vec<u8> = vec![0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        assert_eq!(decode(&wav(1, 1, 8000, 24, &s24)).unwrap().samples, vec![0x4000, -0x4000]);
        assert!(decode(&wav(0x11, 1, 8000, 4, &[0; 4])).unwrap_err().contains("unsupported"));
    }

    #[test]
    fn dmx_and_sniffing() {
        let mut dmx = vec![3, 0];
        dmx.extend_from_slice(&11025u16.to_le_bytes());
        // (DMX, like Chocolate Doom, rejects sounds of 48 bytes or less.)
        dmx.extend_from_slice(&(32u32 + 20).to_le_bytes());
        dmx.extend(std::iter::repeat_n(128, 16));
        dmx.extend_from_slice(&[128, 255, 0]);
        dmx.extend(std::iter::repeat_n(128, 17));
        dmx.extend(std::iter::repeat_n(128, 16));
        let s = decode(&dmx).unwrap();
        assert_eq!((s.rate, s.channels, s.frames()), (11025, 1, 20));
        assert_eq!(&s.samples[..3], &[0, 127 << 8, -128 << 8]);
        assert_eq!(sniff(b"OggS\0\0"), SoundFormat::Ogg);
        assert_eq!(sniff(b"fLaC\0\0"), SoundFormat::Flac);
        assert_eq!(sniff(b"ID3\x03\0"), SoundFormat::Mp3);
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90]), SoundFormat::Mp3);
        assert!(decode(b"hello world").is_err());
    }
}
