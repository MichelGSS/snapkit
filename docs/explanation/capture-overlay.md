# How the capture overlay works

When you start a region capture, SnapKit doesn't take a screenshot and show you a frozen copy, as many tools do. It turns its own window into a transparent layer over all your monitors, and the screen underneath stays live.

## Why a live overlay

- **Nothing is frozen.** Hover effects, menus and videos keep working while you select.
- **The capture happens when you release the mouse**, not when you started. What you see in the selection is what you get.
- **Windows are detected live.** SnapKit lists the visible windows when the selection starts and outlines the one under the pointer, so you can capture a window with one click.

## Staying out of its own screenshots

The overlay is SnapKit's own window. To keep it out of the picture, SnapKit asks Windows to exclude that window from screen captures (`SetWindowDisplayAffinity` with `WDA_EXCLUDEFROMCAPTURE`, available since Windows 10 version 2004). The overlay is still visible to you but invisible to any screen capture, including SnapKit's own. That's why the shot can be taken while the overlay is still on the screen, with no delay.

The same technique hides the small panels and the orange frames shown during recordings and scrolling captures.

## Videos keep playing

Chrome, Edge and Firefox stop drawing a page when another window covers theirs completely, to save power, and a video then looks paused. They can't tell that SnapKit's overlay is transparent. While the overlay is up, SnapKit marks it as a tool window, a kind of window browsers don't count as covering them. That also keeps the overlay out of the taskbar and Alt+Tab.

## No flicker between modes

Switching the main window into the overlay means resizing it to cover every monitor and removing its frame, which takes a few frames. To avoid showing the in-between states, SnapKit cloaks the window (it stays active but the compositor doesn't draw it) until it has been drawn a few times at the right size. The same happens on the way back, so the editor never flashes on the screen before a quick annotation copy hides SnapKit again.

## Why OpenGL

A transparent window on Windows needs a renderer that supports per-pixel transparency. SnapKit uses egui with its OpenGL renderer for this; the default renderer draws a black background instead. That's why SnapKit needs OpenGL, which some remote sessions and virtual machines don't provide.

## Coordinates across monitors

SnapKit works in physical pixels of the virtual desktop, the rectangle that contains all monitors. A region can start on one monitor and end on another. When monitors have different heights, the parts of a capture that fall outside every monitor come out black.

## Related

- [Capture a region, a window or the whole screen](../how-to/capture-region-window-screen.md)
- [System requirements and limits](../reference/requirements-and-limits.md)
