// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's resource-policy operations:
//! `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the
//! handler and the response envelope are all exercised, not just the handler body.
//!
//! The assertions pin RULES over a SET of inputs rather than one request: the
//! round-trip table spans the forms the IAM policy language admits (one statement as an
//! object and as a list, both `Effect` values, `Action` as a string and as a list, every
//! `Principal` form, the documented `Version` values, pretty-printed and minified text);
//! the refusal table walks one grammar violation per row; the size test checks the
//! documented 4 KB bound and one byte past it; and the revision tests assert the
//! conditional-request invariant (a put or delete that names a revision id succeeds only
//! on the current one, and a refused one leaves the stored policy untouched).

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`rum`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on. Request authentication is switched off below, so only the
/// scope's shape matters.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/rum/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The documented ceiling on a resource policy document.
/// Source: `smithy_query(query="operation", operation="PutResourcePolicy")` — the
/// operation's doc says "The maximum size of the policy is 4 KB", and
/// `PolicySizeLimitExceededException` says "The limit is 4 KB".
const MAX_POLICY_DOCUMENT_BYTES: usize = 4 * 1024;

/// `PolicyRevisionId`'s modelled length bounds.
/// Source: `smithy_query(query="shape", shape="PolicyRevisionId")` — `@length(min: 1,
/// max: 255)`.
const POLICY_REVISION_ID_BOUNDS: (usize, usize) = (1, 255);

/// One emulator: a shared engine, the RUM service, and the router that dispatches to
/// it. Authentication is off so a test needs no signing key.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(engine));
    router
}

/// One HTTP answer, kept as raw bytes so a case can assert on an EMPTY body as well as
/// on JSON.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the answered document as a string, panicking when it is absent.
    fn member(&self, case: &str, member: &str) -> String {
        self.json(case)[member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {member} member, got {:?}",
                    String::from_utf8_lossy(&self.body)
                )
            })
            .to_string()
    }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://rum.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", authorization(REGION))
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    Reply {
        status,
        body: response.into_body(),
    }
}

/// Create one app monitor named `name`, asserting the call succeeded.
async fn create_app_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    let reply = call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({"Name": name, "Domain": "example.com"})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `PutResourcePolicy`: `PUT /appmonitor/{Name}/policy`.
async fn put_policy(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    revision_id: Option<&str>,
) -> Reply {
    let mut body = json!({"PolicyDocument": document});
    if let Some(revision_id) = revision_id {
        body.as_object_mut()
            .expect("a JSON object")
            .insert("PolicyRevisionId".to_string(), json!(revision_id));
    }
    call(
        router,
        "PUT",
        &format!("/appmonitor/{name}/policy"),
        Some(body),
    )
    .await
}

/// `GetResourcePolicy`: `GET /appmonitor/{Name}/policy`.
async fn get_policy(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/appmonitor/{name}/policy"), None).await
}

/// `DeleteResourcePolicy`: `DELETE /appmonitor/{Name}/policy`, with `PolicyRevisionId`
/// bound to the `policyRevisionId` query string (`@httpQuery`).
async fn delete_policy(
    router: &MultiServiceRouter,
    name: &str,
    revision_id: Option<&str>,
) -> Reply {
    let path = match revision_id {
        Some(revision_id) => format!("/appmonitor/{name}/policy?policyRevisionId={revision_id}"),
        None => format!("/appmonitor/{name}/policy"),
    };
    call(router, "DELETE", &path, None).await
}

/// Assert a reply is the modelled refusal `code` with `status`.
fn assert_refused(reply: &Reply, case: &str, status: u16, code: &str) {
    assert_eq!(
        reply.status,
        status,
        "case {case:?}: expected HTTP {status}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let actual = reply.error_code(case);
    assert_eq!(
        actual, code,
        "case {case:?}: expected error code {code}, got {actual:?}"
    );
}

/// A `PolicyRevisionId` the service minted: a non-empty string inside the modelled
/// length bounds.
fn assert_modelled_revision_id(case: &str, revision_id: &str) {
    let (min, max) = POLICY_REVISION_ID_BOUNDS;
    assert!(
        revision_id.len() >= min && revision_id.len() <= max,
        "case {case:?}: expected a PolicyRevisionId of {min}..{max} characters, \
         got {revision_id:?} ({} characters)",
        revision_id.len()
    );
}

/// The policy documents a well-formed IAM resource policy may take, one row per form the
/// IAM JSON policy language admits. Rows differ along: whether `Version` is declared and
/// which documented value it carries, whether `Statement` is one object or a list (and
/// how many entries), which `Effect`, whether the action is `Action` or `NotAction` and
/// whether it is a string or a list, which `Principal` form, whether `Resource`, `Sid`,
/// `Id` and `Condition` are present, and whether the text is minified or indented.
fn well_formed_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one-statement-as-object-no-version",
            json!({
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "rum:PutRumEvents"
                }
            })
            .to_string(),
        ),
        (
            "one-statement-as-list-current-version",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowOneAccount",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "rum:PutRumEvents",
                    "Resource": "*"
                }]
            })
            .to_string(),
        ),
        (
            "legacy-version",
            json!({
                "Version": "2008-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "rum:PutRumEvents"
                }]
            })
            .to_string(),
        ),
        (
            "two-statements-allow-and-deny",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": ["arn:aws:iam::111122223333:root"]},
                        "Action": ["rum:PutRumEvents", "rum:GetAppMonitorData"],
                        "Resource": ["*"]
                    },
                    {
                        "Effect": "Deny",
                        "Principal": {"AWS": "arn:aws:iam::444455556666:root"},
                        "Action": "rum:PutRumEvents"
                    }
                ]
            })
            .to_string(),
        ),
        (
            "not-action-and-not-principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "NotAction": ["rum:PutRumEvents"],
                    "NotResource": "*"
                }]
            })
            .to_string(),
        ),
        (
            "policy-id-and-condition",
            json!({
                "Version": "2012-10-17",
                "Id": "rum-resource-policy",
                "Statement": [{
                    "Sid": "AllowFromOneOrg",
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "rum:PutRumEvents",
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {"aws:PrincipalOrgID": ["o-abcdef0123"]},
                        "Bool": {"aws:SecureTransport": true},
                        "NumericLessThan": {"aws:MultiFactorAuthAge": 3600}
                    }
                }]
            })
            .to_string(),
        ),
        (
            "service-and-federated-principals",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {
                        "Service": "cloudwatch.amazonaws.com",
                        "Federated": ["cognito-identity.amazonaws.com"],
                        "CanonicalUser": "79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be"
                    },
                    "Action": "rum:PutRumEvents"
                }]
            })
            .to_string(),
        ),
        (
            "three-statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": "rum:PutRumEvents"},
                    {"Effect": "Allow", "Principal": "*", "Action": "rum:GetAppMonitorData"},
                    {"Effect": "Deny", "Principal": "*", "Action": "rum:DeleteAppMonitor"}
                ]
            })
            .to_string(),
        ),
        (
            // Indented, multi-line text: the stored document must come back with the
            // same bytes, not re-serialized.
            "pretty-printed-text",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Effect\": \"Allow\",\n      \"Principal\": {\"AWS\": \"*\"},\n      \
             \"Action\": \"rum:PutRumEvents\"\n    }\n  ]\n}\n"
                .to_string(),
        ),
        (
            // Non-ASCII text inside the document: byte equality must survive it too.
            "non-ascii-sid",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "zulässigeRegel",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "rum:PutRumEvents"
                }]
            })
            .to_string(),
        ),
    ]
}

/// One grammar violation per row. Each document is refused with
/// `MalformedPolicyDocumentException` — "The policy document that you specified is not
/// formatted correctly" — and stores nothing.
fn malformed_documents() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        ("unterminated-json", "{not valid json".to_string()),
        ("empty-text", String::new()),
        ("blank-text", "   ".to_string()),
        ("top-level-array", "[]".to_string()),
        ("top-level-string", "\"a policy\"".to_string()),
        ("top-level-number", "17".to_string()),
        ("top-level-null", "null".to_string()),
        ("trailing-comma", "{\"Statement\": [],}".to_string()),
    ];
    let documents = [
        ("no-statement", json!({"Version": "2012-10-17"})),
        ("empty-statement-list", json!({"Statement": []})),
        ("statement-as-string", json!({"Statement": "Allow"})),
        ("statement-entry-as-string", json!({"Statement": ["Allow"]})),
        (
            "statement-missing-effect",
            json!({"Statement": [{"Principal": "*", "Action": "rum:PutRumEvents"}]}),
        ),
        (
            "effect-wrong-case",
            json!({"Statement": [{"Effect": "allow", "Action": "rum:PutRumEvents"}]}),
        ),
        (
            "effect-unknown-value",
            json!({"Statement": [{"Effect": "Permit", "Action": "rum:PutRumEvents"}]}),
        ),
        (
            "effect-as-list",
            json!({"Statement": [{"Effect": ["Allow"], "Action": "rum:PutRumEvents"}]}),
        ),
        (
            "both-action-and-not-action",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "NotAction": "rum:DeleteAppMonitor"
            }]}),
        ),
        (
            "no-action-at-all",
            json!({"Statement": [{"Effect": "Allow", "Principal": "*"}]}),
        ),
        (
            "action-as-number",
            json!({"Statement": [{"Effect": "Allow", "Action": 7}]}),
        ),
        (
            "action-as-empty-list",
            json!({"Statement": [{"Effect": "Allow", "Action": []}]}),
        ),
        (
            "action-list-with-non-string",
            json!({"Statement": [{"Effect": "Allow", "Action": ["rum:PutRumEvents", 7]}]}),
        ),
        (
            "resource-as-object",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Resource": {"Arn": "*"}
            }]}),
        ),
        (
            "principal-as-number",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Principal": 1
            }]}),
        ),
        (
            "principal-as-empty-object",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Principal": {}
            }]}),
        ),
        (
            "principal-entry-as-object",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Principal": {"AWS": {"Account": "111122223333"}}
            }]}),
        ),
        (
            "sid-as-number",
            json!({"Statement": [{"Effect": "Allow", "Action": "rum:PutRumEvents", "Sid": 1}]}),
        ),
        (
            "condition-as-string",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Condition": "StringEquals"
            }]}),
        ),
        (
            "condition-operator-as-string",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Condition": {"StringEquals": "aws:PrincipalOrgID"}
            }]}),
        ),
        (
            "condition-value-as-object",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "rum:PutRumEvents",
                "Condition": {"StringEquals": {"aws:PrincipalOrgID": {"id": "o-abcdef0123"}}}
            }]}),
        ),
        (
            "unknown-version",
            json!({
                "Version": "2019-01-01",
                "Statement": [{"Effect": "Allow", "Action": "rum:PutRumEvents"}]
            }),
        ),
        (
            "version-as-number",
            json!({
                "Version": 20_121_017,
                "Statement": [{"Effect": "Allow", "Action": "rum:PutRumEvents"}]
            }),
        ),
        (
            "id-as-number",
            json!({
                "Id": 7,
                "Statement": [{"Effect": "Allow", "Action": "rum:PutRumEvents"}]
            }),
        ),
        (
            "unknown-top-level-element",
            json!({
                "Statements": [{"Effect": "Allow", "Action": "rum:PutRumEvents"}]
            }),
        ),
        (
            "statement-hoisted-to-top-level",
            json!({"Effect": "Allow", "Action": "rum:PutRumEvents", "Principal": "*"}),
        ),
    ];
    rows.extend(
        documents
            .into_iter()
            .map(|(case, document)| (case, document.to_string())),
    );
    rows
}

/// A well-formed policy document of exactly `bytes` bytes, padded through its `Sid`.
fn sized_policy_document(bytes: usize) -> String {
    let document = |padding: &str| {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{padding}\",\
             \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"arn:aws:iam::111122223333:root\"}},\
             \"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}}]}}"
        )
    };
    let unpadded = document("").len();
    assert!(
        bytes >= unpadded,
        "a policy document cannot be shorter than {unpadded} bytes"
    );
    let padded = document(&"A".repeat(bytes - unpadded));
    assert_eq!(
        padded.len(),
        bytes,
        "the padded document must be exactly {bytes} bytes"
    );
    padded
}

#[tokio::test]
async fn put_then_get_returns_the_stored_document_byte_for_byte() {
    let router = emulator();
    let mut revision_ids = BTreeSet::new();

    for (index, (case, document)) in well_formed_documents().into_iter().enumerate() {
        let name = format!("policy-roundtrip-{index}");
        create_app_monitor(&router, case, &name).await;

        let put = put_policy(&router, &name, &document, None).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected PutResourcePolicy to answer 200, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let put_document = put.member(case, "PolicyDocument");
        assert_eq!(
            put_document, document,
            "case {case:?}: expected PutResourcePolicy to report the document it was \
             given, got {put_document:?}"
        );
        let revision_id = put.member(case, "PolicyRevisionId");
        assert_modelled_revision_id(case, &revision_id);
        assert!(
            revision_ids.insert(revision_id.clone()),
            "case {case:?}: PolicyRevisionId {revision_id:?} was already minted for an \
             earlier put; each revision must be distinct"
        );

        // Round trip: the reader reports the same bytes and the same revision.
        let get = get_policy(&router, &name).await;
        assert_eq!(
            get.status,
            200,
            "case {case:?}: expected GetResourcePolicy to answer 200, got {} {:?}",
            get.status,
            String::from_utf8_lossy(&get.body)
        );
        let read_document = get.member(case, "PolicyDocument");
        assert_eq!(
            read_document, document,
            "case {case:?}: expected GetResourcePolicy to report the stored document \
             byte for byte, got {read_document:?}"
        );
        let read_revision = get.member(case, "PolicyRevisionId");
        assert_eq!(
            read_revision, revision_id,
            "case {case:?}: expected GetResourcePolicy to report the revision the put \
             minted ({revision_id:?}), got {read_revision:?}"
        );

        // A read must not change what the next reader sees.
        let again = get_policy(&router, &name).await;
        assert_eq!(
            again.member(case, "PolicyDocument"),
            document,
            "case {case:?}: a second GetResourcePolicy must report the same document"
        );
        assert_eq!(
            again.member(case, "PolicyRevisionId"),
            revision_id,
            "case {case:?}: a second GetResourcePolicy must report the same revision"
        );
    }
}

#[tokio::test]
async fn malformed_documents_are_refused_and_store_nothing() {
    let router = emulator();
    // One monitor that already holds a good policy, so each refusal can be checked to
    // have left the stored policy alone as well.
    let holder = "policy-malformed-holder";
    let good = well_formed_documents()
        .into_iter()
        .next()
        .expect("at least one well-formed document")
        .1;
    create_app_monitor(&router, "holder", holder).await;
    let stored_revision = put_policy(&router, holder, &good, None)
        .await
        .member("holder", "PolicyRevisionId");

    for (index, (case, document)) in malformed_documents().into_iter().enumerate() {
        let name = format!("policy-malformed-{index}");
        create_app_monitor(&router, case, &name).await;

        let put = put_policy(&router, &name, &document, None).await;
        assert_refused(&put, case, 400, "MalformedPolicyDocumentException");

        // Nothing was stored: the monitor still carries no policy at all.
        let get = get_policy(&router, &name).await;
        assert_refused(&get, case, 404, "PolicyNotFoundException");

        // And a refused put never replaces a policy that is already there.
        let onto_holder = put_policy(&router, holder, &document, None).await;
        assert_refused(&onto_holder, case, 400, "MalformedPolicyDocumentException");
        let held = get_policy(&router, holder).await;
        assert_eq!(
            held.member(case, "PolicyDocument"),
            good,
            "case {case:?}: a refused put must leave the stored document unchanged"
        );
        assert_eq!(
            held.member(case, "PolicyRevisionId"),
            stored_revision,
            "case {case:?}: a refused put must leave the stored revision unchanged"
        );
    }
}

#[tokio::test]
async fn documents_are_accepted_up_to_the_documented_size_limit() {
    let router = emulator();
    // Each published bound, and one byte past the maximum.
    let rows = [
        ("well under the limit", 256_usize, 200_u16),
        ("one byte under the limit", MAX_POLICY_DOCUMENT_BYTES - 1, 200),
        ("exactly at the limit", MAX_POLICY_DOCUMENT_BYTES, 200),
        ("one byte over the limit", MAX_POLICY_DOCUMENT_BYTES + 1, 400),
        ("far over the limit", MAX_POLICY_DOCUMENT_BYTES * 4, 400),
    ];

    for (index, (case, bytes, expected_status)) in rows.into_iter().enumerate() {
        let name = format!("policy-size-{index}");
        create_app_monitor(&router, case, &name).await;
        let document = sized_policy_document(bytes);

        let put = put_policy(&router, &name, &document, None).await;
        if expected_status == 200 {
            assert_eq!(
                put.status,
                200,
                "case {case:?} ({bytes} bytes): expected 200, got {} {:?}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
            let read = get_policy(&router, &name).await;
            assert_eq!(
                read.member(case, "PolicyDocument"),
                document,
                "case {case:?} ({bytes} bytes): expected the stored document to read back"
            );
        } else {
            assert_refused(&put, case, 400, "PolicySizeLimitExceededException");
            let read = get_policy(&router, &name).await;
            assert_refused(&read, case, 404, "PolicyNotFoundException");
        }
    }
}

#[tokio::test]
async fn a_conditional_put_succeeds_only_on_the_current_revision() {
    let router = emulator();
    let documents = well_formed_documents();

    for (index, (case, first)) in documents.iter().enumerate() {
        let name = format!("policy-revision-{index}");
        // Something to replace the first document with, so each row's second put really
        // changes the stored bytes.
        let second = &documents[(index + 1) % documents.len()].1;
        create_app_monitor(&router, case, &name).await;

        // A revision id named before any policy exists can never be the current one.
        let premature = put_policy(&router, &name, first, Some("00000000-0000-4000-8000-000000000000")).await;
        assert_refused(&premature, case, 400, "InvalidPolicyRevisionIdException");
        assert_refused(&get_policy(&router, &name).await, case, 404, "PolicyNotFoundException");

        // An unconditional put stores the first revision.
        let first_put = put_policy(&router, &name, first, None).await;
        let first_revision = first_put.member(case, "PolicyRevisionId");

        // A conditional put on the current revision succeeds and mints a NEW one.
        let second_put = put_policy(&router, &name, second, Some(&first_revision)).await;
        assert_eq!(
            second_put.status,
            200,
            "case {case:?}: a put naming the current revision must succeed, got {} {:?}",
            second_put.status,
            String::from_utf8_lossy(&second_put.body)
        );
        let second_revision = second_put.member(case, "PolicyRevisionId");
        assert_modelled_revision_id(case, &second_revision);
        assert_ne!(
            second_revision, first_revision,
            "case {case:?}: every accepted put must mint a new revision id, got \
             {second_revision:?} twice"
        );

        // The id the caller held is now stale, so the same call again is refused and the
        // stored policy is left alone.
        let stale = put_policy(&router, &name, first, Some(&first_revision)).await;
        assert_refused(&stale, case, 400, "InvalidPolicyRevisionIdException");
        let after_stale = get_policy(&router, &name).await;
        assert_eq!(
            after_stale.member(case, "PolicyDocument"),
            *second,
            "case {case:?}: a refused conditional put must leave the document unchanged"
        );
        assert_eq!(
            after_stale.member(case, "PolicyRevisionId"),
            second_revision,
            "case {case:?}: a refused conditional put must leave the revision unchanged"
        );

        // A put naming no revision is unconditional: it replaces whatever is there.
        let third_put = put_policy(&router, &name, first, None).await;
        let third_revision = third_put.member(case, "PolicyRevisionId");
        assert_ne!(
            third_revision, second_revision,
            "case {case:?}: an unconditional put must still mint a new revision"
        );
        assert_eq!(
            get_policy(&router, &name).await.member(case, "PolicyDocument"),
            *first,
            "case {case:?}: an unconditional put must replace the stored document"
        );
    }
}

#[tokio::test]
async fn delete_is_conditional_on_the_revision_and_then_reports_no_policy() {
    let router = emulator();
    let documents = well_formed_documents();

    // Row 0 deletes without naming a revision, row 1 deletes naming the current one:
    // both are accepted, and both leave the monitor with no policy.
    for (row, name_revision_on_delete) in [false, true].into_iter().enumerate() {
        for (index, (case, document)) in documents.iter().enumerate() {
            let name = format!("policy-delete-{row}-{index}");
            create_app_monitor(&router, case, &name).await;

            // Deleting before any policy exists is PolicyNotFoundException.
            assert_refused(
                &delete_policy(&router, &name, None).await,
                case,
                404,
                "PolicyNotFoundException",
            );

            let revision = put_policy(&router, &name, document, None)
                .await
                .member(case, "PolicyRevisionId");

            // A stale revision id refuses the delete and the policy stays readable.
            let stale = delete_policy(&router, &name, Some("00000000-0000-4000-8000-000000000000"))
                .await;
            assert_refused(&stale, case, 400, "InvalidPolicyRevisionIdException");
            assert_eq!(
                get_policy(&router, &name).await.member(case, "PolicyDocument"),
                *document,
                "case {case:?}: a refused delete must leave the policy in place"
            );

            let deleted = delete_policy(
                &router,
                &name,
                name_revision_on_delete.then_some(revision.as_str()),
            )
            .await;
            assert_eq!(
                deleted.status,
                200,
                "case {case:?}: expected DeleteResourcePolicy to answer 200, got {} {:?}",
                deleted.status,
                String::from_utf8_lossy(&deleted.body)
            );
            assert_eq!(
                deleted.member(case, "PolicyRevisionId"),
                revision,
                "case {case:?}: delete must report the revision of the policy it removed"
            );

            // The policy is gone: reading it, and deleting it again, both report that.
            assert_refused(
                &get_policy(&router, &name).await,
                case,
                404,
                "PolicyNotFoundException",
            );
            assert_refused(
                &delete_policy(&router, &name, None).await,
                case,
                404,
                "PolicyNotFoundException",
            );

            // And the monitor can take a fresh policy afterwards, under a new revision.
            let again = put_policy(&router, &name, document, None).await;
            let new_revision = again.member(case, "PolicyRevisionId");
            assert_ne!(
                new_revision, revision,
                "case {case:?}: a policy stored after a delete must carry a new revision"
            );
        }
    }
}

#[tokio::test]
async fn the_policy_operations_refuse_an_app_monitor_that_does_not_exist() {
    let router = emulator();
    let document = well_formed_documents()
        .into_iter()
        .next()
        .expect("at least one well-formed document")
        .1;

    // Both ways a name can name nothing: never created, and created then deleted.
    let deleted = "policy-missing-deleted";
    create_app_monitor(&router, "deleted", deleted).await;
    put_policy(&router, deleted, &document, None).await;
    let dropped = call(&router, "DELETE", &format!("/appmonitor/{deleted}"), None).await;
    assert_eq!(
        dropped.status,
        200,
        "expected DeleteAppMonitor to answer 200, got {} {:?}",
        dropped.status,
        String::from_utf8_lossy(&dropped.body)
    );

    for name in ["policy-missing-never-created", deleted] {
        let case = format!("name={name}");
        assert_refused(
            &put_policy(&router, name, &document, None).await,
            &case,
            404,
            "ResourceNotFoundException",
        );
        assert_refused(
            &get_policy(&router, name).await,
            &case,
            404,
            "ResourceNotFoundException",
        );
        assert_refused(
            &delete_policy(&router, name, None).await,
            &case,
            404,
            "ResourceNotFoundException",
        );
    }

    // A monitor recreated under a name that once had a policy starts with none: the
    // policy belongs to the resource, not to the name.
    create_app_monitor(&router, "recreated", deleted).await;
    assert_refused(
        &get_policy(&router, deleted).await,
        "recreated",
        404,
        "PolicyNotFoundException",
    );
}

#[tokio::test]
async fn each_app_monitor_carries_its_own_policy() {
    let router = emulator();
    let documents = well_formed_documents();
    let mut names = Vec::new();

    for (index, (case, document)) in documents.iter().enumerate() {
        let name = format!("policy-isolated-{index}");
        create_app_monitor(&router, case, &name).await;
        let revision = put_policy(&router, &name, document, None)
            .await
            .member(case, "PolicyRevisionId");
        names.push((name, document.clone(), revision));
    }

    // Every monitor still reports its OWN document and revision after all the others
    // were written.
    for (name, document, revision) in &names {
        let case = format!("name={name}");
        let read = get_policy(&router, name).await;
        assert_eq!(
            read.member(&case, "PolicyDocument"),
            *document,
            "case {case:?}: expected each monitor to report its own policy document"
        );
        assert_eq!(
            read.member(&case, "PolicyRevisionId"),
            *revision,
            "case {case:?}: expected each monitor to report its own policy revision"
        );
    }

    // Deleting one monitor's policy leaves every other one untouched.
    let (first_name, _, _) = &names[0];
    delete_policy(&router, first_name, None).await;
    for (name, document, _) in names.iter().skip(1) {
        let case = format!("name={name}");
        assert_eq!(
            get_policy(&router, name).await.member(&case, "PolicyDocument"),
            *document,
            "case {case:?}: deleting one monitor's policy must not touch another's"
        );
    }
}
