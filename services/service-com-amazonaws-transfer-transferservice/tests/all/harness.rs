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
    engine: Arc<engine::InMemoryEngine>,
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
            service_com_amazonaws_transfer_transferservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// This emulator's store — the SAME one the handlers reach through
    /// `crate::state::service_state(&*self.engine)`, since `state::<T>()` is keyed by
    /// type on the one shared engine (see `state_roundtrip.rs`).
    ///
    /// For a suite that has to SEED a resource whose write operation is still a
    /// scaffold stub; a suite whose write operation exists goes over the wire instead.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_transfer_transferservice::ComAmazonawsTransferTransferserviceState,
    > {
        self.engine.state()
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
    is_prefixed_lower_hex_id(id, "s-", 17)
}

/// `SshPublicKeyId` is modelled `len 21..21`, pattern `^key-[0-9a-f]{17}$`.
pub(crate) fn is_modelled_key_id(id: &str) -> bool {
    is_prefixed_lower_hex_id(id, "key-", 17)
}

/// `WebAppId` is modelled `len 24..24`, pattern `^webapp-[0-9a-f]{17}$`.
pub(crate) fn is_modelled_web_app_id(id: &str) -> bool {
    is_prefixed_lower_hex_id(id, "webapp-", 17)
}

/// `DescribedWebAppVpcConfig.VpcEndpointId` is modelled `len 22..22`, pattern
/// `^vpce-[0-9a-f]{17}$`.
pub(crate) fn is_modelled_vpc_endpoint_id(id: &str) -> bool {
    is_prefixed_lower_hex_id(id, "vpce-", 17)
}

/// `ConnectorId` is modelled `len 19..19`, pattern `^c-([0-9a-f]{17})$`.
pub(crate) fn is_modelled_connector_id(id: &str) -> bool {
    id.len() == 19
        && id.starts_with("c-")
        && id[2..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// `ProfileId` is modelled `len 19..19`, pattern `^p-([0-9a-f]{17})$`.
pub(crate) fn is_modelled_profile_id(id: &str) -> bool {
    is_prefixed_lower_hex_id(id, "p-", 17)
}

/// Whether `id` is `prefix` followed by exactly `digits` lowercase hex digits — the
/// ONE copy of the `<prefix>-[0-9a-f]{n}` rule every minted identifier in this model
/// follows, so each predicate above is just its prefix and digit count.
fn is_prefixed_lower_hex_id(id: &str, prefix: &str, digits: usize) -> bool {
    id.strip_prefix(prefix).is_some_and(|rest| {
        rest.len() == digits
            && rest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

/// `HostKeyId` is modelled `len 25..25`, pattern `^hostkey-[0-9a-f]{17}$`.
pub(crate) fn is_modelled_host_key_id(id: &str) -> bool {
    id.len() == 25
        && id.starts_with("hostkey-")
        && id[8..].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// An OpenSSH Ed25519 private key and the SHA256 fingerprint `ssh-keygen -l` prints
/// for it, both produced here by the OpenSSH key-format crate — so the expectation
/// comes from the key, not from the handler under test.
pub(crate) fn generated_host_key() -> (String, String) {
    let keypair = ssh_key::private::Ed25519Keypair::random(&mut ssh_key::rand_core::OsRng);
    let key = ssh_key::PrivateKey::from(keypair);
    let material = key
        .to_openssh(ssh_key::LineEnding::LF)
        .expect("the generated key encodes as an OpenSSH private key")
        .to_string();
    let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
    (material, fingerprint)
}

/// The SSH algorithm name of private key MATERIAL, read back out of the material by
/// the OpenSSH key-format crate — one of the values `DescribedHostKey.Type` documents
/// (`ssh-rsa`, `ssh-ed25519`, `ecdsa-sha2-nistp256/384/521`).
pub(crate) fn host_key_algorithm(material: &str) -> String {
    ssh_key::PrivateKey::from_openssh(material)
        .expect("the test's own key material parses as an OpenSSH private key")
        .algorithm()
        .as_str()
        .to_string()
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

/// Drain every page of a `List*` operation, returning the per-page item counts and the
/// concatenated item key at `key_member`.
///
/// Shared by every `@paginated` suite in this crate, so the paging walk is written
/// once: `base` is the request without paging members, `max_results` the page bound to
/// send (`None` sends none), `items_member` the output's list member and `key_member`
/// the identifier to collect from each item.
pub(crate) async fn drain_pages(
    emulator: &Emulator,
    op: &str,
    base: &Value,
    items_member: &str,
    key_member: &str,
    max_results: Option<i64>,
) -> (Vec<usize>, Vec<String>) {
    let mut page_sizes = Vec::new();
    let mut keys = Vec::new();
    let mut next: Option<String> = None;

    loop {
        let mut request = base.clone();
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 200,
            "{op} (MaxResults {max_results:?}): expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let items = answer
            .get(items_member)
            .as_array()
            .cloned()
            .unwrap_or_default();
        page_sizes.push(items.len());
        for item in &items {
            keys.push(
                item.get(key_member)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        match answer.get("NextToken").as_str() {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 64,
            "{op} (MaxResults {max_results:?}): paging did not terminate"
        );
    }

    (page_sizes, keys)
}
