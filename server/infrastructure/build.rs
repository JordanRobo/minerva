// `embed_migrations!` reads the shared migrations directory at compile time,
// so rebuild the crate when it changes.
fn main() {
    println!("cargo:rerun-if-changed=../migrations");
}
