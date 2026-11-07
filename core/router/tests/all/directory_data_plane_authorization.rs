// SPDX-License-Identifier: CC0-1.0

//! The DIRECTORY a Directory Service Data request names, across the two services that
//! share it.
//!
//! `ds:CreateMicrosoftAD` creates a directory and `ds:DeleteDirectory` removes it; the
//! Directory Service DATA plane (`ds-data`) creates none — its whole surface is the users,
//! groups and memberships INSIDE a directory. So whether a `DirectoryId` a data-plane
//! request carries is one this account holds is knowable there only because the control
//! plane recorded it, and the two services are separate crates that can only agree because
//! both reach the one shared inventory.
//!
//! The rule these rows pin: a data-plane request is served for a directory the control
//! plane HOLDS — the one `ds:DescribeDirectories` reports — and whose DS Data feature that
//! plane has turned ON (`ds:EnableDirectoryDataAccess`), and refused for every other
//! well-formed `DirectoryId` with `AccessDeniedException` (HTTP 403, `Reason` `IAM_AUTH`),
//! which discloses nothing about the directory. The lifecycle follows the control plane's:
//! a directory it has deleted stops being served, whatever the data plane stored while it
//! existed.
//!
//! The DISABLE direction of that switch — and the identity of the refusal a read, a write
//! and a listing get behind it — is the sibling suite `directory_data_access_cycle`.
//!
//! Only this configuration — the Directory Service control plane and its data plane mounted
//! on ONE engine, which is what `core/router` ships — makes the join observable at all.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The two signing names this suite signs for, which is how the router picks the service
/// that serves each request (`smithy_query(query="service_info")`: `ds` and `ds-data`).
const DS: &str = "ds";
const DS_DATA: &str = "ds-data";

/// The `X-Amz-Target` prefix the control plane's `awsJson1_1` protocol names an operation
/// under: its service shape's name.
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";

/// The error the data plane refuses an unreachable directory with, and the modelled
/// `AccessDeniedReason` it carries.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const IAM_AUTH: &str = "IAM_AUTH";

/// Well-formed `DirectoryId`s (`^d-[0-9a-f]{10}$`) that no control-plane create in this
/// suite ever mints, varied along their digits.
const UNCREATED_DIRECTORIES: [&str; 3] = ["d-0000000000", "d-ffffffffff", "d-5e9207cba4"];

/// Past one of the control plane's transition spans: a create answers before the directory
/// is in service and the data-access toggle answers before the feature has moved, and the
/// clock — not a later call — settles both.
pub(super) const PAST_ONE_SPAN: Duration = Duration::from_millis(700);

type DirectoryService =
    service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService<
        engine::InMemoryEngine,
    >;
type DirectoryServiceData =
    service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService<
        engine::InMemoryEngine,
    >;

/// One answer, reduced to what the rows assert on.
#[derive(Debug)]
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(super) fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A string member of the answer, or the empty string when it carries none.
    pub(super) fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Both services, over ONE shared engine exactly as `router/src/lib.rs` mounts them.
pub(super) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(super) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(DirectoryService::new(Arc::clone(&engine)));
        router.register_service(DirectoryServiceData::new(engine));
        Self { router }
    }

    /// One control-plane call: `POST /` with the operation in `X-Amz-Target`.
    pub(super) async fn control_plane(&self, operation: &str, body: Value) -> Answer {
        self.dispatch(
            DS,
            "/",
            Some(format!("{DS_TARGET_PREFIX}.{operation}")),
            Some(body),
        )
        .await
    }

    /// One data-plane call: `POST <route>?DirectoryId=<id>` with a JSON body.
    pub(super) async fn data_plane(&self, route: &str, directory: &str, body: Value) -> Answer {
        self.dispatch(
            DS_DATA,
            &format!("{route}?DirectoryId={directory}"),
            None,
            Some(body),
        )
        .await
    }

    async fn dispatch(
        &self,
        signing_name: &str,
        uri: &str,
        target: Option<String>,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/json");
        if let Some(target) = target {
            request = request.header("x-amz-target", target);
        }
        let response = self
            .router
            .dispatch(request.body(payload).expect("a well-formed request"))
            .await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }
}

/// A minimal well-formed `CreateMicrosoftAD` body: the three members the operation
/// requires, with one directory server per subnet.
pub(super) fn create_microsoft_ad(name: &str) -> Value {
    json!({
        "Name": name,
        "Password": "P@ssw0rd-emulator-1",
        "VpcSettings": {
            "VpcId": "vpc-0a1b2c3d",
            "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        },
    })
}

/// Every `DirectoryId` the control plane reports holding.
async fn held_directories(emulator: &Emulator) -> Vec<String> {
    let listed = emulator
        .control_plane("DescribeDirectories", json!({}))
        .await;
    listed.body["DirectoryDescriptions"]
        .as_array()
        .map(|descriptions| {
            descriptions
                .iter()
                .filter_map(|description| description["DirectoryId"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The data plane serves the directories the control plane holds and refuses every other
/// well-formed `DirectoryId`, and a directory the control plane deletes stops being served.
#[tokio::test]
async fn the_data_plane_serves_the_directories_the_control_plane_holds() {
    let emulator = Emulator::new();

    // Two directories of this account, so no row can pass by serving "the only one".
    let mut created: Vec<String> = Vec::new();
    for name in ["corp.example.com", "sales.example.org"] {
        let answer = emulator
            .control_plane("CreateMicrosoftAD", create_microsoft_ad(name))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: CreateMicrosoftAD must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        created.push(answer.text("DirectoryId"));
    }

    // The control plane's OWN read reports exactly those directories: the data plane's
    // answers below are about directories that really exist in the plane that owns them.
    let held = held_directories(&emulator).await;
    assert_eq!(
        held, created,
        "expected DescribeDirectories to report the created directories, got {held:?}"
    );

    // The data plane answers for a directory only while the control plane's DS Data feature
    // is on for it, and a create leaves it off: the stage transition each create owes has to
    // fall due before the enable is admitted, and the enable's own transition before the
    // data plane serves anything.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for directory in &created {
        let enabled = emulator
            .control_plane(
                "EnableDirectoryDataAccess",
                json!({"DirectoryId": directory}),
            )
            .await;
        assert_eq!(
            enabled.status, 200,
            "directory {directory:?}: EnableDirectoryDataAccess must be accepted, got {} \
             with {:?}",
            enabled.status, enabled.body
        );
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Each held directory serves the data plane's own lifecycle: a create, then a read of
    // that same record.
    for directory in &created {
        let created_user = emulator
            .data_plane(
                "/Users/CreateUser",
                directory,
                json!({"SAMAccountName": "jdoe", "GivenName": "Jane"}),
            )
            .await;
        assert_eq!(
            created_user.status, 200,
            "directory {directory:?}: CreateUser must be served, got {} with {:?}",
            created_user.status, created_user.body
        );
        let described = emulator
            .data_plane(
                "/Users/DescribeUser",
                directory,
                json!({"SAMAccountName": "jdoe"}),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "directory {directory:?}: DescribeUser must report the stored user, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("SID"),
            created_user.text("SID"),
            "directory {directory:?}: the describe must answer the created record, got {:?}",
            described.body
        );
    }

    // A well-formed id no create minted is refused, and the refusal says nothing about it.
    for directory in UNCREATED_DIRECTORIES {
        for (route, body) in [
            ("/Users/ListUsers", json!({})),
            ("/Users/CreateUser", json!({"SAMAccountName": "jdoe"})),
            (
                "/Users/DescribeUser",
                json!({"SAMAccountName": "jdoe"}),
            ),
        ] {
            let answer = emulator.data_plane(route, directory, body).await;
            assert_eq!(
                answer.status, 403,
                "case {route} on {directory}: expected HTTP 403, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                ACCESS_DENIED_EXCEPTION,
                "case {route} on {directory}: expected {ACCESS_DENIED_EXCEPTION}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body.get("Reason").and_then(Value::as_str),
                Some(IAM_AUTH),
                "case {route} on {directory}: expected Reason {IAM_AUTH}, got {:?}",
                answer.body
            );
            assert!(
                !answer.body.to_string().contains(directory),
                "case {route} on {directory}: the refusal must not name the directory, got {:?}",
                answer.body
            );
        }
    }

    // The lifecycle follows the control plane: the deleted directory is refused, the other
    // one is still served.
    let deleted = created.remove(0);
    let delete = emulator
        .control_plane("DeleteDirectory", json!({"DirectoryId": deleted}))
        .await;
    assert_eq!(
        delete.status, 200,
        "DeleteDirectory must be accepted, got {} with {:?}",
        delete.status, delete.body
    );

    let after_delete = emulator
        .data_plane("/Users/ListUsers", &deleted, json!({}))
        .await;
    assert_eq!(
        after_delete.error_code(),
        ACCESS_DENIED_EXCEPTION,
        "a deleted directory must stop being served, got {} with {:?}",
        after_delete.status,
        after_delete.body
    );

    let survivor = &created[0];
    let still_served = emulator
        .data_plane("/Users/ListUsers", survivor, json!({}))
        .await;
    assert_eq!(
        still_served.status, 200,
        "directory {survivor:?}: one directory's delete must not touch another, got {} with {:?}",
        still_served.status, still_served.body
    );
}
