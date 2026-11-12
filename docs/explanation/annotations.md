# How annotations work

## Annotations are objects, not paint

When you draw an arrow in SnapKit, it isn't painted onto the image right away. SnapKit keeps a list of objects (each arrow, rectangle, text, marker and blur) and redraws the image from the original capture every time something changes.

That design is what makes these things possible:

- **Change anything later.** Select an arrow you drew ten steps ago and change its color, or fix a typo in a callout.
- **Undo in any order of complexity.** Undo goes back to an earlier version of the whole list, so undoing a move or a color change is as reliable as undoing a new arrow.
- **Erase without damage.** The **Eraser** restores pixels from the original capture, so it never leaves a white hole.
- **Pixelation that really hides.** Pixelate and blur are applied to the final image, so the hidden text isn't recoverable from a copied or saved file.

## When annotations become part of the image

The object list is tied to the image's coordinate system. Some changes replace the whole image with a new one:

- **Crop** and **Cut out**
- **Rotate**, **Flip** and **Resize**
- **Border** and **Shadow**

After one of these, the annotations drawn before it are drawn into the new image and can't be selected anymore. That's why the guide recommends cropping and resizing first and annotating afterward.

## What the library stores

The library stores the final image as a PNG file, the same image you'd get with **Copy** or **Save PNG**. It doesn't store the object list.

That keeps the library simple and portable: every capture is a normal image any app can open. The trade-off is that when you reopen a capture from the library in a later session, its earlier annotations are already part of the image. You can add new annotations on top, but you can't select the old ones.

While a capture is open in the editor, SnapKit saves the image back to the library automatically after you stop editing for a moment, so the library always has your latest version.

## Quick annotation uses the same model

Quick annotation is the same editor, drawn in place over the screen. That's why you can press **Ctrl+E** at any moment and continue in the full editor with every annotation still editable.

When you move or resize a quick annotation, SnapKit takes a new shot of the new area and shifts the object list to match, so your annotations stay editable through the change.

## Related

- [Change or remove an annotation](../how-to/change-annotations.md)
- [Editor tools and options](../reference/editor-tools.md)
