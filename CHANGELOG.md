# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-27

### Added

- Live view capture from a Canon EOS M50 over USB/PTP, through libgphoto2.
- One exclusive PTP session for the whole run, with detection of a wedged or a
  disconnected camera, and recovery through `gphoto2 --reset`.
- A bounded frame queue that drops the oldest frame, a calibration window, and a
  measurement of the true frame size and frame rate.
- The macOS virtual camera sink: a loopback WebSocket frame transport, a browser
  source in OBS, and obs-websocket control of OBS Virtual Camera.
- `camera-cli` with the commands `measure`, `stream`, `serve`, `control` and
  `help`.
- Start and stop control through a Unix socket, without a new PTP session.
- A TOML configuration file with validation that names the offending key.

### Known limits

- macOS only. The Windows and Linux backends are not implemented.
- Canon EOS M50 only. No other camera is tested.
- 1024 by 576 pixels at about 24 frames per second. This is the USB/PTP limit.
- OBS Studio must be installed for a virtual camera.
