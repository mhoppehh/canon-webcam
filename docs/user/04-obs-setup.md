# Document 4 — OBS setup

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-004                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document tells you how to install OBS, how to enable the OBS WebSocket server and how to approve the OBS camera extension.

The software uses OBS as the **virtual camera**. Without OBS, no other program can select the camera.

Do the steps in the sequence given.

## 2. Install OBS

Step 1. Go to the OBS web site and download the macOS installer.

Step 2. Move `OBS.app` to the directory `/Applications`.

WARNING: Do not use an OBS copy from a game platform or from a package manager. Do not move `OBS.app` to a different directory. OBS refuses to install its virtual camera from any other directory. The software stops with the message `OBS is not installed in /Applications`.

Step 3. Start OBS one time. Then quit OBS. The first start makes the camera extension.

Step 4. Make sure that OBS is in the correct directory:

```
ls -d /Applications/OBS.app
```

Expected result: The command shows `/Applications/OBS.app`.

## 3. Approve the OBS camera extension

OBS makes the virtual camera with a macOS camera extension. macOS needs your approval one time. Only an administrator can approve it. The software cannot do this step.

Step 1. Start OBS.

Step 2. Start the virtual camera in OBS. Use the button `Start Virtual Camera`.

Step 3. Look for a macOS message about the camera extension. Select `Open System Settings`.

Step 4. In `System Settings`, open `Privacy & Security`. Look for the OBS extension notice. Select `Allow`.

Step 5. If the system asks for the administrator password, enter it.

Step 6. Quit OBS. Then start OBS again.

Step 7. Start the virtual camera again. Make sure that OBS shows no error.

NOTE: The approval occurs one time only. After the approval, each start of the virtual camera operates without a message.

## 4. Enable the OBS WebSocket server

Step 1. Start OBS.

Step 2. Open `Tools` and then `WebSocket Server Settings`.

Step 3. Select `Enable WebSocket server`.

Step 4. Write the port number. The default port is `4455`. Keep this value unless another program uses the port.

Step 5. Select `Enable Authentication`.

Step 6. Select `Show Connect Info`. Note the password. You need the password for the configuration file.

Step 7. Select `Apply` and then `OK`.

Step 8. Make sure that the server answers. The port must accept a connection:

```
nc -z 127.0.0.1 4455 && echo "the server answers"
```

Expected result: The message `the server answers`.

CAUTION: Write the same password in the configuration file. A different password stops the software with the message `OBS refused the WebSocket password`. The password is in the `[macos]` table of the configuration file. Document 6 gives the keys.

## 5. What the software does in OBS

The software controls OBS with the WebSocket server. It does these steps:

**Table 1 — OBS operations**

| When  | Operation                                                                                                                                                                                                                                                                                                                             |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Start | Read the current program scene. Make the scene `canon-webcam` if it is absent. Make the input `canon-webcam source` if it is absent. This input is a browser source. Point the browser source to the local live view transport. Make the scene the program scene. Start the virtual camera. Confirm that the virtual camera operates. |
| Stop  | Stop the virtual camera. Remove the input. Remove the scene. Make the previous program scene the program scene again.                                                                                                                                                                                                                 |

The software does not quit OBS. OBS stays open after the run. Each later start uses the same OBS instance.

NOTE: The software confirms the virtual camera with the OBS status request. The request `StartVirtualCam` can report success when the virtual camera does not start. The software does not trust that report.

## 6. Select the OBS virtual camera in your program

Do these steps after the software publishes frames. The name of the camera in other programs is `OBS Virtual Camera`.

**Table 2 — Example programs**

| Program         | Procedure                                                           |
| --------------- | ------------------------------------------------------------------- |
| Discord         | `Settings`, `Voice & Video`, `Camera`, select `OBS Virtual Camera`. |
| Microsoft Teams | `Settings`, `Devices`, `Camera`, select `OBS Virtual Camera`.       |
| Google Meet     | `Settings`, `Video`, `Camera`, select `OBS Virtual Camera`.         |

NOTE: The camera appears in the list only while the virtual camera operates. Start the feed before you look for the camera in the program.

## 7. Next step

Continue with [Document 5](05-command-reference.md). Start the live feed.
