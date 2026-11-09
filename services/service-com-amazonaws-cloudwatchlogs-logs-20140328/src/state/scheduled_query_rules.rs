// SPDX-License-Identifier: CC0-1.0

//! The rules a scheduled query's configuration is held to, shared by the two operations that
//! STATE one — `CreateScheduledQuery` and `UpdateScheduledQuery` — so a configuration a create
//! accepts is one an update accepts and both refuse the same request in the same words.
//!
//! Three rules live here:
//!
//! * `executionRoleArn` ("The ARN of the IAM role that grants permissions to execute the query")
//!   must be an IAM ROLE ARN, which the service's own request validator holds as a regular
//!   expression ([`EXECUTION_ROLE_ARN_PATTERN`]) and states as the member-constraint refusal
//!   every other parameter outside its constraint is stated with;
//! * the log groups a query reads ("An array of log group names or ARNs to query") are resolved
//!   against the log groups the account HOLDS, so a write naming one it does not hold is refused
//!   about that log group rather than about the scheduled query;
//! * `endTimeOffset` has an effective value a reader reports whether or not the write stated
//!   one ([`DEFAULT_END_TIME_OFFSET_SECONDS`]).

use super::ComAmazonawsCloudwatchlogsLogs20140328State as State;

/// The member both writes of a scheduled query name its executing role in.
pub const EXECUTION_ROLE_ARN_MEMBER: &str = "executionRoleArn";

/// The expression a scheduled query's `executionRoleArn` must satisfy, as the SERVICE publishes
/// it for that member: an IAM role ARN of one of the AWS partitions, with the empty Region an
/// IAM ARN carries, a 12-digit account id and a `role/` resource part naming the role.
///
/// Stated UNANCHORED, which is the form this service's pattern-rendering rule states for an
/// expression AWS publishes (see `requirement` in `crate::service`), and the ONE definition the
/// refusal's text and the check itself both read — so the constraint a request is held to cannot
/// drift from the constraint the refusal states.
pub const EXECUTION_ROLE_ARN_PATTERN: &str =
    r"arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\/(.+)";

/// [`EXECUTION_ROLE_ARN_PATTERN`] compiled once. A `@pattern` is satisfied by a value the
/// expression MATCHES, which for an unanchored expression is a value it matches anywhere in,
/// exactly as the service's own validator applies it.
static EXECUTION_ROLE_ARN: std::sync::LazyLock<::regex::Regex> = std::sync::LazyLock::new(|| {
    ::regex::Regex::new(EXECUTION_ROLE_ARN_PATTERN)
        .expect("the published executionRoleArn expression compiles")
});

/// The refusal message for an `executionRoleArn` that is not an IAM role ARN, or `None` for one
/// that is.
///
/// The text is the service's one member-constraint sentence
/// ([`super::validation_message`]) — the member that failed and the expression it failed, and
/// nothing of the value submitted there.
pub fn execution_role_arn_refusal(role_arn: &str) -> Option<String> {
    (!EXECUTION_ROLE_ARN.is_match(role_arn)).then(|| {
        super::validation_message(
            EXECUTION_ROLE_ARN_MEMBER,
            &format!("Member must satisfy regular expression pattern: {EXECUTION_ROLE_ARN_PATTERN}"),
        )
    })
}

/// The refusal message for the FIRST of `identifiers` naming a log group this account does not
/// hold, in the order the request named them, or `None` when every one of them names a group it
/// holds.
///
/// A `ScheduledQueryLogGroupIdentifiers` entry is a log group's name or its ARN, so each is
/// resolved to the name the store is keyed by ([`super::log_group_name_from_identifier`]), the
/// way every other member that names a log group by either form is. The sentence is the one this
/// service states for a log group the account does not hold
/// ([`super::query_admission::log_group_not_found_refusal`]), so a scheduled query's missing log
/// group reads exactly like a started query's.
pub fn unheld_log_group_refusal(store: &State, identifiers: &[String]) -> Option<String> {
    identifiers
        .iter()
        .map(|identifier| super::log_group_name_from_identifier(identifier))
        .find(|name| !store.log_groups.contains_key(*name))
        .map(super::query_admission::log_group_not_found_refusal)
}

/// The end of the lookback window of a scheduled query whose write stated no `endTimeOffset`:
/// the offset "in seconds that defines the end of the lookback period for the query", which for
/// a query that states none ends the window AT the execution instant — no seconds before it.
pub const DEFAULT_END_TIME_OFFSET_SECONDS: i64 = 0;

/// The `endTimeOffset` a write RESOLVES to: the offset it stated, or
/// [`DEFAULT_END_TIME_OFFSET_SECONDS`] when it stated none.
///
/// A scheduled query always runs over a window with an end, so the stored record carries the
/// effective offset rather than the absence of one and every reader of the query reports it.
pub fn effective_end_time_offset(stated: Option<i64>) -> Option<i64> {
    Some(stated.unwrap_or(DEFAULT_END_TIME_OFFSET_SECONDS))
}

#[cfg(test)]
mod tests {
    use super::{
        effective_end_time_offset, execution_role_arn_refusal, DEFAULT_END_TIME_OFFSET_SECONDS,
        EXECUTION_ROLE_ARN_PATTERN,
    };

    /// The published expression admits an IAM role ARN of each partition it names and refuses
    /// every other shape of value, and the refusal states that expression.
    #[test]
    fn the_execution_role_expression_admits_an_iam_role_arn_of_each_partition() {
        // (case, the value, whether it satisfies the expression)
        let cases: [(&str, &str, bool); 12] = [
            ("the standard partition", "arn:aws:iam::000000000000:role/svc", true),
            ("the China partition", "arn:aws-cn:iam::123456789012:role/svc", true),
            (
                "the GovCloud partition",
                "arn:aws-us-gov:iam::123456789012:role/svc",
                true,
            ),
            ("an ISO partition", "arn:aws-iso:iam::123456789012:role/svc", true),
            ("an ISO-B partition", "arn:aws-iso-b:iam::123456789012:role/svc", true),
            (
                "a role under a path",
                "arn:aws:iam::123456789012:role/service-role/svc",
                true,
            ),
            ("not an ARN at all", "scheduled-query-execution", false),
            ("an ARN of another service", "arn:aws:logs:us-east-1:123456789012:log-group:g", false),
            ("an IAM user rather than a role", "arn:aws:iam::123456789012:user/someone", false),
            ("an account id that is too short", "arn:aws:iam::12345:role/svc", false),
            ("a role with no name", "arn:aws:iam::123456789012:role/", false),
            ("an IAM ARN naming a Region", "arn:aws:iam:us-east-1:123456789012:role/svc", false),
        ];
        for (case, value, admitted) in cases {
            let refusal = execution_role_arn_refusal(value);
            assert_eq!(
                refusal.is_none(),
                admitted,
                "case {case:?}: expected admitted={admitted}, got {refusal:?}"
            );
            if let Some(said) = refusal {
                assert_eq!(
                    said,
                    format!(
                        "1 validation error detected: Value at 'executionRoleArn' failed to \
                         satisfy constraint: Member must satisfy regular expression pattern: \
                         {EXECUTION_ROLE_ARN_PATTERN}"
                    ),
                    "case {case:?}: the refusal states the member and the expression"
                );
            }
        }
    }

    /// A write that states no end offset resolves to the documented one; a write that states
    /// one resolves to exactly it, including the boundary values.
    #[test]
    fn the_end_offset_a_write_states_none_for_resolves_to_the_default() {
        // (case, what the write stated, the offset it resolves to)
        let cases: [(&str, Option<i64>, i64); 4] = [
            ("stated none", None, DEFAULT_END_TIME_OFFSET_SECONDS),
            ("stated the default itself", Some(0), 0),
            ("stated a window of one hour", Some(3_600), 3_600),
            ("stated a window of a day", Some(86_400), 86_400),
        ];
        for (case, stated, expected) in cases {
            let actual = effective_end_time_offset(stated);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {expected}, got {actual:?}"
            );
        }
    }
}
