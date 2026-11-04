// SPDX-License-Identifier: CC0-1.0

//! WHICH of its two refusals this service answers an `AccessString` with.
//!
//! `AccessString` is constrained by the model with `@pattern "\S"` alone
//! (`smithy_query(query="operation",operation="ModifyUser")`), so the whole RBAC
//! rule grammar is enforced server-side and the refusal MESSAGE is the only
//! diagnosis a client gets. ElastiCache gives two, and they say different things:
//!
//! * a rule list whose syntax reads as a rule list but which names a command
//!   CATEGORY the engine does not publish (`+@<name>` / `-@<name>`) is refused
//!   with [`UNKNOWN_CATEGORY`], which states that the access string carries an
//!   invalid category and points the caller at the published list;
//! * an input that is not rule-list syntax at all keeps the one sentence
//!   [`crate::state::ACCESS_STRING_INVALID`] holds.
//!
//! A rule is therefore read as a CATEGORY rule by its form — `+@` or `-@`
//! followed by a name — and the published catalog below decides whether the name
//! is one; whether the rest of the list is a rule list stays
//! [`crate::state::access_string_rejection`]'s decision, which [`refusal`] defers
//! to rather than restating.
//!
//! One place, so `CreateUser` and both of `ModifyUser`'s members take the same
//! two classes and word each the same way.

/// The sentence ElastiCache refuses a rule list naming a command category it
/// does not publish with.
///
/// Fixed text, with no variable part: it names neither the category that was
/// asked for nor the member that carried the list, and sends the caller to the
/// published catalog instead.
pub const UNKNOWN_CATEGORY: &str = "Access string contains invalid category(s). Please verify the \
                                    list of available category in user-api.";

/// The command categories the engine publishes, which are the names a category
/// rule may carry.
///
/// Source: the engine's own ACL category catalog (`ACL CAT` on Valkey / Redis
/// OSS), which is the list ElastiCache's RBAC documentation sends a caller to —
/// the model publishes no vocabulary for it, so the engine's catalog IS the rule.
/// `all` is in it because `+@all` / `-@all` is the baseline form of a category
/// rule (see `crate::state::ACCESS_STRING_DENY_ALL`) even though `ACL CAT` does
/// not list it as a category of its own.
const COMMAND_CATEGORIES: [&str; 22] = [
    "admin",
    "all",
    "bitmap",
    "blocking",
    "connection",
    "dangerous",
    "fast",
    "geo",
    "hash",
    "hyperloglog",
    "keyspace",
    "list",
    "pubsub",
    "read",
    "scripting",
    "set",
    "slow",
    "sortedset",
    "stream",
    "string",
    "transaction",
    "write",
];

/// Whether `category` is one the engine publishes, compared without regard to
/// case, as the engine reads its rule names.
fn is_published_category(category: &str) -> bool {
    COMMAND_CATEGORIES
        .iter()
        .any(|published| published.eq_ignore_ascii_case(category))
}

/// The category name `rule` carries, for a rule of the category FORM, and `None`
/// for every other rule — a keyword, a key or channel pattern, or a command rule
/// naming a command rather than a category.
fn category_named(rule: &str) -> Option<&str> {
    rule.strip_prefix('+')
        .or_else(|| rule.strip_prefix('-'))?
        .strip_prefix('@')
}

/// Why `access_string` is not a rule list this service can apply, or `None` when
/// it is one — the two refusals this module's docs name, in the order the service
/// diagnoses them.
pub fn refusal(access_string: &str) -> Option<&'static str> {
    // A selector carries its rules inside parentheses (`(+@read ~key1)`), so the
    // parentheses around a rule are not part of the rule.
    let names_unpublished_category = access_string
        .split_whitespace()
        .map(|token| token.trim_matches(|character| character == '(' || character == ')'))
        .filter_map(category_named)
        .any(|category| !is_published_category(category));
    if names_unpublished_category {
        return Some(UNKNOWN_CATEGORY);
    }
    crate::state::access_string_rejection(access_string)
}
