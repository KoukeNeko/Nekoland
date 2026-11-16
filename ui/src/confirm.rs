//! `nekoland-confirm restart|shutdown|logout|empty-trash`: asks before the commands that
//! end the session or lose files for good, then carries out the one asked about.

slint::include_modules!();

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
    dialog.on_cancelled(|| {
        let _ = slint::quit_event_loop();
    });
    let command: Vec<String> = command.iter().map(|part| part.to_string()).collect();
    dialog.on_confirmed(move || {
        if let Err(err) = std::process::Command::new(&command[0])
            .args(&command[1..])
            .spawn()
        {
            eprintln!("{}: {err}", command.join(" "));
        }
        let _ = slint::quit_event_loop();
    });
    dialog.run()?;
    Ok(())
}
