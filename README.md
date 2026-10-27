# Canon M50 Virtual Camera

## 1. Purpose

This document set tells you how to build, install and use this software.

The software makes a Canon EOS M50 camera operate as a camera for other computer programs. Examples of these programs are Discord, Microsoft Teams and Google Meet.

The Canon EOS M50 has no USB video class mode. The software reads the **live view** of the camera over USB. Then it sends the live view to the macOS **virtual camera**.

## 2. Status of this release

| Item                     | Status                                                   |
| ------------------------ | -------------------------------------------------------- |
| Operating system         | macOS only. The Windows and Linux backends are not made. |
| Camera                   | Canon EOS M50 only. No other camera is tested.           |
| Virtual camera mechanism | OBS Virtual Camera. This is the only mechanism.          |
| Frame size               | 1024 pixels by 576 pixels. This is the maximum.          |
| Frame rate               | About 24 frames per second.                              |
| Resolution 1080p         | Not possible over USB.                                   |

Read [Document 8](docs/user/08-device-compatibility.md) before you install this software on a different computer or with a different camera.

## 3. Document index

| Document                                  | Content                                             |
| ----------------------------------------- | --------------------------------------------------- |
| [1](docs/user/01-requirements.md)         | Requirements: computer, software, camera, OBS.      |
| [2](docs/user/02-build-and-install.md)    | Clone, build, install, and verify the installation. |
| [3](docs/user/03-camera-setup.md)         | Camera connection and camera settings.              |
| [4](docs/user/04-obs-setup.md)            | OBS installation and OBS settings.                  |
| [5](docs/user/05-command-reference.md)    | Commands, options, examples, exit codes.            |
| [6](docs/user/06-configuration.md)        | Configuration file reference.                       |
| [7](docs/user/07-troubleshooting.md)      | Messages, causes and actions.                       |
| [8](docs/user/08-device-compatibility.md) | Camera and computer compatibility.                  |

Related documents:

| Document          | Content                                                       |
| ----------------- | ------------------------------------------------------------- |
| `GLOSSARY.md`     | The words this document set uses.                             |
| `docs/adr/`       | The architecture decisions and the evidence for them.         |
| `docs/internal/`  | The development records: plan, tickets, research and results. |
| `LICENSE`         | The licence of this software.                                 |
| `CONTRIBUTING.md` | How to build, test and send a change.                         |
| `SECURITY.md`     | How to report a security problem.                             |

## 4. Short procedure

Do these steps in this sequence:

1. Read [Document 1](docs/user/01-requirements.md). Make sure that you have the necessary software and hardware.
2. Read [Document 2](docs/user/02-build-and-install.md). Build and install the software.
3. Read [Document 4](docs/user/04-obs-setup.md). Install OBS and set its WebSocket server.
4. Connect the camera. Do the steps in [Document 3](docs/user/03-camera-setup.md).
5. Start the feed. Do the steps in [Document 5](docs/user/05-command-reference.md).

NOTE: Do step 3 before step 5. The software cannot make a virtual camera without OBS.

## 5. Words used in this document set

This document set uses the terms in `GLOSSARY.md`. Table 1 gives the most important terms.

**Table 1 — Important terms**

| Term                | Meaning                                                                    |
| ------------------- | -------------------------------------------------------------------------- |
| Live view           | The continuous preview that the camera sends over USB.                     |
| PTP session         | The one exclusive handle to the camera. Only one can exist.                |
| Sink                | The part of the software that receives frames and publishes them.          |
| Virtual camera      | The camera that other programs select. OBS makes it.                       |
| Wedged camera       | The camera is on the USB bus, but each operation fails with `PTP Timeout`. |
| Disconnected camera | The camera is not on the USB bus.                                          |
| Recovery reset      | The command `gphoto2 --reset`. It destroys a wedged PTP session.           |
| Frame size          | The pixel size of one live view frame.                                     |

## 6. Safety notes

WARNING: Do not start a firmware update on the camera. This software does not need a firmware update.

CAUTION: Do not use the camera property `output` with the values 6, 7, 10 or 11. A user reported that these values cause a camera error. You must remove the battery to clear this error. This software never writes these values.

CAUTION: Use one PTP session only. Do not start a second program that communicates with the camera at the same time. Examples are `gphoto2` in a terminal, Image Capture and Photos. A second program can stop all communication with the camera.

NOTE: If all communication with the camera stops, run the command `gphoto2 --reset`. If the command does not help, remove the USB cable. Then connect the cable again.

## 7. Configuration file

The program operates without a configuration file. For the virtual camera, make a file from `canon-webcam.example.toml`. The file holds the OBS WebSocket password. Do not commit the file. [Document 6](docs/user/06-configuration.md) gives the reference.

## 8. Licence

This software uses the MIT licence. Read `LICENSE`.

## 9. Contributing

Read `CONTRIBUTING.md` before you send a change. Report a security problem as `SECURITY.md` tells you.
