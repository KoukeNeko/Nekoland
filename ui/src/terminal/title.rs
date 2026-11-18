//! How a tab is named when the user has not named it, as Nekominal's title format: slots separated by commas, each
//! the first of its parts (separated by `|`) that has something, the slots that have nothing left out and the others
//! joined by ` · `. The parts: what the program called itself, the folder, the profile, the shell and the agent.

pub const DEFAULT: &str = "title|folder|profile";
const SEPARATOR: &str = " · ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Title,
    Folder,
    Profile,
    Shell,
    Agent,
}

impl Part {
    fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "title" => Some(Self::Title),
            "folder" => Some(Self::Folder),
            "profile" => Some(Self::Profile),
            "shell" => Some(Self::Shell),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// The slots of a format, or None when a part is not one or there is no slot.
pub fn parse(format: &str) -> Option<Vec<Vec<Part>>> {
    let slots = format
        .split(',')
        .map(str::trim)
        .filter(|slot| !slot.is_empty())
        .map(|slot| slot.split('|').map(Part::parse).collect::<Option<Vec<_>>>())
        .collect::<Option<Vec<_>>>()?;
    (!slots.is_empty()).then_some(slots)
}

pub fn is_valid(format: &str) -> bool {
    parse(format).is_some()
}

/// The title `format` makes of what `value` gives for each part.
pub fn compose(format: &str, value: impl Fn(Part) -> Option<String>) -> String {
    let slots = parse(format).or_else(|| parse(DEFAULT)).unwrap_or_default();
    slots
        .iter()
        .filter_map(|parts| parts.iter().find_map(|part| value(*part).filter(|text| !text.is_empty())))
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_slot_takes_its_first_part_that_has_something() {
        let value = |part| match part {
            Part::Folder => Some("src".to_string()),
            Part::Shell => Some("bash".to_string()),
            _ => None,
        };
        assert_eq!(compose(DEFAULT, value), "src");
        assert_eq!(compose("agent|title, shell, folder", value), "bash · src");
    }

    #[test]
    fn a_format_with_an_unknown_part_is_refused() {
        assert!(is_valid("folder, title|profile"));
        assert!(!is_valid("folder, colour"));
        assert!(!is_valid(" , "));
    }
}
