//! Audio recording for videos: system sound (what plays through the speakers or headphones) and the
//! microphone, mixed into one WAV file.
//!
//! SnapKit records the audio itself, apart from the video, and joins the two when the recording
//! ends. Letting FFmpeg record audio and video live in the same process makes the screen capture
//! drop frames: it holds the video back while it waits for the audio.
//!
//! The two sources arrive in different formats (sample rate, channel count). Each one is converted
//! to a common format, 48 kHz stereo, as soon as it arrives.
//!
//! The recording follows the clock: at every moment it writes exactly as many samples as the
//! elapsed time calls for, and fills whatever didn't arrive with silence (Windows delivers nothing
//! from the system sound while no sound is playing). This way the file's length tracks real time,
//! which lets it line up with the video.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Common recording format.
pub const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// The recording stays this far behind the clock, to absorb the uneven way Windows delivers audio
/// blocks.
const DELAY: Duration = Duration::from_millis(100);

/// Queue of samples already converted (interleaved stereo, 48 kHz).
type Ring = Arc<Mutex<VecDeque<f32>>>;

#[derive(Clone, Copy)]
enum Source {
    /// System output sound (WASAPI in "loopback" mode).
    System,
    Microphone,
}

/// Downmixes an audio frame with `channels` channels to stereo. Windows channel order: FL FR FC LFE
/// BL BR SL SR.
fn downmix(frame: &[f32]) -> [f32; 2] {
    match frame.len() {
        0 => [0.0, 0.0],
        1 => [frame[0], frame[0]],
        2 => [frame[0], frame[1]],
        n => {
            // The center channel carries the voices in 5.1/7.1 sound: it has to go to both sides.
            let center = 0.707 * frame[2];
            let (mut l, mut r) = (frame[0] + center, frame[1] + center);
            if n >= 6 {
                l += 0.5 * frame[4];
                r += 0.5 * frame[5];
            }
            if n >= 8 {
                l += 0.5 * frame[6];
                r += 0.5 * frame[7];
            }
            [l, r]
        }
    }
}

/// Converts an audio stream to 48 kHz stereo, chunk by chunk, with linear interpolation.
struct Converter {
    channels: usize,
    /// Input frames per output frame.
    step: f64,
    /// Fractional position of the next output frame, counted from `prev`.
    pos: f64,
    /// Last frame of the previous chunk.
    prev: [f32; 2],
}

impl Converter {
    fn new(rate: u32, channels: u16) -> Self {
        Self { channels: channels.max(1) as usize, step: rate as f64 / RATE as f64, pos: 0.0, prev: [0.0; 2] }
    }

    fn convert(&mut self, input: &[f32], out: &mut VecDeque<f32>) {
        let frames: Vec<[f32; 2]> = input.chunks_exact(self.channels).map(downmix).collect();
        let n = frames.len();
        // The interpolated sequence is [prev, f0, f1, …]: index 0 is the previous frame.
        let at = |i: usize| if i == 0 { self.prev } else { frames[i - 1] };
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push_back(a[0] + (b[0] - a[0]) * t);
            out.push_back(a[1] + (b[1] - a[1]) * t);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if let Some(last) = frames.last() {
            self.prev = *last;
        }
    }
}

/// If nobody is consuming, keep only the last half second.
fn trim(ring: &mut VecDeque<f32>) {
    let max = (RATE as usize * CHANNELS as usize) / 2;
    if ring.len() > max {
        let excess = ring.len() - max;
        ring.drain(..excess);
    }
}

/// Opens the capture of one source; the converted samples go to `ring`.
fn open_stream(source: Source, ring: Ring) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let (device, config) = match source {
        Source::System => {
            let d = host.default_output_device().ok_or(t!("nenhum dispositivo de saída de som", "no sound output device"))?;
            let c = d.default_output_config().map_err(|e| e.to_string())?;
            (d, c)
        }
        Source::Microphone => {
            let d = host.default_input_device().ok_or(t!("nenhum microfone encontrado", "no microphone found"))?;
            let c = d.default_input_config().map_err(|e| e.to_string())?;
            (d, c)
        }
    };
    let mut converter = Converter::new(config.sample_rate(), config.channels());
    let mut push = move |samples: &[f32]| {
        let mut ring = ring.lock().unwrap();
        converter.convert(samples, &mut ring);
        trim(&mut ring);
    };
    let on_error = |_| {};
    // For the system sound: asking for an *input* stream on an *output* device is how WASAPI does
    // loopback.
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            device.build_input_stream(config.config(), move |data: &[f32], _: &_| push(data), on_error, None)
        }
        cpal::SampleFormat::I16 => device.build_input_stream(
            config.config(),
            move |data: &[i16], _: &_| {
                let floats: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                push(&floats);
            },
            on_error,
            None,
        ),
        other => return Err(tf!("formato de áudio não suportado: {other:?}", "unsupported audio format: {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// How many samples (counting both channels) should have been recorded after `elapsed`.
fn samples_due(elapsed: Duration) -> u64 {
    let frames = (elapsed.saturating_sub(DELAY).as_secs_f64() * RATE as f64) as u64;
    frames * CHANNELS as u64
}

/// Takes `count` samples from each queue, adds them and clamps; whatever is missing becomes
/// silence.
fn mix_due(rings: &[Ring], count: usize) -> Vec<i16> {
    let mut mixed = vec![0f32; count];
    for ring in rings {
        let mut ring = ring.lock().unwrap();
        let available = count.min(ring.len());
        for (slot, sample) in mixed.iter_mut().zip(ring.drain(..available)) {
            *slot += sample;
        }
    }
    mixed.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// WAV header (16-bit PCM) for `data_bytes` bytes of audio.
fn wav_header(data_bytes: u32) -> [u8; 44] {
    let block = CHANNELS * 2;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * block as u32).to_le_bytes());
    h[32..34].copy_from_slice(&block.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

/// Records audio at the pace of the clock until told to stop; returns the audio bytes recorded.
fn write_wav(path: &Path, rings: &[Ring], stop: &AtomicBool) -> std::io::Result<u32> {
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(&wav_header(0))?;
    let start = Instant::now();
    let mut written: u64 = 0;
    loop {
        let stopping = stop.load(Ordering::Relaxed);
        let due = samples_due(start.elapsed());
        if due > written {
            for sample in mix_due(rings, (due - written) as usize) {
                file.write_all(&sample.to_le_bytes())?;
            }
            written = due;
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let data_bytes = (written * 2) as u32;
    // Now that the size is known, the header is written again.
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&wav_header(data_bytes))?;
    file.flush()?;
    Ok(data_bytes)
}

/// An audio recording in progress.
pub struct AudioRecording {
    stop: Arc<AtomicBool>,
    captures: Vec<JoinHandle<()>>,
    writer: JoinHandle<std::io::Result<u32>>,
    path: PathBuf,
}

impl AudioRecording {
    /// Starts recording the requested sources to `path`. Returns `None` if none opens, and always
    /// the list of warnings about the sources that failed.
    pub fn start(system: bool, microphone: bool, path: &Path) -> (Option<Self>, Vec<String>) {
        let stop = Arc::new(AtomicBool::new(false));
        let mut rings: Vec<Ring> = Vec::new();
        let mut captures = Vec::new();
        let mut warnings = Vec::new();
        let wanted = [
            (system, Source::System, t!("som do sistema", "system sound")),
            (microphone, Source::Microphone, t!("microfone", "microphone")),
        ];
        for (_, source, label) in wanted.into_iter().filter(|w| w.0) {
            let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
            // Each capture lives on its own thread: a `cpal` stream can't change threads.
            let (tx, rx) = channel::<Result<(), String>>();
            let (stop_c, ring_c) = (stop.clone(), ring.clone());
            let handle = std::thread::spawn(move || {
                let stream = match open_stream(source, ring_c) {
                    Ok(stream) => {
                        let _ = tx.send(Ok(()));
                        stream
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                while !stop_c.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(stream);
            });
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => {
                    rings.push(ring);
                    captures.push(handle);
                }
                Ok(Err(e)) => warnings.push(tf!("sem {label}: {e}", "no {label}: {e}")),
                Err(_) => warnings.push(tf!("sem {label}: a captura não respondeu", "no {label}: the capture did not respond")),
            }
        }
        if rings.is_empty() {
            return (None, warnings);
        }
        let (stop_w, path_w) = (stop.clone(), path.to_owned());
        let writer = std::thread::spawn(move || write_wav(&path_w, &rings, &stop_w));
        let recording = Self { stop, captures, writer, path: path.to_owned() };
        (Some(recording), warnings)
    }

    /// Stops recording and returns the file and its length.
    pub fn stop(self) -> Result<(PathBuf, Duration), String> {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.captures {
            let _ = t.join();
        }
        let data_bytes = self
            .writer
            .join()
            .map_err(|_| t!("a gravação de áudio falhou", "the audio recording failed").to_owned())?
            .map_err(|e| e.to_string())?;
        let seconds = data_bytes as f64 / (RATE as f64 * CHANNELS as f64 * 2.0);
        Ok((self.path, Duration::from_secs_f64(seconds)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert_all(rate: u32, channels: u16, chunks: &[&[f32]]) -> Vec<f32> {
        let mut c = Converter::new(rate, channels);
        let mut out = VecDeque::new();
        for chunk in chunks {
            c.convert(chunk, &mut out);
        }
        out.into()
    }

    #[test]
    fn downmix_handles_mono_stereo_and_surround() {
        assert_eq!(downmix(&[0.5]), [0.5, 0.5]);
        assert_eq!(downmix(&[0.1, 0.2]), [0.1, 0.2]);
        // 5.1 with only the center channel (voice): it has to come out the same on both sides.
        let voice = downmix(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(voice[0] > 0.7 && voice[0] == voice[1]);
        // 7.1: the back and side channels go to the right side.
        let left_only = downmix(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        assert_eq!(left_only, [1.0, 0.0]);
    }

    #[test]
    fn same_rate_passes_samples_through_with_one_frame_delay() {
        let out = convert_all(RATE, 2, &[&[0.1, 0.2, 0.3, 0.4], &[0.5, 0.6]]);
        assert_eq!(out, vec![0.0, 0.0, 0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn resampling_keeps_duration_regardless_of_chunking() {
        // 1 second at 44.1 kHz mono, in uneven chunks → ~1 second at 48 kHz stereo.
        let input: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.01).sin()).collect();
        let whole = convert_all(44_100, 1, &[&input]);
        let pieces = convert_all(44_100, 1, &[&input[..1000], &input[1000..1003], &input[1003..]]);
        assert!((whole.len() as i64 - 96_000).abs() <= 4, "{} amostras", whole.len());
        assert_eq!(whole.len(), pieces.len());
        assert!(whole.iter().zip(&pieces).all(|(a, b)| (a - b).abs() < 1e-4), "splitting into chunks doesn't change the sound");
        // 96 kHz → half the frames.
        assert!((convert_all(96_000, 2, &[&vec![0.0; 9600]]).len() as i64 - 4800).abs() <= 4);
    }

    #[test]
    fn mix_sums_sources_pads_silence_and_clips() {
        let ring = |v: &[f32]| -> Ring { Arc::new(Mutex::new(v.iter().copied().collect())) };
        let (a, b) = (ring(&[0.5, 0.5, 0.9]), ring(&[0.25]));
        let mixed = mix_due(&[a.clone(), b], 4);
        assert_eq!(mixed, vec![24575, 16383, 29490, 0]);
        assert!(a.lock().unwrap().is_empty());
        // A sum above the maximum is clamped, not wrapped around.
        assert_eq!(mix_due(&[ring(&[0.8]), ring(&[0.8])], 1), vec![32767]);
    }

    #[test]
    fn samples_due_follows_the_clock_after_the_delay() {
        assert_eq!(samples_due(Duration::from_millis(50)), 0, "still within the delay");
        assert_eq!(samples_due(DELAY + Duration::from_secs(1)), 96_000);
    }

    #[test]
    fn wav_header_describes_48k_stereo_pcm16() {
        let h = wav_header(1000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 1036);
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 192_000);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 1000);
    }

    #[test]
    fn wav_file_duration_matches_wall_time_even_in_silence() {
        let path = std::env::temp_dir().join(format!("snapkit-test-audio-{}.wav", std::process::id()));
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, r, s) = (path.clone(), ring.clone(), stop.clone());
        let t = std::thread::spawn(move || write_wav(&p, &[r], &s));
        std::thread::sleep(Duration::from_millis(600));
        stop.store(true, Ordering::Relaxed);
        let data_bytes = t.join().unwrap().unwrap();
        let seconds = data_bytes as f64 / 192_000.0;
        assert!((0.45..0.65).contains(&seconds), "{seconds} s");
        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len(), 44 + data_bytes as usize);
        assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()), data_bytes, "header updated");
        let _ = std::fs::remove_file(&path);
    }
}
