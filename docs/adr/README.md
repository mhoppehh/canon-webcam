# Architecture decision records

An architecture decision record (ADR) gives one decision, its context, and the
consequences. Read an ADR to learn why the software has a given shape.

| ADR  | Decision                                                        |
| ---- | --------------------------------------------------------------- |
| 0001 | One PTP session for the whole run.                              |
| 0002 | OBS Virtual Camera is the macOS virtual camera mechanism.       |
| 0003 | The capture layer binds libgphoto2 through the `gphoto2` crate. |
| 0004 | The software does not select a live view size.                  |
| 0005 | Start and stop control through a Unix socket.                   |

`0002-macos-virtual-camera-mechanism-evidence.md` is the evidence appendix for
ADR 0002. It records the command output that the decision rests on.
