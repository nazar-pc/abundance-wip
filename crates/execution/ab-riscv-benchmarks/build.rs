#[cfg(not(feature = "build-contract"))]
fn main() {}

#[cfg(feature = "build-contract")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use ab_contract_file::ContractFile;
    use ab_contracts_tooling::build::{BuildOptions, build_cdylib};
    use ab_contracts_tooling::convert::convert;
    use ab_contracts_tooling::target_specification::TargetSpecification;
    use std::env;
    use std::fs::{read, write};
    use std::path::PathBuf;

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("Always set by Cargo; qed"));

    let target_specification = TargetSpecification::create(&out_dir)?;

    // Printed so that a run which meant to override them cannot quietly turn out not to have.
    if target_specification.is_overridden() {
        println!(
            "cargo::warning=guest built with AB_GUEST_CPU={:?} AB_GUEST_FEATURES={:?}",
            env::var("AB_GUEST_CPU").unwrap_or_default(),
            env::var("AB_GUEST_FEATURES").unwrap_or_default(),
        );
    }

    let cdylib_path = build_cdylib(BuildOptions {
        package: None,
        features: None,
        // The nested invocation compiles this build script again, and without `build-contract` it
        // does not need any of the build dependencies, which would otherwise be compiled a second
        // time for the `contract` profile
        no_default_features: true,
        profile: "contract",
        target_specification_path: target_specification.path(),
        target_dir: None,
    })?;

    let contract_path = cdylib_path.with_extension("");

    let input_bytes = read(cdylib_path)?;
    let output_bytes = convert(&input_bytes)?;
    ContractFile::parse(&output_bytes, |_| Ok(()))?;
    write(&contract_path, output_bytes)?;

    println!("cargo::rustc-env=CONTRACT_PATH={}", contract_path.display());

    // Without this the contract is silently reused when only the guest's code generation options
    // changed, which makes a sweep report the same numbers for every configuration
    println!("cargo::rerun-if-env-changed=AB_EXTRA_RUSTFLAGS");
    println!("cargo::rerun-if-env-changed=AB_GUEST_CPU");
    println!("cargo::rerun-if-env-changed=AB_GUEST_FEATURES");
    println!("cargo::rerun-if-env-changed=AB_GUEST_TOOLCHAIN");

    Ok(())
}
