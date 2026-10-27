# 10: Wire the platform sink into the CLI

**What to build:** `camera-cli` has only ever sunk frames to `StatsSink`. `MacOsVirtualCameraSink` is constructed in tests and nowhere else, so no run of the binary can publish to OBS, and therefore no consumer application can list the camera. This ticket is small but it is the one that makes the product reachable: without it, the hardware criteria on tickets 02, 04, 06 and 07 have no runnable path, and the evidence command ticket 04 records (`cargo run -p camera-cli -- stream --sink virtual-camera --duration-secs 600`) names a flag that does not exist.

**Blocked by:** 04, 06, 07

**Status:** resolved (hardware verification outstanding)

- [x] `stream` and `serve` can select the platform sink, and the default is stated in `--help`.
- [x] The macOS sink is built from the configuration the mechanism validation already reads (the macOS mechanism section), not from new ad-hoc flags beside it.
- [x] A run against the platform sink reports which sink published and its own status counters — consumer page connected, frames published, frames dropped — instead of only the stats sink's counters.
- [x] Selecting the platform sink where the mechanism is not compiled in fails with the actionable message that names the setting or command to apply, rather than silently falling back to the stats sink.
- [x] `control start` and `control stop` release and re-take the platform sink, not only the stats sink, so the stop/start contract of ticket 07 holds for the real backend.
- [ ] A run publishes to OBS and a consumer video application lists the camera and shows the feed (needs OBS in `/Applications` and the camera).
- [x] The evidence commands recorded on tickets 02, 04, 06 and 07 run as written.

## Comments

### The selection surface

`--sink stats|virtual-camera` on `stream` and `serve`, and `--sink stats` is the
default, stated in `--help`. `virtual-camera` is not a second mechanism flag: it
selects the **platform backend**, whose mechanism is still the `mechanism` key
that the configuration already validates. The prerequisites ADR-0002's checks
read come from a `[macos]` table in that same configuration file —
`obs_app_path`, `websocket_endpoint`, `websocket_password`, `obs_log_directory`
and `launch_obs` — parsed by `camera_core::config` and read through
`StreamingConfig::macos_prerequisites`. The scene and input names stay the ADR's
constants.

Both `stream` and the `serve`/`control` feed factory build their sink through
`camera_cli::sink::SinkPlan`, and `SinkPlan::factory()` hands out a **new** sink
for every segment and every start. That is the whole of "a stop releases the
virtual camera and a start takes it again": there is no second construction path
to keep in step, and no sink is ever reused after it was closed.

Selecting `virtual-camera` where it cannot be built is an error, never a
fallback: the configured mechanism is checked with `VirtualCameraMechanism`, so a
platform without the mechanism answers with the configuration's own wording, and
a build without the platform backend answers that it is not compiled in.

### What was verified, and how

No OBS is installed in `/Applications` on this machine and no camera is attached,
so nothing here opens the platform sink successfully. What a machine without OBS
*can* prove is the selection, the configuration plumbing, the refusal and the
report.

- `camera-cli/tests/sink_selection.rs`
  - `the_flag_selects_the_platform_sink_and_the_default_is_the_counting_sink` —
    the default is `stats`, `--sink stats` and `--sink virtual-camera` select what
    they say, and a value that is neither is refused naming both.
  - `the_help_text_states_the_default_sink` — `--help` carries `--sink <name>`,
    says which value is the default, and describes the platform sink.
  - `selecting_the_platform_sink_where_it_is_not_compiled_in_does_not_fall_back` —
    `SinkPlan::select_for` refuses `virtual-camera` on a platform with no backend
    with the message that says the live view feed has nowhere to go, and still
    offers the counting sink when it is asked for by name.
  - `a_missing_obs_is_reported_with_the_configured_path_and_the_real_instruction`
    — a `[macos]` table pointing `obs_app_path` at a path that does not exist makes
    `stream --sink virtual-camera` fail with the configured path in the message and
    the ADR's own "Install OBS Studio" instruction, not the counting sink's
    counters and not the compiled-in default path.
  - `a_run_report_names_the_counting_sink_that_published` — a real synthetic run's
    report says `sink: stats (the counting sink; …)` and prints no platform
    counters.
- `camera-cli/tests/report_stream.rs`
  - `the_report_names_the_platform_sink_and_its_own_counters` — the report names
    `obs-virtual-camera` as the sink that published and prints the sink's own
    consumer page state, frames published, frames dropped and one-off notes,
    alongside (not instead of) the pipeline's counters.
  - `the_counting_sink_reports_no_platform_counters_at_all` — zeros are never
    printed for the counting sink, because they would read as a measurement it
    never made.
  - `a_platform_sink_that_published_nothing_does_not_read_as_a_publication` — a
    platform run that never reached a consumer page says so.
- `camera-cli/tests/control.rs`
  - `a_start_builds_the_platform_sink_through_the_same_path_a_stream_run_uses` —
    `serve --sink virtual-camera` with a configuration pointing at an absent OBS
    answers `started`, the run then ends by itself, and `control status` answers
    `error: …` with the OBS prerequisite's own message naming the configured path.
    That is the same construction path `stream` uses, reached through the control
    socket, and a failed run is released (`control stop` answers
    `already-stopped`).
- `camera-core/tests/config.rs`
  - `the_macos_section_is_read_as_the_mechanism_prerequisites` — every key of the
    `[macos]` table reaches `ObsPrerequisites`.
  - `the_macos_section_defaults_to_the_application_obs_insists_on` —
    `/Applications/OBS.app` and launching on are the defaults.
  - `a_misspelt_macos_key_is_refused_by_name` — `macos.obs_path` is refused and
    the refusal lists the keys the section does have.
- `camera-core/tests/macos_virtual_camera_sink.rs`
  - `the_sink_counters_outlive_the_close_so_a_run_report_can_print_them` — two
    frames reach the consumer page, the sink is closed, and its own counters still
    report two published frames and a consumer page that attached. Without this
    the report would have printed zeros, because the transport goes with the open
    sink.

`camera-core/tests/control_socket.rs`, `start_stop_control.rs`,
`start_stop_cycles.rs`, `camera-cli/tests/binary.rs` and `measure_command.rs` pass
unchanged: `--synthetic` and the stats sink behave exactly as before, and the
`status` word still answers `running` or `stopped` for a feed that did not fail.

### Evidence commands that now run as written

- Ticket 04: `cargo run -p camera-cli -- stream --sink virtual-camera --duration-secs 600`
  — the flag exists and the command is accepted; on this machine it reaches the
  camera and ends with the camera's own failure, not a usage error. The consumer
  application half of that ticket still needs hardware.
- Ticket 02: `cargo run -p camera-cli -- stream --synthetic --duration-secs 6 --resolution 1080p`
  runs as written (the longer soaks still need the camera).
- Ticket 07: the `camera-cli serve --socket /tmp/cw.sock --synthetic` sequence runs
  as written; `serve` now also prints the sink it will publish through.
- Ticket 06 records no command line, so there was nothing to change. No ticket's
  criteria were rewritten.

### Unverified (no OBS in `/Applications`, no camera attached)

- **A consumer application listing the camera.** Needs OBS installed under
  `/Applications`, its camera extension approved, and a device list in Discord,
  Teams or Meet. Closing evidence is the AVFoundation listing containing
  `OBS Virtual Camera` and the feed in a consumer application.
- **That a run against the platform sink publishes for ten minutes.** Same
  prerequisite plus a real M50 as the frame source. Closing evidence: the command
  above, with the report showing non-zero `sink frames published` and
  `sink consumer page connected: yes`.
- **That the device is released and re-taken across `control stop`/`control start`.**
  The construction is shared and a fresh sink is built per start, and a failed
  start is shown to reach the platform sink's own prerequisites, but the OBS-side
  release is only observable with OBS running: closing evidence is a consumer
  application losing and regaining the device across a stop/start.
