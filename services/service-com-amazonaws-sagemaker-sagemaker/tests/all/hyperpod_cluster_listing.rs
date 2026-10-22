// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker HyperPod clusters: `CreateCluster` mints them and
//! `ListClusters` reports them back, filtered, sorted and paged.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! `ClusterArn` form for every name shape and region, the create/list round trip for the
//! members a summary carries, the clock-owed `Creating` -> `InService` transition, and
//! `ListClusters` for every documented filter, sort key, sort order and page size.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, message, str_field, Emulator, ACCOUNT,
};

/// Longer than `state::CLUSTER_IN_SERVICE_AFTER` (300 ms), so the clock owes the
/// `Creating` -> `InService` transition by the time the next call runs.
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// Spacing between creates, so `CreationTime` ordering is unambiguous and a filter
/// bound can be placed strictly between two clusters.
const CREATE_SPACING: Duration = Duration::from_millis(5);

/// A training plan ARN in the modelled form: `TrainingPlanArn` is `len 50..2048`,
/// pattern `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:training-plan/`
/// (smithy_query operation=ListClusters).
fn training_plan_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:training-plan/{name}")
}

/// A minimal instance group. `InstanceCount`, `InstanceGroupName` and `ExecutionRole`
/// are the group's required members (smithy_query
/// shape=ClusterInstanceGroupSpecification).
fn instance_group(name: &str, instance_count: i64) -> Value {
    json!({
        "InstanceGroupName": name,
        "InstanceCount": instance_count,
        "InstanceType": "ml.c5.xlarge",
        "ExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/hyperpod-{name}"),
    })
}

/// `instance_group`, reserved against a training plan.
fn instance_group_on_plan(name: &str, plan: &str) -> Value {
    let mut group = instance_group(name, 1);
    group["TrainingPlanArn"] = Value::String(training_plan_arn(plan));
    group
}

/// A `CreateCluster` body naming `name`, with one instance group plus `extra`'s members.
fn cluster_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "ClusterName": name,
        "InstanceGroups": [instance_group("workers", 2)],
    });
    for (member, value) in extra.as_object().expect("extra members are an object") {
        body[member] = value.clone();
    }
    body
}

/// The summaries a `ListClusters` response carries.
fn summaries(listed: &Value) -> Vec<Value> {
    listed
        .get("ClusterSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ClusterSummaries in {listed}"))
        .clone()
}

/// The `ClusterName`s a `ListClusters` response carries, in the order it carried them.
fn listed_names(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| str_field(summary, "ClusterName"))
        .collect()
}

/// The id a `ClusterArn` carries, checked against the modelled form:
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$`
/// (smithy_query operation=CreateCluster).
fn cluster_arn_id(case: &str, region: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:cluster/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a ClusterArn starting {prefix:?}, got {arn:?}")
    });
    assert!(
        id.len() == 12
            && id
                .chars()
                .all(|glyph| glyph.is_ascii_lowercase() || glyph.is_ascii_digit()),
        "case {case:?}: a ClusterArn's tail must match [a-z0-9]{{12}}, got {id:?}"
    );
    id.to_string()
}

/// Create the four clusters the filter, sort and page cases read, spaced in time and
/// named so every filter has both a match and a non-match. Answers their
/// `CreationTime`s, keyed by name in creation order.
async fn seed_clusters(emulator: &Emulator) -> Vec<(String, f64)> {
    let rows: Vec<(&str, Value)> = vec![
        ("alpha-one", json!({})),
        (
            "beta-two",
            json!({ "InstanceGroups": [instance_group_on_plan("g", "plan-shared")] }),
        ),
        (
            "demo-three",
            json!({ "InstanceGroups": [instance_group_on_plan("g", "plan-shared")] }),
        ),
        (
            "demo-four",
            json!({ "InstanceGroups": [instance_group_on_plan("g", "plan-solo")] }),
        ),
    ];

    let mut created = Vec::new();
    for (name, extra) in rows {
        emulator.ok("CreateCluster", &cluster_body(name, &extra)).await;
        tokio::time::sleep(CREATE_SPACING).await;
        created.push(name.to_string());
    }

    // The creation times the service recorded, read back from the listing.
    let listed = emulator.ok("ListClusters", &json!({})).await;
    let mut times = Vec::new();
    for name in created {
        let time = summaries(&listed)
            .iter()
            .find(|summary| str_field(summary, "ClusterName") == name)
            .and_then(|summary| summary.get("CreationTime").and_then(Value::as_f64))
            .unwrap_or_else(|| panic!("seeded cluster {name} must be listed with a CreationTime"));
        times.push((name, time));
    }
    times
}

/// RULE: `CreateCluster` persists the cluster under its name and mints it one ARN of the
/// modelled form — `arn:aws:sagemaker:<signed region>:<account>:cluster/<12 chars of
/// [a-z0-9]>`, unique per cluster — and `ListClusters` reports every stored cluster once,
/// replaying the name and ARN it was created with whatever the creation spec looked like.
#[tokio::test]
async fn created_clusters_are_listed_with_the_identity_they_were_created_with() {
    // Rows differ along the axes the rule generalises over: the name's shape (one
    // character, digits, hyphens, and the 63-character modelled maximum) and the shape
    // of the creation spec (group count, orchestrator, VPC, tags, restricted groups).
    let rows: Vec<(&str, Value)> = vec![
        ("c", json!({})),
        ("cluster1", json!({ "NodeRecovery": "Automatic" })),
        (
            "demo-cluster-with-hyphens",
            json!({
                "InstanceGroups": [
                    instance_group("controller", 1),
                    instance_group("workers", 8),
                ],
                "Tags": [{ "Key": "team", "Value": "research" }],
            }),
        ),
        (
            // `ClusterName` is `len 1..63` (smithy_query operation=CreateCluster), so
            // this row sits on the upper bound.
            "a234567890123456789012345678901234567890123456789012345678901b",
            json!({
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-01234567"],
                    "Subnets": ["subnet-01234567"],
                },
                "Orchestrator": { "Eks": { "ClusterArn": format!("arn:aws:eks:us-east-1:{ACCOUNT}:cluster/hyperpod") } },
            }),
        ),
    ];

    // The region is an axis too: the ARN takes it from the caller's credential scope.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let mut minted: Vec<(String, String)> = Vec::new();

        for (name, extra) in &rows {
            let case = format!("region={region} name={name}");
            let created = emulator
                .call_in(region, "CreateCluster", &cluster_body(name, extra))
                .await;
            assert_eq!(
                created.0, 200,
                "case {case:?}: CreateCluster must answer 200, got {} with {:?}",
                created.0, created.1
            );
            let arn = str_field(&created.1, "ClusterArn");
            let id = cluster_arn_id(&case, region, &arn);
            assert!(
                minted.iter().all(|(_, seen)| *seen != id),
                "case {case:?}: every cluster gets its own id, but {id:?} repeated in {minted:?}"
            );
            minted.push(((*name).to_string(), id));

            let listed = emulator.call_in(region, "ListClusters", &json!({})).await;
            assert_eq!(
                listed.0, 200,
                "case {case:?}: ListClusters must answer 200, got {} with {:?}",
                listed.0, listed.1
            );
            let matches: Vec<Value> = summaries(&listed.1)
                .into_iter()
                .filter(|summary| str_field(summary, "ClusterName") == *name)
                .collect();
            assert_eq!(
                matches.len(),
                1,
                "case {case:?}: the created cluster must be listed exactly once, got {matches:?}"
            );
            assert_eq!(
                str_field(&matches[0], "ClusterArn"),
                arn,
                "case {case:?}: the listed ARN must be the minted one, got {:?}",
                matches[0]
            );
        }

        // Every cluster created on this emulator is still listed, exactly once each.
        let listed = emulator.call_in(region, "ListClusters", &json!({})).await;
        let mut names = listed_names(&listed.1);
        names.sort();
        let mut expected: Vec<String> = minted.iter().map(|(name, _)| name.clone()).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case region={region}: ListClusters must report every stored cluster once, got {:?}",
            listed.1
        );
    }
}

/// RULE: a summary's `TrainingPlanArns` are the training plans the cluster's instance
/// groups reserve capacity against — present when a group names one, omitted when none
/// does — and `TrainingPlanArn` selects exactly the clusters whose list contains it.
#[tokio::test]
async fn training_plan_arns_are_reported_and_filtered_on() {
    let emulator = Emulator::new();

    // Rows differ along the number of distinct plans the groups name, including the
    // repeated-plan row (one entry, not two) and the no-plan row (member omitted).
    let shared = training_plan_arn("plan-shared");
    let solo = training_plan_arn("plan-solo");
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no-plan", json!({}), vec![]),
        (
            "one-plan",
            json!({ "InstanceGroups": [instance_group_on_plan("g", "plan-shared")] }),
            vec![shared.clone()],
        ),
        (
            "two-plans",
            json!({
                "InstanceGroups": [
                    instance_group_on_plan("a", "plan-shared"),
                    instance_group_on_plan("b", "plan-solo"),
                ],
            }),
            vec![shared.clone(), solo.clone()],
        ),
        (
            "repeated-plan",
            json!({
                "InstanceGroups": [
                    instance_group_on_plan("a", "plan-solo"),
                    instance_group_on_plan("b", "plan-solo"),
                ],
            }),
            vec![solo.clone()],
        ),
    ];

    for (name, extra, _) in &rows {
        emulator
            .ok("CreateCluster", &cluster_body(name, extra))
            .await;
    }

    let listed = emulator.ok("ListClusters", &json!({})).await;
    for (name, _, expected) in &rows {
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| str_field(summary, "ClusterName") == *name)
            .unwrap_or_else(|| panic!("case {name:?}: the cluster must be listed"));
        let reported: Vec<String> = summary
            .get("TrainingPlanArns")
            .and_then(Value::as_array)
            .map(|arns| {
                arns.iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            &reported, expected,
            "case {name:?}: TrainingPlanArns, got {summary:?}"
        );
    }

    // Filtering by a plan selects every cluster associated with it and nothing else.
    let unused_plan = training_plan_arn("plan-nobody-uses");
    let filters: Vec<(&str, Vec<&str>)> = vec![
        (shared.as_str(), vec!["one-plan", "two-plans"]),
        (solo.as_str(), vec!["repeated-plan", "two-plans"]),
        (unused_plan.as_str(), vec![]),
    ];
    for (plan, expected) in filters {
        let listed = emulator
            .ok("ListClusters", &json!({ "TrainingPlanArn": plan }))
            .await;
        let mut names = listed_names(&listed);
        names.sort();
        assert_eq!(
            names, expected,
            "case TrainingPlanArn={plan:?}: filtered names, got {listed:?}"
        );
    }
}

/// RULE: a created cluster is reported `Creating` until the instant the clock owes it
/// `InService`, from when every later read reports `InService` — and the transition
/// happens because the clock passed that instant, not because anybody listed it.
#[tokio::test]
async fn a_cluster_settles_from_creating_to_in_service_when_the_clock_owes_it() {
    let emulator = Emulator::new();

    // Rows: a cluster listed straight away, and one listed only after the due instant,
    // so the transition cannot be an artefact of the first read.
    for (case, polls) in [("listed-immediately", 3_u32), ("listed-once-late", 0)] {
        let name = format!("settle-{case}");
        emulator
            .ok("CreateCluster", &cluster_body(&name, &json!({})))
            .await;

        for poll in 0..polls {
            let listed = emulator
                .ok("ListClusters", &json!({ "NameContains": name }))
                .await;
            let status = str_field(&summaries(&listed)[0], "ClusterStatus");
            // The first read happens immediately after the create, so the transient
            // status must still be observable; a later poll in the same window may
            // already have crossed the due instant on a loaded host, and either answer
            // is correct there as long as it is one of the two states of this span.
            let allowed: &[&str] = if poll == 0 {
                &["Creating"]
            } else {
                &["Creating", "InService"]
            };
            assert!(
                allowed.contains(&status.as_str()),
                "case {case:?} poll {poll}: expected one of {allowed:?} before the due instant, \
                 got {status:?} in {listed:?}"
            );
        }

        tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

        for read in 0..2 {
            let listed = emulator
                .ok("ListClusters", &json!({ "NameContains": name }))
                .await;
            assert_eq!(
                str_field(&summaries(&listed)[0], "ClusterStatus"),
                "InService",
                "case {case:?} read {read}: a cluster is InService once the clock owes it, \
                 got {listed:?}"
            );
        }
    }
}

/// RULE: `NameContains` keeps the clusters whose name contains the substring, and
/// `CreationTimeAfter` / `CreationTimeBefore` keep those created inside the window;
/// filters combine by intersection.
#[tokio::test]
async fn list_clusters_filters_by_name_and_by_the_creation_window() {
    let emulator = Emulator::new();
    let times = seed_clusters(&emulator).await;
    // Creation order is alpha-one, beta-two, demo-three, demo-four. A bound placed
    // strictly between two creations splits them unambiguously, whatever precision the
    // wire's epoch-seconds double has.
    let first = times[0].1;
    let last = times[3].1;
    let split_after_first = (times[0].1 + times[1].1) / 2.0;
    let split_before_last = (times[2].1 + times[3].1) / 2.0;
    let all = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows differ along the axis each filter generalises over: the substring (a prefix,
    // an infix, a whole name, a hyphen, one that matches nothing) and the window (open
    // at each end, closed on both, and one that excludes everything).
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no-filter", json!({}), all.clone()),
        ("prefix", json!({ "NameContains": "demo" }), vec!["demo-four", "demo-three"]),
        ("infix", json!({ "NameContains": "a-o" }), vec!["alpha-one"]),
        ("whole-name", json!({ "NameContains": "beta-two" }), vec!["beta-two"]),
        ("hyphen", json!({ "NameContains": "-" }), all.clone()),
        ("no-match", json!({ "NameContains": "gamma" }), vec![]),
        (
            "after-the-whole-set",
            json!({ "CreationTimeAfter": first - 1.0 }),
            all.clone(),
        ),
        (
            "after-the-first",
            json!({ "CreationTimeAfter": split_after_first }),
            vec!["beta-two", "demo-four", "demo-three"],
        ),
        (
            "before-the-first-split",
            json!({ "CreationTimeBefore": split_after_first }),
            vec!["alpha-one"],
        ),
        (
            "window-around-the-middle",
            json!({
                "CreationTimeAfter": split_after_first,
                "CreationTimeBefore": split_before_last,
            }),
            vec!["beta-two", "demo-three"],
        ),
        (
            "empty-window",
            json!({ "CreationTimeAfter": last + 60.0, "CreationTimeBefore": last + 120.0 }),
            vec![],
        ),
        (
            "name-and-window-intersect-to-nothing",
            json!({ "NameContains": "demo", "CreationTimeBefore": split_after_first }),
            vec![],
        ),
        (
            "name-and-window-intersect",
            json!({ "NameContains": "demo", "CreationTimeAfter": split_after_first }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, mut body, expected) in cases {
        body["SortBy"] = Value::String("NAME".to_string());
        let listed = emulator.ok("ListClusters", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case:?}: filtered names, got {listed:?}"
        );
    }
}

/// RULE: results are ordered by the requested `SortBy` in the requested `SortOrder`,
/// defaulting to `CREATION_TIME` ascending, with `ClusterName` as the tie-break; a value
/// outside either enum is refused.
#[tokio::test]
async fn list_clusters_sorts_by_every_documented_key_and_order() {
    let emulator = Emulator::new();
    seed_clusters(&emulator).await;

    let by_creation = vec!["alpha-one", "beta-two", "demo-three", "demo-four"];
    let by_creation_desc: Vec<&str> = by_creation.iter().copied().rev().collect();
    let by_name = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];
    let by_name_desc: Vec<&str> = by_name.iter().copied().rev().collect();

    // Rows: every (SortBy, SortOrder) pair the model allows, plus the defaults each
    // absent member documents ("The default value is CREATION_TIME", "The default value
    // is Ascending" — smithy_query operation=ListClusters).
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("defaults", json!({}), by_creation.clone()),
        ("creation-time-ascending", json!({ "SortBy": "CREATION_TIME", "SortOrder": "Ascending" }), by_creation.clone()),
        ("creation-time-descending", json!({ "SortBy": "CREATION_TIME", "SortOrder": "Descending" }), by_creation_desc),
        ("name-ascending", json!({ "SortBy": "NAME", "SortOrder": "Ascending" }), by_name.clone()),
        ("name-descending", json!({ "SortBy": "NAME", "SortOrder": "Descending" }), by_name_desc),
        ("sort-by-only", json!({ "SortBy": "NAME" }), by_name),
        ("sort-order-only", json!({ "SortOrder": "Ascending" }), by_creation),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListClusters", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case:?}: ListClusters order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // A value outside either enum is refused, not silently sorted some other way.
    for (case, body) in [
        ("unknown-sort-by", json!({ "SortBy": "CreationTime" })),
        ("unknown-sort-order", json!({ "SortOrder": "ascending" })),
    ] {
        let (status, parsed) = emulator.call("ListClusters", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: error code, got {parsed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_with_max_results_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    seed_clusters(&emulator).await;
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListClusters), so 1 and 100
    // are the published bounds.
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({
                "SortBy": "NAME",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListClusters", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page must hold at most MaxResults entries, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must walk the whole ordered result exactly once, got {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call("ListClusters", &json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: `ListClusters` is a reporting call: beyond materialising the status transition
/// the clock already owed, two identical listings of an unchanged store are identical,
/// and no listing consumes, hides or reorders a cluster.
#[tokio::test]
async fn listing_clusters_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_clusters(&emulator).await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    // Rows: the listings a client actually issues, each repeated.
    let bodies = [
        json!({}),
        json!({ "SortBy": "NAME", "SortOrder": "Descending" }),
        json!({ "MaxResults": 2 }),
        json!({ "NameContains": "demo" }),
    ];
    for body in &bodies {
        let first = emulator.ok("ListClusters", body).await;
        let second = emulator.ok("ListClusters", body).await;
        assert_eq!(
            first, second,
            "case {body}: a second identical listing must answer the same, got {first:?} then \
             {second:?}"
        );
    }
}

/// RULE: a `ClusterName` already in state is refused with the modelled `ResourceInUse`
/// and leaves the stored cluster exactly as it was — never overwritten, never duplicated.
#[tokio::test]
async fn a_cluster_name_already_in_use_is_refused_with_resource_in_use() {
    let emulator = Emulator::new();

    // Rows differ along what the second request changes besides the name, so none of
    // them can be read as an update.
    let rows: Vec<(&str, Value)> = vec![
        ("same-spec", json!({})),
        (
            "different-groups",
            json!({ "InstanceGroups": [instance_group("other", 7)] }),
        ),
        ("different-recovery", json!({ "NodeRecovery": "None" })),
    ];

    for (case, extra) in rows {
        let name = format!("dup-{case}");
        let created = emulator
            .ok("CreateCluster", &cluster_body(&name, &json!({})))
            .await;
        let arn = str_field(&created, "ClusterArn");

        let (status, parsed) = emulator
            .call("CreateCluster", &cluster_body(&name, &extra))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name already in use must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case:?}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&arn),
            "case {case:?}: the refusal must name the existing cluster {arn:?}, got {parsed:?}"
        );

        let listed = emulator
            .ok("ListClusters", &json!({ "NameContains": name }))
            .await;
        let matching = summaries(&listed);
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: the refused create must not add a cluster, got {listed:?}"
        );
        assert_eq!(
            str_field(&matching[0], "ClusterArn"),
            arn,
            "case {case:?}: the stored cluster must keep its ARN, got {listed:?}"
        );
    }
}

/// RULE: a request outside what the model or the member docs allow is refused with HTTP
/// 400 — never a 200 with invented members, and never a 501.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound (`MaxResults` range 1..100, `ClusterName`
    // len 1..63, `NameContains` len 0..63 pattern ^[a-zA-Z0-9\-]+$ — smithy_query
    // operation=ListClusters/CreateCluster), plus the `Orchestrator` doc's "you must
    // provide exactly one orchestrator configuration: either Eks or Slurm".
    let too_long_name = "a".repeat(64);
    let too_long_contains = "a".repeat(64);
    let eks = json!({ "ClusterArn": format!("arn:aws:eks:us-east-1:{ACCOUNT}:cluster/x") });
    let cases: Vec<(&str, &str, Value)> = vec![
        ("max-results-below-1", "ListClusters", json!({ "MaxResults": 0 })),
        ("max-results-above-100", "ListClusters", json!({ "MaxResults": 101 })),
        (
            "name-contains-above-63",
            "ListClusters",
            json!({ "NameContains": too_long_contains }),
        ),
        (
            "cluster-name-above-63",
            "CreateCluster",
            json!({ "ClusterName": too_long_name, "InstanceGroups": [instance_group("g", 1)] }),
        ),
        (
            "orchestrator-with-both-configurations",
            "CreateCluster",
            json!({
                "ClusterName": "both-orchestrators",
                "InstanceGroups": [instance_group("g", 1)],
                "Orchestrator": { "Eks": eks, "Slurm": {} },
            }),
        ),
        (
            "orchestrator-with-no-configuration",
            "CreateCluster",
            json!({
                "ClusterName": "no-orchestrator",
                "InstanceGroups": [instance_group("g", 1)],
                "Orchestrator": {},
            }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {target}, got {status} with {parsed}"
        );
    }

    // Nothing above reached the store.
    let listed = emulator.ok("ListClusters", &json!({})).await;
    assert!(
        summaries(&listed).is_empty(),
        "a refused request must persist nothing, got {listed:?}"
    );
}
