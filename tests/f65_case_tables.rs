//! The Unicode case tables of the native and WASM runtimes are the ones the
//! interpreter's `Upper` / `Lower` use: this test rebuilds them from Rust's
//! `char::to_uppercase` / `to_lowercase` (and, for the final sigma of
//! `str::to_lowercase`, the Cased / Case_Ignorable sets) and compares them
//! with the tables in both runtimes. After a toolchain update changes the
//! Unicode version, the message carries the tables to paste in.

const BEGIN: &str =
    "// ── Unicode case tables (generated; tests/f65_case_tables.rs checks them) ──\n";
const END: &str = "// ── end of Unicode case tables ──";

/// Group sorted (code point, mapped) pairs into disjoint runs
/// `(start, end, delta, stride)`: every `stride`-th code point from `start`
/// through `end` maps to itself plus `delta`.
fn single_runs(pairs: &[(u32, u32)]) -> Vec<(u32, u32, i64, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let (cp, mapped) = pairs[i];
        let delta = mapped as i64 - cp as i64;
        let mut best = (cp, delta, 1u32, 1usize);
        for stride in [1u32, 2u32] {
            let mut end = cp;
            let mut count = 1usize;
            while let Some(&(next, next_mapped)) = pairs.get(i + count) {
                if next == end + stride && next_mapped as i64 - next as i64 == delta {
                    end = next;
                    count += 1;
                } else {
                    break;
                }
            }
            if count > best.3 {
                best = (cp, delta, stride, count);
            }
        }
        let (start, delta, stride, count) = best;
        out.push((start, start + stride * (count as u32 - 1), delta, stride));
        i += count;
    }
    out
}

fn ranges_of(set: &[bool]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut cp = 0;
    while cp < set.len() {
        if set[cp] {
            let start = cp;
            while cp + 1 < set.len() && set[cp + 1] {
                cp += 1;
            }
            out.push((start as u32, cp as u32));
        }
        cp += 1;
    }
    out
}

fn expected_tables() -> String {
    let max = 0x110000usize;
    let mut up_single = Vec::new();
    let mut up_multi = Vec::new();
    let mut lo_single = Vec::new();
    let mut lo_multi = Vec::new();
    let mut cased = vec![false; max];
    let mut ignorable = vec![false; max];
    for cp in 0..max as u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let up: Vec<u32> = c.to_uppercase().map(|x| x as u32).collect();
        if up.len() > 1 {
            up_multi.push((cp, up));
        } else if up[0] != cp {
            up_single.push((cp, up[0]));
        }
        let low: Vec<u32> = c.to_lowercase().map(|x| x as u32).collect();
        if low.len() > 1 {
            lo_multi.push((cp, low));
        } else if low[0] != cp {
            lo_single.push((cp, low[0]));
        }
        // Cased: "cΣ" ends in the final sigma exactly when c is cased.
        let is_cased = format!("{c}\u{03A3}").to_lowercase().ends_with('\u{03C2}');
        cased[cp as usize] = is_cased;
        // Case_Ignorable: for a cased c, "AΣc" keeps the sigma final only
        // when c is skipped; for another c, "AcΣ" has a final sigma only
        // when c is skipped.
        ignorable[cp as usize] = if is_cased {
            format!("A\u{03A3}{c}").to_lowercase().chars().nth(1) == Some('\u{03C2}')
        } else {
            format!("A{c}\u{03A3}").to_lowercase().ends_with('\u{03C2}')
        };
    }
    let (major, minor, update) = char::UNICODE_VERSION;
    let mut out = String::new();
    out.push_str("// Generated from the interpreter's Unicode tables: Upper / Lower follow\n");
    out.push_str("// Rust's char::to_uppercase / to_lowercase, and the final-sigma rule of\n");
    out.push_str(&format!(
        "// str::to_lowercase reads the Cased / Case_Ignorable sets. Unicode {major}.{minor}.{update}.\n"
    ));
    for (name, single, multi) in [
        ("UPPER", &up_single, &up_multi),
        ("LOWER", &lo_single, &lo_multi),
    ] {
        let runs = single_runs(single);
        // The runtimes look runs up by binary search over disjoint ranges.
        assert!(
            runs.windows(2).all(|w| w[0].1 < w[1].0),
            "{name} runs overlap"
        );
        out.push_str(&format!(
            "static const int32_t TAIDA_CASE_{name}_RUNS[][4] = {{\n"
        ));
        for (start, end, delta, stride) in runs {
            out.push_str(&format!(
                "    {{0x{start:X}, 0x{end:X}, {delta}, {stride}}},\n"
            ));
        }
        out.push_str("};\n");
        out.push_str(&format!(
            "static const uint32_t TAIDA_CASE_{name}_SPECIAL[][4] = {{\n"
        ));
        for (cp, mapped) in multi {
            let mut cols = mapped.clone();
            cols.resize(3, 0);
            out.push_str(&format!(
                "    {{0x{cp:X}, 0x{:X}, 0x{:X}, 0x{:X}}},\n",
                cols[0], cols[1], cols[2]
            ));
        }
        out.push_str("};\n");
    }
    for (name, set) in [("CASED", &cased), ("CASE_IGNORABLE", &ignorable)] {
        out.push_str(&format!(
            "static const uint32_t TAIDA_{name}_RANGES[][2] = {{\n"
        ));
        for (start, end) in ranges_of(set) {
            out.push_str(&format!("    {{0x{start:X}, 0x{end:X}}},\n"));
        }
        out.push_str("};\n");
    }
    out
}

#[test]
fn runtime_case_tables_match_the_interpreter() {
    let expected = expected_tables();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in [
        "src/codegen/native_runtime/core.c",
        "src/codegen/runtime_core_wasm/02_containers.inc.c",
    ] {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        let start = source
            .find(BEGIN)
            .unwrap_or_else(|| panic!("{file}: no case tables"))
            + BEGIN.len();
        let end = start + source[start..].find(END).unwrap();
        assert!(
            source[start..end] == expected,
            "{file}: the case tables differ from the interpreter's; replace them with:\n{expected}"
        );
    }
}
