# Capture modes

All capture buttons are on the left side of the SnapKit top bar. When the window is narrow, the bar wraps to two or three rows.

| Button | What it captures | Selection | Result opens in |
|---|---|---|---|
| **Region** | An area you drag, or the window you select | Live overlay on all monitors | Quick annotation, or the editor if quick annotation is off or a delay is set |
| **Full screen** | The monitor the pointer is on | None | Editor |
| **All screens** | Every monitor in one image. Shown only with two or more monitors. | None | Editor |
| **Scrolling** | A scrolling area, from start to end, scrolled automatically | Window or dragged area | Editor |
| **Panoramic** | An area you scroll by hand, in any direction | Window or dragged area | Editor |
| **Video** | An area recorded as MP4 with sound | Window or dragged area | **Recording saved** window |
| **GIF** | An area recorded straight to an animated GIF, without sound | Window or dragged area | Status bar message with the file name |
| **Step by step** | A shot of an area at every click and **Enter** | Window or dragged area | Editor, as one stacked guide |

## Selection overlay

When a mode needs a selection, SnapKit shows a transparent overlay over all monitors. The screen stays live underneath.

| Element | Description |
|---|---|
| Window outline | The window under the pointer, with its size in pixels. Select it to capture the whole window. |
| Magnifier | An enlarged view of the pixels around the pointer, for precise edges. |
| Scrolling arrows | In **Region** mode, over windows larger than about 180 × 140 points: vertical, horizontal and both-ways scrolling capture. |
| Size label | The size of the current selection, in pixels. |
| Confirmation bar | For an adjustable region: **Capture**, **Redo** and **✕**. |

### Adjustable region

In **Scrolling**, **Panoramic**, **Video**, **GIF** and **Step by step** modes, and in **Region** mode with a delay or with quick annotation off, a dragged area isn't captured immediately:

- Drag the middle of the area to move it.
- Drag one of the eight handles to resize it.
- Confirm with **Enter**, **Ctrl+C**, a double-click inside the area, or **Capture**.
- Select **Redo** to draw the area again, or press **Esc** to cancel.

## Options menu

| Option | Values | Default |
|---|---|---|
| **Wait before capturing** | **No delay**, **Delay 3 s**, **Delay 5 s**, **Delay 10 s** | No delay |
| **Include the mouse cursor** | On or off | Off |

The **Options** button shows the active options, for example **Options: Delay 5 s, with cursor**.

## Limits

| Mode | Limit |
|---|---|
| Scrolling | 80 shots or 32,000 pixels in the scrolling direction |
| Step by step | 40 steps per session |
| Region | At least 4 × 4 pixels; smaller selections are canceled |

## Related

- [Capture a region, a window or the whole screen](../how-to/capture-region-window-screen.md)
- [How the capture overlay works](../explanation/capture-overlay.md)
