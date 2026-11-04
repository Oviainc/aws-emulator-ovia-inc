// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workflow-VERSION slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the version-specific helpers live here.
//!
//! The rules under test:
//!
//! * a version is a resource of its own, stored once inside the workflow its name is
//!   unique within: whatever `CreateWorkflowVersion` carried is what a later
//!   `GetWorkflowVersion` reports, and NOTHING is taken from the workflow, whose
//!   configuration a version does not inherit;
//!   * a version name is unique per workflow, so a second create under a name the
//!     workflow already holds is refused while the same name on another workflow is
//!     accepted, and a replayed `requestId` answers with the version the first call
//!     created rather than minting a second one;
//! * a freshly created version owes the clock its move to `ACTIVE`, which a read
//!   observes but never causes;
//! * the listing reports every version its workflow holds and nothing else: each item
//!   carries exactly the members the list shape models, read from the same record the
//!   single-version read answers from, in one stable order the page bounds walk; the
//!   workflow it lists is resolved from the request's id narrowed by `type` and
//!   `workflowOwnerId`, so an id, type or owner no workflow of this account matches is a
//!   modelled 404 rather than an empty page;
//! * a version `DeleteWorkflowVersion` removed is a modelled 404 everywhere afterwards,
//!   and while a workflow still holds any version, deleting the workflow is refused.

use serde_json::{json, Value};
use service_com_amazonaws_omics_omics::state::MAX_TAGS;
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    assert_absent, assert_arn, assert_members, assert_timestamp, assert_uuid, call,
    create_workflow, created_id, delete_workflow, emulator, error_code, expected_digest,
    get_workflow, items, tag_map, ACCOUNT, CAPACITY_RANGE, POLL_INTERVAL, SETTLE_DEADLINE,
    STORAGE_TYPES, WORKFLOW_ENGINES,
};

/// `POST /workflow/{workflowId}/version`.
async fn create_version(
    router: &MultiServiceRouter,
    workflow_id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/workflow/{workflow_id}/version"),
        Some(body),
    )
    .await
}

/// `GET /workflow/{workflowId}/version/{versionName}`.
async fn get_version(
    router: &MultiServiceRouter,
    workflow_id: &str,
    version_name: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workflow/{workflow_id}/version/{version_name}"),
        None,
    )
    .await
}

/// `DELETE /workflow/{workflowId}/version/{versionName}`.
async fn delete_version(
    router: &MultiServiceRouter,
    workflow_id: &str,
    version_name: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/workflow/{workflow_id}/version/{version_name}"),
        None,
    )
    .await
}

/// `GET /workflow/{workflowId}/version{query}`.
async fn list_versions(
    router: &MultiServiceRouter,
    workflow_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workflow/{workflow_id}/version{query}"),
        None,
    )
    .await
}

/// The `versionName` of each item in a listing page, in the order the page reported
/// them.
fn item_version_names(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|item| {
            item.get("versionName")
                .and_then(Value::as_str)
                .unwrap_or("<no versionName>")
                .to_string()
        })
        .collect()
}

/// The `message` a modelled error carried.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A workflow to hang versions on, named after the case that needs it.
async fn workflow_for_versions(router: &MultiServiceRouter, request_id: &str) -> String {
    let (status, body) =
        create_workflow(router, &json!({ "requestId": format!("wf-{request_id}") })).await;
    created_id(request_id, status, &body)
}

/// Read the version until it reports `expected`, and answer that response.
///
/// Polling rather than sleeping a fixed span: the status is owed to the clock, so what is
/// asserted is that it arrives, not when a single read happened to land.
async fn await_version_status(
    router: &MultiServiceRouter,
    workflow_id: &str,
    version_name: &str,
    expected: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = get_version(router, workflow_id, version_name).await;
        assert_eq!(
            status, 200,
            "version {version_name} of workflow {workflow_id}: expected 200, got {body:?}"
        );
        if body.get("status").and_then(Value::as_str) == Some(expected) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "version {version_name} of workflow {workflow_id}: expected status {expected} \
             within {SETTLE_DEADLINE:?}, last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A row of the version create/read-back table: a request body, and the members a later
/// `GetWorkflowVersion` must report for it.
struct VersionCase {
    /// How the row is reported in a failure, and the version's name.
    case: &'static str,
    /// The request body, without the `versionName`/`requestId` this row's `case` fills in.
    request: Value,
    /// Every member the read-back must carry, exactly.
    expected: Value,
    /// Members the read-back must NOT carry, because nothing stored them.
    absent: Vec<&'static str>,
}

/// The table. The rule is "a Get reports what the create stored", so the rows differ along
/// what the rule generalises over: which optional members are present, every value of the
/// `engine` and `storageType` catalogs, both published bounds of the `storageCapacity`
/// range, the number of `parameterTemplate` and `tags` entries, the form the definition
/// arrived in (none, URI, ZIP, both, or a repository) — which is also what decides whether
/// a content `digest` can be reported at all — and the forms the modelled
/// `WorkflowVersionName` pattern `^[A-Za-z0-9][A-Za-z0-9\-\._]*$` admits.
fn version_cases() -> Vec<VersionCase> {
    let zip: &[u8] = b"PK\x03\x04 emulator workflow version archive";
    let other_zip: &[u8] = b"PK\x03\x04 a second, different archive";
    vec![
        VersionCase {
            // Nothing but the two required members: every optional one must be absent,
            // not defaulted and not borrowed from the workflow.
            case: "1",
            request: json!({}),
            expected: json!({ "type": "PRIVATE" }),
            absent: vec![
                "description",
                "engine",
                "main",
                "definition",
                "digest",
                "parameterTemplate",
                "storageCapacity",
                "storageType",
                "accelerators",
                "tags",
                "readmePath",
                "workflowBucketOwnerId",
                "definitionRepositoryDetails",
                "metadata",
                "readme",
                "profiles",
                "profileParameterTemplates",
                "statusMessage",
            ],
            },
        VersionCase {
            case: "2.7.0",
            request: json!({
                "definitionZip": aws_smithy_types::base64::encode(zip),
                "engine": WORKFLOW_ENGINES[0],
                "storageType": STORAGE_TYPES[0],
                "storageCapacity": CAPACITY_RANGE.0,
                "accelerators": "GPU",
                "description": "the first released version",
                "parameterTemplate": { "sample": { "description": "the sample", "optional": false } },
                "tags": tag_map(1),
            }),
            expected: json!({
                "type": "PRIVATE",
                "engine": WORKFLOW_ENGINES[0],
                "storageType": STORAGE_TYPES[0],
                "storageCapacity": CAPACITY_RANGE.0,
                "accelerators": "GPU",
                "description": "the first released version",
                "digest": expected_digest(zip),
                "parameterTemplate": { "sample": { "description": "the sample", "optional": false } },
                "tags": tag_map(1),
            }),
            // Nothing named an S3 definition, so there is no `definition` to report.
            absent: vec!["definition", "main", "readmePath"],
        },
        VersionCase {
            case: "release-2",
            request: json!({
                "definitionUri": "s3://omics-emulator-bucket/release-2.zip",
                "main": "workflow-definition/main.nf",
                "engine": WORKFLOW_ENGINES[1],
                "storageType": STORAGE_TYPES[1],
                "storageCapacity": CAPACITY_RANGE.1,
                "readmePath": "docs/README.md",
                "workflowBucketOwnerId": ACCOUNT,
                "parameterTemplate": {
                    "reads": { "description": "input reads" },
                    "reference": { "description": "reference genome", "optional": true },
                    "threads": { "optional": true },
                },
                "tags": tag_map(MAX_TAGS),
            }),
            expected: json!({
                "type": "PRIVATE",
                "definition": "s3://omics-emulator-bucket/release-2.zip",
                "main": "workflow-definition/main.nf",
                "engine": WORKFLOW_ENGINES[1],
                "storageType": STORAGE_TYPES[1],
                "storageCapacity": CAPACITY_RANGE.1,
                "readmePath": "docs/README.md",
                "workflowBucketOwnerId": ACCOUNT,
                "parameterTemplate": {
                    "reads": { "description": "input reads" },
                    "reference": { "description": "reference genome", "optional": true },
                    "threads": { "optional": true },
                },
                "tags": tag_map(MAX_TAGS),
            }),
            // Only a URI, so this emulator never read the archive: no content `digest`
            // may be reported for it.
            absent: vec!["digest", "description", "accelerators"],
        },
        VersionCase {
            case: "ver_4",
            request: json!({
                "definitionZip": aws_smithy_types::base64::encode(other_zip),
                "definitionUri": "s3://omics-emulator-bucket/ver-4.zip",
                "engine": WORKFLOW_ENGINES[2],
            }),
            expected: json!({
                "type": "PRIVATE",
                "definition": "s3://omics-emulator-bucket/ver-4.zip",
                "digest": expected_digest(other_zip),
                "engine": WORKFLOW_ENGINES[2],
            }),
            absent: vec!["main", "storageType", "storageCapacity", "tags"],
        },
        VersionCase {
            case: "5",
            request: json!({
                "engine": WORKFLOW_ENGINES[3],
                "definitionRepository": {
                    "connectionArn": format!(
                        "arn:aws:codeconnections:us-west-2:{ACCOUNT}:connection/omics-emulator"
                    ),
                    "fullRepositoryId": "genomics-org/pipelines",
                    // Source: `smithy_query(query="shape",shape="SourceReferenceType")`
                    // -> BRANCH | TAG | COMMIT.
                    "sourceReference": { "type": "TAG", "value": "v5.0.0" },
                },
            }),
            expected: json!({
                "type": "PRIVATE",
                "engine": WORKFLOW_ENGINES[3],
                "definitionRepositoryDetails": {
                    "connectionArn": format!(
                        "arn:aws:codeconnections:us-west-2:{ACCOUNT}:connection/omics-emulator"
                    ),
                    "fullRepositoryId": "genomics-org/pipelines",
                    "sourceReference": { "type": "TAG", "value": "v5.0.0" },
                },
            }),
            // A repository is not an archive this emulator read either, so still no
            // content digest.
            absent: vec!["digest", "definition"],
        },
    ]
}

/// The request body for a row: its own members, plus the name and token.
fn version_request(version: &VersionCase) -> Value {
    let mut body = version
        .request
        .as_object()
        .expect("a row's request is an object")
        .clone();
    body.insert("versionName".to_string(), json!(version.case));
    body.insert(
        "requestId".to_string(),
        json!(format!("token-{}", version.case)),
    );
    Value::Object(body)
}

#[tokio::test]
async fn a_created_version_is_read_back_exactly_as_it_was_stored() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "readback").await;
    let cases = version_cases();

    let mut created: Vec<(String, String)> = Vec::new();
    for version in &cases {
        let (status, body) = create_version(&router, &workflow_id, &version_request(version)).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {body:?}",
            version.case
        );
        let arn = body
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: response carried no arn", version.case));
        // A version is named inside its workflow's own resource path.
        assert_arn(
            version.case,
            arn,
            "workflow",
            &format!("{workflow_id}/version/{}", version.case),
        );
        let uuid = body.get("uuid").and_then(Value::as_str).unwrap_or_default();
        assert_uuid(version.case, uuid);
        assert_members(
            version.case,
            "CreateWorkflowVersion",
            &json!({
                "workflowId": workflow_id,
                "versionName": version.case,
                // A just-created version reports the transient status, so the span the
                // clock owes is never zero.
                "status": "CREATING",
            }),
            &body,
        );
        assert_eq!(
            body.get("tags"),
            version.expected.get("tags"),
            "case {:?}: the create must echo the tags it stored, got {body:?}",
            version.case
        );
        created.push((arn.to_string(), uuid.to_string()));
    }

    for (version, (arn, uuid)) in cases.iter().zip(&created) {
        // Read until the clock has settled the CREATING -> ACTIVE transition the create
        // owes, then assert the whole record that read reported.
        let body = await_version_status(&router, &workflow_id, version.case, "ACTIVE").await;
        assert_members(version.case, "GetWorkflowVersion", &version.expected, &body);
        assert_absent(version.case, &version.absent, &body);
        assert_members(
            version.case,
            "GetWorkflowVersion",
            &json!({
                "workflowId": workflow_id,
                "versionName": version.case,
                "arn": arn,
                "uuid": uuid,
                "status": "ACTIVE",
            }),
            &body,
        );
        assert_timestamp(version.case, "creationTime", &body);
    }
}

#[tokio::test]
async fn a_version_inherits_no_configuration_from_its_workflow() {
    let router = emulator();
    // The workflow carries a value for every member a version could have borrowed.
    let (status, body) = create_workflow(
        &router,
        &json!({
            "requestId": "parent",
            "name": "parent-workflow",
            "description": "the workflow, not the version",
            "engine": "WDL",
            "main": "main.wdl",
            "definitionUri": "s3://omics-emulator-bucket/parent.zip",
            "storageType": "STATIC",
            "storageCapacity": 1200,
            "accelerators": "GPU",
            "readmePath": "docs/parent.md",
            "parameterTemplate": { "sample": { "description": "the sample" } },
            "tags": { "team": "genomics" },
        }),
    )
    .await;
    let workflow_id = created_id("parent", status, &body);

    // The version declares none of them.
    let (status, body) = create_version(
        &router,
        &workflow_id,
        &json!({ "versionName": "bare", "requestId": "bare" }),
    )
    .await;
    assert_eq!(status, 201, "a bare version must be created, got {body:?}");

    let version = await_version_status(&router, &workflow_id, "bare", "ACTIVE").await;
    assert_absent(
        "no inheritance",
        &[
            "description",
            "engine",
            "main",
            "definition",
            "storageType",
            "storageCapacity",
            "accelerators",
            "readmePath",
            "parameterTemplate",
            "tags",
        ],
        &version,
    );

    // And the workflow still reports its own configuration: creating a version changed
    // nothing about it.
    let (status, workflow) = get_workflow(&router, &workflow_id).await;
    assert_eq!(status, 200, "the workflow must still read, got {workflow:?}");
    assert_members(
        "no inheritance",
        "GetWorkflow",
        &json!({
            "description": "the workflow, not the version",
            "engine": "WDL",
            "main": "main.wdl",
            "definition": "s3://omics-emulator-bucket/parent.zip",
            "storageType": "STATIC",
            "storageCapacity": 1200,
            "accelerators": "GPU",
            "readmePath": "docs/parent.md",
            "parameterTemplate": { "sample": { "description": "the sample" } },
            "tags": { "team": "genomics" },
        }),
        &workflow,
    );
}

#[tokio::test]
async fn a_version_name_is_unique_within_its_workflow_only() {
    let router = emulator();
    let first = workflow_for_versions(&router, "unique-first").await;
    let second = workflow_for_versions(&router, "unique-second").await;

    // The rule generalises over the name, so the rows are the forms the modelled
    // `WorkflowVersionName` pattern `^[A-Za-z0-9][A-Za-z0-9\-\._]*$` admits.
    for name in ["v1", "2.7.0", "release-1", "ver_1", "9"] {
        let (status, body) = create_version(
            &router,
            &first,
            &json!({ "versionName": name, "requestId": format!("first-{name}") }),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {body:?}");
        let uuid = body
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // The same name again, under a token the first call did not use: refused,
        // because the name is already taken on this workflow.
        let (status, body) = create_version(
            &router,
            &first,
            &json!({ "versionName": name, "requestId": format!("again-{name}") }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name:?}: a duplicate version name must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {body:?}"
        );

        // The refusal left the stored version exactly as it was.
        let (status, body) = get_version(&router, &first, name).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {body:?}");
        assert_eq!(
            body.get("uuid").and_then(Value::as_str),
            Some(uuid.as_str()),
            "case {name:?}: a refused duplicate must not replace the version, got {body:?}"
        );

        // The SAME name on another workflow is a different version, so it is accepted.
        let (status, body) = create_version(
            &router,
            &second,
            &json!({ "versionName": name, "requestId": format!("second-{name}") }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: the same name on another workflow must be accepted, got {body:?}"
        );
        assert_eq!(
            body.get("workflowId").and_then(Value::as_str),
            Some(second.as_str()),
            "case {name:?}: expected the second workflow's id, got {body:?}"
        );
        assert_ne!(
            body.get("uuid").and_then(Value::as_str),
            Some(uuid.as_str()),
            "case {name:?}: two versions must not share a uuid, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_replayed_request_id_answers_with_the_version_it_created() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "idempotent").await;

    // The rule generalises over the token and over what the retry asks for, so the rows
    // differ in both: a retry naming the same version, and one naming another.
    for (token, first_name, retry_name) in [
        ("token-same", "v1", "v1"),
        ("token-other", "v2", "v2-renamed"),
    ] {
        let (status, first) = create_version(
            &router,
            &workflow_id,
            &json!({ "versionName": first_name, "requestId": token }),
        )
        .await;
        assert_eq!(status, 201, "case {token:?}: expected 201, got {first:?}");

        let (status, replay) = create_version(
            &router,
            &workflow_id,
            &json!({ "versionName": retry_name, "requestId": token }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {token:?}: a replayed token must answer, got {replay:?}"
        );
        assert_members(
            token,
            "CreateWorkflowVersion replay",
            &json!({
                "arn": first.get("arn").cloned().unwrap_or(Value::Null),
                "uuid": first.get("uuid").cloned().unwrap_or(Value::Null),
                "versionName": first_name,
                "workflowId": workflow_id,
            }),
            &replay,
        );

        // Nothing second was created: the name the retry asked for exists only when it
        // is the first call's own name.
        if retry_name != first_name {
            let (status, body) = get_version(&router, &workflow_id, retry_name).await;
            assert_eq!(
                status, 404,
                "case {token:?}: a replay must not mint a second version, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_missing_workflow_or_version_is_a_modelled_404() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "absent").await;
    let (status, body) = create_version(
        &router,
        &workflow_id,
        &json!({ "versionName": "present", "requestId": "present" }),
    )
    .await;
    assert_eq!(status, 201, "the one real version must exist, got {body:?}");

    // A workflow this emulator does not hold: there is no collection for the version to
    // join. The rows differ in the id, which the message names.
    for absent_workflow in ["7", "999999999", "0"] {
        let (status, body) = create_version(
            &router,
            absent_workflow,
            &json!({ "versionName": "v1", "requestId": format!("create-{absent_workflow}") }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {absent_workflow:?}: expected 404, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {absent_workflow:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Workflow {absent_workflow} not found"),
            "case {absent_workflow:?}: unexpected message in {body:?}"
        );
    }

    // A version nobody created, on a workflow that exists and on one that does not.
    for (case, workflow, name) in [
        ("absent name", workflow_id.as_str(), "nope"),
        ("absent name, dotted", workflow_id.as_str(), "1.0.0"),
        ("absent workflow", "999999999", "present"),
    ] {
        let (status, body) = get_version(&router, workflow, name).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Workflow {workflow} with version name {name} not found"),
            "case {case:?}: unexpected message in {body:?}"
        );

        let (status, body) = delete_version(&router, workflow, name).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Workflow {workflow} version {name} not found"),
            "case {case:?}: unexpected message in {body:?}"
        );
    }

    // A version named on another account's workflow is not this account's to read, and a
    // type the workflow is not narrows the lookup away as well.
    // Source: `smithy_query(query="shape",shape="WorkflowType")` -> PRIVATE | READY2RUN.
    for (case, query, expected) in [
        ("this account", format!("?workflowOwnerId={ACCOUNT}"), 200),
        ("another account", "?workflowOwnerId=210987654321".to_string(), 404),
        ("the workflow's own type", "?type=PRIVATE".to_string(), 200),
        ("another type", "?type=READY2RUN".to_string(), 404),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/workflow/{workflow_id}/version/present{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_version_reaches_active_on_the_clock_and_not_on_a_read() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "clock").await;
    // `unread` is created FIRST, so the transition it is owed falls due no later than
    // `read`'s: once `read` has settled, `unread`'s due instant has certainly passed,
    // whatever the machine's timing.
    for name in ["unread", "read"] {
        let (status, body) = create_version(
            &router,
            &workflow_id,
            &json!({ "versionName": name, "requestId": format!("clock-{name}") }),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {body:?}");
    }

    // The transient status is observable: a read taken straight after the create still
    // sees it, so the span is never zero.
    let (_, before) = get_version(&router, &workflow_id, "read").await;
    assert_eq!(
        before.get("status").and_then(Value::as_str),
        Some("CREATING"),
        "a version read before its transition is due must still be CREATING, got {before:?}"
    );

    let after = await_version_status(&router, &workflow_id, "read", "ACTIVE").await;
    // Reading does not move it again, and does not restamp the creation time.
    let (_, again) = get_version(&router, &workflow_id, "read").await;
    assert_eq!(
        again.get("status"),
        after.get("status"),
        "a second read must report the same status, got {again:?}"
    );
    assert_eq!(
        again.get("creationTime"),
        after.get("creationTime"),
        "a second read must report the same creationTime, got {again:?}"
    );

    // The transition was owed to the clock, not to the polling: the version nobody read
    // until now reports ACTIVE on its FIRST read.
    let (status, unread) = get_version(&router, &workflow_id, "unread").await;
    assert_eq!(status, 200, "expected 200, got {unread:?}");
    assert_eq!(
        unread.get("status").and_then(Value::as_str),
        Some("ACTIVE"),
        "a version nobody polled must still have settled, got {unread:?}"
    );
}

#[tokio::test]
async fn a_workflow_that_still_holds_versions_cannot_be_deleted() {
    let router = emulator();
    // The rule generalises over how many versions the workflow holds: each row deletes
    // them one at a time and the delete is refused until the last one is gone.
    for count in 1usize..=3 {
        let workflow_id = workflow_for_versions(&router, &format!("blocked-{count}")).await;
        let names: Vec<String> = (0..count).map(|n| format!("v{n}")).collect();
        for name in &names {
            let (status, body) = create_version(
                &router,
                &workflow_id,
                &json!({ "versionName": name, "requestId": format!("{workflow_id}-{name}") }),
            )
            .await;
            assert_eq!(status, 201, "case {count}: expected 201, got {body:?}");
        }

        for (deleted, name) in names.iter().enumerate() {
            let (status, body) = delete_workflow(&router, &workflow_id).await;
            assert_eq!(
                status, 400,
                "case {count}: with {} version(s) left the workflow delete must be refused, \
                 got {body:?}",
                count - deleted
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {count}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                "Cannot delete the requested workflow, as it has one or more versions. \
                 Delete the versions and try again.",
                "case {count}: unexpected message in {body:?}"
            );
            let (status, body) = get_workflow(&router, &workflow_id).await;
            assert_eq!(
                status, 200,
                "case {count}: a refused delete must leave the workflow in place, got {body:?}"
            );

            let (status, body) = delete_version(&router, &workflow_id, name).await;
            assert_eq!(
                status, 202,
                "case {count}: deleting version {name} must be accepted, got {body:?}"
            );
            let (status, body) = get_version(&router, &workflow_id, name).await;
            assert_eq!(
                status, 404,
                "case {count}: a deleted version must be gone, got {body:?}"
            );
        }

        // Every version is gone, so the workflow delete is now accepted — and the
        // workflow with it.
        let (status, body) = delete_workflow(&router, &workflow_id).await;
        assert_eq!(
            status, 202,
            "case {count}: the last version gone, the workflow delete must be accepted, \
             got {body:?}"
        );
        let (status, body) = get_workflow(&router, &workflow_id).await;
        assert_eq!(
            status, 404,
            "case {count}: an accepted delete must remove the workflow, got {body:?}"
        );
    }
}

#[tokio::test]
async fn an_export_of_a_version_is_refused_rather_than_answered() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "export").await;
    let (status, body) = create_version(
        &router,
        &workflow_id,
        &json!({ "versionName": "exported", "requestId": "export" }),
    )
    .await;
    assert_eq!(status, 201, "expected 201, got {body:?}");

    // An export asks for a presigned URL to an exported object; this emulator serves no
    // object store, so the request is refused with 501 `NotImplemented` instead of a 200
    // that would read as an export which succeeded and produced nothing. The rule
    // generalises over every value the modelled export list admits and over asking for
    // more than one.
    // Source: `smithy_query(query="shape",shape="WorkflowExport")` -> DEFINITION | README.
    for (case, query) in [
        ("DEFINITION", "?export=DEFINITION"),
        ("README", "?export=README"),
        ("both", "?export=DEFINITION&export=README"),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/workflow/{workflow_id}/version/exported{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 501,
            "case {case:?}: an export must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body:?}"
        );
    }

    // A read that asks for no export is served in full from state.
    let (status, body) = get_version(&router, &workflow_id, "exported").await;
    assert_eq!(status, 200, "a read without an export must succeed, got {body:?}");
}

#[tokio::test]
async fn more_tags_than_the_service_accepts_are_refused() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "tags").await;
    // The bound is MAX_TAGS, documented on the `tags` member ("You can define up to 50
    // tags for the workflow"), so the row at the bound is accepted and the row one past
    // it is refused.
    for (case, count, expect_ok) in [
        ("at the tag bound", MAX_TAGS, true),
        ("one past the tag bound", MAX_TAGS + 1, false),
    ] {
        let (status, body) = create_version(
            &router,
            &workflow_id,
            &json!({
                "versionName": format!("tagged-{count}"),
                "requestId": format!("tags-{count}"),
                "tags": tag_map(count),
            }),
        )
        .await;
        if expect_ok {
            assert_eq!(status, 201, "case {case:?}: expected 201, got {body:?}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ListWorkflowVersions
// ---------------------------------------------------------------------------

/// Every member the listing's item shape models — which is everything an item may carry.
/// Source: `smithy_query(query="shape",shape="WorkflowVersionListItem")` -> arn,
/// workflowId, versionName, description, status, type, digest, creationTime, metadata.
const VERSION_LIST_ITEM_MEMBERS: &[&str] = &[
    "arn",
    "workflowId",
    "versionName",
    "description",
    "status",
    "type",
    "digest",
    "creationTime",
    "metadata",
];

/// Members `GetWorkflowVersion` reports that the list item shape does NOT model, so no
/// listing may carry them.
const NOT_ON_A_VERSION_LIST_ITEM: &[&str] = &[
    "uuid",
    "engine",
    "main",
    "definition",
    "parameterTemplate",
    "storageCapacity",
    "storageType",
    "accelerators",
    "tags",
    "readmePath",
    "workflowBucketOwnerId",
    "definitionRepositoryDetails",
    "statusMessage",
];

#[tokio::test]
async fn the_listing_reports_every_version_from_the_record_the_read_answers_from() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "listing").await;
    // The rule is "the listing reports each stored version's own members", so the rows
    // are the create/read-back table: every form a version can have been stored in,
    // including the ones that decide whether a `description` or a content `digest` exists
    // to report at all.
    let cases = version_cases();
    for version in &cases {
        let (status, body) = create_version(&router, &workflow_id, &version_request(version)).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {body:?}",
            version.case
        );
    }
    // Settle every version first, so the listing and the read are compared on the same
    // side of the clock.
    for version in &cases {
        await_version_status(&router, &workflow_id, version.case, "ACTIVE").await;
    }

    let (status, body) = list_versions(&router, &workflow_id, "").await;
    let page = items("every version", status, &body);
    let mut expected_names: Vec<String> = cases.iter().map(|row| row.case.to_string()).collect();
    expected_names.sort();
    assert_eq!(
        item_version_names(&page),
        expected_names,
        "expected every stored version once, got {page:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "a page holding the whole collection must carry no nextToken, got {body:?}"
    );

    for version in &cases {
        let item = page
            .iter()
            .find(|item| {
                item.get("versionName").and_then(Value::as_str) == Some(version.case)
            })
            .unwrap_or_else(|| {
                panic!("case {:?}: the listing omitted it, got {page:?}", version.case)
            });
        let (status, read) = get_version(&router, &workflow_id, version.case).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from the read, got {read:?}",
            version.case
        );
        // One record, two readers: every member the item shape models carries what the
        // single-version read reports for it, present or absent.
        for member in VERSION_LIST_ITEM_MEMBERS {
            assert_eq!(
                item.get(*member),
                read.get(*member),
                "case {:?}: expected {member} = {:?} as GetWorkflowVersion reports it, \
                 got {:?} in the whole item {item:?}",
                version.case,
                read.get(*member),
                item.get(*member)
            );
        }
        assert_absent(version.case, NOT_ON_A_VERSION_LIST_ITEM, item);
        assert_members(
            version.case,
            "ListWorkflowVersions",
            &json!({
                "workflowId": workflow_id,
                "versionName": version.case,
                "type": "PRIVATE",
                "status": "ACTIVE",
            }),
            item,
        );
        assert_arn(
            version.case,
            item.get("arn").and_then(Value::as_str).unwrap_or_default(),
            "workflow",
            &format!("{workflow_id}/version/{}", version.case),
        );
        assert_timestamp(version.case, "creationTime", item);
        // The two members a row's own stored configuration decides.
        for member in ["digest", "description"] {
            assert_eq!(
                item.get(member),
                version.expected.get(member),
                "case {:?}: expected {member} = {:?} as the create stored it, got {:?}",
                version.case,
                version.expected.get(member),
                item.get(member)
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_through_one_stable_order() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "version-paging").await;
    // Created in an order that is NOT the order the listing reports, so what the pages
    // concatenate to is the collection's own order rather than the order of the creates.
    let created = ["beta", "alpha", "echo", "charlie", "delta"];
    for name in created {
        let (status, body) = create_version(
            &router,
            &workflow_id,
            &json!({ "versionName": name, "requestId": format!("paging-{name}") }),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {body:?}");
    }
    // A version is keyed by the name it is unique under, so that name's order is the one
    // stable order the pages walk.
    let mut expected: Vec<String> = created.iter().map(|name| name.to_string()).collect();
    expected.sort();

    // The rule generalises over the page size: the pages concatenate to the whole
    // collection in that order whatever size is asked for, and only a page with more
    // behind it carries a nextToken.
    for page_size in [1usize, 2, 3, 5, 100] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&startingToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = list_versions(&router, &workflow_id, &query).await;
            let page = items(&format!("page size {page_size}"), status, &body);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page carried {} items",
                page.len()
            );
            seen.extend(item_version_names(&page));
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "page size {page_size}: paging did not terminate"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected every version in version-name order"
        );
    }

    // The last page of an exactly-filled collection carries no token.
    let (status, body) = list_versions(&router, &workflow_id, "?maxResults=5").await;
    let page = items("exact fit", status, &body);
    assert_eq!(page.len(), 5, "expected all five versions, got {page:?}");
    assert!(
        body.get("nextToken").is_none(),
        "a page with nothing behind it must carry no nextToken, got {body:?}"
    );

    // A token this operation never issued is a bad request, not a page.
    let (status, body) = list_versions(&router, &workflow_id, "?startingToken=not-a-token").await;
    assert_eq!(
        status, 400,
        "an unissued startingToken must be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body:?}"
    );

    // Nor is another listing's token one this listing issued: a second workflow gives
    // `ListWorkflows` a further page, so it mints a token of its own.
    let _second = workflow_for_versions(&router, "version-paging-second").await;
    let (status, body) = call(&router, "GET", "/workflow?maxResults=1", None).await;
    items("workflow page", status, &body);
    let workflow_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ListWorkflows nextToken, got {body:?}"))
        .to_string();
    let (status, body) = list_versions(&router, &workflow_id, "?maxResults=1").await;
    items("version page", status, &body);
    let version_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ListWorkflowVersions nextToken, got {body:?}"))
        .to_string();

    // Each token resumes its OWN listing,
    let (status, body) =
        list_versions(&router, &workflow_id, &format!("?startingToken={version_token}")).await;
    assert_eq!(
        status, 200,
        "a ListWorkflowVersions token must resume ListWorkflowVersions, got {body:?}"
    );
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workflow?startingToken={workflow_token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a ListWorkflows token must resume ListWorkflows, got {body:?}"
    );

    // and is refused by the other, rather than silently resuming the wrong collection.
    for (case, token, into_versions) in [
        ("workflow token given to the version listing", &workflow_token, true),
        ("version token given to ListWorkflows", &version_token, false),
    ] {
        let (status, body) = if into_versions {
            list_versions(&router, &workflow_id, &format!("?startingToken={token}")).await
        } else {
            call(
                &router,
                "GET",
                &format!("/workflow?startingToken={token}"),
                None,
            )
            .await
        };
        assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // A version name is a position inside ONE workflow's collection, so a token minted
    // while paging one workflow is not a resume point in another's: it is refused rather
    // than read as a position among that workflow's own names.
    let other_workflow = workflow_for_versions(&router, "version-paging-other").await;
    for name in ["alpha", "zulu"] {
        let (status, body) = create_version(
            &router,
            &other_workflow,
            &json!({ "versionName": name, "requestId": format!("other-{name}") }),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {body:?}");
    }
    let (status, body) =
        list_versions(&router, &other_workflow, &format!("?startingToken={version_token}")).await;
    assert_eq!(
        status, 400,
        "a token issued for another workflow must be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body:?}"
    );
    // The other workflow pages with its OWN token, and the token it mints is refused
    // back on the first workflow for the same reason.
    let (status, body) = list_versions(&router, &other_workflow, "?maxResults=1").await;
    let page = items("other workflow page", status, &body);
    assert_eq!(
        item_version_names(&page),
        vec!["alpha".to_string()],
        "expected the other workflow's own first page, got {page:?}"
    );
    let other_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a nextToken for the other workflow, got {body:?}"))
        .to_string();
    let (status, body) =
        list_versions(&router, &other_workflow, &format!("?startingToken={other_token}")).await;
    let page = items("other workflow resumed", status, &body);
    assert_eq!(
        item_version_names(&page),
        vec!["zulu".to_string()],
        "a workflow's own token must resume its own walk, got {page:?}"
    );
    let (status, body) =
        list_versions(&router, &workflow_id, &format!("?startingToken={other_token}")).await;
    assert_eq!(
        status, 400,
        "a token issued for another workflow must be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body:?}"
    );
}

#[tokio::test]
async fn the_listing_resolves_its_workflow_before_it_reports_any_version() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "narrowed").await;
    let (status, body) = create_version(
        &router,
        &workflow_id,
        &json!({ "versionName": "present", "requestId": "narrowed" }),
    )
    .await;
    assert_eq!(status, 201, "expected 201, got {body:?}");

    // The collection belongs to the workflow the request's id, `type` and
    // `workflowOwnerId` resolve: a type the workflow is not, or an owner that is not this
    // emulator's account, resolves no workflow at all.
    // Source: `smithy_query(query="shape",shape="WorkflowType")` -> PRIVATE | READY2RUN.
    let rows: Vec<(&str, String, u16)> = vec![
        ("no narrowing", String::new(), 200),
        ("this account", format!("?workflowOwnerId={ACCOUNT}"), 200),
        (
            "another account",
            "?workflowOwnerId=210987654321".to_string(),
            404,
        ),
        ("the workflow's own type", "?type=PRIVATE".to_string(), 200),
        ("another type", "?type=READY2RUN".to_string(), 404),
        (
            "both, matching",
            format!("?type=PRIVATE&workflowOwnerId={ACCOUNT}"),
            200,
        ),
    ];
    for (case, query, expected) in &rows {
        let (status, body) = list_versions(&router, &workflow_id, query).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
        if *expected == 200 {
            let page = items(case, status, &body);
            assert_eq!(
                item_version_names(&page),
                vec!["present".to_string()],
                "case {case:?}: expected the one stored version, got {page:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // An id no workflow carries is that same modelled 404, worded as this slice words an
    // absent workflow, and never an empty page.
    for absent in ["999999999", "1"] {
        let (status, body) = list_versions(&router, absent, "").await;
        assert_eq!(status, 404, "case {absent:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {absent:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Workflow {absent} not found"),
            "case {absent:?}: unexpected message in {body:?}"
        );
    }

    // A workflow that holds no version is an EMPTY page, not a 404: that workflow exists.
    let empty = workflow_for_versions(&router, "no-versions").await;
    let (status, body) = list_versions(&router, &empty, "").await;
    let page = items("no versions", status, &body);
    assert!(
        page.is_empty(),
        "a workflow with no version must list nothing, got {page:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "an empty page must carry no nextToken, got {body:?}"
    );

    // What the store holds is what the listing reports: a deleted version leaves it, and
    // the rest stay.
    let (status, body) = create_version(
        &router,
        &workflow_id,
        &json!({ "versionName": "second", "requestId": "narrowed-second" }),
    )
    .await;
    assert_eq!(status, 201, "expected 201, got {body:?}");
    let (status, body) = delete_version(&router, &workflow_id, "present").await;
    assert_eq!(status, 202, "the delete must be accepted, got {body:?}");
    let (status, body) = list_versions(&router, &workflow_id, "").await;
    let page = items("after a delete", status, &body);
    assert_eq!(
        item_version_names(&page),
        vec!["second".to_string()],
        "a deleted version must leave the listing, got {page:?}"
    );
}

#[tokio::test]
async fn the_listing_observes_the_clock_and_changes_nothing() {
    let router = emulator();
    let workflow_id = workflow_for_versions(&router, "clock-listing").await;
    for name in ["v1", "v2"] {
        let (status, body) = create_version(
            &router,
            &workflow_id,
            &json!({ "versionName": name, "requestId": format!("clock-list-{name}") }),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {body:?}");
    }

    // The transient status is observable through the listing as well, so the span the
    // clock owes is never zero.
    let (status, body) = list_versions(&router, &workflow_id, "").await;
    let page = items("before the clock", status, &body);
    for item in &page {
        assert_eq!(
            item.get("status").and_then(Value::as_str),
            Some("CREATING"),
            "a version listed before its transition is due must still be CREATING, \
             got {item:?}"
        );
    }

    for name in ["v1", "v2"] {
        await_version_status(&router, &workflow_id, name, "ACTIVE").await;
    }

    let (status, body) = list_versions(&router, &workflow_id, "").await;
    let settled = items("after the clock", status, &body);
    for item in &settled {
        assert_eq!(
            item.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "a settled version must be listed as ACTIVE, got {item:?}"
        );
    }

    // Listing again reports exactly the same page: the read moved nothing on and
    // restamped nothing, so a later caller sees what this one did.
    let (status, body) = list_versions(&router, &workflow_id, "").await;
    let again = items("repeated", status, &body);
    assert_eq!(
        again, settled,
        "a repeated listing must report the same page, got {again:?}"
    );

    // And the single-version read agrees with it, instant for instant.
    for item in &settled {
        let name = item
            .get("versionName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (status, read) = get_version(&router, &workflow_id, name).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {read:?}");
        assert_eq!(
            item.get("creationTime"),
            read.get("creationTime"),
            "case {name:?}: expected the same creationTime the read reports, got {item:?}"
        );
        assert_eq!(
            item.get("status"),
            read.get("status"),
            "case {name:?}: expected the same status the read reports, got {item:?}"
        );
    }
}
