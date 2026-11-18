//! A `.deb` package file opened in Market: what it says of itself, how it stands to the version
//! already installed, and what apt would install or remove along with it. apt installs it
//! from the file, so that its dependencies come from the system's repositories.

use std::{
    cmp::Ordering,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use crate::sources::{self, Plan};

/// apt and dpkg speak English under it, so that what they say can be read, in UTF-8 so that
/// a package's own scripts still read and write any text.
const PLAIN_LOCALE: &str = "C.UTF-8";
const PLAIN_LOCALE_ASSIGNMENT: &str = "LC_ALL=C.UTF-8";
/// The architecture of a package that runs on any processor.
const ANY_ARCHITECTURE: &str = "all";
const KIB: u64 = 1024;
const MISSING_FILE: &str = "找不到檔案";
const NOT_A_PACKAGE: &str = "檔案損毀或不是 .deb 套件";

/// The fields of a package's control file that Market shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub summary: String,
    pub description: String,
    /// Who made the package, without their address.
    pub maintainer: String,
    pub homepage: String,
    /// What it takes once installed, in KiB.
    pub installed_size: Option<u64>,
}

impl Package {
    /// What it takes once installed, as Market writes sizes; the file's own size where the
    /// package does not say.
    pub fn size(&self, file_bytes: u64) -> String {
        sources::human_size(self.installed_size.map_or(file_bytes, |kib| kib * KIB))
    }
}

/// A package apt would install or upgrade along with the file's.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub name: String,
    pub version: String,
    /// The version it replaces, when it is an upgrade.
    pub from: Option<String>,
}

/// What installing the file would do to other packages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub installs: Vec<Change>,
    pub removes: Vec<String>,
}

/// How the file's version stands to the one installed.
#[derive(Clone, Debug, PartialEq)]
pub enum Standing {
    New,
    /// The file is newer than the version installed, which it names.
    Upgrade(String),
    Same,
    /// The file is older than the version installed, which it names.
    Downgrade(String),
}

/// Everything Market knows of a package file once it is read.
#[derive(Clone, Debug)]
pub struct Inspection {
    pub package: Package,
    pub file_bytes: u64,
    pub installed: Option<String>,
    /// What apt would install and remove with it, or why it cannot.
    pub changes: Result<Changes, String>,
    /// This computer runs programs of the package's architecture.
    pub fits: bool,
    /// The desktop entries the installed package put on the system, to open it with.
    pub entries: Vec<PathBuf>,
}

impl Inspection {
    pub fn standing(&self) -> Standing {
        standing(self.installed.as_deref(), &self.package.version)
    }
}

/// Reads a package file and asks dpkg and apt about it.
pub fn inspect(path: &Path) -> Result<Inspection, String> {
    let package = read_package(path)?;
    let file_bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or_default();
    let installed = sources::installed_versions(std::iter::once(package.name.as_str()))
        .remove(&package.name);
    let fits = architecture_fits(&package.architecture, &machine_architectures());
    // apt would plan an install that dpkg then refuses for its architecture.
    let changes = if fits {
        simulate(path, &package.name)
    } else {
        Ok(Changes::default())
    };
    let entries = if installed.is_some() {
        desktop_entries(&package.name)
    } else {
        Vec::new()
    };
    Ok(Inspection {
        package,
        file_bytes,
        installed,
        changes,
        fits,
        entries,
    })
}

fn read_package(path: &Path) -> Result<Package, String> {
    if !path.exists() {
        return Err(MISSING_FILE.into());
    }
    let out = plain_command("dpkg-deb")
        .arg("-f")
        .arg(path)
        .output()
        .map_err(|err| format!("dpkg-deb: {err}"))?;
    if !out.status.success() {
        return Err(sources::explain_failure(&String::from_utf8_lossy(&out.stderr)));
    }
    parse_control(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| NOT_A_PACKAGE.into())
}

fn plain_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env("LC_ALL", PLAIN_LOCALE);
    command
}

/// A control file's fields, each continuation line kept on a line of its own.
fn control_fields(text: &str) -> HashMap<String, String> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(value) = current.as_ref().and_then(|name| fields.get_mut(name)) {
                value.push('\n');
                value.push_str(&line[1..]);
            }
        } else if let Some((name, value)) = line.split_once(':') {
            current = Some(name.to_string());
            fields.insert(name.to_string(), value.trim().to_string());
        }
    }
    fields
}

/// The package a control file describes, as `dpkg-deb -f` prints it.
pub fn parse_control(text: &str) -> Option<Package> {
    let fields = control_fields(text);
    let field = |name: &str| fields.get(name).cloned().unwrap_or_default();
    let (name, version) = (field("Package"), field("Version"));
    if name.is_empty() || version.is_empty() {
        return None;
    }
    let description = field("Description");
    let (summary, long) = description.split_once('\n').unwrap_or((&description, ""));
    Some(Package {
        name,
        version,
        architecture: field("Architecture"),
        summary: summary.trim().to_string(),
        description: long_description(long),
        maintainer: without_address(&field("Maintainer")),
        homepage: field("Homepage"),
        installed_size: field("Installed-Size").parse().ok(),
    })
}

/// A long description as text: its lines run together into paragraphs, `.` between them,
/// and the lines indented further kept as they are, as Debian's policy has them read.
fn long_description(lines: &str) -> String {
    let mut text = String::new();
    for line in lines.lines() {
        if line.trim() == "." {
            text.push_str("\n\n");
        } else if line.starts_with(' ') {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line.trim());
        } else {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(line.trim());
        }
    }
    text.trim().to_string()
}

fn without_address(person: &str) -> String {
    person
        .split_once('<')
        .map_or(person, |(name, _)| name)
        .trim()
        .to_string()
}

/// Compares two Debian versions as dpkg does: the epoch, then the upstream version, then the
/// revision, each part by its runs of letters (`~` before nothing before letters before the
/// rest) and of digits (by their numbers).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (epoch_a, upstream_a, revision_a) = split_version(a);
    let (epoch_b, upstream_b, revision_b) = split_version(b);
    epoch_a
        .cmp(&epoch_b)
        .then_with(|| compare_part(upstream_a, upstream_b))
        .then_with(|| compare_part(revision_a, revision_b))
}

fn split_version(version: &str) -> (u64, &str, &str) {
    let (epoch, rest) = match version.split_once(':') {
        Some((epoch, rest)) if epoch.chars().all(|c| c.is_ascii_digit()) => {
            (epoch.parse().unwrap_or_default(), rest)
        }
        _ => (0, version),
    };
    let (upstream, revision) = rest.rsplit_once('-').unwrap_or((rest, ""));
    (epoch, upstream, revision)
}

fn compare_part(mut a: &str, mut b: &str) -> Ordering {
    while !a.is_empty() || !b.is_empty() {
        let (letters_a, rest_a) = split_run(a, |c| !c.is_ascii_digit());
        let (letters_b, rest_b) = split_run(b, |c| !c.is_ascii_digit());
        let letters = compare_letters(letters_a, letters_b);
        if letters != Ordering::Equal {
            return letters;
        }
        let (digits_a, rest_a) = split_run(rest_a, |c| c.is_ascii_digit());
        let (digits_b, rest_b) = split_run(rest_b, |c| c.is_ascii_digit());
        let digits = compare_digits(digits_a, digits_b);
        if digits != Ordering::Equal {
            return digits;
        }
        (a, b) = (rest_a, rest_b);
    }
    Ordering::Equal
}

fn split_run(text: &str, belongs: impl Fn(char) -> bool) -> (&str, &str) {
    let end = text.find(|c: char| !belongs(c)).unwrap_or(text.len());
    text.split_at(end)
}

fn compare_letters(a: &str, b: &str) -> Ordering {
    let (mut chars_a, mut chars_b) = (a.chars(), b.chars());
    loop {
        match (chars_a.next(), chars_b.next()) {
            (None, None) => return Ordering::Equal,
            (left, right) => {
                let order = letter_order(left).cmp(&letter_order(right));
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Where a character sorts in a version: `~` before the end of the run, the end before
/// letters, letters before anything else.
fn letter_order(c: Option<char>) -> i64 {
    const AFTER_LETTERS: i64 = 256;
    match c {
        Some('~') => -1,
        None => 0,
        Some(c) if c.is_ascii_alphabetic() => c as i64,
        Some(c) => c as i64 + AFTER_LETTERS,
    }
}

/// Compares runs of digits by their numbers, however long they are.
fn compare_digits(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

pub fn standing(installed: Option<&str>, version: &str) -> Standing {
    let Some(installed) = installed else {
        return Standing::New;
    };
    match compare_versions(version, installed) {
        Ordering::Greater => Standing::Upgrade(installed.to_string()),
        Ordering::Equal => Standing::Same,
        Ordering::Less => Standing::Downgrade(installed.to_string()),
    }
}

/// The architectures this computer runs packages of: its own, then those dpkg was told to
/// take as well.
fn machine_architectures() -> Vec<String> {
    ["--print-architecture", "--print-foreign-architectures"]
        .iter()
        .filter_map(|flag| plain_command("dpkg").arg(flag).output().ok())
        .flat_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|arch| !arch.is_empty())
        .collect()
}

pub fn architecture_fits(architecture: &str, machine: &[String]) -> bool {
    architecture == ANY_ARCHITECTURE || machine.iter().any(|arch| arch == architecture)
}

/// What `apt-get --simulate install` would do besides installing the file's own package.
fn simulate(path: &Path, own: &str) -> Result<Changes, String> {
    let out = plain_command("apt-get")
        .args(["--simulate", "install"])
        .arg(path)
        .output()
        .map_err(|err| format!("apt-get: {err}"))?;
    let said = String::from_utf8_lossy(&out.stdout);
    if out.status.success() {
        return Ok(parse_simulation(&said, own));
    }
    Err(sources::explain_failure(&format!(
        "{said}\n{}",
        String::from_utf8_lossy(&out.stderr)
    )))
}

/// The `Inst` and `Remv` lines of a simulation: `Inst name [old] (version origin [arch])`, the
/// old version there only for an upgrade, and `Remv name [old]`.
pub fn parse_simulation(text: &str, own: &str) -> Changes {
    let mut changes = Changes::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Inst ") {
            let Some(change) = parse_install(rest) else { continue };
            if change.name != own {
                changes.installs.push(change);
            }
        } else if let Some(rest) = line.strip_prefix("Remv ") {
            if let Some(name) = rest.split(' ').next().map(bare_name) {
                changes.removes.push(name.to_string());
            }
        }
    }
    changes
}

fn parse_install(rest: &str) -> Option<Change> {
    let (name, rest) = rest.split_once(' ')?;
    let (from, rest) = match rest.strip_prefix('[') {
        Some(after) => {
            let (old, rest) = after.split_once(']')?;
            (Some(old.to_string()), rest.trim_start())
        }
        None => (None, rest),
    };
    let version = rest.strip_prefix('(')?.split(' ').next()?;
    Some(Change {
        name: bare_name(name).to_string(),
        version: version.to_string(),
        from,
    })
}

/// A package's name without the `:arch` apt adds to one of another architecture.
fn bare_name(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

/// The desktop entries an installed package put where applications are listed.
fn desktop_entries(name: &str) -> Vec<PathBuf> {
    plain_command("dpkg-query")
        .args(["-L", name])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|path| path.contains("/applications/") && path.ends_with(".desktop"))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Installs the file with apt, which fetches what it depends on: again over the same version,
/// and over a newer one only when asked to.
pub fn install_plan(path: &Path, standing: &Standing) -> Plan {
    let mut command: Vec<String> = std::iter::once("env")
        .chain([PLAIN_LOCALE_ASSIGNMENT])
        .chain(sources::APT_GET[1..].iter().copied())
        .chain(["install"])
        .map(str::to_string)
        .collect();
    match standing {
        Standing::Same => command.push("--reinstall".into()),
        Standing::Downgrade(_) => command.push("--allow-downgrades".into()),
        Standing::New | Standing::Upgrade(_) => {}
    }
    command.push(path.to_string_lossy().into_owned());
    Plan {
        admin: vec![command],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "Package: hello\nVersion: 2.12.3-1\nArchitecture: amd64\nMaintainer: Ubuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\nInstalled-Size: 132\nDepends: libc6 (>= 2.34)\nHomepage: https://www.gnu.org/software/hello/\nDescription: example package based on GNU hello\n The GNU hello program produces a familiar, friendly greeting.  It\n allows non-programmers to use a classic computer science tool.\n .\n Seriously, though: this is an example.\n  verbatim line\n";

    #[test]
    fn control_fields_become_the_package() {
        let package = parse_control(HELLO).unwrap();
        assert_eq!(package.name, "hello");
        assert_eq!(package.version, "2.12.3-1");
        assert_eq!(package.architecture, "amd64");
        assert_eq!(package.maintainer, "Ubuntu Developers");
        assert_eq!(package.summary, "example package based on GNU hello");
        assert_eq!(
            package.description,
            "The GNU hello program produces a familiar, friendly greeting.  It allows non-programmers to use a classic computer science tool.\n\nSeriously, though: this is an example.\nverbatim line"
        );
        assert_eq!(package.installed_size, Some(132));
        assert_eq!(package.size(0), "135 KB");
        assert_eq!(package.homepage, "https://www.gnu.org/software/hello/");
    }

    #[test]
    fn a_control_file_without_name_or_version_is_no_package() {
        assert_eq!(parse_control("Version: 1.0\n"), None);
        assert_eq!(parse_control("garbage"), None);
    }

    #[test]
    fn versions_compare_as_dpkg_compares_them() {
        let cases = [
            ("1.0", "1.0", Ordering::Equal),
            ("1.0", "1.0-0", Ordering::Equal),
            ("1.10", "1.9", Ordering::Greater),
            ("1.0~rc1", "1.0", Ordering::Less),
            ("1.0~rc1", "1.0~rc2", Ordering::Less),
            ("1.0a", "1.0", Ordering::Greater),
            ("1.0+b1", "1.0", Ordering::Greater),
            ("1.0+b1", "1.0a", Ordering::Greater),
            ("1:0.9", "2.0", Ordering::Greater),
            ("1.0-2", "1.0-10", Ordering::Less),
            ("3.03+dfsg2-8build1", "3.03+dfsg2-8", Ordering::Greater),
            ("2.0-1ubuntu1", "2.0-1", Ordering::Greater),
            ("0001.2", "1.2", Ordering::Equal),
            ("1.2.3-4-5", "1.2.3-4", Ordering::Greater),
            ("99999999999999999999.1", "99999999999999999998.9", Ordering::Greater),
        ];
        for (a, b, expected) in cases {
            assert_eq!(compare_versions(a, b), expected, "{a} against {b}");
            assert_eq!(compare_versions(b, a), expected.reverse(), "{b} against {a}");
        }
    }

    #[test]
    fn the_installed_version_sets_the_standing() {
        assert_eq!(standing(None, "1.0"), Standing::New);
        assert_eq!(standing(Some("0.9"), "1.0"), Standing::Upgrade("0.9".into()));
        assert_eq!(standing(Some("1.0"), "1.0"), Standing::Same);
        assert_eq!(standing(Some("1:0.1"), "1.0"), Standing::Downgrade("1:0.1".into()));
    }

    #[test]
    fn a_simulation_names_what_else_comes_and_goes() {
        let text = "Reading state information...\nThe following NEW packages will be installed:\n  cowsay neko-pulls sl\nInst cowsay (3.03+dfsg2-8build1 Ubuntu:26.10/stonking [all])\nInst libc6 [2.39-0ubuntu1] (2.40-1ubuntu1 Ubuntu:26.10/stonking [amd64])\nInst neko-pulls (1.0 local-deb [all])\nInst libfoo:i386 (1.2 Ubuntu:26.10/stonking [i386])\nRemv hello-traditional [2.10-5]\nConf cowsay (3.03+dfsg2-8build1 Ubuntu:26.10/stonking [all])\n";
        let changes = parse_simulation(text, "neko-pulls");
        assert_eq!(
            changes.installs,
            [
                Change {
                    name: "cowsay".into(),
                    version: "3.03+dfsg2-8build1".into(),
                    from: None
                },
                Change {
                    name: "libc6".into(),
                    version: "2.40-1ubuntu1".into(),
                    from: Some("2.39-0ubuntu1".into())
                },
                Change {
                    name: "libfoo".into(),
                    version: "1.2".into(),
                    from: None
                },
            ]
        );
        assert_eq!(changes.removes, ["hello-traditional"]);
    }

    #[test]
    fn packages_for_any_processor_or_this_one_fit() {
        let machine = ["amd64".to_string(), "i386".to_string()];
        assert!(architecture_fits("all", &machine));
        assert!(architecture_fits("amd64", &machine));
        assert!(architecture_fits("i386", &machine));
        assert!(!architecture_fits("arm64", &machine));
    }

    #[test]
    fn the_install_asks_apt_for_the_file_and_what_the_standing_needs() {
        let path = Path::new("/home/neko/下載/my app_1.0_amd64.deb");
        let plan = install_plan(path, &Standing::New);
        assert!(plan.user.is_empty());
        let command = &plan.admin[0];
        assert_eq!(&command[..2], ["env", PLAIN_LOCALE_ASSIGNMENT]);
        assert!(command.contains(&"apt-get".to_string()));
        assert_eq!(command.last().unwrap(), "/home/neko/下載/my app_1.0_amd64.deb");
        assert!(!command.contains(&"--reinstall".to_string()));
        let same = install_plan(path, &Standing::Same);
        assert!(same.admin[0].contains(&"--reinstall".to_string()));
        let older = install_plan(path, &Standing::Downgrade("2.0".into()));
        assert!(older.admin[0].contains(&"--allow-downgrades".to_string()));
    }
}
