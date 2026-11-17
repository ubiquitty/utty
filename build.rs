// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn run(command: &mut Command) {
    let status = command.status().expect("start native build command");
    assert!(status.success(), "native build failed: {command:?}");
}

fn configure() -> Command {
    let mut command = Command::new("cmake");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        command
            .env("CC", "clang")
            .env("CXX", "clang++")
            .arg("-DCMAKE_CXX_FLAGS=-stdlib=libc++");
    }
    command
}

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = if env::var("PROFILE").unwrap() == "release" {
        "Release"
    } else {
        "Debug"
    };
    let jobs = env::var("NUM_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 4)
        .to_string();
    let sdk = env::var_os("UBIQUITTY_CORE_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| out.join("sdk"));
    if env::var_os("UBIQUITTY_CORE_SDK").is_none() {
        let core = root.join("ThirdParty/ubiquitty-core");
        assert!(
            core.join("CMakeLists.txt").exists(),
            "run git submodule update --init first"
        );
        run(configure()
            .arg("-S")
            .arg(&core)
            .arg("-B")
            .arg(out.join("core"))
            .args([
                "-G",
                "Ninja",
                "-DUBIQUITTY_FRONTEND=Core",
                "-DUBIQUITTY_QUIC=OFF",
            ])
            .arg(format!("-DCMAKE_BUILD_TYPE={profile}"))
            .arg("-DCMAKE_INSTALL_LIBDIR=lib")
            .arg(format!("-DCMAKE_INSTALL_PREFIX={}", sdk.display())));
        run(Command::new("cmake")
            .arg("--build")
            .arg(out.join("core"))
            .args(["--target", "ubiquitty-kit", "-j", &jobs]));
        run(Command::new("cmake").arg("--install").arg(out.join("core")));
    }
    let native = out.join("native");
    run(configure()
        .arg("-S")
        .arg(root.join("native"))
        .arg("-B")
        .arg(&native)
        .args(["-G", "Ninja"])
        .arg(format!("-DCMAKE_BUILD_TYPE={profile}"))
        .arg(format!(
            "-DUbiquittyCore_DIR={}/lib/cmake/UbiquittyCore",
            sdk.display()
        )));
    run(Command::new("cmake")
        .arg("--build")
        .arg(&native)
        .args(["--target", "tui-core", "-j", &jobs]));
    // CMake resolves the exported Core targets; Rust consumes their archive paths.
    // A linker group resolves the POSIX backend's circular static links on ELF.
    let apple = env::var("CARGO_CFG_TARGET_VENDOR").unwrap() == "apple";
    if !apple {
        println!("cargo:rustc-link-arg=-Wl,--start-group");
    }
    for archive in fs::read_to_string(native.join("rust-archives.txt"))
        .unwrap()
        .lines()
    {
        println!("cargo:rustc-link-arg={archive}");
    }
    if !apple {
        println!("cargo:rustc-link-arg=-Wl,--end-group");
    }
    println!("cargo:rustc-link-arg=-lc++");
    if !apple {
        println!("cargo:rustc-link-arg=-lutil");
    }
    for path in ["native", "ThirdParty/ubiquitty-core", "build.rs"] {
        println!("cargo:rerun-if-changed={}", Path::new(path).display());
    }
    println!("cargo:rerun-if-env-changed=UBIQUITTY_CORE_SDK");
}
