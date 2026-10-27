# Document 2 — Build and install

| Item             | Value                                   |
| ---------------- | --------------------------------------- |
| Document number  | UG-002                                  |
| Date             | 2026-10-27                              |
| Writing standard | ASD-STE100 Simplified Technical English |

## 1. Purpose

This document tells you how to get the source code, build the program, install the program and verify the installation.

Do the steps in the sequence given. Document 1 gives the requirements.

## 2. Get the source code

Step 1. Open a terminal.

Step 2. Go to the directory where you keep source code.

Step 3. Get the source code with git:

```
git clone <repository-url> canon-webcam
cd canon-webcam
```

Step 4. Make sure that git uses the branch that contains the current work:

```
git status
```

NOTE: The current implementation is on the branch `feat/virtual-camera`. Ask the repository owner which branch to use.

## 3. Build the program

Step 1. Make sure that pkg-config finds libgphoto2:

```
pkg-config --modversion libgphoto2
```

Result: The command shows a version number, for example `2.5.34`.

If the command shows an error, install libgphoto2 and pkg-config again. Document 1 gives the commands.

Step 2. Build the release program:

```
cargo build --release
```

The build downloads the Rust libraries. Then it compiles the program. The first build takes some minutes.

Result: The build ends with the message `Finished \`release\` profile`.

Step 3. The program is in this file:

```
target/release/camera-cli
```

Step 4. Make sure that the program starts:

```
./target/release/camera-cli help
```

Result: The program shows the help text. The help text names these commands: `measure`, `stream`, `serve`, `control` and `help`.

NOTE: Use `--release` for the live feed. A debug build is slower. The live feed can then drop frames.

## 4. Install the program

You can use the program from the build directory, or you can install it in the Cargo binary directory. Select one method.

### 4.1 Method 1 — Use the build directory

Step 1. Move the program to a directory in your `PATH`. This example uses `/usr/local/bin`:

```
install -m 0755 target/release/camera-cli /usr/local/bin/camera-cli
```

If the directory needs administrator rights, use `sudo` before the command.

Step 2. Make sure that the shell finds the program:

```
which camera-cli
```

### 4.2 Method 2 — Install with Cargo

Step 1. Run this command in the repository root:

```
cargo install --path camera-cli
```

Step 2. Cargo installs the program in `~/.cargo/bin`. Make sure that this directory is in your `PATH`.

Step 3. Make sure that the shell finds the program:

```
which camera-cli
```

NOTE: Both methods install the program only. The program needs libgphoto2 and OBS at run time. Do not delete libgphoto2 after the build.

## 5. Verify the installation

Do these checks in the sequence given. A failure in a check tells you which part of the installation is not correct.

### 5.1 Check the program

```
camera-cli help
```

Expected result: The help text. This check does not need a camera.

### 5.2 Check the camera

Step 1. Connect the camera. Make sure that the camera is on.

Step 2. Run this command:

```
gphoto2 --auto-detect
```

Expected result: One line with `Canon EOS M50`.

If the camera is absent, read Document 7, Section 3.

Step 3. Make sure that no other program uses the camera. Close Image Capture, Photos and all `gphoto2` processes.

### 5.3 Check the live view pipeline without OBS

Step 1. Run a synthetic run. A synthetic run uses no camera and no OBS:

```
camera-cli stream --synthetic --duration-secs 5
```

Expected result: The program shows the measured frame size, the measured frame rate and the counters of the run.

Step 2. Run a synthetic run with the platform sink. This step tests the OBS connection:

```
camera-cli stream --synthetic --sink virtual-camera --duration-secs 5
```

Expected result: The program connects to OBS and shows the message `sink: obs-virtual-camera`.

If the program shows a message about the OBS WebSocket server, read Document 4 and Document 7.

### 5.4 Check the test suite (optional)

This check is for developers. It shows that the source code operates correctly on your computer:

```
cargo test --workspace
```

Expected result: Each test target shows `test result: ok`. No test fails.

## 6. Update the installation

Step 1. Get the new source code:

```
git pull
```

Step 2. Build and install the program again. Repeat Section 3 and Section 4.

## 7. Remove the installation

Step 1. If you used Cargo, remove the program:

```
cargo uninstall camera-cli
```

Step 2. If you used Method 1, remove the file:

```
rm /usr/local/bin/camera-cli
```

Step 3. Remove the build files if you do not need them:

```
cargo clean
```

Removing the program does not change OBS, the camera or the configuration file.

## 8. Next step

Continue with [Document 4](04-obs-setup.md). Set up OBS before you start the live feed.
