fn main() {
    for (env, default) in [
        ("COORDINATOR_JAR", "../device/build/coordinator-server.jar"),
        (
            "COORDINATOR_SO_X86_64",
            "../device/build/libcoordinator-x86_64.so",
        ),
        (
            "COORDINATOR_SO_AARCH64",
            "../device/build/libcoordinator-aarch64.so",
        ),
        ("SKILL_MD", "../md/SKILL.md"),
    ] {
        println!("cargo::rerun-if-env-changed={env}");
        let path = std::env::var(env).unwrap_or_else(|_| {
            let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            format!("{manifest}/{default}")
        });
        println!("cargo::rustc-env={env}={path}");
    }
}
