fn main() {
    println!("cargo:rerun-if-env-changed=HERAKLES_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=HERAKLES_GOOGLE_CLIENT_SECRET");
    tauri_build::build()
}
