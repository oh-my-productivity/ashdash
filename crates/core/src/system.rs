//! System commands (lock, sleep, empty trash, ...).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCommand {
    Lock,
    Sleep,
    SleepDisplays,
    LogOut,
    Restart,
    ShutDown,
    ScreenSaver,
    EmptyTrash,
}

impl SystemCommand {
    pub const ALL: [SystemCommand; 8] = [
        SystemCommand::Lock,
        SystemCommand::Sleep,
        SystemCommand::SleepDisplays,
        SystemCommand::LogOut,
        SystemCommand::Restart,
        SystemCommand::ShutDown,
        SystemCommand::ScreenSaver,
        SystemCommand::EmptyTrash,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Sleep => "Sleep",
            Self::SleepDisplays => "Sleep Displays",
            Self::LogOut => "Log Out",
            Self::Restart => "Restart",
            Self::ShutDown => "Shut Down",
            Self::ScreenSaver => "Start Screen Saver",
            Self::EmptyTrash => "Empty Trash",
        }
    }

    pub fn subtitle(&self) -> &'static str {
        match self {
            Self::Lock => "Turn the display off (locks if a password is required on wake)",
            Self::Sleep => "Put the Mac to sleep",
            Self::SleepDisplays => "Turn the displays off",
            Self::LogOut => "Log out the current user",
            Self::Restart => "Restart the Mac",
            Self::ShutDown => "Shut down the Mac",
            Self::ScreenSaver => "Start the screen saver",
            Self::EmptyTrash => "Permanently delete everything in the Trash",
        }
    }

    /// Destructive commands ask for a second confirmation.
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Self::LogOut | Self::Restart | Self::ShutDown | Self::EmptyTrash)
    }

    pub fn id(&self) -> String {
        format!("system:{}", self.title())
    }

    pub fn run(&self) -> std::io::Result<()> {
        let osa = |script: &str| {
            crate::actions::system_command("osascript")
                .arg("-e")
                .arg(script)
                .status()
        };
        let status = match self {
            Self::Lock => crate::actions::system_command("pmset").arg("displaysleepnow").status(),
            Self::Sleep => crate::actions::system_command("pmset").arg("sleepnow").status(),
            Self::SleepDisplays => crate::actions::system_command("pmset").arg("displaysleepnow").status(),
            Self::LogOut => osa(r#"tell application "System Events" to log out"#),
            Self::Restart => osa(r#"tell application "System Events" to restart"#),
            Self::ShutDown => osa(r#"tell application "System Events" to shut down"#),
            Self::ScreenSaver => crate::actions::system_command("open")
                .args(["-a", "ScreenSaverEngine"])
                .status(),
            Self::EmptyTrash => osa(r#"tell application "Finder" to empty trash"#),
        }?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{} failed ({status})", self.title())))
        }
    }
}
