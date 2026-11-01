// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two name-pattern list operations — `ListLogGroups` and
//! `ListAggregateLogGroupSummaries` — and for the `logGroupNamePattern` member they
//! share.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a `logGroupNamePattern` is the match expression its member
//! documents — up to five `|`-separated alternatives, each a case-sensitive substring
//! match, or a prefix match when it carries a leading `^` — and a request carrying one
//! is answered with the log groups (or the aggregate counts) it selects, which is an
//! EMPTY list when it selects none. The only values refused are the ones the member's
//! own bounds exclude: the `@length(min: 3, max: 129)` on the whole value, more than
//! five alternatives, an alternative shorter than the documented three characters, and a
//! character outside the documented class. An alternative LONGER than the `@pattern`'s
//! 24-character segment is NOT refused — the service answers 200 for one, and a pattern
//! naming a real log group is longer than that.
//!
//! The tables below span that class: the forms a pattern takes (anchored, unanchored,
//! one alternative up to five), the lengths at each published bound and one past it, the
//! name forms a log group takes, and the other filters the operations layer on top
//! (class, tags, data sources, field indexes, linked accounts). Any single pattern an
//! issue happens to mention is one row.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS, LOG_GROUP_NAME_REGEX_PATTERN,
};

use crate::harness::{Logs, REGION};

/// How long a case waits before treating a freshly created log group as part of the
/// aggregate view: past the span that view lags its log groups by, plus room for a loaded
/// host's own scheduling.
fn past_aggregation_lag() -> std::time::Duration {
    std::time::Duration::from_millis(
        u64::try_from(AGGREGATE_SUMMARY_VISIBILITY_DELAY_MS).unwrap_or(0) + 600,
    )
}

/// The log groups every matching table below is run against. They span the forms
/// `LogGroupName`'s pattern (`^[\.\-_/#A-Za-z0-9]+$`) admits — a path, a dotted and
/// hashed name, a bare word — and, deliberately, names whose single `/`-free run is far
/// longer than the 24 characters `LogGroupNameRegexPattern`'s `@pattern` allows one
/// alternative, plus two that differ only in case.
///
/// Listed here in ASCII order, which is the order a log group list is enumerated in, so
/// a table's expected rows can be read straight off it.
const GROUPS: [&str; 6] = [
    "/aws/apigateway/welcome",
    "/aws/lambda/checkout-service-production-handler",
    "/aws/lambda/payments",
    "Discovery",
    "app.metrics/2024#v1",
    "discovery",
];

/// Every log group class, from the model's `LogGroupClass` enum (`DELIVERY` |
/// `INFREQUENT_ACCESS` | `STANDARD`).
const LOG_GROUP_CLASSES: [&str; 3] = ["DELIVERY", "INFREQUENT_ACCESS", "STANDARD"];

/// The two values `groupBy` takes, from the model's
/// `ListAggregateLogGroupSummariesGroupBy` enum.
const GROUP_BY_VALUES: [&str; 2] = ["DATA_SOURCE_NAME_AND_TYPE", "DATA_SOURCE_NAME_TYPE_AND_FORMAT"];

/// Create a log group, optionally in a named class and with tags.
async fn create_group(logs: &Logs, name: &str, class: Option<&str>, tags: Option<Value>) {
    let mut body = json!({"logGroupName": name});
    if let Some(class) = class {
        body["logGroupClass"] = json!(class);
    }
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    logs.ok("CreateLogGroup", &body).await;
}

/// The store every matching table runs against.
async fn store_with_groups() -> Logs {
    let logs = Logs::new();
    for name in GROUPS {
        create_group(&logs, name, None, None).await;
    }
    logs
}

/// The `logGroups` member of a `ListLogGroups` response, which a success always carries.
fn log_groups(case: &str, body: &Value) -> Vec<Value> {
    body.get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: a ListLogGroups success carries logGroups, got {body}"))
        .clone()
}

/// The log group names one `ListLogGroups` call reports, in the order it reports them.
async fn listed_names(logs: &Logs, case: &str, request: &Value) -> Vec<String> {
    let body = logs.ok("ListLogGroups", request).await;
    log_groups(case, &body)
        .iter()
        .map(|summary| {
            summary
                .get("logGroupName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a summary carries logGroupName, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The `aggregateLogGroupSummaries` member of a `ListAggregateLogGroupSummaries`
/// response, which a success always carries.
fn aggregate_summaries(case: &str, body: &Value) -> Vec<Value> {
    body.get("aggregateLogGroupSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: a ListAggregateLogGroupSummaries success carries aggregateLogGroupSummaries, got {body}")
        })
        .clone()
}

/// Every form of `logGroupNamePattern`, against the log groups it selects out of
/// [`GROUPS`]. The last row selects nothing, which is a success with an empty list.
fn matching_table() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "one alternative of 47 characters, far past the modelled 24",
            "/aws/lambda/checkout-service-production-handler",
            vec!["/aws/lambda/checkout-service-production-handler"],
        ),
        (
            "a 35-character unanchored substring",
            "checkout-service-production-handler",
            vec!["/aws/lambda/checkout-service-production-handler"],
        ),
        (
            "anchored, so a prefix match",
            "^/aws/lambda",
            vec![
                "/aws/lambda/checkout-service-production-handler",
                "/aws/lambda/payments",
            ],
        ),
        (
            "unanchored, so a substring match anywhere in the name",
            "lambda",
            vec![
                "/aws/lambda/checkout-service-production-handler",
                "/aws/lambda/payments",
            ],
        ),
        (
            "the member's own example: an anchored and an unanchored alternative",
            "^/aws/lambda|discovery",
            vec![
                "/aws/lambda/checkout-service-production-handler",
                "/aws/lambda/payments",
                "discovery",
            ],
        ),
        (
            "all matches are case sensitive",
            "Discovery",
            vec!["Discovery"],
        ),
        (
            "an anchored name that differs from another only in case",
            "^discovery",
            vec!["discovery"],
        ),
        (
            "the punctuation the pattern's character class admits",
            "app.metrics/2024#v1",
            vec!["app.metrics/2024#v1"],
        ),
        (
            "the five alternatives the member allows, anchored and not",
            "^/aws/apigateway|payments|metrics|zzz-none|Discover",
            vec![
                "/aws/apigateway/welcome",
                "/aws/lambda/payments",
                "Discovery",
                "app.metrics/2024#v1",
            ],
        ),
        (
            "a pattern no log group matches",
            "zzz-no-such-log-group",
            vec![],
        ),
    ]
}

/// `logGroupNamePattern` values the member's bounds exclude, which both operations
/// refuse. Each row names the bound it crosses.
fn refused_patterns() -> Vec<(&'static str, String)> {
    let longest = "a".repeat(129);
    vec![
        (
            "shorter than the modelled @length minimum of 3",
            "ab".to_string(),
        ),
        (
            "one character past the modelled @length maximum of 129",
            format!("{longest}a"),
        ),
        (
            "six alternatives, one past the documented five",
            "aaa|bbb|ccc|ddd|eee|fff".to_string(),
        ),
        (
            "an alternative shorter than the documented three characters",
            "/aws/lambda|ab".to_string(),
        ),
        (
            "an anchor with fewer than three characters after it",
            "^ab".to_string(),
        ),
        (
            "an empty alternative, which is no pattern at all",
            "/aws/lambda|".to_string(),
        ),
        (
            "a character outside the documented class",
            "aws lambda".to_string(),
        ),
        (
            "a regular-expression metacharacter the class excludes",
            "^/aws/.*".to_string(),
        ),
    ]
}

/// `logGroupNamePattern` values inside the member's bounds, which both operations
/// accept: each published bound, and the segment length the `@pattern` caps at 24 both
/// at that cap and past it.
fn accepted_patterns() -> Vec<(&'static str, String)> {
    vec![
        (
            "the modelled @length minimum of 3",
            "abc".to_string(),
        ),
        (
            "the modelled @length maximum of 129",
            "a".repeat(129),
        ),
        (
            "an alternative of exactly the @pattern's 24-character cap",
            "a".repeat(24),
        ),
        (
            "an alternative one character past that cap",
            "a".repeat(25),
        ),
        (
            "an alternative of 128 characters, anchored",
            format!("^{}", "a".repeat(128)),
        ),
        (
            "the five alternatives the member allows",
            "aaa|bbb|ccc|ddd|eee".to_string(),
        ),
        (
            "every character the documented class admits",
            "aZ9.-_/#".to_string(),
        ),
    ]
}

/// A `ListAggregateLogGroupSummaries` request carrying `pattern`, grouped the way `index`
/// selects, so the two `groupBy` values are both exercised across a table.
fn aggregate_request(pattern: Option<&str>, index: usize) -> Value {
    let mut body = json!({"groupBy": GROUP_BY_VALUES[index % GROUP_BY_VALUES.len()]});
    if let Some(pattern) = pattern {
        body["logGroupNamePattern"] = json!(pattern);
    }
    body
}

/// Every form of pattern selects exactly the log groups it matches, in log-group-name
/// order, and a pattern that matches nothing is a 200 with an empty list.
#[tokio::test]
async fn a_pattern_selects_the_log_groups_it_matches() {
    let logs = store_with_groups().await;

    for (case, pattern, expected) in matching_table() {
        let names = listed_names(
            &logs,
            case,
            &json!({"logGroupNamePattern": pattern}),
        )
        .await;
        assert_eq!(
            names, expected,
            "case {case} ({pattern}): expected {expected:?}, got {names:?}"
        );
    }

    // No pattern at all: every log group in the store, in name order.
    let names = listed_names(&logs, "no pattern", &json!({})).await;
    assert_eq!(
        names,
        GROUPS.to_vec(),
        "case no pattern: every log group is listed in name order, got {names:?}"
    );
}

/// A pattern inside the member's bounds is answered, not refused — including the segment
/// lengths the modelled `@pattern` caps at 24 — and a pattern outside them is refused
/// with the operation's own modelled error code. Both list operations agree.
#[tokio::test]
async fn pattern_bounds_decide_refusal_not_the_segment_cap() {
    let logs = store_with_groups().await;

    for (index, (case, pattern)) in accepted_patterns().into_iter().enumerate() {
        let body = logs
            .ok("ListLogGroups", &json!({"logGroupNamePattern": pattern}))
            .await;
        // Accepted, so the response is a list — empty where nothing matches.
        log_groups(case, &body);

        let body = logs
            .ok(
                "ListAggregateLogGroupSummaries",
                &aggregate_request(Some(&pattern), index),
            )
            .await;
        aggregate_summaries(case, &body);
    }

    for (index, (case, pattern)) in refused_patterns().into_iter().enumerate() {
        assert_eq!(
            logs.err("ListLogGroups", &json!({"logGroupNamePattern": pattern}))
                .await,
            // `ListLogGroups` models only InvalidParameterException and
            // ServiceUnavailableException, so a refusal carries the former.
            "InvalidParameterException",
            "case {case} ({pattern}): ListLogGroups refuses a pattern outside the member's bounds"
        );
        assert_eq!(
            logs.err(
                "ListAggregateLogGroupSummaries",
                &aggregate_request(Some(&pattern), index),
            )
            .await,
            // The same code: the recordings show CloudWatch Logs answering
            // InvalidParameterException for an input-constraint violation on this
            // operation, not the ValidationException it also models.
            "InvalidParameterException",
            "case {case} ({pattern}): ListAggregateLogGroupSummaries refuses it too"
        );
    }

    // A refusal names the member and the constraint the value broke, on the template
    // this service words a constraint violation with: "N validation error detected: Value
    // at '<member>' failed to satisfy constraint: <constraint>" — carrying nothing of the
    // submitted value, and stating a `@pattern` constraint as the model spells it (the one
    // rendering rule, on `requirement` in src/service.rs).
    for operation in ["ListLogGroups", "ListAggregateLogGroupSummaries"] {
        for (case, pattern, constraint) in [
            ("below the member's minimum length", "ab".to_string(), "length"),
            (
                "a character outside the member's class",
                "aws lambda".to_string(),
                "regular expression pattern",
            ),
        ] {
            let mut request = json!({"logGroupNamePattern": pattern});
            if operation == "ListAggregateLogGroupSummaries" {
                request["groupBy"] = json!(GROUP_BY_VALUES[0]);
            }
            let (status, body) = logs.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation} {case}: the pattern is refused, got {status} {body}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.starts_with("1 validation error detected: Value at \
                                     'logGroupNamePattern' failed to satisfy constraint: ")
                    && message.contains(constraint),
                "case {operation} {case}: the refusal names the member and the constraint on the \
                 service's template, got {message}"
            );
            assert!(
                !message.contains(&pattern),
                "case {operation} {case}: the template carries no submitted value, got {message}"
            );
            let states_the_modelled_pattern = !message.contains("regular expression pattern")
                || message.ends_with(LOG_GROUP_NAME_REGEX_PATTERN);
            assert!(
                states_the_modelled_pattern,
                "case {operation} {case}: a pattern constraint states the modelled expression, \
                 got {message}"
            );
        }
    }
}

/// An empty result is a success: both operations answer 200 with an empty list for a
/// request that selects nothing, whether the store holds no log group at all or holds
/// log groups none of which match.
#[tokio::test]
async fn an_empty_result_is_a_success_not_a_refusal() {
    let empty = Logs::new();
    let populated = store_with_groups().await;

    // Every (store, pattern) pair that selects nothing: an empty store with and without
    // a pattern, and a populated store under a pattern nothing matches.
    let rows: Vec<(&str, &Logs, Option<&str>)> = vec![
        ("an empty store, no pattern", &empty, None),
        (
            "an empty store, an unanchored pattern",
            &empty,
            Some("zzz-no-such-log-group"),
        ),
        (
            "an empty store, an anchored pattern",
            &empty,
            Some("^zzz-none"),
        ),
        (
            "log groups, none of which match",
            &populated,
            Some("zzz-no-such-log-group"),
        ),
        (
            "log groups, none of which match an anchored pattern",
            &populated,
            Some("^zzz-none"),
        ),
    ];

    for (index, (case, logs, pattern)) in rows.into_iter().enumerate() {
        let mut request = json!({});
        if let Some(pattern) = pattern {
            request["logGroupNamePattern"] = json!(pattern);
        }
        let names = listed_names(logs, case, &request).await;
        assert!(
            names.is_empty(),
            "case {case} ({pattern:?}): expected an empty logGroups list, got {names:?}"
        );
        let body = logs.ok("ListLogGroups", &request).await;
        assert_eq!(
            body.get("nextToken"),
            None,
            "case {case} ({pattern:?}): an exhausted enumeration hands back no token, got {body}"
        );

        let body = logs
            .ok(
                "ListAggregateLogGroupSummaries",
                &aggregate_request(pattern, index),
            )
            .await;
        let summaries = aggregate_summaries(case, &body);
        assert!(
            summaries.is_empty(),
            "case {case} ({pattern:?}): no log group in this store is associated with a data \
             source, so the aggregate counts nothing, got {summaries:?}"
        );
        assert_eq!(
            body.get("nextToken"),
            None,
            "case {case} ({pattern:?}): an exhausted enumeration hands back no token, got {body}"
        );
    }
}

/// Each summary is the stored log group: the name, the ARN `DescribeLogGroups` reports
/// for the same group, and the class it was created in. The class filter selects on the
/// same stored value.
#[tokio::test]
async fn a_summary_reports_the_stored_log_group() {
    let logs = Logs::new();
    // One group per class, so no row can pass on a default.
    for (index, class) in LOG_GROUP_CLASSES.iter().enumerate() {
        create_group(&logs, &format!("classes/group-{index}"), Some(class), None).await;
    }

    for (index, class) in LOG_GROUP_CLASSES.iter().enumerate() {
        let name = format!("classes/group-{index}");
        let body = logs
            .ok("ListLogGroups", &json!({"logGroupNamePattern": name}))
            .await;
        let summaries = log_groups(&name, &body);
        assert_eq!(
            summaries.len(),
            1,
            "case {name}: exactly the one group whose name matches, got {summaries:?}"
        );
        let summary = &summaries[0];
        assert_eq!(
            summary.get("logGroupClass").and_then(Value::as_str),
            Some(*class),
            "case {name}: the summary reports the class the group was created in, got {summary}"
        );

        // The ARN is the group's own: the one `DescribeLogGroups` reports for it, built
        // from the request's credential scope.
        let described = logs
            .ok("DescribeLogGroups", &json!({"logGroupIdentifiers": [name]}))
            .await;
        let described = described
            .get("logGroups")
            .and_then(Value::as_array)
            .and_then(|groups| groups.first().cloned())
            .unwrap_or_else(|| panic!("case {name}: DescribeLogGroups reports the group"));
        let arn = summary
            .get("logGroupArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name}: a summary carries logGroupArn, got {summary}"));
        assert_eq!(
            Some(arn),
            described.get("logGroupArn").and_then(Value::as_str),
            "case {name}: the summary's ARN is the one the group reports, got {summary}"
        );
        assert!(
            arn.contains(REGION) && arn.ends_with(&name),
            "case {name}: the ARN names the request's region and the group, got {arn}"
        );

        // `logGroupClass` filters on that same stored value.
        let filtered = listed_names(
            &logs,
            class,
            &json!({"logGroupClass": class, "logGroupNamePattern": "^classes/"}),
        )
        .await;
        assert_eq!(
            filtered,
            vec![name.clone()],
            "case {class}: the class filter selects only the group in that class, got {filtered:?}"
        );
    }
}

/// `logGroupTags` selects on the tags the log group was created with, with the matching
/// its member documents: many values for one key are OR'd, no values means "tagged with
/// that key at all", `*` is a case-insensitive wildcard, `!` negates, and several
/// filters are AND'd.
#[tokio::test]
async fn tag_filters_select_on_the_stored_tags() {
    let logs = Logs::new();
    create_group(
        &logs,
        "tagged/prod-a",
        None,
        Some(json!({"Env": "production", "Team": "core"})),
    )
    .await;
    create_group(&logs, "tagged/prod-b", None, Some(json!({"Env": "Production"}))).await;
    create_group(
        &logs,
        "tagged/dev",
        None,
        Some(json!({"Env": "dev", "Team": "core"})),
    )
    .await;
    create_group(&logs, "tagged/none", None, None).await;
    create_group(&logs, "tagged/empty", None, Some(json!({"Env": ""}))).await;

    let table: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one exact value, matched case-sensitively",
            json!([{"key": "Env", "values": ["production"]}]),
            vec!["tagged/prod-a"],
        ),
        (
            "no values, so any or no value under that key",
            json!([{"key": "Env"}]),
            vec![
                "tagged/dev",
                "tagged/empty",
                "tagged/prod-a",
                "tagged/prod-b",
            ],
        ),
        (
            "more than one value for a key is OR",
            json!([{"key": "Env", "values": ["production", "dev"]}]),
            vec!["tagged/dev", "tagged/prod-a"],
        ),
        (
            "a trailing wildcard, matched case-insensitively",
            json!([{"key": "Env", "values": ["prod*"]}]),
            vec!["tagged/prod-a", "tagged/prod-b"],
        ),
        (
            "a leading wildcard",
            json!([{"key": "Env", "values": ["*duction"]}]),
            vec!["tagged/prod-a", "tagged/prod-b"],
        ),
        (
            "negation, matched case-sensitively",
            json!([{"key": "Env", "values": ["!production"]}]),
            vec!["tagged/dev", "tagged/empty", "tagged/prod-b"],
        ),
        (
            "filters across keys are AND",
            json!([{"key": "Env", "values": ["production"]}, {"key": "Team", "values": ["core"]}]),
            vec!["tagged/prod-a"],
        ),
        (
            "a key no log group carries",
            json!([{"key": "Nope", "values": ["x"]}]),
            vec![],
        ),
    ];

    for (case, filters, expected) in table {
        let names = listed_names(&logs, case, &json!({"logGroupTags": filters})).await;
        assert_eq!(
            names, expected,
            "case {case} ({filters}): expected {expected:?}, got {names:?}"
        );
    }

    // A term carries at most the one negation prefix and the wildcards its own member
    // bounds (`^!?\*?([\p{L}\p{Z}\p{N}_.:/=+\-@]*)\*?$`), so the shapes outside that are
    // refused at the boundary rather than matched here. That member keeps its modelled
    // constraints: only `logGroupNamePattern` carries `skip_validation`.
    for (case, term) in [
        ("a doubled negation prefix", "!!production".to_string()),
        (
            "a long run of negation prefixes",
            format!("{}production", "!".repeat(64)),
        ),
        ("a wildcard inside the term", "prod*tion".to_string()),
    ] {
        let (status, body) = logs
            .call(
                "ListLogGroups",
                &json!({"logGroupTags": [{"key": "Env", "values": [term]}]}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case} ({term}): a tag filter term outside its member's pattern is refused, \
             got {status} {body}"
        );
    }
}

/// The filters this store can only answer negatively still answer 200 with an empty
/// list: no log group here is associated with a data source, and none carries a field
/// index, so a request that requires either selects nothing. A request that searches
/// only another account does the same.
#[tokio::test]
async fn filters_nothing_in_the_store_satisfies_select_nothing() {
    let logs = store_with_groups().await;

    let table: Vec<(&str, Value)> = vec![
        (
            "a data source filter by name",
            json!({"dataSources": [{"name": "amazon_vpc"}]}),
        ),
        (
            "a data source filter by name and type",
            json!({"dataSources": [{"name": "amazon_vpc", "type": "flow"}]}),
        ),
        (
            "one field index name",
            json!({"fieldIndexNames": ["requestId"]}),
        ),
        (
            "several field index names",
            json!({"fieldIndexNames": ["requestId", "accountId"]}),
        ),
        (
            "a search of another account only",
            json!({"includeLinkedAccounts": true, "accountIdentifiers": ["210987654321"]}),
        ),
    ];

    for (case, mut request) in table {
        let names = listed_names(&logs, case, &request).await;
        assert!(
            names.is_empty(),
            "case {case}: nothing in the store satisfies it, got {names:?}"
        );

        // The same filter crossed with a pattern that DOES match is still empty: the
        // filters are combined, not alternatives.
        request["logGroupNamePattern"] = json!("^/aws/lambda");
        let names = listed_names(&logs, case, &request).await;
        assert!(
            names.is_empty(),
            "case {case} with a matching pattern: the filters are combined, got {names:?}"
        );
    }

    // The same request naming THIS account does match, so the rows above are the filter
    // and not a blanket refusal.
    let names = listed_names(
        &logs,
        "a search of this account",
        &json!({
            "includeLinkedAccounts": true,
            "accountIdentifiers": [engine::emulator_account_id()],
            "logGroupNamePattern": "^/aws/lambda",
        }),
    )
    .await;
    assert_eq!(
        names,
        vec![
            "/aws/lambda/checkout-service-production-handler".to_string(),
            "/aws/lambda/payments".to_string(),
        ],
        "case a search of this account: the account's own log groups are listed, got {names:?}"
    );
}

/// `limit` bounds the page and `nextToken` resumes exactly where it left off, for every
/// page size, and the last page of an enumeration hands back no token.
#[tokio::test]
async fn pages_walk_the_whole_match_without_repeating_or_dropping_one() {
    let logs = Logs::new();
    // Seven groups under one prefix, so several page sizes divide the set unevenly.
    let expected: Vec<String> = (0..7).map(|index| format!("paged/group-{index}")).collect();
    for name in &expected {
        create_group(&logs, name, None, None).await;
    }

    for limit in 1..=8 {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"logGroupNamePattern": "^paged/", "limit": limit});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let body = logs.ok("ListLogGroups", &request).await;
            let page = log_groups(&format!("limit {limit}"), &body);
            assert!(
                page.len() <= limit,
                "case limit {limit}: a page holds at most the limit, got {} entries",
                page.len()
            );
            seen.extend(page.iter().map(|summary| {
                summary
                    .get("logGroupName")
                    .and_then(Value::as_str)
                    .expect("a summary carries logGroupName")
                    .to_string()
            }));
            pages += 1;
            assert!(
                pages <= 16,
                "case limit {limit}: the walk terminates, got {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case limit {limit}: the pages are the whole match, once each, in name order"
        );
    }

    // A token is a position in ONE filtered enumeration: continuing with a different
    // filter, or with a token this service never minted, is refused.
    let first = logs
        .ok(
            "ListLogGroups",
            &json!({"logGroupNamePattern": "^paged/", "limit": 1}),
        )
        .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a page with more to come hands back a token, got {first}"))
        .to_string();
    for (case, request) in [
        (
            "a token issued for a different pattern",
            json!({"logGroupNamePattern": "^paged/group-1", "limit": 1, "nextToken": token}),
        ),
        (
            "a token this service never minted",
            json!({"logGroupNamePattern": "^paged/", "limit": 1, "nextToken": "not-a-token"}),
        ),
        (
            "a token minted by another operation",
            json!({"logGroupNamePattern": "^paged/", "limit": 1, "nextToken": "7b226b696e64223a224f74686572227d"}),
        ),
    ] {
        assert_eq!(
            logs.err("ListLogGroups", &request).await,
            "InvalidParameterException",
            "case {case}: a continuation is only valid for the enumeration it came from"
        );
    }

    // The token DOES resume that same enumeration.
    let resumed = listed_names(
        &logs,
        "a token for the same enumeration",
        &json!({"logGroupNamePattern": "^paged/", "limit": 6, "nextToken": token}),
    )
    .await;
    assert_eq!(
        resumed,
        expected[1..].to_vec(),
        "case a token for the same enumeration: it resumes after the page it was minted on"
    );
}

/// Both `groupBy` values are answered for every filter combination, over the grouping the
/// log groups of this store fall in: no operation of this model associates a data source
/// with a log group, so every group here shares the all-unknown grouping, and the summary
/// counts the groups the request selected. A filter that requires a data source selects
/// none of them, which is an empty list and no token.
#[tokio::test]
async fn the_aggregate_answers_every_group_by_and_filter() {
    let logs = store_with_groups().await;
    // The aggregate view lags its log groups, so the whole store is in it only once that
    // span has passed (the lag itself is the subject of
    // `a_log_group_enters_the_aggregate_view_after_its_span`).
    tokio::time::sleep(past_aggregation_lag()).await;

    // (case, the filter, how many of this store's log groups it selects) — `None` where
    // the filter selects no log group at all.
    let filters: Vec<(&str, Value, Option<i64>)> = vec![
        ("no filter at all", json!({}), Some(GROUPS.len() as i64)),
        (
            "an anchored pattern that matches",
            json!({"logGroupNamePattern": "^/aws/lambda"}),
            Some(2),
        ),
        (
            "a pattern longer than the modelled segment cap",
            json!({"logGroupNamePattern": "/aws/lambda/checkout-service-production-handler"}),
            Some(1),
        ),
        (
            // Every group of this store was created in the default class.
            "a class filter",
            json!({"logGroupClass": "STANDARD"}),
            Some(GROUPS.len() as i64),
        ),
        (
            "a class filter no log group here is in",
            json!({"logGroupClass": "INFREQUENT_ACCESS"}),
            None,
        ),
        (
            // "Filters the results by data source characteristics to include only log
            // groups associated with the specified data sources": a log group no data
            // source ingests into is associated with none.
            "a data source filter",
            json!({"dataSources": [{"name": "amazon_vpc", "type": "flow"}]}),
            None,
        ),
        ("a page bound", json!({"limit": 50}), Some(GROUPS.len() as i64)),
        (
            "a search of another account only",
            json!({"includeLinkedAccounts": true, "accountIdentifiers": ["210987654321"]}),
            None,
        ),
    ];

    for group_by in GROUP_BY_VALUES {
        for (case, filter, selected) in &filters {
            let mut request = filter.clone();
            request["groupBy"] = json!(group_by);
            let body = logs
                .ok("ListAggregateLogGroupSummaries", &request)
                .await;
            let summaries = aggregate_summaries(case, &body);
            let Some(selected) = *selected else {
                assert!(
                    summaries.is_empty(),
                    "case {case} grouped by {group_by}: no log group is selected, so there is \
                     nothing to count, got {summaries:?}"
                );
                assert_eq!(
                    body.get("nextToken"),
                    None,
                    "case {case} grouped by {group_by}: nothing follows an empty page, got {body}"
                );
                continue;
            };

            assert_eq!(
                summaries.len(),
                1,
                "case {case} grouped by {group_by}: the selected log groups share one \
                 grouping, got {summaries:?}"
            );
            assert_eq!(
                summaries[0].get("logGroupCount").and_then(Value::as_i64),
                Some(selected),
                "case {case} grouped by {group_by}: expected {selected} log groups in the \
                 grouping, got {:?}",
                summaries[0]
            );
            assert_eq!(
                unknown_identifiers(group_by),
                summaries[0]
                    .get("groupingIdentifiers")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                "case {case} grouped by {group_by}: every characteristic grouped by is \
                 reported, unknown for a log group no data source ingests into, got {:?}",
                summaries[0]
            );
            // "The operation paginates results. By default, it returns up to 50 results
            // and includes a token to retrieve more results": a page carrying a summary
            // carries the token that resumes after it, and walking that token ends the
            // enumeration.
            let token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case} grouped by {group_by}: a page of summaries carries a token, got {body}")
                })
                .to_string();
            request["nextToken"] = json!(token);
            let resumed = logs
                .ok("ListAggregateLogGroupSummaries", &request)
                .await;
            assert!(
                aggregate_summaries(case, &resumed).is_empty()
                    && resumed.get("nextToken").is_none(),
                "case {case} grouped by {group_by}: the continuation past the last grouping \
                 ends the enumeration, got {resumed}"
            );
        }
    }
}

/// The `groupingIdentifiers` a grouping of log groups that no data source ingests into
/// reports, for each `groupBy`: every characteristic it groups by, each unknown.
fn unknown_identifiers(group_by: &str) -> Vec<Value> {
    let mut identifiers = vec![
        json!({"key": "dataSource.Name", "value": "Unknown"}),
        json!({"key": "dataSource.Type", "value": "Unknown"}),
    ];
    if group_by == "DATA_SOURCE_NAME_TYPE_AND_FORMAT" {
        identifiers.push(json!({"key": "dataSource.Format", "value": "Unknown"}));
    }
    identifiers
}

/// The aggregate view LAGS its log groups: a group aggregated straight after its create is
/// not counted yet, and the same request made once the span has passed counts it — while
/// `ListLogGroups` reports it immediately throughout, so the lag belongs to the aggregate
/// view alone and not to the store.
#[tokio::test]
async fn a_log_group_enters_the_aggregate_view_after_its_span() {
    let logs = Logs::new();
    // Three groups created one after another, so the assertion is about the span and not
    // about one name.
    let names = ["lagging/one", "lagging/two", "lagging/three"];
    for name in names {
        create_group(&logs, name, None, None).await;
    }

    for group_by in GROUP_BY_VALUES {
        let fresh = logs
            .ok(
                "ListAggregateLogGroupSummaries",
                &json!({"groupBy": group_by, "logGroupNamePattern": "^lagging/"}),
            )
            .await;
        assert!(
            aggregate_summaries("straight after the creates", &fresh).is_empty()
                && fresh.get("nextToken").is_none(),
            "case grouped by {group_by}: a log group is not in the aggregate view yet, got \
             {fresh}"
        );
    }

    // The same groups ARE listed meanwhile: only the aggregate view lags.
    let listed = listed_names(
        &logs,
        "straight after the creates",
        &json!({"logGroupNamePattern": "^lagging/"}),
    )
    .await;
    assert_eq!(
        listed.len(),
        names.len(),
        "case ListLogGroups: a created log group is listed at once, got {listed:?}"
    );

    tokio::time::sleep(past_aggregation_lag()).await;

    for group_by in GROUP_BY_VALUES {
        let settled = logs
            .ok(
                "ListAggregateLogGroupSummaries",
                &json!({"groupBy": group_by, "logGroupNamePattern": "^lagging/"}),
            )
            .await;
        let summaries = aggregate_summaries("once the span has passed", &settled);
        assert_eq!(
            summaries.len(),
            1,
            "case grouped by {group_by}: the three groups share one grouping, got \
             {summaries:?}"
        );
        assert_eq!(
            summaries[0].get("logGroupCount").and_then(Value::as_i64),
            Some(names.len() as i64),
            "case grouped by {group_by}: every group is counted once the span has passed, \
             got {:?}",
            summaries[0]
        );
        assert_eq!(
            unknown_identifiers(group_by),
            summaries[0]
                .get("groupingIdentifiers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            "case grouped by {group_by}: the grouping is the all-unknown one, got {:?}",
            summaries[0]
        );
    }
}
