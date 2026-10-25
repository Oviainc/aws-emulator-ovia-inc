// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplication` is a REPORT, pinned over a table of stored
//! applications and driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule this suite pins: a reporting read answers from the persisted record
//! the request addresses and writes nothing, so every member it reports is the
//! one the store holds, repeated reads of the same application agree, reading
//! one application cannot change what another reports, and a read that resolves
//! to no application is a modelled refusal that leaves the store as it was.
//!
//! `application_lifecycle` already pins the create -> read round trip, the
//! `(workspaceName, id)` addressing and the owed `CREATING` -> `ACTIVE`
//! transition; this suite adds the reporting invariant and reuses that suite's
//! request helpers rather than growing a second copy of them.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ApplicationRecord, ACTIVATION_DELAY,
};

use crate::application_lifecycle::{create, create_workspace, describe};
use crate::asset_lifecycle::{error_code, router_with_engine};

/// One row of the reporting table.
#[derive(Debug)]
struct ReadCase {
    label: &'static str,
    /// `WorkspaceName`: len 1..64, pattern `^[a-zA-Z0-9_-]+$` (smithy_query
    /// query="shape" shape="WorkspaceName").
    workspace: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    idc_instance_arn: &'static str,
    tags: Option<Value>,
}

/// The rows: applications differing along everything the create may vary — the
/// workspace they live in (so the table spans several workspaces rather than
/// one), the form and length of the name `ApplicationName` admits, a
/// description present and absent, the partition and shape of the Identity
/// Center instance ARN the `idcApplicationArn` is derived from, and tags
/// absent, one and several (smithy_query query="operation"
/// operation="CreateApplication").
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "required members only, shortest workspace",
            workspace: "a",
            name: "A",
            description: None,
            idc_instance_arn: "arn:aws:sso:::instance/ssoins-0000000000000001",
            tags: None,
        },
        ReadCase {
            label: "punctuated name, no description, one tag",
            workspace: "shop_floor-1",
            name: "Line (3) v1.2_check-run",
            description: None,
            idc_instance_arn: "arn:aws-us-gov:sso:::instance/ssoins-govcloud01",
            tags: Some(json!({ "tier": "gold" })),
        },
        ReadCase {
            label: "described application, several tags",
            workspace: "WORKSPACE-2",
            name: "Reporting Dashboard",
            description: Some("daily-rollup_dashboard"),
            idc_instance_arn: "arn:aws-cn:sso:::instance/ssoins-cn-0001",
            tags: Some(json!({ "tier": "silver", "owner": "ops", "env": "test" })),
        },
        ReadCase {
            label: "an instance ARN carrying no instance segment",
            workspace: "w_3",
            name: "No.Instance.Segment",
            description: Some("edge_case-record"),
            idc_instance_arn: "arn:aws:sso:::identitystore/d-9999999999",
            tags: None,
        },
    ]
}

/// The `CreateApplication` body one row sends.
fn create_body(case: &ReadCase) -> Value {
    let mut body = json!({
        "workspaceName": case.workspace,
        "name": case.name,
        "idcInstanceArn": case.idc_instance_arn,
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

/// The stored application record `id` resolves to, cloned out of the shared
/// store. The INDEPENDENT oracle for what a report must say: the assertions
/// below compare the wire answer against the persisted record rather than
/// against the create's own answer.
fn stored(shared: &engine::InMemoryEngine, id: &str) -> ApplicationRecord {
    let state = service_state(shared).expect("the iotsitewise store");
    let store = state.lock();
    store
        .applications
        .get(id)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds no application {id}"))
}

/// Every stored application, rendered as a sorted, comparable list: what a read
/// must leave exactly as it found it. Spells out the timestamps and the owed
/// transition's due instant too, so a read that silently settled or re-stamped
/// a record shows up here.
fn store_snapshot(shared: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(shared).expect("the iotsitewise store");
    let store = state.lock();
    let mut rows: Vec<String> = store
        .applications
        .values()
        .map(|record| {
            let mut tags: Vec<String> = record
                .tags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            tags.sort();
            format!(
                "{id}|{arn}|{workspace}|{name}|{description:?}|{idc_instance:?}|\
                 {idc_application}|{subdomain}|{created:?}|{updated:?}|{active:?}|\
                 {token:?}|{tags}",
                id = record.id,
                arn = record.arn,
                workspace = record.workspace_name,
                name = record.name,
                description = record.description,
                idc_instance = record.idc_instance_arn,
                idc_application = record.idc_application_arn,
                subdomain = record.dns_subdomain,
                created = record.created_at,
                updated = record.updated_at,
                active = record.active_at,
                token = record.client_token,
                tags = tags.join(","),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn describe_reports_the_stored_record_and_repeated_reads_agree() {
    let (router, shared) = router_with_engine();
    let cases = read_cases();

    // Every row gets its own workspace: an application is created IN one, and
    // the table spans several so a read cannot be answering from "the only
    // application there is".
    let mut minted: Vec<String> = Vec::new();
    for case in &cases {
        create_workspace(&router, case.workspace).await;
        let (status, created) = create(&router, create_body(case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateApplication expected 200, got {status} {created}",
            case.label
        );
        minted.push(created["id"].as_str().unwrap_or_default().to_string());
    }

    // Let the transition the create owes fall due, so every read below reports
    // the same terminal status and the answers are comparable with each other.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;

    let before = store_snapshot(&shared);
    assert_eq!(
        before.len(),
        cases.len(),
        "expected {} stored applications before any read, got {}",
        cases.len(),
        before.len()
    );

    // First pass: each report must be the stored record, member by member.
    let mut first: Vec<Value> = Vec::new();
    for (case, id) in cases.iter().zip(&minted) {
        let (status, described) = describe(&router, case.workspace, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeApplication expected 200, got {status} {described}",
            case.label
        );
        let record = stored(&shared, id);
        assert_eq!(
            (
                described["id"].as_str().unwrap_or_default(),
                described["arn"].as_str().unwrap_or_default(),
                described["workspaceName"].as_str().unwrap_or_default(),
                described["name"].as_str().unwrap_or_default(),
                described["dnsSubdomain"].as_str().unwrap_or_default(),
                described["idcApplicationArn"].as_str().unwrap_or_default(),
            ),
            (
                record.id.as_str(),
                record.arn.as_str(),
                record.workspace_name.as_str(),
                record.name.as_str(),
                record.dns_subdomain.as_str(),
                record.idc_application_arn.as_str(),
            ),
            "case {:?}: the report must be the stored record, got {described}",
            case.label
        );
        assert_eq!(
            described["description"].as_str(),
            record.description.as_deref(),
            "case {:?}: the reported description must be the stored one, got {described}",
            case.label
        );
        assert_eq!(
            described["status"].as_str().unwrap_or_default(),
            "ACTIVE",
            "case {:?}: an application whose owed transition is due reports ACTIVE, \
             got {described}",
            case.label
        );
        // Never updated since it was created, so the two instants are the same
        // one, and both are real wall-clock timestamps rather than zero.
        let created_at = &described["createdAt"];
        let updated_at = &described["updatedAt"];
        assert_eq!(
            created_at, updated_at,
            "case {:?}: a never-updated application reports updatedAt == createdAt, \
             got createdAt {created_at} and updatedAt {updated_at}",
            case.label
        );
        assert!(
            created_at.as_f64().unwrap_or_default() > 0.0,
            "case {:?}: createdAt must be a real wall-clock instant, got {created_at}",
            case.label
        );
        first.push(described);
    }

    // Second pass: the SAME requests in reverse order, several rounds. A report
    // is repeatable, and reading one application cannot change what another
    // one reports.
    for round in 0..3 {
        for (at, (case, id)) in cases.iter().zip(&minted).enumerate().rev() {
            let (status, again) = describe(&router, case.workspace, id).await;
            assert_eq!(
                status, 200,
                "case {:?}: round {round} expected 200, got {status} {again}",
                case.label
            );
            assert_eq!(
                again, first[at],
                "case {:?}: round {round} must report exactly what the first read did, \
                 got {again}",
                case.label
            );
        }
    }

    let after = store_snapshot(&shared);
    assert_eq!(
        after, before,
        "reads must change nothing a later caller sees, got {after:?}"
    );
}

/// The store-snapshot half of the rule, for the refusal path: `(workspaceName,
/// id)` addressing and the not-found refusals themselves are pinned by
/// `application_lifecycle::an_application_is_addressed_by_its_workspace_and_its_id`;
/// what is pinned HERE is that those refusals write nothing and leave the
/// served read answering exactly what it did before them.
#[tokio::test]
async fn a_read_resolving_to_no_application_refuses_and_changes_nothing() {
    let (router, shared) = router_with_engine();
    let case = &read_cases()[2];
    create_workspace(&router, case.workspace).await;
    create_workspace(&router, "other-workspace").await;
    let (status, created) = create(&router, create_body(case)).await;
    assert_eq!(
        status, 200,
        "CreateApplication expected 200, got {status} {created}"
    );
    let id = created["id"].as_str().unwrap_or_default().to_string();

    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let before = store_snapshot(&shared);
    let (status, served) = describe(&router, case.workspace, &id).await;
    assert_eq!(status, 200, "the served read got {status} {served}");

    // Each way a read can resolve to no application: an id nothing minted, this
    // application's id under another workspace this service does hold, and its
    // id under a workspace no create opened.
    let absent = [
        (
            "an id no create minted",
            case.workspace,
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("the id under another workspace", "other-workspace", id.clone()),
        ("the id under an unopened workspace", "never-opened", id.clone()),
    ];
    for (label, workspace, requested) in &absent {
        let (status, refused) = describe(&router, workspace, requested).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected a modelled not-found refusal, got {status} {refused}"
        );
    }

    let after = store_snapshot(&shared);
    assert_eq!(
        after, before,
        "a refused read must change nothing a later caller sees, got {after:?}"
    );
    // And the application the store does hold still reports exactly what it did
    // before those refusals.
    let (status, again) = describe(&router, case.workspace, &id).await;
    assert_eq!(
        (status, &again),
        (200, &served),
        "the served read must be unaffected by the refused ones, got {status} {again}"
    );
}
