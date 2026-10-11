fn main() {
    // The release workflow passes the tag's timestamp (UTC, "YYYY-MM-DD HH:MM:SS") in this variable.
    println!("cargo:rerun-if-env-changed=SUBWOOFA_BUILD_STAMP");
}
