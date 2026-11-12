# Contributing to SnapKit

Thanks for your interest in SnapKit. Bug reports, feature ideas, documentation fixes and code are all welcome.

## Report a bug or suggest a feature

[Open an issue](https://github.com/MichelGSS/snapkit/issues/new/choose) and pick the template that fits. For bugs, include:

- What you were doing, what you expected and what happened.
- Your Windows version, the number of monitors and their scaling.
- A screenshot or a short recording, if it helps. Remember to hide anything private.

Please search the existing issues first.

## Improve the documentation

The user guide lives in [`docs/`](docs/README.md) and follows the [Diátaxis](https://diataxis.fr) framework:

| Folder | Content | Write it as… |
|---|---|---|
| `docs/tutorials/` | Learning-oriented lessons | A guided lesson with a concrete result |
| `docs/how-to/` | Goal-oriented procedures | Numbered steps for one task |
| `docs/reference/` | Facts to look up | Tables and lists, no instructions |
| `docs/explanation/` | Background and reasons | Discussion, no steps |

Follow the [Microsoft Writing Style Guide](https://learn.microsoft.com/style-guide/welcome/):

- Address the reader as **you**, use active voice and the present tense.
- Use sentence-style capitalization for headings.
- Write UI labels in **bold**, exactly as they appear in the English interface.
- Use **select** for buttons, menus and options; **press** for keys; **drag** for mouse drags.
- Write key combinations as **Ctrl+Shift+S**.
- Keep one action per numbered step.

## Contribute code

### Set up

1. Install [Rust](https://rustup.rs) (stable) and the Visual Studio Build Tools with the **Desktop development with C++** workload.
2. Fork and clone the repository.
3. Run `cargo run` to build and start SnapKit.
4. Optionally, run the `scripts\fetch-*.ps1` scripts to enable OCR and video. See [Build SnapKit from source](docs/how-to/build-from-source.md).

### Make a change

1. Create a branch from `main`.
2. Keep the change focused on one thing.
3. Add or update tests next to the code you change (`#[cfg(test)] mod tests` at the end of each file).
4. Run the tests:

   ```powershell
   cargo test
   ```

5. Update the user guide in `docs/` if the change affects what users see or do.
6. Open a pull request that explains the problem and the solution.

### Code guidelines

- **No network access.** SnapKit is 100% local; features that need a server don't fit the project.
- **Every user-facing text in both languages.** Write it with the `t!` or `tf!` macro, Portuguese first and English second: `t!("Copiar", "Copy")`. See [`src/i18n.rs`](src/i18n.rs).
- **Comments in English**, explaining why rather than what.
- **Match the surrounding style.** The code isn't run through `cargo fmt`, so keep lines and layout consistent with the file you're editing.
- **Windows API calls** go in [`src/win.rs`](src/win.rs), each `unsafe` block with a `SAFETY:` comment.
- **Original work only.** Don't copy code, icons, texts or layouts from other screen capture products.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
