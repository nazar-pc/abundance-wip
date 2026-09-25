//! Build an ELF `cdylib` with the contract

use crate::target_specification::TARGET_SPECIFICATION_NAME;
use anyhow::Context;
use cargo_metadata::MetadataCommand;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Options for building a contract
#[derive(Debug)]
pub struct BuildOptions<'a> {
    /// Package to build.
    ///
    /// A package in the current directory is built if not specified explicitly.
    pub package: Option<&'a str>,
    /// Comma separated list of features to activate
    pub features: Option<&'a str>,
    /// Do not activate the `default` feature
    pub no_default_features: bool,
    /// Build artifacts with the specified profile
    pub profile: &'a str,
    /// Path to the target specification JSON file
    pub target_specification_path: &'a Path,
    /// Custom target directory to use instead of the default one
    pub target_dir: Option<&'a Path>,
}

/// Build a `cdylib` with the contract and return the path to the resulting ELF file
pub fn build_cdylib(options: BuildOptions<'_>) -> anyhow::Result<PathBuf> {
    let BuildOptions {
        package,
        features,
        no_default_features,
        profile,
        target_specification_path,
        target_dir,
    } = options;

    let mut command_builder = Command::new("cargo");
    command_builder
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        // Remove Clippy's wrapper in place that makes Cargo fingerprint contract artifacts
        // differently under `cargo clippy` than under `cargo build`, rebuilding all of them on
        // every switch between the two
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        // Hack for enabling RISC-V Zknh backend in `sha2` crate since it is a nightly-only feature,
        // and they really don't like using normal features for it.
        //
        // `RUSTFLAGS` is replaced rather than extended because whatever the host build is using
        // does not apply to a guest built for a different target, so `AB_EXTRA_RUSTFLAGS` is the
        // way to pass flags to the guest specifically. It exists for sweeping code generation
        // options against the interpreter benchmarks, see
        // `specs/discussions/riscv-interpreter-measurements.md`.
        .env(
            "RUSTFLAGS",
            format!(
                r#"--cfg sha2_backend="riscv-zknh" --cfg sha2_backend_riscv_zknh="compact" {}"#,
                env::var("AB_EXTRA_RUSTFLAGS").unwrap_or_default()
            ),
        )
        .args([
            "rustc",
            "-Zbuild-std=core",
            "--crate-type",
            "cdylib",
            "-Zjson-target-spec",
            "--target",
            target_specification_path
                .to_str()
                .context("Path to target specification file is not valid UTF-8")?,
        ]);

    if env::var("MIRI_SYSROOT").is_ok() {
        command_builder
            .env_remove("RUSTC")
            .env_remove("RUSTC_WRAPPER");
    }

    // Build the guest with a different compiler than the host, which is how a patched LLVM gets
    // exercised on contract code, see `specs/discussions/riscv-interpreter-measurements.md`.
    //
    // Setting `RUSTUP_TOOLCHAIN` alone does nothing here. Cargo passes `RUSTC` to build scripts as
    // an absolute path into the host toolchain rather than as the `rustup` shim, and the nested
    // Cargo honors that variable, so the override is inherited straight past and the guest is built
    // by the host compiler with nothing to indicate it. The same applies to `RUSTDOC`, and
    // `LD_LIBRARY_PATH` points at the host toolchain's library directory.
    if let Ok(toolchain) = env::var("AB_GUEST_TOOLCHAIN") {
        command_builder
            .env_remove("RUSTC")
            .env_remove("RUSTDOC")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("LD_LIBRARY_PATH");

        // An absolute path is taken as the compiler itself, anything else as a `rustup` toolchain
        // name, so both `~/rustc-interp/rust/build/host/stage2/bin/rustc` and a linked toolchain
        // work.
        if Path::new(&toolchain).is_absolute() {
            command_builder.env("RUSTC", &toolchain);
        } else {
            command_builder.env("RUSTUP_TOOLCHAIN", &toolchain);
        }
    }

    if let Some(package) = package {
        command_builder.args([
            "--package",
            package,
            "--features",
            &format!("{package}/guest"),
        ]);
    } else {
        command_builder.args(["--features", "guest"]);
    }
    if let Some(features) = features {
        command_builder.args(["--features", features]);
    }
    if no_default_features {
        command_builder.arg("--no-default-features");
    }

    command_builder.args(["--profile", profile]);

    let metadata = MetadataCommand::new()
        .exec()
        .context("Failed to fetch cargo metadata")?;

    let target_directory = if let Some(target_dir) = target_dir {
        command_builder.args([
            "--target-dir",
            target_dir
                .to_str()
                .context("Path to target directory is not valid UTF-8")?,
        ]);
        target_dir
    } else {
        metadata.target_directory.as_std_path()
    };

    let cdylib_path = target_directory
        .join(TARGET_SPECIFICATION_NAME)
        .join(if profile == "dev" { "debug" } else { profile })
        .join({
            let package_name = if let Some(package) = package {
                package
            } else {
                let current_dir = env::current_dir().context("Failed to get current directory")?;
                let current_manifest = current_dir.join("Cargo.toml");
                metadata
                    .packages
                    .iter()
                    .find_map(|package| {
                        if package.manifest_path == current_manifest {
                            Some(&package.name)
                        } else {
                            None
                        }
                    })
                    .context("Failed to find package name")?
            };

            format!("{}.contract.so", package_name.replace('-', "_"))
        });

    debug!(
        ?package,
        ?features,
        ?profile,
        ?target_specification_path,
        cdylib_path = ?cdylib_path,
        command = ?command_builder,
        "Building ELF `cdylib` contract"
    );

    let status = command_builder
        .status()
        .context("Failed to build a contract")?;

    if !status.success() {
        return Err(anyhow::anyhow!("Failed to build a contract"));
    }

    Ok(cdylib_path)
}
