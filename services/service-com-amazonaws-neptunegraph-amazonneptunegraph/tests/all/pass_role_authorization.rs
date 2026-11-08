// SPDX-License-Identifier: CC0-1.0

//! The pass-role authorization boundary of this service, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! # The rule
//!
//! Three operations take a `roleArn` the service has to ASSUME on the caller's behalf —
//! `StartImportTask`, `CreateGraphUsingImportTask` and `StartExportTask`
//! (`smithy_query(query="operation",
//! operation="StartImportTask,CreateGraphUsingImportTask,StartExportTask")`: each
//! declares `roleArn` `@required`). Whether the caller may hand that role over is an
//! authorization decision about the caller and that one member, and it is taken BEFORE
//! anything else about the request is checked, so:
//!
//! * a role of the caller's own account is passable, and the call proceeds;
//! * a role of ANOTHER account is refused `AccessDeniedException` (HTTP 403,
//!   `smithy_query(query="shape",shape="AccessDeniedException")`), because the service
//!   can establish no trust relationship with it;
//! * a value that names no IAM role at all is refused with the same 403, naming the
//!   principal that made the call rather than the shape the value broke — WHICH principal
//!   that is is `tests/all/pass_role_denial_principal.rs`;
//! * either refusal holds whatever ELSE the request breaks — a member outside its
//!   modelled pattern does not turn the denial into a 400 — and leaves the store
//!   untouched, so no task and no graph comes into existence.
//!
//! # What the tables span
//!
//! The role's account and partition; the path form of a passable role (`role/…`,
//! `role/service-role/…`, a multi-segment path); the ways a value can name no role (a
//! bare word, an empty string, another IAM resource type, another service's ARN, a
//! Region where IAM has none, a short account, a role with no name); which OTHER member
//! of the request is also broken; which access key signed the call; and, for every row,
//! each of the three operations that carries the member.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The access key id most of these calls are signed with. No store of this emulator
/// holds it, which is the position the long-term credential the emulator publishes is in.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The sentence AWS refuses a role of another account with.
const CROSS_ACCOUNT_DENIAL: &str = "Cross-account pass role is not allowed.";

/// An IAM role ARN of `account`, in `partition`, under the resource path `path`.
///
/// Spelled from the parts the modelled `RoleArn` pattern fixes
/// (`^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`), so a row
/// differs from its neighbours only in the part it is about.
fn role_arn(partition: &str, account: &str, path: &str) -> String {
    format!("arn:{partition}:iam::{account}:{path}")
}

/// A role ARN of the one account this emulator serves, under the resource path `path`.
fn own_role_arn(path: &str) -> String {
    role_arn("aws", engine::emulator_account_id(), path)
}

/// The IAM principal a denial names for every call this suite makes: the identity the
/// credential resolves to, which for a key no store of this emulator holds is the account's
/// own published principal — the one `sts:GetCallerIdentity` answers with. This test binary
/// mounts this service alone, so every key it signs with is such a key, and the expectation
/// does not vary with the key; `tests/all/pass_role_denial_principal.rs` drives the keys that
/// resolve to a user or a session, against STS itself.
fn expected_principal() -> String {
    service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ambient_identity().arn
}

/// The sentence AWS refuses a value that names no role the caller may pass with.
fn not_authorized_denial() -> String {
    format!(
        "User: {} is not authorized to perform: null",
        expected_principal()
    )
}

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request signed with `access_key_id`, and return its status
/// and parsed JSON body.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Dispatch one request signed with [`ACCESS_KEY_ID`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_as(router, ACCESS_KEY_ID, method, path, body).await
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({ "graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case graph {name}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case graph {name}: no id in {created}"))
        .to_string();
    wait_until_available(router, &id).await;
    id
}

/// Poll a graph until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, graph_id: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            status, 200,
            "case graph {graph_id}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case graph {graph_id}: never became AVAILABLE");
}

/// How many entries a listing reports under `member`.
async fn listed(router: &MultiServiceRouter, path: &str, member: &str) -> usize {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {path}: expected 200 from the listing, got {status}: {body}"
    );
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {path}: no {member} list in {body}"))
        .len()
}

/// What the store holds: the graph, import-task and export-task counts, which an
/// authorization failure must leave exactly as it found them.
async fn stored_counts(router: &MultiServiceRouter) -> (usize, usize, usize) {
    (
        listed(router, "/graphs", "graphs").await,
        listed(router, "/importtasks", "tasks").await,
        listed(router, "/exporttasks", "tasks").await,
    )
}

/// One of the three operations that asks this service to assume a role.
#[derive(Clone, Copy)]
enum PassRoleOperation {
    StartImport,
    CreateGraphUsingImport,
    StartExport,
}

/// Printed as the operation's modelled name, which is what a failing case has to name.
impl std::fmt::Debug for PassRoleOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StartImport => "StartImportTask",
            Self::CreateGraphUsingImport => "CreateGraphUsingImportTask",
            Self::StartExport => "StartExportTask",
        })
    }
}

impl PassRoleOperation {
    /// Every operation the rule covers.
    const ALL: [Self; 3] = [
        Self::StartImport,
        Self::CreateGraphUsingImport,
        Self::StartExport,
    ];

    /// A request this operation accepts in every respect but the role it names.
    ///
    /// `graph_name` is used only by `CreateGraphUsingImportTask`, which mints its own
    /// graph; the other two load from, or export, the graph `graph_id` names.
    fn request(self, graph_id: &str, graph_name: &str, role_arn: &str) -> (String, Value) {
        match self {
            Self::StartImport => (
                format!("/graphs/{graph_id}/importtasks"),
                json!({ "source": "s3://import-bucket/data/", "roleArn": role_arn }),
            ),
            Self::CreateGraphUsingImport => (
                "/importtasks".to_string(),
                json!({
                    "graphName": graph_name,
                    "source": "s3://import-bucket/data/",
                    "roleArn": role_arn,
                }),
            ),
            Self::StartExport => (
                "/exporttasks".to_string(),
                json!({
                    "graphIdentifier": graph_id,
                    "roleArn": role_arn,
                    "format": "CSV",
                    "destination": "s3://export-bucket/dump/",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                }),
            ),
        }
    }

    /// The same request with ONE OTHER member put outside its modelled constraint — the
    /// kind of value the boundary's own walk refuses before any handler runs, so a row
    /// built from this is what shows which decision is reached first.
    fn request_breaking_another_member(self, graph_id: &str, role_arn: &str) -> (String, Value) {
        match self {
            // `graphIdentifier` is an `@httpLabel` carrying the modelled
            // `^g-[a-z0-9]{10}$` pattern.
            Self::StartImport => (
                "/graphs/not-a-graph-id/importtasks".to_string(),
                json!({ "source": "s3://import-bucket/data/", "roleArn": role_arn }),
            ),
            // `graphName` carries `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
            Self::CreateGraphUsingImport => (
                "/importtasks".to_string(),
                json!({
                    "graphName": "Not A Graph Name",
                    "source": "s3://import-bucket/data/",
                    "roleArn": role_arn,
                }),
            ),
            // `kmsKeyIdentifier` carries the modelled `KmsKeyArn` pattern.
            Self::StartExport => (
                "/exporttasks".to_string(),
                json!({
                    "graphIdentifier": graph_id,
                    "roleArn": role_arn,
                    "format": "CSV",
                    "destination": "s3://export-bucket/dump/",
                    "kmsKeyIdentifier": "not-a-kms-key",
                }),
            ),
        }
    }

    /// The listing a task this operation mints would appear in, and the member that
    /// carries it.
    fn listing(self) -> (&'static str, &'static str) {
        match self {
            Self::StartExport => ("/exporttasks", "tasks"),
            Self::StartImport | Self::CreateGraphUsingImport => ("/importtasks", "tasks"),
        }
    }

    /// The read that answers a task this operation minted, by the id it reported.
    fn read_path(self, task_id: &str) -> String {
        match self {
            Self::StartExport => format!("/exporttasks/{task_id}"),
            Self::StartImport | Self::CreateGraphUsingImport => {
                format!("/importtasks/{task_id}")
            }
        }
    }
}

/// A role of the caller's own account is passable, whichever of the published resource
/// paths it carries and whichever operation is handed it: the call is accepted and the
/// role it named is the role the stored task reports back.
#[tokio::test]
async fn a_role_of_the_callers_own_account_is_passable() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "pass-role-target").await;

    // Every resource path the modelled `RoleArn` pattern admits: `role` plus one path
    // segment, the `role/service-role` form it names explicitly, and a longer path,
    // which `(/[\w+=,.@-]+)+` also admits.
    let paths = [
        "role/NeptuneTaskRole",
        "role/service-role/NeptuneTaskRole",
        "role/team/analytics/NeptuneTaskRole",
        "role/neptune_task+role=1,2.3@x-y",
    ];
    for (row, path) in paths.iter().enumerate() {
        for operation in PassRoleOperation::ALL {
            let role = own_role_arn(path);
            let case = format!("{operation:?} passing {role}");
            wait_until_available(&router, &graph_id).await;
            let (request_path, body) = operation.request(
                &graph_id,
                &format!("passable-{row}-{}", PassRoleOperation::ALL.len()),
                &role,
            );
            let (status, answered) = call(&router, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201, got {status}: {answered}"
            );
            assert_eq!(
                answered["roleArn"],
                json!(role),
                "case {case:?}: expected the request's role to be reported, got {}",
                answered["roleArn"]
            );

            // The record is readable, so the accepted call really did mint a task
            // rather than answer one.
            let task_id = answered["taskId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no taskId in {answered}"));
            let (read_status, read) =
                call(&router, "GET", &operation.read_path(task_id), None).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected 200 reading the minted task, got {read_status}: {read}"
            );
            assert_eq!(
                read["roleArn"],
                json!(role),
                "case {case:?}: expected the stored task to carry the request's role, got {}",
                read["roleArn"]
            );
        }
    }
}

/// A role of another account is refused 403 with AWS's cross-account sentence, by every
/// operation that takes one, and nothing it would have created exists afterwards.
#[tokio::test]
async fn a_role_of_another_account_is_denied_and_stores_nothing() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "cross-account-target").await;
    let before = stored_counts(&router).await;

    // The account is what the decision turns on, so the rows differ in the account they
    // name, in the partition that account sits in, and in the role's resource path.
    let roles = [
        role_arn("aws", "111111111111", "role/foreign-task-role"),
        role_arn("aws", "123456789012", "role/service-role/foreign-task-role"),
        role_arn("aws", "999999999999", "role/team/foreign-task-role"),
        role_arn("aws-cn", "111111111111", "role/foreign-task-role"),
        role_arn("aws-us-gov", "210987654321", "role/foreign-task-role"),
    ];
    for role in &roles {
        for operation in PassRoleOperation::ALL {
            let case = format!("{operation:?} passing {role}");
            let (request_path, body) = operation.request(&graph_id, "cross-account-graph", role);
            let (status, answered) = call(&router, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {answered}"
            );
            assert_eq!(
                answered["message"],
                json!(CROSS_ACCOUNT_DENIAL),
                "case {case:?}: expected {CROSS_ACCOUNT_DENIAL:?}, got {}",
                answered["message"]
            );
        }
    }

    assert_eq!(
        stored_counts(&router).await,
        before,
        "case a denied pass-role call: the store must hold exactly what it held before, \
         (graphs, import tasks, export tasks)"
    );
}

/// A value that names no IAM role is refused with the same 403, and the sentence names
/// the principal that made the call — read from the request's own credential, so two
/// callers are named differently.
#[tokio::test]
async fn a_value_naming_no_iam_role_is_denied_and_names_the_caller() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "no-role-target").await;
    let before = stored_counts(&router).await;

    // One row per way a value can fail to name a role the caller may pass, spanning the
    // parts the modelled `RoleArn` pattern fixes: it must be an ARN at all, of an `aws`
    // partition, of the `iam` service with no Region (IAM is global), of a twelve-digit
    // account, and of a `role/` resource carrying at least one path segment.
    let values = [
        ("a bare word", "not-an-arn".to_string()),
        ("an empty value", String::new()),
        ("an ARN of another service", "arn:aws:s3:::import-bucket".to_string()),
        ("another IAM resource type", own_role_arn("user/someone")),
        ("an instance profile", own_role_arn("instance-profile/NeptuneTaskRole")),
        (
            "a Region where IAM has none",
            format!(
                "arn:aws:iam:{REGION}:{}:role/NeptuneTaskRole",
                engine::emulator_account_id()
            ),
        ),
        ("an account that is not twelve digits", role_arn("aws", "12345", "role/NeptuneTaskRole")),
        ("a partition that is not an AWS one", role_arn("azure", engine::emulator_account_id(), "role/NeptuneTaskRole")),
        ("a role with no name", own_role_arn("role")),
        ("a role whose name is empty", own_role_arn("role/")),
        ("a role name outside the modelled character set", own_role_arn("role/Neptune Task Role")),
    ];
    for (why, value) in &values {
        for operation in PassRoleOperation::ALL {
            let case = format!("{operation:?} passing {why} ({value:?})");
            let (request_path, body) = operation.request(&graph_id, "no-role-graph", value);
            let (status, answered) = call(&router, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {answered}"
            );
            assert_eq!(
                answered["message"],
                json!(not_authorized_denial()),
                "case {case:?}: expected {:?}, got {}",
                not_authorized_denial(),
                answered["message"]
            );
        }
    }

    // Every caller of this suite signs with a key no store holds, so each is the
    // account's own published principal; a key that resolves to a user or a session is
    // driven by `tests/all/pass_role_denial_principal.rs`.
    for access_key_id in ["AKIDSECONDCALLER", "AKIDTHIRDCALLER"] {
        for operation in PassRoleOperation::ALL {
            let case = format!("{operation:?} signed by {access_key_id}");
            let (request_path, body) = operation.request(&graph_id, "no-role-graph", "not-an-arn");
            let (status, answered) =
                call_as(&router, access_key_id, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {answered}"
            );
            assert_eq!(
                answered["message"],
                json!(not_authorized_denial()),
                "case {case:?}: expected {:?}, got {}",
                not_authorized_denial(),
                answered["message"]
            );
        }
    }

    assert_eq!(
        stored_counts(&router).await,
        before,
        "case a denied pass-role call: the store must hold exactly what it held before, \
         (graphs, import tasks, export tasks)"
    );
}

/// The pass-role decision is reached BEFORE every other check: a request that also
/// breaks another member's modelled constraint is still the 403 the role earns, while
/// the same request with a passable role is the 400 that member earns.
#[tokio::test]
async fn the_pass_role_decision_is_reached_before_the_other_checks() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "ordering-target").await;

    // Rows: the two classes of unpassable role, each against a request whose OTHER
    // member is outside its modelled pattern, and the passable role that shows the same
    // request is otherwise a constraint failure.
    let rows: [(&str, String, u16, &str); 3] = [
        (
            "a role of another account",
            role_arn("aws", "111111111111", "role/foreign-task-role"),
            403,
            "AccessDeniedException",
        ),
        (
            "a value naming no role",
            "not-an-arn".to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "a passable role",
            own_role_arn("role/NeptuneTaskRole"),
            400,
            "ValidationException",
        ),
    ];
    for (why, role, expected_status, expected_code) in &rows {
        for operation in PassRoleOperation::ALL {
            let case = format!("{operation:?} with a broken member and {why}");
            let (request_path, body) = operation.request_breaking_another_member(&graph_id, role);
            let (status, answered) = call(&router, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?}: expected {expected_status}, got {status}: {answered}"
            );
            assert_eq!(
                error_code(&answered),
                *expected_code,
                "case {case:?}: expected {expected_code}, got {answered}"
            );
        }
    }

    // A graph the request names but the store does not hold is a 404 for a passable
    // role, and still the role's 403 for one the caller may not pass: the authorization
    // decision is settled before the resource is resolved too.
    let absent_graph = "g-0000000000";
    for (why, role, expected_status, expected_code) in [
        (
            "a passable role",
            own_role_arn("role/NeptuneTaskRole"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a role of another account",
            role_arn("aws", "111111111111", "role/foreign-task-role"),
            403,
            "AccessDeniedException",
        ),
        (
            "a value naming no role",
            "not-an-arn".to_string(),
            403,
            "AccessDeniedException",
        ),
    ] {
        for operation in [
            PassRoleOperation::StartImport,
            PassRoleOperation::StartExport,
        ] {
            let case = format!("{operation:?} on an absent graph with {why}");
            let (request_path, body) = operation.request(absent_graph, "absent-graph", &role);
            let (status, answered) = call(&router, "POST", &request_path, Some(body)).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status}: {answered}"
            );
            assert_eq!(
                error_code(&answered),
                expected_code,
                "case {case:?}: expected {expected_code}, got {answered}"
            );
        }
    }

    // Nothing above minted a task, whichever way it was refused.
    for operation in PassRoleOperation::ALL {
        let (path, member) = operation.listing();
        assert_eq!(
            listed(&router, path, member).await,
            0,
            "case {operation:?}: a refused call must leave {path} empty"
        );
    }
}
