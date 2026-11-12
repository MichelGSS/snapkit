//! Step-by-step capture: while the user does a task, each mouse click inside the chosen area (and
//! each Enter) becomes a shot of that area. At the end, the shots are stacked into a single guide:
//! each step has an editable caption on top and a numbered marker at the click point.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use image::RgbaImage;

use crate::capture;
use crate::render::{Op, StepGlyph, StepShape};
use crate::win::{self, PxRect};

/// Limit of shots per session: a guide longer than this becomes too heavy an image.
pub const MAX_STEPS: usize = 40;

const MARGIN: u32 = 24;
const GAP: u32 = 28;
/// Strip above each shot, where the step's caption goes.
const CAPTION_HEIGHT: u32 = 40;
const CAPTION_SIZE: f32 = 22.0;
const CAPTION_COLOR: [u8; 4] = [30, 30, 34, 255];
const BACKGROUND: [u8; 4] = [236, 236, 240, 255];
const MARKER_COLOR: [u8; 4] = [235, 64, 52, 255];
const MARKER_RADIUS: f32 = 16.0;

/// What the user did to produce the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    LeftClick,
    RightClick,
    Enter,
}

impl Trigger {
    /// Initial caption text; the user rewrites it by double-clicking in the editor.
    fn caption(self, number: usize) -> String {
        let action = match self {
            Trigger::LeftClick => t!("clique aqui", "click here"),
            Trigger::RightClick => t!("clique com o botão direito", "right-click"),
            Trigger::Enter => t!("pressione Enter", "press Enter"),
        };
        tf!("Passo {number}: {action}", "Step {number}: {action}")
    }
}

/// A shot of the area, taken at the moment of a click or an Enter.
pub struct Shot {
    pub image: RgbaImage,
    pub trigger: Trigger,
    /// Where the cursor was, in shot pixels. `None` for keyboard steps.
    pub click: Option<(i32, i32)>,
}

/// Session in progress: a thread watches the mouse and the Enter key and takes a shot at each
/// action.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    shots: Arc<Mutex<Vec<Shot>>>,
    thread: Option<JoinHandle<()>>,
}

fn contains(r: PxRect, p: (i32, i32)) -> bool {
    p.0 >= r.0 && p.0 < r.2 && p.1 >= r.1 && p.1 < r.3
}

impl Recorder {
    /// Starts watching the actions inside `rect` (screen coordinates). Clicks on the window
    /// `own_hwnd` (our panel) don't count. `on_shot` is called after each shot.
    pub fn start(rect: PxRect, own_hwnd: isize, on_shot: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let shots = Arc::new(Mutex::new(Vec::new()));
        let (stop_flag, list) = (stop.clone(), shots.clone());
        let thread = std::thread::spawn(move || {
            let read = || [win::left_button_down(), win::right_button_down(), win::enter_down()];
            // Whatever is already pressed when it starts doesn't count: the click on Start, for
            // example.
            let mut was = read();
            while !stop_flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(8));
                let now = read();
                let pressed = [now[0] && !was[0], now[1] && !was[1], now[2] && !was[2]];
                was = now;
                let trigger = match pressed {
                    [true, _, _] => Trigger::LeftClick,
                    [_, true, _] => Trigger::RightClick,
                    [_, _, true] => Trigger::Enter,
                    _ => continue,
                };
                let at = win::cursor_pos();
                let on_panel = win::window_rect(own_hwnd).is_some_and(|panel| contains(panel, at));
                // A click has to land in the area; Enter counts with the cursor anywhere.
                let in_area = trigger == Trigger::Enter || contains(rect, at);
                if !in_area || on_panel || list.lock().unwrap().len() >= MAX_STEPS {
                    continue;
                }
                // The shot is taken right after the press, before the screen reacts in most
                // programs: the guide shows what to do, not the result.
                if let Ok(image) = capture::grab_virtual(rect, false) {
                    let click = (trigger != Trigger::Enter).then_some((at.0 - rect.0, at.1 - rect.1));
                    list.lock().unwrap().push(Shot { image, trigger, click });
                    on_shot();
                }
            }
        });
        Self { stop, shots, thread: Some(thread) }
    }

    pub fn count(&self) -> usize {
        self.shots.lock().unwrap().len()
    }

    /// Throws away the last shot (an action that wasn't part of the task).
    pub fn undo_last(&self) {
        self.shots.lock().unwrap().pop();
    }

    /// Stops watching and returns the shots, in the order they were taken.
    pub fn finish(mut self) -> Vec<Shot> {
        self.halt();
        std::mem::take(&mut *self.shots.lock().unwrap())
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Stacks the shots into a single image and returns, separately, each step's annotations: the
/// caption and, if there was a click, the numbered marker. They go as annotations so they stay
/// editable in the editor (double-clicking rewrites the caption, dragging moves the marker).
pub fn compose(shots: &[Shot]) -> Option<(RgbaImage, Vec<Op>)> {
    let widest = shots.iter().map(|s| s.image.width()).max()?;
    let count = shots.len() as u32;
    let total: u32 = shots.iter().map(|s| s.image.height()).sum();
    let height = total + count * CAPTION_HEIGHT + GAP * (count - 1) + 2 * MARGIN;
    // Minimum width so the longest caption fits even with a narrow area.
    let width = widest.max(420) + 2 * MARGIN;
    let mut guide = RgbaImage::from_pixel(width, height, image::Rgba(BACKGROUND));
    let mut notes = Vec::new();
    let mut top = MARGIN;
    for (i, shot) in shots.iter().enumerate() {
        notes.push(Op::Text {
            pos: (MARGIN as f32, top as f32 + 4.0),
            text: shot.trigger.caption(i + 1),
            color: CAPTION_COLOR,
            size: CAPTION_SIZE,
            boxed: false,
        });
        let image_top = top + CAPTION_HEIGHT;
        image::imageops::replace(&mut guide, &shot.image, MARGIN as i64, image_top as i64);
        if let Some(click) = shot.click {
            // The marker stays entirely inside the shot, even with the click right at the edge.
            let (w, h) = (shot.image.width() as f32, shot.image.height() as f32);
            let x = (click.0 as f32).clamp(MARKER_RADIUS.min(w / 2.0), (w - MARKER_RADIUS).max(w / 2.0));
            let y = (click.1 as f32).clamp(MARKER_RADIUS.min(h / 2.0), (h - MARKER_RADIUS).max(h / 2.0));
            notes.push(Op::Step {
                center: (MARGIN as f32 + x, image_top as f32 + y),
                number: i as u32 + 1,
                color: MARKER_COLOR,
                radius: MARKER_RADIUS,
                shape: StepShape::Circle,
                glyph: StepGlyph::Number,
            });
        }
        top = image_top + shot.image.height() + GAP;
    }
    Some((guide, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(w: u32, h: u32, color: [u8; 4], trigger: Trigger, click: Option<(i32, i32)>) -> Shot {
        Shot { image: RgbaImage::from_pixel(w, h, image::Rgba(color)), trigger, click }
    }

    #[test]
    fn guide_stacks_the_shots_with_a_caption_and_a_marker_each() {
        let (red, blue) = ([200, 0, 0, 255], [0, 0, 200, 255]);
        let shots = [
            shot(500, 50, red, Trigger::LeftClick, Some((30, 20))),
            shot(480, 60, blue, Trigger::RightClick, Some((479, 59))),
        ];
        let (guide, notes) = compose(&shots).unwrap();
        assert_eq!((guide.width(), guide.height()), (548, 50 + 60 + 2 * 40 + 28 + 48));
        assert_eq!(guide.get_pixel(24, 24 + 40).0, red, "the shot goes below the caption");
        assert_eq!(guide.get_pixel(24, 24 + 20).0, BACKGROUND, "caption strip");
        let second_top = 24 + 40 + 50 + 28 + 40;
        assert_eq!(guide.get_pixel(24, second_top).0, blue, "the second shot comes below the first");

        // For each step: the caption and then the marker.
        assert_eq!(notes.len(), 4);
        let Op::Text { text, pos, .. } = &notes[0] else { panic!("esperava legenda: {:?}", notes[0]) };
        assert_eq!((text.as_str(), *pos), ("Passo 1: clique aqui", (24.0, 28.0)));
        let Op::Step { center, number, .. } = &notes[1] else { panic!("esperava marcador: {:?}", notes[1]) };
        assert_eq!((*center, *number), ((54.0, 84.0), 1));
        let Op::Text { text, .. } = &notes[2] else { panic!() };
        assert_eq!(text, "Passo 2: clique com o botão direito");
        // Click in the corner: the marker is pulled inside the shot.
        let Op::Step { center, number, .. } = &notes[3] else { panic!() };
        assert_eq!((*center, *number), ((24.0 + 464.0, second_top as f32 + 44.0), 2));
        assert!(compose(&[]).is_none());
    }

    #[test]
    fn keyboard_steps_have_a_caption_but_no_marker() {
        let shots = [shot(100, 40, [9, 9, 9, 255], Trigger::Enter, None)];
        let (guide, notes) = compose(&shots).unwrap();
        assert_eq!(guide.width(), 420 + 48, "a narrow area gets wider so the caption fits");
        assert_eq!(notes.len(), 1);
        let Op::Text { text, .. } = &notes[0] else { panic!() };
        assert_eq!(text, "Passo 1: pressione Enter");
    }

    #[test]
    fn recorder_starts_empty_and_stops_cleanly() {
        let recorder = Recorder::start((0, 0, 10, 10), 0, || {});
        assert_eq!(recorder.count(), 0);
        recorder.undo_last();
        assert!(recorder.finish().is_empty());
    }
}
