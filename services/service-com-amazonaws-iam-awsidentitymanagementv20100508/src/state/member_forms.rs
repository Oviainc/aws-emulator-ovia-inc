// SPDX-License-Identifier: CC0-1.0

//! The published FORMS an input member's value is taken against, and the sentences IAM
//! refuses a value outside one with.
//!
//! IAM answers one fixed sentence for a form, where the model-level constraint walk states
//! one clause per broken constraint and echoes the value. Two readings of that:
//! [`super::MEMBER_FORMAT_REFUSALS`] names the members whose WHOLE form (length and alphabet
//! together) is one sentence, and [`pattern_refusal`] here answers for a value outside the
//! published PATTERN of the shape a member targets, naming that member, while the member's
//! LENGTH stays the walk's own clause — each of these sentences states a form and no length.
//! Both are read by [`crate::service`], where a refusal raised before any handler ran is
//! worded.

#[allow(unused_imports)]
use super::*;

/// The character class IAM publishes for an entity NAME: "upper and lowercase alphanumeric
/// characters with no spaces. You can also include any of the following characters: _+=,.@-"
/// (`DetachUserPolicy$UserName`, the same prose on every name member). It is the `@pattern`
/// of every name shape in the model (`smithy_query(query="shape",
/// shape="userNameType,roleNameType,groupNameType,instanceProfileNameType,policyNameType")`).
const ENTITY_NAME_CHARACTER_CLASS: &str = r"^[\w+=,.@-]+$";

/// AWS's sentence for a value outside [`ENTITY_NAME_CHARACTER_CLASS`], with the offending
/// input member in place of `{member}` and nothing of the value.
const ENTITY_NAME_REFUSAL: &str = "The specified value for {member} is invalid. It must \
                                   contain only alphanumeric characters and/or the \
                                   following: +=,.@_-";

/// The pattern IAM publishes for an entity PATH: "either a forward slash (/) by itself or a
/// string that must begin and end with forward slashes" (`CreateUser$Path`), which is
/// `pathType`'s `@pattern` (`smithy_query(query="shape", shape="pathType")`), spelled as the
/// model spells it.
const ENTITY_PATH_PATTERN: &str = r"^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$";

/// AWS's sentence for a value outside [`ENTITY_PATH_PATTERN`]. A path is delimited at BOTH
/// ends, where a path PREFIX only has to open with the delimiter
/// (`super::INVALID_PATH_PREFIX`) — two published forms, so two sentences. Shared with
/// [`super::entity_path`], which takes the same form where the walk cannot.
pub(super) const ENTITY_PATH_REFUSAL: &str = "The specified value for {member} is invalid. \
                                              It must begin and end with / and contain only \
                                              alphanumeric characters and/or / characters.";

/// The published patterns IAM states a sentence of its OWN for: `(the pattern as the model
/// declares it, AWS's sentence with `{member}` where the member goes)`.
///
/// Keyed by the PATTERN rather than by the member, because the sentence states the form that
/// pattern describes: one entry answers for every member of every operation whose shape
/// publishes that form, and names whichever member the request broke it on.
///
/// SOURCE, for adding a pattern: AWS's own sentence for a value outside it — the recorded
/// refusal, or the format statement in the IAM API Reference — which reads `The specified
/// value for <member> is invalid. It must <the published form>`. A pattern nobody has such a
/// statement for is absent and keeps the model-level clause
/// (`tests/all/range_refusal_wording.rs` reads those back).
const PATTERN_REFUSALS: &[(&str, &str)] = &[
    (ENTITY_NAME_CHARACTER_CLASS, ENTITY_NAME_REFUSAL),
    (ENTITY_PATH_PATTERN, ENTITY_PATH_REFUSAL),
];

/// AWS's sentence for a value that broke the `@pattern` `pattern` on the member `path`
/// names, or `None` when IAM states no sentence of its own for that pattern.
///
/// The LAST path segment names the member, already spelled the way a refusal names it
/// (lowerCamel, as `aws_smithy_schema_server::constraints::path::member` builds it).
pub fn pattern_refusal(path: &str, pattern: &str) -> Option<String> {
    let sentence = PATTERN_REFUSALS
        .iter()
        .find_map(|(published, sentence)| (*published == pattern).then_some(*sentence))?;
    let member = path.split('.').next_back()?;
    Some(sentence.replace("{member}", member))
}
