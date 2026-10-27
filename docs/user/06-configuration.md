# Document 6 — Configuration reference

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-006                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document gives the configuration file, its keys and its defaults.

The program operates without a configuration file. Each key has a default value. A file is necessary only when you change a value. For the virtual camera, a file is the usual method, because the OBS password and the OBS path are in the file.

## 2. Use the configuration file

Step 1. Make the file. This example is `canon-webcam.toml`:

```
mechanism = "obs-virtual-camera"
pixel_format = "rgb24"
upscale = false

[macos]
obs_app_path = "/Applications/OBS.app"
websocket_endpoint = "127.0.0.1:4455"
websocket_password = "your-password-here"
launch_obs = true
```

Step 2. Send the file to the program:

```
camera-cli stream --sink virtual-camera --config ./canon-webcam.toml
```

Step 3. Protect the file. The file contains the OBS password:

```
chmod 600 canon-webcam.toml
```

NOTE: A command-line option overrides the value in the file. The file is read first.

## 3. Top-level keys

**Table 1 — Top-level keys**

| Key            | Type       | Default                | Meaning                                         |
| -------------- | ---------- | ---------------------- | ----------------------------------------------- |
| `resolution`   | Frame size | Not set                | A requested size. The camera does not honor it. |
| `fps`          | Number     | Not set                | A frame rate preference for the sink.           |
| `pixel_format` | String     | `"rgb24"`              | The normalized pixel format of the pipeline.    |
| `upscale`      | Boolean    | `false`                | Resize the frames to `resolution`.              |
| `mechanism`    | String     | `"obs-virtual-camera"` | The virtual camera mechanism.                   |
| `macos`        | Table      | Section 4              | The prerequisites of the macOS mechanism.       |

### 3.1 `resolution`

The key accepts these forms:

- `"1080p"` and `"720p"`: the common names.
- A width and a height, for example `"1024x576"`.

CAUTION: `resolution` is a preference, not a command to the camera. The camera does not honor a size request. The report always gives the measured frame size. Do not use this key to increase the resolution. Document 8 gives the hardware limit.

### 3.2 `fps`

The key accepts a positive number, for example `24` or `30`. The value is a preference for the sink. The measured frame rate in the report is the true rate.

### 3.3 `pixel_format`

The pipeline makes one format. The only allowed value is `"rgb24"`.

### 3.4 `upscale`

The value `true` resizes each frame to `resolution`. The value `false` publishes the frames at their measured frame size.

The resized frames have more pixels, but they have no more detail. For this reason, the program calls this operation resizing.

If `upscale` is `true`, `resolution` must be set. A missing `resolution` is an error.

### 3.5 `mechanism`

On macOS, the only allowed value is `"obs-virtual-camera"`. A different value is an error. The program does not fall back to another mechanism.

## 4. The `[macos]` table

The table carries the values that the OBS checks read. Table 2 gives the keys.

**Table 2 — Keys of the `[macos]` table**

| Key                  | Type           | Default                                         | Meaning                                   |
| -------------------- | -------------- | ----------------------------------------------- | ----------------------------------------- |
| `obs_app_path`       | Path           | `/Applications/OBS.app`                         | The OBS application bundle.               |
| `websocket_endpoint` | Socket address | `127.0.0.1:4455`                                | The address of the OBS WebSocket server.  |
| `websocket_password` | String         | Not set                                         | The password of the OBS WebSocket server. |
| `obs_log_directory`  | Path           | `~/Library/Application Support/obs-studio/logs` | The directory of the OBS log files.       |
| `launch_obs`         | Boolean        | `true`                                          | Start OBS if it does not run.             |

### 4.1 `obs_app_path`

Use the default value. OBS cannot install its virtual camera from another directory.

### 4.2 `websocket_endpoint`

Write the address and the port. The port must agree with the OBS setting `Tools`, `WebSocket Server Settings`.

### 4.3 `websocket_password`

Write the password of the OBS WebSocket server. Select `Show Connect Info` in OBS to see the password.

If the OBS server has no authentication, omit this key.

### 4.4 `obs_log_directory`

This directory holds the OBS log files. The program reads the log to find the reason when the virtual camera does not start.

The default value is correct for almost all users. If you set this key, write an absolute path. The program does not expand the character `~`.

### 4.5 `launch_obs`

The value `true` lets the program start OBS when OBS does not run. The program waits 20 seconds for OBS to answer. The value `false` makes a stopped OBS an error.

The program never quits OBS.

## 5. Validation

The program checks the file before it opens the camera. Each error names the key and the expected value.

**Table 3 — Configuration errors**

| Error                                             | Cause                                          | Action                                      |
| ------------------------------------------------- | ---------------------------------------------- | ------------------------------------------- |
| `the configuration file is not valid TOML`        | A syntax error in the file.                    | Correct the syntax.                         |
| `<key> is not a configuration key`                | An unknown top-level key, or a spelling error. | Use the keys in Table 1.                    |
| `macos.<key> is not a key of the macOS mechanism` | An unknown key in the `[macos]` table.         | Use the keys in Table 2.                    |
| `<key> is <value>, which is not ...`              | A value of the wrong type or shape.            | Use the type in Table 1 or Table 2.         |
| `upscale is on but resolution is not set`         | `upscale = true` without `resolution`.         | Set `resolution`, or set `upscale = false`. |
| `the configuration file <path> could not be read` | A wrong path, or no permission.                | Correct the path or the permissions.        |

## 6. Next step

Continue with [Document 7](07-troubleshooting.md) if a message occurs.
