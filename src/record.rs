//! Screen video recording, done by the FFmpeg bundled with the program (`ffmpeg` folder next to the
//! executable).
//!
//! During the recording FFmpeg captures only the image of the area (`gdigrab`), while SnapKit
//! records the audio separately ([`crate::audio`]). When it stops, the two are joined into an MP4
//! (H.264 + AAC). Recording everything live in the same process made the image drop frames.
//!
//! The alignment is done at the end: video and audio stop at the same moment, but the audio starts
//! a little earlier (FFmpeg takes a few tenths of a second to start). The excess at the start of
//! the audio is dropped when joining.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::AudioRecording;
use crate::platform::PxRect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub system_audio: bool,
    pub microphone: bool,
    pub fps: u32,
    pub quality: crate::config::VideoQuality,
    pub draw_mouse: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            system_audio: true,
            microphone: false,
            fps: 30,
            quality: crate::config::VideoQuality::High,
            draw_mouse: true,
        }
    }
}

/// Path of the bundled `ffmpeg` (`ffmpeg.exe` on Windows), if it exists.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("SNAPKIT_FFMPEG_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(exe_dir.join("ffmpeg"));
    }
    #[cfg(debug_assertions)]
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("ffmpeg"));
    candidates.into_iter().map(|d| d.join(crate::platform::exe_name("ffmpeg"))).find(|p| p.is_file())
}

fn ffmpeg_command(ffmpeg: &Path) -> Command {
    let mut cmd = crate::platform::background_command(ffmpeg);
    // Bundled `ffmpeg.exe` embeds `<dpiAwareness>PerMonitorV2</dpiAwareness>` in its manifest;
    // clearing `__COMPAT_LAYER` ensures an inherited `HighDpiAware` AppCompat shim never downgrades
    // it to single-monitor System DPI on mixed-scaling multi-monitor setups.
    cmd.env_remove("__COMPAT_LAYER");
    cmd
}

/// FFmpeg command line to record the image of `rect` to `out`.
fn capture_args(rect: PxRect, options: Options, out: &Path) -> Vec<String> {
    // H.264 requires an even width and height.
    let (w, h) = ((rect.2 - rect.0) & !1, (rect.3 - rect.1) & !1);
    let fps = options.fps.clamp(10, 60);
    // Bit rate proportional to the area (~8 Mbit/s at 1920×1080), scaled by quality preset.
    let base_kbps = (w as i64 * h as i64) / 260;
    let kbps = ((base_kbps as f64 * options.quality.bitrate_factor()).round() as i64).clamp(1000, 35_000);
    let draw_mouse = if options.draw_mouse { "1" } else { "0" };
    let mut a: Vec<String> = [
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-thread_queue_size",
        "1024",
        "-f",
        "gdigrab",
        "-draw_mouse",
        draw_mouse,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut add = |key: &str, value: String| a.extend([key.to_owned(), value]);
    add("-framerate", fps.to_string());
    add("-offset_x", rect.0.to_string());
    add("-offset_y", rect.1.to_string());
    add("-video_size", format!("{w}x{h}"));
    add("-i", "desktop".to_owned());
    add("-c:v", "libopenh264".to_owned());
    add("-b:v", format!("{kbps}k"));
    add("-pix_fmt", "yuv420p".to_owned());
    add("-movflags", "+faststart".to_owned());
    a.push(out.to_string_lossy().into_owned());
    a
}

/// Command line to join the video and the audio, dropping `skip` from the start of the audio.
fn mux_args(video: &Path, audio: &Path, skip: Duration, out: &Path) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut a: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(path(video));
    a.extend(["-ss".to_owned(), format!("{:.3}", skip.as_secs_f64()), "-i".to_owned(), path(audio)]);
    // The image is copied without re-encoding; only the audio is converted to AAC.
    for s in ["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest", "-movflags", "+faststart"] {
        a.push(s.to_owned());
    }
    a.push(path(out));
    a
}

/// Reads the length from FFmpeg's description of a file: `Duration: 00:01:02.50, …`.
fn parse_duration(description: &str) -> Option<Duration> {
    let start = description.find("Duration: ")? + "Duration: ".len();
    let value = description[start..].split(',').next()?.trim();
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    Some(Duration::from_secs_f64(hours * 3600.0 + minutes * 60.0 + seconds))
}

fn media_duration(ffmpeg: &Path, file: &Path) -> Option<Duration> {
    let output = ffmpeg_command(ffmpeg).args(["-hide_banner", "-i"]).arg(file).output().ok()?;
    // The description goes to the error output (FFmpeg complains about the missing output file).
    parse_duration(&String::from_utf8_lossy(&output.stderr))
}

/// Recordings folder: the user's `Videos\SnapKit` (`Movies/SnapKit` on macOS).
pub fn videos_dir() -> PathBuf {
    crate::platform::movies_dir().join("SnapKit")
}

/// A recording in progress.
pub struct Recorder {
    ffmpeg: PathBuf,
    child: Child,
    stderr_reader: Option<std::thread::JoinHandle<String>>,
    audio: Option<AudioRecording>,
    /// Final file and the two temporary ones (image and audio).
    path: PathBuf,
    video_part: PathBuf,
    audio_part: PathBuf,
    started: Instant,
    stopped: bool,
    /// Warnings about audio that couldn't be included.
    pub warnings: Vec<String>,
}

impl Recorder {
    /// Starts recording the area `rect` (screen coordinates) to the file `dir\name.mp4`.
    pub fn start(rect: PxRect, options: Options, dir: &Path, name: &str) -> Result<Self, String> {
        let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
        if rect.2 - rect.0 < 16 || rect.3 - rect.1 < 16 {
            return Err(t!("área pequena demais para gravar", "area too small to record").to_owned());
        }
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{name}.mp4"));
        let video_part = dir.join(format!("{name}.video.mp4"));
        let audio_part = dir.join(format!("{name}.audio.wav"));

        // Audio that fails doesn't stop the recording: it goes on without that source, with a
        // warning.
        let (audio, warnings) = if options.system_audio || options.microphone {
            AudioRecording::start(options.system_audio, options.microphone, &audio_part)
        } else {
            (None, Vec::new())
        };

        let child = ffmpeg_command(&ffmpeg)
            .args(capture_args(rect, options, &video_part))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match child {
            Ok(mut child) => {
                crate::platform::end_with_us(&child);
                // Drain stderr in the background so a full pipe buffer can never block FFmpeg.
                let stderr_reader = child.stderr.take().map(|mut stderr| {
                    std::thread::spawn(move || {
                        let mut buf = String::new();
                        let _ = stderr.read_to_string(&mut buf);
                        buf
                    })
                });
                Ok(Self {
                    ffmpeg,
                    child,
                    stderr_reader,
                    audio,
                    path,
                    video_part,
                    audio_part,
                    started: Instant::now(),
                    stopped: false,
                    warnings,
                })
            }
            Err(e) => {
                if let Some(a) = audio {
                    let _ = a.stop();
                    let _ = std::fs::remove_file(&audio_part);
                }
                Err(tf!("não foi possível iniciar o FFmpeg: {e}", "could not start FFmpeg: {e}"))
            }
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Did FFmpeg exit on its own? (That only happens on error.)
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Ends the recording, joins image and audio and returns the final file.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        self.stopped = true;
        // "q" on standard input asks FFmpeg to close the file properly; dropping stdin closes the
        // pipe so FFmpeg sees EOF as well.
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        // The audio stops at the same moment: that's what lets the two be aligned at the end.
        let audio = self.audio.take().map(AudioRecording::stop);

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        let errors = self
            .stderr_reader
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        if !std::fs::metadata(&self.video_part).is_ok_and(|m| m.len() > 0) {
            let _ = std::fs::remove_file(&self.video_part);
            let _ = std::fs::remove_file(&self.audio_part);
            return Err(tf!("a gravação não gerou arquivo. {}", "the recording produced no file. {}", errors.trim()));
        }

        let result = match audio {
            Some(Ok((wav, audio_len))) => self.mux(&wav, audio_len),
            // No audio (not requested, or it failed): the image file is already the result.
            _ => std::fs::rename(&self.video_part, &self.path).map_err(|e| e.to_string()),
        };
        let _ = std::fs::remove_file(&self.audio_part);
        match result {
            Ok(()) => Ok(self.path.clone()),
            // If joining fails, the recorded image isn't lost: it just has no sound.
            Err(e) => match std::fs::rename(&self.video_part, &self.path) {
                Ok(()) => Ok(self.path.clone()),
                Err(_) => {
                    let _ = std::fs::remove_file(&self.video_part);
                    Err(e)
                }
            },
        }
    }

    fn mux(&self, wav: &Path, audio_len: Duration) -> Result<(), String> {
        let video_len = media_duration(&self.ffmpeg, &self.video_part).ok_or(t!("duração do vídeo desconhecida", "unknown video length"))?;
        let skip = audio_len.saturating_sub(video_len);
        let output = ffmpeg_command(&self.ffmpeg)
            .args(mux_args(&self.video_part, wav, skip, &self.path))
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() && self.path.is_file() {
            let _ = std::fs::remove_file(&self.video_part);
            Ok(())
        } else {
            let _ = std::fs::remove_file(&self.path);
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        if let Some(audio) = self.audio.take() {
            let _ = audio.stop();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        let _ = std::fs::remove_file(&self.video_part);
        let _ = std::fs::remove_file(&self.audio_part);
    }
}

/// A recording that can be paused. FFmpeg can't pause a screen capture, so each stretch between
/// pauses is a separate recording; when it stops, the stretches are joined without re-encoding.
pub struct Session {
    ffmpeg: PathBuf,
    rect: PxRect,
    options: Options,
    dir: PathBuf,
    name: String,
    /// Stretches already closed, in order.
    parts: Vec<PathBuf>,
    current: Option<Recorder>,
    /// Total time of the stretches already closed.
    recorded: Duration,
    stopped: bool,
    /// Warnings about audio that couldn't be included.
    pub warnings: Vec<String>,
}

impl Session {
    /// Starts recording the area `rect` (screen coordinates); the result will be `dir\name.mp4`.
    pub fn start(rect: PxRect, options: Options, dir: &Path, name: &str) -> Result<Self, String> {
        let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
        let mut session = Self {
            ffmpeg,
            rect,
            options,
            dir: dir.to_path_buf(),
            name: name.to_owned(),
            parts: Vec::new(),
            current: None,
            recorded: Duration::ZERO,
            stopped: false,
            warnings: Vec::new(),
        };
        session.resume()?;
        Ok(session)
    }

    pub fn is_paused(&self) -> bool {
        self.current.is_none()
    }

    /// Closes the stretch in progress. The screen isn't recorded until [`Self::resume`].
    pub fn pause(&mut self) -> Result<(), String> {
        let Some(recorder) = self.current.take() else { return Ok(()) };
        self.recorded += recorder.elapsed();
        self.parts.push(recorder.stop()?);
        Ok(())
    }

    /// Starts a new stretch.
    pub fn resume(&mut self) -> Result<(), String> {
        if self.current.is_some() {
            return Ok(());
        }
        let part = format!("{}.part{}", self.name, self.parts.len() + 1);
        let recorder = Recorder::start(self.rect, self.options, &self.dir, &part)?;
        for w in &recorder.warnings {
            if !self.warnings.contains(w) {
                self.warnings.push(w.clone());
            }
        }
        self.current = Some(recorder);
        Ok(())
    }

    /// Time recorded, not counting the pauses.
    pub fn elapsed(&self) -> Duration {
        self.recorded + self.current.as_ref().map_or(Duration::ZERO, Recorder::elapsed)
    }

    /// Did FFmpeg exit on its own? (That only happens on error.)
    pub fn has_exited(&mut self) -> bool {
        self.current.as_mut().is_some_and(Recorder::has_exited)
    }

    /// Ends the recording, joins the stretches and returns the final file.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        // An error in the last stretch doesn't throw away the earlier ones.
        let last = self.pause();
        self.stopped = true;
        let out = self.dir.join(format!("{}.mp4", self.name));
        match self.parts.as_slice() {
            [] => Err(last.err().unwrap_or_else(|| t!("nada foi gravado", "nothing was recorded").to_owned())),
            [only] => std::fs::rename(only, &out).map(|()| out).map_err(|e| e.to_string()),
            parts => {
                let list = self.dir.join(format!("{}.parts.txt", self.name));
                std::fs::write(&list, concat_list(parts)).map_err(|e| e.to_string())?;
                let mut output = ffmpeg_command(&self.ffmpeg)
                    .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
                    .arg(&list)
                    .args(["-c", "copy", "-movflags", "+faststart"])
                    .arg(&out)
                    .output()
                    .map_err(|e| e.to_string())?;
                if !output.status.success() {
                    // If one stretch had audio and another fell back to video-only, retry joining
                    // the video stream alone rather than failing the whole recording.
                    let _ = std::fs::remove_file(&out);
                    if let Ok(fallback) = ffmpeg_command(&self.ffmpeg)
                        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
                        .arg(&list)
                        .args(["-map", "0:v", "-c:v", "copy", "-movflags", "+faststart"])
                        .arg(&out)
                        .output()
                        && fallback.status.success()
                    {
                        output = fallback;
                    }
                }
                let _ = std::fs::remove_file(&list);
                if output.status.success() && out.is_file() {
                    for part in parts {
                        let _ = std::fs::remove_file(part);
                    }
                    Ok(out)
                } else {
                    let _ = std::fs::remove_file(&out);
                    Err(tf!(
                        "não foi possível emendar os trechos (eles continuam na pasta). {}",
                        "could not join the clips (they are still in the folder). {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        drop(self.current.take());
        for part in self.parts.drain(..) {
            let _ = std::fs::remove_file(part);
        }
    }
}

/// List of files in the format FFmpeg uses for joining: one `file '…'` line per stretch.
fn concat_list(parts: &[PathBuf]) -> String {
    parts.iter().map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', r"'\''"))).collect()
}

/// Metadata probed from a video file via FFmpeg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub fps: f64,
    pub has_audio: bool,
}

fn parse_video_info(description: &str) -> Option<VideoInfo> {
    let duration = parse_duration(description)?.as_secs_f64();
    let video_line = description.lines().find(|l| l.contains("Video:"))?;
    let mut dims = None;
    for token in video_line.split([',', ' ', '[', ']', '(', ')']) {
        if let Some((w_str, h_str)) = token.split_once('x')
            && !w_str.is_empty()
            && !h_str.is_empty()
            && !w_str.starts_with('0')
            && w_str.bytes().all(|b| b.is_ascii_digit())
            && h_str.bytes().all(|b| b.is_ascii_digit())
            && let (Ok(w), Ok(h)) = (w_str.parse::<u32>(), h_str.parse::<u32>())
            && w > 0
            && h > 0
        {
            dims = Some((w, h));
            break;
        }
    }
    let (width, height) = dims?;
    let fps = video_line
        .split(" fps")
        .next()
        .and_then(|before| before.rsplit(',').next())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(30.0);
    let has_audio = description.lines().any(|l| l.contains("Audio:"));
    Some(VideoInfo { width, height, duration, fps, has_audio })
}

/// Probes dimensions, duration, frame rate, and audio presence of a video file.
pub fn probe(video: &Path) -> Option<VideoInfo> {
    let ffmpeg = find_ffmpeg()?;
    let output = ffmpeg_command(&ffmpeg).args(["-hide_banner", "-i"]).arg(video).output().ok()?;
    parse_video_info(&String::from_utf8_lossy(&output.stderr))
}

fn extract_frame_with(
    ffmpeg: &Path,
    video: &Path,
    at_secs: f64,
    scale_height: Option<u32>,
) -> Result<image::RgbaImage, String> {
    let run = |secs: f64| -> std::io::Result<std::process::Output> {
        let mut cmd = ffmpeg_command(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{:.3}", secs.max(0.0)), "-i"])
            .arg(video);
        if let Some(h) = scale_height {
            cmd.args(["-vf", &format!("scale=-2:{}", h.max(32))]);
        }
        cmd.args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    };
    let mut output = run(at_secs).map_err(|e| e.to_string())?;
    if output.stdout.is_empty() && at_secs > 0.0 {
        output = run(0.0).map_err(|e| e.to_string())?;
    }
    if output.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    image::load_from_memory(&output.stdout).map(|img| img.to_rgba8()).map_err(|e| e.to_string())
}

/// Extracts a single RGBA frame from `video` at `at_secs`.
pub fn extract_frame(video: &Path, at_secs: f64) -> Result<image::RgbaImage, String> {
    let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
    extract_frame_with(&ffmpeg, video, at_secs, None)
}

/// Fast single-frame RGBA extraction using rawvideo pipe instead of PNG encode/decode when
/// dimensions `(width, height)` are known; falls back to [`extract_frame`] otherwise.
pub fn extract_frame_fast(
    video: &Path,
    at_secs: f64,
    width: u32,
    height: u32,
) -> Result<image::RgbaImage, String> {
    if width == 0 || height == 0 {
        return extract_frame(video, at_secs);
    }
    let ffmpeg = find_ffmpeg().ok_or(t!(
        "o FFmpeg não foi encontrado (pasta ffmpeg)",
        "FFmpeg was not found (ffmpeg folder)"
    ))?;
    let need = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .unwrap_or(0);
    if need == 0 {
        return extract_frame_with(&ffmpeg, video, at_secs, None);
    }

    let run_raw = |secs: f64| -> Option<image::RgbaImage> {
        let mut child = ffmpeg_command(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &format!("{:.3}", secs.max(0.0)),
                "-i",
            ])
            .arg(video)
            .args([
                "-frames:v",
                "1",
                "-s",
                &format!("{width}x{height}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        crate::platform::end_with_us(&child);
        let mut stdout = child.stdout.take()?;
        let mut buf = vec![0u8; need];
        let ok = stdout.read_exact(&mut buf).is_ok();
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();
        if ok {
            image::RgbaImage::from_raw(width, height, buf)
        } else {
            None
        }
    };

    if let Some(img) = run_raw(at_secs) {
        return Ok(img);
    }
    if at_secs > 0.0
        && let Some(img) = run_raw(0.0)
    {
        return Ok(img);
    }
    extract_frame_with(&ffmpeg, video, at_secs, None)
}

fn stream_args(
    video: &Path,
    start_secs: f64,
    end_secs: Option<f64>,
    info: VideoInfo,
    max_width: u32,
) -> (Vec<String>, u32, u32, u32) {
    let w0 = info.width.max(2);
    let h0 = info.height.max(2);
    let (pw, ph) = if max_width > 0 && w0 > max_width {
        let scale = max_width as f64 / w0 as f64;
        let sw = ((w0 as f64 * scale).round() as u32 & !1).max(2);
        let sh = ((h0 as f64 * scale).round() as u32 & !1).max(2);
        (sw, sh)
    } else {
        ((w0 & !1).max(2), (h0 & !1).max(2))
    };
    let fps = info.fps.round().clamp(15.0, 60.0) as u32;
    let start = start_secs.max(0.0);
    let mut a = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-ss".to_owned(),
        format!("{start:.3}"),
    ];
    if let Some(end) = end_secs
        && end > start
    {
        a.extend(["-to".to_owned(), format!("{end:.3}")]);
    }
    a.extend([
        "-i".to_owned(),
        video.to_string_lossy().into_owned(),
        "-vf".to_owned(),
        format!("scale={pw}:{ph}:flags=fast_bilinear"),
        "-r".to_owned(),
        fps.to_string(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "rgba".to_owned(),
        "-".to_owned(),
    ]);
    (a, pw, ph, fps)
}

/// Continuous raw-RGBA video frame stream decoded by a single background `ffmpeg` child process.
pub struct FrameStream {
    child: Child,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<(f64, image::RgbaImage)>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    stderr_reader: Option<std::thread::JoinHandle<()>>,
    stopped: bool,
}

impl FrameStream {
    /// Starts streaming scaled raw-RGBA frames from `video` starting at `start_secs` up to `end_secs`,
    /// paced in real time according to `info.fps` and `speed`.
    pub fn start(
        video: &Path,
        start_secs: f64,
        end_secs: Option<f64>,
        info: VideoInfo,
        speed: f64,
        max_width: u32,
        on_frame: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        let ffmpeg = find_ffmpeg().ok_or(t!(
            "o FFmpeg não foi encontrado (pasta ffmpeg)",
            "FFmpeg was not found (ffmpeg folder)"
        ))?;
        let start = start_secs.max(0.0);
        let (args, pw, ph, fps) = stream_args(video, start, end_secs, info, max_width);
        let frame_bytes = (pw as usize) * (ph as usize) * 4;
        let eff_speed = speed.clamp(0.25, 4.0);

        let mut child = ffmpeg_command(&ffmpeg)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| tf!("não foi possível iniciar o FFmpeg: {e}", "could not start FFmpeg: {e}"))?;
        crate::platform::end_with_us(&child);

        let stderr_reader = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut sink = [0u8; 1024];
                while let Ok(n) = stderr.read(&mut sink) {
                    if n == 0 {
                        break;
                    }
                }
            })
        });

        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let latest: Arc<Mutex<Option<(f64, image::RgbaImage)>>> = Arc::new(Mutex::new(None));

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "FFmpeg stdout unavailable".to_owned())?;
        let stop_clone = stop.clone();
        let finished_clone = finished.clone();
        let latest_clone = latest.clone();

        let reader = std::thread::spawn(move || {
            let mut stream_started: Option<Instant> = None;
            let frame_interval = 1.0 / (fps as f64 * eff_speed);
            let max_lag = Duration::from_secs_f64(frame_interval);
            let mut frame_idx: u64 = 0;
            let mut skipped_in_a_row: u32 = 0;
            let mut scratch = vec![0u8; frame_bytes];

            while !stop_clone.load(Ordering::Relaxed) {
                let ts = start + (frame_idx as f64) / (fps as f64);
                if let Some(end) = end_secs
                    && ts > end + 1e-4
                {
                    break;
                }
                if stdout.read_exact(&mut scratch).is_err() {
                    break;
                }
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                let started = *stream_started.get_or_insert_with(Instant::now);
                let target = started + Duration::from_secs_f64(frame_idx as f64 * frame_interval);
                let now = Instant::now();

                if target > now {
                    // Ahead of schedule: sleep in short slices so stop() wakes quickly.
                    while !stop_clone.load(Ordering::Relaxed) {
                        let cur = Instant::now();
                        if cur >= target {
                            break;
                        }
                        let rem = (target - cur).min(Duration::from_millis(12));
                        std::thread::sleep(rem);
                    }
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }
                } else if frame_idx > 0
                    && now.duration_since(target) > max_lag
                    && skipped_in_a_row < 3
                {
                    // Behind schedule by more than 1 frame: advance pipe without publishing
                    // so playback stays real-time without building latency.
                    skipped_in_a_row += 1;
                    frame_idx += 1;
                    continue;
                }

                skipped_in_a_row = 0;
                if let Some(img) = image::RgbaImage::from_raw(pw, ph, scratch.clone()) {
                    *latest_clone.lock().unwrap() = Some((ts, img));
                    on_frame();
                }
                frame_idx += 1;
            }

            if !stop_clone.load(Ordering::Relaxed) {
                finished_clone.store(true, Ordering::SeqCst);
                on_frame();
            }
        });

        Ok(Self {
            child,
            stop,
            finished,
            latest,
            reader: Some(reader),
            stderr_reader,
            stopped: false,
        })
    }

    /// Takes the most recently decoded `(timestamp_secs, RgbaImage)` frame, if a new one arrived.
    pub fn take_frame(&self) -> Option<(f64, image::RgbaImage)> {
        self.latest.lock().unwrap().take()
    }

    /// Returns true once the stream has reached `end_secs` or EOF.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Stops the background FFmpeg process and cleans up reader threads.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.child.kill();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_reader.take() {
            let _ = handle.join();
        }
        let _ = self.child.wait();
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Extracts `count` evenly spaced thumbnail frames scaled to `thumb_height` pixels tall.
pub fn extract_filmstrip(video: &Path, count: usize, thumb_height: u32) -> Vec<image::RgbaImage> {
    let Some(ffmpeg) = find_ffmpeg() else { return Vec::new() };
    let duration = probe(video).map(|i| i.duration).unwrap_or(0.0);
    let n = count.max(1);
    let h = thumb_height.max(32);
    let mut frames = Vec::with_capacity(n);
    for i in 0..n {
        let t = if duration <= 0.0 || n == 1 {
            0.0
        } else {
            ((i as f64 + 0.5) / n as f64 * duration).min((duration - 0.05).max(0.0))
        };
        if let Ok(frame) = extract_frame_with(&ffmpeg, video, t, Some(h)) {
            frames.push(frame);
        }
    }
    frames
}

/// Length of a video already recorded.
pub fn video_duration(video: &Path) -> Option<Duration> {
    media_duration(&find_ffmpeg()?, video)
}

/// Options for editing/exporting a video (trim, speed, mute, crop, resize).
#[derive(Clone, Debug, PartialEq)]
pub struct VideoEditOptions {
    pub start: f64,
    pub end: Option<f64>,
    pub speed: f64,
    pub mute: bool,
    /// Optional crop `(x, y, width, height)` in video pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
    /// Maximum output width (`0` keeps original/cropped width).
    pub max_width: u32,
}

impl Default for VideoEditOptions {
    fn default() -> Self {
        Self { start: 0.0, end: None, speed: 1.0, mute: false, crop: None, max_width: 0 }
    }
}

/// Public wrapper around [`atempo_filter`] used by [`crate::audio::AudioPlayer`].
pub fn atempo_filter_str(speed: f64) -> String {
    atempo_filter(speed)
}

/// Computes normalized (`0.0..=1.0`) peak/RMS waveform bars from mono float samples.
pub fn waveform_buckets(samples: &[f32], buckets: usize) -> Vec<f32> {
    let n = buckets.max(1);
    if samples.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0.0f32; n];
    let len = samples.len();
    for (b, slot) in out.iter_mut().enumerate() {
        let s0 = b * len / n;
        let s1 = ((b + 1) * len / n).max(s0 + 1).min(len);
        let mut sum_sq = 0.0f32;
        let mut peak = 0.0f32;
        for &s in &samples[s0..s1] {
            let v = if s.is_finite() { s.abs().min(1.5) } else { 0.0 };
            sum_sq += v * v;
            peak = peak.max(v);
        }
        let rms = (sum_sq / (s1 - s0) as f32).sqrt();
        *slot = rms * 0.65 + peak * 0.35;
    }
    let max_val = out.iter().copied().fold(0.0f32, f32::max);
    if max_val > 1e-4 {
        let norm = max_val.max(0.08);
        for v in &mut out {
            *v = (*v / norm).clamp(0.0, 1.0);
        }
    } else {
        out.fill(0.0);
    }
    out
}

/// Decodes the audio stream of `video` at 4 kHz mono and returns `buckets` normalized amplitude
/// values (`0.0..=1.0`) for timeline visualization.
pub fn extract_waveform(video: &Path, buckets: usize) -> Vec<f32> {
    let Some(ffmpeg) = find_ffmpeg() else { return Vec::new() };
    let Ok(output) = ffmpeg_command(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .args(["-vn", "-ac", "1", "-ar", "4000", "-f", "f32le", "-"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() || output.stdout.len() < 4 {
        return Vec::new();
    }
    let (chunks, _) = output.stdout.as_chunks::<4>();
    let samples: Vec<f32> = chunks.iter().map(|b| f32::from_le_bytes(*b)).collect();
    waveform_buckets(&samples, buckets)
}

fn atempo_filter(speed: f64) -> String {
    let mut remaining = speed.clamp(0.25, 4.0);
    let mut stages = Vec::new();
    while remaining > 2.0 + 1e-4 {
        stages.push("atempo=2.0".to_owned());
        remaining /= 2.0;
    }
    while remaining < 0.5 - 1e-4 {
        stages.push("atempo=0.5".to_owned());
        remaining /= 0.5;
    }
    if (remaining - 1.0).abs() > 0.01 || stages.is_empty() {
        stages.push(format!("atempo={remaining:.3}"));
    }
    stages.join(",")
}

fn edit_video_args(video: &Path, opts: &VideoEditOptions, out: &Path) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut a: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(path(video));
    if opts.start > 0.0 {
        a.extend(["-ss".to_owned(), format!("{:.3}", opts.start)]);
    }
    if let Some(end) = opts.end {
        a.extend(["-to".to_owned(), format!("{end:.3}")]);
    }

    let speed = opts.speed.clamp(0.25, 4.0);
    let mut vf_parts = Vec::new();
    if let Some((x, y, w, h)) = opts.crop {
        let cw = (w & !1).max(2);
        let ch = (h & !1).max(2);
        vf_parts.push(format!("crop={cw}:{ch}:{x}:{y}"));
    }
    if opts.max_width > 0 {
        let mw = (opts.max_width & !1).max(2);
        vf_parts.push(format!("scale='min({mw},iw)':-2"));
    }
    if (speed - 1.0).abs() > 0.01 {
        vf_parts.push(format!("setpts={:.4}*PTS", 1.0 / speed));
    }
    if !vf_parts.is_empty() {
        a.extend(["-vf".to_owned(), vf_parts.join(",")]);
    }

    for s in ["-c:v", "libopenh264", "-b:v", "8000k", "-pix_fmt", "yuv420p"] {
        a.push(s.to_owned());
    }
    if opts.mute {
        a.push("-an".to_owned());
    } else {
        if (speed - 1.0).abs() > 0.01 {
            a.extend(["-af".to_owned(), atempo_filter(speed)]);
        }
        for s in ["-c:a", "aac", "-b:a", "160k"] {
            a.push(s.to_owned());
        }
    }
    a.extend(["-movflags".to_owned(), "+faststart".to_owned(), path(out)]);
    a
}

/// Applies trim, crop, resize, speed, and mute options to `video` and writes the result to `out`.
/// Uses a temporary `.tmp.mp4` file and renames atomically so `out == video` is safe.
pub fn edit_video(video: &Path, opts: &VideoEditOptions, out: &Path) -> Result<PathBuf, String> {
    let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
    if let Some(end) = opts.end
        && end - opts.start < 0.1
    {
        return Err(t!("trecho curto demais", "clip too short").to_owned());
    }
    let has_audio = probe(video).is_none_or(|i| i.has_audio);
    let mut effective = opts.clone();
    if !has_audio {
        effective.mute = true;
    }
    let tmp = out.with_extension("tmp.mp4");
    let output = ffmpeg_command(&ffmpeg)
        .args(edit_video_args(video, &effective, &tmp))
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() && tmp.is_file() {
        std::fs::rename(&tmp, out).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })?;
        Ok(out.to_path_buf())
    } else {
        let _ = std::fs::remove_file(&tmp);
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn trim_args(video: &Path, start: f64, end: f64, out: &Path) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut a: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(path(video));
    // After "-i" the cut is frame-accurate; the image is re-encoded because of that.
    a.extend(["-ss".to_owned(), format!("{start:.3}"), "-to".to_owned(), format!("{end:.3}")]);
    for s in ["-c:v", "libopenh264", "-b:v", "8000k", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"] {
        a.push(s.to_owned());
    }
    a.push(path(out));
    a
}

/// Saves only the clip between `start` and `end` (in seconds) to a new file, next to the original.
pub fn trim(video: &Path, start: f64, end: f64) -> Result<PathBuf, String> {
    let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
    if end - start < 0.1 {
        return Err(t!("trecho curto demais", "clip too short").to_owned());
    }
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out = video.with_file_name(format!("{stem}-clip.mp4"));
    let output = ffmpeg_command(&ffmpeg).args(trim_args(video, start, end, &out)).output().map_err(|e| e.to_string())?;
    if output.status.success() && out.is_file() {
        Ok(out)
    } else {
        let _ = std::fs::remove_file(&out);
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// How to turn a video into a GIF.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GifOptions {
    pub fps: u32,
    /// Maximum width in pixels; 0 keeps the video's.
    pub width: u32,
    /// Clip of the video, in seconds; an empty `end` goes to the end.
    pub start: f64,
    pub end: Option<f64>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self { fps: 12, width: 960, start: 0.0, end: None }
    }
}

fn gif_args(video: &Path, options: &GifOptions, gif: &Path) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut a: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(path(video));
    if options.start > 0.0 {
        a.extend(["-ss".to_owned(), format!("{:.3}", options.start)]);
    }
    if let Some(end) = options.end {
        a.extend(["-to".to_owned(), format!("{end:.3}")]);
    }
    let scale = match options.width {
        0 => String::new(),
        w => format!("scale='min({w},iw)':-1:flags=lanczos,"),
    };
    // A palette of its own for the video: without it the GIF comes out with poor, dithered colors.
    let filter = format!("fps={},{scale}split[a][b];[a]palettegen[p];[b][p]paletteuse", options.fps.clamp(1, 60));
    a.extend(["-vf".to_owned(), filter, path(gif)]);
    a
}

/// Converts a video to a GIF at `out_gif`.
pub fn to_gif_at(video: &Path, options: &GifOptions, out_gif: &Path) -> Result<PathBuf, String> {
    let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
    let output = ffmpeg_command(&ffmpeg).args(gif_args(video, options, out_gif)).output().map_err(|e| e.to_string())?;
    if output.status.success() && out_gif.is_file() {
        Ok(out_gif.to_path_buf())
    } else {
        let _ = std::fs::remove_file(out_gif);
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Converts a video to a GIF, next to the original.
pub fn to_gif(video: &Path, options: &GifOptions) -> Result<PathBuf, String> {
    to_gif_at(video, options, &video.with_extension("gif"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_uses_even_size_and_screen_offsets() {
        // Monitor to the left of the primary one: negative coordinates; an odd size becomes even.
        let a = capture_args((-1920, 191, -1119, 792), Options::default(), Path::new("saida.mp4")).join(" ");
        assert!(a.contains("-thread_queue_size 1024 -f gdigrab"), "{a}");
        assert!(a.contains("-offset_x -1920 -offset_y 191 -video_size 800x600 -i desktop"), "{a}");
        assert!(a.contains("-framerate 30") && a.contains("-c:v libopenh264"), "{a}");
        assert!(a.contains("-draw_mouse 1"), "{a}");
        assert!(a.ends_with("-movflags +faststart saida.mp4"), "{a}");

        let no_mouse = Options { fps: 60, draw_mouse: false, ..Options::default() };
        let b = capture_args((0, 0, 800, 600), no_mouse, Path::new("b.mp4")).join(" ");
        assert!(b.contains("-draw_mouse 0") && b.contains("-framerate 60"), "{b}");
    }

    #[test]
    fn stream_args_scales_to_even_max_width_and_clamps_fps() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            duration: 10.0,
            fps: 59.94,
            has_audio: true,
        };
        let (args, pw, ph, fps) = stream_args(Path::new("clip.mp4"), 1.25, Some(5.5), info, 1280);
        let joined = args.join(" ");
        assert_eq!((pw, ph, fps), (1280, 720, 60));
        assert!(
            joined.contains("-ss 1.250 -to 5.500 -i clip.mp4 -vf scale=1280:720:flags=fast_bilinear -r 60 -f rawvideo -pix_fmt rgba -"),
            "{joined}"
        );
    }

    #[test]
    fn bitrate_scales_with_area_within_limits() {
        let kbps = |rect, quality| {
            let opts = Options { quality, ..Options::default() };
            let a = capture_args(rect, opts, Path::new("s.mp4"));
            let i = a.iter().position(|x| x == "-b:v").unwrap();
            a[i + 1].clone()
        };
        assert_eq!(kbps((0, 0, 1920, 1080), crate::config::VideoQuality::High), "7975k");
        assert_eq!(kbps((0, 0, 1920, 1080), crate::config::VideoQuality::Standard), "5184k");
        assert_eq!(kbps((0, 0, 1920, 1080), crate::config::VideoQuality::Ultra), "12760k");
        assert_eq!(kbps((0, 0, 100, 100), crate::config::VideoQuality::Standard), "1000k");
        assert_eq!(kbps((0, 0, 7680, 4320), crate::config::VideoQuality::Ultra), "35000k");
    }

    #[test]
    fn mux_copies_video_and_skips_leading_audio() {
        let a = mux_args(Path::new("v.mp4"), Path::new("a.wav"), Duration::from_millis(350), Path::new("o.mp4"));
        let joined = a.join(" ");
        assert!(joined.contains("-i v.mp4 -ss 0.350 -i a.wav"), "{joined}");
        assert!(joined.contains("-c:v copy -c:a aac") && joined.contains("-shortest"), "{joined}");
        assert_eq!(a.last().unwrap(), "o.mp4");
    }

    #[test]
    fn duration_is_parsed_from_ffmpeg_description() {
        let text = "Input #0, mov,mp4, from 'x.mp4':\n  Duration: 00:01:02.50, start: 0.000000, bitrate: 814 kb/s\n";
        assert_eq!(parse_duration(text), Some(Duration::from_millis(62_500)));
        assert_eq!(parse_duration("  Duration: N/A, start: 1.0"), None);
        assert_eq!(parse_duration("nothing here"), None);
    }

    #[test]
    fn probe_parses_ffmpeg_description() {
        let sample = "\
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'clip.mp4':
  Duration: 00:00:12.40, start: 0.000000, bitrate: 4120 kb/s
  Stream #0:0[0x1](und): Video: h264 (Constrained Baseline) (avc1 / 0x31637661), yuv420p(progressive), 1920x1080, 3955 kb/s, 29.97 fps, 30 tbr, 15360 tbn (default)
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706d), 48000 Hz, stereo, fltp, 160 kb/s (default)
";
        let info = parse_video_info(sample).unwrap();
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert!((info.duration - 12.40).abs() < 1e-4);
        assert!((info.fps - 29.97).abs() < 1e-4);
        assert!(info.has_audio);

        let silent = "\
Input #0, mov,mp4, from 'silent.mp4':
  Duration: 00:01:05.00, start: 0.000000, bitrate: 2000 kb/s
  Stream #0:0(und): Video: h264 (avc1 / 0x31637661), yuv420p, 1280x720 [SAR 1:1 DAR 16:9], 60 fps, 60 tbr
";
        let info2 = parse_video_info(silent).unwrap();
        assert_eq!(info2.width, 1280);
        assert_eq!(info2.height, 720);
        assert!((info2.duration - 65.0).abs() < 1e-4);
        assert!((info2.fps - 60.0).abs() < 1e-4);
        assert!(!info2.has_audio);
    }

    #[test]
    fn edit_video_args_builds_filters_and_audio_options() {
        let opts = VideoEditOptions {
            start: 1.25,
            end: Some(8.5),
            speed: 2.0,
            mute: false,
            crop: Some((10, 20, 641, 481)),
            max_width: 1280,
        };
        let args = edit_video_args(Path::new("in.mp4"), &opts, Path::new("out.mp4")).join(" ");
        assert!(args.contains("-ss 1.250 -to 8.500"), "{args}");
        assert!(args.contains("crop=640:480:10:20,scale='min(1280,iw)':-2,setpts=0.5000*PTS"), "{args}");
        assert!(args.contains("-af atempo=2.000"), "{args}");
        assert!(args.contains("-c:v libopenh264") && args.contains("-pix_fmt yuv420p"), "{args}");

        let muted = VideoEditOptions { mute: true, ..VideoEditOptions::default() };
        let args_m = edit_video_args(Path::new("in.mp4"), &muted, Path::new("out.mp4")).join(" ");
        assert!(args_m.contains("-an") && !args_m.contains("-c:a"), "{args_m}");
    }

    /// FFmpeg's description of the file (streams, length, frames per second).
    fn describe(file: &Path) -> String {
        let out = ffmpeg_command(&find_ffmpeg().unwrap()).args(["-hide_banner", "-i"]).arg(file).output().unwrap();
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    /// Average frames per second, read from "…, 29.67 fps, …".
    fn fps(description: &str) -> f64 {
        let line = description.lines().find(|l| l.contains("Video:")).expect("no video stream");
        let before = line.split(" fps").next().unwrap();
        before.rsplit(',').next().unwrap().trim().parse().unwrap()
    }

    /// True when the current desktop session does not allow screen capture (CI or sandboxed
    /// environment).
    fn screen_unavailable() -> bool {
        crate::in_ci() || crate::capture::grab_virtual((0, 0, 64, 64), false).is_err()
    }

    /// Records 3 s of a corner of the screen with system sound and microphone, using the real
    /// FFmpeg, and checks what matters most: the image doesn't drop frames and the sound gets in.
    /// Requires `vendor/ffmpeg` (scripts/fetch-ffmpeg.ps1).
    #[test]
    fn records_smooth_video_with_audio_and_converts_to_gif() {
        if screen_unavailable() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-video-{}", std::process::id()));
        let options = Options { system_audio: true, microphone: true, ..Options::default() };
        let mut rec = Recorder::start((0, 0, 320, 240), options, &dir, "test").unwrap();
        let had_audio = rec.audio.is_some();
        std::thread::sleep(Duration::from_secs(3));
        assert!(!rec.has_exited(), "FFmpeg shouldn't have exited on its own");
        let video = rec.stop().unwrap();

        let description = describe(&video);
        assert!(fps(&description) > 25.0, "image dropping frames:
{description}");
        let length = parse_duration(&description).unwrap().as_secs_f64();
        assert!((2.3..3.6).contains(&length), "length {length} s");
        if had_audio {
            assert!(description.contains("Audio: aac"), "no sound in the file:
{description}");
            assert!(description.contains("stereo"), "{description}");
        }
        // The temporary files are gone; only the final file is left.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");

        let gif = to_gif(&video, &GifOptions::default()).unwrap();
        assert!(std::fs::metadata(&gif).unwrap().len() > 500);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concat_list_quotes_each_part() {
        let parts = [PathBuf::from(r"C:\v\a.part1.mp4"), PathBuf::from(r"C:\v\it's.part2.mp4")];
        assert_eq!(concat_list(&parts), "file 'C:\\v\\a.part1.mp4'\nfile 'C:\\v\\it'\\''s.part2.mp4'\n");
    }

    #[test]
    fn gif_and_trim_commands_follow_the_options() {
        let gif = gif_args(Path::new("v.mp4"), &GifOptions::default(), Path::new("v.gif")).join(" ");
        assert!(gif.contains("-i v.mp4 -vf fps=12,scale='min(960,iw)':-1:flags=lanczos,split"), "{gif}");
        assert!(!gif.contains("-ss") && !gif.contains("-to"), "{gif}");

        let options = GifOptions { fps: 24, width: 0, start: 1.5, end: Some(4.0) };
        let gif = gif_args(Path::new("v.mp4"), &options, Path::new("v.gif")).join(" ");
        assert!(gif.contains("-i v.mp4 -ss 1.500 -to 4.000 -vf fps=24,split[a][b]"), "{gif}");

        let cut = trim_args(Path::new("v.mp4"), 2.0, 5.25, Path::new("o.mp4")).join(" ");
        assert!(cut.contains("-i v.mp4 -ss 2.000 -to 5.250 -c:v libopenh264"), "{cut}");
        assert!(cut.ends_with("o.mp4"), "{cut}");
    }

    /// Records with a pause in the middle, trims a clip and makes a GIF of it, with the real
    /// FFmpeg.
    #[test]
    fn paused_recording_becomes_one_video_without_the_pause() {
        if screen_unavailable() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-pause-{}", std::process::id()));
        let options = Options { system_audio: false, microphone: false, ..Options::default() };
        let mut session = Session::start((0, 0, 240, 160), options, &dir, "pause").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        session.pause().unwrap();
        assert!(session.is_paused());
        let at_pause = session.elapsed();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(session.elapsed(), at_pause, "the clock stops during the pause");
        session.resume().unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let video = session.stop().unwrap();

        let length = video_duration(&video).unwrap().as_secs_f64();
        assert!((2.0..4.0).contains(&length), "3 s recorded and 1.5 s paused; came out as {length} s");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "the parts are gone after being joined: {left:?}");

        let cut = trim(&video, 0.5, 1.5).unwrap();
        let cut_length = video_duration(&cut).unwrap().as_secs_f64();
        assert!((0.8..1.3).contains(&cut_length), "1 s clip came out as {cut_length} s");
        let gif = to_gif(&video, &GifOptions { fps: 8, width: 120, start: 0.5, end: Some(1.5) }).unwrap();
        assert!(std::fs::metadata(&gif).unwrap().len() > 300);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn video_without_audio_is_just_renamed() {
        if screen_unavailable() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-mute-{}", std::process::id()));
        let options = Options { system_audio: false, microphone: false, ..Options::default() };
        let rec = Recorder::start((0, 0, 200, 160), options, &dir, "mute").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let video = rec.stop().unwrap();
        let description = describe(&video);
        assert!(!description.contains("Audio:"), "{description}");
        assert!(fps(&description) > 25.0, "{description}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn waveform_buckets_normalizes_peaks_and_handles_silence() {
        assert!(waveform_buckets(&[], 16).is_empty());
        assert_eq!(waveform_buckets(&[0.0; 100], 8), vec![0.0; 8]);
        let mut samples = vec![0.02f32; 400];
        for s in &mut samples[100..200] {
            *s = 0.8;
        }
        let bars = waveform_buckets(&samples, 4);
        assert_eq!(bars.len(), 4);
        assert!((bars[1] - 1.0).abs() < 1e-4, "loudest quarter normalizes to 1.0: {bars:?}");
        assert!(bars[0] < 0.1 && bars[2] < 0.1, "{bars:?}");
    }
}
