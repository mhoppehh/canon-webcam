# Document 8 — Device compatibility

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-008                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the compatibility of the software with the cameras and with the operating systems.

Read this document before you buy hardware or before you install the software on a second computer.

## 2. Operating systems

**Table 1 — Operating systems**

| Operating system          | Status               | Note                                                                                           |
| ------------------------- | -------------------- | ---------------------------------------------------------------------------------------------- |
| macOS 26.6.2              | Supported and tested | The virtual camera uses the OBS camera extension.                                              |
| macOS 15 and later        | Supported            | The camera extension is the OBS mechanism.                                                     |
| macOS 12.3 until macOS 15 | Not tested           | OBS can use an older camera mechanism on these versions. The software does not test that path. |
| Windows                   | Not available        | The Windows backend is not made.                                                               |
| Linux                     | Not available        | The Linux backend is not made.                                                                 |

The program is built for the operating system of the build computer. A macOS build cannot publish a virtual camera on Windows or Linux.

## 3. Cameras

**Table 2 — Camera compatibility**

| Camera                                         | Status               | Note                                                                                                          |
| ---------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------- |
| Canon EOS M50, firmware 3-1.1.0                | Supported and tested | The measured frame size is 1024 pixels by 576 pixels at about 24 frames per second.                           |
| Canon EOS M50, other firmware                  | Not tested           | The firmware can change the camera properties. Test the command `measure` first.                              |
| Other Canon EOS models with live view over PTP | Not tested           | The software can operate. The camera property `output` can have other values. Test with `camera-cli measure`. |
| Canon EOS models without live view over PTP    | Not supported        | The camera supplies no frames.                                                                                |
| Other camera brands                            | Not supported        | The software uses Canon camera properties.                                                                    |

WARNING: Do not use the software with a camera model that is not tested, if the camera is necessary for your work. A rejected property write can stop all communication with a Canon camera. A recovery reset or a cable reconnect clears the condition. Document 7 gives the procedure.

## 4. Frame size and frame rate

**Table 3 — Measured values**

| Item                        | Value                          |
| --------------------------- | ------------------------------ |
| Maximum frame size over USB | 1024 pixels by 576 pixels      |
| Frame rate                  | About 24 frames per second     |
| Data rate                   | About 6.5 megabytes per second |
| 1080p over USB              | Not possible                   |

The USB connection sets the limit. The camera compresses the live view with Motion JPEG. The limit is the resolution of the live view stream, not the bandwidth of the cable.

To use 1080p, use the HDMI output of the camera with a video capture device. This software does not read the HDMI output.

## 5. Computer requirements

**Table 4 — Computer compatibility**

| Item            | Requirement                                             |
| --------------- | ------------------------------------------------------- |
| Processor       | Apple silicon or Intel                                  |
| USB port        | USB 2.0 High Speed or better                            |
| Memory          | 1 gigabyte more than macOS and OBS use                  |
| Free disk space | 2 gigabytes for the build, 20 megabytes for the program |

A USB 3 port does not increase the frame size or the frame rate. A USB 3 port operates at the USB 2.0 speed of the camera.

## 6. Programs that can select the camera

The virtual camera has the name `OBS Virtual Camera`. Each macOS program that uses AVFoundation can select it.

**Table 5 — Tested programs**

| Program         | Status                                     |
| --------------- | ------------------------------------------ |
| Discord         | Not tested with this software              |
| Microsoft Teams | Not tested with this software              |
| Google Meet     | Not tested with this software              |
| OBS itself      | Tested as the source of the virtual camera |

Document 4, Section 6 gives the selection procedure.

## 7. What is not supported

The software does not do these operations:

- It does not record video or audio.
- It does not control the camera focus, the exposure or the zoom.
- It does not update the camera firmware.
- It does not write data to the memory card.
- It does not supply the HDMI signal.
- It does not make a virtual camera on Windows or Linux in this release.
- It does not use the camera property `liveviewsize`. This property has no effect on the M50.

## 8. Open questions in this release

These items are not verified. Table 6 gives each item and the test that is necessary.

**Table 6 — Items not verified**

| Item                                          | Necessary test                                                                                                              |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| A consumer program lists and shows the camera | Install OBS in `/Applications`, approve the extension, then start the feed and select the camera in Discord, Teams or Meet. |
| Ten minutes of stable live view               | `camera-cli stream --sink virtual-camera --duration-secs 600` with a camera attached.                                       |
| CPU and memory use for 30 minutes             | `camera-cli stream --duration-secs 1800` and `ps -o %cpu,rss -p <pid>` each minute.                                         |
| A cable reconnect during a run                | Remove the cable during a run. Connect the cable again. Make sure that the feed continues.                                  |
| A second Canon camera model                   | Run `camera-cli measure` with the model attached.                                                                           |
| Windows and Linux                             | Build and test on a Windows computer and on a Linux computer.                                                               |

NOTE: This document records the limits. `docs/internal/plan/test-results.md` records the measurements. Read both documents before a problem report.
