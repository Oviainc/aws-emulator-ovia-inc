// SPDX-License-Identifier: CC0-1.0

//! Which log groups a `StartQuery` request reads, and whether the request may be ADMITTED at
//! all — the rules that relate the query LANGUAGE to where the log groups are named and to
//! what the named groups are.
//!
//! `StartQuery` documents two of them itself. The log groups are named in exactly ONE place:
//! "a StartQuery operation must include one of the following: Either exactly one of the
//! following parameters: logGroupName, logGroupNames, or logGroupIdentifiers; Or the
//! queryString must include a SOURCE command to select log groups for the query" — and the
//! SQL query language is the exception, where "you specify the log group names inside the
//! querystring instead of here" (`StartQuery$logGroupIdentifiers`), so for that language the
//! statement's own `FROM` clause is that one place and the two legacy members are not.
//!
//! The third rule is about the groups rather than the request: a query in one of the
//! OpenSearch languages (SQL, PPL) reads log groups of ONE log class, so a set mixing the
//! standard and infrequent-access classes is refused. The log class is a property of the log
//! group (`CreateLogGroup$logGroupClass`) and invisible in `StartQuery`'s input, so the rule
//! can only be applied where the named groups are resolved against state — here.
//!
//! Each refusal states the sentence CloudWatch Logs states for its class, inside the SDK
//! envelope every refusal of this service carries
//! ([`crate::state::with_sdk_error_envelope`]), so the status and code stated in the text are
//! the ones the response answers with.

use std::collections::BTreeSet;

use super::ComAmazonawsCloudwatchlogsLogs20140328State as State;

/// The sentence a request whose SQL statement names no log groups at all is refused with.
///
/// For the SQL language the statement IS where the log groups are named, so a statement with
/// no source clause the service can resolve — one that is not a `SELECT` at all, or one whose
/// `FROM` clause names nothing — leaves the request naming no log groups to query. Fixed text
/// for the class: it carries nothing of the request.
pub const SQL_SOURCES_UNRESOLVED_MESSAGE: &str = "Log group tag queries are not supported";

/// That sentence, which is the whole message: a request refused for naming no log groups to
/// query carries the sentence alone.
pub fn sql_sources_unresolved_refusal() -> String {
    SQL_SOURCES_UNRESOLVED_MESSAGE.to_string()
}

/// The sentence a query reading log groups of two different log classes is refused with.
/// Fixed text for the class, naming neither the groups nor their classes.
pub const MIXED_LOG_CLASS_MESSAGE: &str = "Querying is supported only in the same log class.";

/// That sentence inside the envelope of the `InvalidParameterException` it is refused under.
pub fn mixed_log_class_refusal() -> String {
    super::with_sdk_error_envelope(MIXED_LOG_CLASS_MESSAGE, super::ENVELOPE_INVALID_PARAMETER)
}

/// The sentence a request that names the log groups to query in TWO places is refused with:
/// a member naming them beside a query string that names them itself.
///
/// `StartQuery` admits exactly one place ("a StartQuery operation must include one of the
/// following: Either exactly one of the following parameters … Or the queryString must
/// include a SOURCE command"), and for the SQL language the statement's own source clause is
/// that place — so a request carrying one of the members that name log groups BY NAME beside
/// such a statement names them twice. Fixed text for the class, naming the places a request
/// may name them in and nothing of the request itself.
pub const LOG_GROUP_SOURCE_CONFLICT_MESSAGE: &str =
    "Only one of logGroupName, logGroupNames, logGroupIdentifiers, or a source named in the \
     queryString can be specified";

/// That sentence inside the envelope of the `InvalidParameterException` it is refused under.
pub fn log_group_source_conflict_refusal() -> String {
    super::with_sdk_error_envelope(
        LOG_GROUP_SOURCE_CONFLICT_MESSAGE,
        super::ENVELOPE_INVALID_PARAMETER,
    )
}

/// The sentence a query over a log group this account does not hold is refused with: the
/// group the request named, and the account it was looked for in — the two variable parts of
/// the fixed text, so the caller can tell WHICH of the groups it named is missing.
pub fn log_group_not_found_refusal(log_group_name: &str) -> String {
    super::with_sdk_error_envelope(
        &format!(
            "Log group '{log_group_name}' does not exist for account ID '{}'",
            ::engine::emulator_account_id()
        ),
        super::ENVELOPE_RESOURCE_NOT_FOUND,
    )
}

/// Whether `names` names log groups of more than one log class, which is what one query may
/// not read. Groups this account does not hold have no class and count for nothing here; a
/// query over fewer than two groups can span nothing.
pub fn spans_two_log_classes(store: &State, names: &[String]) -> bool {
    let classes: BTreeSet<&str> = names
        .iter()
        .filter_map(|name| store.log_groups.get(name))
        .map(|group| group.log_group_class.as_str())
        .collect();
    classes.len() > 1
}

/// Whether a pipelined query string SELECTS ITS OWN log groups: its first stage is the
/// command that names them, which Logs Insights QL writes `SOURCE` and PPL writes `source`
/// or `search`.
///
/// A query that does is a query whose log groups are named in the query string, so the
/// request need name none of the three members — the second of the two forms `StartQuery`
/// documents for naming them.
pub fn pipeline_selects_its_own_sources(query_string: &str) -> bool {
    query_string.split('|').any(|stage| {
        stage
            .split_whitespace()
            .next()
            .is_some_and(|command| {
                let command = command.to_lowercase();
                command == SOURCE_COMMAND || command == SEARCH_COMMAND
            })
    })
}

/// The command a pipelined query names its log groups with, as `StartQuery` spells it.
const SOURCE_COMMAND: &str = "source";

/// The command PPL opens a pipeline with, which names them the same way.
const SEARCH_COMMAND: &str = "search";

#[cfg(test)]
mod tests {
    use super::{pipeline_selects_its_own_sources, LOG_GROUP_SOURCE_CONFLICT_MESSAGE};

    /// A pipelined query's own source command is read wherever the pipeline writes it, and a
    /// pipeline that writes none selects no log groups of its own.
    #[test]
    fn a_pipelines_own_source_command_is_read_from_the_query_string() {
        // (case, the query string, whether the pipeline names its own log groups)
        let cases: [(&str, &str, bool); 6] = [
            ("no source command", "fields @timestamp, @message", false),
            (
                "a SOURCE command opening the pipeline",
                "SOURCE logGroups(namePrefix: ['/aws/']) | fields @message",
                true,
            ),
            (
                "a source command further along",
                "fields @message | source 'a-group'",
                true,
            ),
            ("a PPL search command", "search source = my-group | head 5", true),
            (
                "a field whose name opens with the command's word",
                "fields sourceAddress | stats count(*) as n by sourceAddress",
                false,
            ),
            ("an empty query string", "", false),
        ];
        for (case, query_string, expected) in cases {
            let actual = pipeline_selects_its_own_sources(query_string);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    /// The conflict sentence names the places a request may name its log groups in, and
    /// nothing of the request.
    #[test]
    fn the_conflict_sentence_names_the_places_the_log_groups_may_be_named_in() {
        for place in [
            "logGroupName",
            "logGroupNames",
            "logGroupIdentifiers",
            "queryString",
        ] {
            assert!(
                LOG_GROUP_SOURCE_CONFLICT_MESSAGE.contains(place),
                "case {place}: the sentence names every place the log groups may be named in, \
                 got {LOG_GROUP_SOURCE_CONFLICT_MESSAGE:?}"
            );
        }
    }
}
