# Capture a region, a window or the whole screen

Use these captures for most screenshots. They take a single shot of what's on the screen right now.

## Capture a region

1. Press **Alt+Shift+S** from any app, or select **Region** in the SnapKit top bar.
2. Drag from one corner of the area to the opposite corner.

   The size of the selection appears next to it, and a magnifier next to the pointer helps you line up the edges pixel by pixel.

3. Release the mouse button.

What happens next depends on [Preferences](../reference/preferences.md):

- With quick annotation on (the default), the capture stays on the screen with a toolbar. Annotate it and press **Enter** to copy it. See [Annotate right on the screen](quick-annotation.md).
- With quick annotation off, the capture opens in the editor.

A region can span several monitors. Press **Esc** at any time to cancel.

## Capture a window

1. Press **Alt+Shift+S**, or select **Region**.
2. Move the pointer over the window. SnapKit outlines it and shows its size.
3. Select the window (single click, without dragging).

SnapKit captures the whole window, without the desktop around it.

> [!TIP]
> On large windows, arrows appear over the outlined window. Select an arrow to capture the window with automatic scrolling instead. See [Capture a long page](capture-scrolling.md).

## Capture the whole screen

Select **Full screen** in the top bar. SnapKit captures the monitor the pointer is on and opens the result in the editor.

## Capture every monitor

If you have more than one monitor, select **All screens** in the top bar. SnapKit captures every monitor in a single image.

When the monitors have different heights, the empty space around the shorter ones comes out black.

## Include the mouse pointer

By default, the pointer isn't included. To include it:

1. Select **Options** in the top bar.
2. Select **Include the mouse cursor**.

The setting applies to every capture until you turn it off. The **Options** button shows **with cursor** while it's on.

## Related

- [Capture menus and tooltips with a delay](capture-with-delay.md)
- [Capture modes](../reference/capture-modes.md)
- [How the capture overlay works](../explanation/capture-overlay.md)
