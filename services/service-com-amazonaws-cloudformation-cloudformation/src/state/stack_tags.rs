// SPDX-License-Identifier: CC0-1.0

//! The tag keys a CALLER may put on a stack, a change set or a stack set, and the reserved
//! namespace every part of this crate reads tag keys against.
//!
//! `Tag$Key` is "a string used to identify this tag ... Tags owned by Amazon Web Services
//! have the reserved prefix: `aws:`", so a key under that prefix names a tag the SERVICE
//! owns: CloudFormation writes its own `aws:cloudformation:*` keys onto every resource a
//! stack provisions ([`super::provisioning`]), and a request supplying one would be claiming
//! a key the service decides. Every operation that takes a caller's `Tags` is held to the
//! same rule, so a tag one request cannot put on a stack no other request can put on it
//! either.
//!
//! The namespace itself is defined here ([`RESERVED_TAG_KEY_PREFIX`],
//! [`is_reserved_tag_key`]) for the parts of this crate that read tag keys against it: this
//! refusal, and the provisioning arms that keep service-owned keys out of the tag lists
//! another service's API is called with. `super::provisioner` carries a private copy of the
//! same two items that this module cannot take over, because that file is far longer than a
//! file a change may touch; a change that splits it should delegate it to here.

/// The tag-key prefix `Tag$Key` documents as the one the tags Amazon Web Services owns
/// carry, and therefore the one no caller's key may open with.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// Matched case-insensitively, as the services that refuse such a key match it: the
/// namespace is AWS's whichever case a request spells it in.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Err(message)` for a `Tags` list carrying a key in the reserved namespace.
pub fn validate_tags(tags: &[crate::ops::types::Tag]) -> Result<(), String> {
    if tags.iter().any(|tag| is_reserved_tag_key(&tag.key)) {
        return Err(RESERVED_TAG_KEY.to_string());
    }
    Ok(())
}

/// [`validate_tags`] for a request whose `Tags` member may be absent, which is every
/// operation that takes one: a request naming no tags states no key to hold to the rule.
pub fn validate_optional_tags(tags: Option<&[crate::ops::types::Tag]>) -> Result<(), String> {
    validate_tags(tags.unwrap_or_default())
}

/// The `ValidationError` message CloudFormation answers a caller-supplied tag key in the
/// reserved namespace.
///
/// ENTIRELY fixed text: the sentence states which keys a caller may not set and names
/// neither the key the request sent nor the operation that refused it — so the one sentence
/// is the answer wherever a `Tags` list reaches this service.
pub const RESERVED_TAG_KEY: &str = "aws: prefixed tag key names are not allowed for external use.";

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(key: &str) -> crate::ops::types::Tag {
        crate::ops::types::Tag {
            key: key.to_string(),
            value: "v".to_string(),
        }
    }

    #[test]
    fn a_caller_owned_key_is_admitted() {
        for key in ["Name", "aw:s", "team", "a", "AWSTeam", "not-aws:prefixed"] {
            let judged = validate_tags(&[tag(key)]);
            assert!(
                judged.is_ok(),
                "case {key:?}: expected the key to be admitted, got {judged:?}"
            );
        }
    }

    #[test]
    fn a_key_in_the_reserved_namespace_is_refused_in_any_case() {
        for key in [
            "aws:mine",
            "AWS:mine",
            "Aws:cloudformation:stack-name",
            "aws:",
        ] {
            let refused = validate_tags(&[tag("ok"), tag(key)]);
            assert_eq!(
                refused,
                Err(RESERVED_TAG_KEY.to_string()),
                "case {key:?}: expected the reserved-namespace refusal, got {refused:?}"
            );
        }
    }
}
