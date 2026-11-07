// SPDX-License-Identifier: CC0-1.0

//! `CreateWorkspaceServiceAccount`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: a create mints exactly one service account on the
//! workspace the request names, stores it there with the name and role the request
//! carried and an id unique within that workspace, and reports the record it stored;
//! a request that names no such workspace, a workspace Grafana version below the
//! documented floor, or a name that workspace already holds stores nothing and is
//! refused with the modelled error.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::{
    service_state, ServiceAccount, LIFECYCLE_TRANSITION, SERVICE_ACCOUNT_MIN_GRAFANA_MAJOR,
    SERVICE_ACCOUNT_QUOTA,
};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Every value of `Role`, the shape `CreateWorkspaceServiceAccountRequest$grafanaRole`
/// targets. Source: `smithy_query(query="shape", shape="Role")` — `values: ["ADMIN",
/// "EDITOR", "VIEWER"]`. Written out here so a role the model admits but the handler
/// drops fails this test.
const EVERY_ROLE: [&str; 3] = ["ADMIN", "EDITOR", "VIEWER"];

/// `ServiceAccountName`'s published bounds. Source: `smithy_query(query="shape",
/// shape="ServiceAccountName")` — `length: {min: 1, max: 128}`.
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 128;

/// The emulator under test: the router a client talks to, beside the one shared engine
/// its handlers write through, so a test can read the state back as stored.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A workspace to hang service accounts off, on `version` when one is named.
async fn workspace_on(emulator: &Emulator, version: Option<&str>) -> String {
    let mut request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });
    if let Some(version) = version {
        request["grafanaVersion"] = json!(version);
    }
    let created = call(&emulator.router, "POST", "/workspaces", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "the workspace this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    created.body["workspace"]["id"]
        .as_str()
        .expect("CreateWorkspace answers an id")
        .to_string()
}

async fn workspace(emulator: &Emulator) -> String {
    workspace_on(emulator, None).await
}

async fn create_account(
    emulator: &Emulator,
    workspace_id: &str,
    name: &str,
    role: &str,
) -> Answer {
    call(
        &emulator.router,
        "POST",
        &format!("/workspaces/{workspace_id}/serviceaccounts"),
        Some(json!({"name": name, "grafanaRole": role})),
    )
    .await
}

/// The service accounts the shared store holds for `workspace_id`, as persisted.
fn stored_accounts(emulator: &Emulator, workspace_id: &str) -> Vec<ServiceAccount> {
    let state = service_state(&*emulator.engine).expect("the grafana store is reachable");
    let store = state.lock();
    store
        .workspaces
        .get(workspace_id)
        .map(|workspace| workspace.service_accounts.clone())
        .unwrap_or_default()
}

/// A create reports the account it stored, and the store holds it afterwards.
///
/// The rows differ along what the rule generalises over: the role (every value `Role`
/// admits) and the form and size of the name (one character, the maximum length,
/// punctuation, spaces, non-ASCII).
#[tokio::test]
async fn a_create_stores_the_account_it_reports() {
    let max_length_name = "v".repeat(NAME_MAX_LEN);
    let cases: [(&str, &str, &str); 6] = [
        ("single-character name, ADMIN", "a", "ADMIN"),
        ("dotted name, EDITOR", "ops.automation", "EDITOR"),
        ("name at the maximum length, VIEWER", &max_length_name, "VIEWER"),
        ("name carrying spaces, ADMIN", "nightly reporting job", "ADMIN"),
        ("non-ASCII name, EDITOR", "обзор-сервис", "EDITOR"),
        ("name carrying digits, VIEWER", "dashboards-2026", "VIEWER"),
    ];

    let emulator = emulator();
    let workspace_id = workspace(&emulator).await;
    let mut minted = Vec::new();

    for (case, name, role) in cases {
        let created = create_account(&emulator, &workspace_id, name, role).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.string("name"),
            name,
            "case {case:?}: the answer must report the stored name, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("grafanaRole"),
            role,
            "case {case:?}: the answer must report the stored role, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("workspaceId"),
            workspace_id,
            "case {case:?}: the answer must report the workspace it stored into, got {:?}",
            created.body
        );

        let id = created.string("id");
        assert!(
            !id.is_empty(),
            "case {case:?}: a create must mint an id, got {:?}",
            created.body
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: id {id:?} was already minted in this workspace: {minted:?}"
        );
        minted.push(id.clone());

        // Read back: the store holds exactly the record the answer described.
        let accounts = stored_accounts(&emulator, &workspace_id);
        let stored = accounts
            .iter()
            .find(|account| account.id == id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created account {id:?} is not in the store: {accounts:?}")
            });
        assert_eq!(
            (stored.name.as_str(), stored.grafana_role.as_str()),
            (name, role),
            "case {case:?}: the stored record must carry the created name and role, got {stored:?}"
        );
        assert!(
            !stored.disabled,
            "case {case:?}: a new service account is enabled, got {stored:?}"
        );
        assert_eq!(
            stored.to_summary().is_disabled,
            "false",
            "case {case:?}: isDisabled renders the boolean, got {:?}",
            stored.to_summary()
        );
    }

    let accounts = stored_accounts(&emulator, &workspace_id);
    assert_eq!(
        accounts.len(),
        cases.len(),
        "each create stores exactly one account, got {accounts:?}"
    );
}

/// A name is unique within a workspace — a repeat is refused and stores nothing — but
/// two workspaces may each hold an account under the same name.
#[tokio::test]
async fn a_name_is_unique_within_one_workspace_only() {
    let emulator = emulator();
    let first = workspace(&emulator).await;
    let second = workspace(&emulator).await;

    for (case, name, role) in [
        ("short name", "automation", "ADMIN"),
        ("name carrying punctuation", "ci/cd.runner", "VIEWER"),
    ] {
        let created = create_account(&emulator, &first, name, role).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let id = created.string("id");

        // The same name again, in the same workspace, under either role.
        for repeated_role in EVERY_ROLE {
            let refused = create_account(&emulator, &first, name, repeated_role).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: a repeated name as {repeated_role} must be refused, got {} \
                 with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.body["resourceId"].as_str().unwrap_or_default(),
                id,
                "case {case:?}: the conflict names the account already holding the name, \
                 got {:?}",
                refused.body
            );
            assert_eq!(
                refused.body["resourceType"].as_str().unwrap_or_default(),
                "ServiceAccount",
                "case {case:?}: the conflict is about a service account, got {:?}",
                refused.body
            );
        }

        let held = stored_accounts(&emulator, &first);
        assert_eq!(
            held.iter().filter(|account| account.name == name).count(),
            1,
            "case {case:?}: a refused create must store nothing, got {held:?}"
        );

        // The other workspace is a different namespace for the same name.
        let elsewhere = create_account(&emulator, &second, name, role).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case:?}: the same name in another workspace must be accepted, got {} \
             with {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.string("workspaceId"),
            second,
            "case {case:?}: the account belongs to the workspace it was created in, got {:?}",
            elsewhere.body
        );
    }
}

/// `CreateWorkspaceServiceAccount` documents the version floor: "You can only create
/// service accounts for workspaces that are compatible with Grafana version 9 and
/// above." The rows cover the bound itself, one below it, and versions either side.
#[tokio::test]
async fn only_a_workspace_on_grafana_nine_or_above_holds_service_accounts() {
    assert_eq!(
        SERVICE_ACCOUNT_MIN_GRAFANA_MAJOR, 9,
        "the documented floor is Grafana version 9"
    );

    // (case, the workspace's grafanaVersion, whether a create is accepted)
    let cases: [(&str, Option<&str>, bool); 9] = [
        ("one major below the floor", Some("8.4"), false),
        ("the major below, no minor", Some("8"), false),
        ("far below the floor", Some("6.7"), false),
        // A version no numeric major can be read off is below the floor, not above it.
        ("a version with no numeric major", Some("latest"), false),
        ("the floor itself", Some("9"), true),
        ("the floor with a minor", Some("9.4"), true),
        ("above the floor", Some("10.4"), true),
        ("far above the floor", Some("11.0"), true),
        ("whatever the service defaults to", None, true),
    ];

    let emulator = emulator();
    for (case, version, accepted) in cases {
        let workspace_id = workspace_on(&emulator, version).await;
        let answer = create_account(&emulator, &workspace_id, "automation", "ADMIN").await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?} (version {version:?}): expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                stored_accounts(&emulator, &workspace_id).len(),
                1,
                "case {case:?}: the accepted create must be stored"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?} (version {version:?}): expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert!(
                stored_accounts(&emulator, &workspace_id).is_empty(),
                "case {case:?}: a refused create must store nothing, got {:?}",
                stored_accounts(&emulator, &workspace_id)
            );
        }
    }
}

/// A `workspaceId` that names no workspace is a request against a resource that does
/// not exist — including the id of a workspace that has been deleted, which releases
/// the accounts it held.
#[tokio::test]
async fn a_create_against_no_workspace_is_refused() {
    let emulator = emulator();
    let live = workspace(&emulator).await;
    let deleted = workspace(&emulator).await;
    assert_eq!(
        create_account(&emulator, &deleted, "automation", "ADMIN")
            .await
            .status,
        200,
        "the workspace about to be deleted must accept a create first"
    );
    assert_eq!(
        call(&emulator.router, "DELETE", &format!("/workspaces/{deleted}"), None)
            .await
            .status,
        202,
        "DeleteWorkspace must accept the delete this case needs"
    );
    // The delete is ACCEPTED, not finished: the workspace is removed from the store once
    // the removal the clock owes it falls due, and only then does its id name nothing.
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;

    // (case, the workspaceId, expected status, expected wire error code)
    let cases: [(&str, &str, u16, &str); 5] = [
        ("an id no workspace carries", "g-0123456789", 404, "ResourceNotFoundException"),
        ("the id of a deleted workspace", &deleted, 404, "ResourceNotFoundException"),
        // `WorkspaceId` is `@pattern("^g-[0-9a-f]{10}$")`, so these never name a
        // workspace at all and are refused at the boundary.
        ("an id with no g- prefix", "0123456789ab", 400, "ValidationException"),
        ("an id that is too short", "g-0123456", 400, "ValidationException"),
        ("an id carrying non-hex digits", "g-0123456xyz", 400, "ValidationException"),
    ];

    for (case, workspace_id, status, code) in cases {
        let answer = create_account(&emulator, workspace_id, "automation", "ADMIN").await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for {workspace_id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }

    assert!(
        stored_accounts(&emulator, &live).is_empty(),
        "no refused create may land in another workspace, got {:?}",
        stored_accounts(&emulator, &live)
    );
}

/// The modelled constraints on the request: `name`'s length bounds and `grafanaRole`'s
/// value set. Both bounds are checked, and one character past each.
///
/// Both members travel in the request BODY, so a value outside their modelled bounds is
/// refused by the service's request validator as a bad request — not with the modelled
/// `ValidationException`, which is what a refusal the HANDLER raises carries.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let emulator = emulator();
    let workspace_id = workspace(&emulator).await;

    // (case, name, role, expected status)
    let cases: [(&str, String, &str, u16); 7] = [
        ("one character below the minimum length", "a".repeat(NAME_MIN_LEN - 1), "ADMIN", 400),
        ("the minimum length", "a".repeat(NAME_MIN_LEN), "ADMIN", 200),
        ("the maximum length", "b".repeat(NAME_MAX_LEN), "EDITOR", 200),
        ("one character past the maximum length", "c".repeat(NAME_MAX_LEN + 1), "VIEWER", 400),
        ("a role the model does not admit", "role-case-1".to_string(), "SUPERUSER", 400),
        ("a modelled role in the wrong case", "role-case-2".to_string(), "admin", 400),
        ("an empty role", "role-case-3".to_string(), "", 400),
    ];

    for (case, name, role, status) in cases {
        let answer = create_account(&emulator, &workspace_id, &name, role).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException from the body validator, got \
                 {:?}",
                answer.body
            );
            assert!(
                stored_accounts(&emulator, &workspace_id)
                    .iter()
                    .all(|account| account.name != name),
                "case {case:?}: a refused create must store nothing under {name:?}"
            );
        }
    }
}

/// The store is bounded: past the quota a create is refused rather than adding another
/// record, and the refusal is the modelled `ServiceQuotaExceededException`.
#[tokio::test]
async fn a_workspace_holds_a_bounded_number_of_service_accounts() {
    let emulator = emulator();
    let workspace_id = workspace(&emulator).await;

    for index in 0..SERVICE_ACCOUNT_QUOTA {
        let answer = create_account(&emulator, &workspace_id, &format!("sa-{index}"), "VIEWER").await;
        assert_eq!(
            answer.status, 200,
            "create {index} of {SERVICE_ACCOUNT_QUOTA} must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused = create_account(&emulator, &workspace_id, "one-too-many", "VIEWER").await;
    assert_eq!(
        refused.status, 402,
        "a create past the quota must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        refused.body
    );
    assert_eq!(
        stored_accounts(&emulator, &workspace_id).len(),
        SERVICE_ACCOUNT_QUOTA,
        "the refused create must store nothing"
    );

    // Another workspace is bounded on its own, not by this one.
    let other = workspace(&emulator).await;
    assert_eq!(
        create_account(&emulator, &other, "one-too-many", "VIEWER")
            .await
            .status,
        200,
        "the quota is per workspace, so another workspace still accepts a create"
    );
}
