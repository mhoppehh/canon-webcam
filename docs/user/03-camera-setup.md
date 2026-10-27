# Document 3 — Camera setup

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-003                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document tells you how to connect the camera and how to prepare the camera for live view.

The software sets the necessary camera properties. You do not change them in the camera menu. But you must know the limits in this document.

## 2. Connect the camera

Step 1. Make sure that the battery is charged.

Step 2. Set the camera power switch to `ON`.

Step 3. Connect the USB data cable between the camera and the computer.

Step 4. Use a USB port on the computer. Do not use an unpowered hub.

Step 5. Make sure that the camera is in the correct mode. Use the video mode or the photo mode. The measured results use the video mode.

Step 6. Make sure that no other program uses the camera. Close these programs:

- Image Capture
- Photos
- Any terminal that runs `gphoto2`

CAUTION: Use one PTP session only. If a second program opens the camera, the camera can stop all communication. Then the message `PTP Timeout` occurs.

Step 7. Make sure that macOS finds the camera:

```
gphoto2 --auto-detect
```

Expected result: One line with `Canon EOS M50`, or the model name of your Canon camera.

## 3. What the software sets in the camera

The software opens one PTP session. In that session it applies these settings:

**Table 1 — Settings that the software applies**

| Setting                                  | Value            | Effect                                        |
| ---------------------------------------- | ---------------- | --------------------------------------------- |
| Camera property `output`                 | `3` (`TFT + PC`) | Selects the large frame size.                 |
| Camera action `/main/actions/viewfinder` | `1`              | Starts live view. The camera screen comes on. |

The software applies these settings on each start. It does not use the camera property `liveviewsize`. This property has no effect on this camera.

## 4. The camera screen and autofocus

When the software starts live view, the camera screen comes on.

Step 1. Look at the camera screen.

Step 2. If the camera shows an autofocus mode selection, touch the screen one time. Then select an autofocus mode.

NOTE: The live view operates without this step. But the camera can show the selection continuously.

## 5. Frame size

**Table 2 — Frame sizes**

| Condition                                 | Frame size                |
| ----------------------------------------- | ------------------------- |
| Live view active, `output` = 3            | 1024 pixels by 576 pixels |
| The first 5 frames after live view starts | 512 pixels by 288 pixels  |
| `output` = 8 (`MOBILE2`)                  | 512 pixels by 288 pixels  |

The software discards the first frames. It calibrates the expected frame size from the frame data. It then publishes only frames of the expected frame size.

Do not set a frame size in the camera menu. The camera does not honor a size request. The measured frame size in the report is the true frame size.

## 6. Frame rate

The measured frame rate is about 24 frames per second at 1024 pixels by 576 pixels. The data rate is about 6.5 megabytes per second.

This frame rate is less than the USB 2.0 limit. A USB 3 port does not increase the frame size or the frame rate.

## 7. The camera property `output`

Table 3 gives the values of the camera property `output`.

**Table 3 — Values of the property `output`**

| Value        | Camera label           | Status                                                         |
| ------------ | ---------------------- | -------------------------------------------------------------- |
| 0            | `Off`                  | Allowed. Large frame size. Not individually tested.            |
| 1            | `TFT`                  | Allowed. Large frame size. Not individually tested.            |
| 2            | `PC`                   | Allowed. Large frame size. Not individually tested.            |
| 3            | `TFT + PC`             | Allowed and verified. Use this value.                          |
| 8            | `MOBILE2`              | Refused. This value gives the small frame size.                |
| 6, 7, 10, 11 | PC and MOBILE combined | Refused. A user report says these values cause a camera error. |

The program refuses each value that is not in the allowed list. It refuses the value before it opens the camera.

## 8. Disconnect the camera

Step 1. Stop the software. Use `control stop` or `control shutdown` if you use the control socket. If you use `stream`, wait until the run ends, or stop the run.

Step 2. Remove the USB cable.

NOTE: If you remove the cable during a run, the software detects a disconnected camera. It closes the sink and stops the run. Document 7, Section 5 gives the messages and the actions.

## 9. Next step

Continue with [Document 4](04-obs-setup.md).
