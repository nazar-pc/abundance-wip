//! Abundance target specification for contracts

use anyhow::Context;
use dirs::cache_dir;
use std::borrow::Cow;
use std::env;
use std::fs::{File, create_dir_all};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

pub(crate) const TARGET_SPECIFICATION_NAME: &str = "riscv64-unknown-none-abundance";
const TARGET_SPECIFICATION_FILE_NAME: &str = "riscv64-unknown-none-abundance.json";
const TARGET_SPECIFICATION: &str = include_str!("riscv64-unknown-none-abundance.json");
const DEFAULT_CPU: &str = r#""cpu": "generic-rv64","#;
const FEATURES_PREFIX: &str = r#""features": ""#;

/// The target specification to write out, with the LLVM CPU and subtarget features overridden if
/// `AB_GUEST_CPU` and `AB_GUEST_FEATURES` ask for it.
///
/// The CPU decides which scheduling model and cost model LLVM applies to the guest, and the
/// features decide the tuning, so overriding them separately is what tells the two apart. Both are
/// measurement knobs and have to work without editing a checked-in file. `AB_GUEST_FEATURES` is a
/// comma separated list applied to the specification's own, each entry replacing whatever the list
/// says about the same feature, so `-interpreter-target` subtracts and `+interpreter-target` adds.
/// It goes to LLVM as written, without passing through rustc's own notion of which target features
/// exist, which is the point. See `specs/discussions/riscv-interpreter-measurements.md`.
fn target_specification() -> anyhow::Result<Cow<'static, str>> {
    override_target_specification(
        env::var("AB_GUEST_CPU").ok(),
        env::var("AB_GUEST_FEATURES").ok(),
    )
}

/// [`target_specification()`] with the overrides passed in rather than read from the environment
fn override_target_specification(
    cpu: Option<String>,
    features: Option<String>,
) -> anyhow::Result<Cow<'static, str>> {
    if cpu.is_none() && features.is_none() {
        return Ok(Cow::Borrowed(TARGET_SPECIFICATION));
    }

    let mut specification = TARGET_SPECIFICATION.to_string();

    if let Some(cpu) = cpu {
        anyhow::ensure!(
            specification.contains(DEFAULT_CPU),
            "Target specification says `{}` rather than `{DEFAULT_CPU}`, so `AB_GUEST_CPU` has \
            nothing to replace. It is most likely edited by hand; restore it with `git checkout` \
            and pass the CPU through this variable instead",
            specification
                .lines()
                .find(|line| line.contains(r#""cpu""#))
                .unwrap_or("<no cpu at all>")
                .trim()
        );
        specification = specification.replace(DEFAULT_CPU, &format!(r#""cpu": "{cpu}","#));
    }

    if let Some(overrides) = features {
        let start = specification.find(FEATURES_PREFIX).with_context(|| {
            format!(
                "Target specification no longer contains `{FEATURES_PREFIX}`, so \
                    `AB_GUEST_FEATURES` cannot extend it"
            )
        })? + FEATURES_PREFIX.len();
        let end = start
            + specification
                .get(start..)
                .and_then(|rest| rest.find('"'))
                .context("Unterminated `features` in the target specification")?;
        let mut list = specification
            .get(start..end)
            .context("`features` in the target specification is not valid UTF-8")?
            .split(',')
            .filter(|feature| !feature.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        // Each override replaces the list's own entry for that feature rather than following it:
        // rustc rejects a specification that both enables and disables a feature, so appending
        // cannot subtract anything the list turns on.
        for feature in overrides
            .split(',')
            .map(str::trim)
            .filter(|feature| !feature.is_empty())
        {
            let name = feature.strip_prefix(['+', '-']).with_context(|| {
                format!("`{feature}` in `AB_GUEST_FEATURES` must start with `+` or `-`")
            })?;
            list.retain(|existing| existing.get(1..) != Some(name));
            list.push(feature.to_string());
        }
        specification.replace_range(start..end, &list.join(","));
    }

    Ok(Cow::Owned(specification))
}

/// Target specification for contracts
#[derive(Debug)]
pub struct TargetSpecification {
    path: PathBuf,
    overridden: bool,
    _file: File,
}

impl TargetSpecification {
    /// Create a target specification instance.
    ///
    /// `base_directory` is used to store the target specification JSON file.
    pub fn create(base_directory: &Path) -> anyhow::Result<Self> {
        let target_specification = target_specification()?;
        let overridden = matches!(target_specification, Cow::Owned(_));
        let path = base_directory.join(TARGET_SPECIFICATION_FILE_NAME);
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .context("Failed to open target specification file")?;

        // Ensure the target specification file has expected content
        loop {
            file.lock_shared()
                .context("Failed to lock target specification file")?;

            let mut actual_target_specification = String::with_capacity(target_specification.len());
            file.seek(std::io::SeekFrom::Start(0))
                .context("Failed to seek to start of target specification file")?;
            file.read_to_string(&mut actual_target_specification)
                .context("Failed to read target specification file")?;

            if actual_target_specification == target_specification.as_ref() {
                break;
            }

            file.unlock()
                .context("Failed to unlock target specification file")?;
            file.lock()
                .context("Failed to lock target specification file")?;
            file.set_len(0)
                .context("Failed to truncate target specification file")?;
            file.seek(std::io::SeekFrom::Start(0))
                .context("Failed to seek to start of target specification file")?;
            file.write_all(target_specification.as_bytes())
                .context("Failed to write target specification file")?;
            file.sync_all()
                .context("Failed to sync target specification file")?;
            file.unlock()
                .context("Failed to unlock target specification file")?;
        }

        Ok(Self {
            path,
            overridden,
            _file: file,
        })
    }

    /// Create (if not exists) and return the default base directory used for storing the target
    /// specifications JSON file
    pub fn default_base_dir() -> anyhow::Result<PathBuf> {
        let app_dir = cache_dir()
            .context("Failed to get cache directory")?
            .join("ab-contracts");
        create_dir_all(&app_dir)
            .with_context(|| format!("Failed to create cache directory {}", app_dir.display()))?;

        Ok(app_dir)
    }

    /// Get the path to the target specification JSON file
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether `AB_GUEST_CPU` or `AB_GUEST_FEATURES` changed the specification.
    ///
    /// A checkout that predates those variables ignores them without saying so, and a measurement
    /// made that way silently compares a build against itself, so the caller is expected to report
    /// this.
    pub fn is_overridden(&self) -> bool {
        self.overridden
    }
}
