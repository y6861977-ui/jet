//! Parsing a package specifier "name[@range]" — a port of spec.ts.

/// Split "name[@range]" handling scoped names (@scope/name@range).
pub fn split_name_range(spec: &str) -> (String, String) {
    // The first @ of a scoped name is part of the name, so we look for the LAST @.
    if let Some(at) = spec.rfind('@') {
        if at > 0 {
            return (spec[..at].to_string(), spec[at + 1..].to_string());
        }
    }
    (spec.to_string(), "latest".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_scoped() {
        assert_eq!(
            split_name_range("express"),
            ("express".into(), "latest".into())
        );
        assert_eq!(
            split_name_range("express@5.2.1"),
            ("express".into(), "5.2.1".into())
        );
        assert_eq!(
            split_name_range("@scope/pkg"),
            ("@scope/pkg".into(), "latest".into())
        );
        assert_eq!(
            split_name_range("@scope/pkg@^1.0.0"),
            ("@scope/pkg".into(), "^1.0.0".into())
        );
    }
}
