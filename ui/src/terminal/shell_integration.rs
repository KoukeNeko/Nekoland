//! The bash start-up file that marks prompts and commands with OSC 133, so that a pane knows
//! when a command runs and how it ended. It reads the user's own `~/.bashrc` first. While the window
//! names Claude Code settings, `claude` runs with them, unless the user has a `claude` function of
//! their own (see `usage`).

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

const FOLDER: &str = "nekoland-terminal";
const FILE: &str = "bashrc";
const BASHRC: &str = r#"[ -f ~/.bashrc ] && . ~/.bashrc
__nekoland_prompt() {
    local status=$?
    printf '\e]133;D;%s\a' "$status"
    return $status
}
PROMPT_COMMAND="__nekoland_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS1="\[\e]133;A\a\]${PS1}\[\e]133;B\a\]"
PS0="${PS0}\e]133;C\a"
if [ -n "$NEKOLAND_TERMINAL_CLAUDE_SETTINGS" ] && ! declare -F claude >/dev/null; then
    claude() {
        local settings=$NEKOLAND_TERMINAL_CLAUDE_SETTINGS
        # A project that sets a status line of its own keeps it (--settings would win over it): only the hooks go in.
        grep -qs '"statusLine"' .claude/settings*.json </dev/null && settings=${settings%claude-settings.json}claude-hooks.json
        command claude --settings "$settings" "$@"
    }
fi
"#;

static INSTALLED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Writes the file where the user's runtime files are; panes start bash without it when it
/// could not be written.
pub fn install() {
    INSTALLED.get_or_init(|| match write_bashrc() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("nekoland-terminal: could not write the bash start-up file: {error}");
            None
        }
    });
}

pub fn path() -> Option<&'static Path> {
    INSTALLED.get().and_then(|path| path.as_deref())
}

fn write_bashrc() -> std::io::Result<PathBuf> {
    let folder = gtk::glib::user_runtime_dir().join(FOLDER);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(FILE);
    std::fs::write(&path, BASHRC)?;
    Ok(path)
}
