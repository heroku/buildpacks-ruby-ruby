 ## Setup DiffMigrateLayer for new layer Metadata

Starting from scratch, add dependencies:

```term
$ cargo add cache_diff --features bullet_stream
$ cargo add magic_migrate toml serde bullet_stream
$ cargo add commons --git https://github.com/heroku/buildpacks-ruby --branch main
```

In a layer file, define a metadata struct:

```rust
use cache_diff::CacheDiff;
use serde::{Deserialize, Serialize};

 #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, CacheDiff)]
 #[serde(deny_unknown_fields)]
pub(crate) struct MetadataV1 {
    #[cache_diff(rename = "Ruby version")]
    pub(crate) version: String,
}

pub(crate) type Metadata = MetadataV1;
```

This code:

- Allows the struct to be [`serde::Serialize`]/[`serde::Deserialize`] as toml
- Sets some convenient traits [`Debug`], [`Clone`], [`Eq`], [`PartialEq`]
- Defines how the metadata is used to invalidate the cache with the [`CacheDiff`] derive
- Sets a convienece type alias for the latest Metadata

In this code if the `version` field changes then the cache will be invalidated.

Now tell it how to migrate invalid metadata:


```rust
use magic_migrate::TryMigrate;
// ...
# use cache_diff::CacheDiff;
# use serde::{Deserialize, Serialize};
#
# #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, CacheDiff)]
# #[serde(deny_unknown_fields)]
# pub(crate) struct MetadataV1 {
#     #[cache_diff(rename = "Ruby version")]
#     pub(crate) version: String,
# }
#
# pub(crate) type Metadata = MetadataV1;

 #[derive(Debug)]
pub(crate) enum MigrationError {}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

magic_migrate::try_migrate_toml_chain!(
    error: MigrationError,
    chain: [MetadataV1]
);
```

This code:

- Defines an error type (so we can populate it when we need to add a failable migration)
- The error type needs to be `Display` and `Debug`
- Uses the `magic_migrate::try_migrate_toml_chain` macro to tell our code how it can migrate from one type to the next.
  This will implement `TryMigrate` on every struct in the `chain` argument. In this case there's only one metadata value,
  but we will implement this behavior now so it's easy to extend later.


At this point we've implemented `CacheDiff` and `TryMigrate` on our metadata, so we can define a layer:

```rust
use commons::layer::diff_migrate::{DiffMigrateLayer, Meta};

use bullet_stream::{Print, state::SubBullet};
use std::io::Stdout;

use libcnb::layer::{LayerState, EmptyLayerCause};
use libcnb::data::layer_name;
use libcnb::Buildpack;
use libcnb::build::BuildContext;
use libcnb::layer_env::LayerEnv;

// ...
# use magic_migrate::TryMigrate;
# use cache_diff::CacheDiff;
# use serde::{Deserialize, Serialize};
#
# #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, CacheDiff)]
# #[serde(deny_unknown_fields)]
# pub(crate) struct MetadataV1 {
#     #[cache_diff(rename = "Ruby version")]
#     pub(crate) version: String,
# }
#
# pub(crate) type Metadata = MetadataV1;
#
#  #[derive(Debug)]
# pub(crate) enum MigrationError {}
#
# impl std::fmt::Display for MigrationError {
#     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
#         todo!()
#     }
# }
#
# magic_migrate::try_migrate_toml_chain!(
#     error: MigrationError,
#     chain: [MetadataV1]
# );

fn install_ruby(version: &str, path: &std::path::Path) {
    todo!()
}

pub(crate) fn call<B>(
    context: &BuildContext<B>,
    mut bullet: Print<SubBullet<Stdout>>,
    metadata: &Metadata,
) -> libcnb::Result<(Print<SubBullet<Stdout>>, LayerEnv), <B as Buildpack>::Error>
where
  B: Buildpack
{
    let layer_ref = DiffMigrateLayer {
        build: true,
        launch: true,
    }
    .cached_layer(layer_name!("ruby"), context, metadata)?;
    match &layer_ref.state {
        LayerState::Restored { cause } => {
            bullet = bullet.sub_bullet(cause);
        }
        LayerState::Empty { cause } => {
            match cause {
                EmptyLayerCause::NewlyCreated => {}
                EmptyLayerCause::InvalidMetadataAction { cause }
                | EmptyLayerCause::RestoredLayerAction { cause } => {
                    bullet = bullet.sub_bullet(cause);
                }
            }
            let timer = bullet.start_timer("Installing");
            install_ruby(&metadata.version, &layer_ref.path());
            bullet = timer.done();
        }
    }
    Ok((bullet, layer_ref.read_env()?))
}
```

The signature

- Defines a `call` function that:
  - Takes a build context. In your code you'll want to replace the generic with a concrete buildpack type.
  - Takes a bullet_stream printer for maximal printing consistency
  - A `Metadata` struct constructed externally

The logic of the function uses [`DiffMigrateLayer`] to create a layer that is both available at build and launch time. It creates a layer named "ruby" and passes in our metadata. When this executes it will:

- Create the layer if it doesn't exist yet
- Invalidate the cache if the `version` attribute changed and return a `Meta::Message` explaining why
- Keep the cache if the version did not change and return the old `Meta::Data` (useful if not every attribute is used as a cache key)
- Migrate any old metadata (not applicable yet)
- Write the new metadata to the layer

The return value is a `LayerRef` which we are using in a match statement. If the cache was restored it will emit that information to the buildpack user. If it was invalidated (if the version changed) it will emit that. When the layer is empty for any reason it will "install ruby" with a timer printed to stdout.

A successful run of this function returns a tuple with `bullet_stream::Print<SubBullet<Stdout>>` which can be used to continue streaming and a `LayerEnv` which can be used to pass on any environment varible modifications from this layer (if any are added in the future).

## Add a Metadata migration

Over time, you might realize that your Metadata didn't accurately reflect your correct domain. For example, you might realize that OS distribution and version number are important and when they change, the cache needs to be cleared. If you simply added these fields to `MetadataV1` you would trigger invalid metadata which has to be handled. So instead we can add whatever fields we want to a new struct named `MetadataV2` and tell our program how to migrate from one to the other.

> This might seem like overkill, but consider we might not stop at just these two versions we could have a V3 or v4 etc. Even trivial modifications, such as renaming an existing field could accidentally trigger this invalid metadata. In isolation, it's easy to migrate from one version to the other, but there's no guarantee that buildpack users will deploy at a regular cadence. We need to handle the situation where we're on V5 of metadata and users need to upgrade V1 and v4 at the same time.

Let's add that new metadata now:

```rust
use commons::layer::diff_migrate::{DiffMigrateLayer, Meta};
use magic_migrate::TryMigrate;
use cache_diff::CacheDiff;
use serde::{Deserialize, Serialize};

# #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
# #[serde(deny_unknown_fields)]
# pub(crate) struct MetadataV1 {
#     pub(crate) version: String,
# }
#
#
#  #[derive(Debug)]
# pub(crate) enum MigrationError {}
#
# impl std::fmt::Display for MigrationError {
#     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
#         todo!()
#     }
# }
#
 #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, CacheDiff)]
 #[serde(deny_unknown_fields)]
pub(crate) struct MetadataV2 {
    #[cache_diff(rename = "Ruby version")]
    pub(crate) version: String,

    #[cache_diff(rename = "OS distribution")]
    pub(crate) distro: String
}

fn get_distro_from_current_os() -> String {
   // Just pretend, ok
   todo!();
}

impl TryFrom<MetadataV1> for MetadataV2 {
    type Error = MigrationError;

    fn try_from(old: MetadataV1) -> Result<Self, Self::Error> {
        Ok(Self {
            version: old.version,
            distro: get_distro_from_current_os()
        })
    }
}

pub(crate) type Metadata = MetadataV2;

magic_migrate::try_migrate_toml_chain!(
    error: MigrationError,
    chain: [MetadataV1, MetadataV2]
);
```

Here we added:

- A new struct `MetadataV2` with a new field `distro` that `V1` does not have.
- Updated the `type Metadata = MetadataV2` to `V2`
- Added `MetadataV2` to the end of our migration chain.

Now when our layer logic is called, it will first try to deserialize the contents into `MetadataV2` if it can it will return that and continue on to the cache invalidation logic. If not, it will try to deserialize the old toml into `MetadataV1`. If it can, then it will and then migrate from `MetadataV1` to `MetadataV2` using the `TryFrom` and `TryMigrate` traits.

## Handle migration errors

The logic so far doesn't need an error state, but what if we did? What if we realized we wanted to add another field for CPU architecture, and we also know that only versions greater than 2 support ARM? Let's add that logic and find out:

```rust
use commons::layer::diff_migrate::{DiffMigrateLayer, Meta};
use magic_migrate::TryMigrate;
use cache_diff::CacheDiff;
use serde::{Deserialize, Serialize};

# #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
# #[serde(deny_unknown_fields)]
# pub(crate) struct MetadataV1 {
#     pub(crate) version: String,
# }
#
#
 #[derive(Debug)]
pub(crate) enum MigrationError {
    InvalidVersionArch {
        version: String,
        arch: String,
    }
}
#
# impl std::fmt::Display for MigrationError {
#     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
#         todo!()
#     }
# }
#
#  #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
#  #[serde(deny_unknown_fields)]
# pub(crate) struct MetadataV2 {
#     pub(crate) version: String,
#     pub(crate) distro: String
# }
#
# fn get_distro_from_current_os() -> String {
#    // Just pretend, ok
#    todo!();
# }
#
# impl TryFrom<MetadataV1> for MetadataV2 {
#     type Error = MigrationError;
#
#     fn try_from(old: MetadataV1) -> Result<Self, Self::Error> {
#         Ok(Self {
#             version: old.version,
#             distro: get_distro_from_current_os()
#         })
#     }
# }
#

 #[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, CacheDiff)]
 #[serde(deny_unknown_fields)]
pub(crate) struct MetadataV3 {
    #[cache_diff(rename = "Ruby version")]
    pub(crate) version: String,

    #[cache_diff(rename = "OS distribution")]
    pub(crate) distro: String,

    #[cache_diff(rename = "CPU architecture")]
    pub(crate) arch: String
}

# fn get_arch_from_current_cpu() -> String { todo!(); }

impl TryFrom<MetadataV2> for MetadataV3 {
    type Error = MigrationError;

    fn try_from(old: MetadataV2) -> Result<Self, Self::Error> {
        let distro = get_distro_from_current_os();
        let arch = get_arch_from_current_cpu();
        if old.version.starts_with("1.") && &arch == "arm64" {
            Err(
                MigrationError::InvalidVersionArch {
                    version: old.version,
                    arch: arch
                }
            )
        } else {
            Ok(Self {
                version: old.version,
                distro: old.distro,
                arch: arch
            })
        }
    }
}

pub(crate) type Metadata = MetadataV3;

magic_migrate::try_migrate_toml_chain!(
    error: MigrationError,
    chain: [MetadataV1, MetadataV2, MetadataV3]
);
```

What did we do? We added:

- A new `MetadataV3` with a new field `Arch`
- A new error variant to our `MigrationError` named `InvalidVersionArch`.
- A new `TryFrom<MetadataV2>` to `MetadataV3` that fails if we try to re-use version 1.x on an `arm64` CPU (an arbitrary specification made for this example).

Then we:

- Updated the `type Metadata = MetadataV3` to `V3`
- Added `MetadataV3` to the end of our migration chain.

Now when metadata is loaded it will go down the chain in reverse, it will try to load `V3` if it fails go to `V2` if it fails go to `V1`. If a match is successful it will reverse the process, migrating from `V1` to `V2` to `V3` etc. If our error condition is hit where someone is using version 1.x with an ARM CPU then an that will halt the migration process and trigger clearing the cache.

## Recap

The two traits `CacheDiff` and `TryMigrate` are relatively simple, but combined, give the program enough information to make previously tedious or complicated logic the default.

## Q&A

- Q: Wait, do I have to support metadata schemas (structs) forever?
- A: No. You can drop old structs whenver you feel it's necessary or invalidate the metadata at any time you like. The key with making your metadata migrate-able is that you don't HAVE to invalidate with every change. It makes it easier to ship the behavior that's best for you and your users.

- Q: Can I use default logic without having to implement both traits? It seems odd to add a `TryMigrate` trait for a scenario where we might never need one.
- A: If you put in work adopting this migration pattern and never need it, it's one crate, one trait, and one struct. Not that much work. But a co-worker or contributor new to buildpacks needs to modify it, or a future tired you needs to modify it...it's easier to extend an existing pattern than remember the esoteric rules and edge cases of what will and won't serialize into a struct.

- Q: You used `Metadata` as a type alias for use outside of the module. If you have multiple modules wouldn't they all have the same import? Shouldn't you namespace them somehow?
- A: Having to remember a naming convention for metadata in various layer modules is needless creativity. Instead of importing the struct, import the module and use that as a namespace, for example:

```text
use ruby_layer;
use bundler_layer;

//...

ruby_layer::Metadata {
  //...
}

bundler_layer::Metadata {
  //...
}
```

When you rev your metadata version, you'll need to add or modify any attributes that changed, but your imports and struct names won't need to change. Any use in type signatures doesn't need to be refactored.

- Q: What bad habbits did you use here for the sake of making the example easier that I should avoid?
- A: Having all of your metadata fields be strings will not yield a strongly typed program. It will be "stringly" typed instead. Best practice would be to make purpose-built structs or if you must use strings, use the [New Type pattern](https://doc.rust-lang.org/rust-by-example/generics/new_types.html).

- Q: Any other tips?
- A: Sure!
  - Make sure to `#[serde(deny_unknown_fields)]` on your metadata structs
  - Don't use overly flexible types such as `Option<String>` unless you really have to. Metadata can be loaded wither with or without that attribute which might not be exactly what you want when you're deserializing old metadata.
  - For layers that need to execute commands (such as `bundle install`), you can [use the `fun_run` crate](https://docs.rs/fun_run/0.2.0/fun_run/) which helps clearly print what's happening and gives lots of information when things fail.
  - Beware if v1 and v3 have the same named attributes, but different semantics rust will happily serialize the values stored from v1 into v3 and you'll never get an error or warning and your `TryFrom` code won't fire. This is also a problem when not using the `TryMigrate` pattern, so stay on the lookout.
  - For extremly important cache invalidation logic, add unit tests.
