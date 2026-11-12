# Why SnapKit is 100% local

SnapKit doesn't have an account, a cloud service, an update checker, analytics or telemetry. It never opens a network connection. Everything it does, including text recognition and video encoding, runs on your computer.

## Screenshots are sensitive by nature

People who write documentation capture what they're working on: admin consoles, customer tickets, internal dashboards, code, configuration files, error messages with tokens in them. A screenshot tool sees all of it.

Many capture tools upload images to a server to share them, to recognize text, or to "improve the product". Even when that's done carefully, it means your screenshots leave your control. In regulated environments, such as healthcare, finance or public services, it can also mean a tool isn't allowed at all.

SnapKit takes the opposite approach: if a feature can't run locally, SnapKit doesn't have it.

## What "local" means in practice

| Feature | How it stays local |
|---|---|
| Capture | Windows screen capture APIs, inside the SnapKit process |
| Annotation | Drawn in memory by SnapKit's own renderer |
| Text recognition | Tesseract and a neural model, both bundled and run on your CPU |
| Video and GIF | FFmpeg, bundled, run as a local process |
| Library | PNG files and a JSON index in your **Pictures** folder |
| Preferences | A JSON file in `%APPDATA%\SnapKit` |

Because nothing depends on a server, SnapKit works the same on a plane, in an air-gapped network or behind a strict proxy.

## Free and open

SnapKit is released under the [MIT License](../../LICENSE). You can use it at work and at home, share it with your team, read every line of the source code, and change it. There's no paid tier and no feature held back.

The bundled third-party components keep their own open-source licenses. They're listed in [THIRD-PARTY-NOTICES.md](../../THIRD-PARTY-NOTICES.md).

## Lightweight by design

SnapKit is a single native Windows program written in Rust. It doesn't bundle a browser engine, doesn't install services and doesn't run anything in the background beyond its own process. When you quit it, nothing of it keeps running.

## Your data stays yours

- Your captures are regular PNG files you can move, back up or delete.
- Uninstalling SnapKit leaves your captures in place.
- To remove every trace of SnapKit, delete three folders. See [Files and folders](../reference/files-and-folders.md).

## Related

- [Hide passwords and personal data](../how-to/hide-sensitive-data.md)
- [How text recognition works](text-recognition.md)
