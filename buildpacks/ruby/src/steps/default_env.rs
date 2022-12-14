use crate::{
    layers::{EnvDefaultsLayer, EnvSecretKeyBaseLayer},
    RubyBuildpack, RubyBuildpackError,
};
use libcnb::{build::BuildContext, data::layer_name, layer_env::Scope, Env};

// Set default environment values
pub(crate) fn default_env(
    context: &BuildContext<RubyBuildpack>,
    platform_env: &Env,
) -> libcnb::Result<Env, RubyBuildpackError> {
    // Get system env vars
    let mut env = Env::from_current();

    // Apply User env vars
    // TODO reject harmful vars like GEM_PATH
    for (k, v) in platform_env {
        env.insert(k, v);
    }

    // Setup default environment variables
    let secret_key_base_layer =
        context //
            .handle_layer(layer_name!("secret_key_base"), EnvSecretKeyBaseLayer)?;
    env = secret_key_base_layer.env.apply(Scope::Build, &env);

    let env_defaults_layer = context //
        .handle_layer(layer_name!("env_defaults"), EnvDefaultsLayer)?;
    env = env_defaults_layer.env.apply(Scope::Build, &env);

    Ok(env)
}
