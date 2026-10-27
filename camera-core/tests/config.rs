//! The configuration surface: five keys, TOML first and command line second.
//!
//! Validation happens here, long before a camera is opened. Every rejection names
//! the offending key, because a typo discovered halfway through a capture is a
//! worse failure than refusing to start.

use camera_core::config::{ConfigError, ConfigOverrides, StreamingConfig, UPSCALE_DETAIL_NOTE};
use camera_core::frame_size::FrameSize;
use camera_core::mechanism::{Platform, VirtualCameraMechanism};
use camera_core::pixel_format::PixelFormat;

const MACOS: Platform = Platform::MacOs;

fn resolve(file: Option<&str>) -> Result<StreamingConfig, ConfigError> {
    StreamingConfig::resolve(file, ConfigOverrides::default(), MACOS)
}

fn resolve_with(
    file: Option<&str>,
    overrides: ConfigOverrides,
) -> Result<StreamingConfig, ConfigError> {
    StreamingConfig::resolve(file, overrides, MACOS)
}

#[test]
fn an_empty_configuration_uses_the_documented_defaults() {
    let config = resolve(None).expect("defaults must be usable");

    assert_eq!(config.requested_size(), None);
    assert_eq!(config.fps_preference(), None);
    assert_eq!(config.pixel_format(), PixelFormat::Rgb24);
    assert!(!config.upscale(), "upscale defaults off");
    assert_eq!(
        config.mechanism(),
        VirtualCameraMechanism::ObsVirtualCamera,
        "macOS has exactly one mechanism, so it is the default"
    );
    assert_eq!(config.resize_target(), None);
}

#[test]
fn all_five_keys_are_read() {
    let file = r#"
resolution = "1080p"
fps = 24
pixel_format = "rgb24"
upscale = true
mechanism = "obs-virtual-camera"
"#;

    let config = resolve(Some(file)).expect("must resolve");

    assert_eq!(config.requested_size(), Some(FrameSize::new(1920, 1080)));
    assert_eq!(config.fps_preference(), Some(24.0));
    assert_eq!(config.pixel_format(), PixelFormat::Rgb24);
    assert!(config.upscale());
    assert_eq!(config.mechanism(), VirtualCameraMechanism::ObsVirtualCamera);
}

#[test]
fn a_fractional_frame_rate_preference_is_accepted() {
    let config = resolve(Some("fps = 23.976")).expect("must resolve");

    assert_eq!(config.fps_preference(), Some(23.976));
}

#[test]
fn a_requested_size_is_only_a_preference() {
    let config = resolve(Some("resolution = \"1080p\"")).expect("must resolve");

    assert_eq!(
        config.requested_size(),
        Some(FrameSize::new(1920, 1080)),
        "the requested size is recorded as asked for"
    );
    assert_eq!(
        config.resize_target(),
        None,
        "and it is not applied to the frames unless upscaling is asked for"
    );
}

#[test]
fn v4l2_on_macos_is_a_configuration_error_rather_than_a_fallback() {
    let error = resolve(Some("mechanism = \"v4l2loopback\""))
        .expect_err("a mechanism macOS does not have must be refused");

    assert!(
        matches!(error, ConfigError::Mechanism(_)),
        "error was {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("`mechanism`"), "message was {message}");
    assert!(message.contains("v4l2loopback"), "message was {message}");
    assert!(message.contains("macOS"), "message was {message}");
    assert!(
        message.contains("obs-virtual-camera"),
        "the refusal must say what macOS does have, message was {message}"
    );
    assert!(
        !message.contains("falling back") && !message.contains("using obs"),
        "the refusal must not read as a fallback, message was {message}"
    );
}

#[test]
fn a_mechanism_from_another_platform_is_refused_on_that_platform_too() {
    let error = StreamingConfig::resolve(
        Some("mechanism = \"obs-virtual-camera\""),
        ConfigOverrides::default(),
        Platform::Linux,
    )
    .expect_err("Linux has no mechanism in this build");

    assert!(
        error.to_string().contains("no virtual camera mechanism"),
        "message was {error}"
    );
}

#[test]
fn an_unknown_key_is_named_rather_than_ignored() {
    let error = resolve(Some("resoluton = \"1080p\"")).expect_err("a typo must be refused");

    assert_eq!(
        error,
        ConfigError::UnknownKey {
            key: "resoluton".to_owned()
        }
    );
    let message = error.to_string();
    assert!(message.contains("`resoluton`"), "message was {message}");
    assert!(message.contains("resolution"), "message was {message}");
}

#[test]
fn a_resolution_that_is_not_a_frame_size_names_the_key() {
    let error = resolve(Some("resolution = \"huge\"")).expect_err("must be refused");

    let message = error.to_string();
    assert!(message.contains("`resolution`"), "message was {message}");
    assert!(message.contains("huge"), "message was {message}");
}

#[test]
fn a_frame_rate_that_is_not_positive_names_the_key() {
    for file in ["fps = 0", "fps = -30", "fps = \"24\""] {
        let error = resolve(Some(file)).expect_err("must be refused");
        assert!(
            error.to_string().contains("`fps`"),
            "for {file:?} the message was {error}"
        );
    }
}

#[test]
fn a_pixel_format_the_pipeline_does_not_produce_names_the_key() {
    let error = resolve(Some("pixel_format = \"yuv420p\"")).expect_err("must be refused");

    assert!(
        error.to_string().contains("`pixel_format`"),
        "message was {error}"
    );
}

#[test]
fn an_upscale_that_is_not_a_boolean_names_the_key() {
    let error = resolve(Some("upscale = \"yes\"")).expect_err("must be refused");

    let message = error.to_string();
    assert!(message.contains("`upscale`"), "message was {message}");
    assert!(message.contains("boolean"), "message was {message}");
}

#[test]
fn upscaling_without_a_size_to_resize_to_names_the_key() {
    let error = resolve(Some("upscale = true")).expect_err("must be refused");

    assert_eq!(error, ConfigError::UpscaleWithoutResolution);
    let message = error.to_string();
    assert!(message.contains("`upscale`"), "message was {message}");
    assert!(message.contains("`resolution`"), "message was {message}");
}

#[test]
fn the_upscale_note_labels_resizing_as_resizing_and_says_it_adds_no_detail() {
    let config = resolve(Some("resolution = \"1080p\"\nupscale = true")).expect("must resolve");

    let note = config.upscale_note().expect("upscaling must be labelled");

    assert!(note.contains("resizing"), "note was {note}");
    assert!(note.contains("1920x1080"), "note was {note}");
    assert!(note.contains(UPSCALE_DETAIL_NOTE), "note was {note}");
    assert!(note.contains("not extra resolution"), "note was {note}");
    assert_eq!(
        config.resize_target(),
        Some(FrameSize::new(1920, 1080)),
        "with upscaling on the requested size is the resize target"
    );
}

#[test]
fn no_upscale_note_is_produced_when_resizing_is_off() {
    let config = resolve(Some("resolution = \"1080p\"")).expect("must resolve");

    assert_eq!(config.upscale_note(), None);
}

#[test]
fn command_line_values_override_the_file() {
    let overrides = ConfigOverrides {
        resolution: Some("720p".to_owned()),
        fps: Some("30".to_owned()),
        pixel_format: Some("rgb24".to_owned()),
        upscale: Some(false),
        mechanism: Some("obs-virtual-camera".to_owned()),
    };

    let config = resolve_with(
        Some("resolution = \"1080p\"\nfps = 24\nupscale = true"),
        overrides,
    )
    .expect("must resolve");

    assert_eq!(config.requested_size(), Some(FrameSize::new(1280, 720)));
    assert_eq!(config.fps_preference(), Some(30.0));
    assert!(!config.upscale(), "the command line wins over the file");
}

#[test]
fn a_command_line_override_is_validated_the_same_way_as_the_file() {
    let overrides = ConfigOverrides {
        mechanism: Some("media-foundation".to_owned()),
        ..ConfigOverrides::default()
    };

    let error = resolve_with(Some("mechanism = \"obs-virtual-camera\""), overrides)
        .expect_err("an override must be validated too");

    let message = error.to_string();
    assert!(message.contains("`mechanism`"), "message was {message}");
    assert!(
        message.contains("media-foundation"),
        "message was {message}"
    );
}

#[test]
fn malformed_toml_is_refused() {
    let error = resolve(Some("resolution = ")).expect_err("must be refused");

    assert!(
        matches!(error, ConfigError::InvalidToml { .. }),
        "error was {error:?}"
    );
    assert!(
        error.to_string().contains("not valid TOML"),
        "message was {error}"
    );
}

#[test]
fn a_missing_file_is_an_unreadable_configuration() {
    let error = StreamingConfig::load(
        std::path::Path::new("/nonexistent/canon-webcam.toml"),
        ConfigOverrides::default(),
        MACOS,
    )
    .expect_err("must be refused");

    assert!(
        matches!(error, ConfigError::Unreadable { .. }),
        "error was {error:?}"
    );
}

#[test]
fn the_pipeline_is_built_from_the_resize_target_only_when_upscaling_is_on() {
    use camera_core::calibration::CalibrationPolicy;
    use std::time::Duration;

    let policy = CalibrationPolicy::new(20, Duration::from_secs(3), 8).expect("valid policy");

    let plain = resolve(Some("resolution = \"1080p\""))
        .expect("must resolve")
        .pipeline_config(policy.clone());
    assert_eq!(plain.resize_to(), None);

    let upscaled = resolve(Some("resolution = \"1080p\"\nupscale = true"))
        .expect("must resolve")
        .pipeline_config(policy);
    assert_eq!(upscaled.resize_to(), Some(FrameSize::new(1920, 1080)));
}

#[cfg(target_os = "macos")]
#[test]
fn the_macos_section_is_read_as_the_mechanism_prerequisites() {
    // The platform backend is built from this table, not from flags beside it, so
    // a value here must reach the prerequisite the backend checks.
    let file = r#"
[macos]
obs_app_path = "/tmp/nowhere/OBS.app"
websocket_endpoint = "127.0.0.1:4466"
websocket_password = "canon-webcam"
obs_log_directory = "/tmp/nowhere/logs"
launch_obs = false
"#;

    let config = resolve(Some(file)).expect("must resolve");
    let prerequisites = config.macos_prerequisites();

    assert_eq!(
        prerequisites.obs_app_path,
        std::path::PathBuf::from("/tmp/nowhere/OBS.app")
    );
    assert_eq!(
        prerequisites.websocket_endpoint,
        "127.0.0.1:4466"
            .parse()
            .expect("a socket address was written")
    );
    assert_eq!(
        prerequisites.websocket_password.as_deref(),
        Some("canon-webcam")
    );
    assert_eq!(
        prerequisites.obs_log_directory,
        std::path::PathBuf::from("/tmp/nowhere/logs")
    );
    assert!(!prerequisites.launch_obs, "a test turns launching off");
}

#[cfg(target_os = "macos")]
#[test]
fn the_macos_section_defaults_to_the_application_obs_insists_on() {
    let prerequisites = resolve(None).expect("must resolve");
    let prerequisites = prerequisites.macos_prerequisites();

    assert_eq!(
        prerequisites.obs_app_path,
        std::path::PathBuf::from("/Applications/OBS.app"),
        "P1 reads the path OBS installs its virtual camera from"
    );
    assert!(
        prerequisites.launch_obs,
        "a real run may start OBS; only a test turns that off"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_misspelt_macos_key_is_refused_by_name() {
    let error = resolve(Some("[macos]\nobs_path = \"/Applications/OBS.app\""))
        .expect_err("a typo in the macOS section must be refused");

    assert!(
        matches!(error, ConfigError::UnknownMacOsKey { .. }),
        "error was {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("macos.obs_path"), "was {message}");
    assert!(
        message.contains("macos.obs_app_path"),
        "the refusal must name the keys the section does have, was {message}"
    );
}
