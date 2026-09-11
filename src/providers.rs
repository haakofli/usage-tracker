//! Which AI CLIs are on this machine, and which of them the dock shows.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderId {
    Claude,
    Codex,
    Copilot,
    Gemini,
    Cursor,
}

impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Copilot,
        Self::Gemini,
        Self::Cursor,
    ];

    /// Stable key for the settings file; renaming one would silently reset a
    /// user's choice, so these never change.
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Copilot => "Copilot",
            Self::Gemini => "Gemini",
            Self::Cursor => "Cursor",
        }
    }

    /// Whether the dock can actually read a quota for this provider.
    ///
    /// Detection and readability are separate questions. Gemini publishes no
    /// remaining-quota figure anywhere — its own CLI can only report the
    /// current session — and Cursor's is behind an undocumented endpoint, so
    /// both are listed but cannot be ticked. Inventing a ring for them would
    /// read as real.
    pub fn has_quota_source(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Copilot)
    }

    /// Directory under the user profile that marks the CLI as installed.
    fn home_marker(self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::Copilot => ".copilot",
            Self::Gemini => ".gemini",
            Self::Cursor => ".cursor",
        }
    }

    fn binaries(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Copilot => &["copilot", "gh"],
            Self::Gemini => &["gemini"],
            Self::Cursor => &["cursor"],
        }
    }

    pub fn is_installed(self) -> bool {
        if home_dir_exists(self.home_marker()) || self.binaries().iter().any(|b| on_path(b)) {
            return true;
        }
        // Copilot is most often used through an editor extension, which leaves
        // neither a CLI on `PATH` nor a `~/.copilot` directory — only a signed-in
        // token. That token is the thing the dock actually needs, so finding one
        // is better evidence than either of the above.
        matches!(self, Self::Copilot) && crate::copilot::token().is_some()
    }
}

fn home_dir_exists(name: &str) -> bool {
    crate::platform::home_dir()
        .map(|home| home.join(name).is_dir())
        .unwrap_or(false)
}

/// Walks `PATH` directly rather than spawning a shell: launching several
/// processes at startup just to answer "is this installed" is not worth it.
fn on_path(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    // Windows resolves a bare name through these; elsewhere an executable
    // carries no extension, so the bare path is the only candidate.
    let extensions: &[&str] = if cfg!(windows) {
        &["exe", "cmd", "bat", "ps1"]
    } else {
        &[]
    };
    // `split_paths` rather than splitting on a literal separator, which is `;`
    // on Windows and `:` everywhere else.
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| {
            let base = dir.join(binary);
            base.is_file()
                || extensions
                    .iter()
                    .any(|ext| base.with_extension(ext).is_file())
        })
}

pub fn installed() -> Vec<ProviderId> {
    ProviderId::ALL
        .into_iter()
        .filter(|p| p.is_installed())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for p in ProviderId::ALL {
            assert_eq!(ProviderId::from_key(p.key()), Some(p));
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = ProviderId::ALL.iter().map(|p| p.key()).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "provider keys must be unique");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert_eq!(ProviderId::from_key("nope"), None);
    }

    /// Only providers with a reader may claim a quota source; the rest would
    /// render as empty rings that look like "nothing used".
    #[test]
    fn only_readable_providers_claim_a_quota_source() {
        for p in [ProviderId::Claude, ProviderId::Codex, ProviderId::Copilot] {
            assert!(p.has_quota_source(), "{} has a reader", p.label());
        }
        for p in [ProviderId::Gemini, ProviderId::Cursor] {
            assert!(!p.has_quota_source(), "{} has no reader", p.label());
        }
    }

    #[test]
    #[ignore = "depends on what is installed on this machine"]
    fn finds_something_installed() {
        assert!(!installed().is_empty());
    }
}
