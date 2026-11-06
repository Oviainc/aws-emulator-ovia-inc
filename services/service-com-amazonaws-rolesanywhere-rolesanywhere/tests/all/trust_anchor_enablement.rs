// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DisableTrustAnchor`.
//!
//! The rule under test: a trust anchor's `enabled` flag is a persisted member of the
//! stored record, and the operation that clears it writes `false` onto the trust anchor
//! the request addresses and nothing else — the answer is the whole record as it now
//! stands, the write is visible to every later read and list projection on the same
//! engine, writing a flag that already holds that value is still a success reporting
//! that value, no other stored anchor is touched, and an id no stored anchor carries is
//! the operation's modelled `ResourceNotFoundException` (404) instead of a success that
//! lands nowhere
//! (`smithy_query(query="operation", operation="DisableTrustAnchor")`: input
//! `trustAnchorId`, output the required `trustAnchor`, errors `AccessDeniedException`
//! 403 and `ResourceNotFoundException` 404).
//!
//! The tables below therefore differ along what the rule generalises over: the
//! `sourceType` the stored anchor carries (every value the modelled `TrustAnchorType`
//! enum lists), how many times the flag is written, how many anchors are stored
//! alongside the addressed one, and each way an id can name no stored anchor.
//!
//! The harness below is local to this suite: each sibling suite's client is private to
//! its own module, and the targets are compiled into one test binary only as separate
//! modules.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle. `SourceData$x509CertificateData` is `@length(1, 204800)`
/// with no `@pattern`, so any non-empty string is a legal certificate bundle on the wire
/// (`smithy_query(query="shape", shape="SourceData")`).
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW5hYmxlbWVudC1zdWl0ZS1jZXJ0\n-----END CERTIFICATE-----\n";

/// The Region the suite signs for.
const REGION: &str = "us-west-2";

/// Every value the modelled `TrustAnchorType` enum accepts, in the order the model lists
/// them (`smithy_query(query="shape", shape="TrustAnchorType")`).
const TRUST_ANCHOR_TYPES: [&str; 3] = [
    "AWS_ACM_PCA",
    "CERTIFICATE_BUNDLE",
    "SELF_SIGNED_REPOSITORY",
];

/// The wording a refusal carries when the trust anchor a request ADDRESSES by
/// `trustAnchorId` is not stored: wholly fixed text, the same every trust-anchor
/// operation that addresses one answers a miss with.
const TRUST_ANCHOR_NOT_FOUND_MESSAGE: &str = "TrustAnchor not found";

/// Every `TrustAnchorDetail` member that a call clearing the flag must leave exactly as
/// the record held it (`smithy_query(query="shape", shape="TrustAnchorDetail")` lists
/// `trustAnchorId`, `trustAnchorArn`, `name`, `source`, `enabled`, `createdAt`,
/// `updatedAt`, `notificationSettings`, `crlDpRevocationEnabled`; `enabled` is what the
/// call writes and `updatedAt` is when it wrote, so the other seven are untouched).
const UNTOUCHED_MEMBERS: [&str; 7] = [
    "trustAnchorId",
    "trustAnchorArn",
    "name",
    "source",
    "createdAt",
    "notificationSettings",
    "crlDpRevocationEnabled",
];

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

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
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
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
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

    async fn create_trust_anchor(&self, body: Value) -> Answer {
        self.send("POST", "/trustanchors", Some(body)).await
    }

    async fn get_trust_anchor(&self, id: &str) -> Answer {
        self.send("GET", &format!("/trustanchor/{id}"), None).await
    }

    async fn delete_trust_anchor(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/trustanchor/{id}"), None)
            .await
    }

    async fn disable_trust_anchor(&self, id: &str) -> Answer {
        self.send("POST", &format!("/trustanchor/{id}/disable"), None)
            .await
    }

    async fn list_trust_anchors(&self) -> Answer {
        self.send("GET", "/trustanchors", None).await
    }
}

/// The ARN of a Private CA, in the form `SourceData$acmPcaArn` names for a trust anchor
/// of type `AWS_ACM_PCA`.
fn acm_pca_arn(suffix: &str) -> String {
    format!("arn:aws:acm-pca:{REGION}:000000000000:certificate-authority/{suffix}")
}

/// A `Source` of this type carrying the certificate data member the model ties to it:
/// `x509CertificateData` is "Included for trust anchors of type `CERTIFICATE_BUNDLE`"
/// and `acmPcaArn` "Included for trust anchors of type `AWS_ACM_PCA`"
/// (`smithy_query(query="shape", shape="SourceData")`); the model ties neither member to
/// `SELF_SIGNED_REPOSITORY`, so a bundle stands in there.
fn source(source_type: &str, suffix: &str) -> Value {
    let source_data = if source_type == "AWS_ACM_PCA" {
        json!({"acmPcaArn": acm_pca_arn(suffix)})
    } else {
        json!({"x509CertificateData": CERTIFICATE_BUNDLE_PEM})
    };
    json!({"sourceType": source_type, "sourceData": source_data})
}

/// The `trustAnchor` a successful call answered with.
fn anchor_of(case: &str, expected_status: u16, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
    answer
        .body
        .get("trustAnchor")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no trustAnchor in {answer:?}"))
}

/// One member of a trust anchor a call answered with.
fn member_of(case: &str, anchor: &Value, member: &str) -> Value {
    anchor
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {anchor:?}"))
}

/// The id of a trust anchor a call answered with.
fn id_of(case: &str, anchor: &Value) -> String {
    member_of(case, anchor, "trustAnchorId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: trustAnchorId is not a string in {anchor:?}"))
        .to_string()
}

/// The `enabled` flag a trust anchor reports.
fn enabled_of(case: &str, anchor: &Value) -> bool {
    member_of(case, anchor, "enabled")
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: enabled is not a boolean in {anchor:?}"))
}

/// An instant a trust anchor reports, parsed from the RFC 3339 string the
/// `@timestampFormat("date-time")` members go onto the wire as. Instants are per-run
/// values, so only their ORDER is ever asserted, never a literal.
fn instant_of(case: &str, anchor: &Value, member: &str) -> DateTime {
    let rendered = member_of(case, anchor, member);
    let text = rendered
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is not a string in {anchor:?}"));
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} is not an RFC 3339 instant: {error}")
    })
}

/// The entry the list projection reports for one anchor.
fn listed_anchor(case: &str, listed: &Answer, id: &str) -> Value {
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected ListTrustAnchors 200, got {listed:?}"
    );
    listed
        .body
        .get("trustAnchors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no trustAnchors array in {listed:?}"))
        .iter()
        .find(|entry| entry.get("trustAnchorId").and_then(Value::as_str) == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {id} is not in the listing {listed:?}"))
}

/// The `message` an answer carries.
fn message_of(answer: &Answer) -> String {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Clearing the flag writes `false` onto the addressed anchor, reports the whole record,
/// leaves every other member as the record held it, and is visible to the read and to
/// the list projection; writing it again is still a success reporting `false`.
///
/// The rows differ along what the rule generalises over: the `sourceType` the stored
/// anchor carries (every value the modelled enum lists) and how many times the flag is
/// written (once, then again on an anchor that already holds it).
#[tokio::test]
async fn disabling_a_trust_anchor_clears_enabled_for_every_modelled_source_type() {
    let client = Client::new();
    for source_type in TRUST_ANCHOR_TYPES {
        let case = source_type;
        let created = anchor_of(
            case,
            201,
            &client
                .create_trust_anchor(json!({
                    "name": format!("{source_type} anchor"),
                    "source": source(source_type, "enablement-ca"),
                    "enabled": true,
                }))
                .await,
        );
        let id = id_of(case, &created);
        assert!(
            enabled_of(case, &created),
            "case {case:?}: a created anchor the request enabled must report enabled true, \
             got {created:?}"
        );
        let created_at = instant_of(case, &created, "createdAt");

        // Writing the flag twice: the first call changes it, the second addresses an
        // anchor that already holds the written value.
        for attempt in 1..=2u32 {
            let case = format!("{source_type}-attempt-{attempt}");
            let case = case.as_str();
            let disabled = anchor_of(case, 200, &client.disable_trust_anchor(&id).await);
            assert!(
                !enabled_of(case, &disabled),
                "case {case:?}: the answer must report enabled false, got {disabled:?}"
            );
            for member in UNTOUCHED_MEMBERS {
                assert_eq!(
                    disabled.get(member),
                    created.get(member),
                    "case {case:?}: {member} must stay as the record held it ({:?}), got {:?}",
                    created.get(member),
                    disabled.get(member)
                );
            }
            let updated_at = instant_of(case, &disabled, "updatedAt");
            assert!(
                updated_at >= created_at,
                "case {case:?}: updatedAt must be at or after createdAt ({created_at:?}), got \
                 {updated_at:?}"
            );

            // The write is state, not a canned answer: the read and the list projection
            // report the same record.
            let read_back = anchor_of(case, 200, &client.get_trust_anchor(&id).await);
            assert_eq!(
                read_back, disabled,
                "case {case:?}: GetTrustAnchor must report what the call wrote ({disabled:?}), \
                 got {read_back:?}"
            );
            let listed = client.list_trust_anchors().await;
            let projection = listed_anchor(case, &listed, &id);
            assert_eq!(
                projection.get("enabled"),
                Some(&Value::Bool(false)),
                "case {case:?}: the list projection must report enabled false, got \
                 {projection:?}"
            );
        }
    }
}

/// Only the anchor the request addresses is written: every other stored anchor keeps its
/// flag and the instant it was last updated.
///
/// The rows differ along what the rule generalises over: how many anchors are stored
/// beside the addressed one, and which position in the stored order is addressed.
#[tokio::test]
async fn disabling_one_trust_anchor_leaves_every_other_stored_anchor_alone() {
    for stored_count in [1usize, 2, 5] {
        for addressed in 0..stored_count {
            let case = format!("{stored_count}-anchors-addressing-{addressed}");
            let case = case.as_str();
            let client = Client::new();

            let mut anchors = Vec::new();
            for index in 0..stored_count {
                let created = anchor_of(
                    case,
                    201,
                    &client
                        .create_trust_anchor(json!({
                            "name": format!("anchor {index}"),
                            "source": source("CERTIFICATE_BUNDLE", "untouched-ca"),
                            "enabled": true,
                        }))
                        .await,
                );
                anchors.push(created);
            }

            let addressed_id = id_of(case, &anchors[addressed]);
            let answer = client.disable_trust_anchor(&addressed_id).await;
            let disabled = anchor_of(case, 200, &answer);
            assert!(
                !enabled_of(case, &disabled),
                "case {case:?}: the addressed anchor must report enabled false, got {disabled:?}"
            );

            for (index, created) in anchors.iter().enumerate() {
                if index == addressed {
                    continue;
                }
                let other_id = id_of(case, created);
                let read_back = anchor_of(case, 200, &client.get_trust_anchor(&other_id).await);
                assert_eq!(
                    read_back, *created,
                    "case {case:?}: anchor {index} must be exactly as its create left it \
                     ({created:?}), got {read_back:?}"
                );
            }
        }
    }
}

/// An id no stored anchor carries is the operation's modelled `ResourceNotFoundException`
/// (404), worded the way every trust-anchor operation words an addressed miss, and
/// nothing is created for it.
///
/// The rows differ along what the rule generalises over: each way a well-formed id can
/// name no stored anchor — never created at all, and the id of an anchor that was
/// created and then deleted.
#[tokio::test]
async fn disabling_an_unknown_trust_anchor_is_refused_as_not_found() {
    let client = Client::new();

    // A well-formed `Uuid` (`@length(36, 36)`, pattern
    // `[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}`) that no create minted.
    let never_created = "0123abcd-0000-0000-0000-000000000000".to_string();

    let created = anchor_of(
        "setup",
        201,
        &client
            .create_trust_anchor(json!({
                "name": "anchor to delete",
                "source": source("CERTIFICATE_BUNDLE", "deleted-ca"),
            }))
            .await,
    );
    let deleted_id = id_of("setup", &created);
    let deleted = client.delete_trust_anchor(&deleted_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"setup\": expected DeleteTrustAnchor 200, got {deleted:?}"
    );

    for (case, id) in [
        ("never-created", never_created),
        ("already-deleted", deleted_id),
    ] {
        let answer = client.disable_trust_anchor(&id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown id must be refused with HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: the refusal must be the operation's modelled error, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            TRUST_ANCHOR_NOT_FOUND_MESSAGE,
            "case {case:?}: the refusal must carry the wording this service answers an \
             addressed miss with, got {answer:?}"
        );

        let read_back = client.get_trust_anchor(&id).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: the refused call must not have created anything, got {read_back:?}"
        );
    }
}
