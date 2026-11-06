// SPDX-License-Identifier: CC0-1.0

//! Snapshots and usage limits, end to end over the wire, plus the remediation message a
//! blocked `DeleteNamespace` owes.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than the one request
//! the issue quoted:
//!
//! 1. A snapshot of a namespace is taken through the compute attached to it, so
//!    `CreateSnapshot` against a namespace carrying no workgroup is refused with the
//!    operation's 404 `ResourceNotFoundException` and stores nothing, and the same request
//!    succeeds once a workgroup on that namespace exists — after which the snapshot is
//!    what `GetSnapshot` / `ListSnapshots` report.
//! 2. A usage limit bounds one resource, so `CreateUsageLimit` accepts a `resourceArn`
//!    only when it resolves to a resource this account holds in this Region; one that
//!    resolves to nothing is refused with the 400 `ValidationException` and creates
//!    nothing.
//! 3. A dependency-ordering refusal states the remediation: deleting a namespace a
//!    workgroup is attached to answers `ConflictException` whose message names THAT
//!    workgroup as the thing to delete first.
//!
//! The harness (emulator, signed call, error-code reader, create helpers) is shared with
//! `namespace_workgroup_lifecycle`, which owns it.

use serde_json::{json, Value};

use crate::namespace_workgroup_lifecycle::{
    at, call, call_in, create_namespace, create_workgroup, emulator, error_code, is_uuid, ACCOUNT,
};

/// A workgroup ARN for `region`/`account` naming `workgroup_id`, the shape
/// `CreateWorkgroup` mints. Used to build ARNs that are structurally perfect and name
/// nothing.
fn workgroup_arn(region: &str, account: &str, workgroup_id: &str) -> String {
    format!("arn:aws:redshift-serverless:{region}:{account}:workgroup/{workgroup_id}")
}

/// A namespace ARN, as [`workgroup_arn`].
fn namespace_arn(region: &str, account: &str, namespace_id: &str) -> String {
    format!("arn:aws:redshift-serverless:{region}:{account}:namespace/{namespace_id}")
}

/// A UUID that names no resource: distinct from anything the emulator minted.
const ABSENT_ID: &str = "3f8c1a7e-5b2d-4c6e-9a10-7d4e2f6b8c90";

// ---------------------------------------------------------------------------
// Rule 1 — a snapshot is taken through the namespace's compute.
// ---------------------------------------------------------------------------

/// One row of the snapshot table: a namespace in `region`, the retention the create asks
/// for, and the retention the snapshot must report.
#[derive(Debug, Clone, Copy)]
struct SnapshotCase {
    region: &'static str,
    namespace: &'static str,
    workgroup: &'static str,
    snapshot: &'static str,
    /// `retentionPeriod` on the request, absent for a row that names none.
    retention: Option<i32>,
    /// The `snapshotRetentionPeriod` / `snapshotRemainingDays` the answer must carry.
    expected_retention: i32,
}

#[tokio::test]
async fn a_snapshot_needs_a_workgroup_on_its_namespace_and_is_then_served_from_state() {
    // The rows differ along the inputs the rule generalises over: the Region the ARNs are
    // minted in, and the retention period — absent, the documented lower bound of 1 day,
    // the documented upper bound of 3653 days, and the "retain indefinitely" sentinel -1.
    let cases = [
        SnapshotCase {
            region: "us-east-1",
            namespace: "snap-ns-default",
            workgroup: "snap-wg-default",
            snapshot: "snap-default",
            retention: None,
            expected_retention: -1,
        },
        SnapshotCase {
            region: "us-east-1",
            namespace: "snap-ns-oneday",
            workgroup: "snap-wg-oneday",
            snapshot: "snap-oneday",
            retention: Some(1),
            expected_retention: 1,
        },
        SnapshotCase {
            region: "eu-west-2",
            namespace: "snap-ns-tenyears",
            workgroup: "snap-wg-tenyears",
            snapshot: "snap-tenyears",
            retention: Some(3653),
            expected_retention: 3653,
        },
        SnapshotCase {
            region: "ap-southeast-2",
            namespace: "snap-ns-forever",
            workgroup: "snap-wg-forever",
            snapshot: "snap-forever",
            retention: Some(-1),
            expected_retention: -1,
        },
    ];

    let router = emulator();
    for case in cases {
        let (status, body) = call_in(
            &router,
            case.region,
            "CreateNamespace",
            json!({ "namespaceName": case.namespace, "adminUsername": "admin" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNamespace expected 200, got {status} with {body}"
        );
        let namespace = body["namespace"].clone();

        // The request every row makes twice: once with no compute, once with it.
        let mut request = json!({
            "namespaceName": case.namespace,
            "snapshotName": case.snapshot,
        });
        if let Some(retention) = case.retention {
            request["retentionPeriod"] = json!(retention);
        }

        // A namespace with no workgroup has nothing to take the snapshot through.
        let (status, body) = call_in(&router, case.region, "CreateSnapshot", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: CreateSnapshot without a workgroup got {status} with {body}"
        );

        // And the refusal created nothing.
        let (status, body) = call_in(
            &router,
            case.region,
            "ListSnapshots",
            json!({ "namespaceName": case.namespace }),
        )
        .await;
        assert_eq!(
            (status, &body["snapshots"]),
            (200, &json!([])),
            "case {case:?}: ListSnapshots after the refusal got {status} with {body}"
        );

        // Attaching compute makes the identical request succeed.
        let (status, body) = call_in(
            &router,
            case.region,
            "CreateWorkgroup",
            json!({
                "workgroupName": case.workgroup,
                "namespaceName": case.namespace,
                "baseCapacity": 8,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWorkgroup expected 200, got {status} with {body}"
        );

        let (status, body) = call_in(&router, case.region, "CreateSnapshot", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSnapshot with a workgroup expected 200, got {status} \
             with {body}"
        );
        let snapshot = body["snapshot"].clone();
        assert_eq!(
            snapshot["namespaceName"],
            json!(case.namespace),
            "case {case:?}: the snapshot is bound to its namespace, got {:?}",
            snapshot["namespaceName"]
        );
        assert_eq!(
            snapshot["namespaceArn"], namespace["namespaceArn"],
            "case {case:?}: namespaceArn is the namespace's own, got {:?}",
            snapshot["namespaceArn"]
        );
        assert_eq!(
            snapshot["adminUsername"],
            json!("admin"),
            "case {case:?}: adminUsername comes off the namespace, got {:?}",
            snapshot["adminUsername"]
        );
        assert_eq!(
            snapshot["ownerAccount"],
            json!(ACCOUNT),
            "case {case:?}: ownerAccount is this account, got {:?}",
            snapshot["ownerAccount"]
        );
        assert_eq!(
            snapshot["status"],
            json!("CREATING"),
            "case {case:?}: a caller acting straight after the create sees the transient \
             status, got {:?}",
            snapshot["status"]
        );
        assert_eq!(
            snapshot["snapshotRetentionPeriod"],
            json!(case.expected_retention),
            "case {case:?}: got {:?}",
            snapshot["snapshotRetentionPeriod"]
        );
        assert_eq!(
            snapshot["snapshotRemainingDays"],
            json!(case.expected_retention),
            "case {case:?}: the countdown starts full, got {:?}",
            snapshot["snapshotRemainingDays"]
        );
        assert!(
            snapshot["snapshotCreateTime"].is_number(),
            "case {case:?}: snapshotCreateTime must be set, got {:?}",
            snapshot["snapshotCreateTime"]
        );
        let snapshot_arn = snapshot["snapshotArn"].as_str().unwrap_or_default().to_string();
        let minted_id = snapshot_arn.rsplit('/').next().unwrap_or_default();
        assert!(
            is_uuid(minted_id),
            "case {case:?}: snapshotArn ends in a minted UUID, got {snapshot_arn:?}"
        );
        assert_eq!(
            snapshot_arn,
            format!(
                "arn:aws:redshift-serverless:{}:{ACCOUNT}:snapshot/{minted_id}",
                case.region
            ),
            "case {case:?}: snapshotArn carries the request's Region and this account"
        );

        // The read path reports exactly the stored record — by name and by ARN.
        for key in [
            json!({ "snapshotName": case.snapshot }),
            json!({ "snapshotArn": snapshot_arn }),
        ] {
            let (status, body) = call_in(&router, case.region, "GetSnapshot", key.clone()).await;
            assert_eq!(
                (status, &body["snapshot"]),
                (200, &snapshot),
                "case {case:?}: GetSnapshot({key}) got {status} with {body}"
            );
        }
        let (status, body) = call_in(
            &router,
            case.region,
            "ListSnapshots",
            json!({ "namespaceName": case.namespace }),
        )
        .await;
        assert_eq!(
            (status, &body["snapshots"]),
            (200, &json!([snapshot])),
            "case {case:?}: ListSnapshots scoped to the namespace got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_reaches_available_on_its_own_after_being_created() {
    let router = emulator();
    create_namespace(&router, "settle-snap-ns").await;
    create_workgroup(&router, "settle-snap-wg", "settle-snap-ns").await;

    let (_, body) = call(
        &router,
        "CreateSnapshot",
        json!({ "namespaceName": "settle-snap-ns", "snapshotName": "settle-snap" }),
    )
    .await;
    assert_eq!(
        body["snapshot"]["status"],
        json!("CREATING"),
        "CreateSnapshot: got {:?}",
        body["snapshot"]["status"]
    );

    // Nobody calls anything for the span; the clock alone settles it.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (_, body) = call(
        &router,
        "GetSnapshot",
        json!({ "snapshotName": "settle-snap" }),
    )
    .await;
    assert_eq!(
        body["snapshot"]["status"],
        json!("AVAILABLE"),
        "GetSnapshot once the transition fell due: got {:?}",
        body["snapshot"]["status"]
    );
}

#[tokio::test]
async fn the_snapshot_name_is_the_snapshot_identity_and_an_unknown_one_is_not_found() {
    let router = emulator();
    create_namespace(&router, "dup-snap-ns").await;
    create_workgroup(&router, "dup-snap-wg", "dup-snap-ns").await;
    let (status, _) = call(
        &router,
        "CreateSnapshot",
        json!({ "namespaceName": "dup-snap-ns", "snapshotName": "dup-snap" }),
    )
    .await;
    assert_eq!(status, 200, "the first CreateSnapshot must succeed");

    let (status, body) = call(
        &router,
        "CreateSnapshot",
        json!({ "namespaceName": "dup-snap-ns", "snapshotName": "dup-snap" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (409, "ConflictException"),
        "CreateSnapshot on an existing name: got {status} with {body}"
    );

    // Snapshotting a namespace that does not exist at all is a NAME refusal, which this
    // service answers with HTTP 400 carrying the same modelled shape — see
    // `state::namespace_not_found` and tests/all/namespace_response_fidelity.rs.
    let (status, body) = call(
        &router,
        "CreateSnapshot",
        json!({ "namespaceName": "no-such-ns", "snapshotName": "orphan-snap" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "CreateSnapshot into an unknown namespace: got {status} with {body}"
    );

    for key in [
        json!({ "snapshotName": "no-such-snap" }),
        json!({ "snapshotArn": format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:snapshot/{ABSENT_ID}") }),
    ] {
        let (status, body) = call(&router, "GetSnapshot", key.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "GetSnapshot({key}): got {status} with {body}"
        );
    }

    // Both identifying members are optional in the model, but a read has to address one.
    let (status, body) = call(&router, "GetSnapshot", json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "GetSnapshot naming neither member: got {status} with {body}"
    );
}

#[tokio::test]
async fn the_snapshot_retention_period_is_refused_outside_its_documented_range() {
    let router = emulator();
    create_namespace(&router, "retention-ns").await;
    create_workgroup(&router, "retention-wg", "retention-ns").await;

    // The documented rule: either -1 (retain indefinitely) or 1..=3653 days. Each bound
    // is tested, and one step past it.
    let accepted = [-1, 1, 2, 3652, 3653];
    let refused = [-3, -2, 0, 3654, i32::MAX];

    for (index, retention) in accepted.into_iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateSnapshot",
            json!({
                "namespaceName": "retention-ns",
                "snapshotName": format!("retention-ok-{index}"),
                "retentionPeriod": retention,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case retentionPeriod={retention}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["snapshot"]["snapshotRetentionPeriod"],
            json!(retention),
            "case retentionPeriod={retention}: got {:?}",
            body["snapshot"]["snapshotRetentionPeriod"]
        );
    }

    for (index, retention) in refused.into_iter().enumerate() {
        let name = format!("retention-bad-{index}");
        let (status, body) = call(
            &router,
            "CreateSnapshot",
            json!({
                "namespaceName": "retention-ns",
                "snapshotName": name,
                "retentionPeriod": retention,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case retentionPeriod={retention}: got {status} with {body}"
        );
        // A refused create stores nothing.
        let (status, body) = call(&router, "GetSnapshot", json!({ "snapshotName": name })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case retentionPeriod={retention}: the refusal must store nothing, got \
             {status} with {body}"
        );
    }
}

#[tokio::test]
async fn list_snapshots_pages_and_narrows_by_every_filter_it_models() {
    let router = emulator();
    for (namespace, workgroup) in [("list-ns-a", "list-wg-a"), ("list-ns-b", "list-wg-b")] {
        create_namespace(&router, namespace).await;
        create_workgroup(&router, workgroup, namespace).await;
    }
    // Five in the first namespace, one in the second: the page bound counts MATCHING
    // records, so a scoped listing pages over the five alone.
    for index in 0..5 {
        let (status, _) = call(
            &router,
            "CreateSnapshot",
            json!({
                "namespaceName": "list-ns-a",
                "snapshotName": format!("list-a-{index}"),
            }),
        )
        .await;
        assert_eq!(status, 200, "snapshot list-a-{index} must be created");
    }
    let (_, body) = call(
        &router,
        "CreateSnapshot",
        json!({ "namespaceName": "list-ns-b", "snapshotName": "list-b-0" }),
    )
    .await;
    let in_b = body["snapshot"].clone();
    let created_at = in_b["snapshotCreateTime"].as_f64().unwrap_or_default();

    // Page bounds: 2 -> 2 + 2 + 1 with a token on the first two pages only.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({ "namespaceName": "list-ns-a", "maxResults": 2 });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(&router, "ListSnapshots", request).await;
        assert_eq!(status, 200, "ListSnapshots page {pages}: got {body}");
        let page = body["snapshots"].as_array().cloned().unwrap_or_default();
        assert!(
            page.len() <= 2,
            "ListSnapshots page {pages}: maxResults=2 is a bound, got {} records",
            page.len()
        );
        for snapshot in &page {
            assert_eq!(
                snapshot["namespaceName"],
                json!("list-ns-a"),
                "ListSnapshots page {pages}: the filter admitted {:?}",
                snapshot["namespaceName"]
            );
            seen.push(
                snapshot["snapshotName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() || pages > 5 {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![
            "list-a-0".to_string(),
            "list-a-1".to_string(),
            "list-a-2".to_string(),
            "list-a-3".to_string(),
            "list-a-4".to_string(),
        ],
        "the pages walk every matching snapshot once, in name order, got {seen:?}"
    );
    assert_eq!(
        pages, 3,
        "five records at a bound of two is three pages, got {pages}"
    );

    // Each remaining filter, one row per member, asserted by the names it admits.
    let windows: [(Value, Vec<&str>); 6] = [
        (json!({ "namespaceArn": in_b["namespaceArn"] }), vec!["list-b-0"]),
        (json!({ "ownerAccount": ACCOUNT }), vec![
            "list-a-0", "list-a-1", "list-a-2", "list-a-3", "list-a-4", "list-b-0",
        ]),
        (json!({ "ownerAccount": "999999999999" }), vec![]),
        (
            json!({ "namespaceName": "list-ns-b", "startTime": created_at - 60.0 }),
            vec!["list-b-0"],
        ),
        (
            json!({ "namespaceName": "list-ns-b", "startTime": created_at + 60.0 }),
            vec![],
        ),
        (
            json!({ "namespaceName": "list-ns-b", "endTime": created_at - 60.0 }),
            vec![],
        ),
    ];
    for (request, expected) in windows {
        let (status, body) = call(&router, "ListSnapshots", request.clone()).await;
        let names: Vec<String> = body["snapshots"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|snapshot| {
                snapshot["snapshotName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            (status, names.as_slice()),
            (
                200,
                expected
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<String>>()
                    .as_slice()
            ),
            "case ListSnapshots({request}): got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a usage limit's resourceArn has to resolve.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_usage_limit_is_refused_unless_its_resource_arn_resolves() {
    let router = emulator();
    let namespace = create_namespace(&router, "limit-ns").await;
    let workgroup = create_workgroup(&router, "limit-wg", "limit-ns").await;
    let real_workgroup_arn = workgroup["workgroupArn"].as_str().unwrap_or_default().to_string();
    let real_namespace_arn = namespace["namespaceArn"].as_str().unwrap_or_default().to_string();
    let workgroup_id = workgroup["workgroupId"].as_str().unwrap_or_default();

    // Every row is structurally a valid ARN of this service except the last two, and each
    // names NO resource: a fresh id, another account, another Region, another resource
    // kind, and two strings that are not ARNs at all.
    let unresolvable = [
        workgroup_arn("us-east-1", ACCOUNT, ABSENT_ID),
        namespace_arn("us-east-1", ACCOUNT, ABSENT_ID),
        workgroup_arn("us-east-1", "999999999999", workgroup_id),
        workgroup_arn("eu-central-1", ACCOUNT, workgroup_id),
        format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:snapshot/{ABSENT_ID}"),
        "not-an-arn".to_string(),
        String::new(),
    ];
    for resource_arn in &unresolvable {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": resource_arn,
                "usageType": "serverless-compute",
                "amount": 60,
                "period": "daily",
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case resourceArn={resource_arn:?}: got {status} with {body}"
        );
    }

    // None of those refusals created anything.
    let (status, body) = call(
        &router,
        "ListUsageLimits",
        json!({ "resourceArn": real_workgroup_arn }),
    )
    .await;
    assert_eq!(
        (status, &body["usageLimits"]),
        (200, &json!([])),
        "ListUsageLimits after the refusals: got {status} with {body}"
    );

    // An ARN that DOES resolve is accepted, for either resource kind, and the record is
    // what the reads report. The rows differ along usage type, amount, period and
    // breach action; the ones the request omits take their documented defaults.
    let accepted: [(&str, Value, &str, &str); 3] = [
        (
            &real_workgroup_arn,
            json!({
                "resourceArn": real_workgroup_arn,
                "usageType": "serverless-compute",
                "amount": 60,
                "period": "daily",
            }),
            "daily",
            "log",
        ),
        (
            &real_workgroup_arn,
            json!({
                "resourceArn": real_workgroup_arn,
                "usageType": "serverless-compute",
                "amount": 1,
                "breachAction": "deactivate",
            }),
            "monthly",
            "deactivate",
        ),
        (
            &real_namespace_arn,
            json!({
                "resourceArn": real_namespace_arn,
                "usageType": "cross-region-datasharing",
                "amount": 1024,
                "period": "weekly",
                "breachAction": "emit-metric",
            }),
            "weekly",
            "emit-metric",
        ),
    ];
    let mut created: Vec<Value> = Vec::new();
    for (resource_arn, request, period, breach_action) in accepted {
        let (status, body) = call(&router, "CreateUsageLimit", request.clone()).await;
        assert_eq!(
            status, 200,
            "case CreateUsageLimit({request}): expected 200, got {status} with {body}"
        );
        let limit = body["usageLimit"].clone();
        assert_eq!(
            limit["resourceArn"],
            json!(resource_arn),
            "case CreateUsageLimit({request}): got {:?}",
            limit["resourceArn"]
        );
        assert_eq!(
            limit["amount"], request["amount"],
            "case CreateUsageLimit({request}): got {:?}",
            limit["amount"]
        );
        assert_eq!(
            limit["period"],
            json!(period),
            "case CreateUsageLimit({request}): got {:?}",
            limit["period"]
        );
        assert_eq!(
            limit["breachAction"],
            json!(breach_action),
            "case CreateUsageLimit({request}): got {:?}",
            limit["breachAction"]
        );
        let id = limit["usageLimitId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid(&id),
            "case CreateUsageLimit({request}): usageLimitId must be a minted UUID, got {id:?}"
        );
        assert_eq!(
            limit["usageLimitArn"],
            json!(format!(
                "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:usagelimit/{id}"
            )),
            "case CreateUsageLimit({request}): got {:?}",
            limit["usageLimitArn"]
        );

        // Round trip: the id the create minted reads the same record back.
        let (status, body) = call(&router, "GetUsageLimit", json!({ "usageLimitId": id })).await;
        assert_eq!(
            (status, &body["usageLimit"]),
            (200, &limit),
            "case GetUsageLimit({id}): got {status} with {body}"
        );
        created.push(limit);
    }

    // And the listing narrows by both members it models.
    let listings: [(Value, usize); 4] = [
        (json!({}), 3),
        (json!({ "resourceArn": real_workgroup_arn }), 2),
        (json!({ "resourceArn": real_namespace_arn }), 1),
        (
            json!({
                "resourceArn": real_workgroup_arn,
                "usageType": "cross-region-datasharing",
            }),
            0,
        ),
    ];
    for (request, expected) in listings {
        let (status, body) = call(&router, "ListUsageLimits", request.clone()).await;
        let found = body["usageLimits"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            (status, found.len()),
            (200, expected),
            "case ListUsageLimits({request}): got {status} with {body}"
        );
        for limit in &found {
            assert!(
                created.contains(limit),
                "case ListUsageLimits({request}): reported a record no create stored: \
                 {limit}"
            );
        }
    }

    // A second limit on the same resource, usage type and period is one bound too many.
    let (status, body) = call(
        &router,
        "CreateUsageLimit",
        json!({
            "resourceArn": real_workgroup_arn,
            "usageType": "serverless-compute",
            "amount": 99,
            "period": "daily",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (409, "ConflictException"),
        "a duplicate usage limit: got {status} with {body}"
    );
}

// The non-positive `amount` refusal and the unknown-`usageLimitId` 404 are pinned by
// `usage_limit_lifecycle.rs`, which owns the rest of this operation's surface; they are
// not repeated here.

// ---------------------------------------------------------------------------
// Rule 3 — the blocked DeleteNamespace states its remediation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_namespace_with_a_workgroup_attached_names_that_workgroup() {
    // The rows differ along the names involved and how many workgroups are attached: the
    // message names the workgroup that blocks the delete, whatever it is called.
    let cases: [(&str, &[&str]); 3] = [
        ("teardown-ns", &["teardown-wg"]),
        ("ns-with-long-workgroup-name", &["a-rather-long-workgroup-name"]),
        ("ns-with-two", &["wg-alpha", "wg-beta"]),
    ];

    let router = emulator();
    for (namespace, workgroups) in cases {
        create_namespace(&router, namespace).await;
        for workgroup in workgroups {
            create_workgroup(&router, workgroup, namespace).await;
        }

        let (status, body) = call(
            &router,
            "DeleteNamespace",
            json!({ "namespaceName": namespace }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (409, "ConflictException"),
            "case {namespace}: DeleteNamespace with compute attached got {status} with {body}"
        );
        let blocking = workgroups
            .iter()
            .min()
            .expect("every row attaches at least one workgroup");
        assert_eq!(
            body["message"],
            json!(format!(
                "Delete the associated workgroup {blocking} before you delete the namespace."
            )),
            "case {namespace}: the message must name the blocking workgroup, got {:?}",
            body["message"]
        );

        // The refusal changed nothing: both resources still read back.
        let (status, body) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": namespace }),
        )
        .await;
        assert_eq!(
            (status, at(&body, "namespace.namespaceName")),
            (200, &json!(namespace)),
            "case {namespace}: the namespace survives the refusal, got {body}"
        );
        for workgroup in workgroups {
            let (status, body) = call(
                &router,
                "GetWorkgroup",
                json!({ "workgroupName": workgroup }),
            )
            .await;
            assert_eq!(
                (status, at(&body, "workgroup.workgroupName")),
                (200, &json!(workgroup)),
                "case {namespace}/{workgroup}: the workgroup survives the refusal, got {body}"
            );
        }

        // Once the compute is gone the same DeleteNamespace is accepted.
        for workgroup in workgroups {
            let (status, body) = call(
                &router,
                "DeleteWorkgroup",
                json!({ "workgroupName": workgroup }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {namespace}/{workgroup}: DeleteWorkgroup got {status} with {body}"
            );
        }
        let (status, body) = call(
            &router,
            "DeleteNamespace",
            json!({ "namespaceName": namespace }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {namespace}: DeleteNamespace once the compute is gone got {status} \
             with {body}"
        );
    }
}
