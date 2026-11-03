// SPDX-License-Identifier: CC0-1.0

//! How the `/tags/{ResourceArn}` surface refuses a value its ARN member cannot be, end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rule: a tagging operation addresses its resource by an ARN, which that surface
//! PARSES before anything resolves it, so a value whose SYNTAX is not that of one of this
//! service's resource ARNs is a malformed-request refusal of the surface itself — HTTP 400,
//! the wire error code it publishes (`BadRequestException`, which is not the name of any
//! shape the model declares for these operations), and its own one-line reason. That
//! sentence states nothing of the request: not the submitted value, not the member it was
//! bound to, and not the expression the model constrains it with. Only a value whose syntax
//! the surface CAN read reaches the resource lookup, which answers its own refusal for an
//! ARN that resolves to nothing (404, `NotFoundException`).
//!
//! Each row of the table varies what the rule generalises over: the operation (all three
//! the one route carries, with their three HTTP methods) and the FORM the syntax fails in
//! (no ARN shape at all, a truncated ARN, an empty required ARN field, a value past the
//! member's own `@length` maximum, an ARN of another service, an ARN of this service naming
//! another resource kind), plus whether a second member of the same request is invalid too.
//! Both controls are in the same suite: a well-formed ARN that resolves to nothing is still
//! the lookup's own 404, and the stored environment's ARN is still served.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The region the `AUTHORIZATION` scope signs for.
const REGION: &str = "us-east-1";

/// The response header restJson1 resolves an error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The wire error code this surface publishes for a value it cannot parse as a resource
/// ARN, spelled out here rather than read from the crate so this suite pins the code a
/// client's error dispatch really sees.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The code the resource LOOKUP publishes when the parse succeeded and the ARN named no
/// environment.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The fixed sentence a malformed-ARN refusal carries, spelled out here rather than read
/// from the crate so this suite pins the text a client really reads. It has no variable
/// part: the surface states the parse failure, not the request that caused it.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn: bad syntax";

/// `EnvironmentArn`'s own `@length` maximum, from
/// `smithy_query(query="shape",shape="EnvironmentArn",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`.
const RESOURCE_ARN_MAX_LENGTH: usize = 1224;

/// The member path the framework's own constraint envelope would have named, which this
/// refusal must not state.
const FRAMEWORK_MEMBER_PATH: &str = "resourceArn";

/// The opening of the framework's own constraint envelope, which this refusal must not be.
const FRAMEWORK_ENVELOPE: &str = "validation error";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    error_type_header: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    /// The body as a JSON document when it is one, and `Value::Null` when it is not.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The sentence a restJson1 client reads off a refusal: the `message` member of the
    /// error document.
    fn message(&self) -> String {
        let document = self.document();
        document
            .get("message")
            .or_else(|| document.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code a restJson1 client resolves: the `x-amzn-errortype` header
    /// first, then the body's own discriminator.
    fn error_code(&self) -> String {
        let named = self.error_type_header.clone().unwrap_or_else(|| {
            self.document()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        named
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let error_type_header = response
            .headers()
            .get(ERROR_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status: response.status().as_u16(),
            error_type_header,
            body: response.into_body().to_vec(),
        }
    }

    /// Create one environment with every optional member absent.
    async fn create(&self, name: &str) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{name}"),
            Some(json!({
                "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
                "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
                "DagS3Path": "dags",
                "NetworkConfiguration": {
                    "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "SecurityGroupIds": ["sg-00000000000000001"],
                },
            })),
        )
        .await
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// The ARN of an environment of this account and Region, the one form `EnvironmentArn`
/// admits.
fn environment_arn(name: &str) -> String {
    format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}")
}

/// One call of each operation the `/tags/{ResourceArn}` route carries, for the label
/// `label`: `(operation, method, uri, body)`. `tagKeys` is always carried where the model
/// requires it, so the ARN is the only thing a row is about.
fn tagging_calls(label: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{label}"),
            None,
        ),
        (
            "TagResource",
            "POST",
            format!("/tags/{label}"),
            Some(json!({"Tags": {"Environment": "Staging"}})),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{label}?tagKeys=Environment"),
            None,
        ),
    ]
}

/// Values whose syntax is not one of this service's resource ARNs: `(case, value)`.
fn unparseable_arns() -> Vec<(&'static str, String)> {
    vec![
        ("no ARN shape at all", "not-an-mwaa-arn".to_string()),
        ("the prefix alone", "arn".to_string()),
        ("a truncated ARN", "arn:aws:airflow".to_string()),
        (
            "an ARN with no resource field",
            format!("arn:aws:airflow:{REGION}:{ACCOUNT}"),
        ),
        (
            "an ARN with an empty resource field",
            format!("arn:aws:airflow:{REGION}:{ACCOUNT}:"),
        ),
        (
            "an ARN with an empty partition",
            format!("arn::airflow:{REGION}:{ACCOUNT}:environment/Env"),
        ),
        (
            "a prefix of another case",
            format!("ARN:aws:airflow:{REGION}:{ACCOUNT}:environment/Env"),
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:airflow:{REGION}:00000000000:environment/Env"),
        ),
        (
            "this service's ARN naming another resource kind",
            format!("arn:aws:airflow:{REGION}:{ACCOUNT}:dag/Env"),
        ),
        (
            "an environment segment that is empty",
            format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/"),
        ),
        (
            "one character past the member's @length maximum",
            "n".repeat(RESOURCE_ARN_MAX_LENGTH + 1),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::my-airflow-bucket".to_string(),
        ),
        (
            "another service's ARN with an account",
            format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        ),
        (
            "another service's ARN in another partition",
            "arn:aws-cn:lambda:cn-north-1:123456789012:function:thing".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_unparseable_resource_arn_is_refused_by_the_tagging_surface_on_every_operation() {
    let harness = Harness::new();
    let stored = "Tagged";
    assert_eq!(
        harness.create(stored).await.status,
        200,
        "the fixture create must succeed"
    );

    for (case, value) in unparseable_arns() {
        assert!(
            value.len() <= RESOURCE_ARN_MAX_LENGTH + 1,
            "case {case:?}: the table's own value is {} characters",
            value.len()
        );
        for (operation, method, uri, body) in tagging_calls(&encode_label(&value)) {
            let case = format!("{operation} / {case}");
            let answer = harness.send(method, &uri, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for a value that is no resource ARN, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                BAD_REQUEST_EXCEPTION,
                "case {case:?}: expected the code this surface publishes, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_type_header.as_deref(),
                Some(BAD_REQUEST_EXCEPTION),
                "case {case:?}: restJson1 resolves the shape from {ERROR_TYPE_HEADER}, got {:?}",
                answer.error_type_header
            );
            let expected = INVALID_RESOURCE_ARN.to_string();
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            // The refusal is about the ARN the surface wanted, not about the request that
            // missed it: none of the three things the framework's own envelope would have
            // stated appears in it.
            let text = answer.text();
            // A value short enough to be a WORD of the fixed sentence ("arn") is not an
            // echo of the request, so the check is on values the sentence cannot contain
            // by construction.
            if !expected.contains(value.as_str()) {
                assert!(
                    !text.contains(value.as_str()),
                    "case {case:?}: the refusal must not echo the submitted value, got {text}"
                );
            }
            assert!(
                !text.contains(FRAMEWORK_MEMBER_PATH) && !text.contains("ResourceArn"),
                "case {case:?}: the refusal must not name the member, got {text}"
            );
            assert!(
                !text.contains(FRAMEWORK_ENVELOPE) && !text.contains("regular expression"),
                "case {case:?}: the refusal must not be the constraint envelope, got {text}"
            );
        }
    }

    // A request whose ARN is unparseable AND whose body breaks another member's own
    // constraint is still the ARN refusal: the surface parses the address before it reads
    // the request. (`TagMap` is `@length(1, 50)`, so an empty map is outside it.)
    let both = harness
        .send(
            "POST",
            &format!("/tags/{}", encode_label("not-an-mwaa-arn")),
            Some(json!({"Tags": {}})),
        )
        .await;
    assert_eq!(
        both.status, 400,
        "a request wrong in two ways is still refused, got {} {}",
        both.status,
        both.text()
    );
    assert_eq!(
        both.error_code(),
        BAD_REQUEST_EXCEPTION,
        "the ARN is parsed before the body is read, got {:?}: {}",
        both.error_code(),
        both.text()
    );
    assert_eq!(
        both.message(),
        INVALID_RESOURCE_ARN,
        "the ARN is parsed before the body is read, got {:?}",
        both.message()
    );
}

#[tokio::test]
async fn a_parseable_resource_arn_still_reaches_the_resource_lookup() {
    let harness = Harness::new();
    let stored = "Tagged";
    assert_eq!(
        harness.create(stored).await.status,
        200,
        "the fixture create must succeed"
    );

    // Control 1: ARNs the surface CAN parse are not refused by it — they resolve, and an
    // ARN that resolves to nothing is the lookup's own 404 with its own code and sentence.
    // The rows differ along the partition, the Region, the account and the environment
    // segment `EnvironmentArn`'s pattern allows.
    for (case, arn) in [
        ("another name", environment_arn("NeverCreated")),
        (
            "another Region",
            format!("arn:aws:airflow:eu-west-3:{ACCOUNT}:environment/Another_7"),
        ),
        (
            "another partition and account",
            "arn:aws-cn:airflow:cn-north-1:123456789012:environment/Overseas".to_string(),
        ),
        ("a one-character name", environment_arn("A")),
    ] {
        for (operation, method, uri, body) in tagging_calls(&encode_label(&arn)) {
            let case = format!("{operation} / {case}");
            let answer = harness.send(method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: a parseable ARN that resolves to nothing is the lookup's 404, \
                 got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                NOT_FOUND_EXCEPTION,
                "case {case:?}: expected the lookup's own published code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                "Resource was not found",
                "case {case:?}: expected the lookup's own sentence, got {:?}",
                answer.message()
            );
        }
    }

    // Control 2: the stored environment's own ARN is served on all three operations, and a
    // refused call left its tags exactly as they were.
    let label = encode_label(&environment_arn(stored));
    let tagged = harness
        .send(
            "POST",
            &format!("/tags/{label}"),
            Some(json!({"Tags": {"Environment": "Staging"}})),
        )
        .await;
    assert_eq!(
        tagged.status,
        200,
        "tagging the stored environment must succeed, got {} {}",
        tagged.status,
        tagged.text()
    );
    let refused = harness
        .send(
            "POST",
            &format!("/tags/{}", encode_label("not-an-mwaa-arn")),
            Some(json!({"Tags": {"Refused": "yes"}})),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "the malformed-ARN write must be refused, got {}",
        refused.status
    );
    let listed = harness.send("GET", &format!("/tags/{label}"), None).await;
    assert_eq!(
        listed.document().get("Tags").cloned(),
        Some(json!({"Environment": "Staging"})),
        "the refused write must leave the tag map as the served write left it, got {}",
        listed.text()
    );
    let untagged = harness
        .send(
            "DELETE",
            &format!("/tags/{label}?tagKeys=Environment"),
            None,
        )
        .await;
    assert_eq!(
        untagged.status,
        200,
        "untagging the stored environment must succeed, got {} {}",
        untagged.status,
        untagged.text()
    );
    let emptied = harness.send("GET", &format!("/tags/{label}"), None).await;
    assert!(
        emptied
            .document()
            .get("Tags")
            .and_then(Value::as_object)
            .is_none_or(|tags| !tags.contains_key("Environment")),
        "the removed key must be gone, got {}",
        emptied.text()
    );
}
