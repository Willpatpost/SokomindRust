// `sqlx::migrate!` embeds the migrations at compile time; without this, a
// migration-only change leaves a stale binary.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
