//! Dragging library files out of SnapKit into Finder or another program (macOS).
//!
//! egui reports the start of a drag over a library item; from there AppKit takes over with an
//! `NSDraggingSession` that carries the files as `NSURL`s. The session runs on its own in the event
//! loop, so [`start_file_drag`] returns right away. AppKit keeps the button release for itself, so
//! when the session ends a release is handed back to our window: otherwise egui would still think
//! the button is held.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent,
    NSEventModifierFlags, NSEventType, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSPoint, NSProcessInfo, NSRect, NSSize, NSString, NSURL};

/// Size of the file icon that follows the cursor, in points.
const ICON_SIDE: f64 = 48.0;

define_class!(
    /// Source of the drag: it only offers a copy, so the library always keeps its files.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SnapKitFileDragSource"]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, screen_point: NSPoint, _operation: NSDragOperation) {
            release_button(self.mtm(), screen_point);
        }
    }
);

impl DragSource {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's plain initializer; the class has no fields of its own.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// One source for every drag, kept alive for the whole run (AppKit doesn't retain it).
    static SOURCE: OnceCell<Retained<DragSource>> = const { OnceCell::new() };
    /// The window the drag in progress started from.
    static DRAG_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

fn uptime() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// The mouse event that starts the session: the one being handled, if it's the left button over
/// our window (the usual case), or an equivalent one made up at the cursor.
fn drag_event(app: &NSApplication, window: &NSWindow) -> Option<Retained<NSEvent>> {
    if let Some(event) = app.currentEvent()
        && matches!(event.r#type(), NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged)
        && event.windowNumber() == window.windowNumber()
    {
        return Some(event);
    }
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDragged,
        window.mouseLocationOutsideOfEventStream(),
        NSEventModifierFlags::empty(),
        uptime(),
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
}

/// Starts dragging the files, as Finder does. Returns `true` if the drag started; the drop itself
/// happens later, in the event loop.
pub fn start_file_drag(paths: &[PathBuf]) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    if paths.is_empty() {
        return false;
    }
    let app = NSApplication::sharedApplication(mtm);
    let from_event = app.currentEvent().and_then(|e| e.window(mtm));
    let Some(window) = from_event.or_else(|| app.keyWindow()).or_else(|| app.mainWindow()) else { return false };
    let Some(view) = window.contentView() else { return false };
    let Some(event) = drag_event(&app, &window) else { return false };
    let at = view.convertPoint_fromView(event.locationInWindow(), None);

    let workspace = NSWorkspace::sharedWorkspace();
    let items: Vec<Retained<NSDraggingItem>> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let ns_path = NSString::from_str(&path.to_string_lossy());
            let url = NSURL::fileURLWithPath(&ns_path);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), ProtocolObject::from_ref(&*url));
            let icon = workspace.iconForFile(&ns_path);
            icon.setSize(NSSize::new(ICON_SIDE, ICON_SIDE));
            // Several files fan out a little, like a stack.
            let offset = i.min(4) as f64 * 6.0;
            let frame = NSRect::new(
                NSPoint::new(at.x - ICON_SIDE / 2.0 + offset, at.y - ICON_SIDE / 2.0 + offset),
                NSSize::new(ICON_SIDE, ICON_SIDE),
            );
            let contents: &AnyObject = &icon;
            // SAFETY: the contents of a dragging item may be an NSImage, as here.
            unsafe { item.setDraggingFrame_contents(frame, Some(contents)) };
            item
        })
        .collect();

    let source = SOURCE.with(|s| s.get_or_init(|| DragSource::new(mtm)).clone());
    let session = view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    session.setAnimatesToStartingPositionsOnCancelOrFail(true);
    DRAG_WINDOW.with(|w| *w.borrow_mut() = Some(window));
    true
}

/// Gives the window the button release that the drag session swallowed.
fn release_button(mtm: MainThreadMarker, screen_point: NSPoint) {
    let Some(window) = DRAG_WINDOW.with(|w| w.borrow_mut().take()) else { return };
    let up = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseUp,
        window.convertPointFromScreen(screen_point),
        NSEventModifierFlags::empty(),
        uptime(),
        window.windowNumber(),
        None,
        0,
        1,
        0.0,
    );
    if let Some(up) = up {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&up, false);
    }
}
