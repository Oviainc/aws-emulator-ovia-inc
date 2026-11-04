// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the pipeline resource-policy operations — `PutResourcePolicy`,
//! `GetResourcePolicy` and `DeleteResourcePolicy` — driven the way a client drives them:
//! one HTTP request per call through the same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test is that a pipeline carries ONE resource-based policy document on
//! its own record: a write replaces it with the submitted document verbatim when that
//! document is a JSON object, every read answers the stored document — and the EMPTY JSON
//! document while nothing is attached, because a pipeline that exists always has one — a
//! removal leaves the pipeline with the empty document whether or not it carried a policy,
//! an ARN naming no pipeline is refused 404 by all three, and a document that is not a
//! JSON object is refused 400 with nothing attached.
//!
//! The rows of each table differ along what the rule generalises over: the document's
//! size (both published `@length` bounds and one byte past each), its nesting, its
//! character set, its key order, and — for the refusals — the way the document fails to
//! be a JSON object.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so nothing under test can depend on a
/// default-region code path.
const REGION: &str = "ap-southeast-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than the service's own transition span, so a pipeline is settled
/// before the policy calls run.
const PAST_TRANSITION: Duration =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN
        .saturating_add(Duration::from_millis(400));

/// `ResourcePolicy`'s published `@length` bounds (smithy_query: operation
/// PutResourcePolicy, member `Policy` — `len 2..204800`).
const POLICY_MIN_LENGTH: usize = 2;
const POLICY_MAX_LENGTH: usize = 204_800;

/// The document a pipeline nothing is attached to has: the empty JSON object.
const EMPTY_DOCUMENT: &str = "{}";

/// What one call answered: the status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol may render
    /// the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }

    /// The `Policy` member this answer carries, or `None` when it carries none.
    fn policy(&self) -> Option<String> {
        self.body
            .get("Policy")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `ResourceArn` member this answer carries, or `None` when it carries none.
    fn resource_arn(&self) -> Option<String> {
        self.body
            .get("ResourceArn")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `ResourceArn` is an `@httpLabel` on a single path segment, so an ARN's `:` and `/` have
/// to travel as `%3A` and `%2F` or the request addresses a different route.
fn encode_label(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The ARN of a pipeline in the signed region, as `CreatePipeline` mints it.
fn arn_of(pipeline_name: &str) -> String {
    format!("arn:aws:osis:{REGION}:{ACCOUNT}:pipeline/{pipeline_name}")
}

fn policy_path(arn: &str) -> String {
    format!("/2022-01-01/osis/resourcePolicy/{}", encode_label(arn))
}

async fn put_policy(router: &MultiServiceRouter, arn: &str, policy: &str) -> Answer {
    call(
        router,
        "PUT",
        &policy_path(arn),
        Some(json!({ "Policy": policy })),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &policy_path(arn), None).await
}

async fn delete_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "DELETE", &policy_path(arn), None).await
}

async fn delete_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2022-01-01/osis/deletePipeline/{name}"),
        None,
    )
    .await
}

/// Create a pipeline and let the `CREATING` -> `ACTIVE` transition the clock owes come
/// due, so the policy cases are not about a pipeline's status.
async fn settled_pipeline(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(json!({
            "PipelineName": name,
            "MinUnits": 1,
            "MaxUnits": 2,
            "PipelineConfigurationBody": "version: \"2\"\npolicy-pipeline:\n  source:\n    http:\n",
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    arn_of(name)
}

/// A JSON object document of exactly `length` bytes: `{"k":"<padding>"}`.
fn document_of_length(length: usize) -> String {
    let envelope = r#"{"k":""#.len() + r#""}"#.len();
    assert!(
        length >= envelope,
        "a {length}-byte object document cannot be built: the envelope alone is {envelope} bytes"
    );
    format!(r#"{{"k":"{}"}}"#, "p".repeat(length - envelope))
}

/// Valid policy documents: each row differs in what the rule generalises over — size
/// (including the published minimum and maximum), nesting, character set and key order.
fn valid_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "the shortest object, which is the published length minimum",
            "{}".to_string(),
        ),
        (
            "an IAM-style statement list",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                    "Action": "osis:Ingest",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "keys in an order a service must not reorder",
            r#"{"Statement":[],"Version":"2012-10-17"}"#.to_string(),
        ),
        (
            "nested objects, arrays and non-string scalars",
            r#"{"a":{"b":[1,2.5,true,null,{"c":[]}]},"d":{}}"#.to_string(),
        ),
        (
            "non-ASCII characters and an escape sequence",
            r#"{"note":"ingesti\u00f3n — \"quoted\"","emoji":"🚰"}"#.to_string(),
        ),
        (
            "insignificant whitespace and newlines the service stores as given",
            "{\n  \"Version\" : \"2012-10-17\" ,\n  \"Statement\" : [ ]\n}\n".to_string(),
        ),
        (
            // One byte past the minimum: at this size an object can only be the empty one
            // plus a byte of insignificant whitespace.
            "one byte past the published length minimum",
            "{ }".to_string(),
        ),
        (
            "exactly the published length maximum",
            document_of_length(POLICY_MAX_LENGTH),
        ),
    ]
}

/// Documents that are not a JSON policy document: each row fails to be a JSON object in a
/// different way.
const NON_OBJECT_DOCUMENTS: &[(&str, &str)] = &[
    ("an unterminated object", "{this is not valid json"),
    ("a truncated member", r#"{"Version":}"#),
    ("a trailing comma", r#"{"Version":"2012-10-17",}"#),
    ("a JSON array", r#"["Allow"]"#),
    ("a JSON string", r#""Allow""#),
    ("a JSON number", "12"),
    ("the JSON null literal", "null"),
    ("two documents in one", "{}{}"),
    ("YAML rather than JSON", "Version: 2012-10-17"),
];

#[tokio::test]
async fn policy_write_is_read_back_byte_for_byte() {
    let router = new_router();
    for (index, (case, document)) in valid_documents().into_iter().enumerate() {
        let name = format!("policy-pipeline-{index}");
        let arn = settled_pipeline(&router, case, &name).await;

        let written = put_policy(&router, &arn, &document).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected 200 from PutResourcePolicy, got {} / {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.policy().as_deref(),
            Some(document.as_str()),
            "case {case:?}: PutResourcePolicy must answer the document it stored, got {:?}",
            written.policy()
        );
        assert_eq!(
            written.resource_arn().as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the pipeline ARN {arn:?}, got {:?}",
            written.resource_arn()
        );

        let read = get_policy(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetResourcePolicy, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.policy().as_deref(),
            Some(document.as_str()),
            "case {case:?}: GetResourcePolicy must answer the stored document, got {:?}",
            read.policy()
        );
        assert_eq!(
            read.resource_arn().as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the pipeline ARN {arn:?}, got {:?}",
            read.resource_arn()
        );
    }
}

#[tokio::test]
async fn a_write_replaces_the_document_the_pipeline_carried() {
    let router = new_router();
    let case = "every valid document written over the previous one";
    let arn = settled_pipeline(&router, case, "policy-replacement").await;

    let mut previous: Option<String> = None;
    for (row, document) in valid_documents() {
        let written = put_policy(&router, &arn, &document).await;
        assert_eq!(
            written.status, 200,
            "case {row:?}: expected 200 from PutResourcePolicy, got {} / {:?}",
            written.status, written.body
        );
        let read = get_policy(&router, &arn).await;
        assert_eq!(
            read.policy().as_deref(),
            Some(document.as_str()),
            "case {row:?}: one policy per resource, so the write replaces {previous:?}; got {:?}",
            read.policy()
        );
        previous = Some(document);
    }

    // Writing the same document twice leaves the same state: the operation is
    // `@idempotent`.
    let document = previous.expect("the table is not empty");
    let repeated = put_policy(&router, &arn, &document).await;
    assert_eq!(
        repeated.status, 200,
        "case {case:?}: a repeated write must be accepted, got {} / {:?}",
        repeated.status, repeated.body
    );
    assert_eq!(
        get_policy(&router, &arn).await.policy().as_deref(),
        Some(document.as_str()),
        "case {case:?}: a repeated write must leave the same document"
    );
}

#[tokio::test]
async fn a_pipeline_with_no_policy_has_the_empty_document() {
    let router = new_router();
    let arn = settled_pipeline(&router, "a pipeline nobody wrote a policy to", "policy-absent")
        .await;

    for case in ["before any write", "after the policy was removed"] {
        if case == "after the policy was removed" {
            let written = put_policy(&router, &arn, r#"{"Version":"2012-10-17"}"#).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from PutResourcePolicy, got {} / {:?}",
                written.status, written.body
            );
            let removed = delete_policy(&router, &arn).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: expected 200 from DeleteResourcePolicy, got {} / {:?}",
                removed.status, removed.body
            );
        }

        let read = get_policy(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: an unattached policy is not a not-found resource, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.policy().as_deref(),
            Some(EMPTY_DOCUMENT),
            "case {case:?}: expected the empty document, got {:?}",
            read.policy()
        );
        assert_eq!(
            read.resource_arn().as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the pipeline ARN {arn:?}, got {:?}",
            read.resource_arn()
        );
    }
}

#[tokio::test]
async fn removal_is_idempotent() {
    let router = new_router();
    let case = "the same removal repeated";
    let arn = settled_pipeline(&router, case, "policy-removal").await;

    put_policy(&router, &arn, r#"{"Statement":[]}"#).await;
    for attempt in 1..=3 {
        let removed = delete_policy(&router, &arn).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: removal {attempt} must be accepted, got {} / {:?}",
            removed.status, removed.body
        );
        let read = get_policy(&router, &arn).await;
        assert_eq!(
            read.policy().as_deref(),
            Some(EMPTY_DOCUMENT),
            "case {case:?}: after removal {attempt} the pipeline must have the empty document, \
             got {:?}",
            read.policy()
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_a_json_object_is_refused_and_attaches_nothing() {
    let router = new_router();
    let attached = r#"{"Version":"2012-10-17","Statement":[]}"#;
    let arn = settled_pipeline(&router, "a pipeline carrying a policy", "policy-refusals").await;
    let written = put_policy(&router, &arn, attached).await;
    assert_eq!(
        written.status, 200,
        "expected 200 from the setup PutResourcePolicy, got {} / {:?}",
        written.status, written.body
    );

    for (case, document) in NON_OBJECT_DOCUMENTS {
        let refused = put_policy(&router, &arn, document).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.message(),
            "Invalid resource policy.",
            "case {case:?}: unexpected refusal wording"
        );

        let read = get_policy(&router, &arn).await;
        assert_eq!(
            read.policy().as_deref(),
            Some(attached),
            "case {case:?}: a refused write must leave the attached document alone, got {:?}",
            read.policy()
        );
    }

    // The document is judged before the resource is resolved, so a request that is both
    // malformed and addressed at no pipeline is refused on the document.
    let nowhere = arn_of("policy-refusals-nowhere");
    for (case, document) in NON_OBJECT_DOCUMENTS {
        let refused = put_policy(&router, &nowhere, document).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} on an ARN naming no pipeline: expected HTTP 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?} on an ARN naming no pipeline: expected ValidationException, got {:?}",
            refused.error_type
        );
    }
}

#[tokio::test]
async fn a_document_outside_the_published_length_bounds_is_refused() {
    let router = new_router();
    let arn = settled_pipeline(&router, "a pipeline for the length bounds", "policy-bounds").await;

    // One byte short of the minimum and one byte past the maximum. Both are refused by the
    // modelled `@length` on `Policy`, before the handler sees the request.
    let cases = [
        (
            "one byte short of the published length minimum",
            "{".repeat(POLICY_MIN_LENGTH - 1),
        ),
        (
            "one byte past the published length maximum",
            document_of_length(POLICY_MAX_LENGTH + 1),
        ),
    ];
    for (case, document) in cases {
        let refused = put_policy(&router, &arn, &document).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            get_policy(&router, &arn).await.policy().as_deref(),
            Some(EMPTY_DOCUMENT),
            "case {case:?}: a refused write must attach nothing"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_pipeline_is_refused_by_every_policy_operation() {
    let router = new_router();
    // One ARN whose pipeline was never created, and one whose pipeline was created and
    // then deleted — a deleted pipeline names no resource either, and its policy went with
    // it.
    let never_created = arn_of("policy-never-created");
    let deleted = settled_pipeline(&router, "a pipeline about to be deleted", "policy-gone").await;
    put_policy(&router, &deleted, r#"{"Statement":[]}"#).await;
    let removed = delete_pipeline(&router, "policy-gone").await;
    assert_eq!(
        removed.status, 200,
        "expected 200 from DeletePipeline, got {} / {:?}",
        removed.status, removed.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    for arn in [never_created, deleted] {
        for (case, answer) in [
            ("GetResourcePolicy", get_policy(&router, &arn).await),
            (
                "PutResourcePolicy",
                put_policy(&router, &arn, r#"{"Statement":[]}"#).await,
            ),
            ("DeleteResourcePolicy", delete_policy(&router, &arn).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {arn:?}: expected HTTP 404, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "case {case:?} on {arn:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.message(),
                "The pipeline could not be found",
                "case {case:?} on {arn:?}: unexpected refusal wording"
            );
        }
    }
}

#[tokio::test]
async fn each_pipeline_carries_its_own_policy() {
    let router = new_router();
    let case = "two pipelines, two documents";
    let first = settled_pipeline(&router, case, "policy-tenant-a").await;
    let second = settled_pipeline(&router, case, "policy-tenant-b").await;

    let first_document = r#"{"Statement":[{"Effect":"Allow","Action":"osis:Ingest"}]}"#;
    let second_document = r#"{"Statement":[{"Effect":"Deny","Action":"osis:Ingest"}]}"#;
    put_policy(&router, &first, first_document).await;
    put_policy(&router, &second, second_document).await;

    for (arn, want) in [(&first, first_document), (&second, second_document)] {
        let read = get_policy(&router, arn).await;
        assert_eq!(
            read.policy().as_deref(),
            Some(want),
            "case {case:?}: {arn:?} must answer its own document, got {:?}",
            read.policy()
        );
    }

    // Removing one pipeline's policy leaves the other's where it was.
    delete_policy(&router, &first).await;
    assert_eq!(
        get_policy(&router, &first).await.policy().as_deref(),
        Some(EMPTY_DOCUMENT),
        "case {case:?}: {first:?} must have the empty document after its removal"
    );
    assert_eq!(
        get_policy(&router, &second).await.policy().as_deref(),
        Some(second_document),
        "case {case:?}: {second:?} must still carry its own document"
    );
}
