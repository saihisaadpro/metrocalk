//! ADR-182 — the sub-engine that turns a rendered cut into a movie file.
//!
//! WHAT THIS CLOSES. ADR-175 gave the engine a way to get a picture out; ADR-177 gave that picture a
//! delivery size that is not the author's window. Both stopped one step short of a deliverable: what
//! came out of a render was 120 numbered PNG files, and the person holding them still had to find,
//! install and drive `ffmpeg` before anybody could watch the thing. ADR-177 wrote that down as owed
//! item 1 — *a sequence is still not a movie* — and it is the last hop of the whole cinematics chain:
//! solve a shot, compose it for a delivery frame, film it at a delivery size, and then hand somebody
//! a file they can double-click.
//!
//! WHY THE OPERATING SYSTEM'S OWN ENCODER, AND NOT A BUNDLED ONE. The three ways to encode H.264 are
//! ship a binary (a licence question, a download, and a second thing to keep current), link a C
//! encoder (a build toolchain on every contributor's machine), or ask the platform. Windows has had a
//! hardware-backed H.264 encoder in the box since Windows 7 — the same one every screen recorder on
//! the machine already uses — so the platform answer costs nothing to ship, nothing to license, and
//! runs on the GPU that just drew the frames. The trait-shaped facade below is the ADR-003 invariant 5
//! discipline applied to it: `windows::` types never cross this module's boundary, so the day this
//! engine encodes on another platform, only the inside of this file changes. A CI gate holds that
//! line (`ci.yml`, the "windows crate stays inside video.rs" step), because "no foreign types in
//! public APIs" is a claim a compiler cannot check.
//!
//! WHAT IS *NOT* ASSUMED. Whether this machine can encode at a given size is **measured, not
//! declared** — [`probe`] builds a real writer at the real size and tears it down again. Windows N
//! editions ship without Media Foundation; the software H.264 encoder tops out at 1920×1088 while the
//! hardware ones reach 4096 and beyond; and a 2160-line scope master is 5162 pixels wide, which is
//! past what most of them will take. Every one of those is a sentence the author reads **before** the
//! click, next to the choice that causes it — never a render that runs for four minutes and then
//! cannot be finalised.

use std::path::Path;

/// What a movie is encoded at. Plain numbers, decided by the caller before anything is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovieSpec {
    /// Pixel width. Even, because 4:2:0 chroma is one sample per 2×2 block and an odd row has no
    /// partner — `render_frame_size` guarantees it.
    pub width: u32,
    /// Pixel height. Even, for the same reason.
    pub height: u32,
    /// Frames per second. The movie's own time base; the samples carry timestamps derived from it, so
    /// a dropped frame shortens the movie rather than shifting everything after it.
    pub fps: u32,
    /// Bits per second the encoder is asked to hold to. See `metrocalk_editor_shell::bitrate_for`.
    pub bitrate: u32,
}

/// Convert one RGBA8 frame to NV12 — the 4:2:0 layout every H.264 encoder takes.
///
/// BT.709 limited range, which is what an HD H.264 stream in an MP4 is read back as; using full range
/// here without saying so in the container is how a render comes back looking washed out in one player
/// and crushed in another. The coefficients are integer, scaled by 2^16, and each triple sums to
/// exactly its own identity — the luma weights to 219/255 so white lands on 235, and both chroma
/// triples to zero so grey lands on 128. That is the arithmetic property the tests below assert, and
/// it is what makes this conversion the same on every machine.
///
/// The chroma sample for a 2×2 block is taken from the block's AVERAGED RGB rather than from averaged
/// per-pixel chroma: a quarter of the multiplies, and the two differ only where a block straddles a
/// hard colour edge, which 4:2:0 is already throwing away.
///
/// `width` and `height` must be even and must match `rgba`; anything else returns an empty buffer
/// rather than reading past the end.
#[must_use]
pub fn rgba_to_nv12(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || rgba.len() < w * h * 4 {
        return Vec::new();
    }
    let mut out = vec![0u8; w * h + w * h / 2];
    let (luma, chroma) = out.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 4;
            let (r, g, b) = (
                i32::from(rgba[p]),
                i32::from(rgba[p + 1]),
                i32::from(rgba[p + 2]),
            );
            // 16 + (219/255)·(0.2126R + 0.7152G + 0.0722B), in 16.16 fixed point.
            luma[y * w + x] = clamp_u8(16 + ((11966 * r + 40254 * g + 4064 * b + 32768) >> 16));
        }
    }
    for by in 0..h / 2 {
        for bx in 0..w / 2 {
            let mut r = 0i32;
            let mut g = 0i32;
            let mut b = 0i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = ((by * 2 + dy) * w + bx * 2 + dx) * 4;
                    r += i32::from(rgba[p]);
                    g += i32::from(rgba[p + 1]);
                    b += i32::from(rgba[p + 2]);
                }
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            let u = 128 + ((-6595 * r - 22186 * g + 28781 * b + 32768) >> 16);
            let v = 128 + ((28784 * r - 26146 * g - 2638 * b + 32768) >> 16);
            let at = (by * w) + bx * 2;
            chroma[at] = clamp_u8(u);
            chroma[at + 1] = clamp_u8(v);
        }
    }
    out
}

fn clamp_u8(v: i32) -> u8 {
    // The conversion above is bounded by construction; this is the one line that says so rather than
    // relying on it, because a single out-of-range sample is a wrapped pixel and not an error anybody
    // would ever trace back to here.
    u8::try_from(v.clamp(0, 255)).unwrap_or(0)
}

/// Whether this machine can encode a movie at `spec`, and the sentence if it cannot.
///
/// MEASURED. It builds the real thing at the real size and throws it away, because every cheaper
/// answer is a guess: the presence of the DLL does not say what the encoder will accept, and the
/// encoder's own limits differ between the software one Windows falls back to and the hardware one on
/// the GPU that just drew the frames.
///
/// # Errors
/// The refusal sentence, addressed to the author, naming the way out.
pub fn probe(spec: MovieSpec) -> Result<(), String> {
    if spec.width == 0 || spec.height == 0 {
        return Err("There is no frame size to encode yet.".into());
    }
    // The DIMENSION refusal belongs to `plan_render`, which states it before anything is created and
    // where a test can hold it without a graphics device. Restating it here would be the same sentence
    // written twice, in two crates, with nothing comparing them — so this asserts the boundary instead
    // and goes on to measure what only the real machine can answer.
    if spec.width > metrocalk_editor_shell::MAX_MOVIE_DIMENSION
        || spec.height > metrocalk_editor_shell::MAX_MOVIE_DIMENSION
    {
        return Err(format!(
            "{} x {} is larger than the {} pixels H.264 encodes reliably. Choose a shorter height, or render a PNG sequence, which has no such limit.",
            spec.width, spec.height, metrocalk_editor_shell::MAX_MOVIE_DIMENSION
        ));
    }
    // REMEMBERED, because the dialog asks on every change of a control and building an encoder is not
    // free — a hardware one spins up the GPU's encode engine. The answer cannot change while the
    // process runs (it is a property of the machine and the size), and the table is at most one row
    // per offered height times rate. Without this, four clicks around the size picker would each stall
    // the engine's heartbeat for as long as an encoder takes to start.
    use std::sync::{Mutex, OnceLock};
    /// One asked-and-answered probe. Named because the table's type is otherwise four generics deep
    /// and says nothing at the point it is declared.
    type Probed = (MovieSpec, Result<(), String>);
    static SEEN: OnceLock<Mutex<Vec<Probed>>> = OnceLock::new();
    let table = SEEN.get_or_init(|| Mutex::new(Vec::new()));
    // The bit rate is derived from the other three, so two specs that differ only there cannot exist;
    // comparing the whole struct keeps this honest if that ever stops being true.
    if let Some((_, answer)) = table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(seen, _)| *seen == spec)
    {
        return answer.clone();
    }
    let answer = platform::probe(spec);
    table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((spec, answer.clone()));
    answer
}

/// A movie being written, one frame at a time.
///
/// Owned by the render job and dropped with it. `finish` is the only way to get a playable file:
/// dropping the writer without it leaves whatever the container had flushed, which is why the job's
/// single exit calls it on every one of its endings.
pub struct MovieWriter {
    inner: platform::Writer,
    spec: MovieSpec,
    frames: u32,
}

impl MovieWriter {
    /// Start a movie at `path`.
    ///
    /// # Errors
    /// A sentence, when the platform cannot encode at this size or the file cannot be created.
    pub fn create(path: &Path, spec: MovieSpec) -> Result<Self, String> {
        Ok(Self {
            inner: platform::Writer::create(path, spec)?,
            spec,
            frames: 0,
        })
    }

    /// Add one RGBA8 frame. Frames are timed from the spec's rate, in order, starting at zero.
    ///
    /// # Errors
    /// A sentence, when the frame is the wrong size or the encoder refused the sample.
    pub fn push_rgba(&mut self, rgba: &[u8]) -> Result<(), String> {
        let nv12 = rgba_to_nv12(rgba, self.spec.width, self.spec.height);
        if nv12.is_empty() {
            return Err(format!(
                "that frame is not the {} x {} the movie was opened at",
                self.spec.width, self.spec.height
            ));
        }
        // EXACT, AND FROM THE INDEX RATHER THAN ACCUMULATED. 24 fps is 416,666.66 hundred-nanosecond
        // ticks; adding a rounded duration per frame drifts a ten-second cut by four milliseconds and
        // a ten-minute one by a quarter of a second. Both edges are computed from the frame number, so
        // the durations absorb the remainder and the last frame lands exactly where it should.
        let tick = |n: u64| -> i64 {
            i64::try_from(n * 10_000_000 / u64::from(self.spec.fps.max(1))).unwrap_or(i64::MAX)
        };
        let start = tick(u64::from(self.frames));
        let end = tick(u64::from(self.frames) + 1);
        self.inner.push(&nv12, start, end - start)?;
        self.frames += 1;
        Ok(())
    }

    /// Close the container so the file is playable, and answer with its size on disk.
    ///
    /// # Errors
    /// A sentence, when the container could not be finalised.
    pub fn finish(self, path: &Path) -> Result<u64, String> {
        self.inner.finish()?;
        Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    }
}

#[cfg(windows)]
mod platform {
    //! Media Foundation, through its sink writer. The only place in the engine that names a
    //! `windows::` type.

    use super::MovieSpec;
    use std::path::Path;
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::{
        eAVEncH264VProfile_High, IMFMediaBuffer, IMFMediaType, IMFSinkWriter, MFCreateMemoryBuffer,
        MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFStartup,
        MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFSTARTUP_NOSOCKET,
        MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
        MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_VERSION,
    };

    /// Pack two 32-bit halves the way every `MFSetAttribute*` helper does. Declared here because those
    /// helpers are C++ inlines with no exported symbol to bind to.
    fn pair(hi: u32, lo: u32) -> u64 {
        (u64::from(hi) << 32) | u64::from(lo)
    }

    /// `MFStartup` once per process. Media Foundation reference-counts its own startup, but calling it
    /// per render would tie the platform's lifetime to a job's, and a second job starting while the
    /// first was tearing down is a race with nothing to gain.
    fn startup() -> Result<(), String> {
        use std::sync::OnceLock;
        static READY: OnceLock<Result<(), String>> = OnceLock::new();
        READY
            .get_or_init(|| {
                // Safety: `MFStartup` takes a version and a flag word and touches nothing of ours. It
                // is called at most once for the life of the process, from inside a `OnceLock`.
                unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.map_err(|e| {
                    format!(
                        "this Windows build has no Media Foundation, so it cannot encode video ({}). Render a PNG sequence instead.",
                        e.message()
                    )
                })
            })
            .clone()
    }

    /// The output (H.264) and input (NV12) media types for `spec`.
    ///
    /// `high` asks for the H.264 **High** profile. It is not decoration: an encoder left to choose
    /// picks **Constrained Baseline** — no B-frames, no CABAC — which is what the first movie this
    /// engine ever wrote came back as, measured out of the file with `ffprobe`. At the same bit rate
    /// that is visibly worse on exactly the content a CAD cutscene is made of: large flat fills with
    /// hard edges. Every H.264 decoder made this century reads High, so it is asked for first and the
    /// caller falls back once if an encoder refuses — trading the whole capability for a quality
    /// setting would be the wrong way round.
    fn media_types(spec: MovieSpec, high: bool) -> Result<(IMFMediaType, IMFMediaType), String> {
        // Safety: every call below is a create-or-set on an object this function owns; the GUID and
        // attribute constants are the crate's own, and no pointer of ours is handed across.
        unsafe {
            let out = windows::Win32::Media::MediaFoundation::MFCreateMediaType()
                .map_err(|e| format!("the video format could not be described: {}", e.message()))?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ok();
            out.SetUINT32(&MF_MT_AVG_BITRATE, spec.bitrate).ok();
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .ok();
            out.SetUINT64(&MF_MT_FRAME_SIZE, pair(spec.width, spec.height))
                .ok();
            out.SetUINT64(&MF_MT_FRAME_RATE, pair(spec.fps.max(1), 1))
                .ok();
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1)).ok();
            if high {
                out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)
                    .ok();
            }

            let input = windows::Win32::Media::MediaFoundation::MFCreateMediaType()
                .map_err(|e| format!("the frame format could not be described: {}", e.message()))?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).ok();
            input
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .ok();
            input
                .SetUINT64(&MF_MT_FRAME_SIZE, pair(spec.width, spec.height))
                .ok();
            input
                .SetUINT64(&MF_MT_FRAME_RATE, pair(spec.fps.max(1), 1))
                .ok();
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1)).ok();
            Ok((out, input))
        }
    }

    /// Build a sink writer at `path` and get as far as accepting frames.
    fn begin(path: &Path, spec: MovieSpec) -> Result<(IMFSinkWriter, u32), String> {
        // HIGH FIRST, THEN WHATEVER THIS ENCODER WILL TAKE. See `media_types`. The retry builds a
        // whole new writer because a sink writer that has already refused a stream is not in a state
        // to be asked a second question, and the first attempt's file is overwritten by the second.
        match begin_with(path, spec, true) {
            Ok(started) => Ok(started),
            Err(_) => begin_with(path, spec, false),
        }
    }

    fn begin_with(
        path: &Path,
        spec: MovieSpec,
        high: bool,
    ) -> Result<(IMFSinkWriter, u32), String> {
        startup()?;
        let url = HSTRING::from(path.as_os_str());
        let (out, input) = media_types(spec, high)?;
        // Safety: `url` outlives the call, the media types are owned above, and the stream index is
        // written into a local. Every failure is turned into a sentence rather than being ignored.
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&url, None, None).map_err(|e| {
                format!(
                    "{} could not be opened for writing: {}",
                    path.display(),
                    e.message()
                )
            })?;
            let stream = writer.AddStream(&out).map_err(|e| {
                format!(
                    "this machine's H.264 encoder will not encode {} x {} at {} fps ({}). Choose a shorter height, or render a PNG sequence.",
                    spec.width, spec.height, spec.fps, e.message()
                )
            })?;
            writer
                .SetInputMediaType(stream, &input, None)
                .map_err(|e| {
                    format!(
                        "this machine's H.264 encoder will not take {} x {} frames ({}). Choose a shorter height, or render a PNG sequence.",
                        spec.width, spec.height, e.message()
                    )
                })?;
            writer
                .BeginWriting()
                .map_err(|e| format!("the movie could not be started: {}", e.message()))?;
            Ok((writer, stream))
        }
    }

    /// Build the real encoder at the real size and throw it away — see [`super::probe`].
    pub fn probe(spec: MovieSpec) -> Result<(), String> {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "metrocalk-encoder-probe-{}x{}-{}.mp4",
            spec.width,
            spec.height,
            std::process::id()
        ));
        let outcome = begin(&path, spec).map(|_| ());
        // The writer is dropped un-finalised on purpose: nothing was written, and the file is only
        // ever a few hundred bytes of container header. Removing it is best-effort — a probe that
        // failed to tidy up must not become a refusal about encoding.
        let _ = std::fs::remove_file(&path);
        outcome
    }

    /// One movie being written.
    pub struct Writer {
        writer: IMFSinkWriter,
        stream: u32,
    }

    impl Writer {
        pub fn create(path: &Path, spec: MovieSpec) -> Result<Self, String> {
            let (writer, stream) = begin(path, spec)?;
            Ok(Self { writer, stream })
        }

        pub fn push(
            &mut self,
            nv12: &[u8],
            start_100ns: i64,
            duration_100ns: i64,
        ) -> Result<(), String> {
            let len = u32::try_from(nv12.len())
                .map_err(|_| "that frame is larger than one media buffer can hold".to_string())?;
            // Safety: the buffer is created here, locked for exactly the length it was created with,
            // and unlocked on every path out. `copy_to` writes `len` bytes into a region the lock just
            // reported as at least `len` long, and the pointer does not escape this block.
            unsafe {
                let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(len)
                    .map_err(|e| format!("a frame buffer could not be made: {}", e.message()))?;
                let mut dst: *mut u8 = std::ptr::null_mut();
                let mut max: u32 = 0;
                buffer
                    .Lock(&mut dst, Some(&mut max), None)
                    .map_err(|e| format!("a frame buffer could not be filled: {}", e.message()))?;
                if dst.is_null() || max < len {
                    let _ = buffer.Unlock();
                    return Err(
                        "the encoder handed back a frame buffer too small for the frame".into(),
                    );
                }
                std::ptr::copy_nonoverlapping(nv12.as_ptr(), dst, nv12.len());
                buffer
                    .Unlock()
                    .map_err(|e| format!("a frame buffer could not be closed: {}", e.message()))?;
                buffer
                    .SetCurrentLength(len)
                    .map_err(|e| format!("a frame length could not be set: {}", e.message()))?;

                let sample = MFCreateSample()
                    .map_err(|e| format!("a frame could not be wrapped: {}", e.message()))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|e| format!("a frame could not be attached: {}", e.message()))?;
                sample
                    .SetSampleTime(start_100ns)
                    .map_err(|e| format!("a frame could not be timed: {}", e.message()))?;
                sample
                    .SetSampleDuration(duration_100ns)
                    .map_err(|e| format!("a frame could not be timed: {}", e.message()))?;
                self.writer
                    .WriteSample(self.stream, &sample)
                    .map_err(|e| format!("a frame could not be encoded: {}", e.message()))?;
            }
            Ok(())
        }

        pub fn finish(self) -> Result<(), String> {
            // Safety: finalising the writer this struct owns, once, as it is consumed.
            unsafe {
                self.writer
                    .Finalize()
                    .map_err(|e| format!("the movie could not be closed: {}", e.message()))
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    //! No encoder here yet. A REFUSAL AND NOT A PANIC, and one that names what the author can do
    //! instead: the sequence path is the whole capability minus the last hop, and it works on every
    //! platform this engine builds for.

    use super::MovieSpec;
    use std::path::Path;

    const UNSUPPORTED: &str =
        "this build of Metrocalk cannot encode video on this platform yet. Render a PNG sequence — it is the same frames, one file each.";

    pub fn probe(_spec: MovieSpec) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }

    pub struct Writer;

    impl Writer {
        pub fn create(_path: &Path, _spec: MovieSpec) -> Result<Self, String> {
            Err(UNSUPPORTED.into())
        }
        pub fn push(&mut self, _nv12: &[u8], _start: i64, _duration: i64) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }
        pub fn finish(self) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        v
    }

    #[test]
    fn nv12_is_the_right_shape() {
        let out = rgba_to_nv12(&flat(4, 2, [0, 0, 0]), 4, 2);
        // Y plane plus a half-height interleaved chroma plane: 4×2 + 4×1.
        assert_eq!(out.len(), 4 * 2 + 4);
    }

    #[test]
    fn white_and_black_land_on_the_studio_swing() {
        // THE PROPERTY THAT MAKES THIS BT.709 LIMITED RANGE and not something adjacent to it. A
        // conversion whose white is 255 instead of 235 produces a movie every player crushes.
        let white = rgba_to_nv12(&flat(2, 2, [255, 255, 255]), 2, 2);
        let black = rgba_to_nv12(&flat(2, 2, [0, 0, 0]), 2, 2);
        assert_eq!(white[0], 235, "white luma");
        assert_eq!(black[0], 16, "black luma");
        // Neither has any colour in it, so both chroma samples must be the neutral 128 EXACTLY — the
        // property the two coefficient triples were rounded to preserve. A drift of one here is a
        // green cast over an entire grey render.
        assert_eq!(&white[4..6], &[128, 128], "white chroma");
        assert_eq!(&black[4..6], &[128, 128], "black chroma");
    }

    #[test]
    fn grey_stays_neutral_at_every_level() {
        for level in 0..=255u8 {
            let out = rgba_to_nv12(&flat(2, 2, [level, level, level]), 2, 2);
            assert_eq!(&out[4..6], &[128, 128], "grey {level} must carry no colour");
        }
    }

    #[test]
    fn primaries_move_chroma_the_way_bt709_says() {
        // Blue is the strongest positive on Cb and negative on Cr; red the reverse. Asserting the
        // DIRECTION rather than the exact number: the numbers are the coefficients' business, but a
        // red and blue swapped anywhere between the readback and the encoder shows up here.
        let red = rgba_to_nv12(&flat(2, 2, [255, 0, 0]), 2, 2);
        let blue = rgba_to_nv12(&flat(2, 2, [0, 0, 255]), 2, 2);
        assert!(red[4] < 128 && red[5] > 128, "red: Cb down, Cr up");
        assert!(blue[4] > 128 && blue[5] < 128, "blue: Cb up, Cr down");
        // And luma follows the 709 weights: green is much brighter than red, which is brighter than
        // blue. A conversion using 601 weights would put red and blue closer together.
        let green = rgba_to_nv12(&flat(2, 2, [0, 255, 0]), 2, 2);
        assert!(green[0] > red[0] && red[0] > blue[0]);
    }

    #[test]
    fn a_frame_of_the_wrong_size_is_refused_rather_than_read_past() {
        assert!(rgba_to_nv12(&flat(4, 4, [0, 0, 0]), 8, 8).is_empty());
        // Odd dimensions have no 2×2 block to sample chroma from.
        assert!(rgba_to_nv12(&flat(3, 3, [0, 0, 0]), 3, 3).is_empty());
    }

    #[test]
    fn chroma_is_taken_per_block_and_not_per_frame() {
        // Left half red, right half blue, 4×2. The two chroma columns must disagree — a conversion
        // that averaged the whole frame would produce two identical neutral samples.
        let mut rgba = Vec::new();
        for _ in 0..2 {
            rgba.extend_from_slice(&[255, 0, 0, 255]);
            rgba.extend_from_slice(&[255, 0, 0, 255]);
            rgba.extend_from_slice(&[0, 0, 255, 255]);
            rgba.extend_from_slice(&[0, 0, 255, 255]);
        }
        let out = rgba_to_nv12(&rgba, 4, 2);
        let chroma = &out[8..12];
        assert!(chroma[0] < 128 && chroma[2] > 128, "left red, right blue");
    }

    /// The top-level box types of an ISO base-media file, in order, with their sizes.
    ///
    /// Written here rather than pulled in: an MP4 is a length-prefixed box list, six lines of it are
    /// enough to answer the only question this test has, and a demuxer dependency to read six lines
    /// would be a second thing to keep current for the sake of one assertion.
    fn mp4_boxes(bytes: &[u8]) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
            let kind = String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned();
            // `0` means "to the end of the file"; `1` means a 64-bit size follows the type.
            let (size, header) = match size {
                0 => (bytes.len() as u64 - at as u64, 8usize),
                1 if at + 16 <= bytes.len() => (
                    u64::from_be_bytes(bytes[at + 8..at + 16].try_into().expect("8 bytes")),
                    16,
                ),
                n => (u64::from(n), 8),
            };
            out.push((kind, size));
            if size < header as u64 {
                break;
            }
            at += usize::try_from(size).unwrap_or(bytes.len());
        }
        out
    }

    /// THE ONE TEST THAT ENCODES. Local-only by design, and `#[ignore]`d rather than silently
    /// skipped: it needs a working H.264 encoder, which every consumer Windows has and the Windows
    /// **Server** image GitHub's `windows-latest` runner uses does not — Media Foundation is an
    /// optional feature there. A test that passed by finding no encoder would be a false green about
    /// the one thing this module exists to do.
    ///
    /// Run it with `cargo test --manifest-path editor-shell/src-tauri/Cargo.toml -- --ignored
    /// writes_a_real_mp4`. Its result is recorded in the ADR-182 report; the shipping gate is the
    /// `.exe` render suite, which is local-only for the same class of reason.
    #[test]
    #[ignore = "needs a platform H.264 encoder; absent on the Windows Server CI image (see doc above)"]
    fn writes_a_real_mp4_whose_container_was_actually_closed() {
        let (w, h, fps) = (640u32, 360u32, 24u32);
        let spec = MovieSpec {
            width: w,
            height: h,
            fps,
            bitrate: metrocalk_editor_shell::bitrate_for(w, h, fps),
        };
        probe(spec).expect("this machine has an H.264 encoder");
        let path =
            std::env::temp_dir().join(format!("metrocalk-video-test-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut movie = MovieWriter::create(&path, spec).expect("opens");
        // A MOVING PICTURE, not 24 copies of one frame: a codec handed identical frames emits almost
        // nothing after the first, and an `mdat` of 300 bytes would satisfy a size assertion while
        // proving that nothing was encoded.
        for i in 0..24u32 {
            let mut frame = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                for x in 0..w {
                    let r = u8::try_from((x + i * 7) % 256).unwrap_or(0);
                    let g = u8::try_from((y + i * 3) % 256).unwrap_or(0);
                    frame.extend_from_slice(&[r, g, 128, 255]);
                }
            }
            movie.push_rgba(&frame).expect("encodes a frame");
        }
        let bytes = movie.finish(&path).expect("finalises");

        let file = std::fs::read(&path).expect("the movie is on disk");
        let boxes = mp4_boxes(&file);
        let kinds: Vec<&str> = boxes.iter().map(|(k, _)| k.as_str()).collect();
        // `ftyp` first — this is an MP4 and not a file with an .mp4 on the end.
        assert_eq!(kinds.first(), Some(&"ftyp"), "boxes: {kinds:?}");
        // `moov` is THE ASSERTION. The sink writer only writes the movie header on `Finalize`, so a
        // container that was dropped rather than closed has media in it and no index — a file every
        // operating system lists at the right size and no player will open. Nothing else in this
        // module can tell those two apart.
        assert!(
            kinds.contains(&"moov"),
            "the container was closed: {kinds:?}"
        );
        let mdat = boxes
            .iter()
            .find(|(k, _)| k == "mdat")
            .expect("there is encoded media")
            .1;
        assert!(mdat > 10_000, "24 moving frames encoded to {mdat} bytes");
        // The `moov` box carries the profile in its `avcC` record — three bytes after the tag: the
        // AVC profile indication, its compatibility flags, and the level. `100` is High and `66` is
        // Constrained Baseline, which is what an encoder left to choose picks and what the first
        // movie this engine ever wrote came back as. Read out of the FILE rather than trusted from
        // the attribute that asked for it, because `SetUINT32` returning `Ok` says the media type
        // accepted the value and nothing at all about what the encoder did with it.
        let avcc = file
            .windows(4)
            .position(|w| w == b"avcC")
            .expect("the movie header carries an AVC decoder configuration record");
        let profile = file[avcc + 5];
        assert!(
            profile == 100 || profile == 77,
            "asked for High (100), accepted Main (77); got profile {profile}"
        );
        assert_eq!(bytes, file.len() as u64, "the ledger's size is the file's");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_size_past_the_h264_ceiling_is_refused_by_name_before_anything_is_created() {
        // 2160 lines at 2.39:1 is 5162 wide — the one size in the picker's own matrix that H.264 will
        // not take. The sentence must name the number AND the way out, because "unsupported" leaves
        // the author with a render they cannot make and no idea which control to change.
        let why = probe(MovieSpec {
            width: 5162,
            height: 2160,
            fps: 24,
            bitrate: metrocalk_editor_shell::bitrate_for(5162, 2160, 24),
        })
        .expect_err("5162 is past the ceiling");
        assert!(why.contains("5162"), "names the size: {why}");
        assert!(why.contains("PNG sequence"), "names the way out: {why}");
    }
}
