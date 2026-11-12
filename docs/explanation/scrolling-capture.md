# How scrolling and panoramic capture work

Both modes solve the same problem: building one large image from many overlapping screenshots. The difference is who does the scrolling.

## Automatic scrolling capture

1. SnapKit brings the window under the area to the front, so nothing covers it during the capture.
2. It scrolls the area back to the start (top or left) with the mouse wheel.
3. It takes a shot, scrolls a step, waits until two shots in a row are identical (the scroll animation has finished), and takes the next shot.
4. For each new shot, it works out how far the content moved and appends only the new part.
5. It stops when two consecutive shots are identical (the end of the content), when you press **Esc**, or when a [limit](../reference/requirements-and-limits.md#capture-limits) is reached.

Short areas get smaller scroll steps so shots always overlap. If an app ignores a step, SnapKit tries again before deciding it has reached the end. If a step jumps too far, it goes back and retries with one-notch steps.

For both directions, SnapKit captures a full vertical strip for each horizontal position and joins the strips side by side.

## Panoramic capture

In panoramic mode, SnapKit watches the area while you scroll. Each new shot is compared with the previous one to find how far the content moved, in any of the four directions, and pasted into a growing mosaic at that position.

If a shot can't be placed, usually because you scrolled too far between two shots, it's counted as lost and skipped. The next shot is compared with the last good one, so you can recover by scrolling back a little.

## How SnapKit finds the overlap

SnapKit uses two methods, fastest first.

**Row signatures.** Every row of pixels gets a short fingerprint. If the whole area scrolled together, the fingerprints of the new shot match a run of fingerprints in the old one, and the offset is exact. This method also detects fixed headers and footers: rows that are identical at the top and bottom of both shots don't move, so they're kept once instead of repeated. The right edge is ignored, because the scroll bar moves in every shot.

**Pixel voting.** When parts of the area don't move, such as a fixed sidebar or a wallpaper behind chat bubbles, whole rows no longer match. SnapKit then samples thin strips of pixels along the direction of movement and lets each "informative" pixel (one that differs from its neighbor) vote for the offsets where it matches. Strips that don't move at all don't vote. The offset with the most votes wins, as long as at least a third of the evidence agrees. Small color differences from anti-aliasing or compression are tolerated.

## Why moving content breaks it

Both methods assume that, between two shots, the content only moved. A playing video, an animated banner or a live counter changes pixels that should have stayed the same. When too much changes, no offset wins and SnapKit stops with **the content changed in a way that could not be stitched**. Pausing animations, or capturing only the part that scrolls, avoids the problem.

## Related

- [Capture a long page with scrolling capture](../how-to/capture-scrolling.md)
- [Capture an area you scroll yourself](../how-to/capture-panoramic.md)
