// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMonitoredResources`.
//!
//! The emulator is driven over the wire through the same [`Emulator`] harness the
//! configuration-lifecycle suite uses, so every assertion is about what a client sees.
//!
//! RULE pinned here: `ListMonitoredResources` reports the resources this account has
//! under DevOps Guru analysis as one `MonitoredResourceIdentifier` per persisted
//! coverage entry — one per covered CloudFormation stack name, then one per
//! `(AppBoundaryKey, TagValue)` pair, the same population `DescribeAccountHealth`
//! counts as `AnalyzedResourceCount` — each carrying that one entry as its
//! `ResourceCollection`, the type of the covered thing as its `Type` and
//! `FULL_PERMISSION` as its `ResourcePermission`, selected by the request's `Filters`
//! and paged at its `MaxResults`. The read changes nothing a later caller sees.

use serde_json::{json, Value};

use super::configuration_lifecycle::{cloud_formation, tags, Emulator, Reply};

/// `MonitoredResourceIdentifier.Type` of a covered CloudFormation stack: the same value
/// `GetCostEstimation` reports for that coverage entry, and a well-formed `ResourceType`
/// (`^[a-zA-Z]+[a-zA-Z0-9-_:]*$`, from
/// `smithy_query(query="shape",shape="ResourceType")`).
const STACK_TYPE: &str = "AWS::CloudFormation::Stack";

/// `MonitoredResourceIdentifier.Type` of one covered `(AppBoundaryKey, TagValue)` pair.
const TAG_TYPE: &str = "AWS::Tag";

/// `ResourcePermission`'s "DevOps Guru may access this resource" value, from
/// `smithy_query(query="shape",shape="ResourcePermission")`; its other value is
/// `MISSING_PERMISSION`.
const FULL_PERMISSION: &str = "FULL_PERMISSION";
const MISSING_PERMISSION: &str = "MISSING_PERMISSION";

/// `POST /monitoredResources` — `ListMonitoredResources`'s http binding, from
/// `smithy_query(query="operation",operation="ListMonitoredResources")`.
async fn list_monitored_resources(
    emulator: &Emulator,
    filters: Option<&Value>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Reply {
    let mut body = json!({});
    if let Some(filters) = filters {
        body["Filters"] = filters.clone();
    }
    if let Some(requested) = max_results {
        body["MaxResults"] = json!(requested);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    emulator
        .call("POST", "/monitoredResources", Some(&body))
        .await
}

/// A `Filters` naming a permission status and the resource types to report.
fn filters(permission: &str, resource_types: &[&str]) -> Value {
    json!({ "ResourcePermission": permission, "ResourceTypeFilters": resource_types })
}

/// An `UpdateResourceCollection` filter carrying the submitted stacks and tag entries,
/// built from the harness's own two halves.
fn collection(stacks: &[&str], tag_entries: &[(&str, &[&str])]) -> Value {
    let mut filter = json!({});
    if !stacks.is_empty() {
        filter["CloudFormation"] = cloud_formation(stacks)["CloudFormation"].clone();
    }
    if !tag_entries.is_empty() {
        filter["Tags"] = tags(tag_entries)["Tags"].clone();
    }
    filter
}

/// One coverage step: an `Action`, the stack names and the tag entries it carries.
type Step<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a [&'a str])]);

/// Apply `steps` through `UpdateResourceCollection`, failing the case if one is refused.
async fn cover(emulator: &Emulator, case: &str, steps: &[Step<'_>]) {
    for (action, stacks, tag_entries) in steps {
        let reply = emulator
            .update_resource_collection(action, collection(stacks, tag_entries))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the {action} setup step must succeed, got {}",
            reply.raw
        );
    }
}

/// One reported `MonitoredResourceIdentifier`, as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    name: Option<String>,
    resource_type: Option<String>,
    permission: Option<String>,
    /// `ResourceCollection.CloudFormation.StackNames`, empty when absent.
    stacks: Vec<String>,
    /// `ResourceCollection.Tags`, as `(AppBoundaryKey, TagValues)`, empty when absent.
    tags: Vec<(String, Vec<String>)>,
    /// Whether the identifier carries a `LastUpdated` at all.
    has_last_updated: bool,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `MonitoredResourceIdentifiers` a reply reported.
fn reported(reply: &Reply) -> Vec<Reported> {
    reply
        .at("/MonitoredResourceIdentifiers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Reported {
                    name: item
                        .get("MonitoredResourceName")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    resource_type: item
                        .get("Type")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    permission: item
                        .get("ResourcePermission")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    stacks: strings(item.pointer("/ResourceCollection/CloudFormation/StackNames")),
                    tags: item
                        .pointer("/ResourceCollection/Tags")
                        .and_then(Value::as_array)
                        .map(|entries| {
                            entries
                                .iter()
                                .map(|entry| {
                                    (
                                        entry
                                            .get("AppBoundaryKey")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                        strings(entry.get("TagValues")),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    has_last_updated: item.get("LastUpdated").is_some(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One coverage entry, and the `MonitoredResourceName` it is expected to be reported
/// under — `None` where the entry's own value is not a well-formed
/// `MonitoredResourceName` (length 1..512 matching `^[\.\-_\/#A-Za-z0-9]+$`, from
/// `smithy_query(query="shape",shape="MonitoredResourceName")`).
#[derive(Debug, Clone, Copy)]
enum Expect<'a> {
    Stack(&'a str, Option<&'a str>),
    Tag(&'a str, &'a str, Option<&'a str>),
}

impl Expect<'_> {
    fn reported(&self) -> Reported {
        match *self {
            Self::Stack(stack_name, name) => Reported {
                name: name.map(ToString::to_string),
                resource_type: Some(STACK_TYPE.to_string()),
                permission: Some(FULL_PERMISSION.to_string()),
                stacks: vec![stack_name.to_string()],
                tags: Vec::new(),
                has_last_updated: false,
            },
            Self::Tag(app_boundary_key, tag_value, name) => Reported {
                name: name.map(ToString::to_string),
                resource_type: Some(TAG_TYPE.to_string()),
                permission: Some(FULL_PERMISSION.to_string()),
                stacks: Vec::new(),
                tags: vec![(
                    app_boundary_key.to_string(),
                    vec![tag_value.to_string()],
                )],
                has_last_updated: false,
            },
        }
    }
}

/// The `AnalyzedResourceCount` `DescribeAccountHealth` reports over the same store.
async fn analyzed_resource_count(emulator: &Emulator) -> Option<i64> {
    let reply = emulator.call("GET", "/accounts/health", None).await;
    assert_eq!(
        reply.status, 200,
        "DescribeAccountHealth must succeed, got {}",
        reply.raw
    );
    reply.at("/AnalyzedResourceCount").and_then(Value::as_i64)
}

/// Walk every page of one selection, returning the resources reported across the walk.
async fn walk(
    emulator: &Emulator,
    case: &str,
    filter: Option<&Value>,
    max_results: Option<i64>,
) -> Vec<Reported> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let reply = list_monitored_resources(emulator, filter, max_results, token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: every page of the walk must succeed, got {}",
            reply.raw
        );
        let page = reported(&reply);
        if let Some(size) = max_results {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must hold at most {size} resources, got {} in {}",
                page.len(),
                reply.raw
            );
        }
        walked.extend(page);
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return walked;
        }
    }
}

// =============================================================================
// 1. The reported population
// =============================================================================

/// RULE: the resources `ListMonitoredResources` reports are the persisted coverage
/// entries — one per covered stack name, then one per `(AppBoundaryKey, TagValue)` pair,
/// in the store's stable order — each carrying that one entry as its
/// `ResourceCollection`, the covered thing's type as its `Type` and `FULL_PERMISSION` as
/// its `ResourcePermission`, with no `LastUpdated` because the store holds no instant for
/// a coverage entry. The count agrees with `DescribeAccountHealth`.
///
/// The rows differ along which half of the coverage is declared, the count, repetition
/// inside one request and across requests, removal, and the FORM of the covered value —
/// including values that are not well-formed `MonitoredResourceName`s, which are reported
/// by their `ResourceCollection` alone.
#[tokio::test]
async fn monitored_resources_are_the_persisted_coverage() {
    let cases: &[(&str, &[Step<'_>], &[Expect<'_>])] = &[
        ("no coverage declared at all", &[], &[]),
        (
            "one covered stack",
            &[("ADD", &["my-stack"], &[])],
            &[Expect::Stack("my-stack", Some("my-stack"))],
        ),
        (
            "three stacks, one repeated inside the request",
            &[("ADD", &["gamma", "alpha", "beta", "beta"], &[])],
            &[
                Expect::Stack("alpha", Some("alpha")),
                Expect::Stack("beta", Some("beta")),
                Expect::Stack("gamma", Some("gamma")),
            ],
        ),
        (
            "one app boundary key with two tag values",
            &[(
                "ADD",
                &[],
                &[("devops-guru-service-name", &["prod", "staging"])],
            )],
            &[
                Expect::Tag("devops-guru-service-name", "prod", Some("prod")),
                Expect::Tag("devops-guru-service-name", "staging", Some("staging")),
            ],
        ),
        (
            "stacks and tags covered together, stacks first",
            &[("ADD", &["s2", "s1"], &[("boundary", &["v"])])],
            &[
                Expect::Stack("s1", Some("s1")),
                Expect::Stack("s2", Some("s2")),
                Expect::Tag("boundary", "v", Some("v")),
            ],
        ),
        (
            "a removed stack stops being monitored",
            &[
                ("ADD", &["keep", "drop"], &[]),
                ("REMOVE", &["drop"], &[]),
            ],
            &[Expect::Stack("keep", Some("keep"))],
        ),
        (
            "the same tag value added by two requests is one resource",
            &[
                ("ADD", &[], &[("boundary", &["only"])]),
                ("ADD", &[], &[("boundary", &["only"])]),
            ],
            &[Expect::Tag("boundary", "only", Some("only"))],
        ),
        (
            // `TagValue` admits `\p{Z}`, `=`, `:`, `+` and `@`, none of which
            // `MonitoredResourceName` admits, so such an entry is reported with its
            // `ResourceCollection` and no name.
            "tag values that are not well-formed MonitoredResourceNames",
            &[(
                "ADD",
                &[],
                &[("boundary", &["two words", "team=payments", "ops@example"])],
            )],
            &[
                Expect::Tag("boundary", "ops@example", None),
                Expect::Tag("boundary", "team=payments", None),
                Expect::Tag("boundary", "two words", None),
            ],
        ),
        (
            // The `*` wildcard alternative of `TagValue`, which
            // `MonitoredResourceName` does not admit either.
            "the wildcard tag value",
            &[("ADD", &[], &[("boundary", &["*", "named"])])],
            &[
                Expect::Tag("boundary", "*", None),
                Expect::Tag("boundary", "named", Some("named")),
            ],
        ),
        (
            // `MonitoredResourceName` admits `.`, `-`, `_`, `/` and `#`; `TagValue`
            // admits every one of those but `#`, so a covered value can carry the
            // other four.
            "a covered value spelled with the punctuation both patterns admit",
            &[("ADD", &[], &[("boundary", &["a.b-c_d/e"])])],
            &[Expect::Tag("boundary", "a.b-c_d/e", Some("a.b-c_d/e"))],
        ),
    ];

    for (case, steps, expects) in cases {
        let emulator = Emulator::new();
        cover(&emulator, case, steps).await;

        let reply = list_monitored_resources(&emulator, None, None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListMonitoredResources must succeed, got {}",
            reply.raw
        );
        let expected: Vec<Reported> = expects.iter().map(Expect::reported).collect();
        let actual = reported(&reply);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the coverage entries {expected:?}, got {actual:?} in {}",
            reply.raw
        );
        assert!(
            reply.at("/NextToken").is_none(),
            "case {case:?}: a coverage this small is one page, so no NextToken, got {}",
            reply.raw
        );
        assert_eq!(
            analyzed_resource_count(&emulator).await,
            Some(actual.len() as i64),
            "case {case:?}: the reported resources must be as many as DescribeAccountHealth counts, got {actual:?}"
        );
    }
}

// =============================================================================
// 2. Filters
// =============================================================================

/// RULE: `Filters.ResourcePermission` reports only the resources whose permission status
/// it names — every entry of this store is accessible to DevOps Guru, so
/// `MISSING_PERMISSION` selects none — and `Filters.ResourceTypeFilters` reports only
/// those whose `Type` it names: that enum names the Amazon Web Services resource types
/// DevOps Guru's own discovery finds INSIDE an analysis boundary, which this emulator
/// never stores, so naming any type selects nothing, while naming none restricts nothing.
///
/// The rows differ in permission, in how many types are named, in WHICH types (values
/// the issue report did not cite included) and in whether the type list is empty.
#[tokio::test]
async fn filters_select_by_permission_and_resource_type() {
    let emulator = Emulator::new();
    cover(
        &emulator,
        "two stacks and one tag value",
        &[("ADD", &["checkout", "payments"], &[("boundary", &["prod"])])],
    )
    .await;
    let everything: Vec<Reported> = [
        Expect::Stack("checkout", Some("checkout")),
        Expect::Stack("payments", Some("payments")),
        Expect::Tag("boundary", "prod", Some("prod")),
    ]
    .iter()
    .map(Expect::reported)
    .collect();

    // `ResourceTypeFilter`'s values, from
    // `smithy_query(query="shape",shape="ResourceTypeFilter")`.
    let cases: &[(&str, Option<Value>, &[Reported])] = &[
        ("no Filters at all", None, &everything),
        (
            "FULL_PERMISSION, no type named",
            Some(filters(FULL_PERMISSION, &[])),
            &everything,
        ),
        (
            "MISSING_PERMISSION, no type named",
            Some(filters(MISSING_PERMISSION, &[])),
            &[],
        ),
        (
            "FULL_PERMISSION and LOG_GROUPS",
            Some(filters(FULL_PERMISSION, &["LOG_GROUPS"])),
            &[],
        ),
        (
            "FULL_PERMISSION and two types the report did not cite",
            Some(filters(FULL_PERMISSION, &["S3_BUCKET", "SQS_QUEUE"])),
            &[],
        ),
        (
            "FULL_PERMISSION and a type repeated",
            Some(filters(
                FULL_PERMISSION,
                &["RDS_DB_INSTANCE", "RDS_DB_INSTANCE"],
            )),
            &[],
        ),
        (
            "MISSING_PERMISSION and one type",
            Some(filters(MISSING_PERMISSION, &["LAMBDA_FUNCTION"])),
            &[],
        ),
        (
            "FULL_PERMISSION and every modelled type at once",
            Some(filters(
                FULL_PERMISSION,
                &[
                    "CLOUDFRONT_DISTRIBUTION",
                    "DYNAMODB_TABLE",
                    "EC2_NAT_GATEWAY",
                    "ECS_CLUSTER",
                    "ECS_SERVICE",
                    "EKS_CLUSTER",
                    "ELASTICACHE_CACHE_CLUSTER",
                    "ELASTICSEARCH_DOMAIN",
                    "ELASTIC_BEANSTALK_ENVIRONMENT",
                    "ELASTIC_LOAD_BALANCER_LOAD_BALANCER",
                    "ELASTIC_LOAD_BALANCING_V2_LOAD_BALANCER",
                    "ELASTIC_LOAD_BALANCING_V2_TARGET_GROUP",
                    "KINESIS_STREAM",
                    "LAMBDA_FUNCTION",
                    "LOG_GROUPS",
                    "OPEN_SEARCH_SERVICE_DOMAIN",
                    "RDS_DB_CLUSTER",
                    "RDS_DB_INSTANCE",
                    "REDSHIFT_CLUSTER",
                    "ROUTE53_HEALTH_CHECK",
                    "ROUTE53_HOSTED_ZONE",
                    "S3_BUCKET",
                    "SAGEMAKER_ENDPOINT",
                    "SNS_TOPIC",
                    "SQS_QUEUE",
                    "STEP_FUNCTIONS_ACTIVITY",
                    "STEP_FUNCTIONS_STATE_MACHINE",
                ],
            )),
            &[],
        ),
    ];

    for (case, filter, expected) in cases {
        let reply = list_monitored_resources(&emulator, filter.as_ref(), None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a modelled Filters must be accepted, got {}",
            reply.raw
        );
        let actual = reported(&reply);
        assert_eq!(
            actual.as_slice(),
            *expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {}",
            reply.raw
        );
    }
}

/// RULE: a `Filters` member outside the values its enum models is a bad request — HTTP
/// 400 `ValidationException` — never an empty report.
///
/// The rows differ in which member carries the unmodelled value, and in its form: a
/// value from the other enum, a lowercase spelling of a modelled value, and a value no
/// enum holds.
#[tokio::test]
async fn an_unmodelled_filter_value_is_refused() {
    let emulator = Emulator::new();
    cover(&emulator, "one covered stack", &[("ADD", &["only"], &[])]).await;

    for (case, filter) in [
        (
            "a ResourcePermission no enum value spells",
            filters("PARTIAL_PERMISSION", &[]),
        ),
        (
            "a ResourcePermission in lowercase",
            filters("full_permission", &[]),
        ),
        (
            "a ResourceTypeFilter from the permission enum",
            filters(FULL_PERMISSION, &[FULL_PERMISSION]),
        ),
        (
            "one modelled type and one that is not",
            filters(FULL_PERMISSION, &["LOG_GROUPS", "FLUX_CAPACITOR"]),
        ),
    ] {
        let reply = list_monitored_resources(&emulator, Some(&filter), None, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unmodelled filter value must be refused, got {}",
            reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
    }
}

// =============================================================================
// 3. Pagination
// =============================================================================

/// RULE: a walk reports every selected resource exactly once, in pages of at most the
/// requested `MaxResults`, and the last page carries no `NextToken`; a `MaxResults`
/// outside the modelled 1..50 is refused with HTTP 400.
///
/// The rows differ in page size: one per page, a size that does not divide the total,
/// both published bounds of `ListMonitoredResourcesMaxResults`, no `MaxResults` at all
/// (the service's own page size) and a size that CHANGES between pages of one walk.
#[tokio::test]
async fn a_walk_reports_every_monitored_resource_exactly_once() {
    let emulator = Emulator::new();
    cover(
        &emulator,
        "seven coverage entries",
        &[(
            "ADD",
            &["alpha", "beta", "delta", "epsilon", "gamma"],
            &[("boundary", &["prod", "staging"])],
        )],
    )
    .await;
    let expected: Vec<Reported> = [
        Expect::Stack("alpha", Some("alpha")),
        Expect::Stack("beta", Some("beta")),
        Expect::Stack("delta", Some("delta")),
        Expect::Stack("epsilon", Some("epsilon")),
        Expect::Stack("gamma", Some("gamma")),
        Expect::Tag("boundary", "prod", Some("prod")),
        Expect::Tag("boundary", "staging", Some("staging")),
    ]
    .iter()
    .map(Expect::reported)
    .collect();

    for (case, max_results) in [
        ("one per page", Some(1)),
        ("three per page, which does not divide seven", Some(3)),
        ("the modelled minimum", Some(1)),
        ("the modelled maximum", Some(50)),
        ("no MaxResults", None),
    ] {
        let walked = walk(&emulator, case, None, max_results).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every coverage entry exactly once, got {walked:?}"
        );
    }

    // A walk whose page size changes between calls: each page holds at most what that
    // call asked for, and the walk still reports each resource exactly once.
    let sizes = [2_i64, 1, 50, 3];
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    let mut step = 0_usize;
    loop {
        let size = sizes[step % sizes.len()];
        let reply = list_monitored_resources(&emulator, None, Some(size), token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "case \"changing page size\" step {step}: the call must succeed, got {}",
            reply.raw
        );
        let page = reported(&reply);
        assert!(
            page.len() as i64 <= size,
            "case \"changing page size\" step {step}: a page must hold at most {size} resources, got {} in {}",
            page.len(),
            reply.raw
        );
        walked.extend(page);
        step += 1;
        match reply.at("/NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, expected,
        "case \"changing page size\": expected every coverage entry exactly once, got {walked:?}"
    );

    // One past each published bound of `ListMonitoredResourcesMaxResults` (range 1..50).
    for (case, requested) in [("below the minimum", 0), ("above the maximum", 51)] {
        let reply = list_monitored_resources(&emulator, None, Some(requested), None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: MaxResults={requested} is outside the modelled range and must be refused, got {}",
            reply.raw
        );
    }
}

/// RULE: a `NextToken` names a page of ONE selection, so a token this service did not
/// issue for the selection being read — another selection's cursor, a rewritten token, or
/// a cursor into coverage that has since shrunk — is refused with HTTP 400
/// `ValidationException` instead of seeking into some other page.
#[tokio::test]
async fn a_token_that_names_no_page_of_this_selection_is_refused() {
    let emulator = Emulator::new();
    cover(
        &emulator,
        "four covered stacks",
        &[("ADD", &["one", "two", "three", "four"], &[])],
    )
    .await;

    let unfiltered = filters(FULL_PERMISSION, &[]);
    let first = list_monitored_resources(&emulator, None, Some(2), None).await;
    assert_eq!(
        first.status, 200,
        "the first page must succeed, got {}",
        first.raw
    );
    let token = first
        .at("/NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "four resources at two per page must leave a cursor, got {}",
                first.raw
            )
        })
        .to_string();

    for (case, filter, token) in [
        (
            // Selects the same resources, but is a different selection, so its cursors
            // are its own.
            "a permission-filtered selection",
            Some(unfiltered.clone()),
            token.clone(),
        ),
        (
            "a selection that names a type",
            Some(filters(FULL_PERMISSION, &["LOG_GROUPS"])),
            token.clone(),
        ),
        ("the token uppercased", None, token.to_uppercase()),
        (
            "a well-formed token of no collection",
            None,
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
    ] {
        let reply =
            list_monitored_resources(&emulator, filter.as_ref(), Some(2), Some(&token)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a token naming no page of this selection must be refused, got {}",
            reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
    }

    // The token stays good for the selection it was issued for, until that coverage no
    // longer holds the page it names.
    let resumed = list_monitored_resources(&emulator, None, Some(2), Some(&token)).await;
    assert_eq!(
        resumed.status, 200,
        "case \"its own selection\": the cursor must resume the walk, got {}",
        resumed.raw
    );
    cover(
        &emulator,
        "the coverage shrinks under the cursor",
        &[("REMOVE", &["one", "two", "three", "four"], &[])],
    )
    .await;
    let stale = list_monitored_resources(&emulator, None, Some(2), Some(&token)).await;
    assert_eq!(
        stale.status, 400,
        "case \"a cursor into emptied coverage\": the stale page must be refused, got {}",
        stale.raw
    );
    assert!(
        stale.names_error("ValidationException"),
        "case \"a cursor into emptied coverage\": the refusal must be a ValidationException, got {}",
        stale.raw
    );
}

// =============================================================================
// 4. The read writes nothing
// =============================================================================

/// RULE: `ListMonitoredResources` is a report, so repeating it answers the same thing and
/// leaves the coverage — and every other read over it — exactly as it was.
#[tokio::test]
async fn listing_monitored_resources_changes_nothing() {
    let emulator = Emulator::new();
    cover(
        &emulator,
        "stacks and tags",
        &[("ADD", &["one", "two"], &[("boundary", &["prod"])])],
    )
    .await;

    let before = analyzed_resource_count(&emulator).await;
    let first = list_monitored_resources(&emulator, None, None, None).await;
    let second = list_monitored_resources(&emulator, None, None, None).await;
    let filtered = list_monitored_resources(
        &emulator,
        Some(&filters(MISSING_PERMISSION, &["LOG_GROUPS"])),
        Some(1),
        None,
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first read must succeed, got {}",
        first.raw
    );
    assert_eq!(
        (second.status, &second.body),
        (first.status, &first.body),
        "case {:?}: two reads must answer the same, got {} then {}",
        "repeated ListMonitoredResources",
        first.raw,
        second.raw
    );
    assert_eq!(
        filtered.status, 200,
        "the filtered read must succeed, got {}",
        filtered.raw
    );

    let third = list_monitored_resources(&emulator, None, None, None).await;
    assert_eq!(
        (third.status, &third.body),
        (first.status, &first.body),
        "case {:?}: a filtered read must not change what an unfiltered one answers, got {} then {}",
        "ListMonitoredResources after a filtered read",
        first.raw,
        third.raw
    );
    assert_eq!(
        analyzed_resource_count(&emulator).await,
        before,
        "case {:?}: the analysed-resource count a later reader sees must be unchanged",
        "DescribeAccountHealth after three reads"
    );

    let coverage = emulator
        .get_resource_collection("AWS_CLOUD_FORMATION", None)
        .await;
    assert_eq!(
        coverage
            .at("/ResourceCollection/CloudFormation/StackNames")
            .cloned(),
        Some(json!(["one", "two"])),
        "case {:?}: the coverage a later reader sees must be unchanged, got {}",
        "GetResourceCollection after three reads",
        coverage.raw
    );
}
