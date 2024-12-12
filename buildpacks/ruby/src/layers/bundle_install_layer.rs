//! Installs Ruby gems (libraries) via `bundle install`
//!
//! Creates the cache where gems live. We want 'bundle install'
//! to execute on every build (as opposed to only when the cache is empty).
//!
//! As a small performance optimization, it will not run if the `Gemfile.lock`,
//! `Gemfile`, or user provided "platform" environment variable have not changed.
//! User applications can opt out of this behavior by setting the environment
//! variable `HEROKU_SKIP_BUNDLE_DIGEST=1`. That would be useful if the application's
//! `Gemfile` sources logic or data from another file that is unknown to the buildpack.
//!
//! Gems can be plain Ruby code which are OS, Architecture, and Ruby version independent.
//! They can also be native extensions that use Ruby's C API or contain libraries that
//! must be compiled and will then be invoked via FFI. These native extensions are
//! OS, Architecture, and Ruby version dependent. Due to this, when one of these changes
//! we must clear the cache and re-run `bundle install`.
use crate::layers::shared::{cached_layer_write_metadata, Meta, MetadataDiff};
use crate::target_id::{TargetId, TargetIdError};
use crate::{BundleWithout, RubyBuildpack, RubyBuildpackError};
use bullet_stream::state::SubBullet;
use bullet_stream::{style, Print};
use commons::{
    display::SentenceList, gemfile_lock::ResolvedRubyVersion, metadata_digest::MetadataDigest,
};
use fun_run::{self, CommandWithName};
use libcnb::data::layer_name;
use libcnb::layer::{EmptyLayerCause, LayerState};
use libcnb::{
    layer_env::{LayerEnv, ModificationBehavior, Scope},
    Env,
};
use magic_migrate::{try_migrate_deserializer_chain, TryMigrate};
use serde::{Deserialize, Deserializer, Serialize};
use std::convert::Infallible;
use std::io::Stdout;
use std::{path::Path, process::Command};

/// When this environment variable is set, the `bundle install` command will always
/// run regardless of whether the `Gemfile`, `Gemfile.lock`, or platform environment
/// variables have changed.
const SKIP_DIGEST_ENV_KEY: &str = "HEROKU_SKIP_BUNDLE_DIGEST";
/// A failsafe, if a programmer made a mistake in the caching logic, rev-ing this
/// key will force a re-run of `bundle install` to ensure the cache is correct
/// on the next build.
pub(crate) const FORCE_BUNDLE_INSTALL_CACHE_KEY: &str = "v1";

pub(crate) fn handle(
    context: &libcnb::build::BuildContext<RubyBuildpack>,
    env: &Env,
    mut bullet: Print<SubBullet<Stdout>>,
    metadata: &Metadata,
    without: &BundleWithout,
) -> libcnb::Result<(Print<SubBullet<Stdout>>, LayerEnv), RubyBuildpackError> {
    let layer_ref = cached_layer_write_metadata(layer_name!("gems"), context, metadata)?;
    let install_state = match &layer_ref.state {
        LayerState::Restored { cause } => {
            bullet = bullet.sub_bullet(cause);
            match cause {
                Meta::Data(old) => install_state(old, metadata),
                Meta::Message(_) => InstallState::Run(String::new()),
            }
        }
        LayerState::Empty { cause } => match cause {
            EmptyLayerCause::NewlyCreated => InstallState::Run(String::new()),
            EmptyLayerCause::InvalidMetadataAction { cause }
            | EmptyLayerCause::RestoredLayerAction { cause } => {
                bullet = bullet.sub_bullet(cause);
                InstallState::Run(String::new())
            }
        },
    };

    let env = {
        let layer_env = layer_env(&layer_ref.path(), &context.app_dir, without);
        layer_ref.write_env(&layer_env)?;
        layer_env.apply(Scope::Build, env)
    };

    match install_state {
        InstallState::Run(reason) => {
            if !reason.is_empty() {
                bullet = bullet.sub_bullet(reason);
            }

            let mut cmd = Command::new("bundle");
            cmd.args(["install"])
                .env_clear() // Current process env vars already merged into env
                .envs(&env);
            let mut cmd = cmd.named_fn(|cmd| display_name(cmd, &env));
            bullet
                .stream_with(
                    format!("Running {}", style::command(cmd.name())),
                    |stdout, stderr| cmd.stream_output(stdout, stderr),
                )
                .map_err(|error| {
                    fun_run::map_which_problem(error, cmd.mut_cmd(), env.get("PATH").cloned())
                })
                .map_err(RubyBuildpackError::BundleInstallCommandError)?;
        }
        InstallState::Skip(checked) => {
            let bundle_install = style::value("bundle install");
            let help = style::important("HELP");

            bullet = bullet
                .sub_bullet(format!(
                    "Skipping {bundle_install} (no changes found in {sources})",
                    sources = SentenceList::new(&checked).join_str("or")
                ))
                .sub_bullet(format!(
                    "{help} To force run {bundle_install} set {}",
                    style::value(format!("{SKIP_DIGEST_ENV_KEY}=1"))
                ));
        }
    }

    Ok((bullet, layer_ref.read_env()?))
}

pub(crate) type Metadata = MetadataV2;
try_migrate_deserializer_chain!(
    chain: [MetadataV1, MetadataV2, MetadataV3],
    error: MetadataMigrateError,
    deserializer: toml::Deserializer::new,
);

impl MetadataDiff for Metadata {
    fn diff(&self, old: &Self) -> Vec<String> {
        let mut differences = Vec::new();
        let Metadata {
            distro_name,
            distro_version,
            cpu_architecture,
            ruby_version,
            force_bundle_install_key: _,
            digest: _,
        } = old;

        if ruby_version != &self.ruby_version {
            differences.push(format!(
                "Ruby version ({old} to {now})",
                old = style::value(ruby_version.to_string()),
                now = style::value(self.ruby_version.to_string())
            ));
        }
        if distro_name != &self.distro_name || distro_version != &self.distro_version {
            differences.push(format!(
                "Distribution ({old} to {now})",
                old = style::value(format!("{distro_name} {distro_version}")),
                now = style::value(format!("{} {}", self.distro_name, self.distro_version))
            ));
        }
        if cpu_architecture != &self.cpu_architecture {
            differences.push(format!(
                "CPU architecture ({old} to {now})",
                old = style::value(cpu_architecture),
                now = style::value(&self.cpu_architecture)
            ));
        }

        differences
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
pub(crate) struct MetadataV1 {
    pub(crate) stack: String,
    pub(crate) ruby_version: ResolvedRubyVersion,
    pub(crate) force_bundle_install_key: String,
    pub(crate) digest: MetadataDigest, // Must be last for serde to be happy https://github.com/toml-rs/toml-rs/issues/142
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
pub(crate) struct MetadataV2 {
    pub(crate) distro_name: String,
    pub(crate) distro_version: String,
    pub(crate) cpu_architecture: String,
    pub(crate) ruby_version: ResolvedRubyVersion,
    pub(crate) force_bundle_install_key: String,
    pub(crate) digest: MetadataDigest, // Must be last for serde to be happy https://github.com/toml-rs/toml-rs/issues/142
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
pub(crate) struct MetadataV3 {
    pub(crate) os_distribution: String,
    pub(crate) cpu_architecture: String,
    pub(crate) ruby_version: ResolvedRubyVersion,
    pub(crate) force_bundle_install_key: String,

    /// A struct that holds the cryptographic hash of components that can
    /// affect the result of `bundle install`. When these values do not
    /// change between deployments we can skip re-running `bundle install` since
    /// the outcome should not change.
    ///
    /// While a fully resolved `bundle install` is relatively fast, it's not
    /// instantaneous. This check can save ~1 second on overall build time.
    ///
    /// This value is cached with metadata, so changing the struct
    /// may cause metadata to be invalidated (and the cache cleared).
    ///
    pub(crate) digest: MetadataDigest, // Must be last for serde to be happy https://github.com/toml-rs/toml-rs/issues/142
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum MetadataMigrateError {
    #[error("Could not migrate metadata {0}")]
    UnsupportedStack(TargetIdError),
}

// CNB spec moved from the concept of "stacks" (i.e. "heroku-22" which represented an OS and system dependencies) to finer
// grained "target" which includes the OS, OS version, and architecture. This function converts the old stack id to the new target id.
impl TryFrom<MetadataV1> for MetadataV2 {
    type Error = MetadataMigrateError;

    fn try_from(v1: MetadataV1) -> Result<Self, Self::Error> {
        let target_id =
            TargetId::from_stack(&v1.stack).map_err(MetadataMigrateError::UnsupportedStack)?;

        Ok(Self {
            distro_name: target_id.distro_name.clone(),
            distro_version: target_id.distro_version.clone(),
            cpu_architecture: target_id.cpu_architecture.clone(),
            ruby_version: v1.ruby_version,
            force_bundle_install_key: v1.force_bundle_install_key,
            digest: v1.digest,
        })
    }
}

impl TryFrom<MetadataV2> for MetadataV3 {
    type Error = Infallible;

    fn try_from(v2: MetadataV2) -> Result<Self, Self::Error> {
        Ok(Self {
            os_distribution: format!("{} {}", v2.distro_name, v2.distro_version),
            cpu_architecture: v2.cpu_architecture,
            ruby_version: v2.ruby_version,
            force_bundle_install_key: v2.force_bundle_install_key,
            digest: v2.digest,
        })
    }
}

#[derive(Debug)]
enum InstallState {
    /// Holds message indicating the reason why we want to run 'bundle install'
    Run(String),

    /// Do not run 'bundle install'
    Skip(Vec<String>),
}

/// Determines if 'bundle install' should execute on a given call to `BundleInstallLatyer::update`
///
fn install_state(old: &Metadata, now: &Metadata) -> InstallState {
    let forced_env = std::env::var_os(SKIP_DIGEST_ENV_KEY);
    let old_key = &old.force_bundle_install_key;
    let now_key = &now.force_bundle_install_key;

    if old_key != now_key {
        InstallState::Run(format!(
            "buildpack author triggered internal change {old_key} to {now_key}"
        ))
    } else if let Some(value) = forced_env {
        let value = value.to_string_lossy();

        InstallState::Run(format!("found {SKIP_DIGEST_ENV_KEY}={value}"))
    } else if let Some(changed) = now.digest.changed(&old.digest) {
        InstallState::Run(format!("{changed}"))
    } else {
        let checked = now.digest.checked_list();
        InstallState::Skip(checked)
    }
}

fn layer_env(layer_path: &Path, app_dir: &Path, without_default: &BundleWithout) -> LayerEnv {
    // CAREFUL: See environment variable warning below vvvvvvvvvv
    let layer_env = LayerEnv::new()
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Override,
            "BUNDLE_PATH", // Directs bundler to install gems to this path.
            layer_path,
        )
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Override,
            "BUNDLE_BIN", // Install executables for all gems into specified path.
            layer_path.join("bin"),
        )
        .chainable_insert(Scope::All, ModificationBehavior::Delimiter, "GEM_PATH", ":")
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Prepend,
            "GEM_PATH", // Tells Ruby where gems are located. Should match `BUNDLE_PATH`.
            layer_path,
        )
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Default,
            "BUNDLE_WITHOUT", // Do not install `development` or `test` groups via bundle install. Additional groups can be specified via user config.
            without_default.as_str(),
        )
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Override,
            "BUNDLE_GEMFILE", // Tells bundler where to find the `Gemfile`
            app_dir.join("Gemfile"),
        )
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Override,
            "BUNDLE_CLEAN", // After successful `bundle install` bundler will automatically run `bundle clean`
            "1",
        )
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Override,
            "BUNDLE_DEPLOYMENT", // Requires the `Gemfile.lock` to be in sync with the current `Gemfile`.
            "1",
        );
    // CAREFUL: Changes to these ^^^^^^^ environment variables
    //
    // Not every run is guaranteed to trigger a `bundle_install`
    // Rev the `force_bundle_install` cache key to ensure consistent
    // state (when appropriate).
    layer_env
}

/// Displays the `bundle install` command with `BUNDLE_` environment variables
/// that we use to configure bundler.
fn display_name(cmd: &mut Command, env: &Env) -> String {
    fun_run::display_with_env_keys(
        cmd,
        env,
        [
            "BUNDLE_BIN",
            "BUNDLE_CLEAN",
            "BUNDLE_DEPLOYMENT",
            "BUNDLE_GEMFILE",
            "BUNDLE_PATH",
            "BUNDLE_WITHOUT",
        ],
    )
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, Default)]
pub(crate) struct BundleDigest {
    env: String,
    gemfile: String,
    lockfile: String,
}

#[cfg(test)]
mod test {
    use crate::layers::shared::strip_ansi;

    use super::*;
    use std::path::PathBuf;

    /// `MetadataDiff` logic controls cache invalidation
    /// When the vec is empty the cache is kept, otherwise it is invalidated
    #[test]
    fn metadata_diff_messages() {
        let tmpdir = tempfile::tempdir().unwrap();
        let app_path = tmpdir.path().to_path_buf();
        let gemfile = app_path.join("Gemfile");
        let env = Env::new();
        let context = FakeContext {
            platform: FakePlatform { env },
            app_path,
        };
        std::fs::write(&gemfile, "iamagemfile").unwrap();

        let old = Metadata {
            ruby_version: ResolvedRubyVersion("3.5.3".to_string()),
            distro_name: "ubuntu".to_string(),
            distro_version: "20.04".to_string(),
            cpu_architecture: "amd64".to_string(),
            force_bundle_install_key: FORCE_BUNDLE_INSTALL_CACHE_KEY.to_string(),
            digest: MetadataDigest::new_env_files(
                &context.platform,
                &[&context.app_path.join("Gemfile")],
            )
            .unwrap(),
        };
        assert_eq!(old.diff(&old), Vec::<String>::new());

        let diff = Metadata {
            ruby_version: ResolvedRubyVersion("3.5.5".to_string()),
            distro_name: old.distro_name.clone(),
            distro_version: old.distro_version.clone(),
            cpu_architecture: old.cpu_architecture.clone(),
            force_bundle_install_key: old.force_bundle_install_key.clone(),
            digest: old.digest.clone(),
        }
        .diff(&old);
        assert_eq!(
            diff.iter().map(strip_ansi).collect::<Vec<String>>(),
            vec!["Ruby version (`3.5.3` to `3.5.5`)".to_string()]
        );

        let diff = Metadata {
            ruby_version: old.ruby_version.clone(),
            distro_name: "alpine".to_string(),
            distro_version: "3.20.0".to_string(),
            cpu_architecture: old.cpu_architecture.clone(),
            force_bundle_install_key: old.force_bundle_install_key.clone(),
            digest: old.digest.clone(),
        }
        .diff(&old);

        assert_eq!(
            diff.iter().map(strip_ansi).collect::<Vec<String>>(),
            vec!["Distribution (`ubuntu 20.04` to `alpine 3.20.0`)".to_string()]
        );

        let diff = Metadata {
            ruby_version: old.ruby_version.clone(),
            distro_name: old.distro_name.clone(),
            distro_version: old.distro_version.clone(),
            cpu_architecture: "arm64".to_string(),
            force_bundle_install_key: old.force_bundle_install_key.clone(),
            digest: old.digest.clone(),
        }
        .diff(&old);
        assert_eq!(
            diff.iter().map(strip_ansi).collect::<Vec<String>>(),
            vec!["CPU architecture (`amd64` to `arm64`)".to_string()]
        );
    }

    #[cfg(test)]
    #[derive(Default, Clone)]
    struct FakeContext {
        app_path: PathBuf,
        platform: FakePlatform,
    }

    #[cfg(test)]
    #[derive(Default, Clone)]
    struct FakePlatform {
        env: libcnb::Env,
    }

    impl libcnb::Platform for FakePlatform {
        fn env(&self) -> &Env {
            &self.env
        }

        fn from_path(_platform_dir: impl AsRef<Path>) -> std::io::Result<Self> {
            unimplemented!()
        }
    }

    /// If this test fails due to user change you may need
    /// to rev a cache key to force 'bundle install'
    /// to re-run otherwise it won't be picked up by
    /// anyone that is seeing `DiffState::Same`
    #[test]
    fn layer_env_change_keep_guard() {
        let layer_env = layer_env(
            &PathBuf::from("layer_path"),
            &PathBuf::from("app_path"),
            &BundleWithout(String::from("development:test")),
        );

        let env = layer_env.apply(Scope::All, &Env::new());

        let actual = commons::display::env_to_sorted_string(&env);
        let expected = r"
BUNDLE_BIN=layer_path/bin
BUNDLE_CLEAN=1
BUNDLE_DEPLOYMENT=1
BUNDLE_GEMFILE=app_path/Gemfile
BUNDLE_PATH=layer_path
BUNDLE_WITHOUT=development:test
GEM_PATH=layer_path
        ";
        assert_eq!(expected.trim(), actual.trim());
    }

    /// Guards the current metadata deserialization
    /// If this fails you need to implement a migration from the last format
    /// to the current format.
    #[test]
    fn metadata_guard() {
        let tmpdir = tempfile::tempdir().unwrap();
        let app_path = tmpdir.path().to_path_buf();
        let gemfile = app_path.join("Gemfile");

        let mut env = Env::new();
        env.insert("SECRET_KEY_BASE", "abcdgoldfish");

        let context = FakeContext {
            platform: FakePlatform { env },
            app_path,
        };
        std::fs::write(&gemfile, "iamagemfile").unwrap();

        let target_id = TargetId::from_stack("heroku-22").unwrap();
        let metadata = Metadata {
            distro_name: target_id.distro_name,
            distro_version: target_id.distro_version,
            cpu_architecture: target_id.cpu_architecture,
            ruby_version: ResolvedRubyVersion(String::from("3.1.3")),
            force_bundle_install_key: String::from("v1"),
            digest: MetadataDigest::new_env_files(
                &context.platform,
                &[&context.app_path.join("Gemfile")],
            )
            .unwrap(),
        };

        let actual = toml::to_string(&metadata).unwrap();
        let gemfile_path = gemfile.display();
        let toml_string = format!(
            r#"
distro_name = "ubuntu"
distro_version = "22.04"
cpu_architecture = "amd64"
ruby_version = "3.1.3"
force_bundle_install_key = "v1"

[digest]
platform_env = "c571543beaded525b7ee46ceb0b42c0fb7b9f6bfc3a211b3bbcfe6956b69ace3"

[digest.files]
"{gemfile_path}" = "32b27d2934db61b105fea7c2cb6159092fed6e121f8c72a948f341ab5afaa1ab"
"#
        )
        .trim()
        .to_string();
        assert_eq!(toml_string, actual.trim());

        let deserialized: Metadata = toml::from_str(&toml_string).unwrap();

        assert_eq!(metadata, deserialized);
    }

    #[test]
    fn metadata_migrate_v1_to_v2() {
        let tmpdir = tempfile::tempdir().unwrap();
        let app_path = tmpdir.path().to_path_buf();
        let gemfile = app_path.join("Gemfile");

        let mut env = Env::new();
        env.insert("SECRET_KEY_BASE", "abcdgoldfish");

        let context = FakeContext {
            platform: FakePlatform { env },
            app_path,
        };
        std::fs::write(&gemfile, "iamagemfile").unwrap();

        let metadata = MetadataV1 {
            stack: String::from("heroku-22"),
            ruby_version: ResolvedRubyVersion(String::from("3.1.3")),
            force_bundle_install_key: String::from("v1"),
            digest: MetadataDigest::new_env_files(
                &context.platform,
                &[&context.app_path.join("Gemfile")],
            )
            .unwrap(),
        };

        let actual = toml::to_string(&metadata).unwrap();
        let gemfile_path = gemfile.display();
        let toml_string = format!(
            r#"
stack = "heroku-22"
ruby_version = "3.1.3"
force_bundle_install_key = "v1"

[digest]
platform_env = "c571543beaded525b7ee46ceb0b42c0fb7b9f6bfc3a211b3bbcfe6956b69ace3"

[digest.files]
"{gemfile_path}" = "32b27d2934db61b105fea7c2cb6159092fed6e121f8c72a948f341ab5afaa1ab"
"#
        )
        .trim()
        .to_string();
        assert_eq!(toml_string, actual.trim());

        let deserialized: MetadataV2 = MetadataV2::try_from_str_migrations(&toml_string)
            .unwrap()
            .unwrap();

        let target_id = TargetId::from_stack(&metadata.stack).unwrap();
        let expected = MetadataV2 {
            distro_name: target_id.distro_name,
            distro_version: target_id.distro_version,
            cpu_architecture: target_id.cpu_architecture,
            ruby_version: metadata.ruby_version,
            force_bundle_install_key: metadata.force_bundle_install_key,
            digest: metadata.digest,
        };
        assert_eq!(expected, deserialized);
    }
}
