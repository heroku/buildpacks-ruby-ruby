use crate::{DetectError, RubyBuildpackError};
use bullet_stream::{state::Bullet, state::SubBullet, style, Print};
use fun_run::{CmdError, CommandWithName};
use indoc::formatdoc;
use std::io::Stdout;
use std::process::Command;
const DEBUG_INFO_STR: &str = "Debug info";

pub(crate) fn on_error(err: libcnb::Error<RubyBuildpackError>) {
    let output = Print::new(std::io::stdout()).without_header();
    let debug_info = style::important(DEBUG_INFO_STR);
    match cause(err) {
        Cause::OurError(error) => log_our_error(output, error),
        Cause::FrameworkError(error) =>
            output
            .bullet(&debug_info)
            .sub_bullet(error.to_string())
            .error(formatdoc! {"
                Error: heroku/buildpack-ruby internal buildpack error

                The framework used by this buildpack encountered an unexpected error.
                This type of error usually indicates there's nothing wrong with your application.

                If you can’t deploy to Heroku due to this issue please check the official Heroku
                status page https://status.heroku.com/ to see if there is an ongoing incident. Once
                all incidents have resolved please retry your build.

                If the issue persists, try to reproduce the behavior locally using the `pack` CLI.
                If you can reproduce the behavior locally and believe you've found a bug in the
                buildpack or framework, please visit the buildpack's GitHub repository at
                https://github.com/heroku/buildpacks-ruby/issues. Search for any existing issues
                related to this error. If none are found, please consider opening a new issue.

                For more details on expected behavior, see the application contract at
                https://github.com/heroku/buildpacks-ruby/blob/main/docs/application_contract.md
                If you notice a difference between the contract and actual buildpack behavior,
                please open an issue with a minimal application to reproduce the problem.

                For application-specific support, consider asking on official Heroku support
                channels or Stack Overflow.
            "}),
    };
}

#[allow(clippy::too_many_lines)]
fn log_our_error(mut output: Print<Bullet<Stdout>>, error: RubyBuildpackError) {
    let git_branch_url =
        style::url("https://devcenter.heroku.com/articles/git#deploy-from-a-branch-besides-main");
    let ruby_versions_url =
        style::url("https://devcenter.heroku.com/articles/ruby-support#ruby-versions");
    let rubygems_status_url = style::url("https://status.rubygems.org/");
    let debug_info = style::important(DEBUG_INFO_STR);

    match error {
        RubyBuildpackError::BuildpackDetectionError(DetectError::Gemfile(error)) => {
            output.error(formatdoc! {"
                Error: `Gemfile` found with error

                There was an error trying to read the contents of the application's Gemfile. \
                The buildpack cannot continue if the Gemfile is unreadable.

                {error}

                Debug using the above information and try again.
            "});
        }
        RubyBuildpackError::BuildpackDetectionError(DetectError::PackageJson(error)) => {
            output.error(formatdoc! {"
                Error: `package.json` found with error

                The Ruby buildpack detected a package.json file but it is not readable \
                due to the following errors:

                {error}

                If your application does not need any node dependencies installed, \
                you may delete this file and try again.

                If you are expecting node dependencies to be installed, please \
                debug using the above information and try again.
            "});
        }
        RubyBuildpackError::BuildpackDetectionError(DetectError::GemfileLock(error)) => {
            output.error(formatdoc! {"
                Error: `Gemfile.lock` found with error

                There was an error trying to read the contents of the application's Gemfile.lock. \
                The buildpack cannot continue if the Gemfile is unreadable.

                {error}

                Debug using the above information and try again.
            "});
        }
        RubyBuildpackError::BuildpackDetectionError(DetectError::YarnLock(error)) => {
            output.error(formatdoc! {"
                Error: `yarn.lock` found with error

                The Ruby buildpack detected a yarn.lock file but it is not readable \
                due to the following errors:

                {error}

                If your application does not need yarn installed, you \
                may delete this file and try again.

                If you are expecting yarn to be installed, please \
                debug using the above information and try again.
            "});
        }
        RubyBuildpackError::MissingGemfileLock(path, error) => {
            output = output
                .bullet(format!(
                    "Could not find {}, details:",
                    style::value(path.to_string_lossy())
                ))
                .sub_bullet(error.to_string())
                .done();

            if let Some(dir) = path.parent() {
                output = debug_cmd(
                    output.bullet(format!(
                        "{debug_info} Contents of the {} directory",
                        style::value(dir.to_string_lossy())
                    )),
                    Command::new("ls").args(["la", &dir.to_string_lossy()]),
                );
            }

            output.error(formatdoc! {"
                Error: `Gemfile.lock` not found

                A `Gemfile.lock` file is required and was not found in the root of your application.

                If you have a `Gemfile.lock` in your application, ensure it is tracked in Git and
                that you’re pushing the correct branch.

                For more information:
                {git_branch_url}
            "});
        }
        RubyBuildpackError::RubyInstallError(error) => {
            // Future:
            // - In the future use a manifest file to list if version is available on a different stack
            // - In the future add a "did you mean" Levenshtein distance to see if they typoed like "3.6.0" when they meant "3.0.6"
            output.bullet(debug_info)
                .sub_bullet(error.to_string())
                .error(formatdoc! {"
                    Error installing Ruby

                    Could not install the detected Ruby version. Ensure that you're using a supported
                    ruby version and try again.

                    Supported ruby versions:
                    {ruby_versions_url}
                "});
        }
        RubyBuildpackError::GemInstallBundlerCommandError(error) => {
            output = output
                .bullet(&debug_info)
                .sub_bullet(error.to_string())
                .done();

            output = debug_cmd(output.bullet(&debug_info), Command::new("gem").arg("env"));

            output.error(formatdoc! {"
                Error installing bundler

                The ruby package managment tool, `bundler`, failed to install. Bundler is required
                to install your application's dependencies listed in the `Gemfile`.

                Check the status page of RubyGems.org:
                {rubygems_status_url}

                Once all incidents have been resolved, please retry your build.
            "});
        }
        RubyBuildpackError::BundleInstallCommandError(error) => {
            // Future:
            // - Grep error output for common things like using sqlite3, use classic buildpack
            let local_command = local_command_debug(&error);
            output
                .bullet(&debug_info)
                .sub_bullet(error.to_string())
                .done()
                .error(formatdoc! {"
                    Error installing your applications's dependencies

                    Could not install gems to the system via bundler. Gems are dependencies
                    your application listed in the `Gemfile` and resolved in the `Gemfile.lock`.

                    {local_command}

                    If you believe that your application is correct, ensure all files are tracked in Git and
                    that you’re pushing the correct branch:
                    {git_branch_url}

                    Use the information above to debug further.
                "});
        }
        RubyBuildpackError::BundleInstallDigestError(path, error) => {
            output = output
                .bullet(&debug_info)
                .sub_bullet(error.to_string())
                .done();

            if let Some(dir) = path.parent() {
                output = debug_cmd(
                    output.bullet(format!(
                        "{debug_info} Contents of the {} directory",
                        style::value(dir.to_string_lossy())
                    )),
                    Command::new("ls").args(["la", &dir.to_string_lossy()]),
                );
            }

            output.error(formatdoc! {"
                Error generating file digest

                An error occurred while generating a file digest. To provide the fastest possible
                install experience, the Ruby buildpack converts your `Gemfile` and `Gemfile.lock`
                into a digest to use in cache invalidation.

                Ensure that the permissions on the files in your application directory are correct and that
                all symlinks correctly resolve.

                If you're unable to resolve this error, you can disable the the digest feature by
                setting the environment variable:

                HEROKU_SKIP_BUNDLE_DIGEST=1
            "});
        }
        RubyBuildpackError::RakeDetectError(error) => {
            // Future:
            // - Annotate with information on requiring test or development only gems in the Rakefile
            let local_command = local_command_debug(&error);
            output
                .bullet(debug_info)
                .sub_bullet(error.to_string())
                .done()
                .error(formatdoc! {"
                    Error detecting rake tasks

                    The Ruby buildpack uses rake task information from your application to guide
                    build logic. Without this information, the Ruby buildpack cannot continue.

                    {local_command}

                    Use the information above to debug further.
                "});
        }
        RubyBuildpackError::RakeAssetsPrecompileFailed(error) => {
            let local_command = local_command_debug(&error);
            output
                .bullet(debug_info)
                .sub_bullet(error.to_string())
                .done()
                .error(formatdoc! {"
                    Error compiling assets

                    An error occured while compiling assets via rake command.

                    {local_command}

                    Use the information above to debug further.
                "});
        }
        RubyBuildpackError::InAppDirCacheError(error) => {
            // Future:
            // - Separate between failures in layer dirs or in app dirs, if we can isolate to an app dir we could debug more
            // to determine if there's bad permissions or bad file symlink
            output
                .bullet(debug_info)
                .sub_bullet(error.to_string())
                .done()
                .error(formatdoc! {"
                    Error caching frontend assets

                    An error occurred while attempting to cache frontend assets, and the Ruby buildpack
                    cannot continue.

                    Ensure that the permissions on the files in your application directory are correct and that
                    all symlinks correctly resolve.
                "});
        }
        RubyBuildpackError::GemListGetError(error) => {
            output = output
                .bullet(&debug_info)
                .sub_bullet(error.to_string())
                .done();

            output = debug_cmd(output.bullet(&debug_info), Command::new("gem").arg("env"));
            output = debug_cmd(
                output.bullet(&debug_info),
                Command::new("bundle").arg("env"),
            );

            output.error(formatdoc! {"
                Error detecting dependencies

                The Ruby buildpack requires information about your application’s dependencies to
                complete the build. Without this information, the Ruby buildpack cannot continue.

                Use the information above to debug further.
            "});
        }
        RubyBuildpackError::MetricsAgentError(error) => {
            output
                .bullet(debug_info)
                .sub_bullet(error.to_string())
                .done()
                .error(formatdoc! {"
                    Error: Could not install Statsd agent

                    An error occured while downloading and installing the metrics agent
                    the buildpack cannot continue.
                "});
        }
    }
}

#[derive(Debug)]
enum Cause {
    OurError(RubyBuildpackError),
    FrameworkError(libcnb::Error<RubyBuildpackError>),
}

fn cause(err: libcnb::Error<RubyBuildpackError>) -> Cause {
    match err {
        libcnb::Error::BuildpackError(err) => Cause::OurError(err),
        err => Cause::FrameworkError(err),
    }
}

fn local_command_debug(error: &CmdError) -> String {
    let cmd_name = replace_app_path_with_relative(style::command(error.name()));

    formatdoc! {"
        Ensure you can run the following command locally with no errors before attempting another build:

        {cmd_name}

    "}
}

fn replace_app_path_with_relative(contents: impl AsRef<str>) -> String {
    let app_path_re = regex::Regex::new("/workspace/").expect("Internal error: regex");

    app_path_re.replace_all(contents.as_ref(), "./").to_string()
}

fn debug_cmd(mut log: Print<SubBullet<Stdout>>, command: &mut Command) -> Print<Bullet<Stdout>> {
    let result = log.stream_with(
        format!("Running debug command {}", style::command(command.name())),
        |stdout, stderr| command.stream_output(stdout, stderr),
    );
    match result {
        Ok(_) => log.done(),
        Err(e) => log.sub_bullet(e.to_string()).done(),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_relative_path() {
        let expected = r#"BUNDLE_DEPLOYMENT="1" BUNDLE_GEMFILE="./Gemfile" BUNDLE_WITHOUT="development:test" bundle install"#;
        let actual = replace_app_path_with_relative(
            r#"BUNDLE_DEPLOYMENT="1" BUNDLE_GEMFILE="/workspace/Gemfile" BUNDLE_WITHOUT="development:test" bundle install"#,
        );
        assert_eq!(expected, &actual);
    }
}
