use bon::builder;
use commons::display::SentenceList;
use libcnb::build::BuildContext;
use libcnb::layer::{CachedLayerDefinition, InvalidMetadataAction, LayerRef, RestoredLayerAction};
use std::fmt::Display;

/// Writes metadata to a layer and returns a reference to the layer
///
/// A function can be used to extract data or state from the old metadata
#[builder]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn cached_layer_builder<B, M, F, T>(
    layer_name: libcnb::data::layer::LayerName,
    context: &BuildContext<B>,
    metadata: &'_ M,
    build: Option<bool>,
    launch: Option<bool>,
    with_data: F,
) -> libcnb::Result<LayerRef<B, CacheState<T>, CacheState<T>>, B::Error>
where
    F: Fn(&M, &M) -> T,
    B: libcnb::Buildpack,
    M: MetadataDiff + magic_migrate::TryMigrate + serde::ser::Serialize + std::fmt::Debug,
    <M as magic_migrate::TryMigrate>::Error: std::fmt::Display,
{
    let layer_ref = context.cached_layer(
        layer_name,
        CachedLayerDefinition {
            build: build.unwrap_or(true),
            launch: launch.unwrap_or(true),
            invalid_metadata_action: &|invalid| {
                let (action, cause) = invalid_metadata_action(invalid);
                match action {
                    InvalidMetadataAction::ReplaceMetadata(_) => {
                        (action, CacheState::Message("Using cache".to_string()))
                    }
                    InvalidMetadataAction::DeleteLayer => (action, CacheState::Message(cause)),
                }
            },
            restored_layer_action: &|old: &M, _| {
                let (action, cause) = restored_layer_action(old, metadata);
                match action {
                    RestoredLayerAction::KeepLayer => {
                        let out = with_data(old, metadata);
                        (action, CacheState::Data(out))
                    }
                    RestoredLayerAction::DeleteLayer => (action, CacheState::Message(cause)),
                }
            },
        },
    )?;
    layer_ref.write_metadata(metadata)?;
    Ok(layer_ref)
}

pub(crate) enum CacheState<T> {
    Message(String),
    Data(T),
}

impl<T> Display for CacheState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl<T> AsRef<str> for CacheState<T> {
    fn as_ref(&self) -> &str {
        match self {
            CacheState::Message(s) => s.as_str(),
            CacheState::Data(_) => "Using cache",
        }
    }
}

/// Default behavior for a cached layer, ensures new metadata is always written
///
/// The metadadata must implement `MetadataDiff` and `TryMigrate` in addition
/// to the typical `Serialize` and `Debug` traits
pub(crate) fn cached_layer_write_metadata<M, B>(
    layer_name: libcnb::data::layer::LayerName,
    context: &BuildContext<B>,
    metadata: &'_ M,
) -> libcnb::Result<LayerRef<B, String, String>, B::Error>
where
    B: libcnb::Buildpack,
    M: MetadataDiff + magic_migrate::TryMigrate + serde::ser::Serialize + std::fmt::Debug,
    <M as magic_migrate::TryMigrate>::Error: std::fmt::Display,
{
    let layer_ref = context.cached_layer(
        layer_name,
        CachedLayerDefinition {
            build: true,
            launch: true,
            invalid_metadata_action: &invalid_metadata_action,
            restored_layer_action: &|old: &M, _| restored_layer_action(old, metadata),
        },
    )?;
    layer_ref.write_metadata(metadata)?;
    Ok(layer_ref)
}

/// Given another metadata object, returns a list of differences between the two
///
/// If no differences, return an empty list
pub(crate) trait MetadataDiff {
    fn diff(&self, old: &Self) -> Vec<String>;
}

/// Standardizes formatting for layer cache clearing behavior
///
/// If the diff is empty, there are no changes and the layer is kept
/// If the diff is not empty, the layer is deleted and the changes are listed
pub(crate) fn restored_layer_action<T>(old: &T, now: &T) -> (RestoredLayerAction, String)
where
    T: MetadataDiff,
{
    let diff = now.diff(old);
    if diff.is_empty() {
        (RestoredLayerAction::KeepLayer, "Using cache".to_string())
    } else {
        (
            RestoredLayerAction::DeleteLayer,
            format!(
                "Clearing cache due to {changes}: {differences}",
                changes = if diff.len() > 1 { "changes" } else { "change" },
                differences = SentenceList::new(&diff)
            ),
        )
    }
}

/// Standardizes formatting for invalid metadata behavior
///
/// If the metadata can be migrated, it is replaced with the migrated version
/// If an error occurs, the layer is deleted and the error displayed
/// If no migration is possible, the layer is deleted and the invalid metadata is displayed
pub(crate) fn invalid_metadata_action<T, S>(invalid: &S) -> (InvalidMetadataAction<T>, String)
where
    T: magic_migrate::TryMigrate,
    S: serde::ser::Serialize + std::fmt::Debug,
    // TODO: Enforce Display + Debug in the library
    <T as magic_migrate::TryMigrate>::Error: std::fmt::Display,
{
    let invalid = toml::to_string(invalid);
    match invalid {
        Ok(toml) => match T::try_from_str_migrations(&toml) {
            Some(Ok(migrated)) => (
                InvalidMetadataAction::ReplaceMetadata(migrated),
                "Replaced metadata".to_string(),
            ),
            Some(Err(error)) => (
                InvalidMetadataAction::DeleteLayer,
                format!("Clearing cache due to metadata migration error: {error}"),
            ),
            None => (
                InvalidMetadataAction::DeleteLayer,
                format!(
                    "Clearing cache due to invalid metadata ({toml})",
                    toml = toml.trim()
                ),
            ),
        },
        Err(error) => (
            InvalidMetadataAction::DeleteLayer,
            format!("Clearing cache due to invalid metadata serialization error: {error}"),
        ),
    }
}

/// Removes ANSI control characters from a string
#[cfg(test)]
pub(crate) fn strip_ansi(input: impl AsRef<str>) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("Clippy checked");
    re.replace_all(input.as_ref(), "").to_string()
}

/// Takes in a directory and returns a minimal build context for use in testing shared caching behavior
///
/// Intented only for use with this buildpack, but meant to be used by multiple layers to assert caching behavior.
#[cfg(test)]
pub(crate) fn temp_build_context<B: libcnb::Buildpack>(
    from_dir: impl AsRef<std::path::Path>,
) -> BuildContext<B> {
    let base_dir = from_dir.as_ref().to_path_buf();
    let layers_dir = base_dir.join("layers");
    let app_dir = base_dir.join("app_dir");
    let platform_dir = base_dir.join("platform_dir");
    let buildpack_dir = base_dir.join("buildpack_dir");
    for dir in [&app_dir, &layers_dir, &buildpack_dir, &platform_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let target = libcnb::Target {
        os: String::new(),
        arch: String::new(),
        arch_variant: None,
        distro_name: String::new(),
        distro_version: String::new(),
    };
    let buildpack_toml_string = include_str!("../../buildpack.toml");
    let platform =
        <<B as libcnb::Buildpack>::Platform as libcnb::Platform>::from_path(&platform_dir).unwrap();
    let buildpack_descriptor: libcnb::data::buildpack::ComponentBuildpackDescriptor<
        <B as libcnb::Buildpack>::Metadata,
    > = toml::from_str(buildpack_toml_string).unwrap();
    let buildpack_plan = libcnb::data::buildpack_plan::BuildpackPlan {
        entries: Vec::<libcnb::data::buildpack_plan::Entry>::new(),
    };
    let store = None;

    BuildContext {
        layers_dir,
        app_dir,
        buildpack_dir,
        target,
        platform,
        buildpack_plan,
        buildpack_descriptor,
        store,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RubyBuildpack;
    use core::panic;
    use libcnb::data::layer_name;
    use libcnb::layer::{EmptyLayerCause, LayerState};
    use magic_migrate::{migrate_toml_chain, try_migrate_deserializer_chain, Migrate, TryMigrate};
    use serde::Deserializer;
    use std::convert::Infallible;

    /// Struct for asserting the behavior of `cached_layer_write_metadata`
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct TestMetadata {
        value: String,
    }
    impl MetadataDiff for TestMetadata {
        fn diff(&self, old: &Self) -> Vec<String> {
            if self.value == old.value {
                vec![]
            } else {
                vec![format!("value ({} to {})", old.value, self.value)]
            }
        }
    }
    migrate_toml_chain! {TestMetadata}

    #[test]
    fn test_cached_layer_write_metadata_restored_layer_action() {
        let temp = tempfile::tempdir().unwrap();
        let context = temp_build_context::<RubyBuildpack>(temp.path());

        // First write
        let result = cached_layer_write_metadata(
            layer_name!("testing"),
            &context,
            &TestMetadata {
                value: "hello".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(
            result.state,
            LayerState::Empty {
                cause: EmptyLayerCause::NewlyCreated
            }
        ));

        // Second write, preserve the contents
        let result = cached_layer_write_metadata(
            layer_name!("testing"),
            &context,
            &TestMetadata {
                value: "hello".to_string(),
            },
        )
        .unwrap();
        let LayerState::Restored { cause } = &result.state else {
            panic!("Expected restored layer")
        };
        assert_eq!(cause, "Using cache");

        // Third write, change the data
        let result = cached_layer_write_metadata(
            layer_name!("testing"),
            &context,
            &TestMetadata {
                value: "world".to_string(),
            },
        )
        .unwrap();

        let LayerState::Empty {
            cause: EmptyLayerCause::RestoredLayerAction { cause },
        } = &result.state
        else {
            panic!("Expected empty layer with restored layer action");
        };
        assert_eq!(
            cause,
            "Clearing cache due to change: value (hello to world)"
        );
    }

    /// Struct for asserting the behavior of `invalid_metadata_action`
    #[derive(serde::Deserialize, serde::Serialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct PersonV1 {
        name: String,
    }
    /// Struct for asserting the behavior of `invalid_metadata_action`
    #[derive(serde::Deserialize, serde::Serialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct PersonV2 {
        name: String,
        updated_at: String,
    }
    // First define how to map from one struct to another
    impl TryFrom<PersonV1> for PersonV2 {
        type Error = NotRichard;
        fn try_from(value: PersonV1) -> Result<Self, NotRichard> {
            if &value.name == "Schneems" {
                Ok(PersonV2 {
                    name: value.name.clone(),
                    updated_at: "unknown".to_string(),
                })
            } else {
                Err(NotRichard {
                    name: value.name.clone(),
                })
            }
        }
    }
    #[derive(Debug, Eq, PartialEq)]
    struct NotRichard {
        name: String,
    }
    impl From<NotRichard> for PersonMigrationError {
        fn from(value: NotRichard) -> Self {
            PersonMigrationError::NotRichard(value)
        }
    }
    #[derive(Debug, Eq, PartialEq, thiserror::Error)]
    enum PersonMigrationError {
        #[error("Not Richard")]
        NotRichard(NotRichard),
    }
    try_migrate_deserializer_chain!(
        deserializer: toml::Deserializer::new,
        error: PersonMigrationError,
        chain: [PersonV1, PersonV2],
    );

    #[test]
    fn test_invalid_metadata_action() {
        let (action, message) = invalid_metadata_action::<PersonV1, _>(&PersonV1 {
            name: "schneems".to_string(),
        });
        assert!(matches!(action, InvalidMetadataAction::ReplaceMetadata(_)));
        assert_eq!(message, "Replaced metadata".to_string());

        let (action, message) = invalid_metadata_action::<PersonV2, _>(&PersonV1 {
            name: "not_richard".to_string(),
        });
        assert!(matches!(action, InvalidMetadataAction::DeleteLayer));
        assert_eq!(
            message,
            "Clearing cache due to metadata migration error: Not Richard".to_string()
        );

        let (action, message) = invalid_metadata_action::<PersonV2, _>(&TestMetadata {
            value: "world".to_string(),
        });
        assert!(matches!(action, InvalidMetadataAction::DeleteLayer));
        assert_eq!(
            message,
            "Clearing cache due to invalid metadata (value = \"world\")".to_string()
        );

        // Unable to produce this error at will: "Clearing cache due to invalid metadata serialization error: {error}"
    }
}
