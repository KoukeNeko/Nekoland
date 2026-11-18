//! The shells a tab can start, as Nekominal finds them on Unix: the login shell first, then the other shells of
//! `/etc/shells` once a name, then the hosts of `~/.ssh/config`; after them the user's own. A profile names a program,
//! its arguments and where it starts.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use gtk::glib;

use super::config::{self, Profile};

const FALLBACK_SHELL: &str = "/bin/bash";
const SSH: &str = "ssh";
/// What an agent run in a tab is told, as Nekominal tells it: links may be printed, and Claude Code may leave the
/// cursor to the terminal.
pub const AGENT_ENVIRONMENT: [(&str, &str); 2] = [("FORCE_HYPERLINK", "1"), ("CLAUDE_CODE_NATIVE_CURSOR", "1")];
/// What a clean environment keeps of the one the terminal started with: who and where the user is, and how to reach
/// the desktop.
const KEPT_VARIABLES: [&str; 14] = [
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "LANG",
    "LANGUAGE",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
];

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn shell_profile(path: &Path) -> Profile {
    let name = file_name(path);
    // Bash as a login shell, as a terminal starts it, so that the profile is read.
    let args = if name == "bash" { "--login" } else { "" };
    Profile {
        name,
        program: path.display().to_string(),
        args: args.to_string(),
        ..Profile::default()
    }
}

fn listed_shells() -> Vec<PathBuf> {
    fs::read_to_string("/etc/shells")
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| line.starts_with('/'))
                .map(PathBuf::from)
                .filter(|path| is_executable(path))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Host` names of an OpenSSH client config that name one host, not a pattern.
pub fn parse_ssh_hosts(config: &str) -> Vec<String> {
    config
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let keyword = line.get(..4)?;
            let rest = line.get(4..)?;
            (keyword.eq_ignore_ascii_case("host") && rest.starts_with(|c: char| c.is_whitespace() || c == '='))
                .then_some(rest.trim_start_matches(|c: char| c.is_whitespace() || c == '='))
        })
        .flat_map(str::split_whitespace)
        .filter(|host| !host.contains(['*', '?', '!']))
        .map(str::to_string)
        .collect()
}

fn ssh_hosts() -> Vec<String> {
    fs::read_to_string(glib::home_dir().join(".ssh/config"))
        .map(|text| parse_ssh_hosts(&text))
        .unwrap_or_default()
}

/// The shells found on the system, each name once.
pub fn discover() -> Vec<Profile> {
    let login = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| is_executable(path))
        .unwrap_or_else(|| PathBuf::from(FALLBACK_SHELL));
    let mut profiles: Vec<Profile> = Vec::new();
    for shell in std::iter::once(login).chain(listed_shells()) {
        let profile = shell_profile(&shell);
        if !profiles.iter().any(|known| known.name == profile.name) {
            profiles.push(profile);
        }
    }
    for host in ssh_hosts() {
        profiles.push(Profile {
            name: host.clone(),
            program: SSH.to_string(),
            args: host,
            ..Profile::default()
        });
    }
    profiles
}

/// Every profile: the ones found, then the user's, a user's profile taking the place of a found one of its name.
pub fn all() -> Vec<Profile> {
    let own = config::current().profiles;
    let mut profiles: Vec<Profile> = discover()
        .into_iter()
        .filter(|found| !own.iter().any(|profile| profile.name == found.name))
        .collect();
    profiles.extend(own);
    profiles
}

/// The profile named so, or the default one, or the first found.
pub fn named(name: Option<&str>) -> Profile {
    named_in(&all(), name)
}

/// Whether a profile of this name was found on the system, so that taking the user's out puts that one back.
pub fn is_found(name: &str) -> bool {
    discover().iter().any(|profile| profile.name == name)
}

/// Of `profiles`, the one named so, or the default one, or the first.
pub fn named_in(profiles: &[Profile], name: Option<&str>) -> Profile {
    let wanted = name.map_or_else(|| config::current().default_profile, str::to_string);
    profiles
        .iter()
        .find(|profile| profile.name == wanted)
        .or_else(|| profiles.first())
        .cloned()
        .unwrap_or_else(|| shell_profile(Path::new(FALLBACK_SHELL)))
}

/// The arguments, split at spaces, a quoted part kept whole.
pub fn split_args(args: &str) -> Vec<String> {
    glib::shell_parse_argv(args)
        .map(|parsed| parsed.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect())
        .unwrap_or_else(|_| args.split_whitespace().map(str::to_string).collect())
}

/// The variables a new shell of `profile` gets, as the settings say: the terminal's own or a clean set, the agents',
/// and the profile's own.
pub fn environment(profile: &Profile) -> (bool, Vec<(String, String)>) {
    let config = config::current();
    let mut variables: Vec<(String, String)> = Vec::new();
    if !config.inherit_environment {
        for name in KEPT_VARIABLES {
            if let Ok(value) = std::env::var(name) {
                variables.push((name.to_string(), value));
            }
        }
    }
    if config.agent_environment {
        variables.extend(AGENT_ENVIRONMENT.map(|(name, value)| (name.to_string(), value.to_string())));
    }
    variables.extend(profile.env.iter().map(|(name, value)| (name.clone(), value.clone())));
    (!config.inherit_environment, variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hosts_that_name_one_machine_are_profiles() {
        let config = "Host work devbox\n  HostName 10.0.0.2\nHost *.internal\nhost=pi\nHostName nope\n";
        assert_eq!(parse_ssh_hosts(config), vec!["work", "devbox", "pi"]);
    }

    #[test]
    fn quoted_arguments_stay_whole() {
        assert_eq!(split_args(r#"-c "echo hi""#), vec!["-c", "echo hi"]);
        assert!(split_args("").is_empty());
    }
}
