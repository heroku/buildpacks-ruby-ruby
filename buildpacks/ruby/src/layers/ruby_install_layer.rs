use crate::{build_output, RubyBuildpack, RubyBuildpackError};
use commons::gemfile_lock::ResolvedRubyVersion;
use flate2::read::GzDecoder;
use libcnb::build::BuildContext;
use libcnb::data::buildpack::StackId;
use libcnb::data::layer_content_metadata::LayerTypes;
use libcnb::layer::{ExistingLayerStrategy, Layer, LayerData, LayerResult, LayerResultBuilder};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use tar::Archive;
use tempfile::NamedTempFile;
use url::Url;

/// # Install Ruby version
///
/// ## Layer dir
///
/// The compiled Ruby tgz file is downloaded to a temporary directory and exported to `<layer-dir>`.
/// The tgz already contains a `bin/` directory with a `ruby` executable file.
///
/// This layer relies on the CNB lifecycle to add `<layer-dir>/bin` to the PATH.
///
/// ## Cache invalidation
///
/// When the Ruby version changes, invalidate and re-run.
///
#[derive(PartialEq, Eq)]
pub(crate) struct RubyInstallLayer {
    pub version: ResolvedRubyVersion,
    pub build_output: build_output::Section,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct RubyInstallLayerMetadata {
    pub stack: StackId,
    pub version: ResolvedRubyVersion,
}

impl Layer for RubyInstallLayer {
    type Buildpack = RubyBuildpack;
    type Metadata = RubyInstallLayerMetadata;

    fn types(&self) -> LayerTypes {
        LayerTypes {
            build: true,
            launch: true,
            cache: true,
        }
    }

    fn create(
        &self,
        context: &BuildContext<Self::Buildpack>,
        layer_path: &Path,
    ) -> Result<LayerResult<Self::Metadata>, RubyBuildpackError> {
        let mut timer = self.build_output.say_with_inline_timer("Installing");

        let tmp_ruby_tgz = NamedTempFile::new()
            .map_err(RubyInstallError::CouldNotCreateDestinationFile)
            .map_err(RubyBuildpackError::RubyInstallError)?;

        let url = download_url(&context.stack_id, &self.version);

        download(url.as_ref(), tmp_ruby_tgz.path())
            .map_err(RubyBuildpackError::RubyInstallError)?;

        untar(tmp_ruby_tgz.path(), layer_path).map_err(RubyBuildpackError::RubyInstallError)?;

        timer.done();

        LayerResultBuilder::new(RubyInstallLayerMetadata {
            stack: context.stack_id.clone(),
            version: self.version.clone(),
        })
        .build()
    }

    fn existing_layer_strategy(
        &self,
        context: &BuildContext<Self::Buildpack>,
        layer_data: &LayerData<Self::Metadata>,
    ) -> Result<ExistingLayerStrategy, RubyBuildpackError> {
        let old = &layer_data.content_metadata.metadata;
        let now = RubyInstallLayerMetadata {
            stack: context.stack_id.clone(),
            version: self.version.clone(),
        };

        match cache_state(old.clone(), now) {
            Changed::Nothing(_version) => {
                self.build_output.say("Using cached version");

                Ok(ExistingLayerStrategy::Keep)
            }
            Changed::Stack(_old, _now) => {
                self.build_output
                    .say_with_details("Clearing cache", "stack changed");

                Ok(ExistingLayerStrategy::Recreate)
            }
            Changed::RubyVersion(_old, _now) => {
                self.build_output
                    .say_with_details("Clearing cache", "ruby version changed");

                Ok(ExistingLayerStrategy::Recreate)
            }
        }
    }
}

fn cache_state(old: RubyInstallLayerMetadata, now: RubyInstallLayerMetadata) -> Changed {
    let RubyInstallLayerMetadata { stack, version } = now;

    if old.stack != stack {
        Changed::Stack(old.stack, stack)
    } else if old.version != version {
        Changed::RubyVersion(old.version, version)
    } else {
        Changed::Nothing(version)
    }
}

#[derive(Debug)]
enum Changed {
    Nothing(ResolvedRubyVersion),
    Stack(StackId, StackId),
    RubyVersion(ResolvedRubyVersion, ResolvedRubyVersion),
}

fn download_url(stack: &StackId, version: impl std::fmt::Display) -> Url {
    let mut url = Url::parse("https://heroku-buildpack-ruby.s3.us-east-1.amazonaws.com")
        .expect("Internal error: bad url");

    url.path_segments_mut()
        .expect("Internal error: bad url")
        .push(stack)
        .push(&format!("ruby-{version}.tgz"));

    url
}

pub(crate) fn download(
    uri: impl AsRef<str>,
    destination: impl AsRef<Path>,
) -> Result<(), RubyInstallError> {
    let mut response_reader = ureq::get(uri.as_ref())
        .call()
        .map_err(|err| RubyInstallError::RequestError(Box::new(err)))?
        .into_reader();

    let mut destination_file = fs_err::File::create(destination.as_ref())
        .map_err(RubyInstallError::CouldNotCreateDestinationFile)?;

    io::copy(&mut response_reader, &mut destination_file)
        .map_err(RubyInstallError::CouldNotWriteDestinationFile)?;

    Ok(())
}

pub(crate) fn untar(
    path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<(), RubyInstallError> {
    let file = fs_err::File::open(path.as_ref()).map_err(RubyInstallError::CouldNotOpenTarFile)?;

    Archive::new(GzDecoder::new(file))
        .unpack(destination.as_ref())
        .map_err(RubyInstallError::CouldNotUnpack)
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum RubyInstallError {
    #[error("Could not open downloaded tar file: {0}")]
    CouldNotOpenTarFile(std::io::Error),

    #[error("Could not untar downloaded file: {0}")]
    CouldNotUnpack(std::io::Error),

    // Boxed to prevent `large_enum_variant` errors since `ureq::Error` is massive.
    #[error("Download error: {0}")]
    RequestError(Box<ureq::Error>),

    #[error("Could not create file: {0}")]
    CouldNotCreateDestinationFile(std::io::Error),

    #[error("Could not write file: {0}")]
    CouldNotWriteDestinationFile(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use libcnb::data::stack_id;

    /// If this test fails due to a change you'll need to implement
    /// `migrate_incompatible_metadata` for the Layer trait
    #[test]
    fn metadata_guard() {
        let metadata = RubyInstallLayerMetadata {
            stack: stack_id!("heroku-22"),
            version: ResolvedRubyVersion(String::from("3.1.3")),
        };

        let actual = toml::to_string(&metadata).unwrap();
        let expected = r#"
stack = "heroku-22"
version = "3.1.3"
"#
        .trim();
        assert_eq!(expected, actual.trim());
    }

    #[test]
    fn test_ruby_url() {
        let out = download_url(&stack_id!("heroku-20"), "2.7.4");
        assert_eq!(
            out.as_ref(),
            "https://heroku-buildpack-ruby.s3.us-east-1.amazonaws.com/heroku-20/ruby-2.7.4.tgz",
        );
    }
}
