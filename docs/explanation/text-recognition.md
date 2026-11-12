# How text recognition works

SnapKit bundles two OCR engines and a layout step that rebuilds the text's shape. Everything runs on your computer.

## Two engines

| Engine | Strengths | Weaknesses | Used |
|---|---|---|---|
| **Tesseract** 5 | Screen text, code, configuration files, capital letters, punctuation | Handwriting, decorative fonts, text over photos | By default |
| **Neural engine** (PP-OCRv3 models on ONNX Runtime) | Handwriting, stylized lettering, text on busy backgrounds | Loses capital letters and indentation in code | With **Read as handwriting**, or when Tesseract is missing or fails |

Tesseract reads English and Portuguese. Both language models are loaded together, so mixed text works without choosing a language.

## Preparing the image

Before recognition, SnapKit prepares a copy of the image:

- **Light background.** Both engines read dark text on a light background best, so mostly dark images, such as a terminal or a dark-themed editor, are inverted.
- **Enlarged.** Screen text is small. Scaling it up three times helps Tesseract the most.
- **Margin.** Small areas get a margin in the corner's color, so the engines don't mistake the edge for a stroke.
- **Tiles.** Very tall images, such as scrolling captures, are split into chunks. Cuts are made on rows of uniform color, between lines of text, so letters are never cut in half.

## Rebuilding the layout

The engines return words and their positions, not formatted text. SnapKit rebuilds the text from the positions:

- Words whose vertical centers line up become one line, even if the baseline is slightly uneven.
- Indentation and wide gaps are reproduced with spaces, based on the typical character width.
- A vertical gap much larger than normal becomes a blank line.
- Punctuation that the OCR split from a word, such as `enabled :`, is reattached.
- Typographic quotes become straight quotes, because only those are valid in code.
- Words with very low confidence, which are usually icons read as letters, are dropped.

This is why code and YAML copied from a screenshot usually keep their indentation and line breaks.

## Why the neural engine works in two steps

The neural engine uses two models. The first marks, pixel by pixel, where there's text; the marked blobs become one rectangle per line. The second reads each rectangle, scaled to a fixed height, and returns the most likely sequence of characters. Lines it isn't confident about are dropped rather than guessed.

## Related

- [Copy text from an image](../how-to/extract-text.md)
- [Third-party notices](../../THIRD-PARTY-NOTICES.md)
