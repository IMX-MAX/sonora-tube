//! The libmpv handle and the thread that renders its frames into memory.
//!
//! mpv is driven through its software render API, which writes each frame into a buffer this
//! crate owns. That costs a copy per frame, and in exchange it needs nothing from the window's
//! graphics backend, so it is the same code on Vulkan, Metal and DirectX.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::RenderImage;
use image::{Frame, RgbaImage};
use libmpv2_sys as sys;
use smallvec::smallvec;

/// How far the glow moves toward each new frame's colour, so a cut fades rather than flashes.
const GLOW_EASE: f32 = 0.04;
/// How many pixels apart the ones sampled for the glow are, across and down.
const GLOW_STEP: usize = 24;

/// The widest frame the renderer produces. A larger video is scaled down by mpv, since every
/// frame is copied to the GPU and a 4K copy sixty times a second is more than the copy is worth.
const WIDEST: i64 = 1920;
/// The narrowest frame the renderer produces, however small the picture is shown.
const SMALLEST: i64 = 64;
/// How long the render thread sleeps between looks at its stop flag when no frame arrives.
const IDLE: Duration = Duration::from_millis(250);
/// `MPV_RENDER_UPDATE_FRAME`: a new frame is ready to be rendered.
const UPDATE_FRAME: u64 = 1;

/// The stream of "a new frame is ready" wake-ups a view waits on.
pub type Frames = UnboundedReceiver<()>;

/// What the render thread and the view share: the newest frame, replaced as each one lands.
type Latest = Arc<Mutex<Option<Arc<RenderImage>>>>;

/// The room the picture is shown in, in device pixels, as its width above its height in one
/// number. Zero until a view has painted it.
type Room = Arc<AtomicU64>;

/// The eased colour of each half of the picture, written by the render thread.
type Glow = Arc<[AtomicU32; 2]>;

/// The mpv core. Send and Sync because the mpv client API is safe to call from any thread.
struct Core(*mut sys::mpv_handle);

// SAFETY: mpv documents its client API as thread-safe for a single handle.
unsafe impl Send for Core {}
// SAFETY: as above.
unsafe impl Sync for Core {}

/// One video being played. Dropping it stops playback and frees mpv.
pub struct Player {
    core: Arc<Core>,
    latest: Latest,
    glow: Glow,
    room: Room,
    stop: Option<Sender<Wake>>,
    thread: Option<JoinHandle<()>>,
}

/// What wakes the render thread.
enum Wake {
    /// mpv has something new for the renderer.
    Update,
    /// The room the picture is shown in changed, so the frame on screen is the wrong size.
    Resize,
    /// The player is being dropped.
    Stop,
}

impl Player {
    /// Starts mpv and its render thread. The receiver gets one message per frame, which is what
    /// a view awaits to know when to repaint.
    pub fn new() -> Result<(Self, Frames)> {
        // SAFETY: mpv_create has no preconditions.
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            bail!("cannot create the mpv core");
        }
        let core = Arc::new(Core(handle));
        for (name, value) in [
            ("vo", "libmpv"),
            ("hwdec", "auto-copy"),
            ("ytdl", "yes"),
            // Captions are drawn by the app over the picture, never by mpv.
            ("sub-visibility", "no"),
            // Start showing the picture as soon as there is some, rather than after a long probe.
            ("demuxer-lavf-probe-info", "nostreams"),
            ("cache", "yes"),
            // Read a little ahead of what plays rather than the whole video.
            ("cache-secs", "30"),
            ("demuxer-max-bytes", "32MiB"),
            ("demuxer-max-back-bytes", "8MiB"),
            ("ytdl-format", "bestvideo[height<=?1080]+bestaudio/best"),
            ("terminal", "no"),
            ("keep-open", "yes"),
            ("audio-client-name", "YouTube+"),
        ] {
            core.option(name, value)?;
        }
        if let Some(bundled) = bundled_ytdl() {
            core.option("script-opts", &format!("ytdl_hook-ytdl_path={bundled}"))?;
        }
        // SAFETY: the handle is live and not yet initialised.
        check(unsafe { sys::mpv_initialize(core.0) }).context("cannot initialise mpv")?;

        let latest = Latest::default();
        let glow = Glow::default();
        let room = Room::default();
        let (wake, woken) = mpsc::channel();
        let (frames, receiver) = unbounded();
        let renderer = Renderer::new(&core, wake.clone())?;
        let thread = std::thread::Builder::new()
            .name("video-render".into())
            .spawn({
                let (latest, glow, room) = (latest.clone(), glow.clone(), room.clone());
                move || renderer.run(&woken, &latest, &glow, &room, &frames)
            })
            .context("cannot start the video render thread")?;

        Ok((
            Self {
                core,
                latest,
                glow,
                room,
                stop: Some(wake),
                thread: Some(thread),
            },
            receiver,
        ))
    }

    /// Plays a picture and the sound that goes with it from their own addresses, from `start`
    /// seconds in, replacing whatever was playing.
    pub fn play(&self, picture: &str, sound: &str, start: f64) -> Result<()> {
        // mpv's own notation for streams that play together, each address led by its length.
        let part = |address: &str| {
            format!(
                "!new_stream;!no_clip;!no_chapters;%{}%{address}",
                address.len()
            )
        };
        self.begin(start);
        self.core.command(&[
            "loadfile",
            &format!("edl://{};{}", part(picture), part(sound)),
        ])
    }

    /// Plays the page of a video through yt-dlp, which is slow to start and knows every site.
    pub fn load_from(&self, source: &str, start: f64) -> Result<()> {
        self.begin(start);
        self.core.command(&["loadfile", source])
    }

    /// Sets where the next file starts, in seconds.
    fn begin(&self, start: f64) {
        let value = match start > 0. {
            true => start.to_string(),
            false => "none".to_owned(),
        };
        if let Err(error) = self.core.set("start", &value) {
            log::warn!("video: cannot set where to start: {error:#}");
        }
    }

    /// Writes the picture on screen to `path` at the video's own size, without captions.
    pub fn snapshot(&self, path: &std::path::Path) -> Result<()> {
        let path = path.to_str().context("the path is not text")?;
        self.core.command(&["screenshot-to-file", path, "video"])
    }

    /// Drops what is playing, leaving the last picture up.
    pub fn stop(&self) {
        if let Err(error) = self.core.command(&["stop"]) {
            log::warn!("video: cannot stop: {error:#}");
        }
    }

    /// Whether nothing is loaded, which after a load means the file could not be opened.
    pub fn idle(&self) -> bool {
        self.core.flag("idle-active").unwrap_or(false)
    }

    /// The colours of the left and right halves of the picture, averaged and eased over time,
    /// as `0xRRGGBB`. Black before the first frame.
    pub fn glow(&self) -> (u32, u32) {
        (
            self.glow[0].load(Ordering::Relaxed),
            self.glow[1].load(Ordering::Relaxed),
        )
    }

    /// Tells the renderer how large the picture is shown, in device pixels, so that it draws
    /// frames no larger than that. A view calls it each time it paints.
    pub fn fit(&self, width: u32, height: u32) {
        let room = u64::from(width) << 32 | u64::from(height);
        if self.room.swap(room, Ordering::Relaxed) != room
            && let Some(wake) = &self.stop
        {
            let _ = wake.send(Wake::Resize);
        }
    }

    /// The newest frame, if one has been rendered yet.
    pub fn frame(&self) -> Option<Arc<RenderImage>> {
        self.latest.lock().ok()?.clone()
    }

    pub fn paused(&self) -> bool {
        self.core.flag("pause").unwrap_or(false)
    }

    pub fn set_paused(&self, paused: bool) {
        let value = match paused {
            true => "yes",
            false => "no",
        };
        if let Err(error) = self.core.set("pause", value) {
            log::warn!("video: cannot change pause: {error:#}");
        }
    }

    /// Where playback is, in seconds.
    pub fn position(&self) -> Option<f64> {
        self.core.double("time-pos")
    }

    /// How long the video is, in seconds. None for a live stream or before the file has opened.
    pub fn duration(&self) -> Option<f64> {
        self.core.double("duration")
    }

    /// Moves by a number of seconds, negative for backwards.
    pub fn seek_by(&self, seconds: f64) {
        if let Err(error) = self
            .core
            .command(&["seek", &seconds.to_string(), "relative"])
        {
            log::warn!("video: cannot seek: {error:#}");
        }
    }

    /// Moves to a position in seconds.
    pub fn seek_to(&self, seconds: f64) {
        if let Err(error) = self
            .core
            .command(&["seek", &seconds.to_string(), "absolute"])
        {
            log::warn!("video: cannot seek: {error:#}");
        }
    }

    /// How fast the video plays, 1 being as recorded.
    pub fn set_speed(&self, speed: f64) {
        if let Err(error) = self.core.set("speed", &speed.to_string()) {
            log::warn!("video: cannot set the speed: {error:#}");
        }
    }

    /// Starts the video over each time it ends, as a Short does, or lets it end.
    pub fn set_looping(&self, looping: bool) {
        let value = match looping {
            true => "inf",
            false => "no",
        };
        if let Err(error) = self.core.set("loop-file", value) {
            log::warn!("video: cannot change looping: {error:#}");
        }
    }

    /// The volume from 0 to 100.
    pub fn set_volume(&self, volume: f64) {
        if let Err(error) = self
            .core
            .set("volume", &volume.clamp(0.0, 100.0).to_string())
        {
            log::warn!("video: cannot set the volume: {error:#}");
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The render context has to be freed before the core, and it lives on the render thread.
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(Wake::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: the render thread has exited and freed its context, and nothing else holds
        // the handle past this point: `core` is only shared with that thread.
        unsafe { sys::mpv_terminate_destroy(self.core.0) };
    }
}

impl Core {
    fn option(&self, name: &str, value: &str) -> Result<()> {
        let (name_c, value_c) = (CString::new(name)?, CString::new(value)?);
        // SAFETY: both strings are valid and NUL-terminated for the length of the call.
        check(unsafe { sys::mpv_set_option_string(self.0, name_c.as_ptr(), value_c.as_ptr()) })
            .with_context(|| format!("cannot set the mpv option {name}"))
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        let (name_c, value_c) = (CString::new(name)?, CString::new(value)?);
        // SAFETY: both strings are valid and NUL-terminated for the length of the call.
        check(unsafe { sys::mpv_set_property_string(self.0, name_c.as_ptr(), value_c.as_ptr()) })
            .with_context(|| format!("cannot set the mpv property {name}"))
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        let owned = args
            .iter()
            .map(|arg| CString::new(*arg))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: the array is NULL-terminated and its strings outlive the call.
        check(unsafe { sys::mpv_command_async(self.0, 0, pointers.as_mut_ptr()) })
            .with_context(|| format!("cannot run the mpv command {}", args[0]))
    }

    fn double(&self, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value = 0f64;
        // SAFETY: the out pointer is a live f64, the type MPV_FORMAT_DOUBLE writes.
        let status = unsafe {
            sys::mpv_get_property(
                self.0,
                name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_DOUBLE,
                (&raw mut value).cast(),
            )
        };
        (status >= 0).then_some(value)
    }

    fn int(&self, name: &str) -> Option<i64> {
        let name = CString::new(name).ok()?;
        let mut value = 0i64;
        // SAFETY: the out pointer is a live i64, the type MPV_FORMAT_INT64 writes.
        let status = unsafe {
            sys::mpv_get_property(
                self.0,
                name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_INT64,
                (&raw mut value).cast(),
            )
        };
        (status >= 0).then_some(value)
    }

    fn flag(&self, name: &str) -> Option<bool> {
        let name = CString::new(name).ok()?;
        let mut value: c_int = 0;
        // SAFETY: the out pointer is a live int, the type MPV_FORMAT_FLAG writes.
        let status = unsafe {
            sys::mpv_get_property(
                self.0,
                name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_FLAG,
                (&raw mut value).cast(),
            )
        };
        (status >= 0).then_some(value != 0)
    }
}

/// mpv's software renderer, owned by the render thread.
struct Renderer {
    core: Arc<Core>,
    context: *mut sys::mpv_render_context,
    /// The sender mpv's update callback holds a pointer to. Freed after the context.
    wake: *mut Sender<Wake>,
}

// SAFETY: the context is created on one thread and then used and freed only on the render
// thread, which mpv allows as long as calls do not overlap.
unsafe impl Send for Renderer {}

impl Renderer {
    fn new(core: &Arc<Core>, wake: Sender<Wake>) -> Result<Self> {
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: sys::MPV_RENDER_API_TYPE_SW.as_ptr().cast_mut().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let mut context = ptr::null_mut();
        // SAFETY: the params array ends with the INVALID entry mpv looks for.
        check(unsafe {
            sys::mpv_render_context_create(&raw mut context, core.0, params.as_mut_ptr())
        })
        .context("cannot create the mpv render context")?;

        let wake = Box::into_raw(Box::new(wake));
        // SAFETY: the pointer stays valid until `Drop` frees the context and then the box.
        unsafe {
            sys::mpv_render_context_set_update_callback(context, Some(on_update), wake.cast());
        }
        Ok(Self {
            core: core.clone(),
            context,
            wake,
        })
    }

    /// Renders every frame mpv announces until the player is dropped.
    fn run(
        self,
        woken: &mpsc::Receiver<Wake>,
        latest: &Latest,
        glow: &Glow,
        room: &Room,
        frames: &UnboundedSender<()>,
    ) {
        let mut eased = [[0f32; 3]; 2];
        loop {
            let resized = match woken.recv_timeout(IDLE) {
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => continue,
                Ok(Wake::Update) => false,
                Ok(Wake::Resize) => true,
            };
            // SAFETY: the context is live and only this thread calls into it.
            let flags = unsafe { sys::mpv_render_context_update(self.context) };
            // A picture shown at a new size is drawn again even when it stands still.
            if flags & UPDATE_FRAME == 0 && !resized {
                continue;
            }
            let Some((image, halves)) = self.render(room.load(Ordering::Relaxed)) else {
                continue;
            };
            for (side, now) in halves.into_iter().enumerate() {
                let mut packed = 0u32;
                for (channel, now) in now.into_iter().enumerate() {
                    let held = &mut eased[side][channel];
                    *held += (now - *held) * GLOW_EASE;
                    packed = packed << 8 | (*held as u32).min(255);
                }
                glow[side].store(packed, Ordering::Relaxed);
            }
            if let Ok(mut slot) = latest.lock() {
                *slot = Some(Arc::new(image));
            }
            if frames.unbounded_send(()).is_err() {
                return;
            }
        }
    }

    /// Renders the current frame as large as it is shown in `room`, and never larger than the
    /// video itself or wider than [`WIDEST`].
    fn render(&self, room: u64) -> Option<(RenderImage, [[f32; 3]; 2])> {
        let (native_width, native_height) = (self.core.int("dwidth")?, self.core.int("dheight")?);
        if native_width <= 0 || native_height <= 0 {
            return None;
        }
        let (across, down) = (i64::from((room >> 32) as u32), i64::from(room as u32));
        let mut width = native_width.min(WIDEST);
        // Letterboxed in the room, the picture is as wide as the room or as tall as it.
        if across > 0 && down > 0 {
            width = width.min(across).min(down * native_width / native_height);
        }
        let width = width.max(SMALLEST);
        let height = (native_height * width / native_width).max(1);
        let (width, height) = (c_int::try_from(width).ok()?, c_int::try_from(height).ok()?);
        let stride = usize::try_from(width).ok()? * 4;
        let mut pixels = vec![0u8; stride * usize::try_from(height).ok()?];

        let mut size = [width, height];
        let mut stride_param = stride;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: c"bgr0".as_ptr().cast_mut().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: (&raw mut stride_param).cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: pixels.as_mut_ptr().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        // SAFETY: the buffer is `stride * height` bytes, which is what the size, format and
        // stride passed alongside it describe, and all of them outlive the call.
        let status = unsafe { sys::mpv_render_context_render(self.context, params.as_mut_ptr()) };
        if let Err(error) = check(status) {
            log::warn!("video: cannot render a frame: {error:#}");
            return None;
        }
        // mpv leaves the fourth byte as padding, and GPUI reads it as alpha.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = u8::MAX;
        }
        let halves = halves(&pixels, usize::try_from(width).ok()?);
        let buffer = RgbaImage::from_raw(width.cast_unsigned(), height.cast_unsigned(), pixels)?;
        Some((RenderImage::new(smallvec![Frame::new(buffer)]), halves))
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: the context is live, and once it is freed mpv no longer calls `on_update`,
        // so the sender it pointed at can go.
        unsafe {
            sys::mpv_render_context_free(self.context);
            drop(Box::from_raw(self.wake));
        }
    }
}

/// The average red, green and blue of the left and right halves of a BGRA frame `width` wide,
/// from a sparse sample of its pixels.
fn halves(pixels: &[u8], width: usize) -> [[f32; 3]; 2] {
    let mut sums = [[0u64; 3]; 2];
    let mut counts = [0u64; 2];
    for row in pixels.chunks_exact(width * 4).step_by(GLOW_STEP) {
        for (column, pixel) in row.as_chunks::<4>().0.iter().enumerate().step_by(GLOW_STEP) {
            let side = usize::from(column * 2 >= width);
            for (channel, sum) in sums[side].iter_mut().enumerate() {
                *sum += u64::from(pixel[2 - channel]);
            }
            counts[side] += 1;
        }
    }
    let mut average = [[0f32; 3]; 2];
    for side in 0..2 {
        for channel in 0..3 {
            average[side][channel] = sums[side][channel] as f32 / counts[side].max(1) as f32;
        }
    }
    average
}

/// mpv's update callback. It runs on an mpv thread and may not call back into mpv, so it only
/// wakes the render thread.
unsafe extern "C" fn on_update(context: *mut c_void) {
    // SAFETY: `context` is the boxed sender `Renderer::new` registered, alive until the render
    // context is freed.
    let wake = unsafe { &*context.cast::<Sender<Wake>>() };
    let _ = wake.send(Wake::Update);
}

/// Turns an mpv status code into a result carrying mpv's own description of the error.
fn check(status: c_int) -> Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: mpv_error_string returns a static NUL-terminated string for any code.
    let message = unsafe { CStr::from_ptr(sys::mpv_error_string(status)) };
    bail!("{}", message.to_string_lossy())
}

/// The yt-dlp to run for a download: the one shipped beside the executable, or else whichever
/// the PATH holds.
pub fn ytdl() -> String {
    bundled_ytdl().unwrap_or_else(|| "yt-dlp".to_owned())
}

/// The folder of the executable when an ffmpeg was shipped in it, which lets yt-dlp join the
/// separate picture and sound of the sharper qualities into one file.
pub fn bundled_ffmpeg() -> Option<String> {
    let name = match cfg!(windows) {
        true => "ffmpeg.exe",
        false => "ffmpeg",
    };
    let folder = std::env::current_exe().ok()?.parent()?.to_path_buf();
    folder
        .join(name)
        .is_file()
        .then(|| folder.to_string_lossy().into_owned())
}

/// The yt-dlp shipped beside the executable, which the Windows installer puts there. Without one
/// mpv looks for yt-dlp on the PATH.
fn bundled_ytdl() -> Option<String> {
    let name = match cfg!(windows) {
        true => "yt-dlp.exe",
        false => "yt-dlp",
    };
    let bundled = std::env::current_exe().ok()?.parent()?.join(name);
    bundled
        .is_file()
        .then(|| bundled.to_string_lossy().into_owned())
}
