//! C25B-008 probe — measure parse-success rate of ```taida blocks in docs/.
//!
//! This is an *exploratory* test (marked `#[ignore]`) used once during Phase 8
//! to size the doctest effort and to capture the initial parse baseline.
//! The real guard test is `c25b_008_doc_examples_parse.rs` which consumes the
//! baseline.
//!
//! Run manually with:
//!     cargo test --release --test c25b_008_doc_examples_probe -- --ignored --nocapture

use std::fs;
use std::path::PathBuf;

fn extract_taida_blocks(md: &str) -> Vec<(usize, String, String)> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut info = String::new();
    let mut body = String::new();
    let mut start = 0;
    for (i, line) in md.lines().enumerate() {
        let trimmed_start = line.trim_start();
        if !in_block {
            if let Some(rest) = trimmed_start.strip_prefix("```") {
                let word = rest.split_whitespace().next().unwrap_or("").to_string();
                if word == "taida" {
                    in_block = true;
                    info = rest.to_string();
                    body.clear();
                    start = i + 1;
                }
            }
        } else if trimmed_start.starts_with("```") {
            blocks.push((start, info.clone(), body.clone()));
            in_block = false;
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    blocks
}

#[test]
#[ignore]
fn probe_doc_taida_blocks_parse_rate() {
    let mut total = 0usize;
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut fail_list: Vec<String> = Vec::new();

    for dir in ["docs/guide", "docs/reference"] {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        paths.sort();
        for path in paths {
            let content = fs::read_to_string(&path).unwrap();
            for (line, info, body) in extract_taida_blocks(&content) {
                total += 1;
                let (_prog, errs) = taida::parser::parse(&body);
                if errs.is_empty() {
                    pass += 1;
                } else {
                    fail += 1;
                    fail_list.push(format!(
                        "{}:{} info='{}' errs={}",
                        path.display(),
                        line,
                        info.trim(),
                        errs.len()
                    ));
                }
            }
        }
    }

    println!("--- doc parse probe ---");
    println!("TOTAL: {}  PASS: {}  FAIL: {}", total, pass, fail);
    println!(
        "PASS%: {:.1}%",
        (pass as f64) / (total.max(1) as f64) * 100.0
    );
    println!("--- failures ---");
    for f in &fail_list {
        println!("{}", f);
    }
}
