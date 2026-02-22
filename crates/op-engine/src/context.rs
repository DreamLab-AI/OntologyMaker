/// External context accumulator, matching Python's ExternalContext.

#[derive(Debug, Clone, Default)]
pub struct ExternalContext {
    pub observations: Vec<String>,
}

impl ExternalContext {
    pub fn new() -> Self {
        Self {
            observations: Vec::new(),
        }
    }

    pub fn add(&mut self, text: impl Into<String>) {
        self.observations.push(text.into());
    }

    /// Generate a summary of the most recent observations.
    pub fn summary(&self, max_items: usize, max_chars: usize) -> String {
        if self.observations.is_empty() || max_items == 0 {
            return "(empty)".to_string();
        }
        let start = if self.observations.len() > max_items {
            self.observations.len() - max_items
        } else {
            0
        };
        let recent = &self.observations[start..];
        let joined = recent.join("\n\n");
        if joined.len() <= max_chars {
            joined
        } else {
            let mut truncated = joined[..max_chars].to_string();
            truncated.push_str("\n...[truncated external context]...");
            truncated
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_context() {
        let ctx = ExternalContext::new();
        assert_eq!(ctx.summary(12, 8000), "(empty)");
    }

    #[test]
    fn test_zero_max_items() {
        let mut ctx = ExternalContext::new();
        ctx.add("something");
        assert_eq!(ctx.summary(0, 8000), "(empty)");
    }

    #[test]
    fn test_add_and_summary() {
        let mut ctx = ExternalContext::new();
        ctx.add("first");
        ctx.add("second");
        ctx.add("third");
        let s = ctx.summary(12, 8000);
        assert!(s.contains("first"));
        assert!(s.contains("second"));
        assert!(s.contains("third"));
    }

    #[test]
    fn test_max_items_limits() {
        let mut ctx = ExternalContext::new();
        for i in 0..20 {
            ctx.add(format!("obs-{}", i));
        }
        let s = ctx.summary(3, 8000);
        assert!(!s.contains("obs-0"));
        assert!(s.contains("obs-17"));
        assert!(s.contains("obs-18"));
        assert!(s.contains("obs-19"));
    }

    #[test]
    fn test_max_chars_truncation() {
        let mut ctx = ExternalContext::new();
        ctx.add("a".repeat(500));
        let s = ctx.summary(12, 100);
        assert!(s.len() < 200);
        assert!(s.contains("truncated external context"));
    }
}
