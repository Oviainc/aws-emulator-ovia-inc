// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the wording of the modelled `ResourceNotFoundException` the
//! operations that ADDRESS a resource by its own id answer with.
//!
//! The rule under test: a refusal for a resource the store does not hold carries the
//! wording the service words that resource class's miss with, never a message this
//! emulator invented — the profile operations report the resource by the ARN its id names
//! in the request's Region and account ("The item doesn't exist. arn: <arn> ; aborting."),
//! while the trust-anchor operations report the wholly fixed "TrustAnchor not found", with
//! no id interpolated. The status (404) and the error code are the same on every path.
//!
//! Each table's rows differ along what the rule generalises over: WHICH id fails to
//! resolve and why (never minted, deleted a moment ago, an id minted for another resource
//! type, an all-zero id), the Region the request is signed for, how many sibling resources
//! the store holds, and whether the call is the read or the delete path.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// The Region the suite signs for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Ids shaped like the modelled `Uuid`
/// (`@length(36, 36)`, `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`, from
/// `smithy_query(query="operation", operation="GetProfile")`) that nothing in the store is
/// ever created under, so the boundary validator accepts them and the handler has to
/// resolve them.
const UNMINTED_IDS: [(&str, &str); 3] = [
    ("all-zero-id", "00000000-0000-0000-0000-000000000000"),
    ("hex-words-id", "deadbeef-dead-beef-dead-beefdeadbeef"),
    ("all-f-id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_profile_in_region(&self, name: &str, region: &str) -> Answer {
        self.send_in_region(
            "POST",
            "/profiles",
            Some(json!({
                "name": name,
                "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/DemoRole")],
            })),
            region,
        )
        .await
    }

    async fn create_trust_anchor(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await
    }

    async fn get_profile_in_region(&self, id: &str, region: &str) -> Answer {
        self.send_in_region("GET", &format!("/profile/{id}"), None, region)
            .await
    }

    async fn delete_profile_in_region(&self, id: &str, region: &str) -> Answer {
        self.send_in_region("DELETE", &format!("/profile/{id}"), None, region)
            .await
    }

    async fn get_trust_anchor_in_region(&self, id: &str, region: &str) -> Answer {
        self.send_in_region("GET", &format!("/trustanchor/{id}"), None, region)
            .await
    }

    async fn delete_trust_anchor_in_region(&self, id: &str, region: &str) -> Answer {
        self.send_in_region("DELETE", &format!("/trustanchor/{id}"), None, region)
            .await
    }
}

/// The ARN a profile id carries in `region`, which is the ARN a refusal naming that id
/// words: `ProfileArn` is
/// `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$")`
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `profileArn`).
fn profile_arn(region: &str, profile_id: &str) -> String {
    format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:profile/{profile_id}")
}

/// The wording an ARN-keyed miss carries.
fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// The wholly fixed wording a trust anchor addressed by `trustAnchorId` misses with.
const TRUST_ANCHOR_NOT_FOUND_MESSAGE: &str = "TrustAnchor not found";

/// Create a trust anchor and answer its id.
async fn trust_anchor_id(client: &Client, name: &str) -> String {
    let created = client.create_trust_anchor(name).await;
    assert_eq!(
        created.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {created:?}"
    );
    created
        .body
        .pointer("/trustAnchor/trustAnchorId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorId in {created:?}"))
        .to_string()
}

/// Create a profile in `region` and answer its id.
async fn profile_id(client: &Client, name: &str, region: &str) -> String {
    let created = client.create_profile_in_region(name, region).await;
    assert_eq!(
        created.status, 201,
        "profile {name:?}: expected CreateProfile 201, got {created:?}"
    );
    created
        .body
        .pointer("/profile/profileId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("profile {name:?}: no profileId in {created:?}"))
        .to_string()
}

/// Assert one refusal is the modelled 404 carrying exactly `expected_message`.
fn assert_not_found(case: &str, answer: &Answer, expected_message: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected status 404, got {}",
        answer.status
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        answer.error_code
    );
    assert_eq!(
        answer.message(),
        Some(expected_message),
        "case {case:?}: expected message {expected_message:?}, got {:?}",
        answer.message()
    );
}

/// `GetProfile` and `DeleteProfile` refuse an id that resolves to no stored profile with
/// the ARN-keyed wording, the ARN composed from the requested id and the Region the
/// request was signed for.
///
/// The table spans why the id does not resolve (never minted, minted for a trust anchor,
/// deleted a moment ago), the Region, and whether other profiles are stored.
#[tokio::test]
async fn profile_not_found_refusals_name_the_profile_by_arn() {
    let client = Client::new();

    // A sibling profile and a trust anchor, so the store is not empty and a trust
    // anchor's id is available as an id that is real but belongs to another resource type.
    let sibling = profile_id(&client, "sibling", REGION).await;
    let anchor = trust_anchor_id(&client, "sibling-anchor").await;
    // A profile created and then deleted: its id was minted, and is now unresolvable.
    let deleted = profile_id(&client, "deleted", REGION).await;
    let removed = client.delete_profile_in_region(&deleted, REGION).await;
    assert_eq!(
        removed.status, 200,
        "case \"delete-the-deleted-fixture\": expected DeleteProfile 200, got {removed:?}"
    );

    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (label, id) in UNMINTED_IDS {
        cases.push((format!("unminted-{label}"), id.to_string(), REGION));
    }
    cases.push((
        "trust-anchor-id-as-profile-id".to_string(),
        anchor.clone(),
        REGION,
    ));
    cases.push(("deleted-profile-id".to_string(), deleted.clone(), REGION));
    // The same unresolvable id signed for other Regions: the ARN's Region follows the
    // request, so the message does too.
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        cases.push((
            format!("unminted-in-{region}"),
            UNMINTED_IDS[1].1.to_string(),
            region,
        ));
    }
    // A profile that exists in one Region's ARN namespace is addressed by id, so a
    // request signed for another Region still resolves it; its own id therefore never
    // appears in this table as a miss. The sibling is only here to keep the store
    // non-empty.
    let _ = &sibling;

    for (case, id, region) in cases {
        let expected = arn_not_found_message(&profile_arn(region, &id));

        let read = client.get_profile_in_region(&id, region).await;
        assert_not_found(&format!("GetProfile/{case}"), &read, &expected);

        let deleted = client.delete_profile_in_region(&id, region).await;
        assert_not_found(&format!("DeleteProfile/{case}"), &deleted, &expected);
    }

    // The store is untouched by the refusals: the sibling is still readable.
    let still_there = client.get_profile_in_region(&sibling, REGION).await;
    assert_eq!(
        still_there.status, 200,
        "case \"sibling-survives-the-refusals\": expected GetProfile 200, got {still_there:?}"
    );
}

/// `GetTrustAnchor` and `DeleteTrustAnchor` refuse an id that resolves to no stored trust
/// anchor with wholly fixed text: no id, no ARN and no Region appear in it.
///
/// The table spans the same dimensions as the profile one.
#[tokio::test]
async fn trust_anchor_not_found_refusals_carry_the_fixed_wording() {
    let client = Client::new();

    let sibling = trust_anchor_id(&client, "sibling-anchor").await;
    let profile = profile_id(&client, "sibling-profile", REGION).await;
    let deleted = trust_anchor_id(&client, "deleted-anchor").await;
    let removed = client.delete_trust_anchor_in_region(&deleted, REGION).await;
    assert_eq!(
        removed.status, 200,
        "case \"delete-the-deleted-fixture\": expected DeleteTrustAnchor 200, got {removed:?}"
    );

    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (label, id) in UNMINTED_IDS {
        cases.push((format!("unminted-{label}"), id.to_string(), REGION));
    }
    cases.push((
        "profile-id-as-trust-anchor-id".to_string(),
        profile.clone(),
        REGION,
    ));
    cases.push((
        "deleted-trust-anchor-id".to_string(),
        deleted.clone(),
        REGION,
    ));
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        cases.push((
            format!("unminted-in-{region}"),
            UNMINTED_IDS[2].1.to_string(),
            region,
        ));
    }

    for (case, id, region) in cases {
        for (path, answer) in [
            (
                "GetTrustAnchor",
                client.get_trust_anchor_in_region(&id, region).await,
            ),
            (
                "DeleteTrustAnchor",
                client.delete_trust_anchor_in_region(&id, region).await,
            ),
        ] {
            let labelled = format!("{path}/{case}");
            assert_not_found(&labelled, &answer, TRUST_ANCHOR_NOT_FOUND_MESSAGE);
            let message = answer.message().unwrap_or_default();
            assert!(
                !message.contains(&id),
                "case {labelled:?}: the wording is fixed, so the requested id must not appear, \
                 got {message:?}"
            );
            assert!(
                !message.contains(region) && !message.contains("arn:"),
                "case {labelled:?}: the wording is fixed, so no Region or ARN may appear, got \
                 {message:?}"
            );
        }
    }

    let still_there = client.get_trust_anchor_in_region(&sibling, REGION).await;
    assert_eq!(
        still_there.status, 200,
        "case \"sibling-survives-the-refusals\": expected GetTrustAnchor 200, got \
         {still_there:?}"
    );
}
