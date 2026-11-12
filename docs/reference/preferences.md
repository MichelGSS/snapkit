# Preferences

Select **Preferences** in the top bar to open the **Preferences** window. Every change is saved immediately to `%APPDATA%\SnapKit\config.json`.

| Setting | Values | Default |
|---|---|---|
| **Global capture shortcut** | Any combination of **Ctrl**, **Alt**, **Shift** and **Win**, plus a key | **Alt+Shift+S** |
| **After choosing the region, annotate right there (Enter copies)** | On or off | On |
| **Language** | **English**, **Português** | The Windows display language |
| **Interface theme** | See [Interface themes](#interface-themes) | **Default** |
| **Closing the window keeps SnapKit in the tray** | On or off | On |
| **Start with Windows (in the tray)** | On or off | Off |

## Global capture shortcut

Starts a region capture from any app.

- **Modifiers:** **Ctrl**, **Alt**, **Shift**, **Win**, in any combination.
- **Keys:** **A**–**Z**, **0**–**9**, **F1**–**F12** and **PrintScreen**.
- A letter or digit needs at least one of **Ctrl**, **Alt** or **Win**, so it doesn't block normal typing. Function keys and **PrintScreen** can be used alone.

Select **Apply shortcut** to register the new combination. If Windows or another app already uses it, SnapKit keeps the previous shortcut and shows a message.

## Interface themes

| Theme | Look |
|---|---|
| **Default** | Dark graphite with an orange accent |
| **Dracula** | Dark purple background, lilac accent |
| **Nord** | Steel blue background, ice blue accent |
| **One Dark** | Dark gray background, blue accent |
| **Monokai** | Greenish black background, pink accent |
| **Gruvbox Dark** | Warm gray background, orange accent |
| **Tokyo Night** | Night blue background, light blue accent |
| **Catppuccin Mocha** | Purplish blue background, light lilac accent |
| **Solarized Light** | The only light theme: beige background, blue accent |

The accent color is used for selections, links, the capture frame and the handles in the editor.

## Settings saved outside the Preferences window

SnapKit also remembers these choices in the same file:

| Setting | Where you change it | Default |
|---|---|---|
| Capture delay | **Options** > **Wait before capturing** | No delay |
| Include the mouse cursor | **Options** > **Include the mouse cursor** | Off |
| Record system sound | Video panel > **System sound** | On |
| Record microphone | Video panel > **Microphone** | Off |
| Styles | Editor > **Styles** | None |

## config.json

`config.json` is a plain JSON file. You can back it up or copy it to another computer. If it's missing or can't be read, SnapKit starts with the defaults; missing fields also take their defaults.

```json
{
  "hotkey": { "ctrl": false, "alt": true, "shift": true, "win": false, "key": "S" },
  "delay_secs": 0,
  "include_cursor": false,
  "close_to_tray": true,
  "record_system_audio": true,
  "record_microphone": false,
  "quick_annotate": true,
  "styles": [],
  "theme": "Padrão",
  "language": "en"
}
```

| Field | Type | Description |
|---|---|---|
| `hotkey` | object | Modifier flags and the key name, as listed in [Global capture shortcut](#global-capture-shortcut) |
| `delay_secs` | number | Capture delay in seconds: `0`, `3`, `5` or `10` |
| `include_cursor` | boolean | Draw the mouse pointer in captures |
| `close_to_tray` | boolean | The window's **X** hides SnapKit instead of quitting |
| `record_system_audio` | boolean | Record system sound in videos |
| `record_microphone` | boolean | Record the microphone in videos |
| `quick_annotate` | boolean | Use quick annotation for region captures |
| `styles` | array | Saved editor styles |
| `theme` | string | Theme identifier. The identifiers are the Portuguese names, so a choice holds in both languages: `Padrão` (Default), `Dracula`, `Nord`, `One Dark`, `Monokai`, `Gruvbox Escuro`, `Tokyo Noite`, `Catppuccin Mocha`, `Solarized Claro`. |
| `language` | string or `null` | `pt` or `en`. `null` means "follow the Windows display language". |

**Start with Windows** isn't stored in this file. It's an entry named `SnapKit` under `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.

## Related

- [Customize SnapKit](../how-to/customize.md)
- [Files and folders](files-and-folders.md)
