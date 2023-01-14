use crate::in_app_dir_cache_layer::InAppDirCacheLayer;
use byte_unit::Byte;
use fs_extra::dir::CopyOptions;
use glob::PatternError;
use libcnb::build::BuildContext;
use libcnb::data::layer::LayerName;
use libcnb::Buildpack;
use std::marker::PhantomData;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use tempfile as _;

/// Store data generated in the `<app_dir>` between builds
///
/// Example:
///
///```rust
///# use libcnb::build::{BuildContext, BuildResult, BuildResultBuilder};
///# use libcnb::data::launch::{LaunchBuilder, ProcessBuilder};
///# use libcnb::data::process_type;
///# use libcnb::detect::{DetectContext, DetectResult, DetectResultBuilder};
///# use libcnb::generic::{GenericError, GenericMetadata, GenericPlatform};
///# use libcnb::{buildpack_main, Buildpack};
///# use libcnb::data::layer_name;
///# use libcnb::data::layer::LayerName;
///
///# pub(crate) struct HelloWorldBuildpack;
///
///use commons::in_app_dir_cache::InAppDirCache;
///
///# impl Buildpack for HelloWorldBuildpack {
///#     type Platform = GenericPlatform;
///#     type Metadata = GenericMetadata;
///#     type Error = GenericError;
///
///#     fn detect(&self, _context: DetectContext<Self>) -> libcnb::Result<DetectResult, Self::Error> {
///#         todo!()
///#     }
///
///#     fn build(&self, context: BuildContext<Self>) -> libcnb::Result<BuildResult, Self::Error> {
///         let public_assets_cache = InAppDirCache::new_and_load(
///             &context,
///             &context.app_dir.join("public").join("assets"),
///         ).unwrap();
///
///         std::fs::write(context.app_dir.join("public").join("assets").join("lol"), "hahaha");
///
///         public_assets_cache.copy_app_path_to_cache();
///
///#        todo!()
///#     }
///# }
/// ```
///
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DirCache {
    pub app_path: PathBuf,
    pub cache_path: PathBuf,
}

pub struct InAppDirCache<B> {
    buildpack: PhantomData<B>,
}

#[derive(thiserror::Error, Debug)]
pub enum CacheError {
    #[error("Cached path not in application directory: {0}")]
    CachedPathNotInAppPath(String),

    #[error("Invalid layer name: {0}")]
    InvalidLayerName(libcnb::data::layer::LayerNameError),

    #[error("IO error: {0}")]
    IoExtraError(fs_extra::error::Error),

    #[error("IO error: {0}")]
    IoError(std::io::Error),

    #[error("Cannot convert OsString into UTF-8 string: {0}")]
    OsStringError(String),

    #[error("An internal error occured while creating a dir glob pattern: {0}")]
    InternalBadGlobError(PatternError),

    #[error("An internal error occured while constructing the layer: {0}")]
    InternalLayerError(String),

    #[error("The OS does not support the retreiving `mtime` information from files: {0}")]
    MtimeUnsupportedOS(std::io::Error),
}

fn to_layer_name(base: &Path, app_path: &Path) -> Result<LayerName, CacheError> {
    let name = app_path
        .strip_prefix(base)
        .map_err(|_| {
            CacheError::CachedPathNotInAppPath(format!(
                "Expected cached app path {} to be in {} but it was not",
                app_path.display(),
                base.display(),
            ))
        })?
        .iter()
        .map(std::ffi::OsStr::to_string_lossy)
        .collect::<Vec<_>>()
        .join("_");

    format!("cache_{name}")
        .parse()
        .map_err(CacheError::InvalidLayerName)
}

impl<B: Buildpack> InAppDirCache<B> {
    /// Creates an ```InAppDirCache``` struct and loads cache contents to app directory
    ///
    /// # Errors
    ///
    /// - Err if either the ```app_path``` or ```cache_path``` cannot be created due to an error
    /// from the OS, such as file permissions.
    /// - Err if the contents of the cache directory cannot be moved to the app directory, perhaps
    /// due to a permissions problem.
    /// - Err if the generated layer name is invalid.
    ///- Err if there's an internal error creating the layer.
    pub fn new_and_load(
        context: &BuildContext<B>,
        app_path: &Path,
    ) -> Result<DirCache, CacheError> {
        let app_path = app_path.to_path_buf();

        let cache_path = context
            .handle_layer(
                to_layer_name(&context.app_dir, &app_path)?,
                InAppDirCacheLayer::new(app_path.clone()),
            )
            .map_err(|error| CacheError::InternalLayerError(format!("{error:?}")))?
            .path;

        let out = DirCache {
            app_path,
            cache_path,
        };
        out.mkdir_p()?;
        out.move_cache_to_app()?;

        Ok(out)
    }
}

impl DirCache {
    /// # Errors
    ///
    /// Fails if either the ```app_path``` or ```cache_path``` cannot be created due to an error
    /// from the OS, such as file permissions.
    fn mkdir_p(&self) -> Result<(), CacheError> {
        std::fs::create_dir_all(&self.app_path).map_err(CacheError::IoError)?;
        std::fs::create_dir_all(&self.cache_path).map_err(CacheError::IoError)?;

        Ok(())
    }

    /// # Errors
    ///
    /// - If the move command fails an `IoExtraError` will be raised by the OS.
    fn move_cache_to_app(&self) -> Result<&Self, CacheError> {
        fs_extra::dir::move_dir(
            &self.cache_path,
            &self.app_path,
            &CopyOptions {
                overwrite: false,
                skip_exist: true,
                copy_inside: true,
                content_only: true,
                ..CopyOptions::default()
            },
        )
        .map_err(CacheError::IoExtraError)?;

        Ok(self)
    }

    /// # Errors
    ///
    /// - If the move command fails an `IoExtraError` will be raised.
    pub fn destructive_move_app_path_to_cache(&self) -> Result<&Self, CacheError> {
        fs_extra::dir::move_dir(
            &self.app_path,
            &self.cache_path,
            &CopyOptions {
                overwrite: false,
                skip_exist: true,
                copy_inside: true,
                content_only: true,
                ..CopyOptions::default()
            },
        )
        .map_err(CacheError::IoExtraError)?;

        Ok(self)
    }

    /// # Errors
    ///
    /// - If the copy command fails an `IoExtraError` will be raised.
    pub fn copy_app_path_to_cache(&self) -> Result<&Self, CacheError> {
        fs_extra::dir::copy(
            &self.app_path,
            &self.cache_path,
            &CopyOptions {
                overwrite: false,
                skip_exist: true,
                copy_inside: true,

                content_only: true,
                ..CopyOptions::default()
            },
        )
        .map_err(CacheError::IoExtraError)?;

        Ok(self)
    }

    /// # Errors
    ///
    /// - The provided ``cache_path`` is not valid UTF-8 (`OsStringErr`).
    /// - Metadata from a file in the ``cache_path`` cannot be retrieved from the OS (`IoError`).
    /// this is needed for mtime retrieval to calculate which file is least recently used.
    /// - If an internal glob pattern is incorrect
    /// - If the OS does not support mtime.
    pub fn least_recently_used_files_above_limit(
        &self,
        max_bytes: Byte,
    ) -> Result<FilesWithSize, CacheError> {
        Self::least_recently_used_files_above_limit_from_path(&self.cache_path, max_bytes)
    }

    fn least_recently_used_files_above_limit_from_path(
        cache_path: &Path,
        max_bytes: Byte,
    ) -> Result<FilesWithSize, CacheError> {
        let max_bytes = max_bytes.get_bytes();
        let glob_string = cache_path
            .join("**/*")
            .into_os_string()
            .into_string()
            .map_err(|e| CacheError::OsStringError(e.to_string_lossy().to_string()))?;

        let mut files = glob::glob(&glob_string)
            .map_err(CacheError::InternalBadGlobError)?
            .filter_map(Result::ok)
            .filter(|p| p.is_file()) // Means we aren't removing empty directories
            .map(MiniPathModSize::new)
            .collect::<Result<Vec<MiniPathModSize>, _>>()?;

        let bytes = files.iter().map(|p| u128::from(p.size)).sum::<u128>();

        if bytes >= max_bytes {
            let mut current_bytes = bytes;
            files.sort_by(|a, b| a.modified.cmp(&b.modified));

            Ok(FilesWithSize {
                bytes,
                files: files
                    .iter()
                    .take_while(|m| {
                        current_bytes -= u128::from(m.size);
                        current_bytes >= max_bytes
                    })
                    .map(|p| p.path.clone())
                    .collect::<Vec<PathBuf>>(),
            })
        } else {
            Ok(FilesWithSize::default())
        }
    }
}

#[derive(Debug, Eq, PartialEq, Default)]
pub struct FilesWithSize {
    pub bytes: u128,
    pub files: Vec<PathBuf>,
}

#[derive(Debug)]
struct MiniPathModSize {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
}

impl MiniPathModSize {
    fn new(path: PathBuf) -> Result<Self, CacheError> {
        let metadata = path.metadata().map_err(CacheError::IoError)?;
        let modified = metadata
            .modified()
            .map_err(CacheError::MtimeUnsupportedOS)?;
        let size = metadata.size();

        Ok(Self {
            path,
            modified,
            size,
        })
    }
}

impl FilesWithSize {
    #[must_use]
    pub fn to_byte(&self) -> Byte {
        Byte::from_bytes(self.bytes)
    }

    /// # Errors
    ///
    /// Returns an error if one of the files to clean cannot be removed
    /// by the operating system.
    pub fn clean(&self) -> Result<(), CacheError> {
        for file in &self.files {
            std::fs::remove_file(file).map_err(CacheError::IoError)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use byte_unit::n_mib_bytes;
    use libcnb::data::layer_name;

    use super::*;

    pub fn touch_file(path: &PathBuf, f: impl FnOnce(&PathBuf)) {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).unwrap();
            }
        }
        std::fs::write(path, "").unwrap();
        f(path);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_to_layer_name() {
        let dir = PathBuf::from_str("muh_base").unwrap();
        let layer = to_layer_name(&dir, &dir.join("my").join("input")).unwrap();
        assert_eq!(layer_name!("cache_my_input"), layer);
    }

    #[test]
    fn test_copying_back_to_cache() {
        let tmpdir = tempfile::tempdir().unwrap();
        let cache_path = tmpdir.path().join("cache");
        let app_path = tmpdir.path().join("app");
        let cache = DirCache {
            app_path: app_path.clone(),
            cache_path: cache_path.clone(),
        };
        cache.mkdir_p().unwrap();

        assert!(app_path.read_dir().unwrap().next().is_none()); // Assert empty dir
        cache.move_cache_to_app().unwrap();
        assert!(app_path.read_dir().unwrap().next().is_none()); // Assert dir not changed

        std::fs::write(app_path.join("lol.txt"), "hahaha").unwrap();

        // Test copy logic from app to cache
        assert!(!cache.cache_path.join("lol.txt").exists());
        assert!(cache_path.read_dir().unwrap().next().is_none());
        cache.copy_app_path_to_cache().unwrap();
        assert!(cache.cache_path.join("lol.txt").exists());
        assert!(cache.app_path.join("lol.txt").exists());
    }

    #[test]
    fn test_moving_back_to_cache() {
        let tmpdir = tempfile::tempdir().unwrap();
        let cache_path = tmpdir.path().join("cache");
        let app_path = tmpdir.path().join("app");
        let cache = DirCache {
            app_path: app_path.clone(),
            cache_path: cache_path.clone(),
        };
        cache.mkdir_p().unwrap();

        assert!(app_path.read_dir().unwrap().next().is_none()); // Assert empty dir
        cache.move_cache_to_app().unwrap();
        assert!(app_path.read_dir().unwrap().next().is_none()); // Assert dir not changed

        std::fs::write(app_path.join("lol.txt"), "hahaha").unwrap();

        // Test copy logic from app to cache
        assert!(!cache.cache_path.join("lol.txt").exists());
        assert!(cache_path.read_dir().unwrap().next().is_none());
        cache.destructive_move_app_path_to_cache().unwrap();
        assert!(cache.cache_path.join("lol.txt").exists());
        assert!(!cache.app_path.join("lol.txt").exists());
    }

    #[test]
    fn test_lru_only_returns_based_on_size() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path().join("dir");

        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(
            DirCache::least_recently_used_files_above_limit_from_path(
                &dir,
                Byte::from_bytes(n_mib_bytes!(0)),
            )
            .unwrap()
            .files
            .len(),
            0
        );

        touch_file(&dir.join("a"), |file| {
            let overage = DirCache::least_recently_used_files_above_limit_from_path(
                &dir,
                Byte::from_bytes(n_mib_bytes!(0)),
            )
            .unwrap();
            assert_eq!(overage.files, vec![file.clone()]);

            let overage = DirCache::least_recently_used_files_above_limit_from_path(
                &dir,
                Byte::from_bytes(n_mib_bytes!(10)),
            )
            .unwrap();
            assert_eq!(overage.files.len(), 0);
        });
    }

    #[test]
    fn test_lru_returns_older_files_first() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path().join("");

        touch_file(&dir.join("z_older"), |a| {
            touch_file(&dir.join("a_newer"), |b| {
                filetime::set_file_mtime(a, filetime::FileTime::from_unix_time(0, 0)).unwrap();
                filetime::set_file_mtime(b, filetime::FileTime::from_unix_time(1, 0)).unwrap();

                let overage = DirCache::least_recently_used_files_above_limit_from_path(
                    &dir,
                    Byte::from_bytes(n_mib_bytes!(0)),
                )
                .unwrap();
                assert_eq!(overage.files, vec![a.clone(), b.clone()]);
            });
        });
    }

    #[test]
    fn test_lru_does_not_grab_directories() {
        let tmpdir = tempfile::tempdir().unwrap();
        let dir = tmpdir.path().join("");
        std::fs::create_dir_all(dir.join("preservation_society")).unwrap();
        let overage = DirCache::least_recently_used_files_above_limit_from_path(
            &dir,
            Byte::from_bytes(n_mib_bytes!(0)),
        )
        .unwrap();
        assert_eq!(overage.files, Vec::<PathBuf>::new());
    }
}
