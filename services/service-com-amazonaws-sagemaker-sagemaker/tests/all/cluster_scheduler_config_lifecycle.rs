// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's cluster-policy surface:
//! `CreateClusterSchedulerConfig` -> `DescribeClusterSchedulerConfig` ->
//! `ListClusterSchedulerConfigs`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The emulator harness is
//! the one `inference_deployment_lifecycle` already defines.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! create/describe round trip for every shape `SchedulerConfig` can take; the minted id
//! and ARN form for every signed region; the name-uniqueness scope the model documents
//! (unique within one cluster, reusable across clusters); the `Creating` -> `Created`
//! transition the clock owes; and `ListClusterSchedulerConfigs` for every documented
//! sort, filter and page size.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::service_state;

use super::inference_deployment_lifecycle::{
    error_code, message, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};

/// Long enough to pass `state::cluster_policy::CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// The `ClusterName`s of the two HyperPod clusters these tests hang their policies off.
/// A cluster policy belongs to a cluster, so each one is created over the wire with
/// `CreateCluster` and addressed by the `ClusterArn` that call mints.
const CLUSTER_ONE: &str = "policy-cluster-one";
/// See [`CLUSTER_ONE`].
const CLUSTER_TWO: &str = "policy-cluster-two";

/// A well-formed `ClusterArn` naming a cluster NOTHING created: `cluster_id` is 12
/// lowercase alphanumerics, as `ClusterArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$`) requires.
fn unheld_cluster_arn(region: &str, cluster_id: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:cluster/{cluster_id}")
}

/// One minimal instance group. `InstanceGroupName`, `InstanceCount` and `ExecutionRole`
/// are its required members (smithy_query shape=ClusterInstanceGroupSpecification) — the
/// same shape `hyperpod_cluster_listing` creates its clusters with.
fn instance_group(name: &str) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": 1,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// Create the HyperPod cluster `name` in `region` over the wire and answer the
/// `ClusterArn` that call minted.
///
/// `CreateClusterSchedulerConfig` resolves `ClusterArn` against the ONE cluster store
/// `CreateCluster` writes and `ListClusters` reports, so the fixture is created through
/// the API — as a client's would be — and the ARN every policy call below names is the
/// service's own rather than one a test invented.
async fn seed_cluster(emulator: &Emulator, region: &str, name: &str) -> String {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateCluster",
            &json!({ "ClusterName": name, "InstanceGroups": [instance_group("workers")] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the cluster fixture {name:?} in {region} must be created, got {status} with {created}"
    );
    str_field(&created, "ClusterArn")
}

/// A `CreateClusterSchedulerConfig` body with only the three required members.
fn minimal_create(name: &str, cluster: &str) -> Value {
    json!({ "Name": name, "ClusterArn": cluster, "SchedulerConfig": {} })
}

/// The summaries `ListClusterSchedulerConfigs` returned.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("ClusterSchedulerConfigSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ClusterSchedulerConfigSummaries list in {body}"))
        .clone()
}

/// The `Name` of every summary in a listing, in the order the listing returned them.
fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, "Name"))
        .collect()
}

/// The `ClusterSchedulerConfigId` of every summary in a listing, in order.
fn listed_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, "ClusterSchedulerConfigId"))
        .collect()
}

/// Whether `id` satisfies `ClusterSchedulerConfigId`'s modelled pattern
/// (`^[a-z0-9]{12}$`).
fn is_modelled_id(id: &str) -> bool {
    id.len() == 12
        && id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// Create `name` in `cluster` and answer its minted id.
async fn create(emulator: &Emulator, name: &str, cluster: &str) -> String {
    let created = emulator
        .ok(
            "CreateClusterSchedulerConfig",
            &minimal_create(name, cluster),
        )
        .await;
    str_field(&created, "ClusterSchedulerConfigId")
}

/// Describe the policy `id` names.
async fn describe(emulator: &Emulator, id: &str) -> Value {
    emulator
        .ok(
            "DescribeClusterSchedulerConfig",
            &json!({ "ClusterSchedulerConfigId": id }),
        )
        .await
}

/// RULE: `CreateClusterSchedulerConfig` persists the policy it was handed under a minted
/// id, and `DescribeClusterSchedulerConfig` replays every member of it unchanged.
#[tokio::test]
async fn describe_replays_exactly_what_create_stored() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;

    // Rows span the shapes `SchedulerConfig` can take: no priority class, one, several;
    // both documented members of `FairShare` and of `IdleResourceSharing`
    // (smithy_query shape="FairShare,IdleResourceSharing": Enabled | Disabled); both
    // published bounds of `PriorityWeight` (range 0..100) and a value between them; and
    // the optional `Description` and `Tags` present as well as absent.
    let cases: Vec<(&str, Value)> = vec![
        ("empty-scheduler-config", minimal_create("minimal", &cluster)),
        (
            "fair-share-enabled-no-classes",
            json!({
                "Name": "fair-share-on",
                "ClusterArn": cluster,
                "SchedulerConfig": { "PriorityClasses": [], "FairShare": "Enabled" },
            }),
        ),
        (
            "fair-share-disabled-idle-sharing-enabled",
            json!({
                "Name": "fair-share-off",
                "ClusterArn": cluster,
                "SchedulerConfig": { "FairShare": "Disabled", "IdleResourceSharing": "Enabled" },
            }),
        ),
        (
            "idle-sharing-disabled-one-class-at-the-lower-weight-bound",
            json!({
                "Name": "one-class",
                "ClusterArn": cluster,
                "SchedulerConfig": {
                    "PriorityClasses": [{ "Name": "batch", "Weight": 0 }],
                    "IdleResourceSharing": "Disabled",
                },
            }),
        ),
        (
            "three-classes-spanning-the-weight-range",
            json!({
                "Name": "three-classes",
                "ClusterArn": cluster,
                "SchedulerConfig": {
                    "PriorityClasses": [
                        { "Name": "critical", "Weight": 100 },
                        { "Name": "standard", "Weight": 42 },
                        { "Name": "best-effort", "Weight": 0 },
                    ],
                    "FairShare": "Enabled",
                    "IdleResourceSharing": "Enabled",
                },
            }),
        ),
        (
            "description-and-tags",
            json!({
                "Name": "fully-described",
                "ClusterArn": cluster,
                "SchedulerConfig": { "PriorityClasses": [{ "Name": "inference", "Weight": 7 }] },
                "Description": "Prioritises inference over batch training.",
                "Tags": [{ "Key": "team", "Value": "ml-platform" }],
            }),
        ),
    ];

    let mut minted = BTreeSet::new();
    for (case, body) in cases {
        let created = emulator.ok("CreateClusterSchedulerConfig", &body).await;
        let id = str_field(&created, "ClusterSchedulerConfigId");
        assert!(
            is_modelled_id(&id),
            "case {case}: the minted id must satisfy ^[a-z0-9]{{12}}$, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case}: every policy must get its own id, got the already-minted {id:?}"
        );
        assert_eq!(
            str_field(&created, "ClusterSchedulerConfigArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:cluster-scheduler-config/{id}"),
            "case {case}: the minted ARN names the kind, region, account and id, got {created:?}"
        );

        let described = describe(&emulator, &id).await;
        assert_eq!(
            str_field(&described, "ClusterSchedulerConfigId"),
            id,
            "case {case}: Describe must answer for the id it was asked about, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ClusterSchedulerConfigArn"),
            str_field(&created, "ClusterSchedulerConfigArn"),
            "case {case}: the described ARN must be the minted one, got {described:?}"
        );
        for member in ["Name", "ClusterArn"] {
            assert_eq!(
                str_field(&described, member),
                str_field(&body, member),
                "case {case}: {member} must be the stored one, got {described:?}"
            );
        }
        assert_eq!(
            described.get("SchedulerConfig"),
            body.get("SchedulerConfig"),
            "case {case}: SchedulerConfig must round trip unchanged, got {described:?}"
        );
        assert_eq!(
            described.get("Description"),
            body.get("Description"),
            "case {case}: Description must round trip unchanged, got {described:?}"
        );
        assert_eq!(
            described
                .get("ClusterSchedulerConfigVersion")
                .and_then(Value::as_i64),
            Some(1),
            "case {case}: a freshly created policy is at version 1, got {described:?}"
        );
        assert!(
            described
                .get("CreationTime")
                .and_then(Value::as_f64)
                .is_some_and(|created_at| created_at > 0.0),
            "case {case}: CreationTime must be the instant the policy was created, got {described:?}"
        );
        assert!(
            described.get("FailureReason").is_none(),
            "case {case}: a policy that has not failed carries no FailureReason, got {described:?}"
        );
    }
}

/// RULE: the ARN a create mints carries the region the caller signed for.
#[tokio::test]
async fn the_minted_arn_names_the_signed_region() {
    let emulator = Emulator::new();

    // Rows span the region-token lengths `ClusterSchedulerConfigArn`'s modelled pattern
    // admits (`[a-z0-9\-]{9,16}`): its lower bound and two longer tokens.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        // A `ClusterName` identifies one cluster for the account, so each row creates its
        // own cluster rather than reusing one name across regions.
        let cluster = seed_cluster(&emulator, region, &format!("{CLUSTER_ONE}-{region}")).await;
        let (status, created) = emulator
            .call_in(
                region,
                "CreateClusterSchedulerConfig",
                &minimal_create("regional", &cluster),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region}: the create must succeed, got {status} with {created}"
        );
        let id = str_field(&created, "ClusterSchedulerConfigId");
        assert_eq!(
            str_field(&created, "ClusterSchedulerConfigArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:cluster-scheduler-config/{id}"),
            "case {region}: the ARN must name the signed region, got {created:?}"
        );
    }
}

/// RULE: a policy name is unique within the cluster its `ClusterArn` names — a second
/// create of that name in that cluster is refused with the modelled `ConflictException`
/// and changes nothing, while the same name in another cluster is a new policy.
#[tokio::test]
async fn a_duplicate_name_is_a_conflict_only_inside_the_same_cluster() {
    let emulator = Emulator::new();
    let one = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;
    let two = seed_cluster(&emulator, "us-east-1", CLUSTER_TWO).await;

    // Rows differ in the shape of the name: one character, hyphenated, mixed case.
    for name in ["a", "shared-policy", "Policy9"] {
        let first = create(&emulator, name, &one).await;

        let (status, refused) = emulator
            .call("CreateClusterSchedulerConfig", &minimal_create(name, &one))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: a duplicate name in one cluster must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {name}: the refusal names the modelled conflict, got {refused:?}"
        );
        assert!(
            message(&refused).contains(name),
            "case {name}: the refusal must name the policy it collided with, got {refused:?}"
        );

        // The same name in another cluster is allowed, and is its own policy.
        let in_other_cluster = create(&emulator, name, &two).await;
        assert_ne!(
            in_other_cluster, first,
            "case {name}: a policy in another cluster must get its own id"
        );

        // The refused create wrote nothing: the first cluster still holds exactly one
        // policy by that name, the one it held before.
        let listed = emulator
            .ok(
                "ListClusterSchedulerConfigs",
                &json!({ "ClusterArn": one }),
            )
            .await;
        let by_that_name: Vec<String> = summaries(&listed)
            .iter()
            .filter(|summary| str_field(summary, "Name") == name)
            .map(|summary| str_field(summary, "ClusterSchedulerConfigId"))
            .collect();
        assert_eq!(
            by_that_name,
            vec![first],
            "case {name}: the refused create must leave the stored policy alone, got {listed:?}"
        );
    }
}

/// Create four policies — three in one cluster, one in another — spaced so
/// `CreationTime` ordering is unambiguous, and answer each name with its creation time,
/// beside the two clusters' `ClusterArn`s.
async fn seed_policies(emulator: &Emulator) -> (Vec<(String, f64)>, String, String) {
    let one = seed_cluster(emulator, "us-east-1", CLUSTER_ONE).await;
    let two = seed_cluster(emulator, "us-east-1", CLUSTER_TWO).await;
    let mut seeded = Vec::new();
    for (name, cluster) in [
        ("alpha-one", &one),
        ("beta-two", &one),
        ("demo-three", &one),
        ("demo-four", &two),
    ] {
        let id = create(emulator, name, cluster).await;
        let described = describe(emulator, &id).await;
        seeded.push((
            name.to_string(),
            described
                .get("CreationTime")
                .and_then(Value::as_f64)
                .expect("CreationTime"),
        ));
        tokio::time::sleep(CREATE_SPACING).await;
    }
    (seeded, one, two)
}

/// RULE: `ListClusterSchedulerConfigs` returns the stored policies filtered by
/// `NameContains`, `ClusterArn` and `Status`, ordered by `SortBy`/`SortOrder`
/// (`SortOrder` defaulting to `Descending`, as its model doc states), and each summary
/// replays what the matching `Describe` reports.
#[tokio::test]
async fn list_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    let (seeded, one, two) = seed_policies(&emulator).await;
    assert_eq!(seeded.len(), 4, "the seeded store holds four policies");
    // Past the create transition, so every policy is in one status and the orders below
    // are decided by the sort key alone.
    tokio::time::sleep(PAST_CREATED_DUE).await;

    // Rows differ along sort field, sort order and the filters' selectivity. `SortBy`'s
    // members come from `SortClusterSchedulerConfigBy` (smithy_query
    // shape="SortClusterSchedulerConfigBy": Name | CreationTime | Status).
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-default-order",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-contains-two-matches",
            json!({ "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "nothing-here" }),
            vec![],
        ),
        (
            "one-cluster",
            json!({ "ClusterArn": one, "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three"],
        ),
        (
            "the-other-cluster",
            json!({ "ClusterArn": two, "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four"],
        ),
        (
            "cluster-and-name-contains-together",
            json!({ "ClusterArn": one, "NameContains": "demo", "SortBy": "Name" }),
            vec!["demo-three"],
        ),
        (
            "status-created",
            json!({ "Status": "Created", "SortBy": "Status", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "status-nothing-is-in",
            json!({ "Status": "Deleted" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListClusterSchedulerConfigs", &body).await;
        let names = listed_names(&listed);
        // `SortBy: Status` leaves every policy tied, so that row is compared as a set:
        // the tie-break is the id, which no case can predict.
        let mut sorted = names.clone();
        sorted.sort();
        let compared = if case == "status-created" {
            sorted
        } else {
            names
        };
        assert_eq!(
            compared, expected,
            "case {case}: the listing order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // Every summary replays what Describe reports for the same policy.
    let listed = emulator
        .ok(
            "ListClusterSchedulerConfigs",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    for (index, (name, created_at)) in seeded.iter().enumerate() {
        let summary = &summaries(&listed)[index];
        let described = describe(&emulator, &str_field(summary, "ClusterSchedulerConfigId")).await;
        for member in [
            "Name",
            "ClusterSchedulerConfigArn",
            "ClusterSchedulerConfigId",
            "ClusterArn",
            "Status",
        ] {
            assert_eq!(
                str_field(summary, member),
                str_field(&described, member),
                "case {name}: {member} in the summary must be what Describe reports"
            );
        }
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: CreationTime in the summary must be the stored one"
        );
        assert_eq!(
            summary
                .get("ClusterSchedulerConfigVersion")
                .and_then(Value::as_i64),
            described
                .get("ClusterSchedulerConfigVersion")
                .and_then(Value::as_i64),
            "case {name}: the summary's version must be the described one"
        );
    }
}

/// RULE: a `CreatedBefore` / `CreatedAfter` bound selects exactly the policies on that
/// side of it, inclusively.
#[tokio::test]
async fn list_filters_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let (seeded, _one, _two) = seed_policies(&emulator).await;
    // A bound strictly between the second and third policy, so the split is unambiguous.
    let split = (seeded[1].1 + seeded[2].1) / 2.0;
    // And one between the third and the fourth, so a window can hold exactly one policy.
    // Both bounds are midpoints rather than a stored `CreationTime`: a wire timestamp is
    // a JSON number, which cannot carry a nanosecond-precision instant exactly.
    let later_split = (seeded[2].1 + seeded[3].1) / 2.0;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "before-the-split",
            json!({ "CreatedBefore": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "after-the-split",
            json!({ "CreatedAfter": split, "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "both-bounds-around-one-policy",
            json!({
                "CreatedAfter": split,
                "CreatedBefore": later_split,
                "SortBy": "CreationTime",
            }),
            vec!["demo-three"],
        ),
        (
            "an-upper-bound-before-everything",
            json!({ "CreatedBefore": split - 60.0 }),
            vec![],
        ),
        (
            "a-window-holding-nothing",
            json!({ "CreatedAfter": seeded[3].1 + 60.0 }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListClusterSchedulerConfigs", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: the time-filtered listing, got {listed:?}"
        );
    }
}

/// RULE: both creation-time bounds are INCLUSIVE, so a bound equal to a policy's
/// `CreationTime` still selects it.
#[tokio::test]
async fn the_creation_time_bounds_are_inclusive() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;
    let id = create(&emulator, "pinned", &cluster).await;

    // The wire carries a timestamp as a JSON number, which cannot represent a
    // nanosecond-precision instant exactly — so the stored `CreationTime` is pinned to a
    // whole second, which it can, and the bounds below are comparable without rounding.
    const PINNED_SECS: u64 = 1_700_000_000;
    const PINNED: f64 = 1_700_000_000.0;
    {
        let state = service_state(&*emulator.engine).expect("the shared engine holds the state");
        let mut store = state.lock();
        let config = store
            .cluster_policies
            .configs
            .get_mut(&id)
            .expect("the created policy");
        config.creation_time = SystemTime::UNIX_EPOCH + Duration::from_secs(PINNED_SECS);
    }

    // Rows: each bound exactly on the stored time, each bound one second to either side,
    // and a window closed on both sides at the stored time.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("lower-bound-on-the-stored-time", json!({ "CreatedAfter": PINNED }), true),
        ("upper-bound-on-the-stored-time", json!({ "CreatedBefore": PINNED }), true),
        (
            "a-window-closed-on-the-stored-time",
            json!({ "CreatedAfter": PINNED, "CreatedBefore": PINNED }),
            true,
        ),
        (
            "lower-bound-a-second-later",
            json!({ "CreatedAfter": PINNED + 1.0 }),
            false,
        ),
        (
            "upper-bound-a-second-earlier",
            json!({ "CreatedBefore": PINNED - 1.0 }),
            false,
        ),
    ];

    for (case, body, selected) in cases {
        let listed = emulator.ok("ListClusterSchedulerConfigs", &body).await;
        let expected = if selected {
            vec![id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: the bound-filtered listing, got {listed:?}"
        );
    }
}

/// RULE: `MaxResults` caps a page and `NextToken` resumes exactly after it, so a
/// paginating client walks the whole result set exactly once.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;
    for index in 0..7 {
        create(&emulator, &format!("policy{index}"), &cluster).await;
    }
    let whole = listed_ids(
        &emulator
            .ok(
                "ListClusterSchedulerConfigs",
                &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await,
    );
    assert_eq!(whole.len(), 7, "the seeded store holds seven policies");

    // Rows: a page of one, a page that does not divide the result set, one that does,
    // the exact size of the result set, and one past it. (The service's own page
    // ceiling, `state::MAX_PAGE_SIZE`, is shared with `ListModels` and pinned there.)
    for page_size in [1, 2, 3, 7, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "MaxResults": page_size,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            });
            if let Some(ref resume) = token {
                body["NextToken"] = json!(resume);
            }
            let listed = emulator.ok("ListClusterSchedulerConfigs", &body).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= page_size.min(7),
                "case {page_size}: a page must not exceed MaxResults, got {listed:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 8,
                "case {page_size}: paging must terminate, walked {walked:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {page_size}: paging must yield the whole result set, in order, exactly once"
        );
    }

    // A token this listing never minted is refused rather than silently restarting.
    let (status, refused) = emulator
        .call(
            "ListClusterSchedulerConfigs",
            &json!({ "NextToken": "not-a-token-this-service-minted" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a NextToken that names no page boundary must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "the refusal of an unknown NextToken, got {refused:?}"
    );
}

/// RULE: a policy is reported `Creating` until the instant the clock owes its
/// `Created` transition, and `Created` from then on — and a read never moves it again.
#[tokio::test]
async fn a_policy_advances_from_creating_to_created_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;
    let id = create(&emulator, "settling", &cluster).await;

    // A caller acting straight after the create still sees the transient status, in the
    // reader and in the listing alike.
    let fresh = describe(&emulator, &id).await;
    assert_eq!(
        str_field(&fresh, "Status"),
        "Creating",
        "a policy describes as Creating before its transition is due, got {fresh:?}"
    );
    let listed = emulator.ok("ListClusterSchedulerConfigs", &json!({})).await;
    assert_eq!(
        summaries(&listed)
            .iter()
            .map(|summary| str_field(summary, "Status"))
            .collect::<Vec<String>>(),
        vec!["Creating".to_string()],
        "the listing reports the same transient status, got {listed:?}"
    );

    tokio::time::sleep(PAST_CREATED_DUE).await;

    let settled = describe(&emulator, &id).await;
    assert_eq!(
        str_field(&settled, "Status"),
        "Created",
        "a policy describes as Created once the transition is due, got {settled:?}"
    );
    assert_eq!(
        settled.get("CreationTime"),
        fresh.get("CreationTime"),
        "the transition must not move CreationTime, got {settled:?}"
    );

    // Reading again changes nothing: the transition was due at one instant, so
    // `LastModifiedTime` is stable across later reads.
    let again = describe(&emulator, &id).await;
    assert_eq!(
        again.get("LastModifiedTime"),
        settled.get("LastModifiedTime"),
        "LastModifiedTime must be stable across reads, got {again:?}"
    );
    assert_eq!(
        str_field(&again, "Status"),
        "Created",
        "a settled policy stays Created, got {again:?}"
    );

    // With a second, fresh policy in the store, the `Status` filter selects on the
    // status each policy is in at this instant.
    let fresh_id = create(&emulator, "still-creating", &cluster).await;
    for (case, status, expected) in [
        ("creating", "Creating", vec![fresh_id.clone()]),
        ("created", "Created", vec![id.clone()]),
    ] {
        let listed = emulator
            .ok(
                "ListClusterSchedulerConfigs",
                &json!({ "Status": status }),
            )
            .await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case}: the Status filter selects the policies in that status, got {listed:?}"
        );
    }
}

/// RULE: an operation addressed at a policy, or a version of one, that the service does
/// not hold is refused with the modelled `ResourceNotFound` — never a 200 carrying
/// nothing.
#[tokio::test]
async fn an_unknown_policy_or_version_is_refused_with_the_modelled_error() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;
    let id = create(&emulator, "the-only-one", &cluster).await;

    // Rows: an id of the modelled shape that was never minted, and versions on both
    // sides of the one version this policy has.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an-id-never-minted",
            json!({ "ClusterSchedulerConfigId": "ffffffffffff" }),
            ),
        (
            "a-version-ahead-of-the-current-one",
            json!({ "ClusterSchedulerConfigId": id, "ClusterSchedulerConfigVersion": 2 }),
        ),
        (
            "version-zero",
            json!({ "ClusterSchedulerConfigId": id, "ClusterSchedulerConfigVersion": 0 }),
        ),
        (
            "a-negative-version",
            json!({ "ClusterSchedulerConfigId": id, "ClusterSchedulerConfigVersion": -1 }),
        ),
    ];

    for (case, body) in cases {
        let (status, refused) = emulator
            .call("DescribeClusterSchedulerConfig", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the read must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: the refusal names the modelled error, got {refused:?}"
        );
    }

    // The version this policy IS at is readable.
    let described = emulator
        .ok(
            "DescribeClusterSchedulerConfig",
            &json!({ "ClusterSchedulerConfigId": id, "ClusterSchedulerConfigVersion": 1 }),
        )
        .await;
    assert_eq!(
        str_field(&described, "ClusterSchedulerConfigId"),
        id,
        "asking for the policy's own version must answer it, got {described:?}"
    );
}

/// RULE: a cluster policy names the cluster it belongs to, so a create whose
/// `ClusterArn` names a cluster the service does not hold is refused with HTTP 400 and
/// writes nothing.
#[tokio::test]
async fn a_policy_for_a_cluster_the_service_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let held = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;

    // Rows: a well-formed ARN for a cluster id nobody created, another such id in
    // another region, and one under another partition — each names a different cluster
    // from the one the store holds. Every id is 12 lowercase alphanumerics, as
    // `ClusterArn`'s modelled pattern requires.
    let cases: Vec<(&str, String)> = vec![
        (
            "a-cluster-id-never-created",
            unheld_cluster_arn("us-east-1", "aaaa11112222"),
        ),
        (
            "a-never-created-cluster-id-in-another-region",
            unheld_cluster_arn("eu-central-1", "bbbb33334444"),
        ),
        (
            "a-never-created-cluster-id-in-another-partition",
            format!("arn:aws-cn:sagemaker:us-east-1:{ACCOUNT}:cluster/cccc55556666"),
        ),
    ];

    for (case, cluster) in cases {
        let (status, refused) = emulator
            .call(
                "CreateClusterSchedulerConfig",
                &minimal_create("orphan", &cluster),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a policy for an absent cluster must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: the refusal of an absent cluster, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            // The wording the real service answers for this condition; it carries
            // nothing from the request, so every unresolvable cluster reads the same.
            "Specified cluster not found",
            "case {case}: the refusal's message, got {refused:?}"
        );
    }

    // Nothing above was stored, and the cluster the store DOES hold still takes a policy.
    let listed = emulator.ok("ListClusterSchedulerConfigs", &json!({})).await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "a refused create must write nothing, got {listed:?}"
    );
    let id = create(&emulator, "orphan", &held).await;
    assert!(
        is_modelled_id(&id),
        "the held cluster must still take a policy, got {id:?}"
    );
}

/// RULE: a request outside what the model or the operation accepts is refused with HTTP
/// 400, and writes nothing.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, "us-east-1", CLUSTER_ONE).await;

    // Rows: one past each published bound these operations state — `Name` len 1..63 and
    // its pattern, `ClusterArn`'s pattern, `PriorityClasses` len 0..10, `PriorityWeight`
    // range 0..100, `MaxResults` range 1..100 — plus a `SortBy` the enum does not carry.
    let too_long = "a".repeat(64);
    let eleven_classes: Vec<Value> = (0..11)
        .map(|index| json!({ "Name": format!("class-{index}"), "Weight": 1 }))
        .collect();
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name-past-63",
            "CreateClusterSchedulerConfig",
            minimal_create(&too_long, &cluster),
        ),
        (
            "empty-name",
            "CreateClusterSchedulerConfig",
            minimal_create("", &cluster),
        ),
        (
            "name-with-an-illegal-character",
            "CreateClusterSchedulerConfig",
            minimal_create("not a policy name", &cluster),
        ),
        (
            "cluster-arn-that-is-not-an-arn",
            "CreateClusterSchedulerConfig",
            minimal_create("fine-name", "not-an-arn-at-all-0123456789"),
        ),
        (
            "eleven-priority-classes",
            "CreateClusterSchedulerConfig",
            json!({
                "Name": "too-many-classes",
                "ClusterArn": cluster,
                "SchedulerConfig": { "PriorityClasses": eleven_classes },
            }),
        ),
        (
            "priority-weight-above-100",
            "CreateClusterSchedulerConfig",
            json!({
                "Name": "weight-too-high",
                "ClusterArn": cluster,
                "SchedulerConfig": { "PriorityClasses": [{ "Name": "x", "Weight": 101 }] },
            }),
        ),
        (
            "priority-weight-below-0",
            "CreateClusterSchedulerConfig",
            json!({
                "Name": "weight-too-low",
                "ClusterArn": cluster,
                "SchedulerConfig": { "PriorityClasses": [{ "Name": "x", "Weight": -1 }] },
            }),
        ),
        (
            "max-results-below-1",
            "ListClusterSchedulerConfigs",
            json!({ "MaxResults": 0 }),
        ),
        (
            "max-results-above-100",
            "ListClusterSchedulerConfigs",
            json!({ "MaxResults": 101 }),
        ),
        (
            "sort-by-outside-the-enum",
            "ListClusterSchedulerConfigs",
            json!({ "SortBy": "Whatever" }),
        ),
        (
            "sort-order-outside-the-enum",
            "ListClusterSchedulerConfigs",
            json!({ "SortOrder": "Sideways" }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, refused) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {refused}"
        );
    }

    // Nothing above reached state: the store is still empty.
    let listed = emulator.ok("ListClusterSchedulerConfigs", &json!({})).await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "a refused request must write nothing, got {listed:?}"
    );
}
