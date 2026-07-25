use std::fmt;

use zeroize::Zeroizing;

/// An owned UTF-8 secret that zeroizes its storage and never exposes its value
/// through `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::SecretString;

    #[test]
    fn debug_is_redacted() {
        let secret = SecretString::new("bearer-secret");
        let rendered = format!("{secret:?}");

        assert_eq!(rendered, "SecretString([REDACTED])");
        assert!(!rendered.contains("bearer-secret"));
        assert_eq!(secret.expose(), "bearer-secret");
    }
}
