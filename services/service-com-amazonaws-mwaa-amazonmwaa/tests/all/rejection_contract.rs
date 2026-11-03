// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a request, and what its two token operations answer when it
//! does not, end to end over HTTP through the router this service mounts in production.
//!
//! Four rules, each asserted over a table whose rows differ along what the rule
//! generalises over — never over one recorded request:
//!
//! 1. Every operation addressed by `Name` resolves the name in the stored environments
//!    first and refuses one that resolves to nothing with HTTP 404 and the wire code
//!    `ResourceNotFoundException`. The message depends on the endpoint the model
//!    hosts the operation on: a control-plane (`api.`) lookup names the environment the
//!    request asked for (`Environment <Name> not found.`), while an operation hosted on the
//!    environment data endpoint (`env.`) is refused by that front door, whose sentence is
//!    fixed and names no environment. Rows vary the name (length, case, the `-`/`_`
//!    characters `EnvironmentName`'s pattern allows) and the operation; the wording rule
//!    itself has its own suite (`tests/all/env_endpoint_not_found_wording.rs`), the BODY
//!    SHAPE each refusal arrives in has another
//!    (`tests/all/env_endpoint_front_door.rs`), and the envelope every other refusal
//!    travels in a third (`tests/all/env_endpoint_payload.rs`).
//! 2. The `/tags/{ResourceArn}` operations refuse a well-formed ARN that resolves to no
//!    environment with HTTP 404 and the WIRE error code the service publishes for them,
//!    `NotFoundException` — the code a client's error dispatch branches on. Rows vary the
//!    partition, the region, the environment segment and the operation.
//! 3. A `@pattern` violation quotes the expression WITHOUT the `^` / `$` anchors the model
//!    spells it with, and leaves every other violation class worded as the framework words
//!    it. Rows vary the operation, the member, where the member is bound (body, nested
//!    structure, URI label) and which anchors the pattern carries (both, leading only,
//!    none); each row's pattern is quoted from the shape named in its comment. The ONE
//!    member outside this rule is the tagging surface's `ResourceArn`, which that surface
//!    words itself (`tests/all/tagging_resource_arn_refusal.rs`).
//! 4. `CreateWebLoginToken` and `CreateCliToken` mint a token for an environment that
//!    EXISTS and report it with the web-server host that environment's own stored
//!    `WebserverUrl` carries; two calls mint two different tokens. Rows vary the
//!    environment.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::service::endpoints::Endpoint;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The access-key id the header above presents. Neither STS nor IAM holds a record of it,
/// so it speaks for the identity this emulator publishes at start — whose NAME is what
/// `CreateWebLoginToken` reports as the calling `IamIdentity` (see
/// `tests/all/web_login_token_caller_identity.rs` for the identity rule itself).
const CALLER_AKID: &str = "AKIDTEST";

/// The name of the identity [`CALLER_AKID`] speaks for: the resource field of that IAM
/// user's ARN, which is the form `IamIdentity` documents ("the name of the IAM identity …
/// For example, `assumed-role/Admin/your-name`").
const CALLER_IAM_IDENTITY: &str = "user/emulator";

/// The Apache Airflow user name that identity maps to: its last path segment.
const CALLER_AIRFLOW_IDENTITY: &str = "emulator";

/// The region the `AUTHORIZATION` scope signs for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    /// `x-amzn-errortype`, which is how a restJson1 client resolves the error shape.
    error_type_header: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body as a JSON document when it is one, and `Value::Null` when it is not — the
    /// refusal the proxied operation relays carries the sentence as plain text rather than
    /// a document.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code a restJson1 client resolves: the `x-amzn-errortype` header
    /// first, then the body's own `__type` discriminator.
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

    /// The sentence a client reads off a refusal, the way an SDK's JSON error parser reads
    /// it: the `message` member of a document body, and the body itself when the body is
    /// not a document.
    fn message(&self) -> String {
        match self.document() {
            Value::Object(object) => object
                .get("message")
                .or_else(|| object.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => self.text(),
        }
    }

    fn member(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
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
            .get("x-amzn-errortype")
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
            Some(valid_create_body()),
        )
        .await
    }

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }
}

/// A create body every member of which satisfies its modelled constraints; the pattern
/// table below replaces exactly one member of it per row.
fn valid_create_body() -> Value {
    json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    })
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// Environment names no create ever stored. They differ along what
/// `EnvironmentName`'s own `@length(1, 80)` and `^[a-zA-Z][0-9a-zA-Z-_]*$` allow: the
/// one-character minimum, mixed case, digits, both separators, and the 80-character
/// maximum.
fn unknown_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "NoSuchEnvironment".to_string(),
        "missing-env_7".to_string(),
        "MixedCase-Env_42".to_string(),
        format!("L{}", "o".repeat(79)),
    ]
}

/// Every operation addressed by `Name`, as `(label, method, uri template, body, endpoint)`.
/// The template's `{}` takes the name, and the endpoint is the one the model's `@endpoint`
/// trait hosts the operation on — read from the crate's endpoint census
/// (`service::endpoints`, checked against the shipped model by
/// `tests/all/endpoint_host_prefix_census.rs`) — which is what decides the refusal's
/// wording, and the shape it travels in.
type NamedCall = (
    &'static str,
    &'static str,
    &'static str,
    Option<Value>,
    Endpoint,
);

fn name_addressed_calls() -> Vec<NamedCall> {
    vec![
        (
            "GetEnvironment",
            "GET",
            "/environments/{}",
            None,
            Endpoint::ControlPlane,
        ),
        (
            "UpdateEnvironment",
            "PATCH",
            "/environments/{}",
            Some(json!({"MaxWorkers": 2})),
            Endpoint::ControlPlane,
        ),
        (
            "DeleteEnvironment",
            "DELETE",
            "/environments/{}",
            None,
            Endpoint::ControlPlane,
        ),
        (
            "CreateCliToken",
            "POST",
            "/clitoken/{}",
            None,
            Endpoint::EnvironmentData,
        ),
        (
            "CreateWebLoginToken",
            "POST",
            "/webtoken/{}",
            None,
            Endpoint::EnvironmentData,
        ),
        (
            "InvokeRestApi",
            "POST",
            "/restapi/{}",
            Some(json!({"Path": "/dags", "Method": "GET"})),
            Endpoint::EnvironmentData,
        ),
    ]
}

#[tokio::test]
async fn every_name_addressed_operation_refuses_an_unknown_environment_with_the_same_404() {
    let harness = Harness::new();
    // One environment exists, so a refusal below is the lookup failing and not an empty
    // store answering.
    assert_eq!(
        harness.create("TheOnlyEnvironment").await.status,
        200,
        "the fixture create must succeed"
    );

    for name in unknown_names() {
        for (operation, method, uri, body, endpoint) in name_addressed_calls() {
            let case = format!("{operation}/{name}");
            let answer = harness
                .send(method, &uri.replace("{}", &name), body.clone())
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 for a name that resolves to nothing, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled wire code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_type_header.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: restJson1 resolves the shape from x-amzn-errortype, got {:?}",
                answer.error_type_header
            );
            // The control plane words the condition from the request; the environment data
            // endpoint's front door words it itself, naming no environment.
            let host_prefix = endpoint.host_prefix();
            let expected = if endpoint == Endpoint::EnvironmentData {
                service_com_amazonaws_mwaa_amazonmwaa::state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE
                    .to_string()
            } else {
                format!("Environment {name} not found.")
            };
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected the {host_prefix} endpoint's wording {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// Well-formed environment ARNs that resolve to no environment. They differ along what
/// `EnvironmentArn`'s pattern
/// (`^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+`) allows: the
/// partition suffix, the region, the account and the environment segment.
fn unresolvable_arns() -> Vec<String> {
    vec![
        format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/NeverCreated"),
        format!("arn:aws:airflow:eu-west-3:{ACCOUNT}:environment/Another_7"),
        "arn:aws-cn:airflow:cn-north-1:123456789012:environment/Overseas".to_string(),
        format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/A"),
    ]
}

#[tokio::test]
async fn the_tagging_operations_refuse_an_unresolvable_arn_with_the_published_wire_code() {
    let harness = Harness::new();
    assert_eq!(harness.create("Tagged").await.status, 200, "fixture create");
    let stored_arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/Tagged");

    for arn in unresolvable_arns() {
        let label = encode_label(&arn);
        for (operation, method, uri, body) in [
            (
                "TagResource",
                "POST",
                format!("/tags/{label}"),
                Some(json!({"Tags": {"k": "v"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("/tags/{label}?tagKeys=k"),
                None,
            ),
            (
                "ListTagsForResource",
                "GET",
                format!("/tags/{label}"),
                None,
            ),
        ] {
            let case = format!("{operation} {arn}");
            let answer = harness.send(method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 for an ARN that resolves to nothing, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?}: expected the wire code the tagging operations publish, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_type_header.as_deref(),
                Some("NotFoundException"),
                "case {case:?}: restJson1 resolves the shape from x-amzn-errortype, got {:?}",
                answer.error_type_header
            );
            assert!(
                !answer.message().is_empty(),
                "case {case:?}: the refusal must carry a message"
            );
        }
    }

    // The positive control: the ARN that DOES resolve is tagged, so the refusals above
    // are the lookup failing rather than the operation refusing every ARN.
    let tagged = harness
        .send(
            "POST",
            &format!("/tags/{}", encode_label(&stored_arn)),
            Some(json!({"Tags": {"k": "v"}})),
        )
        .await;
    assert_eq!(
        tagged.status,
        200,
        "case {stored_arn:?}: tagging the stored environment must succeed, got {} {}",
        tagged.status,
        tagged.text()
    );
    let listed = harness
        .send("GET", &format!("/tags/{}", encode_label(&stored_arn)), None)
        .await;
    assert_eq!(
        listed.json().get("Tags").cloned(),
        Some(json!({"k": "v"})),
        "case {stored_arn:?}: the written tag must read back, got {}",
        listed.text()
    );
}

/// One refusal whose message quotes a `@pattern`: the request that triggers it, the
/// member path the clause names, and the expression the model carries.
struct PatternCase {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The rejected value, as the clause echoes it.
    value: &'static str,
    /// The lowerCamel member path the clause names.
    path: &'static str,
    /// The `@pattern` the member's shape declares, quoted from the model.
    modelled: &'static str,
    /// That expression as AWS advertises it: the model's pattern with a leading `^` and a
    /// trailing `$` removed, and nothing else changed.
    advertised: &'static str,
}

/// A create body with exactly one member replaced by `value`.
fn create_body_with(member: &str, value: &str) -> Value {
    let mut body = valid_create_body();
    body.as_object_mut()
        .expect("the create body is an object")
        .insert(member.to_string(), json!(value));
    body
}

/// The table. Every `modelled` string below is the `smithy.api#pattern` of the shape named
/// in the row's comment, from
/// `smithy_query(query="shape", shape="<Shape>", model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`.
/// None of them is the member the issue cited. `EnvironmentArn` — the only input member
/// whose pattern is anchored at the START only — is absent on purpose: the tagging surface
/// words a value that member cannot be itself, so no constraint clause is rendered for it
/// (that refusal has its own suite, and the anchoring of a start-only expression is
/// asserted by the unit test beside `constraint_clause`).
fn pattern_cases() -> Vec<PatternCase> {
    vec![
        // `S3BucketArn`: anchored at both ends, in a top-level body member.
        PatternCase {
            label: "CreateEnvironment.SourceBucketArn",
            method: "PUT",
            uri: "/environments/PatternBucket".to_string(),
            body: Some(create_body_with("SourceBucketArn", "arn:aws:s3:::Bad_Bucket")),
            value: "arn:aws:s3:::Bad_Bucket",
            path: "sourceBucketArn",
            modelled: r"^arn:aws(-[a-z]+)?:s3:::[a-z0-9.\-]+$",
            advertised: r"arn:aws(-[a-z]+)?:s3:::[a-z0-9.\-]+",
        },
        // `AirflowVersion`: a short anchored expression, so the stripping cannot be
        // mistaken for trimming a long one.
        PatternCase {
            label: "CreateEnvironment.AirflowVersion",
            method: "PUT",
            uri: "/environments/PatternVersion".to_string(),
            body: Some(create_body_with("AirflowVersion", "2.X.Y")),
            value: "2.X.Y",
            path: "airflowVersion",
            modelled: r"^[0-9a-z.]+$",
            advertised: r"[0-9a-z.]+",
        },
        // `KmsKey`: an anchored expression whose body itself ends in `)`, which the
        // stripping must not touch.
        PatternCase {
            label: "CreateEnvironment.KmsKey",
            method: "PUT",
            uri: "/environments/PatternKey".to_string(),
            body: Some(create_body_with("KmsKey", "not-a-kms-key")),
            value: "not-a-kms-key",
            path: "kmsKey",
            modelled: r"^(((arn:aws(-[a-z]+)?:kms:[a-z]{2}-[a-z]+-\d:\d+:)?key\/)?[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}|(arn:aws(-[a-z]+)?:kms:[a-z]{2}-[a-z]+-\d:\d+:)?alias/.+)$",
            advertised: r"(((arn:aws(-[a-z]+)?:kms:[a-z]{2}-[a-z]+-\d:\d+:)?key\/)?[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}|(arn:aws(-[a-z]+)?:kms:[a-z]{2}-[a-z]+-\d:\d+:)?alias/.+)",
        },
        // `WeeklyMaintenanceWindowStart`: NO anchors at all, so the rule must leave the
        // expression exactly as the model spells it.
        PatternCase {
            label: "CreateEnvironment.WeeklyMaintenanceWindowStart",
            method: "PUT",
            uri: "/environments/PatternWindow".to_string(),
            body: Some(create_body_with(
                "WeeklyMaintenanceWindowStart",
                "XYZ:99:99",
            )),
            value: "XYZ:99:99",
            path: "weeklyMaintenanceWindowStart",
            modelled: r"(MON|TUE|WED|THU|FRI|SAT|SUN):([01]\d|2[0-3]):(00|30)",
            advertised: r"(MON|TUE|WED|THU|FRI|SAT|SUN):([01]\d|2[0-3]):(00|30)",
        },
        // `EnvironmentName` on an `@httpLabel`: a member bound to the URI, not the body.
        PatternCase {
            label: "GetEnvironment.Name",
            method: "GET",
            uri: "/environments/0startsWithADigit".to_string(),
            body: None,
            value: "0startsWithADigit",
            path: "name",
            modelled: r"^[a-zA-Z][0-9a-zA-Z-_]*$",
            advertised: r"[a-zA-Z][0-9a-zA-Z-_]*",
        },
    ]
}

#[tokio::test]
async fn a_pattern_violation_quotes_the_expression_without_the_model_anchors() {
    let harness = Harness::new();

    for case in pattern_cases() {
        let label = case.label;
        let answer = harness.send(case.method, &case.uri, case.body).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: a value outside its @pattern must be refused with 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            format!(
                "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {}",
                case.value, case.path, case.advertised
            ),
            "case {label:?}: the clause must quote the advertised pattern, got {:?}",
            answer.message()
        );
        // The point of the rule, stated as its own assertion: the anchored form the model
        // carries never reaches the wire (unless the model spells it without anchors, in
        // which case the two forms are the same string).
        if case.modelled != case.advertised {
            assert!(
                !answer.message().contains(case.modelled),
                "case {label:?}: the anchored pattern must not appear in {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn every_other_violation_class_keeps_the_framework_wording() {
    let harness = Harness::new();
    // `(label, method, uri, body, expected clause)`. One row per violation class the
    // envelope renders beside `@pattern`: an absent `@required` member, a value outside a
    // `@range`, and a value outside a `@length`.
    let mut too_long = valid_create_body();
    too_long
        .as_object_mut()
        .expect("object")
        .insert("AirflowVersion".into(), json!("2".repeat(33)));
    let mut without_role = valid_create_body();
    without_role
        .as_object_mut()
        .expect("object")
        .remove("ExecutionRoleArn");

    for (label, method, uri, body, expected) in [
        (
            "CreateEnvironment without ExecutionRoleArn",
            "PUT",
            "/environments/NoRole".to_string(),
            Some(without_role),
            "1 validation error detected: Value null at 'executionRoleArn' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "ListEnvironments MaxResults=0",
            "GET",
            "/environments?MaxResults=0".to_string(),
            None,
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "CreateEnvironment AirflowVersion too long",
            "PUT",
            "/environments/LongVersion".to_string(),
            Some(too_long),
            format!(
                "1 validation error detected: Value '{}' at 'airflowVersion' failed to satisfy \
                 constraint: Member must have length less than or equal to 32",
                "2".repeat(33)
            ),
        ),
    ] {
        let answer = harness.send(method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {label:?}: expected the framework's own clause, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn the_token_operations_report_the_stored_environments_web_server_host() {
    let harness = Harness::new();
    // Rows differ in the environment the token is minted for; the host is derived from
    // that environment's own record, so each row must get its own.
    let names = ["TokenEnvOne", "token-env_2", "T"];
    for name in names {
        assert_eq!(
            harness.create(name).await.status,
            200,
            "case {name:?}: the fixture create must succeed"
        );
    }

    for name in names {
        let environment = harness.get(name).await;
        let webserver_url = environment
            .json()
            .get("Environment")
            .and_then(|env| env.get("WebserverUrl").cloned())
            .and_then(|url| url.as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("case {name:?}: GetEnvironment reported no WebserverUrl"));
        let expected_host = webserver_url
            .strip_prefix("https://")
            .unwrap_or(&webserver_url)
            .to_string();

        let web = harness
            .send("POST", &format!("/webtoken/{name}"), None)
            .await;
        assert_eq!(
            web.status,
            200,
            "case {name:?}: CreateWebLoginToken must succeed for a stored environment, got {} {}",
            web.status,
            web.text()
        );
        assert_eq!(
            web.member("WebServerHostname"),
            expected_host,
            "case {name:?}: the reported host must be the stored WebserverUrl's host, got {:?}",
            web.member("WebServerHostname")
        );
        assert!(
            !web.member("WebToken").is_empty(),
            "case {name:?}: CreateWebLoginToken must report a token"
        );
        assert_eq!(
            web.member("IamIdentity"),
            CALLER_IAM_IDENTITY,
            "case {name:?}: the reported IAM identity must name the calling identity, got {:?}",
            web.member("IamIdentity")
        );
        assert_ne!(
            web.member("IamIdentity"),
            CALLER_AKID,
            "case {name:?}: the access key id is a credential identifier, never an identity"
        );
        let airflow_identity = web.member("AirflowIdentity");
        assert_eq!(
            airflow_identity, CALLER_AIRFLOW_IDENTITY,
            "case {name:?}: the reported Apache Airflow user name must be the one that \
             identity maps to, got {airflow_identity:?}"
        );
        assert!(
            (1..=64).contains(&airflow_identity.chars().count()),
            "case {name:?}: AirflowIdentity is @length(1, 64), got {airflow_identity:?}"
        );

        let cli = harness
            .send("POST", &format!("/clitoken/{name}"), None)
            .await;
        assert_eq!(
            cli.status,
            200,
            "case {name:?}: CreateCliToken must succeed for a stored environment, got {} {}",
            cli.status,
            cli.text()
        );
        assert_eq!(
            cli.member("WebServerHostname"),
            expected_host,
            "case {name:?}: CreateCliToken must report the same stored host, got {:?}",
            cli.member("WebServerHostname")
        );
        assert!(
            !cli.member("CliToken").is_empty(),
            "case {name:?}: CreateCliToken must report a token"
        );
        assert_ne!(
            cli.member("CliToken"),
            web.member("WebToken"),
            "case {name:?}: the CLI and web tokens are two separate secrets"
        );

        // A token is minted per call, so a second call does not hand back the first one.
        let again = harness
            .send("POST", &format!("/webtoken/{name}"), None)
            .await;
        assert_ne!(
            again.member("WebToken"),
            web.member("WebToken"),
            "case {name:?}: each call must mint a fresh token"
        );
        // And the host, which is read from state rather than minted, does not change.
        assert_eq!(
            again.member("WebServerHostname"),
            expected_host,
            "case {name:?}: the host must be stable across calls, got {:?}",
            again.member("WebServerHostname")
        );
    }

    // Two environments never share a web server, so neither do the hosts their tokens
    // report.
    let hosts: Vec<String> = {
        let mut hosts = Vec::new();
        for name in names {
            hosts.push(
                harness
                    .send("POST", &format!("/webtoken/{name}"), None)
                    .await
                    .member("WebServerHostname"),
            );
        }
        hosts
    };
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            hosts[left], hosts[right],
            "two environments must not report one web-server host, got {:?} and {:?}",
            hosts[left], hosts[right]
        );
    }
}

#[tokio::test]
async fn invoke_rest_api_refuses_an_input_outside_its_modelled_constraints() {
    let harness = Harness::new();
    assert_eq!(
        harness.create("RestApiEnv").await.status,
        200,
        "the fixture create must succeed"
    );

    // What the operation ANSWERS for an environment that exists — the Apache Airflow REST
    // API call routed over that environment's own state — has its own suite
    // (`tests/all/airflow_rest_api.rs`). What belongs here is the refusal: a request the
    // model's own constraints rule out never reaches that routing.
    //
    // A bad request is a bad request: `RestApiPath` is `@length(1, 64)`.
    let refused = harness
        .send(
            "POST",
            "/restapi/RestApiEnv",
            Some(json!({"Path": "/".repeat(65), "Method": "GET"})),
        )
        .await;
    assert_eq!(
        refused.status,
        400,
        "a Path outside @length(1, 64) must be refused with 400, got {} {}",
        refused.status,
        refused.text()
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "a Path outside its bound is a ValidationException, got {:?}",
        refused.error_code()
    );
}
