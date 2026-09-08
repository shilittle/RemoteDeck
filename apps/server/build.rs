fn main() {
    // A new hashed asset filename must invalidate the embedded release binary,
    // even when none of the Rust source files changed.
    println!("cargo:rerun-if-changed=../web/dist");
    assert!(
        std::path::Path::new("../web/dist/index.html").is_file(),
        "Build the browser assets first: pnpm --filter @remotedeck/web build"
    );
}
