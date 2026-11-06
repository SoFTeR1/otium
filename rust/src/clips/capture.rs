//! The screen through DXGI Desktop Duplication: frames stay on the GPU, which also turns them into
//! NV12 at clip size; they come back to memory only for the encoder. The monitor is the one the game
//! is on. A mode change or a game going exclusive loses the duplication: it is simply opened again.

use super::encoder::{self, Encoder, Packet};
use super::{Saved, Shared, ShotRequest};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

const FPS: u32 = 60;
/// a little under 1/60 s: the screen's own 60 Hz, with its jitter, isn't thinned out
const FRAME_GAP: Duration = Duration::from_millis(14);
/// a screen that doesn't change still gets a picture now and then
const STILL_GAP: Duration = Duration::from_millis(250);

pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("clip-capture".into())
        .spawn(move || unsafe {
            use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_LITE, MF_VERSION};
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
            // kept across lost duplications while the size stays the same, so the buffer survives them
            let mut enc: Option<Encoder> = None;
            loop {
                let wanted = shared.want.load(Ordering::Relaxed);
                if !wanted {
                    if enc.take().is_some() {
                        forget(&shared);
                    }
                    if shared.shots.lock().unwrap().is_empty() {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                }
                if let Err(e) = session(&shared, &mut enc) {
                    eprintln!("clips: capture stopped: {e}");
                    fail_shots(&shared);
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        })
        .ok();
}

fn forget(shared: &Shared) {
    shared.video.lock().unwrap().packets.clear();
    shared.audio.lock().unwrap().clear();
}

/// The window whose screen should be recorded.
fn target_monitor(shared: &Shared) -> HMONITOR {
    unsafe {
        let own = shared.own.load(Ordering::Relaxed);
        let fg = GetForegroundWindow();
        let w = if !fg.is_invalid() { fg } else { HWND(own as _) };
        MonitorFromWindow(w, MONITOR_DEFAULTTOPRIMARY)
    }
}

struct Duplication {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    monitor: HMONITOR,
    width: u32,
    height: u32,
    /// the last frame of the game
    frame: ID3D11Texture2D,
}

unsafe fn open_duplication(monitor: HMONITOR) -> Result<Duplication> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let (mut i, mut found) = (0, None);
    'adapters: while let Ok(adapter) = factory.EnumAdapters1(i) {
        i += 1;
        let mut j = 0;
        while let Ok(out) = adapter.EnumOutputs(j) {
            j += 1;
            if out.GetDesc()?.Monitor == monitor {
                found = Some((adapter.clone(), out));
                break 'adapters;
            }
        }
    }
    let (adapter, output) = found.ok_or_else(windows::core::Error::empty)?;
    let (mut device, mut ctx) = (None, None);
    D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, None, D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
    let (device, ctx): (ID3D11Device, ID3D11DeviceContext) = (device.unwrap(), ctx.unwrap());
    let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
    let mode = dup.GetDesc().ModeDesc;
    let (width, height) = (mode.Width, mode.Height);
    let frame = texture(&device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, 0)?;
    Ok(Duplication { device, ctx, dup, monitor, width, height, frame })
}

unsafe fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, usage: D3D11_USAGE, bind: u32, cpu: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    };
    let mut t = None;
    device.CreateTexture2D(&desc, None, Some(&mut t))?;
    Ok(t.unwrap())
}

/// The GPU's video processor: the whole frame into an NV12 picture of clip size.
struct Converter {
    vctx: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    input: ID3D11VideoProcessorInputView,
    output: ID3D11VideoProcessorOutputView,
    nv12: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    packed: Vec<u8>,
}

impl Converter {
    unsafe fn new(d: &Duplication, width: u32, height: u32) -> Result<Self> {
        let vdev: ID3D11VideoDevice = d.device.cast()?;
        let vctx: ID3D11VideoContext = d.ctx.cast()?;
        let rate = DXGI_RATIONAL { Numerator: FPS, Denominator: 1 };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: d.width,
            InputHeight: d.height,
            OutputFrameRate: rate,
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let en = vdev.CreateVideoProcessorEnumerator(&content)?;
        let processor = vdev.CreateVideoProcessor(&en, 0)?;
        vctx.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vctx.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, BOOL(0));
        // full-range RGB in; BT.709 at TV levels out, as the encoder's output says
        let space_in = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
        let space_out = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) };
        vctx.VideoProcessorSetStreamColorSpace(&processor, 0, &space_in);
        vctx.VideoProcessorSetOutputColorSpace(&processor, &space_out);

        let nv12 = texture(&d.device, width, height, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let staging = texture(&d.device, width, height, DXGI_FORMAT_NV12, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
        };
        let mut input = None;
        vdev.CreateVideoProcessorInputView(&d.frame, &en, &in_desc, Some(&mut input))?;
        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut output = None;
        vdev.CreateVideoProcessorOutputView(&nv12, &en, &out_desc, Some(&mut output))?;
        let packed = vec![0u8; (width * height * 3 / 2) as usize];
        Ok(Self { vctx, processor, input: input.unwrap(), output: output.unwrap(), nv12, staging, width, height, packed })
    }

    /// Converts the frame and returns it, tightly packed.
    unsafe fn convert(&mut self, ctx: &ID3D11DeviceContext) -> Result<&[u8]> {
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: BOOL(1),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(self.input.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let mut streams = [stream];
        let res = self.vctx.VideoProcessorBlt(&self.processor, &self.output, 0, &streams);
        std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
        res?;
        ctx.CopyResource(&self.staging, &self.nv12);
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
        let (w, h, pitch) = (self.width as usize, self.height as usize, map.RowPitch as usize);
        let src = map.pData as *const u8;
        // the luma rows, then the half-height rows of colour right after them
        for y in 0..h {
            std::ptr::copy_nonoverlapping(src.add(y * pitch), self.packed.as_mut_ptr().add(y * w), w);
        }
        let uv = src.add(h * pitch);
        for y in 0..h / 2 {
            std::ptr::copy_nonoverlapping(uv.add(y * pitch), self.packed.as_mut_ptr().add((h + y) * w), w);
        }
        ctx.Unmap(&self.staging, 0);
        Ok(&self.packed)
    }
}

/// One stretch on one duplication: returns when the buffer is no longer wanted or the game moved to
/// another monitor. Errors mean the duplication is lost (or there is none to open right now).
unsafe fn session(shared: &Shared, enc: &mut Option<Encoder>) -> Result<()> {
    let d = open_duplication(target_monitor(shared))?;
    let wanted = shared.want.load(Ordering::Relaxed);
    let (w, h) = encoder::fit(d.width, d.height);
    let mut conv = if wanted { Some(Converter::new(&d, w, h)?) } else { None };
    if wanted && enc.as_ref().is_none_or(|e| e.width != w || e.height != h) {
        // another size: what is in the buffer can't be joined with what comes next
        *enc = None;
        let e = Encoder::new(w, h, FPS)?;
        eprintln!("clips: {w}x{h} with {}", e.name);
        let mut v = shared.video.lock().unwrap();
        v.packets.clear();
        v.width = w;
        v.height = h;
        v.fps = e.fps;
        v.bitrate = e.bitrate;
        v.header = None;
        drop(v);
        *enc = Some(e);
    }

    let mut have_frame = false;
    let mut fresh = false;
    let mut last_encoded = Instant::now() - STILL_GAP;
    let mut checked = Instant::now();
    let mut out: Vec<Packet> = Vec::new();
    loop {
        let want = shared.want.load(Ordering::Relaxed);
        if want != conv.is_some() || (!want && shared.shots.lock().unwrap().is_empty()) {
            return Ok(());
        }
        let own = shared.own.load(Ordering::Relaxed);
        let front = own != 0 && GetForegroundWindow().0 as isize == own;
        if shared.paused.swap(front, Ordering::Relaxed) != front {
            shared.clock.lock().unwrap().set_paused(front);
        }
        if !front && checked.elapsed() > Duration::from_secs(1) {
            checked = Instant::now();
            if target_monitor(shared) != d.monitor {
                return Ok(());
            }
        }

        let wait = FRAME_GAP.saturating_sub(last_encoded.elapsed()).as_millis().clamp(1, 16) as u32;
        let (mut info, mut res) = (DXGI_OUTDUPL_FRAME_INFO::default(), None);
        match d.dup.AcquireNextFrame(wait, &mut info, &mut res) {
            Ok(()) => {
                // over the game, OTIUM's picture isn't taken: the last game frame stays for a
                // screenshot from the Control Center. With no game frame yet, a picture is a picture.
                if info.LastPresentTime != 0 && (!front || !have_frame) || !have_frame {
                    if let Some(t) = res.as_ref().and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                        d.ctx.CopyResource(&d.frame, &t);
                        have_frame = true;
                        fresh = !front;
                    }
                }
                drop(res);
                d.dup.ReleaseFrame()?;
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
            Err(e) => return Err(e),
        }

        if have_frame {
            take_screenshots(shared, &d)?;
        }
        let Some(c) = conv.as_mut() else { continue };
        let Some(e) = enc.as_mut() else { return Ok(()) };
        let gap = last_encoded.elapsed();
        if have_frame && !front && ((fresh && gap >= FRAME_GAP) || gap >= STILL_GAP) {
            fresh = false;
            last_encoded = Instant::now();
            let ts = shared.clock.lock().unwrap().now();
            let nv12 = c.convert(&d.ctx)?;
            if let Err(err) = e.push(nv12, ts, 10_000_000 / FPS as i64) {
                *enc = None;
                return Err(err);
            }
        }
        if let Err(err) = e.pump(&mut out) {
            *enc = None;
            return Err(err);
        }
        if !out.is_empty() {
            let mut v = shared.video.lock().unwrap();
            if v.header.is_none() {
                v.header = e.header();
            }
            for p in out.drain(..) {
                v.push(p);
            }
        }
    }
}

/// Takes the frame back from the GPU for the pictures that were asked for; the slow PNG writing
/// happens on its own thread.
unsafe fn take_screenshots(shared: &Shared, d: &Duplication) -> Result<()> {
    let list: Vec<ShotRequest> = std::mem::take(&mut *shared.shots.lock().unwrap());
    if list.is_empty() {
        return Ok(());
    }
    let staging = texture(&d.device, d.width, d.height, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
    d.ctx.CopyResource(&staging, &d.frame);
    let mut map = D3D11_MAPPED_SUBRESOURCE::default();
    d.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
    let (w, h, pitch) = (d.width as usize, d.height as usize, map.RowPitch as usize);
    let src = std::slice::from_raw_parts(map.pData as *const u8, pitch * h);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in src.chunks_exact(pitch) {
        for p in row[..w * 4].chunks_exact(4) {
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    d.ctx.Unmap(&staging, 0);
    let (w, h) = (d.width, d.height);
    std::thread::Builder::new()
        .name("clip-shot".into())
        .spawn(move || {
            for r in list {
                let res = write_png(&r.path, &rgb, w, h).map(|_| Saved { path: r.path.clone(), secs: 0 });
                (r.done)(res);
            }
        })
        .ok();
    Ok(())
}

fn write_png(path: &std::path::Path, rgb: &[u8], w: u32, h: u32) -> std::result::Result<(), String> {
    let fail = |e: String| format!("Не удалось сохранить скриншот: {e}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    }
    image::save_buffer(path, rgb, w, h, image::ColorType::Rgb8).map_err(|e| fail(e.to_string()))
}

/// A picture was asked for and the screen can't be read.
fn fail_shots(shared: &Shared) {
    let list: Vec<ShotRequest> = std::mem::take(&mut *shared.shots.lock().unwrap());
    for r in list {
        (r.done)(Err("Не удалось сделать снимок экрана".into()));
    }
}
