//! # Install the bundler gem
//!
//! ## Layer dir: Install bundler to disk
//!
//! Installs a copy of `bundler` to the `<layer-dir>` with a bundler executable in
//! `<layer-dir>/bin`. Must run before [`crate.steps.bundle_install`].
use crate::layers::shared::{cached_layer_write_metadata, MetadataDiff};
use crate::RubyBuildpack;
use crate::RubyBuildpackError;
use bullet_stream::state::SubBullet;
use bullet_stream::{style, Print};
use cache_diff::CacheDiff;
use commons::gemfile_lock::ResolvedBundlerVersion;
use fun_run::{self, CommandWithName};
use libcnb::data::layer_name;
use libcnb::layer::{EmptyLayerCause, LayerState};
use libcnb::layer_env::{LayerEnv, ModificationBehavior, Scope};
use libcnb::Env;
use magic_migrate::{try_migrate_deserializer_chain, TryMigrate};
use serde::{Deserialize, Deserializer, Serialize};
use std::convert::Infallible;
use std::io::Stdout;
use std::path::Path;
use std::process::Command;

pub(crate) fn handle(
    context: &libcnb::build::BuildContext<RubyBuildpack>,
    env: &Env,
    mut bullet: Print<SubBullet<Stdout>>,
    metadata: &Metadata,
) -> libcnb::Result<(Print<SubBullet<Stdout>>, LayerEnv), RubyBuildpackError> {
    let layer_ref = cached_layer_write_metadata(layer_name!("bundler"), context, metadata)?;
    match &layer_ref.state {
        LayerState::Restored { cause } => {
            bullet = bullet.sub_bullet(cause);
            Ok((bullet, layer_ref.read_env()?))
        }
        LayerState::Empty { cause } => {
            match cause {
                EmptyLayerCause::NewlyCreated => {}
                EmptyLayerCause::InvalidMetadataAction { cause }
                | EmptyLayerCause::RestoredLayerAction { cause } => {
                    bullet = bullet.sub_bullet(cause);
                }
            }
            let (bullet, layer_env) = download_bundler(bullet, env, metadata, &layer_ref.path())?;
            layer_ref.write_env(&layer_env)?;

            Ok((bullet, layer_ref.read_env()?))
        }
    }
}

pub(crate) type Metadata = MetadataV1;
try_migrate_deserializer_chain!(
    deserializer: toml::Deserializer::new,
    error: MetadataError,
    chain: [MetadataV1],
);

impl MetadataDiff for Metadata {
    fn diff(&self, other: &Self) -> Vec<String> {
        <Self as CacheDiff>::diff(self, other)
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, CacheDiff)]
pub(crate) struct MetadataV1 {
    #[cache_diff(rename = "Bundler version")]
    pub(crate) version: ResolvedBundlerVersion,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MetadataError {
    // Update if migrating between a metadata version can error
}

fn download_bundler(
    bullet: Print<SubBullet<Stdout>>,
    env: &Env,
    metadata: &Metadata,
    path: &Path,
) -> Result<(Print<SubBullet<Stdout>>, LayerEnv), RubyBuildpackError> {
    let bin_dir = path.join("bin");
    let gem_path = path;

    let mut cmd = Command::new("gem");
    cmd.args(["install", "bundler"]);
    cmd.args(["--version", &metadata.version.to_string()]) // Specify exact version to install
        .env_clear()
        .envs(env);

    let short_name = fun_run::display(&mut cmd); // Format `gem install --version <version>` without other content for display

    cmd.args(["--install-dir", &format!("{}", gem_path.display())]); // Directory where bundler's contents will live
    cmd.args(["--bindir", &format!("{}", bin_dir.display())]); // Directory where `bundle` executable lives
    cmd.args([
        "--force",       // Overwrite if it already exists
        "--no-document", // Don't install ri or rdoc documentation, which takes extra time
        "--env-shebang", // Start the `bundle` executable with `#! /usr/bin/env ruby`
    ]);

    let timer = bullet.start_timer(format!("Running {}", style::command(short_name)));

    cmd.named_output()
        .map_err(|error| fun_run::map_which_problem(error, cmd.mut_cmd(), env.get("PATH").cloned()))
        .map_err(RubyBuildpackError::GemInstallBundlerCommandError)?;

    let layer_env = LayerEnv::new()
        .chainable_insert(Scope::All, ModificationBehavior::Delimiter, "PATH", ":")
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Prepend,
            "PATH", // Ensure this path comes before default bundler that ships with ruby, don't rely on the lifecycle
            bin_dir,
        )
        .chainable_insert(Scope::All, ModificationBehavior::Delimiter, "GEM_PATH", ":")
        .chainable_insert(
            Scope::All,
            ModificationBehavior::Prepend,
            "GEM_PATH", // Bundler is a gem too, allow it to be required
            gem_path,
        );

    Ok((timer.done(), layer_env))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::layers::shared::strip_ansi;

    #[test]
    fn test_metadata_diff() {
        let old = Metadata {
            version: ResolvedBundlerVersion("2.3.5".to_string()),
        };
        assert!(CacheDiff::diff(&old, &old).is_empty());

        let diff = CacheDiff::diff(
            &Metadata {
                version: ResolvedBundlerVersion("2.3.6".to_string()),
            },
            &old,
        );
        assert_eq!(
            diff.iter().map(strip_ansi).collect::<Vec<String>>(),
            vec!["Bundler version (`2.3.5` to `2.3.6`)"]
        );
    }

    /// If this test fails due to a change you'll need to implement
    /// `migrate_incompatible_metadata` for the Layer trait
    #[test]
    fn metadata_guard() {
        let metadata = Metadata {
            version: ResolvedBundlerVersion(String::from("2.3.6")),
        };

        let actual = toml::to_string(&metadata).unwrap();
        let expected = r#"
version = "2.3.6"
"#
        .trim();
        assert_eq!(expected, actual.trim());
    }
}
