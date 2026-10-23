// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's compute-allocation listing:
//! `ListComputeQuotas` and the `DescribeComputeQuota` reader beside it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on
//! the harness the inference-deployment suite beside this one defines, so what these
//! tests exercise is what a real SDK client gets.
//!
//! `CreateComputeQuota` is still a scaffold stub — it is owned by its own issue, which
//! also owns the `ClusterArn`-existence precondition real SageMaker applies before it
//! stores anything — so these tests seed the definitions through the SHARED engine
//! (`Emulator::store`, the very store `crate::state::service_state` hands a handler, as
//! `state_roundtrip.rs` pins) and then read them back over the wire. Nothing here
//! reaches into a handler: the responses under test are serialised from persisted state
//! by the real request walk.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! summary a listing reports for every stored member; every documented sort key and
//! sort order; every documented filter; every page size; the `Creating` -> `Created`
//! transition the clock owes; and the boundary refusal for a value outside each bound
//! the model publishes on the listing's own input.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    ComputeQuotaConfig, ComputeQuotaResourceConfig, ComputeQuotaTarget, ResourceSharingConfig, Tag,
};
use service_com_amazonaws_sagemaker_sagemaker::state::{self, ComputeQuota};

// One `epoch_seconds` for the crate's suites: the inference-deployment suite defines it
// beside the harness these cases boot on.
use super::inference_deployment_lifecycle::{
    epoch_seconds, error_code, str_field, Emulator, ACCOUNT,
};

/// The region these cases sign for, so every minted ARN names one region.
const REGION: &str = "us-east-1";

/// Long enough to pass `state::COMPUTE_QUOTA_CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// How far in the future a seeded `Creating` definition's transition is due when the
/// case needs it to STAY `Creating`: far enough that no reader can settle it mid-test.
const NOT_DUE_FOR_AGES: Duration = Duration::from_secs(3_600);

/// Spacing between seeded creation instants, so `CreationTime` ordering is unambiguous
/// and a filter bound can be placed strictly between two definitions.
const CREATE_SPACING: Duration = Duration::from_millis(5);

/// Two cluster ids of the form `ClusterArn`'s pattern fixes
/// (`…:cluster/[a-z0-9]{12}`, smithy_query shape=ClusterArn).
const CLUSTER_A: &str = "aaaa0000bbbb";
/// See [`CLUSTER_A`].
const CLUSTER_B: &str = "cccc1111dddd";

/// `SortQuotaBy`'s members — the sort keys `ListComputeQuotas` documents
/// (smithy_query shape=SortQuotaBy).
const SORT_KEYS: [&str; 4] = ["Name", "CreationTime", "Status", "ClusterArn"];

/// `SortOrder`'s members (smithy_query shape=SortOrder).
const SORT_ORDERS: [&str; 2] = ["Ascending", "Descending"];

/// A well-formed `ClusterArn` in [`REGION`] for cluster `id`.
fn cluster_arn(id: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:cluster/{id}")
}

/// Whether `value` matches `ComputeQuotaId`'s modelled pattern, `^[a-z0-9]{12}$`.
fn is_quota_id(value: &str) -> bool {
    value.len() == 12
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && c.is_ascii_alphabetic()))
}

/// One resource entry of a quota's configuration: `InstanceType` is required, every
/// amount optional (smithy_query shape=ComputeQuotaResourceConfig).
fn resource(instance_type: &str, count: i32) -> ComputeQuotaResourceConfig {
    ComputeQuotaResourceConfig {
        instance_type: instance_type.to_string(),
        count: Some(count),
        ..ComputeQuotaResourceConfig::default()
    }
}

/// A definition to seed: everything `CreateComputeQuota` will persist, with the members
/// these cases vary spelled out and the rest defaulted by [`seed`].
struct Seed {
    name: String,
    cluster: String,
    status: String,
    created: SystemTime,
    config: ComputeQuotaConfig,
    target: ComputeQuotaTarget,
    activation_state: String,
    description: Option<String>,
    tags: Vec<Tag>,
}

impl Seed {
    /// A definition named `name` in `cluster`, `Created` (a terminal status, so no
    /// reader can move it) at `created`, with a one-instance-type configuration.
    fn new(name: &str, cluster: &str, created: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            cluster: cluster_arn(cluster),
            status: state::STATUS_QUOTA_CREATED.to_string(),
            created,
            config: ComputeQuotaConfig {
                // `InstanceType` targets `ClusterInstanceType`, an enum the model
                // publishes; this is one of its members (smithy_query
                // shape=ClusterInstanceType).
                compute_quota_resources: Some(vec![resource("ml.g5.xlarge", 2)]),
                ..ComputeQuotaConfig::default()
            },
            target: ComputeQuotaTarget {
                team_name: "research".to_string(),
                fair_share_weight: None,
            },
            activation_state: state::DEFAULT_ACTIVATION_STATE.to_string(),
            description: None,
            tags: Vec::new(),
        }
    }

    fn with_status(mut self, status: &str) -> Self {
        self.status = status.to_string();
        self
    }
}

/// Write `seed` into the shared store under a freshly minted `ComputeQuotaId`, exactly
/// as `CreateComputeQuota` will, and answer that id.
///
/// `due_in` is how long from now the `Creating` -> `Created` transition is owed; it is
/// irrelevant to a definition seeded in a terminal status.
fn seed(emulator: &Emulator, seed: Seed, due_in: Duration) -> String {
    // The harness exposes the ONE shared engine; this is the very store the handlers
    // reach through `state::service_state`, so what is written here is what the next
    // request over the wire reads.
    let store = state::service_state(&*emulator.engine)
        .expect("the service is mounted on the shared in-memory engine");
    let mut guard = store.lock();
    let id = guard.mint_compute_quota_id();
    let arn = state::resource_arn(REGION, "compute-quota", &id);
    guard.compute_quotas.insert(
        id.clone(),
        ComputeQuota {
            id: id.clone(),
            arn,
            name: seed.name,
            description: seed.description,
            version: state::FIRST_COMPUTE_QUOTA_VERSION,
            status: seed.status,
            failure_reason: None,
            cluster_arn: seed.cluster,
            config: seed.config,
            target: seed.target,
            activation_state: seed.activation_state,
            tags: seed.tags,
            creation_time: seed.created,
            last_modified_time: seed.created,
            created_status_at: seed.created + due_in,
        },
    );
    id
}

/// Seed `names` in `cluster`, spaced so their creation instants differ, newest last.
fn seed_many(emulator: &Emulator, cluster: &str, names: &[&str]) -> Vec<String> {
    let base = SystemTime::now();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let created = base + CREATE_SPACING * u32::try_from(index).unwrap_or(0);
            seed(emulator, Seed::new(name, cluster, created), NOT_DUE_FOR_AGES)
        })
        .collect()
}

/// Every number in `value` as an `f64`, so a member the wire renders `16.0` compares
/// equal to the `16` written for it here.
fn numbers_as_floats(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(numbers_as_floats).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), numbers_as_floats(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The `ComputeQuotaSummaries` of a `ListComputeQuotas` response.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("ComputeQuotaSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ComputeQuotaSummaries in {body}"))
        .clone()
}

/// One summary member of every definition a listing returned, in page order.
fn listed(body: &Value, member: &str) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, member))
        .collect()
}

/// RULE: `ListComputeQuotas` reports, for every stored definition, the values that
/// definition holds — the same ones `DescribeComputeQuota` replays for it — with the
/// optional members present exactly where the stored definition carries them.
#[tokio::test]
async fn a_listing_reports_the_stored_definition_and_agrees_with_describe() {
    // Rows differ along the resource members used (instances, accelerators, vCPU,
    // memory, a MIG partition), the sharing strategy, preemption, the activation state,
    // the fair-share weight, the description and the tag count.
    let cases: Vec<(&str, Seed, Value)> = vec![
        (
            "one-instance-type",
            Seed::new("minimal", CLUSTER_A, SystemTime::now()),
            json!({ "ComputeQuotaResources": [{ "InstanceType": "ml.g5.xlarge", "Count": 2 }] }),
        ),
        (
            "every-resource-member",
            Seed {
                config: ComputeQuotaConfig {
                    compute_quota_resources: Some(vec![
                        resource("ml.c5.large", 0),
                        ComputeQuotaResourceConfig {
                            instance_type: "ml.p5.48xlarge".to_string(),
                            accelerators: Some(8),
                            v_cpu: Some(2.5),
                            memory_in_gi_b: Some(16.0),
                            ..ComputeQuotaResourceConfig::default()
                        },
                    ]),
                    preempt_team_tasks: Some("LowerPriority".to_string()),
                    ..ComputeQuotaConfig::default()
                },
                target: ComputeQuotaTarget {
                    team_name: "research".to_string(),
                    fair_share_weight: Some(100),
                },
                ..Seed::new("amounts", CLUSTER_A, SystemTime::now())
            },
            json!({
                "ComputeQuotaResources": [
                    { "InstanceType": "ml.c5.large", "Count": 0 },
                    {
                        "InstanceType": "ml.p5.48xlarge",
                        "Accelerators": 8,
                        "VCpu": 2.5,
                        "MemoryInGiB": 16.0,
                    },
                ],
                "PreemptTeamTasks": "LowerPriority",
            }),
        ),
        (
            "borrowing-strategy",
            Seed {
                config: ComputeQuotaConfig {
                    compute_quota_resources: Some(vec![resource("ml.g5.2xlarge", 4)]),
                    // `ResourceSharingStrategy`'s members (smithy_query
                    // shape=ResourceSharingStrategy); `BorrowLimit` is range 0..10000.
                    resource_sharing_config: Some(ResourceSharingConfig {
                        strategy: "LendAndBorrow".to_string(),
                        borrow_limit: Some(10_000),
                        absolute_borrow_limits: Some(vec![resource("ml.g5.xlarge", 1)]),
                    }),
                    preempt_team_tasks: Some("Never".to_string()),
                },
                ..Seed::new("borrowing", CLUSTER_B, SystemTime::now())
            },
            json!({
                "ComputeQuotaResources": [{ "InstanceType": "ml.g5.2xlarge", "Count": 4 }],
                "ResourceSharingConfig": {
                    "Strategy": "LendAndBorrow",
                    "BorrowLimit": 10000,
                    "AbsoluteBorrowLimits": [{ "InstanceType": "ml.g5.xlarge", "Count": 1 }],
                },
                "PreemptTeamTasks": "Never",
            }),
        ),
        (
            "disabled-with-description-and-tags",
            Seed {
                activation_state: "Disabled".to_string(),
                description: Some("Quota for the research team.".to_string()),
                tags: vec![
                    Tag {
                        key: "team".to_string(),
                        value: "research".to_string(),
                    },
                    Tag {
                        key: "env".to_string(),
                        value: "test".to_string(),
                    },
                ],
                ..Seed::new("annotated", CLUSTER_A, SystemTime::now())
            },
            json!({ "ComputeQuotaResources": [{ "InstanceType": "ml.g5.xlarge", "Count": 2 }] }),
        ),
        (
            "no-resource-amounts-at-all",
            Seed {
                config: ComputeQuotaConfig {
                    compute_quota_resources: Some(Vec::new()),
                    ..ComputeQuotaConfig::default()
                },
                ..Seed::new("empty", CLUSTER_B, SystemTime::now())
            },
            json!({ "ComputeQuotaResources": [] }),
        ),
    ];

    for (case, definition, expected_config) in cases {
        let emulator = Emulator::new();
        let name = definition.name.clone();
        let cluster = definition.cluster.clone();
        let activation = definition.activation_state.clone();
        let description = definition.description.clone();
        let created = definition.created;
        let target_team = definition.target.team_name.clone();
        let weight = definition.target.fair_share_weight;
        let id = seed(&emulator, definition, NOT_DUE_FOR_AGES);

        let listing = emulator.ok("ListComputeQuotas", &json!({})).await;
        let page = summaries(&listing);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: the stored definition is listed, got {listing:?}"
        );
        let summary = &page[0];
        assert!(
            is_quota_id(&str_field(summary, "ComputeQuotaId")),
            "case {case:?}: expected a ^[a-z0-9]{{12}}$ ComputeQuotaId, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ComputeQuotaId"),
            id,
            "case {case:?}: ComputeQuotaId, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ComputeQuotaArn"),
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:compute-quota/{id}"),
            "case {case:?}: ComputeQuotaArn, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "Name"),
            name,
            "case {case:?}: Name, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ClusterArn"),
            cluster,
            "case {case:?}: ClusterArn, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ActivationState"),
            activation,
            "case {case:?}: ActivationState, got {summary:?}"
        );
        assert_eq!(
            summary.get("ComputeQuotaVersion").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: ComputeQuotaVersion, got {summary:?}"
        );
        assert_eq!(
            summary.get("ComputeQuotaConfig").map(numbers_as_floats),
            Some(numbers_as_floats(&expected_config)),
            "case {case:?}: ComputeQuotaConfig, got {summary:?}"
        );
        let mut expected_target = json!({ "TeamName": target_team });
        if let Some(weight) = weight {
            expected_target["FairShareWeight"] = json!(weight);
        }
        assert_eq!(
            summary.get("ComputeQuotaTarget").map(numbers_as_floats),
            Some(numbers_as_floats(&expected_target)),
            "case {case:?}: ComputeQuotaTarget, got {summary:?}"
        );
        let reported = summary
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            (reported - epoch_seconds(created)).abs() < 1e-3,
            "case {case:?}: CreationTime must be the stored instant {}, got {reported}",
            epoch_seconds(created)
        );
        // The listing carries no `NextToken` when nothing is left behind.
        assert!(
            listing.get("NextToken").is_none(),
            "case {case:?}: a complete listing carries no NextToken, got {listing:?}"
        );

        // Every member the summary carries is the one the describe reports.
        let described = emulator
            .ok("DescribeComputeQuota", &json!({ "ComputeQuotaId": id }))
            .await;
        for (member, value) in summary.as_object().expect("a summary object") {
            assert_eq!(
                described.get(member).map(numbers_as_floats),
                Some(numbers_as_floats(value)),
                "case {case:?}: summary member {member} must match DescribeComputeQuota, \
                 got {described:?}"
            );
        }
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            description.as_deref(),
            "case {case:?}: Description, got {described:?}"
        );
        // Nothing failed, so there is no failure to report.
        assert!(
            described.get("FailureReason").is_none(),
            "case {case:?}: FailureReason must be absent, got {described:?}"
        );
        // A read does not change what a later caller sees.
        let again = emulator.ok("ListComputeQuotas", &json!({})).await;
        assert_eq!(
            summaries(&again),
            page,
            "case {case:?}: a second listing reports the same summaries, got {again:?}"
        );
    }
}

/// RULE: `ListComputeQuotas` orders the result by the requested `SortBy` in the
/// requested `SortOrder`, defaulting to `Descending`, and returns every stored
/// definition whichever ordering is asked for.
#[tokio::test]
async fn a_listing_honours_every_documented_sort_key_and_order() {
    let emulator = Emulator::new();
    // Names, clusters and statuses all differ across the seeded definitions, so each
    // sort key has something to order by.
    let base = SystemTime::now();
    let definitions = [
        ("mango", CLUSTER_A, state::STATUS_QUOTA_CREATED),
        ("apple", CLUSTER_B, state::STATUS_QUOTA_CREATING),
        ("pear", CLUSTER_A, state::STATUS_QUOTA_CREATED),
        ("cherry", CLUSTER_B, state::STATUS_QUOTA_CREATING),
        ("banana", CLUSTER_A, state::STATUS_QUOTA_CREATED),
    ];
    for (index, (name, cluster, status)) in definitions.iter().enumerate() {
        let created = base + CREATE_SPACING * u32::try_from(index).unwrap_or(0);
        seed(
            &emulator,
            Seed::new(name, cluster, created).with_status(status),
            NOT_DUE_FOR_AGES,
        );
    }

    for sort_by in SORT_KEYS {
        for sort_order in SORT_ORDERS {
            let case = format!("SortBy={sort_by} SortOrder={sort_order}");
            let listing = emulator
                .ok(
                    "ListComputeQuotas",
                    &json!({ "SortBy": sort_by, "SortOrder": sort_order }),
                )
                .await;
            let page = summaries(&listing);
            assert_eq!(
                page.len(),
                definitions.len(),
                "case {case}: every stored definition is listed, got {listing:?}"
            );

            // The sort key as a comparable value per row, in the order returned.
            let keys: Vec<String> = page
                .iter()
                .map(|summary| match sort_by {
                    "CreationTime" => format!(
                        "{:024.6}",
                        summary
                            .get("CreationTime")
                            .and_then(Value::as_f64)
                            .unwrap_or_default()
                    ),
                    other => str_field(summary, other),
                })
                .collect();
            let ordered = keys.windows(2).all(|pair| {
                if sort_order == "Ascending" {
                    pair[0] <= pair[1]
                } else {
                    pair[0] >= pair[1]
                }
            });
            assert!(
                ordered,
                "case {case}: the page must be ordered by the sort key, got {keys:?}"
            );
        }
    }

    // "By default, listed in Descending order according to by SortBy."
    let defaulted = emulator.ok("ListComputeQuotas", &json!({})).await;
    let explicit = emulator
        .ok(
            "ListComputeQuotas",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
        )
        .await;
    assert_eq!(
        listed(&defaulted, "ComputeQuotaId"),
        listed(&explicit, "ComputeQuotaId"),
        "case defaults: an unqualified listing is CreationTime/Descending, got {defaulted:?}"
    );
}

/// RULE: each of `ListComputeQuotas`' filters selects exactly the stored definitions it
/// matches — the name substring, the cluster, the status, and the creation bounds — and
/// the filters compose.
#[tokio::test]
async fn a_listing_honours_every_documented_filter() {
    let emulator = Emulator::new();
    seed_many(&emulator, CLUSTER_A, &["redapple", "redcherry", "greenpear"]);
    let base = SystemTime::now() + CREATE_SPACING * 3;
    seed(
        &emulator,
        Seed::new("redplum", CLUSTER_B, base).with_status(state::STATUS_QUOTA_CREATING),
        NOT_DUE_FOR_AGES,
    );

    // Rows differ along which filter is applied and how many definitions it selects.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no-filter",
            json!({}),
            vec!["redapple", "redcherry", "greenpear", "redplum"],
        ),
        (
            // `NameContains` targets `EntityName`, so the substring itself has to
            // satisfy that shape's pattern (smithy_query shape=EntityName).
            "name-contains-a-shared-prefix",
            json!({ "NameContains": "red" }),
            vec!["redapple", "redcherry", "redplum"],
        ),
        (
            "name-contains-an-infix",
            json!({ "NameContains": "err" }),
            vec!["redcherry"],
        ),
        (
            "name-contains-nothing-stored",
            json!({ "NameContains": "kiwi" }),
            vec![],
        ),
        (
            "cluster-with-three",
            json!({ "ClusterArn": cluster_arn(CLUSTER_A) }),
            vec!["redapple", "redcherry", "greenpear"],
        ),
        (
            "cluster-with-one",
            json!({ "ClusterArn": cluster_arn(CLUSTER_B) }),
            vec!["redplum"],
        ),
        (
            "cluster-holding-none",
            json!({ "ClusterArn": cluster_arn("eeee2222ffff") }),
            vec![],
        ),
        (
            "status-of-three",
            json!({ "Status": "Created" }),
            vec!["redapple", "redcherry", "greenpear"],
        ),
        (
            "status-of-one",
            json!({ "Status": "Creating" }),
            vec!["redplum"],
        ),
        (
            "status-nothing-has-reached",
            json!({ "Status": "Deleting" }),
            vec![],
        ),
        (
            "cluster-and-name-compose",
            json!({ "ClusterArn": cluster_arn(CLUSTER_A), "NameContains": "red" }),
            vec!["redapple", "redcherry"],
        ),
        (
            "status-and-name-compose",
            json!({ "Status": "Creating", "NameContains": "red" }),
            vec!["redplum"],
        ),
    ];

    for (case, filter, expected) in cases {
        let listing = emulator.ok("ListComputeQuotas", &filter).await;
        let mut got = listed(&listing, "Name");
        got.sort();
        let mut want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: filter {filter} must select exactly those names, got {listing:?}"
        );
    }

    // A `CreatedAfter` / `CreatedBefore` bound selects exactly the definitions on its
    // side of the instant it names. Each bound is placed 1 ms off a stored creation
    // instant — well inside the 5 ms the seeds are spaced by, and clear of the precision
    // a `Timestamp` loses as the epoch-seconds number the wire carries.
    let all = emulator
        .ok(
            "ListComputeQuotas",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let times: Vec<f64> = summaries(&all)
        .iter()
        .map(|summary| {
            summary
                .get("CreationTime")
                .and_then(Value::as_f64)
                .unwrap_or_default()
        })
        .collect();
    let epsilon = 0.001;
    for (index, at) in times.iter().enumerate() {
        let after = emulator
            .ok("ListComputeQuotas", &json!({ "CreatedAfter": at - epsilon }))
            .await;
        assert_eq!(
            summaries(&after).len(),
            times.len() - index,
            "case CreatedAfter=({at} - {epsilon}): the bound selects the definitions \
             created at or after it, got {after:?}"
        );
        let before = emulator
            .ok("ListComputeQuotas", &json!({ "CreatedBefore": at + epsilon }))
            .await;
        assert_eq!(
            summaries(&before).len(),
            index + 1,
            "case CreatedBefore=({at} + {epsilon}): the bound selects the definitions \
             created at or before it, got {before:?}"
        );
    }
    let window = emulator
        .ok(
            "ListComputeQuotas",
            &json!({
                "CreatedAfter": times[1] - epsilon,
                "CreatedBefore": times[2] + epsilon,
            }),
        )
        .await;
    assert_eq!(
        summaries(&window).len(),
        2,
        "case window: both bounds together select the range between them, got {window:?}"
    );
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most
/// `MaxResults` summaries, a `NextToken` appears only while something is left, and the
/// pages concatenate to the unpaged listing.
#[tokio::test]
async fn paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..7).map(|index| format!("quota{index:02}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    seed_many(&emulator, CLUSTER_A, &borrowed);

    let unpaged = listed(
        &emulator
            .ok(
                "ListComputeQuotas",
                &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await,
        "Name",
    );
    assert_eq!(
        unpaged.len(),
        names.len(),
        "case unpaged: every seeded definition is listed, got {unpaged:?}"
    );

    // Rows: every page size from one summary to more than the result holds.
    for page_size in [1_u64, 2, 3, 6, 7, 8, 100] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(next) = &token {
                request["NextToken"] = json!(next);
            }
            let page = emulator.ok("ListComputeQuotas", &request).await;
            let got = listed(&page, "Name");
            assert!(
                got.len() as u64 <= page_size,
                "case {case}: a page holds at most MaxResults, got {got:?}"
            );
            walked.extend(got);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging did not terminate, walked {walked:?}"
            );
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }

    // A token that names no boundary of the current result set is refused rather than
    // silently restarting the listing.
    for (case, token) in [
        ("not-hex", "zzzz"),
        ("hex-naming-no-definition", "6e6f7065"),
        ("empty", ""),
    ] {
        let (status, body) = emulator
            .call("ListComputeQuotas", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a stale NextToken is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: error code, got {body:?}"
        );
    }
}

/// RULE: the page is capped server-side, so no single `ListComputeQuotas` call
/// serialises the whole store however large a `MaxResults` the caller asks for.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let emulator = Emulator::new();
    // One past the server ceiling (`state::MAX_PAGE_SIZE`, 50). The ordering under test
    // is `Name`, so the seeds need no spacing to be unambiguous.
    let now = SystemTime::now();
    for index in 0..51 {
        seed(
            &emulator,
            Seed::new(&format!("bulk{index:03}"), CLUSTER_A, now),
            NOT_DUE_FOR_AGES,
        );
    }

    // Rows: the modelled `MaxResults` maximum, and an absent `MaxResults`.
    for (case, request) in [
        (
            "max-results-100",
            json!({ "SortBy": "Name", "SortOrder": "Ascending", "MaxResults": 100 }),
        ),
        (
            "no-max-results",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        ),
    ] {
        let page = emulator.ok("ListComputeQuotas", &request).await;
        let names = listed(&page, "Name");
        assert_eq!(
            names.len(),
            50,
            "case {case}: the page is capped at the server ceiling, got {} names",
            names.len()
        );
        let token = page.get("NextToken").and_then(Value::as_str);
        assert!(
            token.is_some(),
            "case {case}: a capped page that left something behind carries a NextToken, \
             got {page:?}"
        );
        let rest = emulator
            .ok(
                "ListComputeQuotas",
                &json!({
                    "SortBy": "Name",
                    "SortOrder": "Ascending",
                    "NextToken": token.unwrap_or_default(),
                }),
            )
            .await;
        assert_eq!(
            listed(&rest, "Name"),
            vec!["bulk050".to_string()],
            "case {case}: the next page holds the remainder, got {rest:?}"
        );
    }
}

/// RULE: a definition stored in the non-terminal `Creating` status is reported that way
/// until the transition the clock owes is due, and `Created` from then on — to a
/// listing and to a describe alike, without any further call having to cause it, and
/// with the `LastModifiedTime` the transition was DUE at rather than the one a reader
/// happened to run at.
#[tokio::test]
async fn a_definition_advances_from_creating_to_created_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = SystemTime::now();
    let id = seed(
        &emulator,
        Seed::new("settling", CLUSTER_A, created).with_status(state::STATUS_QUOTA_CREATING),
        state::COMPUTE_QUOTA_CREATED_AFTER,
    );

    // Before the due instant, both readers report the transient status.
    let described = emulator
        .ok("DescribeComputeQuota", &json!({ "ComputeQuotaId": id }))
        .await;
    assert_eq!(
        str_field(&described, "Status"),
        "Creating",
        "case immediately: DescribeComputeQuota, got {described:?}"
    );
    let listing = emulator.ok("ListComputeQuotas", &json!({})).await;
    assert_eq!(
        listed(&listing, "Status"),
        vec!["Creating".to_string()],
        "case immediately: ListComputeQuotas, got {listing:?}"
    );

    tokio::time::sleep(PAST_CREATED_DUE).await;

    // Rows: each reader, and each `Status` filter, once the transition is due.
    let settled = emulator
        .ok("DescribeComputeQuota", &json!({ "ComputeQuotaId": id }))
        .await;
    assert_eq!(
        str_field(&settled, "Status"),
        "Created",
        "case due: DescribeComputeQuota, got {settled:?}"
    );
    for (case, filter, expected) in [
        ("unfiltered", json!({}), vec!["Created".to_string()]),
        (
            "status=Created",
            json!({ "Status": "Created" }),
            vec!["Created".to_string()],
        ),
        ("status=Creating", json!({ "Status": "Creating" }), vec![]),
    ] {
        let page = emulator.ok("ListComputeQuotas", &filter).await;
        assert_eq!(
            listed(&page, "Status"),
            expected,
            "case {case}: ListComputeQuotas after the transition is due, got {page:?}"
        );
    }

    // `LastModifiedTime` moved to the instant the transition was due, so it is the same
    // on every later read.
    let due = epoch_seconds(created + state::COMPUTE_QUOTA_CREATED_AFTER);
    let reported = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        (reported - due).abs() < 1e-3,
        "case due: LastModifiedTime must be the instant the transition was due ({due}), \
         got {reported}"
    );
    let again = emulator
        .ok("DescribeComputeQuota", &json!({ "ComputeQuotaId": id }))
        .await;
    assert_eq!(
        again.get("LastModifiedTime"),
        settled.get("LastModifiedTime"),
        "case stability: LastModifiedTime must not move with the reader, got {again:?}"
    );
    assert_eq!(
        again.get("CreationTime"),
        described.get("CreationTime"),
        "case stability: CreationTime must not move at all, got {again:?}"
    );
}

/// RULE: `DescribeComputeQuota` naming an identifier or a version the store does not
/// hold is refused with the modelled not-found error, never a 200 with empty members.
#[tokio::test]
async fn an_unknown_identifier_or_version_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    let id = seed(
        &emulator,
        Seed::new("present", CLUSTER_A, SystemTime::now()),
        NOT_DUE_FOR_AGES,
    );

    // Rows: an id nothing minted, and a version the definition never reached (it is at
    // version 1, so both a lower and a higher version are absent).
    let cases: Vec<(&str, Value)> = vec![
        ("unknown-id", json!({ "ComputeQuotaId": "0123456789ab" })),
        (
            "known-id-future-version",
            json!({ "ComputeQuotaId": id.clone(), "ComputeQuotaVersion": 2 }),
        ),
        (
            "known-id-version-zero",
            json!({ "ComputeQuotaId": id.clone(), "ComputeQuotaVersion": 0 }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = emulator.call("DescribeComputeQuota", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case:?}: error code, got {body:?}"
        );
    }

    // The version the store holds is found, and so is the definition without one.
    for (case, request) in [
        ("no-version", json!({ "ComputeQuotaId": id.clone() })),
        (
            "current-version",
            json!({ "ComputeQuotaId": id.clone(), "ComputeQuotaVersion": 1 }),
        ),
    ] {
        let described = emulator.ok("DescribeComputeQuota", &request).await;
        assert_eq!(
            str_field(&described, "ComputeQuotaId"),
            id,
            "case {case}: the stored definition is found, got {described:?}"
        );
    }

    // A refused read wrote nothing: the one stored definition is still the only one.
    let listing = emulator.ok("ListComputeQuotas", &json!({})).await;
    assert_eq!(
        listed(&listing, "ComputeQuotaId"),
        vec![id],
        "case stored: a refused read changes no state, got {listing:?}"
    );
}

/// RULE: a listing whose input is outside a member's modelled constraint, or names a
/// value outside an enumerated member's value set, is refused with HTTP 400 rather than
/// served with a silently corrected input.
#[tokio::test]
async fn a_listing_outside_the_modelled_constraints_is_refused() {
    let emulator = Emulator::new();
    seed_many(&emulator, CLUSTER_A, &["alpha", "beta"]);

    // Each bound is the model's own (smithy_query operation=ListComputeQuotas):
    // `MaxResults` range 1..100, `NameContains` len 1..63 with `EntityName`'s pattern,
    // `SortBy` / `SortOrder` / `Status` enums, `ClusterArn`'s pattern.
    let refused: Vec<(&str, Value)> = vec![
        ("max-results-below-the-lower-bound", json!({ "MaxResults": 0 })),
        ("max-results-negative", json!({ "MaxResults": -1 })),
        ("max-results-past-the-upper-bound", json!({ "MaxResults": 101 })),
        ("name-contains-outside-its-pattern", json!({ "NameContains": "not a name" })),
        ("name-contains-empty", json!({ "NameContains": "" })),
        (
            "name-contains-past-its-length-bound",
            json!({ "NameContains": "a".repeat(64) }),
        ),
        ("sort-by-outside-the-enum", json!({ "SortBy": "Whatever" })),
        ("sort-order-outside-the-enum", json!({ "SortOrder": "Sideways" })),
        ("status-outside-the-enum", json!({ "Status": "Melting" })),
        (
            "cluster-arn-outside-its-pattern",
            json!({ "ClusterArn": "arn:aws:sagemaker:us-east-1:000000000000:cluster/NOT" }),
        ),
    ];
    for (case, request) in refused {
        let (status, body) = emulator.call("ListComputeQuotas", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the listing must refuse the request, got {status} {body}"
        );
    }

    // Rows ON each published bound are accepted.
    let accepted: Vec<(&str, Value, usize)> = vec![
        ("max-results-at-the-lower-bound", json!({ "MaxResults": 1 }), 1),
        ("max-results-at-the-upper-bound", json!({ "MaxResults": 100 }), 2),
        (
            "name-contains-at-its-length-lower-bound",
            json!({ "NameContains": "a" }),
            2,
        ),
    ];
    for (case, request, expected) in accepted {
        let listing = emulator.ok("ListComputeQuotas", &request).await;
        assert_eq!(
            summaries(&listing).len(),
            expected,
            "case {case:?}: a value ON the published bound is served, got {listing:?}"
        );
    }
}
