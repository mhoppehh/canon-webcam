# Internal records

This directory holds the working records from the development of this project.
They are published for transparency. They are historical records, not user
documentation. A statement here can contradict the software as it is today.

For instructions, read `README.md` and `docs/user/`.

## Contents

| Path        | Content                                                                                   |
| ----------- | ----------------------------------------------------------------------------------------- |
| `plan/`     | The implementation plan, the pre-implementation test plan, and the recorded test results. |
| `tickets/`  | The implementation tickets, one file per ticket, numbered `01` to `10`.                   |
| `research/` | The research note on virtual camera mechanisms that do not use OBS.                       |
| `agents/`   | The conventions that the coding agents used in this repository.                           |

The documents were renamed when they moved here:

| Old name                      | New name                           |
| ----------------------------- | ---------------------------------- |
| `PLAN.md`                     | `plan/plan.md`                     |
| `PRE-IMPLEMENTATION-TESTS.md` | `plan/pre-implementation-tests.md` |
| `TEST-RESULTS.md`             | `plan/test-results.md`             |

Machine-specific data was removed from these documents before publication. The
camera serial number, the signing identity, and the home directory paths are
redacted.

## Tickets

Several architecture decision records refer to a ticket number. Use this table
to find the ticket.

| Ticket | File                                        | Title                                                  |
| ------ | ------------------------------------------- | ------------------------------------------------------ |
| 01     | `tickets/01-probe-ptp-session.md`           | Capture session foundation and live-view measurement   |
| 01     | `tickets/01-measure-ptp-session.md`         | Capture session foundation and live-view measurement   |
| 02     | `tickets/02-live-view-pipeline.md`          | Sustained live-view stream pipeline                    |
| 03     | `tickets/03-macos-sink-decision.md`         | macOS virtual-camera sink decision                     |
| 04     | `tickets/04-virtual-camera-sink-macos.md`   | Virtual camera sink and macOS backend                  |
| 05     | `tickets/05-ptp-recovery.md`                | PTP failure detection and recovery                     |
| 06     | `tickets/06-sink-teardown-on-disconnect.md` | Sink teardown on camera disconnect                     |
| 07     | `tickets/07-start-stop-control.md`          | Start and stop the feed without restarting the process |
| 08     | `tickets/08-windows-backend.md`             | Windows virtual camera backend                         |
| 09     | `tickets/09-linux-v4l2loopback-backend.md`  | Linux v4l2loopback backend                             |
| 10     | `tickets/10-wire-platform-sink-into-cli.md` | Wire the platform sink into the CLI                    |

Ticket 08 and ticket 09 are not implemented. This release supports macOS only.
