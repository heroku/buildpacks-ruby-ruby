use crate::build_output;
use crate::{MetricsAgentBuildpack, MetricsAgentError};
use flate2::read::GzDecoder;
use libcnb::data::layer_content_metadata::LayerTypes;
use libcnb::layer::ExistingLayerStrategy;
use libcnb::{
    additional_buildpack_binary_path,
    generic::GenericMetadata,
    layer::{Layer, LayerResultBuilder},
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tar::Archive;
use tempfile::NamedTempFile;

/// Agentmon URL
///
/// - Repo: https://github.com/heroku/agentmon
/// - Releases: https://github.com/heroku/agentmon/releases
///
/// To get the latest s3 url:
///
/// ```shell
/// $ curl https://agentmon-releases.s3.amazonaws.com/latest
/// ```
const DOWNLOAD_URL: &str =
    "https://agentmon-releases.s3.amazonaws.com/agentmon-0.3.1-linux-amd64.tar.gz";

#[derive(Debug)]
pub(crate) struct InstallAgentmon {
    pub(crate) section: build_output::Section,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct Metadata {
    download_url: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum InstallAgentmonError {
    #[error("Could not read file permissions {0}")]
    PermissionError(std::io::Error),

    #[error("Could not open file: {0}")]
    CouldNotOpenFile(std::io::Error),

    #[error("Could not untar: {0}")]
    CouldNotUnpack(std::io::Error),

    // Boxed to prevent `large_enum_variant` errors since `ureq::Error` is massive.
    #[error("Download error: {0}")]
    RequestError(Box<ureq::Error>),

    #[error("Could not create file: {0}")]
    CouldNotCreateDestinationFile(std::io::Error),

    #[error("Could not write file: {0}")]
    CouldNotWriteDestinationFile(std::io::Error),
}

impl Layer for InstallAgentmon {
    type Buildpack = MetricsAgentBuildpack;
    type Metadata = Metadata;

    fn types(&self) -> libcnb::data::layer_content_metadata::LayerTypes {
        LayerTypes {
            build: true,
            launch: true,
            cache: true,
        }
    }

    fn create(
        &self,
        _context: &libcnb::build::BuildContext<Self::Buildpack>,
        layer_path: &std::path::Path,
    ) -> Result<
        libcnb::layer::LayerResult<Self::Metadata>,
        <Self::Buildpack as libcnb::Buildpack>::Error,
    > {
        let bin_dir = layer_path.join("bin");

        let mut timer = self.section.say_with_inline_timer("Downloading");
        let agentmon =
            agentmon_download(&bin_dir).map_err(MetricsAgentError::InstallAgentmonError)?;
        timer.done();

        self.section.say("Writing scripts");
        let execd = write_execd_script(&agentmon, layer_path)
            .map_err(MetricsAgentError::InstallAgentmonError)?;

        LayerResultBuilder::new(Metadata {
            download_url: Some(DOWNLOAD_URL.to_string()),
        })
        .exec_d_program("spawn-agentmon", execd)
        .build()
    }

    fn update(
        &self,
        _context: &libcnb::build::BuildContext<Self::Buildpack>,
        layer_data: &libcnb::layer::LayerData<Self::Metadata>,
    ) -> Result<
        libcnb::layer::LayerResult<Self::Metadata>,
        <Self::Buildpack as libcnb::Buildpack>::Error,
    > {
        let layer_path = &layer_data.path;

        self.section.say("Writing scripts");
        let execd = write_execd_script(&layer_path.join("bin").join("agentmon"), layer_path)
            .map_err(MetricsAgentError::InstallAgentmonError)?;

        LayerResultBuilder::new(Metadata {
            download_url: Some(DOWNLOAD_URL.to_string()),
        })
        .exec_d_program("spawn agentmon", execd)
        .build()
    }

    fn existing_layer_strategy(
        &self,
        _context: &libcnb::build::BuildContext<Self::Buildpack>,
        layer_data: &libcnb::layer::LayerData<Self::Metadata>,
    ) -> Result<libcnb::layer::ExistingLayerStrategy, <Self::Buildpack as libcnb::Buildpack>::Error>
    {
        match &layer_data.content_metadata.metadata.download_url {
            Some(url) if url == DOWNLOAD_URL => {
                self.section.say("Using cached metrics agent");
                Ok(ExistingLayerStrategy::Update)
            }
            Some(url) => {
                self.section.say_with_details(
                    "Updating metrics agent",
                    format!("{} to {}", url, DOWNLOAD_URL),
                );
                Ok(ExistingLayerStrategy::Recreate)
            }
            None => Ok(ExistingLayerStrategy::Recreate),
        }
    }

    fn migrate_incompatible_metadata(
        &self,
        _context: &libcnb::build::BuildContext<Self::Buildpack>,
        _metadata: &GenericMetadata,
    ) -> Result<
        libcnb::layer::MetadataMigration<Self::Metadata>,
        <Self::Buildpack as libcnb::Buildpack>::Error,
    > {
        self.section
            .say_with_details("Clearing cache", "invalid metadata");

        Ok(libcnb::layer::MetadataMigration::RecreateLayer)
    }
}

fn write_execd_script(agentmon: &Path, layer_path: &Path) -> Result<PathBuf, InstallAgentmonError> {
    let log = layer_path.join("output.log");
    let execd = layer_path.join("execd");
    let daemon = layer_path.join("launch_daemon");
    let run_loop = layer_path.join("agentmon_loop");

    // Ensure log file exists
    fs_err::write(&log, "").map_err(InstallAgentmonError::CouldNotWriteDestinationFile)?;

    // agentmon_loop boots agentmon continuously
    fs_err::copy(
        additional_buildpack_binary_path!("agentmon_loop"),
        &run_loop,
    )
    .map_err(InstallAgentmonError::CouldNotWriteDestinationFile)?;

    // The `launch_daemon` schedules `agentmon_loop` to run in the background
    fs_err::copy(additional_buildpack_binary_path!("launch_daemon"), &daemon)
        .map_err(InstallAgentmonError::CouldNotWriteDestinationFile)?;

    // The execd bash script will be run by CNB lifecycle, it runs the `launch_daemon`
    fs_err::write(
        &execd,
        format!(
            r#"#!/usr/bin/env bash

                    {daemon} --log {log} --loop-path {run_loop} --agentmon {agentmon}
                "#,
            log = log.display(),
            daemon = daemon.display(),
            run_loop = run_loop.display(),
            agentmon = agentmon.display(),
        ),
    )
    .map_err(InstallAgentmonError::CouldNotCreateDestinationFile)?;

    chmod_plus_x(&execd).map_err(InstallAgentmonError::PermissionError)?;

    Ok(execd)
}

fn agentmon_download(dir: &Path) -> Result<PathBuf, InstallAgentmonError> {
    download_to_dir(DOWNLOAD_URL, dir)?;

    Ok(dir.join("agentmon"))
}

fn download_to_dir(url: impl AsRef<str>, destination: &Path) -> Result<(), InstallAgentmonError> {
    let agentmon_tgz =
        NamedTempFile::new().map_err(DownloadAgentmonError::CouldNotCreateDestinationFile)?;

    download(url, agentmon_tgz.path())?;

    untar(agentmon_tgz.path(), destination)?;

    chmod_plus_x(&destination.join("agentmon")).map_err(DownloadAgentmonError::PermissionError)?;

    Ok(())
}

pub(crate) fn untar(
    path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<(), DownloadAgentmonError> {
    let file =
        fs_err::File::open(path.as_ref()).map_err(DownloadAgentmonError::CouldNotOpenFile)?;

    Archive::new(GzDecoder::new(file))
        .unpack(destination.as_ref())
        .map_err(DownloadAgentmonError::CouldNotUnpack)
}

/// Sets file permissions on the given path to 7xx (similar to `chmod +x <path>`)
///
/// i.e. chmod +x will ensure that the first digit
/// of the file permission is 7 on unix so if you pass
/// in 0o455 it would be mutated to 0o755
pub fn chmod_plus_x(path: &Path) -> Result<(), std::io::Error> {
    let mut perms = fs_err::metadata(path)?.permissions();
    let mut mode = perms.mode();
    mode |= 0o700;
    perms.set_mode(mode);

    fs_err::set_permissions(path, perms)
}

/// Write a script to the target path while adding a bash shebang line and setting execution permissions
fn write_bash_script(path: &Path, script: impl AsRef<str>) -> std::io::Result<()> {
    let script = script.as_ref();
    fs_err::write(path, format!("#!/usr/bin/env bash\n\n{script}"))?;
    chmod_plus_x(path)?;

    Ok(())
}

pub(crate) fn download(
    uri: impl AsRef<str>,
    destination: impl AsRef<Path>,
) -> Result<(), DownloadAgentmonError> {
    let mut response_reader = ureq::get(uri.as_ref())
        .call()
        .map_err(|err| DownloadAgentmonError::RequestError(Box::new(err)))?
        .into_reader();

    let mut destination_file = fs_err::File::create(destination.as_ref())
        .map_err(DownloadAgentmonError::CouldNotCreateDestinationFile)?;

    std::io::copy(&mut response_reader, &mut destination_file)
        .map_err(DownloadAgentmonError::CouldNotWriteDestinationFile)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chmod() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.into_path().join("file");
        std::fs::write(&file, "lol").unwrap();

        let before = file.metadata().unwrap().permissions().mode();

        let foo = before | 0o777;

        dbg!(before);
        dbg!(foo);

        chmod_plus_x(&file).unwrap();

        let after = file.metadata().unwrap().permissions().mode();
        assert!(before != after);

        // Assert executable
        assert_eq!(after, after | 0o700);
    }
}
