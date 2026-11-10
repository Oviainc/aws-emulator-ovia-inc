// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the subscription-filter loop:
//! `PutSubscriptionFilter` / `DescribeSubscriptionFilters`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rules generalise over (destination kind,
//! filter-pattern form, distribution, the `emitSystemFields` catalogue, name order,
//! page size, the per-group quota); any single filter an issue happens to mention is
//! one row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs, REGION};

/// The account the destination ARNs in these tables name. A destination belongs to
/// another service, so the emulator never mints these ARNs itself.
const ACCOUNT: &str = "123456789012";

/// The subscription filters one log group can hold ATTACHED at a time: the service accepts
/// five on one log group and refuses the next put.
const MAX_FILTERS_PER_GROUP: usize = 5;

/// The published bounds of `DescribeSubscriptionFilters$limit`. Source: the
/// `DescribeLimit` shape, `range 1..50`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// The system fields a subscription filter may emit. Source:
/// `PutSubscriptionFilter$emitSystemFields` — "Valid values are @aws.account,
/// @aws.region, and @source.log".
const SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

/// The values `distribution` takes. Source: the model's `Distribution` enum
/// (`ByLogStream` | `Random`).
const DISTRIBUTIONS: [&str; 2] = ["ByLogStream", "Random"];

/// The grouping a filter is configured with when the put named no `distribution`.
/// Source: `PutSubscriptionFilter$distribution` — "By default, log data is grouped by
/// log stream, but the grouping can be set to random for a more even distribution."
const DEFAULT_DISTRIBUTION: &str = DISTRIBUTIONS[0];

/// The bracketed forms a filter pattern can open, as `(open, close)` pairs. Source: the
/// filter-pattern syntax `PutSubscriptionFilter` references — a JSON selector pattern is
/// wrapped in `{ … }`, a space-delimited pattern in `[ … ]`.
const PATTERN_DELIMITERS: [(char, char); 2] = [('{', '}'), ('[', ']')];

/// One put row: everything the request names, so the assertions can be derived from it.
#[derive(Debug, Clone)]
struct Row {
    group: &'static str,
    filter: &'static str,
    pattern: &'static str,
    destination: String,
    role: Option<String>,
    distribution: Option<&'static str>,
    apply_on_transformed: bool,
    criteria: Option<&'static str>,
    system_fields: Option<&'static [&'static str]>,
}

impl Row {
    /// The `PutSubscriptionFilter` request body this row describes: a member the row
    /// leaves out is left out of the request too.
    fn put_body(&self) -> Value {
        let mut body = json!({
            "logGroupName": self.group,
            "filterName": self.filter,
            "filterPattern": self.pattern,
            "destinationArn": self.destination,
            "applyOnTransformedLogs": self.apply_on_transformed,
        });
        if let Some(role) = &self.role {
            body["roleArn"] = json!(role);
        }
        if let Some(distribution) = self.distribution {
            body["distribution"] = json!(distribution);
        }
        if let Some(criteria) = self.criteria {
            body["fieldSelectionCriteria"] = json!(criteria);
        }
        if let Some(fields) = self.system_fields {
            body["emitSystemFields"] = json!(fields);
        }
        body
    }
}

/// An ARN in another service's namespace, of the form that service publishes. The
/// account is another one than the emulator's own, so a cross-account destination is
/// distinguishable from a same-account one.
fn arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{ACCOUNT}:{resource}")
}

/// An ARN owned by the emulator's OWN account — the account every ARN it mints carries,
/// so a destination named with it is a same-account destination.
fn own_account_arn(service: &str, resource: &str) -> String {
    format!(
        "arn:aws:{service}:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// An IAM role ARN — the `roleArn` form, which carries no region because IAM is global.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The rows the round-trip test spans: one per destination kind
/// `PutSubscriptionFilter` documents (a Kinesis data stream, a Firehose delivery
/// stream, a Lambda function, and a logical destination created with `PutDestination`),
/// crossed with the filter-pattern forms, both `distribution` values, both
/// `applyOnTransformedLogs` values and different `emitSystemFields` subsets.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            group: "alpha-group",
            filter: "to-kinesis",
            pattern: "ERROR",
            destination: arn("kinesis", "stream/log-stream-target"),
            role: Some(role_arn("CWLtoKinesisRole")),
            distribution: Some("Random"),
            apply_on_transformed: false,
            criteria: None,
            system_fields: None,
        },
        Row {
            group: "nested/group.name_1",
            filter: "firehose filter",
            pattern: "{ $.eventType = \"UpdateTrail\" }",
            destination: arn("firehose", "deliverystream/cloudwatch-log-processing"),
            role: Some(role_arn("CWLtoFirehoseRole")),
            distribution: None,
            apply_on_transformed: true,
            criteria: Some("@aws.region NOT IN [\"cn-north-1\"]"),
            system_fields: Some(&["@aws.account", "@aws.region"]),
        },
        Row {
            group: "zz",
            filter: "lambda/fn",
            // `filterPattern` is modelled `len 0..1024`, so the empty pattern — every
            // log event — is a legal row.
            pattern: "",
            destination: arn("lambda", "function:log-consumer"),
            role: None,
            distribution: None,
            apply_on_transformed: false,
            criteria: None,
            system_fields: Some(&["@source.log"]),
        },
        Row {
            group: "group#4",
            filter: "x",
            pattern: "%(?i)(error|warn)%",
            destination: arn("logs", "destination:cross-account-destination"),
            role: None,
            distribution: Some("ByLogStream"),
            apply_on_transformed: false,
            criteria: Some("@aws.account = \"123456789012\" AND @aws.region = \"us-east-1\""),
            system_fields: Some(&SYSTEM_FIELDS),
        },
    ]
}

/// Create a log group, which is what a subscription filter is associated with.
async fn create_group(logs: &Logs, group: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
}

/// The `subscriptionFilters` of one log group, in the order the service returns them.
async fn filters_of(logs: &Logs, group: &str) -> Vec<Value> {
    let page = logs
        .ok(
            "DescribeSubscriptionFilters",
            &json!({"logGroupName": group, "limit": LIMIT_MAX}),
        )
        .await;
    subscription_filters(&page)
}

/// The one filter a group holds under `name`.
async fn one_filter(logs: &Logs, group: &str, name: &str) -> Value {
    let found = filters_of(logs, group).await;
    found
        .iter()
        .find(|entry| entry.get("filterName").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| {
            panic!("case {group}/{name}: the filter must be readable back, got {found:?}")
        })
        .clone()
}

/// The `subscriptionFilters` list of a `DescribeSubscriptionFilters` page.
fn subscription_filters(page: &Value) -> Vec<Value> {
    page.get("subscriptionFilters")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("DescribeSubscriptionFilters must return subscriptionFilters, got {page}")
        })
        .clone()
}

/// The `filterName` values of a page, in the order returned.
fn filter_names(page: &Value) -> Vec<String> {
    names_of(&subscription_filters(page))
}

/// The `filterName` values of a list of filters, in the order given.
fn names_of(filters: &[Value]) -> Vec<String> {
    filters
        .iter()
        .map(|filter| {
            filter
                .get("filterName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every filter carries a filterName, got {filter}"))
                .to_string()
        })
        .collect()
}

/// A member of a returned filter, as a string.
fn member<'a>(filter: &'a Value, name: &str) -> Option<&'a str> {
    filter.get(name).and_then(Value::as_str)
}

/// Walk every page of one `DescribeSubscriptionFilters` request at `limit`, asserting
/// each page respects the limit, and answer the filter names in the order returned.
async fn walk(logs: &Logs, request: &Value, limit: i64) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..256 {
        let mut body = request.clone();
        body["limit"] = json!(limit);
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok("DescribeSubscriptionFilters", &body).await;
        let returned = subscription_filters(&page);
        assert!(
            returned.len() <= usize::try_from(limit).expect("a positive limit"),
            "case limit {limit} page {page_index}: expected at most {limit} filters, got {}",
            returned.len()
        );
        names.extend(filter_names(&page));
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case limit {limit}: pagination did not terminate, collected {names:?}");
}

/// Every accepted put is readable back through `DescribeSubscriptionFilters` with the
/// members it named, the log group it was associated with, a live creation time, and no
/// member it did not name — over the whole class of destinations, patterns,
/// distributions and system-field subsets.
#[tokio::test]
async fn put_is_readable_back_through_describe() {
    let logs = Logs::new();
    let before = now_ms();

    for row in round_trip_rows() {
        create_group(&logs, row.group).await;
        logs.ok("PutSubscriptionFilter", &row.put_body()).await;

        let found = one_filter(&logs, row.group, row.filter).await;
        assert_eq!(
            member(&found, "filterName"),
            Some(row.filter),
            "case {row:?}: filterName must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "logGroupName"),
            Some(row.group),
            "case {row:?}: the filter belongs to the log group it was put on, got {found}"
        );
        assert_eq!(
            member(&found, "filterPattern"),
            Some(row.pattern),
            "case {row:?}: filterPattern must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "destinationArn"),
            Some(row.destination.as_str()),
            "case {row:?}: destinationArn must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "roleArn"),
            row.role.as_deref(),
            "case {row:?}: roleArn is reported only when it was specified, got {found}"
        );
        assert_eq!(
            member(&found, "distribution"),
            Some(row.distribution.unwrap_or(DEFAULT_DISTRIBUTION)),
            "case {row:?}: distribution is what was specified, or the documented default, got {found}"
        );
        assert_eq!(
            found.get("applyOnTransformedLogs").and_then(Value::as_bool),
            Some(row.apply_on_transformed),
            "case {row:?}: applyOnTransformedLogs must be echoed, got {found}"
        );
        assert_eq!(
            member(&found, "fieldSelectionCriteria"),
            row.criteria,
            "case {row:?}: fieldSelectionCriteria is reported only when it was specified, got {found}"
        );
        assert_eq!(
            found
                .get("emitSystemFields")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()),
            row.system_fields
                .map(|fields| fields.iter().map(ToString::to_string).collect::<Vec<_>>()),
            "case {row:?}: emitSystemFields is reported only when it was specified, got {found}"
        );
        let creation_time = found
            .get("creationTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {row:?}: a filter carries a creationTime, got {found}"));
        assert!(
            (before..=now_ms()).contains(&creation_time),
            "case {row:?}: creationTime must be the wall clock of the put, got {creation_time}"
        );
    }
}

/// A put under a name the group already holds updates that filter in place: it does not
/// add a second filter, it keeps the creation time, and every other member is replaced
/// by what the new put names — including being dropped when the new put omits it.
#[tokio::test]
async fn put_under_one_name_updates_in_place() {
    let logs = Logs::new();
    let group = "updated-group";
    create_group(&logs, group).await;

    let rows = round_trip_rows();
    let name = "one-name";
    let mut creation_time: Option<i64> = None;
    for row in &rows {
        let mut row = row.clone();
        row.group = group;
        row.filter = name;
        logs.ok("PutSubscriptionFilter", &row.put_body()).await;

        let found = filters_of(&logs, group).await;
        assert_eq!(
            names_of(&found),
            vec![name.to_string()],
            "case {row:?}: a put under one name is an update, not a second filter"
        );
        let filter = found.first().expect("the one filter").clone();
        assert_eq!(
            member(&filter, "destinationArn"),
            Some(row.destination.as_str()),
            "case {row:?}: an update replaces destinationArn, got {filter}"
        );
        assert_eq!(
            member(&filter, "distribution"),
            Some(row.distribution.unwrap_or(DEFAULT_DISTRIBUTION)),
            "case {row:?}: an update that omits distribution resets it to the default, got {filter}"
        );
        assert_eq!(
            member(&filter, "roleArn"),
            row.role.as_deref(),
            "case {row:?}: an update drops roleArn when the put omits it, got {filter}"
        );
        assert_eq!(
            member(&filter, "fieldSelectionCriteria"),
            row.criteria,
            "case {row:?}: an update drops fieldSelectionCriteria when the put omits it, got {filter}"
        );

        let now = filter.get("creationTime").and_then(Value::as_i64);
        match creation_time {
            None => creation_time = now,
            Some(first) => assert_eq!(
                now,
                Some(first),
                "case {row:?}: an update keeps the filter's creation time"
            ),
        }
    }
}

/// The filters of a group are reported ASCII-sorted by filter name, whatever order they
/// were put in; `filterNamePrefix` narrows that enumeration; and paging walks it exactly
/// once at every page size.
#[tokio::test]
async fn filters_are_sorted_prefix_filtered_and_paged() {
    let logs = Logs::new();
    // Rows differ in the order the names are put in and in where they diverge, so the
    // reported order cannot be the insertion order.
    let cases: [[&str; MAX_FILTERS_PER_GROUP]; 3] = [
        ["b", "a", "d", "c", "e"],
        ["alpha-2", "alpha-1", "alpha-10", "beta-1", "alpha-3"],
        ["Z", "a", "A", "z", "M"],
    ];

    for (index, names) in cases.iter().enumerate() {
        let group = format!("sorted-group-{index}");
        create_group(&logs, &group).await;
        for name in names {
            logs.ok(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": name,
                    "filterPattern": "",
                    "destinationArn": arn("firehose", "deliverystream/target"),
                    "roleArn": role_arn("CWLtoFirehoseRole"),
                }),
            )
            .await;
        }
        let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
        expected.sort();

        assert_eq!(
            filter_names(
                &logs
                    .ok(
                        "DescribeSubscriptionFilters",
                        &json!({"logGroupName": group}),
                    )
                    .await
            ),
            expected,
            "case {names:?}: the results are ASCII-sorted by filter name"
        );

        // Every page size from the published minimum through the maximum walks the
        // enumeration exactly once, in that order.
        for limit in [LIMIT_MIN, 2, LIMIT_MAX] {
            assert_eq!(
                walk(&logs, &json!({"logGroupName": group}), limit).await,
                expected,
                "case {names:?} limit {limit}: paging returns every filter exactly once"
            );
        }

        // A prefix selects the names that start with it, and nothing else.
        let prefix = &expected[0][..1];
        let selected: Vec<String> = expected
            .iter()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect();
        assert_eq!(
            filter_names(
                &logs
                    .ok(
                        "DescribeSubscriptionFilters",
                        &json!({"logGroupName": group, "filterNamePrefix": prefix}),
                    )
                    .await
            ),
            selected,
            "case {names:?} prefix {prefix}: only names with that prefix are listed"
        );
    }

    // One past each published bound of `limit` is refused by the modelled `range 1..50`.
    let group = "sorted-group-0";
    for limit in [LIMIT_MIN - 1, LIMIT_MAX + 1] {
        let (status, body) = logs
            .call(
                "DescribeSubscriptionFilters",
                &json!({"logGroupName": group, "limit": limit}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case limit {limit}: a limit outside {LIMIT_MIN}..{LIMIT_MAX} is refused, got {status} {body}"
        );
    }

    // A token this operation did not mint, and one minted for a different enumeration,
    // are both refused rather than misread.
    let token = logs
        .ok(
            "DescribeSubscriptionFilters",
            &json!({"logGroupName": group, "limit": LIMIT_MIN}),
        )
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page smaller than the enumeration carries a nextToken")
        .to_string();
    for (case, request) in [
        (
            "a token from another operation",
            json!({"logGroupName": group, "limit": LIMIT_MIN, "nextToken": "not-a-token"}),
        ),
        (
            "a token from another enumeration",
            json!({
                "logGroupName": group,
                "limit": LIMIT_MIN,
                "filterNamePrefix": "a",
                "nextToken": token,
            }),
        ),
    ] {
        assert_eq!(
            logs.err("DescribeSubscriptionFilters", &request).await,
            "InvalidParameterException",
            "case {case}: a token is only a position within the enumeration that issued it"
        );
    }
}

/// The per-group quota: the filters one log group holds, the next put refused, and a put
/// over a name already stored still accepted because it is an update. The quota is per log
/// group, so another group takes its own.
#[tokio::test]
async fn the_quota_is_counted_per_log_group() {
    let logs = Logs::new();
    let destination = arn("kinesis", "stream/quota-target");
    let role = role_arn("CWLtoKinesisRole");

    for group in ["quota-group-a", "quota-group-b"] {
        create_group(&logs, group).await;
        for index in 0..MAX_FILTERS_PER_GROUP {
            logs.ok(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": format!("filter-{index}"),
                    "filterPattern": "",
                    "destinationArn": destination,
                    "roleArn": role,
                }),
            )
            .await;
        }
        assert_eq!(
            logs.err(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": "one-too-many",
                    "filterPattern": "",
                    "destinationArn": destination,
                    "roleArn": role,
                }),
            )
            .await,
            "LimitExceededException",
            "case {group}: a log group holds at most {MAX_FILTERS_PER_GROUP} subscription filters"
        );
        // At the quota, replacing a filter already stored is still accepted.
        logs.ok(
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "filter-0",
                "filterPattern": "UPDATED",
                "destinationArn": destination,
                "roleArn": role,
            }),
        )
        .await;
        assert_eq!(
            filters_of(&logs, group).await.len(),
            MAX_FILTERS_PER_GROUP,
            "case {group}: an update at the quota does not add a filter"
        );
    }
}

/// What a put and a describe refuse: a log group that does not exist, a `destinationArn`
/// that names none of the supported destination kinds, a system field outside the
/// documented catalogue, and a describe of a group that does not exist.
#[tokio::test]
async fn requests_that_cannot_be_stored_are_refused() {
    let logs = Logs::new();
    let group = "refusal-group";
    create_group(&logs, group).await;
    let destination = arn("firehose", "deliverystream/target");
    let role = role_arn("CWLtoFirehoseRole");

    // "Creates or updates a subscription filter and associates it with the specified log
    // group": there is no such group to associate it with.
    assert_eq!(
        logs.err(
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": "no-such-group",
                "filterName": "orphan",
                "filterPattern": "",
                "destinationArn": destination,
                "roleArn": role,
            }),
        )
        .await,
        "ResourceNotFoundException",
        "case an unknown log group: a filter is associated with a log group that exists"
    );
    assert_eq!(
        logs.err(
            "DescribeSubscriptionFilters",
            &json!({"logGroupName": "no-such-group"}),
        )
        .await,
        "ResourceNotFoundException",
        "case describe of an unknown log group: there are no filters to list"
    );

    // The destination kinds `PutSubscriptionFilter` does NOT document: another
    // service's resource, and CloudWatch Logs resources that are not logical
    // destinations.
    let group_arn = format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{group}");
    for (case, candidate) in [
        ("an S3 bucket", "arn:aws:s3:::my-bucket".to_string()),
        ("an SNS topic", arn("sns", "log-topic")),
        ("an SQS queue", arn("sqs", "log-queue")),
        ("a log group", group_arn.clone()),
        ("a log group with the wildcard suffix", format!("{group_arn}:*")),
        (
            "a log stream",
            format!("{group_arn}:log-stream:a-stream"),
        ),
        ("not an ARN at all", "cloudwatch-log-processing".to_string()),
    ] {
        assert_eq!(
            logs.err(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": "rejected",
                    "filterPattern": "",
                    "destinationArn": candidate,
                    "roleArn": role,
                }),
            )
            .await,
            "InvalidParameterException",
            "case {case} ({candidate}): it is not a supported subscription destination"
        );
    }

    // "Valid values are @aws.account, @aws.region, and @source.log."
    for (case, fields) in [
        ("an unmodelled field", vec!["@aws.partition"]),
        ("a valid field misspelled", vec!["@aws.Region"]),
        (
            "a valid field beside an invalid one",
            vec![SYSTEM_FIELDS[0], "@source.ip"],
        ),
    ] {
        assert_eq!(
            logs.err(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": "rejected",
                    "filterPattern": "",
                    "destinationArn": destination,
                    "roleArn": role,
                    "emitSystemFields": fields,
                }),
            )
            .await,
            "InvalidParameterException",
            "case {case} ({fields:?}): emitSystemFields is held to its documented catalogue"
        );
    }

    // Nothing a refusal touched was stored.
    assert_eq!(
        filters_of(&logs, group).await,
        Vec::<Value>::new(),
        "case after the refusals: a refused put persists nothing"
    );
}

/// A subscription filter is associated WITH a log group, so deleting the group takes its
/// filters with it: a group recreated under the same name holds none.
#[tokio::test]
async fn deleting_the_log_group_removes_its_filters() {
    let logs = Logs::new();
    let group = "ephemeral-group";
    create_group(&logs, group).await;
    logs.ok(
        "PutSubscriptionFilter",
        &json!({
            "logGroupName": group,
            "filterName": "doomed",
            "filterPattern": "",
            "destinationArn": arn("lambda", "function:log-consumer"),
        }),
    )
    .await;
    assert_eq!(
        names_of(&filters_of(&logs, group).await),
        vec!["doomed".to_string()],
        "case before the delete: the filter is stored on the group"
    );

    logs.ok("DeleteLogGroup", &json!({"logGroupName": group}))
        .await;
    create_group(&logs, group).await;
    assert_eq!(
        filters_of(&logs, group).await,
        Vec::<Value>::new(),
        "case after recreating the group: its filters went with the group"
    );
}

/// The `distribution` a filter is configured with, over the whole class: each value the
/// `Distribution` enum declares, and the put that names none — which takes the
/// documented default, so every filter reports a grouping. A value outside the enum is
/// refused.
#[tokio::test]
async fn distribution_is_reported_and_defaults_to_the_documented_grouping() {
    let logs = Logs::new();
    let group = "distribution-group";
    create_group(&logs, group).await;
    let name = "distributed";
    let put = |distribution: Option<&str>| {
        let mut body = json!({
            "logGroupName": group,
            "filterName": name,
            "filterPattern": "",
            "destinationArn": arn("lambda", "function:log-consumer"),
        });
        if let Some(distribution) = distribution {
            body["distribution"] = json!(distribution);
        }
        body
    };

    // One row per enum value, plus the row that names none.
    let mut cases: Vec<(Option<&str>, &str)> = vec![(None, DEFAULT_DISTRIBUTION)];
    cases.extend(DISTRIBUTIONS.iter().map(|value| (Some(*value), *value)));
    for (requested, expected) in cases {
        logs.ok("PutSubscriptionFilter", &put(requested)).await;
        let found = one_filter(&logs, group, name).await;
        assert_eq!(
            member(&found, "distribution"),
            Some(expected),
            "case distribution {requested:?}: expected {expected}, got {found}"
        );
    }

    // A value outside the modelled `Distribution` enum — including an enum value in the
    // wrong case — is refused rather than stored.
    for candidate in ["bylogstream", "RANDOM", "RoundRobin"] {
        let (status, body) = logs
            .call("PutSubscriptionFilter", &put(Some(candidate)))
            .await;
        assert_eq!(
            status, 400,
            "case distribution {candidate}: only a modelled Distribution value is accepted, got {status} {body}"
        );
    }
}

/// A filter pattern is parsed when the filter is configured: one that opens a bracketed
/// form must close it with the matching delimiter, and the brackets nested inside that
/// form must balance. A pattern that opens no bracketed form has no bracket structure to
/// hold to, and a refused put stores nothing.
#[tokio::test]
async fn an_unclosed_bracketed_filter_pattern_is_refused() {
    let logs = Logs::new();
    let accepted_group = "pattern-group";
    let refused_group = "pattern-refusal-group";
    create_group(&logs, accepted_group).await;
    create_group(&logs, refused_group).await;
    let destination = arn("lambda", "function:log-consumer");
    let name = "patterned";
    let put = |group: &str, pattern: &str| {
        json!({
            "logGroupName": group,
            "filterName": name,
            "filterPattern": pattern,
            "destinationArn": destination,
        })
    };

    // Every pattern form the syntax carries, closed: the empty pattern, a term pattern,
    // a regular-expression pattern, a JSON pattern (grouped, indexed, carrying a quoted
    // brace and a bracketed regex), a space-delimited pattern, and one surrounded by
    // whitespace.
    for pattern in [
        "",
        "ERROR",
        "?ERROR ?WARN",
        "%(?i)(error|warn)%",
        "{ $.level = \"ERROR\" }",
        "{ ($.a = 1) || ($.b = 2) }",
        "{ $.items[0].id = 7 }",
        "{ $.items[*].id = 7 }",
        "{ $.message = \"a } brace inside a string\" }",
        "{ $.message = %^\\[ERROR\\]% }",
        "[w1=ERROR, w2]",
        "  { $.level = \"ERROR\" }  ",
    ] {
        logs.ok("PutSubscriptionFilter", &put(accepted_group, pattern))
            .await;
        let found = one_filter(&logs, accepted_group, name).await;
        assert_eq!(
            member(&found, "filterPattern"),
            Some(pattern),
            "case pattern {pattern:?}: a closed pattern is stored as given, got {found}"
        );
    }

    // A pattern that opens a bracketed form and does not close it is refused with the
    // rule, stated for the delimiter pair it opened.
    let unclosed: Vec<(String, String)> = PATTERN_DELIMITERS
        .iter()
        .flat_map(|(open, close)| {
            let rule = format!("If a filter pattern starts with '{open}' it must end with '{close}'");
            [
                format!("{open} $.level = "),
                format!("{open} $.level = \"ERROR\""),
                format!("{open}w1=ERROR, w2"),
            ]
            .map(|pattern| (pattern, rule.clone()))
        })
        .collect();
    for (pattern, rule) in &unclosed {
        let (code, message) = logs
            .refusal("PutSubscriptionFilter", &put(refused_group, pattern))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case pattern {pattern:?}: an unclosed pattern is refused, got {code} {message}"
        );
        assert_eq!(
            &message, rule,
            "case pattern {pattern:?}: the refusal states the rule, got {message}"
        );
    }

    // A pattern that closes the form it opened but whose structure does not close inside
    // it is refused too — the parse covers the whole pattern, not just its ends — with the
    // one fixed text the service answers for a pattern it cannot read. A STRUCTURED form is
    // read whole at configuration time, so a condition the service cannot read is refused
    // where it is written rather than stored to select nothing.
    let unreadable = "Invalid filter pattern".to_string();
    for (pattern, expected) in [
        ("{ { $.level = \"ERROR\" }", unreadable.clone()),
        ("{ $.level = \"ERROR\" } }", unreadable.clone()),
        ("{ ($.a = 1 }", unreadable.clone()),
        ("[w1=ERROR, w2]]", unreadable.clone()),
        ("{ $.message = \"unterminated }", unreadable.clone()),
        ("{ $.message = %unterminated }", unreadable.clone()),
        // A space-delimited slot's condition is read as a condition: a bare parenthesis in
        // one is no part of the syntax it states.
        ("[w1=ERROR, w2=f(x]", unreadable.clone()),
    ] {
        let (code, message) = logs
            .refusal("PutSubscriptionFilter", &put(refused_group, pattern))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case pattern {pattern:?}: a pattern that does not close is refused, got {code} {message}"
        );
        assert_eq!(
            message, expected,
            "case pattern {pattern:?}: the refusal is the fixed text for an unreadable \
             pattern, got {message}"
        );
    }

    // Nothing a refused put named was stored, so the group advertises no subscription.
    assert_eq!(
        filters_of(&logs, refused_group).await,
        Vec::<Value>::new(),
        "case after the refused patterns: a refused put persists nothing"
    );
}

/// A member this operation parses as an ARN, handed a value that is not an ARN, is
/// refused with the service's request-derived validation-error message: it names the
/// member and the value it could not parse. A well-formed ARN naming a resource the
/// operation does not deliver to is a different refusal, reporting the resource kind
/// rather than a parse failure.
#[tokio::test]
async fn a_member_that_is_not_an_arn_reports_a_parse_failure() {
    let logs = Logs::new();
    let group = "arn-group";
    create_group(&logs, group).await;
    let destination = arn("lambda", "function:log-consumer");

    // Values that are not ARNs: a bare name, a hostname, a truncated ARN missing the
    // segments an ARN has, and one whose scheme is not `arn:`.
    for candidate in [
        "not-a-valid-arn",
        "cloudwatch-log-processing",
        "lambda.amazonaws.com",
        "arn:aws:lambda",
        "arn:aws:lambda:eu-west-2",
        "aws:lambda:eu-west-2:123456789012:function:log-consumer",
        " ",
    ] {
        // The rule holds for every ARN member of the request, not only the one the
        // destination is named in.
        for (member_name, body) in [
            (
                "destinationArn",
                json!({
                    "logGroupName": group,
                    "filterName": "rejected",
                    "filterPattern": "",
                    "destinationArn": candidate,
                }),
            ),
            (
                "roleArn",
                json!({
                    "logGroupName": group,
                    "filterName": "rejected",
                    "filterPattern": "",
                    "destinationArn": destination,
                    "roleArn": candidate,
                }),
            ),
        ] {
            let (code, message) = logs.refusal("PutSubscriptionFilter", &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {member_name} {candidate:?}: a value that is not an ARN is refused, got {code} {message}"
            );
            assert_eq!(
                message,
                format!("1 validation error detected: Unable to parse {member_name} {candidate}"),
                "case {member_name} {candidate:?}: the refusal names the member and the value, got {message}"
            );
        }
    }

    // A value that IS an ARN, naming a resource this operation does not deliver to, is
    // refused for the resource it names — not reported as a parse failure.
    let group_arn = format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{group}");
    for candidate in [
        "arn:aws:s3:::my-bucket".to_string(),
        arn("sns", "log-topic"),
        group_arn,
    ] {
        let (code, message) = logs
            .refusal(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": "rejected",
                    "filterPattern": "",
                    "destinationArn": candidate,
                    "roleArn": role_arn("CWLtoTargetRole"),
                }),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {candidate}: an unsupported destination is refused, got {code} {message}"
        );
        assert!(
            !message.starts_with("1 validation error detected"),
            "case {candidate}: an ARN that parses is not reported as a parse failure, got {message}"
        );
    }

    // No refused put stored anything.
    assert_eq!(
        filters_of(&logs, group).await,
        Vec::<Value>::new(),
        "case after the refusals: a refused put persists nothing"
    );
}

/// Delivery to a stream of one of the two streaming services named as a DIRECT destination
/// goes through a role CloudWatch Logs assumes, so a put that names no `roleArn` is refused
/// and the refusal names the vendor it was asked to deliver to. A Lambda function is invoked
/// directly, and a logical destination carries the role it was created with, so neither
/// needs a role.
#[tokio::test]
async fn a_destination_that_needs_a_role_is_refused_without_one() {
    let logs = Logs::new();
    let group = "role-group";
    create_group(&logs, group).await;
    let name = "to-destination";
    let put = |destination: &str, role: Option<String>| {
        let mut body = json!({
            "logGroupName": group,
            "filterName": name,
            "filterPattern": "",
            "destinationArn": destination,
        });
        if let Some(role) = role {
            body["roleArn"] = json!(role);
        }
        body
    };

    // One row per vendor that needs a role: the streaming destinations CloudWatch Logs
    // delivers to by assuming the role the put names.
    for (vendor, destination) in [
        ("kinesis", arn("kinesis", "stream/role-target")),
        ("firehose", arn("firehose", "deliverystream/role-target")),
    ] {
        let (code, message) = logs
            .refusal("PutSubscriptionFilter", &put(&destination, None))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case vendor {vendor} without roleArn: the put is refused, got {code} {message}"
        );
        assert_eq!(
            message,
            format!("destinationArn for vendor {vendor} cannot be used without roleArn"),
            "case vendor {vendor}: the refusal names the vendor, got {message}"
        );
        assert_eq!(
            filters_of(&logs, group).await,
            Vec::<Value>::new(),
            "case vendor {vendor}: a put refused for a missing roleArn stores nothing"
        );

        // The same put, with a role, is accepted and readable back.
        let role = role_arn("CWLtoTargetRole");
        logs.ok(
            "PutSubscriptionFilter",
            &put(&destination, Some(role.clone())),
        )
        .await;
        let found = one_filter(&logs, group, name).await;
        assert_eq!(
            (
                member(&found, "destinationArn"),
                member(&found, "roleArn")
            ),
            (Some(destination.as_str()), Some(role.as_str())),
            "case vendor {vendor}: the configured destination and role are reported, got {found}"
        );
        logs.ok(
            "DeleteSubscriptionFilter",
            &json!({"logGroupName": group, "filterName": name}),
        )
        .await;
    }

    // What needs no role at all: a Lambda function, which is invoked directly, and a
    // logical destination, which was given its own role by `PutDestination` — whichever
    // account holds it.
    for destination in [
        arn("lambda", "function:log-consumer"),
        arn("logs", "destination:cross-account-destination"),
        own_account_arn("logs", "destination:same-account-destination"),
    ] {
        logs.ok("PutSubscriptionFilter", &put(&destination, None))
            .await;
        let found = one_filter(&logs, group, name).await;
        assert_eq!(
            member(&found, "destinationArn"),
            Some(destination.as_str()),
            "case {destination}: it needs no roleArn, got {found}"
        );
        assert_eq!(
            member(&found, "roleArn"),
            None,
            "case {destination}: no role was configured, got {found}"
        );
    }

    // A vendor this operation delivers to nothing in needs no role either: the refusal
    // reports the resource it was handed, not a role requirement for a delivery that
    // could never happen.
    for destination in ["arn:aws:s3:::my-bucket".to_string(), arn("sns", "log-topic")] {
        let (code, message) = logs
            .refusal("PutSubscriptionFilter", &put(&destination, None))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {destination}: an unsupported destination is refused, got {code} {message}"
        );
        assert!(
            !message.contains("roleArn"),
            "case {destination}: an unsupported vendor is refused for its resource, got {message}"
        );
    }
}

/// `DeleteSubscriptionFilter` removes exactly the filter it names from the group it is
/// associated with: the others stay, the name is free for a new filter, and a filter the
/// group does not hold — or a group that does not exist — is refused.
#[tokio::test]
async fn delete_removes_only_the_subscription_filter_it_names() {
    let logs = Logs::new();
    let group = "delete-group";
    let other_group = "delete-group-other";
    create_group(&logs, group).await;
    create_group(&logs, other_group).await;
    let destination = arn("lambda", "function:log-consumer");
    let put = |group: &str, name: &str| {
        json!({
            "logGroupName": group,
            "filterName": name,
            "filterPattern": "",
            "destinationArn": destination,
        })
    };

    // Both filters are within the per-group quota, so the group holds both, and another
    // group holds a filter under one of the same names.
    let names = ["keep", "remove"];
    for name in names {
        logs.ok("PutSubscriptionFilter", &put(group, name)).await;
    }
    logs.ok("PutSubscriptionFilter", &put(other_group, names[1]))
        .await;

    logs.ok(
        "DeleteSubscriptionFilter",
        &json!({"logGroupName": group, "filterName": names[1]}),
    )
    .await;
    assert_eq!(
        names_of(&filters_of(&logs, group).await),
        vec![names[0].to_string()],
        "case after the delete: only the filter the delete named is gone"
    );
    assert_eq!(
        names_of(&filters_of(&logs, other_group).await),
        vec![names[1].to_string()],
        "case another group: a filter of the same name on another group is untouched"
    );

    // The name is free again: a new filter can take it, and the group is back at the
    // quota.
    logs.ok("PutSubscriptionFilter", &put(group, names[1])).await;
    let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(
        names_of(&filters_of(&logs, group).await),
        expected,
        "case re-put after the delete: the name is free again"
    );

    // What a delete refuses: a filter the group does not hold, and a group that does not
    // exist.
    for (case, request) in [
        (
            "a filter the group does not hold",
            json!({"logGroupName": group, "filterName": "never-created"}),
        ),
        (
            "a log group that does not exist",
            json!({"logGroupName": "no-such-group", "filterName": names[0]}),
        ),
    ] {
        assert_eq!(
            logs.err("DeleteSubscriptionFilter", &request).await,
            "ResourceNotFoundException",
            "case {case}: there is nothing to delete"
        );
    }
    assert_eq!(
        names_of(&filters_of(&logs, group).await),
        expected,
        "case after the refused deletes: nothing else was removed"
    );
}
