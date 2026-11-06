//! NV12 frames into H.264 with the best Media Foundation encoder the PC has: the GPU's first (they
//! work asynchronously, through events), Microsoft's software one if nothing else starts.

use windows::core::{Interface, Result, GUID, VARIANT};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

const STREAM: u32 = 0;
const NEED_INPUT: u32 = METransformNeedInput.0 as u32;
const HAVE_OUTPUT: u32 = METransformHaveOutput.0 as u32;

#[derive(Debug)]
pub struct Packet {
    pub data: Vec<u8>,
    /// clock time, 100 ns units
    pub ts: i64,
    pub key: bool,
}

pub struct Encoder {
    mft: IMFTransform,
    /// asynchronous encoders (the GPU's) say when they want a frame and have a packet
    events: Option<IMFMediaEventGenerator>,
    wants: u32,
    /// the newest frame the encoder hasn't taken yet; a newer one replaces it
    queued: Option<IMFSample>,
    own_samples: bool,
    out_size: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

unsafe fn video_type(subtype: &GUID, w: u32, h: u32, fps: u32) -> Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
    t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
    Ok(t)
}

impl Encoder {
    pub unsafe fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
        let flags = MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER;
        let (mut list, mut n) = (std::ptr::null_mut(), 0u32);
        MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut n)?;
        let found = std::slice::from_raw_parts_mut(list, n as usize);
        let bitrate = bitrate_for(width, height, fps);
        let mut last = windows::core::Error::empty();
        let mut opened = None;
        for a in found.iter_mut() {
            // every entry is released, including the ones after the one that works
            let Some(a) = a.take() else { continue };
            if opened.is_some() {
                continue;
            }
            match Self::open(&a, width, height, fps, bitrate) {
                Ok(e) => opened = Some(e),
                Err(e) => {
                    eprintln!("clips: encoder {} didn't start: {e}", friendly_name(&a));
                    let _ = a.ShutdownObject();
                    last = e;
                }
            }
        }
        CoTaskMemFree(Some(list as *const _));
        opened.ok_or(last)
    }

    unsafe fn open(a: &IMFActivate, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let mft: IMFTransform = a.ActivateObject()?;
        let attrs = mft.GetAttributes().ok();
        let asynchronous = attrs.as_ref().is_some_and(|x| x.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0);
        if asynchronous {
            if let Some(x) = &attrs {
                x.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
        }
        if let Some(x) = &attrs {
            let _ = x.SetUINT32(&MF_LOW_LATENCY, 1);
        }
        // settings an encoder may not know are skipped: the stream still plays
        if let Ok(api) = mft.cast::<ICodecAPI>() {
            let set = |what: &GUID, v: VARIANT| {
                let _ = api.SetValue(what, &v);
            };
            set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32));
            set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bitrate));
            // a key frame every 2 seconds: a clip can only start on one
            set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(fps * 2));
            // frames in display order, so packets arrive with rising times
            set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
        }

        let out = video_type(&MFVideoFormat_H264, width, height, fps)?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps * 2)?;
        mft.SetOutputType(STREAM, &out, 0)?;
        let inp = video_type(&MFVideoFormat_NV12, width, height, fps)?;
        inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
        mft.SetInputType(STREAM, &inp, 0)?;

        let info = mft.GetOutputStreamInfo(STREAM)?;
        let own_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
        let out_size = info.cbSize.max(width * height * 2);
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        let events = if asynchronous { Some(mft.cast::<IMFMediaEventGenerator>()?) } else { None };
        let name = friendly_name(a);
        Ok(Self { mft, events, wants: 0, queued: None, own_samples, out_size, name, width, height, fps, bitrate })
    }

    /// SPS and PPS, if the encoder tells them outside the stream.
    pub unsafe fn header(&self) -> Option<Vec<u8>> {
        let t = self.mft.GetOutputCurrentType(STREAM).ok()?;
        let n = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
        let mut b = vec![0u8; n as usize];
        t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut b, None).ok()?;
        (!b.is_empty()).then_some(b)
    }

    /// Hands over a frame: tightly packed NV12.
    pub unsafe fn push(&mut self, nv12: &[u8], ts: i64, dur: i64) -> Result<()> {
        let buf = MFCreateMemoryBuffer(nv12.len() as u32)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), p, nv12.len());
        buf.Unlock()?;
        buf.SetCurrentLength(nv12.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(ts)?;
        s.SetSampleDuration(dur)?;
        self.queued = Some(s);
        Ok(())
    }

    /// Moves frames in and packets out, without waiting.
    pub unsafe fn pump(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        if let Some(events) = self.events.clone() {
            loop {
                let ev = match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(e) => e,
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                    Err(e) => return Err(e),
                };
                match ev.GetType()? {
                    NEED_INPUT => self.wants += 1,
                    HAVE_OUTPUT => {
                        self.take_packet(out)?;
                    }
                    _ => {}
                }
            }
            if self.wants > 0 {
                if let Some(s) = self.queued.take() {
                    self.mft.ProcessInput(STREAM, &s, 0)?;
                    self.wants -= 1;
                }
            }
        } else {
            while self.take_packet(out)? {}
            if let Some(s) = self.queued.take() {
                match self.mft.ProcessInput(STREAM, &s, 0) {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_NOTACCEPTING => self.queued = Some(s),
                    Err(e) => return Err(e),
                }
                while self.take_packet(out)? {}
            }
        }
        Ok(())
    }

    /// One packet, if there is one.
    unsafe fn take_packet(&mut self, out: &mut Vec<Packet>) -> Result<bool> {
        let sample = if self.own_samples {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
            Some(s)
        };
        let mut data = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: STREAM, pSample: std::mem::ManuallyDrop::new(sample), dwStatus: 0, pEvents: std::mem::ManuallyDrop::new(None) }];
        let mut status = 0;
        let res = self.mft.ProcessOutput(0, &mut data, &mut status);
        let [mut data] = data;
        let sample = std::mem::ManuallyDrop::take(&mut data.pSample);
        std::mem::ManuallyDrop::drop(&mut data.pEvents);
        match res {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(false),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                // the encoder settled on the details of its output: agree and carry on
                let t = self.mft.GetOutputAvailableType(STREAM, 0)?;
                self.mft.SetOutputType(STREAM, &t, 0)?;
                return Ok(true);
            }
            Err(e) => return Err(e),
        }
        let Some(s) = sample else { return Ok(false) };
        let buf = s.ConvertToContiguousBuffer()?;
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut len))?;
        let bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
        buf.Unlock()?;
        out.push(Packet { data: bytes, ts: s.GetSampleTime()?, key: s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0 });
        Ok(true)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Some(e) = &self.events {
                if let Ok(gen) = e.cast::<IMFShutdown>() {
                    let _ = gen.Shutdown();
                }
            }
        }
    }
}

unsafe fn friendly_name(a: &IMFActivate) -> String {
    let (mut p, mut n) = (windows::core::PWSTR::null(), 0u32);
    if a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut n).is_ok() {
        let name = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        name
    } else {
        "?".into()
    }
}

/// About 12 Mbit/s for 1080p at 60 frames: a 30-second clip of some 45 MB.
pub fn bitrate_for(w: u32, h: u32, fps: u32) -> u32 {
    ((w as u64 * h as u64 * fps as u64) / 10).clamp(1_500_000, 20_000_000) as u32
}

/// The biggest even size up to 1920x1080 with the screen's shape.
pub fn fit(w: u32, h: u32) -> (u32, u32) {
    let k = (1920.0 / w.max(1) as f64).min(1080.0 / h.max(1) as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * k), even(h as f64 * k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(fit(1920, 1080), (1920, 1080));
        assert_eq!(fit(3840, 2160), (1920, 1080));
        assert_eq!(fit(2560, 1440), (1920, 1080));
        assert_eq!(fit(3440, 1440), (1920, 804));
        assert_eq!(fit(1366, 768), (1366, 768));
        assert_eq!(fit(1367, 769), (1366, 768));
    }

    #[test]
    fn bitrates() {
        assert_eq!(bitrate_for(1920, 1080, 60), 12_441_600);
        assert_eq!(bitrate_for(320, 200, 30), 1_500_000);
    }
}
