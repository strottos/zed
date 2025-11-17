use anyhow::Context;
use clap::Parser;

use fs::RealFs;
use gpui::Application;
use release_channel::{AppCommitSha, AppVersion};
use settings::watch_config_file;
use util::ResultExt;

use std::{
    collections::HashMap,
    io,
    path::Path,
    process,
    sync::{Arc, OnceLock},
    time::Instant,
};

#[derive(Parser, Debug)]
#[command(name = "zedtui", disable_version_flag = true, max_term_width = 100)]
struct Args {
    /// A sequence of space-separated paths or urls that you want to open.
    ///
    /// Use `path:line:row` syntax to open a file at a specific location.
    /// Non-existing paths and directories will ignore `:line:row` suffix.
    ///
    /// URLs can either be `file://` or `zed://` scheme, or relative to <https://zed.dev>.
    paths_or_urls: Vec<String>,

    /// Pairs of file paths to diff. Can be specified multiple times.
    #[arg(long, action = clap::ArgAction::Append, num_args = 2, value_names = ["OLD_PATH", "NEW_PATH"])]
    diff: Vec<String>,

    /// Sets a custom directory for all user data (e.g., database, extensions, logs).
    ///
    /// This overrides the default platform-specific data directory location.
    /// On macOS, the default is `~/Library/Application Support/Zed`.
    /// On Linux/FreeBSD, the default is `$XDG_DATA_HOME/zed`.
    /// On Windows, the default is `%LOCALAPPDATA%\Zed`.
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    user_data_dir: Option<String>,

    /// Prints system specs.
    ///
    /// Useful for submitting issues on GitHub when encountering a bug that
    /// prevents Zed from starting, so you can't run `zed: copy system specs to
    /// clipboard`
    #[arg(long)]
    system_specs: bool,

    /// Output current environment variables as JSON to stdout
    #[arg(long, hide = true)]
    printenv: bool,
}

fn init_paths() -> HashMap<io::ErrorKind, Vec<&'static Path>> {
    [
        paths::config_dir(),
        paths::extensions_dir(),
        paths::languages_dir(),
        paths::debug_adapters_dir(),
        paths::database_dir(),
        paths::logs_dir(),
        paths::temp_dir(),
        paths::hang_traces_dir(),
    ]
    .into_iter()
    .fold(HashMap::default(), |mut errors, path| {
        if let Err(e) = std::fs::create_dir_all(path) {
            errors.entry(e.kind()).or_insert_with(Vec::new).push(path);
        }
        errors
    })
}

fn files_not_created_on_launch(errors: HashMap<io::ErrorKind, Vec<&Path>>) {
    let message = "Zed failed to launch";
    let error_details = errors
        .into_iter()
        .flat_map(|(kind, paths)| {
            #[allow(unused_mut)] // for non-unix platforms
            let mut error_kind_details = match paths.len() {
                0 => return None,
                1 => format!(
                    "{kind} when creating directory {:?}",
                    paths.first().expect("match arm checks for a single entry")
                ),
                _many => format!("{kind} when creating directories {paths:?}"),
            };

            #[cfg(unix)]
            {
                if kind == io::ErrorKind::PermissionDenied {
                    error_kind_details.push_str("\n\nConsider using chown and chmod tools for altering the directories permissions if your user has corresponding rights.\
                        \nFor example, `sudo chown $(whoami):staff ~/.config` and `chmod +uwrx ~/.config`");
                }
            }

            Some(error_kind_details)
        })
        .collect::<Vec<_>>().join("\n\n");

    eprintln!("{message}: {error_details}");

    process::exit(1);
}

pub static STARTUP_TIME: OnceLock<Instant> = OnceLock::new();

pub fn main() {
    STARTUP_TIME.get_or_init(|| Instant::now());

    #[cfg(unix)]
    util::prevent_root_execution();

    let args = Args::parse();

    // `zed --printenv` Outputs environment variables as JSON to stdout
    if args.printenv {
        util::shell_env::print_env();
        return;
    }

    // Set custom data directory.
    if let Some(dir) = &args.user_data_dir {
        paths::set_custom_data_dir(dir);
    }

    #[cfg(target_os = "windows")]
    match util::get_zed_cli_path() {
        Ok(path) => askpass::set_askpass_program(path),
        Err(err) => {
            eprintln!("Error: {}", err);
            if std::option_env!("ZED_BUNDLE").is_some() {
                process::exit(1);
            }
        }
    }

    let file_errors = init_paths();
    if !file_errors.is_empty() {
        files_not_created_on_launch(file_errors);
        return;
    }

    zlog::init();
    let result = zlog::init_output_file(paths::log_file(), Some(paths::old_log_file()));
    if let Err(err) = result {
        panic!("Bailing because we could not open log file: {}", err);
    };

    let app_version = AppVersion::load(env!("CARGO_PKG_VERSION"));
    let app_commit_sha =
        option_env!("ZED_COMMIT_SHA").map(|commit_sha| AppCommitSha::new(commit_sha.to_string()));

    if args.system_specs {
        let system_specs = system_specs::SystemSpecs::new_stateless(
            app_version,
            app_commit_sha,
            *release_channel::RELEASE_CHANNEL,
        );
        println!("Zed System Specs (from CLI):\n{}", system_specs);
        return;
    }

    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .stack_size(10 * 1024 * 1024)
        .thread_name(|ix| format!("RayonWorker{}", ix))
        .build_global()
        .unwrap();

    log::info!(
        "========== starting zed version {}, sha {} ==========",
        app_version,
        app_commit_sha
            .as_ref()
            .map(|sha| sha.short())
            .as_deref()
            .unwrap_or("unknown"),
    );

    let app = Application::headless();

    let git_binary_path =
        if cfg!(target_os = "macos") && option_env!("ZED_BUNDLE").as_deref() == Some("true") {
            app.path_for_auxiliary_executable("git")
                .context("could not find git binary path")
                .log_err()
        } else {
            None
        };
    log::info!("Using git binary path: {:?}", git_binary_path);

    let fs = Arc::new(RealFs::new(git_binary_path, app.background_executor()));
    let user_settings_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::settings_file().clone(),
    );
    let global_settings_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::global_settings_file().clone(),
    );
    let user_keymap_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::keymap_file().clone(),
    );
}
