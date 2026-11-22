//! `nekoland-confirm restart|shutdown|logout|empty-trash`: asks before the commands that
//! end the session or lose files for good, then carries out the one asked about.

slint::include_modules!();

// For its look; the rest is the launcher's.
#[allow(dead_code)]
#[path = "launcher/apps.rs"]
mod apps;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let which = std::env::args().nth(1).unwrap_or_default();
    let (question, consequence, action, command): (&str, &str, &str, &[&str]) = match which.as_str() {
        "restart" => (
            "要立即重新開機嗎？",
            "未儲存的變更可能會遺失。",
            "重新開機",
            &["systemctl", "reboot"],
        ),
        "shutdown" => (
            "要立即關機嗎？",
            "未儲存的變更可能會遺失。",
            "關機",
            &["systemctl", "poweroff"],
        ),
        "logout" => (
            "要登出嗎？",
            "所有程式都會結束，未儲存的變更可能會遺失。",
            "登出",
            &["nekoland", "msg", "quit"],
        ),
        "empty-trash" => (
            "要清空垃圾桶嗎？",
            "垃圾桶裡的項目會永久刪除，無法復原。",
            "清空垃圾桶",
            &["gio", "trash", "--empty"],
        ),
        other => {
            return Err(format!("expected restart, shutdown, logout or empty-trash, not {other:?}").into())
        }
    };

    let dialog = Confirm::new()?;
    slint::set_xdg_app_id("nekoland-confirm")?;
    dialog.set_question(question.into());
    dialog.set_consequence(consequence.into());
    dialog.set_action(action.into());
    dialog.set_glass_opacity(apps::Look::load().glass_opacity);
    dialog.on_cancelled(|| {
        let _ = slint::quit_event_loop();
    });
    let command: Vec<String> = command.iter().map(|part| part.to_string()).collect();
    let refused = format!("無法{action}");
    let weak = dialog.as_weak();
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    dialog.on_confirmed(move || {
        if running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let (command, refused, weak, running) = (command.clone(), refused.clone(), weak.clone(), running.clone());
        // Waited for away from the dialog, so that emptying a large trash leaves it drawn.
        std::thread::spawn(move || {
            let failure = match std::process::Command::new(&command[0]).args(&command[1..]).output() {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(reason(&String::from_utf8_lossy(&output.stderr))),
                Err(err) => Some(err.to_string()),
            };
            let _ = slint::invoke_from_event_loop(move || match failure {
                None => {
                    let _ = slint::quit_event_loop();
                }
                // Said where it was asked, as systemd refuses while updates are installed.
                Some(failure) => {
                    eprintln!("{}: {failure}", command.join(" "));
                    if let Some(dialog) = weak.upgrade() {
                        dialog.set_question(refused.into());
                        dialog.set_consequence(failure.into());
                    }
                    running.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        });
    });
    dialog.run()?;
    Ok(())
}

/// Why a command failed, from what it printed: in words for what is only waited out.
fn reason(stderr: &str) -> String {
    if stderr.contains("inhibitor") {
        return "有程式正在進行不能中斷的工作，例如安裝更新。請稍後再試。".into();
    }
    stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("未知的錯誤").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn a_blocked_restart_says_to_wait() {
        let stderr = "Call to Reboot failed: Operation denied due to active block inhibitor\n";
        assert!(reason(stderr).contains("請稍後再試"));
        assert_eq!(reason("first\nCall failed: Access denied\n\n"), "Call failed: Access denied");
    }
}
