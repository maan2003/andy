use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::assets;

const DEVICE_DIR: &str = "/data/local/tests/coordinator";
const DEVICE_PORT: u16 = 21632;

pub fn start(socket_path: &Path) -> Result<()> {
    let device_dir = DEVICE_DIR.to_string();

    // Check that we're talking to a virtual device
    let is_virtual = adb_getprop("ro.hardware.virtual_device")? == "1"
        || adb_getprop("ro.kernel.qemu")? == "1";
    if !is_virtual {
        eprintln!("###########################################################");
        eprintln!("#  WARNING: This does not appear to be a virtual device!  #");
        eprintln!("#  Refusing to continue to protect physical devices.      #");
        eprintln!("###########################################################");
        bail!("connected device is not a virtual device");
    }

    let so_bytes = select_so()?;

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket dir {}", parent.display()))?;
    }
    let local_spec = format!("localfilesystem:{}", socket_path.display());
    let remote_spec = format!("tcp:{}", DEVICE_PORT);

    // Remove old forward so the socket file is recreated
    let _ = Command::new("adb")
        .args(["forward", "--remove", &local_spec])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let _ = Command::new("adb")
        .args(["shell", "pkill", "-9", "-f", "andy-coordinator"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let setup = format!("rm -rf {device_dir} && mkdir -p {device_dir}");
    run("adb", &["shell", &setup], "prepare device")?;

    push_bytes(
        assets::JAR,
        &format!("{}/coordinator-server.jar", device_dir),
        "push jar",
    )?;
    push_bytes(
        so_bytes,
        &format!("{}/libcoordinator.so", device_dir),
        "push .so",
    )?;

    run(
        "adb",
        &["forward", &local_spec, &remote_spec],
        "configure adb forward",
    )?;

    // Start coordinator — device side spawns daemon and exits.
    // The polling loop in ensure_server waits for it to become ready.
    let classpath = format!("{device_dir}/coordinator-server.jar");
    let lib_path = format!("{device_dir}/libcoordinator.so");
    run(
        "adb",
        &[
            "shell",
            "env",
            &format!("CLASSPATH={classpath}"),
            &format!("ANDY_LIB={lib_path}"),
            "app_process",
            "/system/bin",
            "com.coordinator.Main",
        ],
        "start coordinator",
    )?;

    eprintln!("debug: andy server started");
    Ok(())
}

fn push_bytes(bytes: &[u8], device_path: &str, label: &str) -> Result<()> {
    let mut child = Command::new("adb")
        .args(["exec-in", &format!("cat > {device_path}")])
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("{label}: failed to spawn adb exec-in"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bytes)
        .with_context(|| format!("{label}: failed to write bytes"))?;
    let status = child
        .wait()
        .with_context(|| format!("{label}: failed to wait for adb exec-in"))?;
    if !status.success() {
        bail!("{label}: adb exec-in failed with status {status}");
    }
    Ok(())
}

fn select_so() -> Result<&'static [u8]> {
    let arch = device_arch()?;
    match arch.as_str() {
        "x86_64" => Ok(assets::SO_X86_64),
        "aarch64" => Ok(assets::SO_AARCH64),
        other => bail!("unsupported arch: {other}"),
    }
}

fn run(cmd: &str, args: &[&str], label: &str) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("{label}: failed to spawn {}", format_command(cmd, args)))?;
    if !status.success() {
        bail!(
            "{}: command failed with status {}: {}",
            label,
            status,
            format_command(cmd, args)
        );
    }
    Ok(())
}

fn adb_getprop(prop: &str) -> Result<String> {
    let output = Command::new("adb")
        .args(["shell", "getprop", prop])
        .output()
        .with_context(|| format!("failed to run adb shell getprop {prop}"))?;
    if !output.status.success() {
        bail!("adb shell getprop {prop} failed (is a device connected?)");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn device_arch() -> Result<String> {
    let abi = adb_getprop("ro.product.cpu.abi")?;
    match abi.as_str() {
        "x86_64" => Ok("x86_64".into()),
        "arm64-v8a" => Ok("aarch64".into()),
        other => bail!("unsupported device ABI: {other}"),
    }
}

fn format_command(cmd: &str, args: &[&str]) -> String {
    let mut out = String::from(cmd);
    for arg in args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}
