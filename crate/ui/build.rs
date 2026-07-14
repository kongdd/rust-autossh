//! Embeds `docs/autossh-tunnel.ico` into the PE and pre-rasterizes RGBA for the UI/tray.

use std::path::{Path, PathBuf};

const WINDOW_ICON_PX: u32 = 64;
const TRAY_ICON_PX: u32 = 32;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let icon = manifest_dir.join("../../docs/autossh-tunnel.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    rasterize_icons(&icon, &out_dir);

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let icon = icon
            .canonicalize()
            .unwrap_or_else(|_| icon.clone());
        let mut resources = winres::WindowsResource::new();
        configure_mingw_toolchain(&mut resources);
        resources.set_icon(icon.to_str().expect("icon path is not UTF-8"));
        resources
            .compile()
            .expect("cannot embed Windows icon resource");
        let resource_obj = out_dir.join("resource.o");
        println!("cargo:rustc-link-arg={}", resource_obj.display());
    }
}

fn rasterize_icons(ico: &Path, out_dir: &Path) {
    use image::imageops::FilterType;

    let bytes = std::fs::read(ico).expect("read autossh-tunnel.ico");
    for (size, name) in [
        (WINDOW_ICON_PX, "window_icon.bin"),
        (TRAY_ICON_PX, "tray_icon.bin"),
    ] {
        let image = image::load_from_memory(&bytes).expect("decode .ico in build.rs");
        let image = image.resize_to_fill(size, size, FilterType::Triangle);
        let rgba = image.to_rgba8().into_raw();
        let path = out_dir.join(name);
        std::fs::write(&path, &rgba).expect("write raster icon");
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

#[cfg(not(windows))]
fn configure_mingw_toolchain(resources: &mut winres::WindowsResource) {
    let target = std::env::var("TARGET").unwrap_or_default();
    let Some(prefix) = mingw_tool_prefix(&target) else {
        return;
    };
    resources.set_windres_path(&format!("{prefix}-windres"));
    resources.set_ar_path(&format!("{prefix}-ar"));
    if let Some(dir) = toolchain_bin_dir(&format!("{prefix}-windres")) {
        resources.set_toolkit_path(dir.to_str().expect("toolkit path is UTF-8"));
    }
}

#[cfg(windows)]
fn configure_mingw_toolchain(_resources: &mut winres::WindowsResource) {}

#[cfg(not(windows))]
fn mingw_tool_prefix(target: &str) -> Option<String> {
    if !target.ends_with("-pc-windows-gnu") {
        return None;
    }
    let arch = target.split('-').next()?;
    Some(format!("{arch}-w64-mingw32"))
}

#[cfg(not(windows))]
fn toolchain_bin_dir(tool: &str) -> Option<PathBuf> {
    let path = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .ok()?;
    if !path.status.success() {
        return None;
    }
    let stdout = String::from_utf8(path.stdout).ok()?;
    Path::new(stdout.trim()).parent().map(Path::to_path_buf)
}