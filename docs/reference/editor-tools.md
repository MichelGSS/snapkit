# Editor tools and options

The editor has three parts:

- **Tool column** (left): every tool, in groups.
- **Options bar** (above the image): **Undo**, **Redo**, the **Styles** and **Image** menus, the name of the current tool or selected annotation, and that tool's options. With no annotation selected, it also shows a one-line hint on how to use the tool.
- **Footer** (below the image): image size and zoom controls.

## Tools

| Group | Tool | How to use it | Options |
|---|---|---|---|
| Edit | **Select** | Click an annotation to move, resize, change or delete it. | Those of the selected annotation, plus **Duplicate** and **Delete** |
| | **Eraser** | Drag over an area to restore the original capture there. | **Thickness** |
| Draw | **Arrow** | Drag from the start to the target. | Color, **Thickness**, **Dashed**, **Two heads** |
| | **Line** | Drag from one end to the other. | Color, **Thickness**, **Dashed** |
| | **Rectangle** | Drag from corner to corner. | Color, **Thickness**, **Fill**, **Dashed** |
| | **Ellipse** | Drag from corner to corner of its bounding box. | Color, **Thickness**, **Fill**, **Dashed** |
| | **Pen** | Drag to draw freehand. | Color, **Thickness** |
| Explain | **Text** | Click where the text starts and type. | Color, **Size**, **Background** |
| | **Callout** | Drag from the point to explain to where the balloon goes, then type. | Color, **Size**, **Style** |
| | **Step** | Each click places the next number or letter. | Color, **Size**, **Shape**, label type; **No.** when selected |
| | **Stamp** | Each click places the chosen symbol. | Color, **Size**, **Symbol** |
| Highlight | **Highlighter** | Drag over text to mark it with translucent yellow. | Color when selected |
| | **Spotlight** | Drag over the area that should stay bright. | None |
| | **Magnifier** | Drag over a detail; move and resize the enlarged copy. | **Zoom** |
| Hide | **Pixelate** | Drag over what should become unreadable. | None |
| | **Blur** | Drag over what should become unreadable. | None |
| Image | **Crop** | Drag over the part of the image to keep. | None |
| | **Cut out** | Drag down to remove a horizontal strip, or sideways to remove a vertical one. | None |
| | **Grab text** | Drag over text to recognize and copy it. | None |

The quick annotation toolbar has a subset of these tools: **Select**, **Arrow**, **Rectangle**, **Pen**, **Highlighter**, **Text**, **Step**, **Pixelate** and **Blur**.

## Option values

| Option | Values |
|---|---|
| **Thickness** / **Size** | 1 to 24 |
| **Zoom** (Magnifier) | 1 to 24, which enlarges the copy from about 1.6× to 4.5× |
| **Style** (Callout) | **Rounded**, **Square**, **Outline** |
| **Shape** (Step) | **Circle**, **Square**, **Diamond**, **Hexagon**, **Pin** |
| Label type (Step) | **1 2 3**, **A B C**, **a b c** |
| **No.** (selected Step) | 1 to 999 |
| **Symbol** (Stamp) | **Check**, **Wrong**, **Warning**, **Info**, **Question**, **Star**, **Plus**, **Minus**, **Right arrow**, **Left arrow**, **Up arrow**, **Down arrow**, **Heart**, **Flag**, **Pushpin**, **Cursor** |
| Color | Palette swatches, or **Other color** for any color |

## Styles menu

| Item | Action |
|---|---|
| A style name | Switch to that style's tool and options |
| **×** next to a style | Delete the style |
| **Name (optional)** and **Save** | Save the current tool and options as a new style |

## Image menu

| Item | Action |
|---|---|
| **Rotate left** / **Rotate right** | Rotate the whole image 90 degrees |
| **Flip horizontally** / **Flip vertically** | Mirror the whole image |
| **Size** (width × height, in pixels), **Keep proportions**, **Apply size** | Resize the whole image, up to 20,000 pixels per side |
| **Frame**: width in pixels and **Border in the current color** | Add a solid border |
| **Shadow** | Add a white margin with a soft drop shadow |

## Footer

| Control | Action |
|---|---|
| Image size | Width × height of the image, in pixels |
| **-** / **+** | Zoom out (**Ctrl+Minus**) / zoom in (**Ctrl+Plus**), with the current zoom percentage between them |
| **Fit** | Fit the whole image in the window (**Ctrl+0**) |
| **100%** | Actual size (**Ctrl+1**) |

## Related

- [Keyboard shortcuts](keyboard-shortcuts.md#editor)
- [How annotations work](../explanation/annotations.md)
