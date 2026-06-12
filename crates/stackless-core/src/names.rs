//! Default instance name composition (`{stack.name}-{uuid}`).

use uuid::Uuid;

use crate::types::{DnsName, TypeError};

/// A DNS-safe instance name: `{stack}-{uuid}`.
pub fn compose_instance_name(stack: &str) -> Result<String, TypeError> {
    let id = Uuid::new_v4().hyphenated().to_string();
    let name = format!("{stack}-{id}");
    DnsName::try_new(name).map(DnsName::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_stack_name_is_valid() {
        let name = compose_instance_name("hello").expect("hello + uuid");
        assert!(name.starts_with("hello-"));
        assert!(DnsName::try_new(&name).is_ok());
    }

    #[test]
    fn uuid_segment_matches_hyphenated_form() {
        let name = compose_instance_name("atto").expect("atto + uuid");
        let Some((stack, uuid)) = name.split_once('-') else {
            panic!("expected stack-uuid form, got {name:?}");
        };
        assert_eq!(stack, "atto");
        let parts: Vec<_> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }

    #[test]
    fn overlong_stack_name_rejected() {
        let stack = "a".repeat(27);
        let err = compose_instance_name(&stack).expect_err("stack name too long for uuid suffix");
        assert!(matches!(err, TypeError::InvalidDnsName { .. }));
    }
}