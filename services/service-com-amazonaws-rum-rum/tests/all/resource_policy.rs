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
//! documented 4 KB bound and one byte past it; the layout table pins that a document is
//! stored as its CONTENT (the same policy written in any layout reads back as one
//! canonical text, while whitespace inside a string value is content and survives); and
//! the revision tests assert the conditional-request invariant (a put or delete that
//! names a revision id succeeds only on the current one, and a refused one leaves the
//! stored policy untouched), the stickiness of an ASSIGNED revision id (once a write has
//! assigned one, a put that does not provide the correct current id is refused, whether
//! it names a stale id, a foreign one or none at all, while the delete's weaker rule
//! stays conditional only on the id it provides), and the revision-counter rule (the id a
//! write reports is the policy revision's number, 1 for the first revision an app monitor
//! stores and one more for every later write) over rows that differ in how many writes
//! they make and in how each one is made.

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

/// The `PolicyRevisionId` the FIRST policy revision stored on an app monitor carries.
/// Source: recorded RUM behaviour — the first policy written to an app monitor comes
/// back with `PolicyRevisionId` `1` and the next successful write on the same monitor
/// returns `2`, i.e. the revision id is the revision's number, which is what makes an
/// optimistic-concurrency round trip orderable.
const FIRST_POLICY_REVISION_ID: &str = "1";

/// The message `PutResourcePolicy` answers `InvalidPolicyRevisionIdException` with, for
/// every request of that operation its optimistic-lock check refuses — including one that
/// provided no `PolicyRevisionId` at all after one had been assigned.
/// Source: recorded RUM behaviour — wholly fixed text, carrying no part of the request.
/// The wording of each operation's refusal is pinned over a table of inputs in
/// `policy_revision_refusal_wording.rs`.
const PUT_POLICY_REVISION_MISMATCH_MESSAGE: &str =
    "If policyRevisionID is provided, then it must match the existing revision.";

/// The message `DeleteResourcePolicy` answers the same error class with: recorded RUM
/// behaviour again, worded differently from the put's and with no trailing period.
const DELETE_POLICY_REVISION_MISMATCH_MESSAGE: &str =
    "PolicyRevisionID must match the existing revision";

/// A syntactically valid `PolicyRevisionId` that no revision of any policy here carries.
const FOREIGN_POLICY_REVISION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The message RUM answers `PolicyNotFoundException` with.
/// Source: recorded RUM behaviour — a 404 for a missing resource policy carries this
/// constant text, with nothing from the request in it; the shape models `message` and
/// nothing else (`smithy_query(query="shape", shape="PolicyNotFoundException")`).
const POLICY_NOT_FOUND_MESSAGE: &str = "Resource policy does not exist";

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

/// Assert a refusal reports the error CLASS alone: the body carries `__type` and no other
/// member, so a caller gets the code with no message beside it.
///
/// Source: recorded RUM behaviour for `MalformedPolicyDocumentException` — the body has no
/// `message` member at all, although the model declares one.
fn assert_reports_the_class_alone(reply: &Reply, case: &str) {
    let document = reply.json(case);
    let members: Vec<String> = document
        .as_object()
        .map(|body| {
            body.keys()
                .filter(|key| *key != "__type")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    assert!(
        members.is_empty(),
        "case {case:?}: expected a body carrying the error class alone, got the \
         member(s) {members:?} in {:?}",
        String::from_utf8_lossy(&reply.body)
    );
}

/// The canonical form of a policy document: the text with every whitespace character
/// that falls BETWEEN JSON tokens removed, and everything inside a string literal kept
/// exactly as written.
///
/// This restates the rule the service follows — a `PolicyDocument` is parsed and stored
/// as its content, so the layout the caller chose (indentation, the spaces a
/// pretty-printer puts after `:` and `,`, a trailing newline) is not part of what is
/// stored, while the elements, their order and their values are. Recorded RUM behaviour:
/// a put of a document written with those spaces answers the same policy without them.
fn canonical_form(document: &str) -> String {
    let mut canonical = String::with_capacity(document.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            canonical.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
            canonical.push(character);
        } else if !character.is_whitespace() {
            canonical.push(character);
        }
    }
    canonical
}

/// Assert `answered` is the document `submitted` was: the same JSON content, in the
/// canonical form the service stores.
fn assert_stored_document(case: &str, submitted: &str, answered: &str) {
    let expected = canonical_form(submitted);
    assert_eq!(
        answered, expected,
        "case {case:?}: expected the submitted document's canonical text \
         {expected:?}, got {answered:?}"
    );
    // Nothing but the layout changed: the answer parses to the very same JSON value as
    // the text the caller submitted — same elements, same order, same values.
    let answered_value: Value = serde_json::from_str(answered)
        .unwrap_or_else(|e| panic!("case {case:?}: the answered document must parse: {e}"));
    let submitted_value: Value = serde_json::from_str(submitted)
        .unwrap_or_else(|e| panic!("case {case:?}: the submitted document must parse: {e}"));
    assert_eq!(
        answered_value, submitted_value,
        "case {case:?}: the answered document must carry the submitted document's \
         content, got {answered:?}"
    );
}

/// A `PolicyRevisionId` the service minted: a revision NUMBER, rendered in decimal with
/// no padding and at least 1, inside the modelled length bounds.
fn assert_modelled_revision_id(case: &str, revision_id: &str) {
    let (min, max) = POLICY_REVISION_ID_BOUNDS;
    assert!(
        revision_id.len() >= min && revision_id.len() <= max,
        "case {case:?}: expected a PolicyRevisionId of {min}..{max} characters, \
         got {revision_id:?} ({} characters)",
        revision_id.len()
    );
    let number = revision_id.parse::<u64>();
    assert!(
        number
            .as_ref()
            .is_ok_and(|number| *number >= 1 && number.to_string() == revision_id),
        "case {case:?}: expected a PolicyRevisionId that is a revision number in \
         decimal, got {revision_id:?}"
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
            // Indented, multi-line text: the layout is the caller's, so the stored
            // document comes back as the same policy in canonical form.
            "pretty-printed-text",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Effect\": \"Allow\",\n      \"Principal\": {\"AWS\": \"*\"},\n      \
             \"Action\": \"rum:PutRumEvents\"\n    }\n  ]\n}\n"
                .to_string(),
        ),
        (
            // Non-ASCII text inside the document: it is string content, so it survives
            // unchanged.
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
async fn put_then_get_returns_the_stored_document_as_its_canonical_content() {
    let router = emulator();

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
        assert_stored_document(case, &document, &put_document);
        let revision_id = put.member(case, "PolicyRevisionId");
        assert_modelled_revision_id(case, &revision_id);
        // Each row writes to a FRESH app monitor, so each one reports that monitor's
        // first policy revision — the revision belongs to the monitor's policy, not to
        // the service as a whole.
        assert_eq!(
            revision_id, FIRST_POLICY_REVISION_ID,
            "case {case:?}: expected the first policy stored on an app monitor to be \
             revision {FIRST_POLICY_REVISION_ID}, got {revision_id:?}"
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
        assert_stored_document(case, &document, &read_document);
        assert_eq!(
            read_document, put_document,
            "case {case:?}: expected GetResourcePolicy to report the document the put \
             stored, got {read_document:?}"
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
            put_document,
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
        assert_reports_the_class_alone(&put, case);

        // Nothing was stored: the monitor still carries no policy at all.
        let get = get_policy(&router, &name).await;
        assert_refused(&get, case, 404, "PolicyNotFoundException");

        // And a refused put never replaces a policy that is already there.
        let onto_holder = put_policy(&router, holder, &document, None).await;
        assert_refused(&onto_holder, case, 400, "MalformedPolicyDocumentException");
        assert_reports_the_class_alone(&onto_holder, case);
        let held = get_policy(&router, holder).await;
        assert_stored_document(case, &good, &held.member(case, "PolicyDocument"));
        assert_eq!(
            held.member(case, "PolicyRevisionId"),
            stored_revision,
            "case {case:?}: a refused put must leave the stored revision unchanged"
        );
    }
}

#[tokio::test]
async fn a_documents_layout_is_not_part_of_what_is_stored() {
    let router = emulator();

    // ONE policy, written eight ways. The rows differ only in whitespace BETWEEN JSON
    // tokens — the layout a caller's printer chose — so every one of them must read back
    // as the same canonical text. The `Sid` carries spaces INSIDE a string value, which
    // is content, so it must survive in every row.
    let canonical = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Allow One Account","Effect":"Allow","Principal":"*","Action":"rum:PutRumEvents"}]}"#;
    let layouts: Vec<(&str, String)> = vec![
        ("already-minified", canonical.to_string()),
        (
            "spaces-after-the-separators",
            r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Allow One Account", "Effect": "Allow", "Principal": "*", "Action": "rum:PutRumEvents"}]}"#.to_string(),
        ),
        (
            "spaces-around-every-token",
            r#"{ "Version" : "2012-10-17" , "Statement" : [ { "Sid" : "Allow One Account" , "Effect" : "Allow" , "Principal" : "*" , "Action" : "rum:PutRumEvents" } ] }"#.to_string(),
        ),
        (
            "indented-two-spaces",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Sid\": \"Allow One Account\",\n      \"Effect\": \"Allow\",\n      \
             \"Principal\": \"*\",\n      \"Action\": \"rum:PutRumEvents\"\n    }\n  ]\n}"
                .to_string(),
        ),
        (
            "tab-indented",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\
             \"Sid\": \"Allow One Account\",\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\
             \"Principal\": \"*\",\n\t\t\t\"Action\": \"rum:PutRumEvents\"\n\t\t}\n\t]\n}"
                .to_string(),
        ),
        (
            "crlf-line-endings",
            "{\r\n\"Version\": \"2012-10-17\",\r\n\"Statement\": [{\r\n\"Sid\": \
             \"Allow One Account\",\r\n\"Effect\": \"Allow\",\r\n\"Principal\": \"*\",\
             \r\n\"Action\": \"rum:PutRumEvents\"\r\n}]\r\n}"
                .to_string(),
        ),
        (
            "leading-and-trailing-whitespace",
            format!("\n\t  {canonical}  \n"),
        ),
        (
            "newlines-inside-the-brackets",
            format!(
                "{{\n{}\n}}",
                r#""Version":"2012-10-17",
"Statement":
[
{"Sid":"Allow One Account","Effect":"Allow","Principal":"*","Action":"rum:PutRumEvents"}
]"#
            ),
        ),
    ];

    for (index, (case, submitted)) in layouts.iter().enumerate() {
        let name = format!("policy-layout-{index}");
        create_app_monitor(&router, case, &name).await;

        let put = put_policy(&router, &name, submitted, None).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected PutResourcePolicy to answer 200, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let answered = put.member(case, "PolicyDocument");
        assert_stored_document(case, submitted, &answered);
        assert_eq!(
            answered, canonical,
            "case {case:?}: every layout of one policy must be stored as the same \
             canonical text, got {answered:?}"
        );
        // Whitespace inside a string value is content, not layout.
        assert!(
            answered.contains("Allow One Account"),
            "case {case:?}: the Sid's own spaces must survive, got {answered:?}"
        );

        let read = get_policy(&router, &name).await.member(case, "PolicyDocument");
        assert_eq!(
            read, canonical,
            "case {case:?}: expected GetResourcePolicy to report the canonical text, \
             got {read:?}"
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
            assert_stored_document(case, &document, &read.member(case, "PolicyDocument"));
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
        let premature =
            put_policy(&router, &name, first, Some(FOREIGN_POLICY_REVISION_ID)).await;
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
        assert_stored_document(case, second, &after_stale.member(case, "PolicyDocument"));
        assert_eq!(
            after_stale.member(case, "PolicyRevisionId"),
            second_revision,
            "case {case:?}: a refused conditional put must leave the revision unchanged"
        );

        // A revision id has now been assigned to this policy, so a put that provides
        // none does not provide the correct current one either: it is refused and the
        // stored policy is left alone.
        let unconditional = put_policy(&router, &name, first, None).await;
        assert_refused(
            &unconditional,
            case,
            400,
            "InvalidPolicyRevisionIdException",
        );
        let after_unconditional = get_policy(&router, &name).await;
        assert_stored_document(
            case,
            second,
            &after_unconditional.member(case, "PolicyDocument"),
        );
        assert_eq!(
            after_unconditional.member(case, "PolicyRevisionId"),
            second_revision,
            "case {case:?}: a put refused for not providing the current revision must \
             leave the revision unchanged"
        );

        // The read-revision-then-write loop still goes through: a put naming the
        // revision now in force is accepted and mints the next one.
        let third_put = put_policy(&router, &name, first, Some(&second_revision)).await;
        let third_revision = third_put.member(case, "PolicyRevisionId");
        assert_ne!(
            third_revision, second_revision,
            "case {case:?}: an accepted put must mint a new revision"
        );
        assert_stored_document(
            case,
            first,
            &get_policy(&router, &name)
                .await
                .member(case, "PolicyDocument"),
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
            let stale = delete_policy(&router, &name, Some(FOREIGN_POLICY_REVISION_ID)).await;
            assert_refused(&stale, case, 400, "InvalidPolicyRevisionIdException");
            assert_stored_document(
                case,
                document,
                &get_policy(&router, &name)
                    .await
                    .member(case, "PolicyDocument"),
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
            let gone = get_policy(&router, &name).await;
            assert_refused(&gone, case, 404, "PolicyNotFoundException");
            let message = gone.member(case, "message");
            assert_eq!(
                message, POLICY_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the PolicyNotFoundException message \
                 {POLICY_NOT_FOUND_MESSAGE:?}, got {message:?}"
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

/// How one write of a revision-counter row reaches the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// A put naming no `PolicyRevisionId`. Accepted only while no revision id has been
    /// assigned to the policy: on the monitor's first policy, and on one written after a
    /// delete took the previous policy — with its assigned id — away.
    Unconditional,
    /// A put naming the revision the previous write reported — the documented
    /// read-revision-then-write loop.
    Conditional,
    /// A delete of the policy in force (naming its revision), then a put: the monitor
    /// carries no policy in between.
    DeleteThenPut,
}

#[tokio::test]
async fn revision_ids_count_the_writes_to_each_app_monitors_policy() {
    let router = emulator();
    let documents = well_formed_documents();

    // Rows differ in how many writes they make and in how each write is made: naming no
    // revision, naming the one just read, or coming after the policy was deleted. Every
    // row drives its own app monitor, so every row's first write is revision 1 and each
    // later write is one more, whichever way it was made.
    let rows: Vec<(&str, Vec<Write>)> = vec![
        ("one unconditional write", vec![Write::Unconditional]),
        (
            "the read-revision-then-write loop",
            vec![Write::Unconditional, Write::Conditional, Write::Conditional],
        ),
        (
            "unconditional replacements",
            vec![Write::Unconditional; 4],
        ),
        (
            "a write after the policy was deleted",
            vec![
                Write::Unconditional,
                Write::DeleteThenPut,
                Write::Conditional,
                Write::DeleteThenPut,
            ],
        ),
        (
            "twelve writes cycling through all three ways of writing",
            (0..12)
                .map(|step| match step % 3 {
                    0 => Write::Unconditional,
                    1 => Write::Conditional,
                    _ => Write::DeleteThenPut,
                })
                .collect(),
        ),
    ];

    for (row, (row_case, writes)) in rows.iter().enumerate() {
        let name = format!("policy-revision-counter-{row}");
        create_app_monitor(&router, row_case, &name).await;

        // The number of writes this monitor's policy has taken, which is what the
        // revision id must report, and the revision currently in force.
        let mut written = 0_u64;
        let mut in_force: Option<String> = None;

        for (step, mode) in writes.iter().enumerate() {
            let case = format!("{row_case}: step {step} ({mode:?})");
            let document = &documents[(row + step) % documents.len()].1;

            if *mode == Write::DeleteThenPut {
                let held = in_force
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: no policy to delete"));
                let deleted = delete_policy(&router, &name, Some(&held)).await;
                assert_eq!(
                    deleted.status,
                    200,
                    "case {case:?}: a delete naming the current revision must succeed, \
                     got {} {:?}",
                    deleted.status,
                    String::from_utf8_lossy(&deleted.body)
                );
                let reported = deleted.member(&case, "PolicyRevisionId");
                assert_eq!(
                    reported, held,
                    "case {case:?}: delete must report the revision it removed ({held:?}), \
                     got {reported:?}"
                );
                in_force = None;
            }

            let named = match mode {
                Write::Conditional => in_force.clone(),
                Write::Unconditional | Write::DeleteThenPut => None,
            };
            let put = put_policy(&router, &name, document, named.as_deref()).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: expected PutResourcePolicy to answer 200, got {} {:?}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );

            written += 1;
            let expected = written.to_string();
            let reported = put.member(&case, "PolicyRevisionId");
            assert_modelled_revision_id(&case, &reported);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected write number {written} to report revision \
                 {expected:?}, got {reported:?}"
            );

            // The reader reports the very revision the write stored.
            let read = get_policy(&router, &name).await;
            let read_revision = read.member(&case, "PolicyRevisionId");
            assert_eq!(
                read_revision, expected,
                "case {case:?}: expected GetResourcePolicy to report revision \
                 {expected:?}, got {read_revision:?}"
            );
            in_force = Some(reported);
        }
    }

    // Revisions are per app monitor and are NOT shared: a monitor written once still
    // reports revision 1 while another has counted many writes, and the id one monitor
    // holds does not satisfy another's conditional write.
    let fresh = "policy-revision-counter-fresh";
    create_app_monitor(&router, "fresh monitor", fresh).await;
    let first = put_policy(&router, fresh, &documents[0].1, None).await;
    let fresh_revision = first.member("fresh monitor", "PolicyRevisionId");
    assert_eq!(
        fresh_revision, FIRST_POLICY_REVISION_ID,
        "case \"fresh monitor\": expected revision {FIRST_POLICY_REVISION_ID} on a \
         monitor's first policy, got {fresh_revision:?}"
    );
    let busy = "policy-revision-counter-4";
    let busy_revision = get_policy(&router, busy)
        .await
        .member("busy monitor", "PolicyRevisionId");
    assert_ne!(
        busy_revision, fresh_revision,
        "case \"busy monitor\": a monitor that took many writes must not report the \
         same revision as one that took a single write"
    );
    let crossed = put_policy(&router, busy, &documents[1].1, Some(&fresh_revision)).await;
    assert_refused(
        &crossed,
        "cross-monitor revision",
        400,
        "InvalidPolicyRevisionIdException",
    );
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
        assert_stored_document(&case, document, &read.member(&case, "PolicyDocument"));
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
        assert_stored_document(
            &case,
            document,
            &get_policy(&router, name).await.member(&case, "PolicyDocument"),
        );
    }
}

/// What a request names as its `PolicyRevisionId`, as a class of value rather than a
/// literal: the follow-up requests below differ along this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The member is absent from the request.
    Nothing,
    /// The revision now in force — the only value that IS the correct current one.
    Current,
    /// A revision this policy carried before the write now in force.
    Stale,
    /// A well-formed id no revision of this policy ever carried.
    Foreign,
}

impl Named {
    /// The value this class names, given the revision in force and an earlier one.
    fn value(self, in_force: &str, stale: &str) -> Option<String> {
        match self {
            Named::Nothing => None,
            Named::Current => Some(in_force.to_string()),
            Named::Stale => Some(stale.to_string()),
            Named::Foreign => Some(FOREIGN_POLICY_REVISION_ID.to_string()),
        }
    }
}

#[tokio::test]
async fn a_put_is_refused_unless_it_provides_the_assigned_revision() {
    let router = emulator();
    let documents = well_formed_documents();

    // Every row first writes a policy and then ASSIGNS a revision id to it by writing it
    // again with its `PolicyRevisionId`. The rows then differ in what the next put names
    // — nothing, the current id, a stale one, a foreign one — and in how many assigning
    // writes came before it, because the assignment is a property of the policy and not
    // of the one write that made it. Only naming the current revision is accepted.
    let rows = [
        ("no revision id at all", Named::Nothing, 1, false),
        ("a stale revision id", Named::Stale, 1, false),
        ("a foreign revision id", Named::Foreign, 1, false),
        ("the current revision id", Named::Current, 1, true),
        ("no revision id, three writes later", Named::Nothing, 3, false),
        ("a stale revision id, three writes later", Named::Stale, 3, false),
        (
            "the current revision id, three writes later",
            Named::Current,
            3,
            true,
        ),
    ];

    for (row, (case, named, assigning_writes, accepted)) in rows.into_iter().enumerate() {
        let name = format!("policy-assigned-revision-{row}");
        create_app_monitor(&router, case, &name).await;

        // The monitor's first policy: no revision id has been assigned yet, so a put that
        // provides none is accepted.
        let mut held = documents[row % documents.len()].1.clone();
        let first = put_policy(&router, &name, &held, None).await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: the first policy of a monitor must be storable without a \
             revision id, got {} {:?}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let mut in_force = first.member(case, "PolicyRevisionId");
        let mut stale = in_force.clone();

        // Each of these writes provides the revision in force, which is what assigns a
        // revision id to the policy.
        for step in 0..assigning_writes {
            let document = documents[(row + step + 1) % documents.len()].1.clone();
            let assigning = put_policy(&router, &name, &document, Some(&in_force)).await;
            assert_eq!(
                assigning.status,
                200,
                "case {case:?}: write {step} naming the current revision must succeed, \
                 got {} {:?}",
                assigning.status,
                String::from_utf8_lossy(&assigning.body)
            );
            stale = in_force;
            in_force = assigning.member(case, "PolicyRevisionId");
            held = document;
        }

        // A document that differs from the one in force, so a put that is wrongly
        // accepted shows up in what the reader reports.
        let replacement = documents[(row + assigning_writes + 1) % documents.len()]
            .1
            .clone();
        assert_ne!(
            canonical_form(&replacement),
            canonical_form(&held),
            "case {case:?}: the follow-up put must submit a different document"
        );

        let follow_up =
            put_policy(&router, &name, &replacement, named.value(&in_force, &stale).as_deref())
                .await;
        let read = get_policy(&router, &name).await;
        if accepted {
            assert_eq!(
                follow_up.status,
                200,
                "case {case:?}: a put providing the current revision must succeed, \
                 got {} {:?}",
                follow_up.status,
                String::from_utf8_lossy(&follow_up.body)
            );
            let minted = follow_up.member(case, "PolicyRevisionId");
            assert_modelled_revision_id(case, &minted);
            assert_ne!(
                minted, in_force,
                "case {case:?}: an accepted put must mint a new revision id"
            );
            assert_stored_document(case, &replacement, &read.member(case, "PolicyDocument"));
            assert_eq!(
                read.member(case, "PolicyRevisionId"),
                minted,
                "case {case:?}: the reader must report the revision the put minted"
            );
        } else {
            assert_refused(&follow_up, case, 400, "InvalidPolicyRevisionIdException");
            let message = follow_up.member(case, "message");
            assert_eq!(
                message, PUT_POLICY_REVISION_MISMATCH_MESSAGE,
                "case {case:?}: expected the message \
                 {PUT_POLICY_REVISION_MISMATCH_MESSAGE:?}, got {message:?}"
            );
            // Nothing was written: the policy and the revision in force are untouched.
            assert_stored_document(case, &held, &read.member(case, "PolicyDocument"));
            assert_eq!(
                read.member(case, "PolicyRevisionId"),
                in_force,
                "case {case:?}: a refused put must leave the revision in force, expected \
                 {in_force:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_stays_conditional_only_on_the_revision_it_provides() {
    let router = emulator();
    let documents = well_formed_documents();

    // The delete's member conditions the refusal on the id "that you provide", so an
    // assigned revision id does not make a delete that provides none refusable. Rows
    // differ in what the delete names; after each accepted delete the monitor carries no
    // policy, so a put that provides no revision id is storable again.
    let rows = [
        ("delete naming no revision", Named::Nothing, true),
        ("delete naming the current revision", Named::Current, true),
        ("delete naming a stale revision", Named::Stale, false),
        ("delete naming a foreign revision", Named::Foreign, false),
    ];

    for (row, (case, named, accepted)) in rows.into_iter().enumerate() {
        let name = format!("policy-assigned-revision-delete-{row}");
        create_app_monitor(&router, case, &name).await;

        let document = documents[row % documents.len()].1.clone();
        let stale = put_policy(&router, &name, &document, None)
            .await
            .member(case, "PolicyRevisionId");
        // Assign a revision id to the policy.
        let in_force = put_policy(&router, &name, &document, Some(&stale))
            .await
            .member(case, "PolicyRevisionId");

        let deleted = delete_policy(&router, &name, named.value(&in_force, &stale).as_deref()).await;
        if !accepted {
            assert_refused(&deleted, case, 400, "InvalidPolicyRevisionIdException");
            let message = deleted.member(case, "message");
            assert_eq!(
                message, DELETE_POLICY_REVISION_MISMATCH_MESSAGE,
                "case {case:?}: expected the message \
                 {DELETE_POLICY_REVISION_MISMATCH_MESSAGE:?}, got {message:?}"
            );
            assert_stored_document(
                case,
                &document,
                &get_policy(&router, &name)
                    .await
                    .member(case, "PolicyDocument"),
            );
            continue;
        }
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected DeleteResourcePolicy to answer 200, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert_eq!(
            deleted.member(case, "PolicyRevisionId"),
            in_force,
            "case {case:?}: delete must report the revision it removed"
        );
        assert_refused(
            &get_policy(&router, &name).await,
            case,
            404,
            "PolicyNotFoundException",
        );

        // The assigned revision id went away with the policy it belonged to, so the
        // monitor's next policy is storable without providing one.
        let replacement = documents[(row + 1) % documents.len()].1.clone();
        let again = put_policy(&router, &name, &replacement, None).await;
        assert_eq!(
            again.status,
            200,
            "case {case:?}: a policy stored after the previous one was deleted must not \
             require a revision id, got {} {:?}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert_stored_document(
            case,
            &replacement,
            &get_policy(&router, &name)
                .await
                .member(case, "PolicyDocument"),
        );
    }
}
