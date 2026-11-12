# Security policy

## Supported versions

Security fixes are released for the latest version of SnapKit. Please update before you report a problem.

## Report a vulnerability

Please don't report security problems in public issues.

Use GitHub's [private vulnerability reporting](https://github.com/MichelGSS/snapkit/security/advisories/new) instead. Include:

- The SnapKit version and your Windows version.
- What an attacker could do, and the steps to reproduce it.
- Any proof of concept, logs or screenshots.

You can expect an acknowledgment within a week. Once a fix is ready, it's released and credited to you in the release notes, unless you prefer to stay anonymous.

## Scope

SnapKit doesn't open network connections, so most issues will involve local files, the bundled third-party programs (Tesseract, FFmpeg, ONNX Runtime) or the way SnapKit starts them. Vulnerabilities in those components should also be reported to their own projects.
