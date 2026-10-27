# Document 1 — Requirements

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-001                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the requirements for the computer, the software, the camera and OBS.

Read all sections before you start the installation.

## 2. Computer requirements

**Table 1 — Computer requirements**

| Item             | Requirement                                                                              |
| ---------------- | ---------------------------------------------------------------------------------------- |
| Operating system | macOS. The software is tested on macOS 26.6.2.                                           |
| Processor        | Apple silicon or Intel. The software is tested on an Apple M4 processor.                 |
| USB port         | One free USB port. USB 2.0 High Speed is sufficient.                                     |
| Disk space       | 2 gigabytes for the Rust build files. The installed program uses less than 20 megabytes. |
| Network          | Internet access for the first build. The build downloads Rust libraries.                 |

The program uses the loopback network interface. It opens one local port for the live view transport. It does not send data to the network.

## 3. Software requirements for the build

You need these programs to build the software. Document 2 gives the commands.

**Table 2 — Build requirements**

| Program        | Minimum                           | Purpose                                                                |
| -------------- | --------------------------------- | ---------------------------------------------------------------------- |
| Rust and Cargo | 1.98                              | Builds the software.                                                   |
| libgphoto2     | 2.5.34 is tested                  | Controls the camera. The C library and its header files are necessary. |
| pkg-config     | Any current version               | Finds libgphoto2 for the build.                                        |
| Clang          | From the Xcode command line tools | The libgphoto2 binding is generated during the build.                  |

To install the Xcode command line tools, run this command:

```
xcode-select --install
```

To install libgphoto2 and pkg-config, use Homebrew:

```
brew install libgphoto2 pkg-config
```

To install Rust, use the official installer:

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

NOTE: The build stops with a message if pkg-config cannot find libgphoto2. The message names pkg-config and libgphoto2.

## 4. Software requirements at run time

**Table 3 — Run-time requirements**

| Program         | Purpose                                                                  |
| --------------- | ------------------------------------------------------------------------ |
| libgphoto2      | Controls the camera. The program loads this library each time it starts. |
| gphoto2 command | Runs the recovery reset `gphoto2 --reset` after a wedged camera.         |
| OBS Studio      | Makes the virtual camera. OBS must be in the directory `/Applications`.  |

The program does not use the `gphoto2` command for normal operation. It uses the command only for the recovery reset. If the command is absent, the program still runs. But the program cannot recover a wedged camera.

## 5. OBS requirements

**Table 4 — OBS requirements**

| Item             | Requirement                                                                                  |
| ---------------- | -------------------------------------------------------------------------------------------- |
| Version          | OBS Studio 32.2.1 is tested. The WebSocket plugin 5.7.4 is tested.                           |
| Location         | `/Applications/OBS.app`. OBS refuses to install its virtual camera from any other directory. |
| WebSocket server | Enabled, with authentication and a password.                                                 |
| WebSocket port   | 4455 is the default. You can use a different port.                                           |
| Camera extension | Approved one time in the macOS System Settings.                                              |

WARNING: Do not install OBS from a package manager or from a game platform. An OBS copy outside `/Applications` cannot install its virtual camera. Document 4 gives the procedure.

## 6. Camera requirements

**Table 5 — Camera requirements**

| Item        | Requirement                                                            |
| ----------- | ---------------------------------------------------------------------- |
| Model       | Canon EOS M50. This is the only tested model.                          |
| Firmware    | Version 3-1.1.0 is tested.                                             |
| Battery     | A charged battery. The camera does not supply live view without power. |
| Memory card | Not necessary. The software does not write data to the card.           |
| USB cable   | The data cable of the camera. A charge-only cable does not operate.    |

Document 8 gives the compatibility limits for other Canon models.

## 7. Permissions

Table 6 gives the permissions that the software and OBS use.

**Table 6 — Permissions**

| Permission                | Owner           | When                                                                |
| ------------------------- | --------------- | ------------------------------------------------------------------- |
| Camera extension approval | You, one time   | After the OBS installation.                                         |
| USB device access         | No confirmation | The camera has no protected device class on macOS.                  |
| Local socket              | No confirmation | The program creates one socket file in a directory that you select. |

The program does not request administrator rights. It does not install a system extension. It does not change a system setting.

## 8. Next step

Continue with [Document 2](02-build-and-install.md).
