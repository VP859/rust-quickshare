//! UDP announcements on the local network (anyone can send them, and anyone
//! can sign arbitrary content with their own key).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ferry::discovery::fuzz_announcement(data));
