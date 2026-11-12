# Capture a long page with scrolling capture

Scrolling capture takes a whole web page, document, chat or list that doesn't fit on the screen. SnapKit scrolls the area for you, takes a shot at each step and stitches them into one tall (or wide) image.

## Capture with the Scrolling button

1. Open the page you want to capture and scroll it to where the capture should start. SnapKit scrolls back to the top before it starts.
2. In SnapKit, select **Scrolling** in the top bar.
3. Select the window, or drag over the part that scrolls, for example only the content pane of a browser.
4. If you dragged an area, adjust it if you need to and press **Enter**, or select **Capture**. Selecting a window starts right away.

SnapKit hides its window, frames the area in orange and scrolls down until the end of the content. The result opens in the editor. Press **Esc** to stop early; what was captured so far is kept.

## Capture horizontally or in both directions

1. Press **Alt+Shift+S**, or select **Region**.
2. Move the pointer over a large window. Arrows appear over it.
3. Select the arrow for the direction you want:

   | Arrow | Captures |
   |---|---|
   | Vertical | The page from top to bottom |
   | Horizontal | The page from left to right, for wide tables and timelines |
   | Both ways | A full grid, for content that's larger than the window in both directions |

## Get a clean result

- **Close or pause moving content.** Videos, animations and live counters change between shots and can stop the stitching.
- **Keep the pointer still.** SnapKit scrolls with the mouse wheel over the area. Moving the mouse during the capture sends the scrolling elsewhere.
- **Select only the part that scrolls.** Fixed headers and footers are detected automatically, but a fixed sidebar inside the area adds work. Selecting only the scrolling pane gives the best results.
- **Use panoramic capture for difficult pages.** Some apps scroll in uneven steps or load content as you go. With [panoramic capture](capture-panoramic.md), you scroll yourself, at your own pace.

## Limits

A scrolling capture stops at 80 shots or 32,000 pixels, whichever comes first. The editor shows why the capture stopped, for example **reached the end of the scroll**.

## Related

- [Capture an area you scroll yourself](capture-panoramic.md)
- [How scrolling and panoramic capture work](../explanation/scrolling-capture.md)
