//! E33 native HPACK Huffman hardening regressions.

const NATIVE_NET: &str = include_str!("../src/codegen/native_runtime/net_h1_h2.c");

#[test]
fn native_hpack_huffman_eos_symbol_is_not_truncated() {
    // HPACK's Huffman EOS pseudo-symbol is 256. A uint8_t field silently
    // truncates it to 0, making the explicit EOS rejection checks dead.
    assert!(
        NATIVE_NET
            .contains("typedef struct { uint16_t sym; uint32_t code; uint8_t bits; } H2HuffEntry;"),
        "native HPACK Huffman table must keep 16-bit symbols so EOS remains 256"
    );
    assert!(
        NATIVE_NET.contains("uint16_t sym;\n    uint8_t bits;"),
        "native HPACK Huffman lookup table must keep 16-bit symbols so EOS remains 256"
    );
    assert!(
        NATIVE_NET.contains("{256, 0x3fffffff,30}"),
        "native HPACK Huffman table must retain the RFC 7541 EOS entry"
    );
    assert!(
        NATIVE_NET.contains("if (entry->sym == 256) return -1;"),
        "native HPACK Huffman fast lookup must reject EOS"
    );
    assert!(
        NATIVE_NET.contains("if (H2_HUFFMAN_TABLE[t].sym == 256) return -1;"),
        "native HPACK Huffman slow path must reject EOS"
    );
    assert!(
        NATIVE_NET.contains("dst[out++] = (unsigned char)entry->sym;")
            && NATIVE_NET.contains("dst[out++] = (unsigned char)H2_HUFFMAN_TABLE[t].sym;"),
        "native HPACK Huffman decoder should cast only after EOS has been rejected"
    );
}
