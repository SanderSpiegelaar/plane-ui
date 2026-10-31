use std::fs;
use std::path::{Path, PathBuf};

use planeui_core::config_doc::ConfigDoc;

#[test]
fn every_fixture_round_trips_byte_for_byte() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths: Vec<PathBuf> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures found");

    let failures: Vec<String> = paths
        .iter()
        .filter_map(|path| {
            let source = fs::read_to_string(path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy();
            match ConfigDoc::parse(&source) {
                Err(error) => Some(format!("{name}: parse failed: {error}")),
                Ok(document) if document.to_string() != source => {
                    Some(format!("{name}: output differs from source"))
                }
                Ok(_) => None,
            }
        })
        .collect();

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn crlf_fixture_keeps_crlf_line_endings() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.crlf.toml"),
    )
    .unwrap();
    assert!(source.contains("\r\n"), "fixture lost its CRLF endings");
    assert!(
        !source.replace("\r\n", "").contains('\n'),
        "fixture mixes LF into CRLF"
    );
}
