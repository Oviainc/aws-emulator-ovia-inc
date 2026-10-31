// SPDX-License-Identifier: CC0-1.0

//! The envelope every Security Hub CSPM FAILURE is reported in, and the refusal the
//! insight plane answers an `InsightArn` nothing answers to with.
//!
//! RULE 1: a failure's body carries the error shape's own `Message` and `Code` PLUS the
//! two envelope members no model declares — `Type`, the name of the exception (the same
//! name the `x-amzn-errortype` header reports, which is how a REST-JSON client of this
//! service classifies a failure), and `RequestId`, that response's own identifier. A
//! SUCCESS carries neither.
//!
//! RULE 2: an insight-addressed call whose `InsightArn` resolves to no insight of the
//! caller's hub is refused with the modelled `InvalidInputException` (400) carrying this
//! service's fixed text for that class — never a success computed over no insight, and
//! never an ARN quoted back.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! status code, a header or a serialized member that is wrong on the wire fails here.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for.
const REGION: &str = "us-east-1";

/// The whole message an insight-addressed call is refused with when nothing answers to
/// the `InsightArn` it carried. Written out here rather than imported, so the handler
/// and the expectation cannot drift together.
const INSIGHT_NOT_FOUND: &str = "Cannot find any insight for the given Arn";

/// One emulator with the Security Hub service mounted, driven over HTTP. Each call
/// names the Region it is signed for, which is how a handler learns the Region (the
/// SigV4 credential scope), exactly as a signed SDK request carries it.
struct Harness {
    router: MultiServiceRouter,
}

/// One response, as a client sees it: the status, the `x-amzn-errortype` header, and
/// the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// A body member as a string, or the empty string when it is absent.
    fn member(&self, name: &str) -> String {
        self.body[name].as_str().unwrap_or_default().to_string()
    }
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A hub subscribed in `REGION`: every operation below is hub-scoped.
    async fn subscribed() -> Self {
        let harness = Self::new();
        let answer = harness.call("POST", "/accounts", Some(json!({}))).await;
        assert_eq!(
            answer.status, 200,
            "EnableSecurityHub must succeed, got {} with {}",
            answer.status, answer.body
        );
        harness
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `GetInsightResults`: `GET /insights/results/{InsightArn+}`. The greedy label
    /// carries the ARN's own colons and slashes, exactly as an SDK request does.
    async fn insight_results(&self, insight_arn: &str) -> Answer {
        self.call("GET", &format!("/insights/results/{insight_arn}"), None)
            .await
    }

    /// `DeleteInsight`: `DELETE /insights/{InsightArn+}`.
    async fn delete_insight(&self, insight_arn: &str) -> Answer {
        self.call("DELETE", &format!("/insights/{insight_arn}"), None)
            .await
    }

    /// `CreateInsight`, answering the ARN it minted.
    async fn create_insight(&self, name: &str) -> Answer {
        self.call(
            "POST",
            "/insights",
            Some(json!({ "Name": name, "GroupByAttribute": "SeverityLabel", "Filters": {} })),
        )
        .await
    }

    /// Create one insight and return the ARN it was minted with.
    async fn insight_arn_for(&self, name: &str) -> String {
        let answer = self.create_insight(name).await;
        assert_eq!(
            answer.status, 200,
            "CreateInsight({name}): expected 200, got {} with {}",
            answer.status, answer.body
        );
        answer.member("InsightArn")
    }
}

/// A custom insight's ARN, as `CreateInsight` mints it.
fn insight_arn(region: &str, account_id: &str, id: &str) -> String {
    format!("arn:aws:securityhub:{region}:{account_id}:insight/{account_id}/custom/{id}")
}

/// Is `value` a v4-UUID-shaped identifier — the 8-4-4-4-12 hex grouping an AWS request
/// id carries?
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.iter().map(|group| group.len()).collect::<Vec<_>>() == vec![8, 4, 4, 4, 12]
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Assert RULE 1 of one refusal: the exception's name in `Code`, in `Type` and in the
/// `x-amzn-errortype` header, and a request id of its own. Answers the request id, so a
/// caller can check two refusals do not share one.
fn assert_envelope(case: &str, answer: &Answer, expected_status: u16, exception: &str) -> String {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected {expected_status}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member("Type"),
        exception,
        "case {case:?}: expected Type {exception:?}, got {}",
        answer.body
    );
    assert_eq!(
        answer.member("Code"),
        exception,
        "case {case:?}: expected Code {exception:?}, got {}",
        answer.body
    );
    // The header the restJson1 protocol resolves the error SHAPE from: a body `Type`
    // that disagrees with it would classify the failure twice over.
    assert_eq!(
        answer.error_type, exception,
        "case {case:?}: expected the x-amzn-errortype header {exception:?}, got {:?}",
        answer.error_type
    );
    assert!(
        !answer.member("Message").is_empty(),
        "case {case:?}: a refusal states why, got {}",
        answer.body
    );
    let request_id = answer.member("RequestId");
    assert!(
        is_uuid_shaped(&request_id),
        "case {case:?}: expected a UUID-shaped RequestId, got {request_id:?} in {}",
        answer.body
    );
    request_id
}

// ---------------------------------------------------------------------------
// RULE 1 — the error envelope
// ---------------------------------------------------------------------------

/// One refusal row of the table below: the case, the request to send (method, path,
/// body) and the answer it must get (status, exception name).
type RefusalCase = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    u16,
    &'static str,
);

/// RULE: every failure this service answers carries the exception's name in `Type` (and
/// in the `x-amzn-errortype` header) beside the shape's `Message`/`Code`, plus its own
/// `RequestId`.
///
/// The rows span the refusal classes the service answers with, which is what the rule
/// generalises over: the four modelled client errors of the insight, action-target,
/// standards and hub planes, each reached through a different operation and HTTP method
/// and each carrying a different status.
#[tokio::test]
async fn every_refusal_carries_the_exception_name_and_its_own_request_id() {
    let unknown_action_target = format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:action/custom/absent");
    let unknown_insight = insight_arn(REGION, ACCOUNT_ID, "absent");
    let unenableable_standard =
        format!("arn:aws:securityhub:{REGION}::standards/sim-does-not-exist/v/1.0.0");

    // (case, method, path, body, expected status, expected exception name)
    let cases: Vec<RefusalCase> = vec![
        (
            "GetInsightResults for an ARN no insight has",
            "GET",
            format!("/insights/results/{unknown_insight}"),
            None,
            400,
            "InvalidInputException",
        ),
        (
            "DeleteInsight for an ARN no insight has",
            "DELETE",
            format!("/insights/{unknown_insight}"),
            None,
            400,
            "InvalidInputException",
        ),
        (
            "CreateInsight grouped by something that is not a finding attribute",
            "POST",
            "/insights".to_string(),
            Some(json!({
                "Name": "bad group",
                "GroupByAttribute": "NotAFindingAttribute",
                "Filters": {},
            })),
            400,
            "InvalidInputException",
        ),
        (
            "BatchEnableStandards naming a standard that cannot be enabled",
            "POST",
            "/standards/register".to_string(),
            Some(json!({
                "StandardsSubscriptionRequests": [{ "StandardsArn": unenableable_standard }],
            })),
            400,
            "InvalidInputException",
        ),
        (
            "DeleteActionTarget for an ARN no action target has",
            "DELETE",
            format!("/actionTargets/{unknown_action_target}"),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a second EnableSecurityHub",
            "POST",
            "/accounts".to_string(),
            Some(json!({})),
            409,
            "ResourceConflictException",
        ),
    ];

    let harness = Harness::subscribed().await;
    let mut request_ids: BTreeSet<String> = BTreeSet::new();
    for (case, method, path, body, status, exception) in cases {
        // Twice: the refusal is about the request, so repeating it must repeat the
        // classification — and must NOT repeat the request id.
        for attempt in 1..=2 {
            let answer = harness.call(method, &path, body.clone()).await;
            let label = format!("{case} (attempt {attempt})");
            let request_id = assert_envelope(&label, &answer, status, exception);
            assert!(
                request_ids.insert(request_id.clone()),
                "case {label:?}: every response reports its own RequestId, got the repeat {request_id}"
            );
        }
    }

    // A SUCCESS carries neither envelope member: they report a failure, so a caller
    // cannot read one off a response that did not fail.
    let answer = harness.create_insight("a good insight").await;
    assert_eq!(
        answer.status, 200,
        "CreateInsight: expected 200, got {} with {}",
        answer.status, answer.body
    );
    for member in ["Type", "RequestId"] {
        assert!(
            answer.body[member].is_null(),
            "case \"a success\": expected no {member} in the body, got {}",
            answer.body
        );
    }
}

/// RULE: the envelope is the SERVICE's, so a refusal raised before any hub exists
/// carries it too — the account that is not subscribed is `InvalidAccessException`
/// (401), reported through the same three channels.
///
/// The rows differ in the operation and method that hits the unsubscribed hub: a read,
/// a create, and a delete.
#[tokio::test]
async fn an_unsubscribed_account_is_refused_in_the_same_envelope() {
    let arn = insight_arn(REGION, ACCOUNT_ID, "whatever");

    // (case, method, path, body)
    let cases: [(&str, &str, String, Option<Value>); 4] = [
        ("DescribeHub", "GET", "/accounts".to_string(), None),
        (
            "GetInsightResults",
            "GET",
            format!("/insights/results/{arn}"),
            None,
        ),
        (
            "CreateInsight",
            "POST",
            "/insights".to_string(),
            Some(json!({ "Name": "n", "GroupByAttribute": "SeverityLabel", "Filters": {} })),
        ),
        ("DeleteInsight", "DELETE", format!("/insights/{arn}"), None),
    ];

    for (case, method, path, body) in cases {
        // A fresh emulator per row: the refusal is about there being no hub at all.
        let harness = Harness::new();
        let answer = harness.call(method, &path, body).await;
        assert_envelope(case, &answer, 401, "InvalidAccessException");
    }
}

// ---------------------------------------------------------------------------
// RULE 2 — the insight plane's dangling-ARN refusal
// ---------------------------------------------------------------------------

/// RULE: an `InsightArn` that resolves to no insight of the caller's hub is refused
/// with `InvalidInputException` (400) and this service's FIXED text for that class —
/// the message interpolates nothing, so every form of unknown ARN gets the same string.
///
/// The rows span the forms an unknown insight ARN takes: never created, created and
/// then deleted, the same insight ARN read under another Region or another account, an
/// ARN of another resource kind, and a value that is not an ARN at all.
#[tokio::test]
async fn an_unknown_insight_arn_is_refused_with_the_fixed_message() {
    let harness = Harness::subscribed().await;
    let kept = harness.insight_arn_for("kept").await;
    let removed = harness.insight_arn_for("removed").await;
    let answer = harness.delete_insight(&removed).await;
    assert_eq!(
        answer.status, 200,
        "the first DeleteInsight expected 200, got {} with {}",
        answer.status, answer.body
    );

    let cases: [(&str, String); 6] = [
        (
            "an id that was never created",
            insight_arn(REGION, ACCOUNT_ID, "00000000-0000-0000-0000-000000000000"),
        ),
        ("an insight that was already deleted", removed.clone()),
        (
            "a live insight's ARN under another Region",
            kept.replace(REGION, "eu-west-1"),
        ),
        (
            "a live insight's ARN under another account",
            kept.replace(ACCOUNT_ID, "999999999999"),
        ),
        (
            "a hub ARN rather than an insight ARN",
            format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:hub/default"),
        ),
        ("a bare name rather than an ARN", "my-insight".to_string()),
    ];

    for (case, arn) in cases {
        // Both insight-addressed operations this slice serves answer the same lookup
        // failure the same way.
        for (operation, answer) in [
            ("GetInsightResults", harness.insight_results(&arn).await),
            ("DeleteInsight", harness.delete_insight(&arn).await),
        ] {
            let label = format!("{operation}: {case}");
            assert_envelope(&label, &answer, 400, "InvalidInputException");
            let actual = answer.member("Message");
            assert_eq!(
                actual, INSIGHT_NOT_FOUND,
                "case {label:?}: expected message {INSIGHT_NOT_FOUND:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&arn),
                "case {label:?}: the fixed message quotes no ARN back, got {actual:?}"
            );
        }
    }

    // The insight the rows did not address is untouched by every refusal above.
    let answer = harness.insight_results(&kept).await;
    assert_eq!(
        answer.status, 200,
        "the live insight must still answer, got {} with {}",
        answer.status, answer.body
    );
}

/// RULE: `DeleteInsight` removes the insight record itself — the ARN stops resolving,
/// the delete is not repeatable, and the `Name` the insight held is free again — while
/// the other insights of the hub keep answering.
#[tokio::test]
async fn deleting_an_insight_removes_exactly_that_insight() {
    let harness = Harness::subscribed().await;

    let names = ["first", "second", "third"];
    let mut arns: Vec<String> = Vec::new();
    for name in names {
        arns.push(harness.insight_arn_for(name).await);
    }

    for (position, name) in names.iter().enumerate() {
        let arn = arns[position].clone();
        // Read back through the implemented reader before the delete, so the delete is
        // shown to change a state that was there.
        let answer = harness.insight_results(&arn).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: before the delete expected 200, got {} with {}",
            answer.status, answer.body
        );

        let answer = harness.delete_insight(&arn).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: DeleteInsight expected 200, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("InsightArn"),
            arn,
            "case {name:?}: DeleteInsight must echo the ARN it deleted, got {}",
            answer.body
        );

        // Gone: the reader refuses it, and so does a second delete.
        for (label, answer) in [
            ("read after delete", harness.insight_results(&arn).await),
            ("delete twice", harness.delete_insight(&arn).await),
        ] {
            let case = format!("{name}: {label}");
            assert_envelope(&case, &answer, 400, "InvalidInputException");
        }

        // The insights not addressed are untouched.
        for (other, other_arn) in names.iter().zip(arns.iter()).skip(position + 1) {
            let answer = harness.insight_results(other_arn).await;
            assert_eq!(
                answer.status, 200,
                "case {name:?}: deleting it must leave {other:?} answering, got {} with {}",
                answer.status, answer.body
            );
        }

        // And the freed name is creatable again, which is what proves the record is
        // gone rather than hidden — with its own ARN.
        let reused = harness.insight_arn_for(name).await;
        assert_ne!(
            reused, arn,
            "case {name:?}: the re-created insight must get its own ARN, got {reused}"
        );
        let answer = harness.delete_insight(&reused).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: deleting the re-created insight expected 200, got {} with {}",
            answer.status, answer.body
        );
    }
}
