# Security policy

## 1. Report a problem

Do not open a public issue for a security problem.

Use the private vulnerability report of this repository: open the **Security**
tab, then **Report a vulnerability**. If that page is not available, send a
private message to the maintainer of the repository and say which files are
involved.

Give these items:

- The version or the commit.
- The operating system.
- The steps that show the problem.
- The effect: what an attacker can do.

Expect an answer in 7 days.

## 2. Scope

These parts are in scope:

- The OBS WebSocket password: how the program reads it, stores it and sends it.
- The loopback frame transport and the loopback control socket: which address
  the program binds, and what it accepts.
- The control socket: who can start and stop the feed.
- The files the program writes.

These parts are not in scope:

- A problem in OBS Studio, libgphoto2, gphoto2 or the camera firmware. Report
  that problem to the project that owns it.
- A user who gives the program a configuration file that other users can read.

## 3. Defaults

- The frame transport binds `127.0.0.1` only. It does not listen on a public
  address.
- The control socket is a Unix socket in a temporary directory.
- The program never writes camera firmware.
- The program never sends the OBS password to a remote host.
