// SPDX-License-Identifier: CC0-1.0

//! A CodeArtifact publish and an upstream retention each reach the EventBridge rules that
//! match them, through the WHOLE emulator.
//!
//! The RULE under test: **a package version whose state changes in a CodeArtifact
//! repository is announced once on the account's default EventBridge bus as `source`
//! `aws.codeartifact` / `detail-type` `CodeArtifact Package Version State Change`, naming
//! that version's package ARN in `resources` and the repository the change happened in in
//! `detail`; EventBridge then routes it by its CONTENT, so a rule whose `EventPattern`
//! matches delivers it to that rule's targets and a rule whose pattern does not match
//! receives none of them.** A publish over a repository's npm endpoint is such a change,
//! and so is the RETENTION a downstream repository makes when a request resolves a version
//! from one of its upstream repositories — each announced for the repository it happened
//! in. Source: the EventBridge reference page "Amazon CodeArtifact events".
//!
//! This runs the path a client runs it over: three services (CodeArtifact publishes,
//! EventBridge routes, SQS holds what arrived) reached through the whole emulator — the
//! same `build_router` service set the binary serves — so the in-process publication is
//! claimed and routed by whichever mounted model a client's `events`-scoped call would
//! reach, not by a hand-picked pair of services. The data-plane requests are the raw
//! npm-protocol ones, carrying the repository's bearer token rather than a SigV4
//! signature, so the announcement is made on the request path a package manager actually
//! takes.
//!
//! What only this suite can reach: the `events` signing name is claimed by several mounted
//! models here, and the npm endpoint host is claimed by no service's own endpoint, so both
//! halves of the path have to resolve the way a client's would.
//! `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/tests/all/package_version_state_change_events.rs`
//! pins the same rule over a TABLE of triggers (a publish on either plane, a status move,
//! a removal, a copy, a retention) and pins each announced `detail` against what
//! `DescribePackageVersion` reports for the same version.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region the repository endpoint
/// host names.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` CodeArtifact announces a package version state change
/// under, spelled out here rather than imported from the service crate, so a wrong
/// constant in the implementation cannot make this suite pass.
const SOURCE: &str = "aws.codeartifact";
const STATE_CHANGE: &str = "CodeArtifact Package Version State Change";

/// The bus an AWS service's own events go to.
const DEFAULT_BUS: &str = "default";

/// How long the suite waits for an announcement to arrive before giving up.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// One poll of a queue inside [`DELIVERY_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long the suite leaves the non-matching rule's queue alone before reading it: longer
/// than the span the matching rule's announcements took to arrive, so an empty read is
/// evidence the rule was not a delivery target rather than evidence of having looked too
/// early.
const QUIET_SPAN: Duration = Duration::from_millis(750);

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it for
/// the binary.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(Arc::clone(&engine));
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The status and parsed body of one response.
fn answered(response: &http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `X-Amz-Target` RPC call against the service whose signing name is `signing_name`,
/// which must answer 200.
async fn rpc(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`).
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "events", "AWSEvents", "1.1", operation, body).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// One CodeArtifact control-plane call (`restJson1`), which must answer 200.
async fn codeartifact(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Value {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://codeartifact.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", authorization("codeartifact"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CodeArtifact {method} {uri}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One raw npm-protocol request against the repository endpoint
/// `GetRepositoryEndpoint` hands out, carrying the repository's bearer token.
async fn registry(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    endpoint: &str,
    path: &str,
    token: &str,
    body: Option<Vec<u8>>,
) -> Value {
    let url = format!("{}/{path}", endpoint.trim_end_matches('/'));
    let host = url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let request = http::Request::builder()
        .method(method)
        .uri(&url)
        .header("host", host)
        .header("authorization", format!("Bearer {token}"))
        .body(Bytes::from(body.unwrap_or_default()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the npm endpoint {method} {url}, got {status} \
         with {parsed}"
    );
    parsed
}

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// An SQS queue carrying the grant a rule's target needs — EventBridge makes the
/// target's `sqs:SendMessage` as its own service principal and "relies on
/// resource-based policies" for a queue (`PutTargets`), so a queue that has granted it
/// nothing receives nothing — answering its url and the ARN a rule names it by.
async fn queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({"QueueName": name})).await;
    let url = member(case, &created, "/QueueUrl");
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = member(case, &attributes, "/Attributes/QueueArn");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({"QueueUrl": &url, "Attributes": {"Policy": policy}}),
    )
    .await;
    (url, arn)
}

/// An ENABLED rule on the default bus carrying `pattern`, with `queue_arn` as its one
/// target.
async fn rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    pattern: &Value,
    queue_arn: &str,
) {
    events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": DEFAULT_BUS,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    let targeted = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": DEFAULT_BUS,
            "Targets": [{"Id": "t1", "Arn": queue_arn}],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: every target must be accepted, got {targeted}"
    );
}

/// Every envelope now waiting on the queue at `url`.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Collect envelopes off the queue at `url` until `wanted` have arrived or
/// [`DELIVERY_DEADLINE`] passes.
async fn collect(router: &MultiServiceRouter, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let started = std::time::Instant::now();
    let mut arrived: Vec<Value> = Vec::new();
    while arrived.len() < wanted && started.elapsed() < DELIVERY_DEADLINE {
        arrived.extend(drain(router, case, url).await);
        if arrived.len() < wanted {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    arrived
}

/// The npm publish document `npm publish` sends for one version.
fn publish_document(package: &str, version: &str) -> Vec<u8> {
    let tarball = format!("tarball of {package}@{version}").into_bytes();
    json!({
        "_id": package,
        "name": package,
        "dist-tags": {"latest": version},
        "versions": {
            version: {"name": package, "version": version, "description": "a published package"}
        },
        "_attachments": {
            format!("{package}-{version}.tgz"): {
                "content_type": "application/octet-stream",
                // The runtime's own standard-alphabet encoder, which is what npm's
                // `_attachments` carries a tarball with.
                "data": aws_smithy_types::base64::encode(&tarball),
                "length": tarball.len(),
            }
        },
    })
    .to_string()
    .into_bytes()
}

/// RULE: the publish and the downstream retention are each announced on the default bus and
/// delivered to the rule whose pattern matches them — and to no rule whose pattern does
/// not.
#[tokio::test]
async fn a_publish_and_an_upstream_retention_each_reach_the_matching_rule() {
    let case = "an npm publish into an upstream, then a read through its downstream";
    let router = router();
    let domain = "sim-ca-domain";
    let upstream = "sim-ca-upstream";
    let downstream = "sim-ca-downstream";
    let package = "sim-ca-package";
    let version = "1.0.0";

    // The queue the matching rule targets, and the queue only a rule that cannot match
    // targets.
    let (matching_url, matching_arn) = queue(&router, case, "ca-match").await;
    let (other_url, other_arn) = queue(&router, case, "ca-other").await;

    codeartifact(
        &router,
        case,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(json!({})),
    )
    .await;
    codeartifact(
        &router,
        case,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={upstream}"),
        Some(json!({})),
    )
    .await;
    codeartifact(
        &router,
        case,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={downstream}"),
        Some(json!({"upstreams": [{"repositoryName": upstream}]})),
    )
    .await;

    // The matching rule filters on the domain the changes happen in, inside `detail`; the
    // other names a domain no announcement of this run can carry.
    rule(
        &router,
        case,
        "ca-match",
        &json!({
            "source": [SOURCE],
            "detail-type": [STATE_CHANGE],
            "detail": {"domainName": [domain]},
        }),
        &matching_arn,
    )
    .await;
    rule(
        &router,
        case,
        "ca-other",
        &json!({
            "source": [SOURCE],
            "detail-type": [STATE_CHANGE],
            "detail": {"domainName": ["sim-ca-another-domain"]},
        }),
        &other_arn,
    )
    .await;

    let token = member(
        case,
        &codeartifact(
            &router,
            case,
            "POST",
            &format!("/v1/authorization-token?domain={domain}"),
            Some(json!({})),
        )
        .await,
        "/authorizationToken",
    );
    // The endpoints the client is handed, used exactly as handed out.
    let upstream_endpoint = member(
        case,
        &codeartifact(
            &router,
            case,
            "GET",
            &format!(
                "/v1/repository/endpoint?domain={domain}&repository={upstream}&format=npm"
            ),
            None,
        )
        .await,
        "/repositoryEndpoint",
    );
    let downstream_endpoint = member(
        case,
        &codeartifact(
            &router,
            case,
            "GET",
            &format!(
                "/v1/repository/endpoint?domain={domain}&repository={downstream}&format=npm"
            ),
            None,
        )
        .await,
        "/repositoryEndpoint",
    );

    // The publish, over the endpoint the control plane advertised.
    registry(
        &router,
        case,
        "PUT",
        &upstream_endpoint,
        package,
        &token,
        Some(publish_document(package, version)),
    )
    .await;
    let published_revision = member(
        case,
        &codeartifact(
            &router,
            case,
            "GET",
            &format!(
                "/v1/package/version?domain={domain}&repository={upstream}&format=npm\
                 &package={package}&version={version}"
            ),
            None,
        )
        .await,
        "/packageVersion/revision",
    );

    // The read through the DOWNSTREAM endpoint: the version it does not hold is resolved
    // from its upstream and retained in it, which is a state change of its own.
    registry(
        &router,
        case,
        "GET",
        &downstream_endpoint,
        package,
        &token,
        None,
    )
    .await;
    let retained_revision = member(
        case,
        &codeartifact(
            &router,
            case,
            "GET",
            &format!(
                "/v1/package/version?domain={domain}&repository={downstream}&format=npm\
                 &package={package}&version={version}"
            ),
            None,
        )
        .await,
        "/packageVersion/revision",
    );

    let arrived = collect(&router, case, &matching_url, 2).await;
    assert_eq!(
        arrived.len(),
        2,
        "case {case:?}: expected the publish and the retention to be announced once each, got \
         {arrived:?}"
    );

    for (repository, revision) in [
        (upstream, &published_revision),
        (downstream, &retained_revision),
    ] {
        let envelope = arrived
            .iter()
            .find(|envelope| {
                envelope.pointer("/detail/repositoryName") == Some(&json!(repository))
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an announcement naming repository \
                     {repository:?}, got {arrived:?}"
                )
            });
        let expected_arn = format!(
            "arn:aws:codeartifact:{REGION}:{}:package/{domain}/{repository}/npm//{package}",
            engine::EMULATOR_ACCOUNT_ID
        );
        for (pointer, expected) in [
            ("/source", json!(SOURCE)),
            ("/detail-type", json!(STATE_CHANGE)),
            ("/account", json!(engine::EMULATOR_ACCOUNT_ID)),
            ("/region", json!(REGION)),
            ("/resources", json!([expected_arn])),
            ("/detail/domainName", json!(domain)),
            ("/detail/packageName", json!(package)),
            ("/detail/packageVersion", json!(version)),
            ("/detail/packageVersionState", json!("Published")),
            ("/detail/packageVersionRevision", json!(revision)),
            ("/detail/operationType", json!("Created")),
            ("/detail/sequenceNumber", json!(1)),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} of the {repository:?} announcement to be \
                 {expected}, got {actual} in {envelope}"
            );
        }
    }

    // The negative half: the rule naming another domain matched nothing, so its own queue
    // is still empty — and stays empty over a further span of the clock.
    tokio::time::sleep(QUIET_SPAN).await;
    let withheld = drain(&router, case, &other_url).await;
    assert!(
        withheld.is_empty(),
        "case {case:?}: a rule whose pattern names another domain must receive nothing, got \
         {withheld:?}"
    );
}
