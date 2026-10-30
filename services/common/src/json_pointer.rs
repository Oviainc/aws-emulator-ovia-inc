// SPDX-License-Identifier: CC0-1.0

//! The ONE spelling of a request member as a JSON pointer, for every service whose
//! refusal addresses a failing input that way.
//!
//! A service that reports which member of a request it refused has two spellings to
//! choose between, and they are not interchangeable: the framework's constraint walk
//! reports a DOTTED path (`vectors.1.member.data`), while several AWS services address
//! the same member as the RFC 6901 JSON pointer into the request body
//! (`/vectors/0/data`). The translation between them is not a matter of taste — a
//! collection step, an element's base and the two characters a pointer segment escapes
//! all have one right answer — so it is stated here once rather than per service.
//!
//! This module judges only the SPELLING of a member path. Which violations a refusal
//! states, and in what words, is the service's own
//! (see [`crate::validation`] for the collecting half).
//!
//! Services still carrying their own copy: this rule predates this module and is inlined
//! in the service crates that needed it before there was anywhere shared to put it —
//! `grep -rln 'fn json_pointer' services/*/src` lists them. Those copies are to be
//! MIGRATED onto this module; they are left in place only because this change does not
//! own those services' issues, and each is covered by that service's own suite. Nothing
//! new should inline the rule again — call this module.

/// The segment the framework's dotted member path puts between a collection and one of
/// its elements (`vectors.1.member.data`). A JSON pointer names the position itself, so
/// this step has no counterpart in one.
const COLLECTION_STEP: &str = "member";

/// One member of the request as a JSON pointer rooted at the request body, built from
/// the dotted path the framework walks its constraints with
/// (`aws_smithy_schema_server::constraints::path`).
///
/// Each dotted segment becomes one pointer segment. A step from a collection into one of
/// its elements arrives as `<position-or-key>.member`, which a pointer spells as the
/// position alone: an element's position is renumbered from the framework's 1-based count
/// to the 0-based index RFC 6901 gives an array element, and a map entry's key — a value
/// the caller chose rather than a model name — is kept as sent, with the two characters a
/// pointer segment escapes (`~` and `/`) escaped. A numeric map key is therefore
/// indistinguishable from a list position here and is renumbered as one; the dotted form
/// carries nothing that tells the two apart.
#[must_use]
pub fn json_pointer(dotted: &str) -> String {
    let mut pointer = String::with_capacity(dotted.len() + 1);
    let mut segments = dotted.split('.').peekable();
    while let Some(segment) = segments.next() {
        pointer.push('/');
        if segments.peek() == Some(&COLLECTION_STEP) {
            // The step into the element is spelled by the position alone, so the
            // `member` segment that followed it is consumed here.
            segments.next();
            match segment.parse::<usize>() {
                Ok(position) => pointer.push_str(&position.saturating_sub(1).to_string()),
                Err(_) => pointer.push_str(&escaped_segment(segment)),
            }
        } else {
            pointer.push_str(&escaped_segment(segment));
        }
    }
    pointer
}

/// One pointer segment with the two characters a JSON pointer gives a meaning of its own
/// escaped: `~` as `~0` and `/` as `~1` (RFC 6901), so a caller-chosen key holding either
/// still names one member instead of splitting into two.
fn escaped_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dotted walk path becomes the pointer that names the same member, at every depth
    /// and through every kind of step.
    #[test]
    fn a_dotted_path_becomes_the_pointer_naming_the_same_member() {
        // The rows differ along what the rule generalises over: the depth, the kind of
        // step (struct member, list element, map entry), the element's position, and the
        // characters a caller-chosen key can carry.
        let rows: [(&str, &str, &str); 10] = [
            ("a top-level member", "dimension", "/dimension"),
            (
                "a nested member",
                "encryptionConfiguration.sseType",
                "/encryptionConfiguration/sseType",
            ),
            ("the first element of a list", "vectors.1.member", "/vectors/0"),
            (
                "a member of a later list element",
                "vectors.3.member.data",
                "/vectors/2/data",
            ),
            ("a map entry", "tags.team.member", "/tags/team"),
            ("a list of lists", "outer.2.member.1.member", "/outer/1/0"),
            (
                "a map key carrying a slash",
                "tags.team/web.member",
                "/tags/team~1web",
            ),
            (
                "a map key carrying a tilde",
                "tags.a~b.member",
                "/tags/a~0b",
            ),
            (
                "a map key carrying both, in order",
                "tags.~/.member",
                "/tags/~0~1",
            ),
            (
                "a collection step the walk reports with no position before it",
                "member",
                "/member",
            ),
        ];

        for (case, dotted, expected) in rows {
            let actual = json_pointer(dotted);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
