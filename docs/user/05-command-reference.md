# Document 5 — Command reference

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-005                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the commands, the options, the examples and the exit codes.

## 2. Command summary

```
camera-cli <command> [options]
```

**Table 1 — Commands**

| Command   | Purpose                                                                                                               |
| --------- | --------------------------------------------------------------------------------------------------------------------- |
| `measure` | Capture live view and report the measured frame size and the measured frame rate. No sink.                            |
| `stream`  | Run the live view pipeline. Publish the frames to the sink. Stop at the end of the duration or when the camera stops. |
| `serve`   | Run the same pipeline behind a local socket. The feed stops and starts without a restart of the program.              |
| `control` | Send one command to a running `serve`.                                                                                |
| `help`    | Show the help text.                                                                                                   |

## 3. `measure`

This command opens one PTP session and captures frames. It measures the frame size and the frame rate. It does not publish frames.

```
camera-cli measure [options]
```

**Table 2 — Options for `measure`**

| Option                      | Default | Meaning                                                   |
| --------------------------- | ------- | --------------------------------------------------------- |
| `--output <raw>`            | `3`     | The camera property `output`. Allowed values: 0, 1, 2, 3. |
| `--window-frames <n>`       | `72`    | The maximum number of frames in the calibration window.   |
| `--window-duration-ms <ms>` | `3000`  | The maximum time of the calibration window.               |
| `--min-recurrences <n>`     | `8`     | How often a frame size must occur in the window.          |
| `--sampling-frames <n>`     | `48`    | The number of frames used to measure the rate.            |

Example:

```
camera-cli measure
```

Expected output contains these lines:

```
measured frame size: 1024x576
measured frame rate: 24.00 fps
```

## 4. `stream`

This command runs the live view pipeline until the duration elapses or the camera stops producing frames.

```
camera-cli stream [options]
```

**Table 3 — Options for `stream`**

| Option                        | Default              | Meaning                                                                                                                               |
| ----------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `--config <path>`             | No file              | The configuration file. Document 6 gives the keys.                                                                                    |
| `--sink <name>`               | `stats`              | `stats` counts the frames and publishes nothing. `virtual-camera` publishes the frames through OBS, so other programs see the camera. |
| `--mechanism <name>`          | `obs-virtual-camera` | The virtual camera mechanism. On macOS this is the only allowed value.                                                                |
| `--resolution <size>`         | Not set              | A requested size, for example `1080p` or `1024x576`. This is a preference. The camera does not honor it.                              |
| `--fps <n>`                   | Not set              | A frame rate preference for the sink. The camera does not honor it.                                                                   |
| `--pixel-format <format>`     | `rgb24`              | The normalized pixel format of the pipeline.                                                                                          |
| `--upscale`                   | Off                  | Resize the frames to `--resolution`. This adds no detail.                                                                             |
| `--no-upscale`                | On                   | Do not resize the frames.                                                                                                             |
| `--duration-secs <n>`         | Not set              | Stop after this number of seconds.                                                                                                    |
| `--queue-capacity <n>`        | `4`                  | The number of frames in the bounded capture queue.                                                                                    |
| `--synthetic`                 | Off                  | Use a synthetic frame source. This needs no camera.                                                                                   |
| `--synthetic-fps <n>`         | `24`                 | The frame rate of the synthetic source.                                                                                               |
| `--synthetic-wedge-after <n>` | Not set              | Make the synthetic source stop with `PTP Timeout` after n frames. This shows the recovery procedure.                                  |
| `--synthetic-reset-fails`     | Off                  | Make each recovery reset fail. This shows the exit path of a permanently wedged camera.                                               |
| `--output <raw>`              | `3`                  | The camera property `output`. Document 3 gives the values.                                                                            |
| `--window-frames <n>`         | `72`                 | The maximum number of frames in the calibration window.                                                                               |
| `--window-duration-ms <ms>`   | `3000`               | The maximum time of the calibration window.                                                                                           |
| `--min-recurrences <n>`       | `8`                  | How often a frame size must occur in the window.                                                                                      |

CAUTION: `--sink stats` publishes no frames. With this value, no program sees the camera. Use `--sink virtual-camera` to make the camera available.

Example — publish the camera for a call:

```
camera-cli stream --sink virtual-camera --config ./canon-webcam.toml
```

Example — measure the pipeline with no camera:

```
camera-cli stream --synthetic --duration-secs 10
```

Example — show the recovery procedure with a synthetic camera:

```
camera-cli stream --synthetic --synthetic-wedge-after 40 --duration-secs 10
```

The report contains these lines:

```
measured frame size: 1024x576
measured frame rate: 24.00 fps
sink: obs-virtual-camera
sink consumer page connected: true
sink frames published: 240
sink frames dropped: 0
```

## 5. `serve` and `control`

The `serve` command runs the feed behind a local socket. The `control` command sends one command to the socket.

```
camera-cli serve --socket <path> [stream options]
camera-cli control --socket <path> <command>
```

**Table 4 — Options**

| Option            | Meaning                                                                                     |
| ----------------- | ------------------------------------------------------------------------------------------- |
| `--socket <path>` | The path of the local socket. Required. `serve` makes the socket. `control` connects to it. |

`serve` accepts each option of `stream`. The options apply to each start.

**Table 5 — Control commands**

| Command    | Answers           | Meaning                                                        |
| ---------- | ----------------- | -------------------------------------------------------------- |
| `start`    | `started`         | Take the PTP session and open the sink.                        |
| `start`    | `already-running` | The feed was already running. Nothing changed.                 |
| `stop`     | `stopped`         | Release the PTP session and close the sink.                    |
| `stop`     | `already-stopped` | The feed was already stopped. Nothing changed.                 |
| `status`   | `running`         | The feed runs.                                                 |
| `status`   | `stopped`         | The feed is stopped.                                           |
| `status`   | `error: <reason>` | The last run ended with a failure. The reason follows.         |
| `shutdown` | `shutting-down`   | Stop the feed and close the socket. The `serve` program exits. |

**Table 6 — Exit codes of `control`**

| Code | Meaning                                             |
| ---- | --------------------------------------------------- |
| 0    | The command succeeded. For `status`: the feed runs. |
| 1    | The command failed.                                 |
| 3    | For `status` only: the feed is stopped.             |

Example — start the feed, then start the call:

```
camera-cli serve --socket /tmp/canon-webcam.sock --sink virtual-camera &
camera-cli control --socket /tmp/canon-webcam.sock start
```

Example — stop and start the feed during a call:

```
camera-cli control --socket /tmp/canon-webcam.sock stop
camera-cli control --socket /tmp/canon-webcam.sock start
```

Example — use the state in a script:

```
if camera-cli control --socket /tmp/canon-webcam.sock status > /dev/null; then
  echo "the feed runs"
else
  echo "the feed is stopped"
fi
```

Example — stop the program:

```
camera-cli control --socket /tmp/canon-webcam.sock shutdown
```

NOTE: `serve` starts stopped. A `start` is necessary after each program start. A stop releases the camera, so another program can use the camera between runs.

NOTE: When the socket is ready, `serve` prints one line: `control socket: <path>`. Send the `control` commands only after this line.

NOTE: A stop is bounded. If the camera does not release the session within 2 seconds, `serve` reports this. The camera can then still be held. Document 7, Section 6 gives the action.

NOTE: `--duration-secs` with `serve` limits each start. It does not limit the program.

## 6. Exit codes of `stream` and `measure`

**Table 7 — Exit codes**

| Code | Meaning                                                     |
| ---- | ----------------------------------------------------------- |
| 0    | The run was successful.                                     |
| 1    | The run failed. The message gives the cause and the action. |

The program shows the cause after the run. Examples of causes are a wedged camera, a disconnected camera, a missing OBS setting and a calibration that established no frame size.

## 7. Typical procedures

### 7.1 Procedure — camera for a call

Step 1. Connect the camera. Do the steps in Document 3.

Step 2. Start OBS. Do the steps in Document 4.

Step 3. Start the feed:

```
camera-cli serve --socket /tmp/canon-webcam.sock --sink virtual-camera &
camera-cli control --socket /tmp/canon-webcam.sock start
```

Step 4. In your program, select the camera `OBS Virtual Camera`.

Step 5. After the call, stop the program:

```
camera-cli control --socket /tmp/canon-webcam.sock shutdown
```

### 7.2 Procedure — check the installation

Step 1. Run a synthetic stream:

```
camera-cli stream --synthetic --duration-secs 5
```

Step 2. Run a measurement with the camera:

```
camera-cli measure
```

Step 3. Compare the measured frame size with the value in Document 3, Section 5.

### 7.3 Procedure — report a problem

Step 1. Run the program and copy the complete output.

Step 2. Run this command and copy the output:

```
gphoto2 --auto-detect
```

Step 3. Read Document 7. The message in your output names the cause.

## 8. Next step

Continue with [Document 6](06-configuration.md) to make a configuration file.
