# Heroku Ruby cloud native buildpack (CNB)

[![CI](https://github.com/heroku/buildpacks-ruby/actions/workflows/ci.yml/badge.svg)](https://github.com/heroku/buildpacks-ruby/actions/workflows/ci.yml)

Heroku's official [Cloud Native Buildpacks](https://buildpacks.io/) for the Ruby ecosystem.

## Classic Heroku Buildpack

If you're looking for the repositories of the classic Ruby Heroku buildpack for usage on the Heroku platform, please see [heroku/ruby](https://github.com/heroku/heroku-buildpack-ruby).

If you're migrating from the classic buildpack, known differences are documented below.

## What is a buildpack?

Buildpacks take in an application's code and prepare it to run, usually in production. Unlike a declarative system like Dockerfile, buildpacks are designed to be low or no configuration required. They can inspect the application and make dynamic adjustments using common community standards.

This buildpack is a Cloud Native Buildpack (CNB). CNB is a specification for building [OCI images](https://opencontainers.org/) (like docker). Some services natively support CNBs while other services might require you to generate an image locally and deploy that image through a container registry service.

You can find out more about the CNB project at https://buildpacks.io/.

## Application contract

The sections below describe the expected behavior of the buildpack. The codebase must be updated if a difference exists between this contract and actual behavior. Either the code needs to change to suit the contract, or the contract needs to be updated. If you see a difference, please open an issue with a [minimal application that reproduces the problem](https://www.codetriage.com/example_app).

If you need application-specific support, you can ask on an official Heroku support channel or Stack Overflow.

### Application Contract: Detect

The detect phase determine whether or not this buildpack can execute. It can also be used to request additional functionality via "requiring" behavior from other buildpacks.

- Node version
  - Given a `package.json` file in the root of the application the `heroku/nodejs-engine` buildpack will be required. [See README for behavior](https://github.com/heroku/buildpacks-nodejs/tree/main/buildpacks/nodejs-engine)
- Yarn version
  - Given a `yarn.lock` file in the root of the application the `heroku/nodejs-yarn` buildpack will be required. [See README for behavior](https://github.com/heroku/buildpacks-nodejs/tree/main/buildpacks/nodejs-yarn)
- Java version
  - Given a `Gemfile.lock` file that specifies jruby the `heroku/jvm` buildpack will be required. [See README for behavior](https://github.com/heroku/buildpacks-jvm/)
- Ruby version
  - Given a `Gemfile.lock` this buildpack will execute the Ruby build contract below.

### Application Contract: Build

Once an application has passed the detect phase, the build phase will execute to prepare the application to run.

- Ruby version
  - Given a `Gemfile.lock` with an explicit Ruby version we will install that Ruby version.
  - Given a `Gemfile.lock` without an explicit Ruby version we will install a default Ruby version.
    - When the default value changes, applications without an explicit Ruby version will receive the updated default Ruby version on their next deployment.
  - We will reinstall Ruby if your stack changes.
  - We may skip ruby binary instalation if `HEROKU_USE_SYSTEM_RUBY` environment variable is set by a user or another prior buildpack.
    - We may change `HEROKU_USE_SYSTEM_RUBY` behavior at any time [experimental interface]
    - We cannot guarantee the rest of the application contract when `HEROKU_USE_SYSTEM_RUBY` is set. That means we may close any issues that cannot be reproduced without `HEROKU_USE_SYSTEM_RUBY` set.
- Bundler version
  - Given a `Gemfile.lock` with an explicit Bundler version we will install that bundler version.
  - Given a `Gemfile.lock` without an explicit Bundler version we will install a default Ruby version.
- Ruby Dependencies
  - We MAY install gem dependencies using `bundle install`
    - We will always run `bundle install` for the first build
    - We will sometimes run this command again if we detect one of the following has changed:
      - Gemfile
      - Gemfile.lock
      - User configurable environment variables
    - We will always run `bundle install` on every build if `HEROKU_SKIP_BUNDLE_DIGEST=1` is set.
  - We will run `bundle clean` after a successful `bundle install` via setting `BUNDLE_CLEAN=1` environment variable.
  - We will cache the contents of your gem dependencies.
      - We will invalidate the dependency cache if your stack changes.
      - We will invalidate the dependency cache if your Ruby version changes.
      - We may invalidate the dependency cache if there was a bug in a prior buildpack version that needs to be fixed.
- Gem specific behavior - We will parse your `Gemfile.lock` to determine what dependencies your app need for use in specializing your install behavior (i.e. Rails 5 versus Rails 4). The inclusion of these gems may trigger different behavior:
  - `railties`
- Applications without `rake` in the `Gemfile.lock` or a `Rakefile` variant MAY skip rake task detection.
- Rake execution - We will determine what rake tasks are runnable via the output of `rake -P` against your application.
  - We WILL abort the build if the `rake -p` task fails.
  - We will run `rake assets:precompile` on your app if that task exists on your application.
    - We will skip this `assets:precompile` task if a manifest file exists in the `public/assets` folder that indicates precompiled assets are checked into git.
      - ".sprockets-manifest-*.json"
      - "manifest-*.json"
    - We will abort your build if the `rake assets:precompile` task fails
    - We will run `rake assets:clean` on your app. (Reference: https://github.com/heroku/buildpacks-ruby/blob/d526a49f81becaf571329a1adf5fff0668a6b99a/lib/heroku_buildpack_ruby/assets_precompile.rb#L67-L73, reference: https://github.com/heroku/heroku-buildpack-ruby/blob/951fc728979695990c32df2d4d60ae2d6f6f61c2/lib/language_pack/rails4.rb#L83-L94)
      - We will cache the contents of `public/assets` if `assets:clean` exists on your application. (pending https://github.com/buildpacks/spec/blob/main/buildpack.md#slice-layers support in libcnbrs)
      - We will cache asset "fragments" directories if the `assets:clean` exists on the system. (pending https://github.com/buildpacks/spec/blob/main/buildpack.md#slice-layers support in libcnbrs)
        - We will limit or prune the size of the asset cache in `tmp/cache/assets` to 100 MiB.
- Process types
  - Given an application with the `railties` gem we will run `bin/rails server` while specifying `-p $PORT` and `-e $RAILS_ENV"` by default as the `web` process. Use the `Procfile` to override.
  - If the `railties` gem is not present we will run `rackup` while specifying `-p $PORT` and `-h 0.0.0.0` by default as the `web` process. Use the `Procfile` to override. This requires the `rack` gem and a `config.ru` file.
- Environment variable defaults - We will set a default for the following environment variables:
  - `JRUBY_OPTS="-Xcompile.invokedynamic=false"` - Invoke dynamic is a feature of the JVM intended to enhance support for dynamicaly typed languages (such as Ruby). This caused issues with Physion Passenger 4.0.16 and was disabled [details](https://github.com/heroku/heroku-buildpack-ruby/issues/145).
  - `RACK_ENV=${RACK_ENV:-"production"}` - An environment variable that may affect the behavior of Rack based webservers and webapps.
  - `RAILS_ENV=${RAILS_ENV:-"production"}` - A value used by all Rails apps. By default Rails ships with three environments: `development`, `test,` and `production`. We recommend all apps being deployed to use `production` and recommend against using a custom env such as `staging` [details](https://devcenter.heroku.com/articles/deploying-to-a-custom-rails-environment).
  - `SECRET_KEY_BASE=${SECRET_KEY_BASE:-<generate a secret key>}` - In Rails 4.1+ apps a value is needed to generate cryptographic tokens used for a variety of things. Notably this value is used in generating user sessions so modifying it between builds will have the effect of logging out all users. Heroku provides a default generated value.
- Environment variables modified - In addition to the default list this is a list of environment variables that the buildpack modifies:
  - `BUNDLE_BIN=<bundle-path-dir>/bin` - Install executables for all gems into specified path.
  - `BUNDLE_CLEAN=1` - After successful `bundle install` bundler will automatically run `bundle clean`.
  - `BUNDLE_DEPLOYMENT=1` - Requires the `Gemfile.lock` to be in sync with the current `Gemfile`.
  - `BUNDLE_GEMFILE=<app-dir>/Gemfile` - Tells bundler where to find the `Gemfile`.
  - `BUNDLE_PATH=<bundle-path-dir>` - Directs bundler to install gems to this path
  - `BUNDLE_WITHOUT=development:test:$BUNDLE_WITHOUT` - Do not install `development` or `test` groups via bundle install. Additional groups can be specified via user config.
  - `DISABLE_SPRING="1"` - Spring is a library that attempts to cache application state by forking and manipulating processes with the goal of decreasing development boot time. Disabling it in production removes significant problems [details](https://devcenter.heroku.com/changelog-items/1826).
  - `GEM_PATH=<bundle-path-dir>` - Tells Ruby where gems are located
  - `MALLOC_ARENA_MAX=2` - Controls glibc memory allocation behavior with the goal of decreasing overall memory allocated by Ruby [details](https://devcenter.heroku.com/changelog-items/1683).
  - `PATH` - Various executables are installed and the `PATH` env var will be modified so they can be executed at the system level. This is mostly done via interfaces provided by `libcnb` and CNB layers rather than directly.
  - `RAILS_LOG_TO_STDOUT="enabled"` - Sets the default logging target to STDOUT for Rails 5+ apps. [details](https://blog.heroku.com/container_ready_rails_5)
  - `RAILS_SERVE_STATIC_FILES="enabled"` - Enables the `ActionDispatch::Static` middleware for Rails 5+ apps so that static files such as those in `public/assets` are served by the Ruby webserver such as Puma [details](https://blog.heroku.com/container_ready_rails_5).

### Next-gen application contract

These are tracked things the buildpack will eventually do in the application contract but are not currently priorities.

- [TODO] Warn on invalid Ruby binstub (https://github.com/heroku/heroku-buildpack-ruby/blob/main/lib/language_pack/helpers/binstub_wrapper.rb, and https://github.com/heroku/heroku-buildpack-ruby/blob/main/lib/language_pack/helpers/binstub_check.rb)
- [TODO] Warn on default ruby version (https://github.com/heroku/heroku-buildpack-ruby/blob/26065614274ed1138620806c9a8d705e39b4412c/lib/language_pack/ruby.rb#L532)
- [TODO] Warn on Ruby version not in Gemfile.lock, but present in `bundle platform --ruby`
- [TODO] Warn on outdated ruby version (https://github.com/heroku/heroku-buildpack-ruby/blob/main/lib/language_pack/helpers/outdated_ruby_version.rb)
- [TODO] Run `rails runner` to collect configuration, abort a build if it fails (https://github.com/heroku/heroku-buildpack-ruby/blob/main/lib/language_pack/helpers/rails_runner.rb)
- [TODO] Warn when `RAILS_ENV=staging` (https://github.com/heroku/heroku-buildpack-ruby/blob/2b567c597d5cb110774eb21b9616b311d8e4ee9d/lib/language_pack/rails2.rb#L65-L71)
- [TODO] Warn if we cannot run a Web process
  - [TODO] Warn no Procfile (https://github.com/heroku/heroku-buildpack-ruby/blob/26065614274ed1138620806c9a8d705e39b4412c/lib/language_pack/base.rb#L156-L158)
  - [TODO] Warn Railties, but no `bin/rails` binstub (new)
  - [TODO] Warn no Procfile, no Railties, and missing: `rack` gem or `config.ru` file. (new)
- [TODO-never-fail-rake] Make a buildpack that moves the contents of the Rakefile into another file like `heroku_buildpack_wrapped_rakefile.rb` and then replaces Rake with:

```
begin
  require_relative "heroku_buildpack_wrapped_rakefile.rb"
rescue => e
  STDERR.puts <<~EOM
    Error caught and ignored while loading Rakefile

    To fail the build instead, remove the buildpack TODO

    Message:
    #{e.message}
  EOM
end
```

### Known differences against `heroku/heroku-buildpack-ruby`

This buildpack does not port all behaviors of the [original buildpack for Ruby ](https://github.com/heroku/heroku-buildpack-ruby). This buildpack is also known as "classic". This section outlines the known behaviors where the two buildpacks diverge.

- Rails 5+ support only. The v2 buildpack supports Rails 2+. There are significant maintenance gains for buildpack authors [starting in Rails 5](https://blog.heroku.com/container_ready_rails_5) which was released in 2016. In an effort to reduce overall internal complexity this buildpack does not explicitly support Rails before version 5.
- Rails support is now based on the above application contract. Previously there was no official policy for dropping support for older Rails versions. Support was based on whether or not an older version could run on a currently supported Ruby version. With [Rails LTS](https://railslts.com/) this can mean a very old version of Rails. Now we will actively support Rails versions currently [under the Rails core maintenance policy](https://guides.rubyonrails.org/maintenance_policy.html) provided those versions can run on a supported Ruby version. As Rails versions go out of official maintenance compatibility features may be removed with no supported replacement.
- Default Ruby versions are no longer sticky. Before bundler started recording the Ruby version in the `Gemfile.lock` it was common for developers to forget to declare their Ruby version in their `Gemfile`. When that happens they would receive the default Ruby version. To guard against instability we would record that version and use it on future deploys. Now it's less likely for an app to deploy without a default ruby version, and we do not want to encourage relying on a default for stability (as this otherwise breaks dev-prod parity).
- Ruby versions come from the `Gemfile.lock` only. Before bundler started recording the Ruby version in the `Gemfile.lock`, Heroku would pull the Ruby version via running `bundle platform --ruby` to pull any ruby declaration such as `ruby "3.1.2"` from the `Gemfile`. This creates a bootstrapping problem, because you need a version of Ruby to run `bundle platform` to find the version of Ruby the application needs. Since the Ruby version is now recorded in the `Gemfile.lock`, this extra bootstrapping problem is less needed and applications should rely on their version being in their `Gemfile.lock` instead.
- Bundler version is now based on `Gemfile.lock`. Previously there was a known good version of bundler on the system for Bundler 1.x and Bundler 2.x. With Bundler 2.3+ a feature was added for Bundler to re-exec itself when it found that it was being run with a different version. Also backwards incompatible features were added in patch versions which forced us to upgrade bundler versions for everyone, even when they were less stable. With the new model, you'll get the same version of bundler in production as you use in development. Whatever version is in the `Gemfile.lock` will be installed on the system. The biggest problem users may encounter is that locally they do not install with `BUNDLE_DEPLOYMENT=true` on which raises some edge cases. Developers are not responsible for ensuring their version of bundler is stable and not the cause for their build issue or failure.
- Failure to detect rake tasks will fail a deployment. On all builds `rake -p` is called to find a list of all rake tasks. If this detection task fails then the previous behavior was to fail the build only if the `sprockets` gem was present. The reason was to allow API only apps that don't need to generate assets to have a `Rakefile` that cannot be run in production (the most common reason is they're requiring a library not in their production gemfile group). Now all failure to load a Rakefile will fail. If you want the old behavior you can [TODO](TODO-never-fail-rake). The reason for this change is it's more common that applications will want their builds to fail even if they're not using `sprockets`. It's also just not a good idea to not have your `Rakefile` not runable in production, we shouldn't encourage that pattern.
- Caching of `public/assets` is gated on the presence of `rake assets:clean`. Previously this behavior was gated on the existence of a certain version of the Rails framework.
- Caching of `tmp/cache/assets` (fragments) is gated on the presence of `rake assets:clean`. Previously this behavior was gated on the existence of a certain version of the Rails framework.

## Development

### Setup the project

- Follow setup instructions on https://github.com/Malax/libcnb.rs

### Compile the buildpack

- Run:

```
cd buildpacks/ruby && cargo libcnb package ; cd -
```

### Use that compiled buildpack on an application

To build the application vendored in `buildpacks/ruby/tests/fixtures/default_ruby` you can run:

```
pack build my-image --buildpack target/buildpack/debug/heroku_ruby --path buildpacks/ruby/tests/fixtures/default_ruby --verbose
```

The deployed buildpack ships with a builder that tells the `pack` CLI what other builpacks it needs. In development you must specify them via the `--buildpack` flag before this buildpack.

For example to build an app that needs nodejs:

```
pack build my-image  --buildpack heroku/nodejs-engine --buildpack heroku/procfile  --buildpack target/buildpack/debug/heroku_ruby --path <path/to/application> --verbose
```

List of buildpacks this buildpack depends on:

```
--buildpack heroku/nodejs-engine
--buildpack heroku/nodejs-yarn
--buildpack heroku/jvm
```

### Run the image

Run a command and exit:

```
$ docker run -it --rm --entrypoint='/cnb/lifecycle/launcher' my-image 'which ruby'
/layers/heroku_ruby/ruby/bin
$ docker run -it --rm --entrypoint='/cnb/lifecycle/launcher' my-image 'ruby -v'
ruby 2.7.4p191 (2021-07-07 revision a21a3b7d23) [x86_64-linux]
```

Make sure it doesn't say `/usr/bin/ruby` or another system ruby location

As a oneliner:

```
$(set -pipefail; cd buildpacks/ruby && cargo libcnb package; cd -) &&
docker rmi my-image --force  &&
pack build my-image --buildpack target/buildpack/debug/heroku_ruby --path buildpacks/ruby/tests/fixtures/default_ruby --verbose &&
docker run -it --rm --entrypoint='/cnb/lifecycle/launcher' my-image 'which bundle'
```

Run it interactively:

```
docker run -it --rm --entrypoint='/cnb/lifecycle/launcher' my-image bash
```

Run the webserver:

```
docker run -it --rm --env PORT=9292 -p 9292:9292 my-image
```


Inspect the image:

```
pack inspect my-image
```
