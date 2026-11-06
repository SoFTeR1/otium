//! Writes an MP4 with Media Foundation: the picture as it came from the encoder, the sound encoded
//! to AAC on the way.

use super::encoder::Packet;
use super::{RATE, SEC};
use std::path::Path;
use std::sync::Arc;
use windows::core::{Result, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;

pub struct Info {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub header: Option<Vec<u8>>,
}

const AUDIO_CHUNK: usize = RATE as usize / 10;

/// `pcm` starts where the first packet does.
pub unsafe fn write(path: &Path, info: &Info, packets: &[Arc<Packet>], pcm: &[i16]) -> Result<()> {
    let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
    let res = write_inner(path, info, packets, pcm);
    let _ = MFShutdown();
    res
}

unsafe fn write_inner(path: &Path, info: &Info, packets: &[Arc<Packet>], pcm: &[i16]) -> Result<()> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs: IMFAttributes = attrs.ok_or_else(windows::core::Error::empty)?;
    // written as fast as the disk goes, not at playing speed
    attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
    let url: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let w = MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None, &attrs)?;

    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    t.SetUINT32(&MF_MT_AVG_BITRATE, info.bitrate)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, ((info.width as u64) << 32) | info.height as u64)?;
    t.SetUINT64(&MF_MT_FRAME_RATE, ((info.fps as u64) << 32) | 1)?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
    t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
    let header = info.header.clone().filter(|h| !h.is_empty()).or_else(|| packets.first().and_then(|p| header_from_stream(&p.data)));
    if let Some(h) = &header {
        t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, h)?;
    }
    let video = w.AddStream(&t)?;
    w.SetInputMediaType(video, &t, None)?;

    let aac = MFCreateMediaType()?;
    aac.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    aac.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
    aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE as u32)?;
    aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbit/s
    let audio = w.AddStream(&aac)?;
    let raw = MFCreateMediaType()?;
    raw.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    raw.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    raw.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    raw.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE as u32)?;
    raw.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    raw.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
    raw.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE as u32 * 4)?;
    w.SetInputMediaType(audio, &raw, None)?;

    w.BeginWriting()?;
    let t0 = packets.first().map_or(0, |p| p.ts);
    let mut chunks = pcm.chunks(AUDIO_CHUNK * 2).enumerate().peekable();
    let chunk_time = |i: usize| i as i64 * AUDIO_CHUNK as i64 * SEC / RATE;
    for (i, p) in packets.iter().enumerate() {
        let time = p.ts - t0;
        // both streams in rising time, so the file doesn't need to hold one back for the other
        while let Some((k, c)) = chunks.next_if(|(k, _)| chunk_time(*k) <= time) {
            write_audio(&w, audio, c, chunk_time(k))?;
        }
        let dur = packets.get(i + 1).map_or(SEC / info.fps.max(1) as i64, |n| (n.ts - p.ts).max(1));
        let s = sample(&p.data, time, dur)?;
        if p.key {
            s.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
        }
        w.WriteSample(video, &s)?;
    }
    for (k, c) in chunks {
        write_audio(&w, audio, c, chunk_time(k))?;
    }
    w.Finalize()
}

unsafe fn write_audio(w: &IMFSinkWriter, stream: u32, pcm: &[i16], time: i64) -> Result<()> {
    let bytes = std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2);
    let s = sample(bytes, time, (pcm.len() / 2) as i64 * SEC / RATE)?;
    w.WriteSample(stream, &s)
}

unsafe fn sample(data: &[u8], time: i64, dur: i64) -> Result<IMFSample> {
    let buf = MFCreateMemoryBuffer(data.len() as u32)?;
    let mut p = std::ptr::null_mut();
    buf.Lock(&mut p, None, None)?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    buf.Unlock()?;
    buf.SetCurrentLength(data.len() as u32)?;
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    s.SetSampleTime(time)?;
    s.SetSampleDuration(dur)?;
    Ok(s)
}

/// The SPS and PPS at the start of a key frame's stream, for an encoder that keeps them there.
fn header_from_stream(data: &[u8]) -> Option<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i..i + 3] == [0, 0, 1] {
            starts.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, s) in starts.iter().enumerate() {
        let Some(head) = data.get(s + 3) else { break };
        let end = starts.get(k + 1).copied().unwrap_or(data.len());
        match head & 0x1F {
            7 | 8 => {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&data[s + 3..end]);
            }
            // the picture itself: the header comes before it
            1 | 5 => break,
            _ => {}
        }
    }
    // the zero that belongs to the next 4-byte start code isn't part of a unit
    while out.last() == Some(&0) {
        out.pop();
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_header() {
        let stream = [0, 0, 0, 1, 9, 0x10, 0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 9, 9];
        assert_eq!(header_from_stream(&stream).unwrap(), vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 0, 1, 0x68, 3]);
    }

    #[test]
    fn a_stream_without_a_header_has_none() {
        assert!(header_from_stream(&[0, 0, 1, 0x65, 9]).is_none());
        assert!(header_from_stream(&[]).is_none());
    }
}
