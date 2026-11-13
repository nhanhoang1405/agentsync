use anyhow::{Result, bail};

/// Keep this value alive while the app runs; Linux clipboard ownership can end on drop.
pub struct TextClipboard {
    system: Option<arboard::Clipboard>,
    internal: Option<String>,
    internal_mode_env: &'static str,
}
impl TextClipboard {
    /// Pass an application-specific key, for example `MYAPP_CLIPBOARD`.
    pub fn new(internal_mode_env: &'static str) -> Self {
        Self {
            system: None,
            internal: None,
            internal_mode_env,
        }
    }
    fn connect(&mut self) {
        if self.system.is_none()
            && !cfg!(test)
            && std::env::var(self.internal_mode_env).as_deref() != Ok("internal")
        {
            self.system = arboard::Clipboard::new().ok();
        }
    }
    /// Always retain an internal copy; return whether the desktop clipboard succeeded.
    pub fn copy(&mut self, text: String) -> bool {
        self.connect();
        self.internal = Some(text.clone());
        self.system
            .as_mut()
            .is_some_and(|system| system.set_text(text).is_ok())
    }
    pub fn paste(&mut self) -> Result<String> {
        self.connect();
        if let Some(system) = &mut self.system
            && let Ok(text) = system.get_text()
        {
            return Ok(text);
        }
        if let Some(text) = &self.internal {
            return Ok(text.clone());
        }
        bail!(
            "Clipboard unavailable or empty. Copy inside the app first, or use your terminal's paste shortcut"
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn internal_fallback_keeps_multiline_unicode_and_empty_content() {
        let mut clipboard = TextClipboard::new("MYAPP_CLIPBOARD");
        assert!(clipboard.paste().is_err());
        assert!(!clipboard.copy("hello 世界\nsecond line".into()));
        assert_eq!(clipboard.paste().unwrap(), "hello 世界\nsecond line");
        clipboard.copy(String::new());
        assert_eq!(clipboard.paste().unwrap(), "");
    }
}
