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
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::audio::AudioRecording;
use crate::win::PxRect;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const FRAME_RATE: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub system_audio: bool,
    pub microphone: bool,
}

/// Path of the bundled `ffmpeg.exe`, if it exists.
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
    candidates.into_iter().map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file())
}

fn ffmpeg_command(ffmpeg: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg);
    cmd.creation_flags(CREATE_NO_WINDOW)
        // Without this, Windows "lies" about the screen coordinates to FFmpeg on monitors scaled
        // other than 100%, and the recorded area would come out shifted and blurry.
        .env("__COMPAT_LAYER", "HighDpiAware");
    cmd
}

/// FFmpeg command line to record the image of `rect` to `out`.
fn capture_args(rect: PxRect, out: &Path) -> Vec<String> {
    // H.264 requires an even width and height.
    let (w, h) = ((rect.2 - rect.0) & !1, (rect.3 - rect.1) & !1);
    // Bit rate proportional to the area: ~8 Mbit/s at 1920×1080.
    let kbps = ((w as i64 * h as i64) / 260).clamp(1500, 20_000);
    let mut a: Vec<String> = [
        "-y", "-hide_banner", "-loglevel", "error", "-f", "gdigrab", "-draw_mouse", "1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut add = |key: &str, value: String| a.extend([key.to_owned(), value]);
    add("-framerate", FRAME_RATE.to_string());
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

/// Recordings folder: the user's `Videos\SnapKit`.
pub fn videos_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    home.join("Videos").join("SnapKit")
}

/// A recording in progress.
pub struct Recorder {
    ffmpeg: PathBuf,
    child: Child,
    audio: Option<AudioRecording>,
    /// Final file and the two temporary ones (image and audio).
    path: PathBuf,
    video_part: PathBuf,
    audio_part: PathBuf,
    started: Instant,
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
            .args(capture_args(rect, &video_part))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match child {
            Ok(child) => Ok(Self { ffmpeg, child, audio, path, video_part, audio_part, started: Instant::now(), warnings }),
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
        // "q" on standard input asks FFmpeg to close the file properly.
        if let Some(stdin) = self.child.stdin.as_mut() {
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
        let mut errors = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut errors);
        }
        if !std::fs::metadata(&self.video_part).is_ok_and(|m| m.len() > 0) {
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
            Ok(()) => Ok(self.path),
            // If joining fails, the recorded image isn't lost: it just has no sound.
            Err(e) => match std::fs::rename(&self.video_part, &self.path) {
                Ok(()) => Ok(self.path),
                Err(_) => Err(e),
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
        if self.warnings.is_empty() {
            self.warnings = recorder.warnings.clone();
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
        let out = self.dir.join(format!("{}.mp4", self.name));
        match self.parts.as_slice() {
            [] => Err(last.err().unwrap_or_else(|| t!("nada foi gravado", "nothing was recorded").to_owned())),
            [only] => std::fs::rename(only, &out).map(|()| out).map_err(|e| e.to_string()),
            parts => {
                let list = self.dir.join(format!("{}.parts.txt", self.name));
                std::fs::write(&list, concat_list(parts)).map_err(|e| e.to_string())?;
                let output = ffmpeg_command(&self.ffmpeg)
                    .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
                    .arg(&list)
                    .args(["-c", "copy", "-movflags", "+faststart"])
                    .arg(&out)
                    .output()
                    .map_err(|e| e.to_string())?;
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

/// List of files in the format FFmpeg uses for joining: one `file '…'` line per stretch.
fn concat_list(parts: &[PathBuf]) -> String {
    parts.iter().map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', r"'\''"))).collect()
}

/// Length of a video already recorded.
pub fn video_duration(video: &Path) -> Option<Duration> {
    media_duration(&find_ffmpeg()?, video)
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

/// Converts a video to a GIF, next to the original.
pub fn to_gif(video: &Path, options: &GifOptions) -> Result<PathBuf, String> {
    let ffmpeg = find_ffmpeg().ok_or(t!("o FFmpeg não foi encontrado (pasta ffmpeg)", "FFmpeg was not found (ffmpeg folder)"))?;
    let gif = video.with_extension("gif");
    let output = ffmpeg_command(&ffmpeg).args(gif_args(video, options, &gif)).output().map_err(|e| e.to_string())?;
    if output.status.success() && gif.is_file() {
        Ok(gif)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_uses_even_size_and_screen_offsets() {
        // Monitor to the left of the primary one: negative coordinates; an odd size becomes even.
        let a = capture_args((-1920, 191, -1119, 792), Path::new("saida.mp4")).join(" ");
        assert!(a.contains("-offset_x -1920 -offset_y 191 -video_size 800x600 -i desktop"), "{a}");
        assert!(a.contains("-framerate 30") && a.contains("-c:v libopenh264"), "{a}");
        assert!(a.ends_with("-movflags +faststart saida.mp4"), "{a}");
    }

    #[test]
    fn bitrate_scales_with_area_within_limits() {
        let kbps = |rect| {
            let a = capture_args(rect, Path::new("s.mp4"));
            let i = a.iter().position(|x| x == "-b:v").unwrap();
            a[i + 1].clone()
        };
        assert_eq!(kbps((0, 0, 1920, 1080)), "7975k");
        assert_eq!(kbps((0, 0, 100, 100)), "1500k");
        assert_eq!(kbps((0, 0, 7680, 4320)), "20000k");
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

    /// Records 3 s of a corner of the screen with system sound and microphone, using the real
    /// FFmpeg, and checks what matters most: the image doesn't drop frames and the sound gets in.
    /// Requires `vendor/ffmpeg` (scripts/fetch-ffmpeg.ps1).
    #[test]
    fn records_smooth_video_with_audio_and_converts_to_gif() {
        if crate::in_ci() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-video-{}", std::process::id()));
        let options = Options { system_audio: true, microphone: true };
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
        if crate::in_ci() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-pause-{}", std::process::id()));
        let options = Options { system_audio: false, microphone: false };
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
        if crate::in_ci() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("snapkit-test-mute-{}", std::process::id()));
        let options = Options { system_audio: false, microphone: false };
        let rec = Recorder::start((0, 0, 200, 160), options, &dir, "mute").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let video = rec.stop().unwrap();
        let description = describe(&video);
        assert!(!description.contains("Audio:"), "{description}");
        assert!(fps(&description) > 25.0, "{description}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
