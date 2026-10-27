# Contributing

Read this document before you send a change.

## 1. Requirements

| Item        | Value                                                        |
| ----------- | ------------------------------------------------------------ |
| Rust        | Version 1.98 or later.                                        |
| macOS       | The only supported operating system for a feed.               |
| libgphoto2  | Necessary to build the `camera-core` crate.                   |
| pkg-config  | Necessary to find libgphoto2.                                 |
| OBS Studio  | Necessary to run a feed. Not necessary to build or to test.   |

Install the system packages:

```
brew install libgphoto2 pkg-config
```

On Linux, the equivalent packages are `libgphoto2-dev` and `pkg-config`.

## 2. Build and test

Do these commands in this sequence:

```
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests do not need a camera. They use a synthetic PTP session, a fake
obs-websocket server and committed JPEG frames. A test that needs a real camera
is not accepted.

## 3. Coding rules

- Keep the workspace lint settings. Do not add `#[allow(...)]` to a crate. The
  one module that allows `unsafe` is `camera-core/src/platform/macos/device_probe.rs`.
- Put a new behaviour behind a test. Write the test first when you can.
- Keep the public documentation in ASD-STE100 Simplified Technical English. Use
  the words in `GLOSSARY.md`. Use short sentences and one instruction per
  sentence.
- If a change adds or changes a command, an option, a configuration key or a
  prerequisite, change the matching document in `docs/user/` in the same change.
- Do not commit a configuration file. `canon-webcam.toml` holds the OBS WebSocket
  password and is in `.gitignore`.

## 4. Architecture decisions

Read `docs/adr/` before you change the capture layer, the sink layer or the
virtual camera mechanism. If your change replaces a decision, add a new ADR. Do
not edit an accepted ADR to hide the old decision.

## 5. Send a change

1. Make a branch.
2. Do the steps in Section 2.
3. Write a commit message that says what changed and why.
4. Open a pull request. Give the output of the four commands in Section 2.
5. If the change touches a real camera or a real OBS, say what you tested and
   what you did not test.

Do not open a pull request for a security problem. Read `SECURITY.md`.

## 6. Licence

Send your change under the MIT licence. Read `LICENSE`.
