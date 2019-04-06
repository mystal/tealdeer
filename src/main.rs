//! An implementation of [tldr](https://github.com/tldr-pages/tldr) in Rust.
//
// Copyright (c) 2015-2018 tealdeer developers
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. All files in the project carrying such notice may not be
// copied, modified, or distributed except according to those terms.

#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::similar_names)]
#![allow(clippy::stutter)]

#[cfg(feature = "logging")]
extern crate env_logger;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use ansi_term::Color;
use app_dirs::AppInfo;
use docopt::Docopt;
#[cfg(unix)]
use pager::Pager;
use serde_derive::Deserialize;

mod cache;
mod config;
mod error;
mod formatter;
mod tokenizer;
mod types;

use crate::cache::Cache;
use crate::config::{get_config_path, make_default_config, Config};
use crate::error::TealdeerError::{CacheError, ConfigError, UpdateError};
use crate::formatter::print_lines;
use crate::tokenizer::Tokenizer;
use crate::types::OsType;

const NAME: &str = "tealdeer";
const APP_INFO: AppInfo = AppInfo {
    name: NAME,
    author: NAME,
};
const VERSION: &str = env!("CARGO_PKG_VERSION");
const USAGE: &str = "
Usage:

    tldr [options] <command>...
    tldr [options]

Options:

    -h --help           Show this screen
    -v --version        Show version information
    -l --list           List all commands in the cache
    -f --render <file>  Render a specific markdown file
    -o --os <type>      Override the operating system [linux, osx, sunos, windows]
    -u --update         Update the local cache
    -c --clear-cache    Clear the local cache
    -p --pager          Use a pager to page output
    -q --quiet          Suppress informational messages
    --config-path       Show config file path
    --seed-config       Create a basic config

Examples:

    $ tldr tar
    $ tldr --list

To control the cache:

    $ tldr --update
    $ tldr --clear-cache

To render a local file (for testing):

    $ tldr --render /path/to/file.md
";
const ARCHIVE_URL: &str = "https://github.com/tldr-pages/tldr/archive/master.tar.gz";
const PAGER_COMMAND: &str = "less -R";
const MAX_CACHE_AGE: Duration = Duration::from_secs(2_592_000); // 30 days

#[derive(Debug, Deserialize)]
struct Args {
    arg_command: Option<Vec<String>>,
    flag_help: bool,
    flag_version: bool,
    flag_list: bool,
    flag_render: Option<String>,
    flag_os: Option<OsType>,
    flag_update: bool,
    flag_clear_cache: bool,
    flag_pager: bool,
    flag_quiet: bool,
    flag_config_path: bool,
    flag_seed_config: bool,
}

/// Print page by path
fn print_page(path: &Path, enable_styles: bool) -> Result<(), String> {
    // Open file
    let file = File::open(path).map_err(|msg| format!("Could not open file: {}", msg))?;
    let reader = BufReader::new(file);

    // Look up config file, if none is found fall back to default config.
    let config = match Config::load(enable_styles) {
        Ok(config) => config,
        Err(ConfigError(msg)) => {
            eprintln!("Could not load config: {}", msg);
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Could not load config: {}", e);
            process::exit(1);
        }
    };

    // Create tokenizer and print output
    let mut tokenizer = Tokenizer::new(reader);
    print_lines(&mut tokenizer, &config);

    Ok(())
}

/// Set up display pager
#[cfg(unix)]
fn configure_pager(args: &Args, enable_styles: bool) {
    // Flags have precedence
    if args.flag_pager {
        Pager::with_default_pager(PAGER_COMMAND).setup();
        return;
    }

    // Then check config
    let config = match Config::load(enable_styles) {
        Ok(config) => config,
        Err(ConfigError(msg)) => {
            eprintln!("Could not load config: {}", msg);
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Could not load config: {}", e);
            process::exit(1);
        }
    };

    if config.display.use_pager {
        Pager::with_default_pager(PAGER_COMMAND).setup();
    }
}

/// Check the cache for freshness
fn check_cache(args: &Args, cache: &Cache) {
    if !args.flag_update {
        match cache.last_update() {
            Some(ago) if ago > MAX_CACHE_AGE => {
                if args.flag_quiet {
                    return;
                }
                println!(
                    "{}",
                    Color::Red.paint(format!(
                        "Cache wasn't updated for more than {} days.\n\
                         You should probably run `tldr --update` soon.",
                        MAX_CACHE_AGE.as_secs() / 24 / 3600
                    ))
                );
            }
            None => {
                eprintln!("Cache not found. Please run `tldr --update`.");
                process::exit(1);
            }
            _ => {}
        }
    };
}

#[cfg(feature = "logging")]
fn init_log() {
    env_logger::init();
}

#[cfg(not(feature = "logging"))]
fn init_log() {}

#[cfg(target_os = "linux")]
fn get_os() -> OsType {
    OsType::Linux
}

#[cfg(any(target_os = "macos",
          target_os = "freebsd",
          target_os = "netbsd",
          target_os = "openbsd",
          target_os = "dragonfly"))]
fn get_os() -> OsType {
    OsType::OsX
}

#[cfg(target_os = "windows")]
fn get_os() -> OsType {
    OsType::Windows
}

#[cfg(not(any(target_os = "linux",
              target_os = "macos",
              target_os = "freebsd",
              target_os = "netbsd",
              target_os = "openbsd",
              target_os = "dragonfly",
              target_os = "windows")))]
fn get_os() -> OsType {
    OsType::Other
}

fn main() {
    // Initialize logger
    init_log();

    // Parse arguments
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    // Show version and exit
    if args.flag_version {
        let os = get_os();
        println!("{} v{} ({})", NAME, VERSION, os);
        process::exit(0);
    }

	// Determine the usage of styles
    #[cfg(target_os = "windows")]
    let enable_styles = ansi_term::enable_ansi_support().is_ok();
    #[cfg(not(target_os = "windows"))]
    let enable_styles = true;

    // Configure pager
    #[cfg(unix)]
    configure_pager(&args, enable_styles);

    // Specify target OS
    let os: OsType = match args.flag_os {
        Some(os) => os,
        None => get_os(),
    };

    // Initialize cache
    let cache = Cache::new(ARCHIVE_URL, os);

    // Clear cache, pass through
    if args.flag_clear_cache {
        cache.clear().unwrap_or_else(|e| {
            match e {
                CacheError(msg) | ConfigError(msg) | UpdateError(msg) => {
                    eprintln!("Could not delete cache: {}", msg)
                }
            };
            process::exit(1);
        });
        if !args.flag_quiet {
            println!("Successfully deleted cache.");
        }
    }

    // Update cache, pass through
    if args.flag_update {
        cache.update().unwrap_or_else(|e| {
            match e {
                CacheError(msg) | ConfigError(msg) | UpdateError(msg) => {
                    eprintln!("Could not update cache: {}", msg)
                }
            };
            process::exit(1);
        });
        if !args.flag_quiet {
            println!("Successfully updated cache.");
        }
    }

    // Show config file and path, pass through
    if args.flag_config_path {
        match get_config_path() {
            Ok(config_file_path) => {
                println!("Config path is: {}", config_file_path.to_str().unwrap());
            }
            Err(ConfigError(msg)) => {
                eprintln!("Could not look up config_path: {}", msg);
                process::exit(1);
            }
            Err(_) => {
                eprintln!("Unknown error");
                process::exit(1);
            }
        }
    }

    // Create a basic config and exit
    if args.flag_seed_config {
        match make_default_config() {
            Ok(config_file_path) => {
                println!(
                    "Successfully created seed config file here: {}",
                    config_file_path.to_str().unwrap()
                );
                process::exit(0);
            }
            Err(ConfigError(msg)) => {
                eprintln!("Could not create seed config: {}", msg);
                process::exit(1);
            }
            Err(_) => {
                eprintln!("Unkown error");
                process::exit(1);
            }
        }
    }


    // Render local file and exit
    if let Some(ref file) = args.flag_render {
        let path = PathBuf::from(file);
        if let Err(msg) = print_page(&path, enable_styles) {
            eprintln!("{}", msg);
            process::exit(1);
        } else {
            process::exit(0);
        };
    }

    // List cached commands and exit
    if args.flag_list {
        // Check cache for freshness
        check_cache(&args, &cache);

        // Get list of pages
        let pages = cache.list_pages().unwrap_or_else(|e| {
            match e {
                CacheError(msg) | ConfigError(msg) | UpdateError(msg) => {
                    eprintln!("Could not get list of pages: {}", msg)
                }
            }
            process::exit(1);
        });

        // Print pages
        println!("{}", pages.join(", "));
        process::exit(0);
    }

    // Show command from cache
    if let Some(ref command) = args.arg_command {
        let command = command.join("-");
        // Check cache for freshness
        check_cache(&args, &cache);

        // Search for command in cache
        if let Some(path) = cache.find_page(&command) {
            if let Err(msg) = print_page(&path, enable_styles) {
                eprintln!("{}", msg);
                process::exit(1);
            } else {
                process::exit(0);
            }
        } else {
            if !args.flag_quiet {
                println!("Page {} not found in cache", &command);
                println!("Try updating with `tldr --update`, or submit a pull request to:");
                println!("https://github.com/tldr-pages/tldr");
            }
            process::exit(1);
        }
    }

    // Some flags can be run without a command.
    if !(args.flag_update || args.flag_clear_cache || args.flag_config_path) {
        eprintln!("{}", USAGE);
        process::exit(1);
    }
}

#[cfg(test)]
mod test {
    use docopt::{Docopt, Error};
    use crate::{Args, OsType, USAGE};

    fn test_helper(argv: &[&str]) -> Result<Args, Error> {
        Docopt::new(USAGE).and_then(|d| d.argv(argv.iter()).deserialize())
    }

    #[test]
    fn test_docopt_os_case_insensitive() {
        let argv = vec!["cp", "--os", "LiNuX"];
        let os = test_helper(&argv).unwrap().flag_os.unwrap();
        assert_eq!(OsType::Linux, os);
    }

    #[test]
    fn test_docopt_expect_error() {
        let argv = vec!["cp", "--os", "lindows"];
        assert!(!test_helper(&argv).is_ok());
    }
}
