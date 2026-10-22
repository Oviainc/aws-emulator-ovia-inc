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

/// AWS's documented quota, from `PutSubscriptionFilter`: "Each log group can have up to
/// two subscription filters associated with it."
const MAX_FILTERS_PER_GROUP: usize = 2;

/// The published bounds of `DescribeSubscriptionFilters$limit`. Source: the
/// `DescribeLimit` shape, `range 1..50`.
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// The system fields a subscription filter may emit. Source:
/// `PutSubscriptionFilter$emitSystemFields` — "Valid values are @aws.account,
/// @aws.region, and @source.log".
const SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

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

/// An ARN in another service's namespace, of the form that service publishes.
fn arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{ACCOUNT}:{resource}")
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
            row.distribution,
            "case {row:?}: distribution is reported only when it was specified, got {found}"
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
            row.distribution,
            "case {row:?}: an update drops distribution when the put omits it, got {filter}"
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
    let cases: [[&str; MAX_FILTERS_PER_GROUP]; 3] =
        [["b", "a"], ["alpha-2", "alpha-1"], ["Z", "a"]];

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

/// The per-group quota: two filters associated with one log group, a third refused, and
/// a put over a name already stored still accepted because it is an update. The quota is
/// per log group, so another group takes its own two.
#[tokio::test]
async fn quota_is_two_filters_per_log_group() {
    let logs = Logs::new();
    let destination = arn("kinesis", "stream/quota-target");

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
