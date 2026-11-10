// SPDX-License-Identifier: CC0-1.0

//! What a caller-supplied CloudWatch Logs LOG GROUP NAME has to be.
//!
//! Several emulated services take a log group name as an input member and then WRITE
//! into that group — an access-log subscription, a delivery destination, a job's log
//! configuration. The value is therefore an instruction to create or write a log group,
//! so it has to be a name CloudWatch Logs can actually hold: the empty string, a name
//! past CloudWatch's ceiling, and a name carrying a character outside the published set
//! each name no group anything could ever be written to.
//!
//! CloudWatch Logs publishes one rule for every such name — 1 to
//! [`MAX_LOG_GROUP_NAME_CHARS`] characters, each of them a letter, a digit or one of the
//! characters [`log_group_name_requirement`] lists — and the services that bind such a
//! member typically declare it as a bare string with no `@length` or `@pattern`, so the
//! framework's constraint walk cannot refuse any of it. The rule lives here, once, so no
//! two services disagree about which names CloudWatch Logs holds; each service keeps only
//! the envelope its own API refuses with (the status, the error shape and which member the
//! refusal is keyed by).
//!
//! `service-com-amazonaws-mediapackage-mediapackage` still carries its own copy of this
//! reading (`state::is_log_group_name`), inlined before this module existed and pinned by
//! that service's own refusal suite; it is to be MIGRATED onto this one rather than copied
//! again, exactly as the claim-rule copies this crate lists are. Nothing new should restate
//! the rule — call this module.

/// The shortest log group name CloudWatch Logs holds — so the empty string names no group.
pub const MIN_LOG_GROUP_NAME_CHARS: usize = 1;

/// The longest log group name CloudWatch Logs holds; see [`MIN_LOG_GROUP_NAME_CHARS`].
///
/// This is the ceiling of the service that WOULD hold the group, not any one emulated
/// service's generic string bound: a longer name is one CloudWatch Logs would never
/// accept, whatever memory it would occupy in a store here.
pub const MAX_LOG_GROUP_NAME_CHARS: usize = 512;

/// The non-alphanumeric characters a log group name may carry, as a refusal names them.
const LOG_GROUP_PUNCTUATION_CHARACTERS: &str = ". - _ / #";

/// Whether `c` is a character a CloudWatch Logs log group name may carry.
///
/// CloudWatch Logs' published set: letters, digits and the characters
/// [`LOG_GROUP_PUNCTUATION_CHARACTERS`] names. The letters-and-digits half is ASCII-ONLY
/// — unlike AWS's tagging restrictions, which say "Unicode letters, digits" — because the
/// log-group rule publishes its set as `a-z`, `A-Z` and `0-9`, so an accented or
/// non-Latin name is not one CloudWatch Logs holds.
///
/// Private: a caller judges a whole NAME ([`is_log_group_name`]), which is the unit the
/// rule is about; nothing outside this module needs the per-character half.
fn is_log_group_name_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '#')
}

/// Whether `value` is a name CloudWatch Logs can hold: between
/// [`MIN_LOG_GROUP_NAME_CHARS`] and [`MAX_LOG_GROUP_NAME_CHARS`] characters, every one of
/// them a letter, a digit or one of the characters
/// [`log_group_name_requirement`] lists.
///
/// The length is counted in CHARACTERS, not bytes, so a name is judged by what the caller
/// wrote rather than by how it happened to encode.
pub fn is_log_group_name(value: &str) -> bool {
    let length = value.chars().count();
    (MIN_LOG_GROUP_NAME_CHARS..=MAX_LOG_GROUP_NAME_CHARS).contains(&length)
        && value.chars().all(is_log_group_name_character)
}

/// The requirement a refused log group name failed: the member's WHOLE accepted domain, in
/// one sentence, so a name refused for its length and one refused for a character it
/// carries are answered with the same rule.
///
/// Built here rather than written at each call site, so the sentence and the bounds it
/// states cannot disagree and a suite asserting it reads the rule's own words.
pub fn log_group_name_requirement() -> String {
    format!(
        "must be {MIN_LOG_GROUP_NAME_CHARS} to {MAX_LOG_GROUP_NAME_CHARS} characters of \
         letters, numbers and `{LOG_GROUP_PUNCTUATION_CHARACTERS}`"
    )
}

/// The log group name prefix CloudWatch Logs keeps for groups of its own: "Log group names
/// can't start with the string aws/" (`CreateLogGroup`'s own documentation), so a name in it
/// is not a caller's to create.
pub const RESERVED_LOG_GROUP_NAME_PREFIX: &str = "aws/";

/// Whether `value` is a name in [`RESERVED_LOG_GROUP_NAME_PREFIX`], which CloudWatch Logs
/// creates no group under however well-formed the rest of it is.
pub fn is_reserved_log_group_name(value: &str) -> bool {
    value.starts_with(RESERVED_LOG_GROUP_NAME_PREFIX)
}

/// What CloudWatch Logs states for a create whose name is in that prefix: the member and the
/// string it may not start with.
///
/// The sentence lives here, once, so the service that owns log groups and any other service
/// that creates one on a caller's behalf cannot word the same refusal differently.
pub fn reserved_log_group_name_message() -> String {
    format!("logGroupName must not start with the string '{RESERVED_LOG_GROUP_NAME_PREFIX}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_domain_is_admitted_and_nothing_else_is() {
        for name in [
            "a",
            "7",
            "Z",
            "_",
            "-",
            ".",
            "/",
            "#",
            "/aws/vendedlogs/states/one",
            "UPPER/lower/MiXeD-0.9_#",
        ] {
            assert!(
                is_log_group_name(name),
                "case {name:?}: CloudWatch Logs holds this name, got rejected"
            );
        }
        for name in [
            "",
            "has a space",
            "group!",
            "group$",
            "group*",
            "group:name",
            "grüße",
            "日本語",
        ] {
            assert!(
                !is_log_group_name(name),
                "case {name:?}: CloudWatch Logs holds no such group, got accepted"
            );
        }
    }

    #[test]
    fn the_length_bound_holds_at_each_published_edge() {
        for (length, acceptable) in [
            (MIN_LOG_GROUP_NAME_CHARS - 1, false),
            (MIN_LOG_GROUP_NAME_CHARS, true),
            (MIN_LOG_GROUP_NAME_CHARS + 1, true),
            (MAX_LOG_GROUP_NAME_CHARS - 1, true),
            (MAX_LOG_GROUP_NAME_CHARS, true),
            (MAX_LOG_GROUP_NAME_CHARS + 1, false),
        ] {
            let name = "g".repeat(length);
            assert_eq!(
                is_log_group_name(&name),
                acceptable,
                "case {length} characters: expected acceptable={acceptable}, got {}",
                is_log_group_name(&name)
            );
        }
    }
}
