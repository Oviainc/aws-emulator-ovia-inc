// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the certificate-revocation-list import path: `ImportCrl`
//! and the `GetCrl` read it is proved through.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test: `ImportCrl` mints a CRL id and ARN, persists every member the
//! request carried, and a later read answers from that record — so each test spans a
//! TABLE of requests differing in the members the rule generalises over (name length,
//! blob size and content, the `enabled` flag and its absence, tag count, the trust
//! anchor named and its partition, the signing Region) rather than one recorded call.
//!
//! The harness below is local to this suite: the sibling lifecycle suite's client is
//! private to its own module, and the two targets are compiled into one test binary
//! only as separate modules.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`, so any
/// non-empty string is a legal bundle on the wire.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form. `ImportCrlRequest$crlData` is a `Blob`
/// `@length(1, 300000)` with no `@pattern`
/// (`smithy_query(query="operation", operation="ImportCrl")`), so the bytes are opaque
/// to the service and come back exactly as sent.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The largest `crlData` the model accepts: `Blob` `@length(1, 300000)`.
const MAX_CRL_DATA_BYTES: usize = 300_000;

/// The published `ResourceName` `@length` bounds `ImportCrlRequest$name` targets.
const MIN_NAME_LENGTH: usize = 1;
const MAX_NAME_LENGTH: usize = 255;

/// The Region the tests sign for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
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
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
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

    async fn import_crl(&self, body: Value) -> Answer {
        self.send("POST", "/crls", Some(body)).await
    }

    async fn import_crl_in_region(&self, body: Value, region: &str) -> Answer {
        self.send_in_region("POST", "/crls", Some(body), region)
            .await
    }

    async fn get_crl(&self, id: &str) -> Answer {
        self.send("GET", &format!("/crl/{id}"), None).await
    }
}

/// The modelled `Uuid` form: `@length(36, 36)` plus
/// `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`.
fn is_modelled_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    candidate.len() == 36
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
            })
}

/// The `crlData` wire form: restJson1 carries a `Blob` as standard-alphabet base64, the
/// codec the wire serializer itself uses (`aws_smithy_types::base64`).
fn encode_blob(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// The bytes behind a `crlData` the service returned.
fn decode_blob(case: &str, encoded: Option<&str>) -> Vec<u8> {
    let encoded = encoded.unwrap_or_else(|| panic!("case {case:?}: crlData must be present"));
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: crlData must be base64, got {error}"))
}

/// Create a trust anchor and answer its ARN.
async fn trust_anchor_arn(client: &Client, name: &str) -> String {
    let created = client.create_trust_anchor(name).await;
    assert_eq!(
        created.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {created:?}"
    );
    created
        .body
        .get("trustAnchor")
        .and_then(|anchor| anchor.get("trustAnchorArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {created:?}"))
        .to_string()
}

/// One row of the import table.
struct Case {
    /// The case name every assertion reports.
    name: &'static str,
    /// The `name` the request carries.
    crl_name: String,
    /// The bytes the request carries as `crlData`.
    data: Vec<u8>,
    /// The `enabled` member, or `None` when the request omits it.
    enabled: Option<bool>,
    /// The tags the request carries.
    tags: Vec<(&'static str, &'static str)>,
    /// Which trust anchor ARN the request links the CRL to.
    trust_anchor: String,
    /// The `enabled` the response must report.
    expected_enabled: bool,
}

impl Case {
    fn body(&self) -> Value {
        let mut body = json!({
            "name": self.crl_name,
            "crlData": encode_blob(&self.data),
            "trustAnchorArn": self.trust_anchor,
        });
        if let Some(enabled) = self.enabled {
            body["enabled"] = json!(enabled);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            );
        }
        body
    }
}

/// `ImportCrl` persists the whole record and `GetCrl` rebuilds it unchanged: a table
/// spanning both published `name` `@length` bounds, both published `crlData` `@length`
/// bounds plus a non-UTF-8 blob, `enabled` true / false / absent, tagged and untagged
/// imports, and two different trust anchors (one of them in another partition, which
/// `TrustAnchorArn`'s `@pattern` admits).
#[tokio::test]
async fn imported_crl_round_trips_through_get_crl() {
    let client = Client::new();
    let first_anchor = trust_anchor_arn(&client, "crl-anchor-one").await;
    let second_anchor = trust_anchor_arn(&client, "crl anchor two").await;
    // `TrustAnchorArn` is `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$")`
    // (`smithy_query(query="operation", operation="ImportCrl")`), so a non-default
    // partition is a legal link target too.
    let gov_anchor = format!(
        "arn:aws-us-gov:rolesanywhere:us-gov-west-1:{ACCOUNT}:trust-anchor/\
         11111111-2222-3333-4444-555555555555"
    );

    let cases = vec![
        Case {
            name: "minimal-import-omits-enabled",
            crl_name: "demo crl".to_string(),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: None,
            tags: Vec::new(),
            trust_anchor: first_anchor.clone(),
            expected_enabled: false,
        },
        Case {
            name: "enabled-true",
            crl_name: "enabled-crl".to_string(),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: first_anchor.clone(),
            expected_enabled: true,
        },
        Case {
            name: "enabled-false-and-tagged",
            crl_name: "tagged_crl".to_string(),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: Some(false),
            tags: vec![("env", "test"), ("owner", "guild")],
            trust_anchor: second_anchor.clone(),
            expected_enabled: false,
        },
        Case {
            name: "shortest-name",
            crl_name: "n".repeat(MIN_NAME_LENGTH),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: second_anchor.clone(),
            expected_enabled: true,
        },
        Case {
            name: "longest-name",
            crl_name: "n".repeat(MAX_NAME_LENGTH),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: first_anchor.clone(),
            expected_enabled: true,
        },
        Case {
            name: "smallest-crl-data",
            crl_name: "one byte crl".to_string(),
            data: vec![b'-'],
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: first_anchor.clone(),
            expected_enabled: true,
        },
        Case {
            name: "largest-crl-data",
            crl_name: "biggest crl".to_string(),
            data: vec![b'x'; MAX_CRL_DATA_BYTES],
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: first_anchor.clone(),
            expected_enabled: true,
        },
        Case {
            name: "binary-crl-data",
            crl_name: "der crl".to_string(),
            // A `Blob` is bytes, not text: every byte value must survive the round trip.
            data: (0u8..=255).collect(),
            enabled: Some(false),
            tags: vec![("only", "tag")],
            trust_anchor: second_anchor.clone(),
            expected_enabled: false,
        },
        Case {
            name: "other-partition-trust-anchor",
            crl_name: "gov crl".to_string(),
            data: CRL_PEM.as_bytes().to_vec(),
            enabled: Some(true),
            tags: Vec::new(),
            trust_anchor: gov_anchor,
            expected_enabled: true,
        },
    ];

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut imported: Vec<(&'static str, String, Value)> = Vec::new();
    for case in &cases {
        let body = case.body();
        let answer = client.import_crl(body.clone()).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected ImportCrl 201, got {answer:?}",
            case.name
        );
        let crl = answer.body.get("crl").cloned().unwrap_or(Value::Null);
        let id = crl
            .get("crlId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_uuid(&id),
            "case {:?}: crlId must match the modelled Uuid pattern, got {id:?}",
            case.name
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: every import must mint a fresh crlId, got the repeated {id:?}",
            case.name
        );
        assert_eq!(
            crl.get("crlArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:crl/{id}").as_str()),
            "case {:?}: crlArn must be built from the request's Region and the minted id, \
             got {crl:?}",
            case.name
        );
        assert_eq!(
            crl.get("name").and_then(Value::as_str),
            Some(case.crl_name.as_str()),
            "case {:?}: name must be the submitted one, got {crl:?}",
            case.name
        );
        assert_eq!(
            crl.get("trustAnchorArn").and_then(Value::as_str),
            Some(case.trust_anchor.as_str()),
            "case {:?}: trustAnchorArn must be the submitted link, got {crl:?}",
            case.name
        );
        assert_eq!(
            crl.get("enabled").and_then(Value::as_bool),
            Some(case.expected_enabled),
            "case {:?}: expected enabled {}, got {crl:?}",
            case.name,
            case.expected_enabled
        );
        let returned = decode_blob(case.name, crl.get("crlData").and_then(Value::as_str));
        assert_eq!(
            returned.len(),
            case.data.len(),
            "case {:?}: expected {} crlData bytes, got {}",
            case.name,
            case.data.len(),
            returned.len()
        );
        assert!(
            returned == case.data,
            "case {:?}: crlData must come back byte for byte, got {} differing bytes",
            case.name,
            returned
                .iter()
                .zip(&case.data)
                .filter(|(left, right)| left != right)
                .count()
        );
        let created_at = crl.get("createdAt").cloned().unwrap_or(Value::Null);
        let updated_at = crl.get("updatedAt").cloned().unwrap_or(Value::Null);
        assert!(
            !created_at.is_null() && !updated_at.is_null(),
            "case {:?}: createdAt and updatedAt must be present, got {crl:?}",
            case.name
        );
        assert_eq!(
            created_at, updated_at,
            "case {:?}: an import that has not been updated reports the same createdAt and \
             updatedAt, got {crl:?}",
            case.name
        );

        // The read answers from the stored record, not from the request that is now over.
        let fetched = client.get_crl(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected GetCrl 200, got {fetched:?}",
            case.name
        );
        assert_eq!(
            fetched.body.get("crl").cloned().unwrap_or(Value::Null),
            crl,
            "case {:?}: GetCrl must rebuild exactly what ImportCrl returned",
            case.name
        );
        imported.push((case.name, id, crl));
    }

    // Every earlier import is still readable unchanged: a later import adds a record
    // rather than replacing one.
    for (case, id, crl) in &imported {
        let fetched = client.get_crl(id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: every imported CRL stays readable, got {fetched:?}"
        );
        assert_eq!(
            fetched.body.get("crl").cloned().unwrap_or(Value::Null),
            *crl,
            "case {case:?}: a later import must not disturb this record"
        );
    }
}

/// Two identical imports are two CRLs: the id and ARN are minted per call, never derived
/// from the request.
#[tokio::test]
async fn repeated_identical_imports_mint_distinct_crls() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "repeat-anchor").await;
    let body = json!({
        "name": "same name every time",
        "crlData": encode_blob(CRL_PEM.as_bytes()),
        "trustAnchorArn": anchor,
    });

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for attempt in 1..=3 {
        let case = format!("attempt-{attempt}");
        let answer = client.import_crl(body.clone()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected ImportCrl 201, got {answer:?}"
        );
        let id = answer
            .body
            .get("crl")
            .and_then(|crl| crl.get("crlId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: each import must mint its own crlId, got the repeated {id:?}"
        );
    }
    assert_eq!(
        ids.len(),
        3,
        "expected 3 distinct CRLs from 3 identical imports, got {ids:?}"
    );

    // All three are independently readable.
    for id in &ids {
        let fetched = client.get_crl(id).await;
        assert_eq!(
            fetched.status, 200,
            "case {id:?}: each minted CRL must be readable, got {fetched:?}"
        );
    }
}

/// The CRL ARN is scoped to the Region the request was signed for, like every other ARN
/// this service mints.
#[tokio::test]
async fn crl_arn_carries_the_requests_region() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "region-anchor").await;

    for region in ["us-west-2", "eu-west-1", "ap-southeast-2"] {
        let answer = client
            .import_crl_in_region(
                json!({
                    "name": "regional crl",
                    "crlData": encode_blob(CRL_PEM.as_bytes()),
                    "trustAnchorArn": anchor,
                }),
                region,
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {region:?}: expected ImportCrl 201, got {answer:?}"
        );
        let crl = answer.body.get("crl").cloned().unwrap_or(Value::Null);
        let id = crl
            .get("crlId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            crl.get("crlArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:crl/{id}").as_str()),
            "case {region:?}: crlArn must carry the signed Region, got {crl:?}"
        );
    }
}

/// A `crlId` no import minted is a 404 `ResourceNotFoundException`, never a canned
/// success — for ids of the right shape that were never used and for ids belonging to
/// this service's other resource types.
#[tokio::test]
async fn get_crl_refuses_an_unknown_crl_id() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "unknown-id-anchor").await;
    let imported = client
        .import_crl(json!({
            "name": "a real crl",
            "crlData": encode_blob(CRL_PEM.as_bytes()),
            "trustAnchorArn": anchor.clone(),
        }))
        .await;
    assert_eq!(
        imported.status, 201,
        "expected ImportCrl 201, got {imported:?}"
    );
    // The trust anchor's own id is a well-formed `Uuid` that is not a CRL id: the CRL
    // store is what the read consults, not any resource of this service.
    let anchor_id = anchor
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    for (case, id) in [
        ("never-minted", "00000000-0000-4000-8000-000000000000".to_string()),
        ("all-hex", "abcdef12-3456-7890-abcd-ef1234567890".to_string()),
        ("a-trust-anchor-id", anchor_id),
    ] {
        let answer = client.get_crl(&id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected GetCrl 404 for {id:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("message").and_then(Value::as_str),
            Some("Crl not found"),
            "case {case:?}: expected the fixed not-found wording, got {answer:?}"
        );
    }
}
