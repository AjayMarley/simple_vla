// Compile the FlatBuffers schema at build time.
//
// Prerequisites:
//   apt install flatbuffers-compiler        # Debian/Ubuntu
//   brew install flatbuffers               # macOS
//   cargo install flatc                    # or via cargo
//
// The generated file lands in $OUT_DIR and is included by vision/src/lib.rs.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=schema/vision_frame.fbs");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set by cargo");

    let status = Command::new("flatc")
        .args(["--rust", "--gen-all", "-o", &out_dir, "schema/vision_frame.fbs"])
        .status()
        .expect(
            "flatc not found. \
             Install with: apt install flatbuffers-compiler \
             See: https://github.com/google/flatbuffers/releases",
        );

    assert!(status.success(), "flatc failed to compile vision_frame.fbs");
}
