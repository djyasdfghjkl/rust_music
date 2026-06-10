use std::fs;
use std::path::PathBuf;

fn main() {
    tauri_build::build();

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let source_dir = manifest_dir
        .parent()
        .map(|parent| parent.join("音源"))
        .unwrap_or_else(|| manifest_dir.join("音源"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let generated = out_dir.join("embedded_sources.rs");

    println!("cargo:rerun-if-changed={}", source_dir.display());

    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&source_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("js") {
                println!("cargo:rerun-if-changed={}", path.display());
                entries.push(path);
            }
        }
    }

    entries.sort();

    let mut output = String::from("pub static EMBEDDED_SOURCES: &[(&str, &str)] = &[\n");
    for path in entries {
        if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
            let content = fs::read_to_string(&path).unwrap_or_default();
            output.push_str("    (");
            output.push_str(&quote_str(file_name));
            output.push_str(", ");
            output.push_str(&quote_str(&content));
            output.push_str("),\n");
        }
    }
    output.push_str("];\n");

    fs::write(generated, output).expect("failed to write embedded_sources.rs");
}

fn quote_str(value: &str) -> String {
    format!("{value:?}")
}
