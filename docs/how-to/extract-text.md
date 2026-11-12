# Copy text from an image (OCR)

SnapKit can recognize the text in any capture and copy it to the clipboard, so you don't have to retype error messages, log lines, code or settings. Recognition runs entirely on your computer.

SnapKit reads English and Portuguese, keeps indentation and blank lines, and has a separate engine for handwriting.

## Copy all the text in a capture

1. Open the capture in the editor.
2. Select **Read text** in the top bar.

After a moment, SnapKit copies the text and opens the **Extracted text** window. The status bar says how many lines it read and which engine it used.

In the **Extracted text** window, you can correct the text and select **Copy** to copy it again.

## Copy the text from one area

1. In the editor, select **Grab text** in the **Image** group of the tool column.
2. Drag over the text you want.

SnapKit reads only that area and copies the text. The image isn't changed.

## Read handwriting or decorative lettering

If the result is poor because the text is handwritten, stylized or over a photo:

1. In the **Extracted text** window, select **Read as handwriting**.

SnapKit reads the same area again with its neural engine. This engine handles handwriting and decorative fonts better, but it's worse for code, because it loses capital letters and indentation.

## Get better results

- **Capture at 100% zoom or larger.** Tiny text is harder to read. Zoom in the app before you capture.
- **Prefer screenshots to photos.** Screen text is read much more accurately than text in a photo of a screen.
- **Select only the text.** With **Grab text**, leaving out icons and buttons avoids stray characters.

If you see **No text found in that area**, try a larger area or the handwriting engine.

## Related

- [How text recognition works](../explanation/text-recognition.md)
- [Troubleshoot common problems](troubleshoot.md#text-recognition-isnt-available)
