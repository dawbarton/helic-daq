//! Build script driving the shared identity and linker-layout helpers.
//!
//! An out-of-tree rig gets both from `helic-fw-build` exactly as the in-tree
//! experiments do, which is what makes the emitted identity describe this
//! workspace rather than the platform checkout.

fn main() {
    helic_fw_build::emit_identity();
    helic_fw_build::emit_memory_x();
}
