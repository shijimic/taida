pub const NET_HTTP_PROTOCOL_SYMBOL: &str = "HttpProtocol";
pub const NET_HTTP_PROTOCOL_VARIANTS: [&str; 3] = ["H1", "H2", "H3"];

pub const NET_RUNTIME_BUILTIN_NAMES: [&str; 15] = [
    "httpServe",
    "httpParseRequestHead",
    "httpEncodeResponse",
    "readBody",
    "startResponse",
    "writeChunk",
    "endResponse",
    "sseEvent",
    "readBodyChunk",
    "readBodyAll",
    "wsUpgrade",
    "wsSend",
    "wsReceive",
    "wsClose",
    "wsCloseCode",
];

pub const NET_EXPORT_NAMES: [&str; 16] = [
    "httpServe",
    "httpParseRequestHead",
    "httpEncodeResponse",
    "readBody",
    "startResponse",
    "writeChunk",
    "endResponse",
    "sseEvent",
    "readBodyChunk",
    "readBodyAll",
    "wsUpgrade",
    "wsSend",
    "wsReceive",
    "wsClose",
    "wsCloseCode",
    NET_HTTP_PROTOCOL_SYMBOL,
];

pub fn is_net_runtime_builtin(name: &str) -> bool {
    NET_RUNTIME_BUILTIN_NAMES.contains(&name)
}

pub fn is_net_export_name(name: &str) -> bool {
    NET_EXPORT_NAMES.contains(&name)
}

pub fn net_export_list() -> String {
    NET_EXPORT_NAMES.join(", ")
}

pub fn http_protocol_variant_to_wire(variant: &str) -> Option<&'static str> {
    match variant {
        "H1" => Some("h1.1"),
        "H2" => Some("h2"),
        "H3" => Some("h3"),
        _ => None,
    }
}

pub fn http_protocol_ordinal_to_wire(ordinal: i64) -> Option<&'static str> {
    match ordinal {
        0 => Some("h1.1"),
        1 => Some("h2"),
        2 => Some("h3"),
        _ => None,
    }
}
