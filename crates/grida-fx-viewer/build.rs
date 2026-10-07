use std::path::Path;

fn main() {
    let bundle = Path::new("../../web/viewer/dist");
    println!("cargo:rerun-if-changed={}", bundle.display());
    assert!(
        bundle.join("index.html").is_file(),
        "FX viewer assets are missing: build web/viewer before building the engine"
    );
}
