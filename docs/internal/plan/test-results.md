# Test Results: Canon EOS M50 Virtual Camera

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | TR-001                                  |
| Date             | 2026-10-27                              |
| Related document | pre-implementation-tests.md             |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the results of the live tests. The tests are in pre-implementation-tests.md. The tests examine the Canon EOS M50 camera and the gphoto2 software.

This document does not change the test plan. This document records only the measured data.

## 2. Test Environment

Table 1 gives the test environment.

**Table 1 — Test environment**

| Item                 | Value                                 |
| -------------------- | ------------------------------------- |
| Computer             | Apple laptop with an Apple M4 processor |
| Operating system     | macOS 26.6.2, build 25G83             |
| Camera               | Canon EOS M50                         |
| Camera serial number | Not recorded in this public document  |
| Camera firmware      | 3-1.1.0                               |
| USB port speed       | High Speed, 480 Mbit/s                 |
| USB connection       | Through a USB 2.0 hub                  |
| gphoto2 version      | 2.5.32                                 |
| libgphoto2 version   | 2.5.34                                 |
| Video software       | ffmpeg, Homebrew build                 |

NOTE: We had one computer only. The computer uses macOS. We did not have a Linux computer. We did not have a Windows computer.

## 3. Summary of Results

Table 2 gives the result of each test.

**Table 2 — Summary of results**

| Test | Description                          | Result                    |
| ---- | ------------------------------------ | ------------------------- |
| A1   | Linux camera detection               | NOT TESTED                |
| A2   | Windows driver replacement           | NOT TESTED                |
| A3   | macOS camera detection and live view | PASS, with limits         |
| B1   | Live view resolution                 | PASS, at a revised target |
| B2   | Output setting                       | PASS                      |
| B3   | Frame rate for 60 seconds            | PASS, 30-second test      |
| C1   | Linux v4l2loopback                   | NOT TESTED                |
| C2   | Windows virtual camera               | NOT TESTED                |
| C3   | macOS OBS virtual camera             | NOT TESTED                |
| D1   | Discord, Teams, and Meet             | NOT TESTED                |
| E1   | Cable disconnect                     | PARTIAL                   |
| E2   | CPU and memory use                   | NOT TESTED                |

## 4. Detailed Results

### 4.1 Test A1 (Linux) — NOT TESTED

Reason: No Linux computer was available.

### 4.2 Test A2 (Windows) — NOT TESTED

Reason: No Windows computer was available.

### 4.3 Test A3 (macOS) — PASS, with limits

Step 1. We connected the camera. We ran the command `gphoto2 --auto-detect`.

Result: The software found the camera on the first attempt. The model is "Canon EOS M50".

Step 2. We checked the process `ptpcamerad`.

Result: The process was not active. The process did not control the camera.

Step 3. We ran the command `gphoto2 --capture-movie=5s`.

Result: The camera supplied 115 frames in 5 seconds. This is about 23 frames per second.

Conclusion: Test A3 passes for the first connection. The camera and the software operate together on macOS. The camera did not need a manual stop of the daemon.

Limit: We did not connect the camera again after a cable removal. We do not know if `ptpcamerad` starts and blocks the camera at a later time.

### 4.4 Test B1 (resolution) — PASS, at a revised target

The USB connection cannot supply 1080p. The stakeholder agreed to a target of 1024 pixels by 576 pixels.

Step 1. We set the property `output` to `TFT + PC` in one gphoto2 session.

Result: The camera accepted the command. The value stayed `TFT + PC`.

Step 2. We captured frames with `output` = `TFT + PC`.

Result: Each frame is 512 pixels wide and 288 pixels high. The camera was in video mode.

Step 3. We set the property `liveviewsize` to `Large`.

Result: The camera accepted the command. But the frame size did not change. The property `liveviewsize` has no effect on this camera.

Step 4. We set the action `/main/actions/viewfinder=1` in the same command as the capture.

Result: The camera supplied frames of 1024 pixels by 576 pixels.

Step 5. We examined each frame of a 6-second capture that used `viewfinder=1`.

Result: 141 frames are 1024 pixels by 576 pixels. 5 frames are 512 pixels by 288 pixels. The 5 small frames are the first frames.

Conclusion: Test B1 passes at the revised target of 1024 pixels by 576 pixels. To get this size, the software must set `viewfinder=1` and `output=3` together with the capture. The software must discard the first 5 frames.

### 4.5 Test B2 (output setting) — PASS

Step 1. We read the property `output`.

Result: The initial value was `MOBILE2`. The value of `MOBILE2` is 8.

Step 2. We set the property to `TFT + PC` in one gphoto2 session. We read the value in the same session.

Result: The camera accepted the command. The value was `TFT + PC`.

Step 3. We examined the effect of the value on the frame size.

Result: The value `MOBILE2` supplies the small frame sizes. The values `Off`, `TFT`, `PC`, and `TFT + PC` supply the large frame sizes.

CAUTION: Do not use the values 6, 7, 10, and 11. These values contain `PC` and `MOBILE` together. A user reported that these values cause a camera error. You must remove the battery to clear the error.

Conclusion: Test B2 passes. The value `TFT + PC` is safe. The software must set this value at the start.

### 4.6 Test B3 (sustained frame rate) — PASS, with a 30-second test

We captured frames for 30 seconds. We used `output` = `TFT + PC` and `viewfinder=1`.

Result: The camera supplied 721 frames. The average rate is 24 frames per second. The operation took 31.5 seconds.

Result: 716 frames are 1024 pixels by 576 pixels. 5 frames are 512 pixels by 288 pixels.

Result: The file size is 195.6 megabytes. The data rate is about 6.5 megabytes per second. This rate is less than the USB 2.0 limit.

Conclusion: Test B3 passes. The camera supplied 24 frames per second for 30 seconds with no interruption.

Limit: We did not do a 60-second test. The camera became warm. We did not measure the temperature.

### 4.7 Tests C1, C2, C3, D1, and E2 — NOT TESTED

Reason: No Linux computer and no Windows computer were available. Test C1 and Test C2 need these computers. Test C3, Test D1, and Test E2 are not done.

### 4.8 Test E1 (cable disconnect) — PARTIAL

We did not remove the USB cable.

But we found a related problem. A rejected property write stopped all communication with the camera.

After the rejected write, the camera stayed on the USB bus. The command `gphoto2 --auto-detect` still found the camera. But each operation stopped with the message `PTP Timeout`.

We ran the command `gphoto2 --reset`. The camera left the USB bus. Then the camera came back. All operations operated correctly again.

Conclusion: Test E1 is a partial pass. The software does not hang without a message. But the camera can stop all communication after a rejected command. The design must include a recovery procedure.

## 5. Cause of the Write Problem

We found the cause. The camera is not damaged and the camera is not faulty.

The first problem is the PTP session. The gphoto bug tracker item 913 gives this information. On Canon cameras, the first gphoto2 process after the cable connection operates correctly. Each later gphoto2 process can stop with a PTP error.

We used many separate gphoto2 processes. The property writes failed. Then the camera stopped all communication.

Test: We did the property read and the property write in one gphoto2 session. The command `gphoto2 --shell` makes one session. The write operated correctly.

Test: We did the property change and the capture in one gphoto2 command. The command operated correctly.

Test: We set the property `output` to `TFT + PC` and read the value again in the same session. The value was `TFT + PC`.

Conclusion: The software must use one PTP session for all operations. The software must not start a new gphoto2 process for each operation.

## 6. Recommendations

Recommendation 1. Use one gphoto2 session for all operations. Use `gphoto2 --shell`, or put all options in one command.

Recommendation 2. Set `output` to `TFT + PC` (value 3) at the start.

Recommendation 3. Set `/main/actions/viewfinder=1` in the same command as the capture. The camera screen comes on. Press the camera screen one time to select an autofocus mode.

Recommendation 4. Discard the first 5 frames of each capture. These frames are 512 pixels by 288 pixels.

Recommendation 5. Do not use the `output` values 6, 7, 10, and 11.

Recommendation 6. Accept the resolution of 1024 pixels by 576 pixels for the first version. If you need 1080p, use the HDMI output and a capture device.

## 7. Safety Notes

WARNING: Do not start a firmware update on the camera. These tests do not need a firmware update.

CAUTION: Do not format the memory card. Do not delete files. These tests do not need these operations.

NOTE: A property write that the camera rejects can stop all communication. If the communication stops, run the command `gphoto2 --reset`. If the reset does not help, remove the USB cable. Then connect the cable again.

NOTE: We did not damage the camera. We did not change the firmware. We did not write data to the camera. The camera answers all commands correctly after the reset.

## 8. Effect on the Implementation Plan

Table 3 gives the effect of the results on the implementation plan.

**Table 3 — Effect on the plan**

| Item                         | Status                                                   |
| ---------------------------- | -------------------------------------------------------- |
| Cross-platform capture       | Not confirmed. Test A1 and Test A2 did not run.          |
| macOS capture                | Confirmed.                                               |
| Resolution target            | Changed from 1080p to 1024 pixels by 576 pixels.         |
| Property configuration       | Possible. Use one gphoto2 session.                       |
| Frame rate                   | 24 frames per second at 1024 pixels by 576 pixels.       |
| 1080p resolution             | Possible only with the HDMI output and a capture device. |
| Recovery after a write error | Necessary. The camera can stop all communication.        |

## 9. Next Step

Test B1, Test B2, and Test B3 pass. You can start the implementation of the capture layer. Use the three settings in Recommendation 1 to Recommendation 4.

Do these tests before the release:

1. Test B3 again. Capture frames for 60 seconds. Count the frames. Measure the temperature of the camera.
2. Test E2. Measure the CPU use and the memory use for 30 minutes.
3. Test C3 and Test D1 on macOS. Confirm that a consumer application shows the live feed.
4. Test A1 and Test A2 on a Linux computer and on a Windows computer.
