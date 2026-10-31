//! TICKETs pasted or scanned by the user: any text must be either rejected or
//! turned into an address.
#![no_main]

use ferry::node::NodeTicket;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = NodeTicket::looks_like_ticket(text);
    let _ = NodeTicket::parse_target(text);
    if let Ok(ticket) = NodeTicket::deserialize_from_str(text) {
        // A parsed TICKET can be shared again and reads back the same.
        let again = ticket.serialize_to_string().expect("serializable");
        let back = NodeTicket::deserialize_from_str(&again).expect("round trip");
        assert_eq!(back.addr.id, ticket.addr.id);
        assert_eq!(back.share, ticket.share);
    }
});
