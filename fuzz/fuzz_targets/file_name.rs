//! File and device names from other devices.
#![no_main]

use ferry::pairing::clean_device_name;
use ferry::protocol::messages::MAX_DEVICE_NAME_LEN;
use ferry::transfer::security::sanitize_filename;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|name: &str| {
    if let Ok(clean) = sanitize_filename(name) {
        assert!(!clean.is_empty());
        assert!(!clean.contains(['/', '\\', '\0']) && !clean.contains(".."));
        assert!(!clean.chars().any(char::is_control));
        assert_eq!(sanitize_filename(&clean).as_deref(), Ok(clean.as_str()), "idempotent");
    }
    let device = clean_device_name(name);
    assert!(device.len() <= MAX_DEVICE_NAME_LEN);
    assert!(!device.chars().any(char::is_control));
});
