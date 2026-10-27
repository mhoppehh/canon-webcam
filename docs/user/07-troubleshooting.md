# Document 7 — Troubleshooting

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-007                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the messages of the program. For each message it gives the cause and the action.

## 2. How to read a message

Each failure message has two parts. The first part gives the cause. The second part gives the action. The action names a command, a setting or a directory.

Do these steps first:

Step 1. Read the complete message. Do not read only the first line.

Step 2. Find the first part of the message in Table 1.

Step 3. Do the action in the table.

## 3. Camera messages

**Table 1 — Camera messages**

| Message                                                                                  | Cause                                         | Action                                                                                       |
| ---------------------------------------------------------------------------------------- | --------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `the camera is disconnected: it is absent from the USB bus`                              | The cable is out, or the camera is off.       | Connect the cable. Set the camera to `ON`. Then start the feed again.                        |
| `the camera is wedged: it is on the USB bus but every operation fails (PTP Timeout ...)` | A rejected property write stopped the camera. | Wait for the recovery reset. If the run ends with a failure, do the action in the row below. |
| `the camera did not recover: n recovery resets did not clear the wedged camera`          | Each recovery reset failed.                   | Remove the USB cable. Then connect the cable again. Start the feed again.                    |
| `the camera answers captures but agrees on no frame size`                                | The camera sends frames of different sizes.   | Do the checks in Document 3, Section 5. Make sure that `output` is `3`.                      |
| `PTP Timeout` in the output                                                              | The camera stopped all communication.         | Run `gphoto2 --reset`. If the command does not help, remove the cable and connect it again.  |

NOTE: The program runs the command `gphoto2 --reset` a limited number of times. It does not loop.

## 4. Camera detection

Do these checks if `gphoto2 --auto-detect` shows no camera.

Step 1. Make sure that the camera is on and that the battery is charged.

Step 2. Use the data cable of the camera. Replace a cable that is not known to be good.

Step 3. Use a USB port on the computer. Do not use an unpowered hub.

Step 4. Close these programs: Image Capture, Photos, and each terminal that runs `gphoto2`.

Step 5. Run this command:

```
pgrep -l -f ptpcamerad
```

Result: If `ptpcamerad` is in the list, the system daemon holds the camera. Stop the program that started the daemon. Then connect the cable again.

Step 6. Run `gphoto2 --auto-detect` again.

## 5. OBS messages

**Table 2 — OBS messages**

| Message                                        | Cause                                                                      | Action                                                                                                                                 |
| ---------------------------------------------- | -------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `OBS is not installed in /Applications`        | The OBS bundle is in another directory.                                    | Move `OBS.app` to `/Applications`. Do the steps in Document 4, Section 2.                                                              |
| `OBS's WebSocket server is not answering`      | The server is disabled, the port is different, or OBS does not run.        | Open `Tools`, `WebSocket Server Settings`. Select `Enable WebSocket server`. Make sure that the port agrees with `websocket_endpoint`. |
| `OBS refused the WebSocket password`           | The password in the configuration file is different from the OBS password. | Copy the password from `Show Connect Info` into `websocket_password`.                                                                  |
| `No 'OBS Virtual Camera' device is registered` | The camera extension is not approved.                                      | Approve the extension one time. Do the steps in Document 4, Section 3.                                                                 |
| `the virtual camera did not start`             | OBS accepted the request, but the virtual camera does not operate.         | Read the OBS log file. The directory is `obs_log_directory`.                                                                           |

Do these checks if the messages do not give the cause.

Step 1. Make sure that OBS runs.

Step 2. Make sure that the port accepts a connection:

```
nc -z 127.0.0.1 4455 && echo "the server answers"
```

Step 3. Look for the camera extension:

```
systemextensionsctl list | grep -i obs
```

Result: One line for `com.obsproject.obs-studio.mac-camera-extension`. If the line is absent, do the steps in Document 4, Section 3.

Step 4. Read the last OBS log file in the directory `~/Library/Application Support/obs-studio/logs`.

## 6. Feed messages

**Table 3 — Feed messages**

| Message                                             | Cause                                                         | Action                                                                                |
| --------------------------------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `the live view stream ran for the planned duration` | The run ended correctly.                                      | No action.                                                                            |
| `no frame was delivered`                            | The calibration established no frame size.                    | Make sure that the camera sends frames. Run `camera-cli measure`.                     |
| `the measurement established no frame size`         | The frames have different sizes, or there are too few frames. | Increase `--window-frames` or `--window-duration-ms`.                                 |
| `serve says the camera may still be held`           | The stop did not complete in 2 seconds.                       | Wait 2 seconds. Run `control status`. If the feed runs, run `control stop` again.     |
| `the virtual camera sink failed: ...`               | OBS stopped, or the connection to OBS stopped.                | Start OBS. Make sure that the WebSocket server is enabled. Then start the feed again. |

## 7. Command and configuration messages

**Table 4 — Command and configuration messages**

| Message                                           | Cause                                                                    | Action                                                                            |
| ------------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------------------- |
| `<key> is not a configuration key`                | A spelling error in the configuration file.                              | Use the keys in Document 6, Table 1.                                              |
| `macos.<key> is not a key of the macOS mechanism` | A spelling error in the `[macos]` table.                                 | Use the keys in Document 6, Table 2.                                              |
| `upscale is on but resolution is not set`         | `upscale = true` without `resolution`.                                   | Set `resolution`, or remove `upscale`.                                            |
| `--sink virtual-camera` fails on this computer    | The platform backend is not available in this build.                     | Use `--sink stats` for a demonstration. Use a macOS build for the virtual camera. |
| `the live view feed has nowhere to go`            | The build has no platform backend, so the virtual camera cannot publish. | Use a macOS build. Use `--sink stats` for a demonstration.                        |
| `--socket <path> is required`                     | The `serve` or `control` command has no socket path.                     | Add `--socket <path>` with the same path for both commands.                       |
| `the control socket failed: ...`                  | No `serve` program listens at the path, or the path is different.        | Make sure that `serve` runs. Check the path in both commands.                     |

## 8. Diagnostic commands

Table 5 gives the commands that give information for a problem report.

**Table 5 — Diagnostic commands**

| Command                                                                 | Purpose                                                    |
| ----------------------------------------------------------------------- | ---------------------------------------------------------- |
| `camera-cli help`                                                       | Show the commands and the options.                         |
| `gphoto2 --auto-detect`                                                 | Show the camera and the USB connection.                    |
| `pkg-config --modversion libgphoto2`                                    | Show the libgphoto2 version.                               |
| `nc -z 127.0.0.1 4455`                                                  | Test the OBS WebSocket port.                               |
| `systemextensionsctl list`                                              | Show the approved system extensions.                       |
| `camera-cli stream --synthetic --duration-secs 5`                       | Test the pipeline without a camera.                        |
| `camera-cli stream --synthetic --sink virtual-camera --duration-secs 5` | Test the pipeline and the OBS connection without a camera. |

## 9. Before you write a problem report

Step 1. Do the diagnostic commands in Table 5. Copy the output.

Step 2. Copy the complete output of the program.

Step 3. Copy the last OBS log file.

Step 4. Give the camera model, the camera firmware version and the macOS version.

CAUTION: Remove the OBS password from the files that you send. The output does not contain the password. But the OBS log can contain the password.

## 10. Next step

Read [Document 8](08-device-compatibility.md) for the limits of the supported cameras and computers.
