//! The `output` guard is an allowlist, not a blocklist.
//!
//! Verified on an M50 (see TEST-RESULTS.md §4.5): `TFT + PC` (3) gives the full
//! frame size and `MOBILE2` (8) gives the reduced frame size. Everything else is
//! unverified here, and values combining `PC` and `MOBILE` are reported to need
//! the battery removed to clear the error. So 8 is rejected even though it is
//! verified: it is the reduced size, and it is outside the allowlist.

use camera_core::output_setting::{OutputSetting, OutputSettingError};

#[test]
fn the_allowlisted_values_are_accepted() {
    for raw in OutputSetting::ALLOWED_RAW_VALUES {
        let setting = OutputSetting::from_raw(raw).expect("allowlisted value must be accepted");
        assert_eq!(setting.raw(), raw);
    }
}

#[test]
fn the_full_size_setting_is_tft_plus_pc_with_value_three() {
    let setting = OutputSetting::tft_plus_pc();

    assert_eq!(setting.raw(), 3);
    assert_eq!(setting.choice_name(), "TFT + PC");
    assert!(setting.is_verified_on_m50());
}

#[test]
fn mobile2_is_rejected_even_though_it_is_verified_on_an_m50() {
    let error = OutputSetting::from_raw(8).expect_err("MOBILE2 is the reduced size");

    assert_eq!(error, OutputSettingError::new(8));
    assert_eq!(error.raw(), 8);

    let message = error.to_string();
    assert!(message.contains("MOBILE2"), "message was {message:?}");
    assert!(message.contains("reduced"), "message was {message:?}");
}

#[test]
fn the_rejection_message_explains_which_values_are_verified() {
    let message = OutputSetting::from_raw(6)
        .expect_err("value 6 is refused")
        .to_string();

    assert!(
        message.contains("3") && message.contains("TFT + PC"),
        "message was {message:?}"
    );
    assert!(
        message.contains("battery"),
        "the battery removal report must be mentioned, message was {message:?}"
    );
    assert!(
        message.contains("unverified"),
        "the message must say the rest is unverified, message was {message:?}"
    );
}

#[test]
fn the_pc_and_mobile_combinations_from_the_camera_report_are_rejected() {
    for raw in [6, 7, 10, 11] {
        assert_eq!(
            OutputSetting::from_raw(raw).expect_err("combining PC and MOBILE is rejected"),
            OutputSettingError::new(raw)
        );
    }
}

#[test]
fn every_value_outside_the_allowlist_is_rejected() {
    for raw in 0_u8..=u8::MAX {
        let accepted = OutputSetting::from_raw(raw).is_ok();
        assert_eq!(
            accepted,
            matches!(raw, 0..=3),
            "value {raw} must be accepted only if it is allowlisted"
        );
    }
}

#[test]
fn only_the_full_size_value_is_verified_on_an_m50() {
    assert!(OutputSetting::tft_plus_pc().is_verified_on_m50());
    for raw in [0, 1, 2] {
        let setting = OutputSetting::from_raw(raw).expect("allowlisted");
        assert!(
            !setting.is_verified_on_m50(),
            "value {raw} is allowlisted but not verified on an M50"
        );
    }
}

#[test]
fn the_setting_renders_for_a_human() {
    assert_eq!(OutputSetting::tft_plus_pc().to_string(), "TFT + PC (3)");
}
