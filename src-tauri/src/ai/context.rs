pub struct ContextManager;

impl ContextManager {
    /// Bound untrusted student text before building a provider request.
    pub fn limit_chars(value: &str, maximum: usize) -> String {
        value.chars().take(maximum).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ContextManager;

    #[test]
    fn limits_unicode_by_characters_not_utf8_bytes() {
        assert_eq!(ContextManager::limit_chars("مرحبا بالعالم", 5), "مرحبا");
    }
}