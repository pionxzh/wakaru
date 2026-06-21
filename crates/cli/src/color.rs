use std::io::IsTerminal;

pub struct Styled {
    enabled: bool,
}

impl Styled {
    pub fn for_stderr() -> Self {
        let enabled = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self { enabled }
    }

    pub fn off() -> Self {
        Self { enabled: false }
    }

    pub fn error(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[31m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn warning(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[33m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}
