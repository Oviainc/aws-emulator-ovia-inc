// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One place for the router mount, the signed `POST /` dispatch, the decoded answer
//! and the modelled-shape predicates, so a new suite adds test ROWS rather than a
//! second copy of the transport. Every suite calls the real wire path — `POST /` with
//! `X-Amz-Target: TransferService.<Op>` through [`MultiServiceRouter`] — so what it
//! asserts is what an SDK sees, not what a handler returns.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for. It is read back out of ARNs, so it must not be the
/// handler's fallback default everywhere.
pub(crate) const REGION: &str = "us-east-1";

/// The canonical emulator account id every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// `SERVER_START_SPAN` in `src/state.rs` is 500 ms; wait past it.
pub(crate) const PAST_START_SPAN: Duration = Duration::from_millis(900);

/// An SSH public key whose body satisfies `SshPublicKeyBody`'s modelled pattern
/// `^\s*(ssh|ecdsa)-[a-z0-9-]+[ \t]+(([A-Za-z0-9+/]{4})*...)$`.
pub(crate) const SSH_KEY: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==";

/// A role ARN satisfying `Role`'s modelled pattern `^arn:.*role/\S+$` and `len 20..2048`.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/transfer-s3";

/// One emulator: the shared engine plus the transfer service mounted on a router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP answer, decoded.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The awsJson error code from the body's `__type`, without any shape-id prefix.
    pub(crate) fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t).to_string())
            .unwrap_or_default()
    }

    /// A member of a successful response body.
    pub(crate) fn get(&self, member: &str) -> &Value {
        member_of(&self.body, member)
    }
}

/// The JSON null a missing member reads as, so member access chains without `Option`.
static NULL: Value = Value::Null;

/// A member of a JSON object, or null when it is absent.
pub(crate) fn member_of<'a>(value: &'a Value, member: &str) -> &'a Value {
    value.get(member).unwrap_or(&NULL)
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_transfer_transferservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    pub(crate) async fn call(&self, op: &str, body: Value) -> Answer {
        self.call_in(REGION, op, body).await
    }

    /// Dispatch `op` signed for `region`, so a handler that reads the credential
    /// scope sees it.
    pub(crate) async fn call_in(&self, region: &str, op: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TransferService.{op}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/transfer/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "op {op}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// Create a server from `request` and return its `ServerId`.
    pub(crate) async fn create_server(&self, request: Value) -> String {
        let answer = self.call("CreateServer", request).await;
        assert_eq!(
            answer.status, 200,
            "CreateServer: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        answer
            .get("ServerId")
            .as_str()
            .unwrap_or_else(|| panic!("CreateServer returned no ServerId: {}", answer.body))
            .to_string()
    }

    /// Create a user and assert the create succeeded.
    pub(crate) async fn create_user(&self, server_id: &str, user_name: &str) {
        let answer = self
            .call(
                "CreateUser",
                json!({"ServerId": server_id, "UserName": user_name, "Role": ROLE}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "CreateUser {user_name}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// `ServerId` is modelled `len 19..19`, pattern `^s-([0-9a-f]{17})$`.
pub(crate) fn is_modelled_server_id(id: &str) -> bool {
    id.len() == 19
        && id.starts_with("s-")
        && id[2..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// `SshPublicKeyId` is modelled `len 21..21`, pattern `^key-[0-9a-f]{17}$`.
pub(crate) fn is_modelled_key_id(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("key-")
        && id[4..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// `ProfileId` is modelled `len 19..19`, pattern `^p-([0-9a-f]{17})$`.
pub(crate) fn is_modelled_profile_id(id: &str) -> bool {
    id.len() == 19
        && id.starts_with("p-")
        && id[2..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Assert that `actual` carries each member `expected` names, where a `null`
/// expectation means "the response must not carry this member at all".
pub(crate) fn assert_members(case: &str, actual: &Value, expected: &Value) {
    let expected = expected.as_object().expect("the expectation is an object");
    for (member, want) in expected {
        let got = actual.get(member).unwrap_or(&Value::Null);
        if want.is_null() {
            assert!(
                got.is_null(),
                "case {case:?}: expected {member} to be absent, got {got:?}"
            );
        } else {
            assert_eq!(
                got, want,
                "case {case:?}: expected {member} to be {want:?}, got {got:?}"
            );
        }
    }
}
