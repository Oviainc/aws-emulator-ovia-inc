// SPDX-License-Identifier: CC0-1.0

//! The event a package version's state change announces, end to end over the wire.
//!
//! The RULE under test: **a package version whose state changes in a CodeArtifact
//! repository is announced ONCE on the account's default EventBridge bus, as `source`
//! `aws.codeartifact` / `detail-type` `CodeArtifact Package Version State Change`, naming
//! the version's own PACKAGE ARN in `resources` and carrying, in `detail`, the version's
//! coordinate, the state and revision the change left it at, what the change did to its
//! content and which change of that version it is — whichever plane made the change.** The
//! revision an event announces is therefore the revision `DescribePackageVersion` reports
//! for the same version, and a change nothing made announces nothing.
//!
//! Sources: the EventBridge reference page "Amazon CodeArtifact events", which names the
//! source, the detail type, the default bus and the members of `detail`; and
//! `smithy_query(query="shape", shape="PackageVersionStatus")` for the states a version can
//! be in.
//!
//! Every row drives the production front door — a `MultiServiceRouter` with CodeArtifact,
//! EventBridge and SQS mounted on ONE engine — so what is asserted is the message a client
//! actually receives on a queue a rule targets, not a call into a publisher.
//!
//! The rows differ along what the rule generalises over: WHICH plane changes the version
//! (an npm publish over the repository endpoint, a generic publish through the control
//! plane, a status move, a removal, a copy into another repository, a retention from an
//! upstream repository), WHICH repository the change therefore happened in, whether the
//! package carries a NAMESPACE, and what each change did to the version's content.
//!
//! A further suite walks ONE version through several changes of its own, which is the
//! other dimension of the same rule: each change of a version the repository already holds
//! is announced in its own right, with the next sequence number of that version and a
//! deduplication id of its own, and a call that changes nothing is announced not at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{auth_for, ACCOUNT, REGION};

/// The `source` and `detail-type` CodeArtifact announces a package version state change
/// under, spelled out here rather than imported from the service crate, so a wrong
/// constant in the implementation cannot make this suite pass.
const SOURCE: &str = "aws.codeartifact";
const STATE_CHANGE: &str = "CodeArtifact Package Version State Change";

/// The bus an AWS service's own events go to: the account's default event bus.
const DEFAULT_BUS: &str = "default";

/// How long a row waits for an announcement to reach the queue before it gives up: far
/// above the span the in-process publication takes, and driven off what has ARRIVED rather
/// than off a fixed sleep, so a slow machine delays a row instead of breaking it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// One poll of the queue inside [`DELIVERY_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator a row drives: CodeArtifact publishes the announcement through the
/// in-process callback, EventBridge routes it by its content, and SQS holds what arrived.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
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
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
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

/// One CodeArtifact control-plane call (`restJson1`), answered as its parsed body and
/// status.
async fn codeartifact(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(value.to_string()),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A CodeArtifact control-plane call that must answer 200.
async fn control(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Value {
    let (status, parsed) = codeartifact(router, method, uri, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CodeArtifact {method} {uri}, got {status} with \
         {parsed}"
    );
    parsed
}

/// `PublishPackageVersion`, whose asset bytes are the HTTP PAYLOAD and whose declared
/// digest is the `x-amz-content-sha256` header the model binds `assetSHA256` to.
async fn publish_generic_version(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    content: Vec<u8>,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/octet-stream")
        .header("x-amz-content-sha256", asset_sha256(&content))
        .body(bytes::Bytes::from(content))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from PublishPackageVersion, got {status} with {parsed}"
    );
    parsed
}

/// Where an npm-protocol request is addressed: the domain whose published endpoint host it
/// is sent to, the repository under it, and the path within that endpoint.
struct Addressed<'a> {
    domain: &'a str,
    repository: &'a str,
    path: &'a str,
}

/// One raw npm-protocol request against a repository endpoint, carrying the repository's
/// bearer token rather than a SigV4 signature. It must answer 200.
async fn registry(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    addressed: &Addressed<'_>,
    token: &str,
    body: Option<Vec<u8>>,
) -> Value {
    let Addressed {
        domain,
        repository,
        path,
    } = addressed;
    let host = format!("{domain}-{ACCOUNT}.d.codeartifact.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("/npm/{repository}/{path}"))
        .header("host", host)
        .header("authorization", format!("Bearer {token}"))
        .body(bytes::Bytes::from(body.unwrap_or_default()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the npm endpoint {method} \
         /npm/{repository}/{path}, got {status} with {parsed}"
    );
    parsed
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

/// An ENABLED rule on the default bus matching this suite's source and detail type for
/// `domain`, with `queue_arn` as its one target.
async fn rule(router: &MultiServiceRouter, case: &str, domain: &str, queue_arn: &str) {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [STATE_CHANGE],
        "detail": {"domainName": [domain]},
    });
    events(
        router,
        case,
        "PutRule",
        &json!({
            "Name": format!("r-{domain}"),
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
            "Rule": format!("r-{domain}"),
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

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
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
/// [`DELIVERY_DEADLINE`] passes; answers everything that did arrive.
async fn collect(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
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

/// The bytes a row publishes as a version's tarball. Opaque content: what matters is that
/// one asset arrived with the version.
fn tarball(package: &str, version: &str) -> Vec<u8> {
    format!("tarball of {package}@{version}").into_bytes()
}

/// The npm publish document `npm publish` sends for ONE version: the package's name, its
/// manifest, the `latest` tag and the tarball attached base64-encoded.
fn publish_document(display_name: &str, unscoped: &str, version: &str) -> Vec<u8> {
    use base64::Engine as _;

    let bytes = tarball(unscoped, version);
    json!({
        "_id": display_name,
        "name": display_name,
        "dist-tags": {"latest": version},
        "versions": {
            version: {
                "name": display_name,
                "version": version,
                "description": format!("{display_name} at {version}"),
            }
        },
        "_attachments": {
            format!("{display_name}-{version}.tgz"): {
                "content_type": "application/octet-stream",
                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "length": bytes.len(),
            }
        },
    })
    .to_string()
    .into_bytes()
}

/// The `versionRevision` `DescribePackageVersion` reports for a version — what the
/// announcement's `packageVersionRevision` has to equal.
/// The whole coordinate a version is addressed by, for the control-plane read below.
struct Coordinate<'a> {
    domain: &'a str,
    repository: &'a str,
    format: &'a str,
    namespace: Option<&'a str>,
    package: &'a str,
    version: &'a str,
}

async fn described_revision(
    router: &MultiServiceRouter,
    case: &str,
    coordinate: &Coordinate<'_>,
) -> String {
    let Coordinate {
        domain,
        repository,
        format,
        namespace,
        package,
        version,
    } = coordinate;
    let mut uri = format!(
        "/v1/package/version?domain={domain}&repository={repository}&format={format}\
         &package={package}&version={version}"
    );
    if let Some(namespace) = namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let described = control(router, case, "GET", &uri, None).await;
    member(case, &described, "/packageVersion/revision")
}

/// Which plane a row changes the version through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// `PUT /<package>` over the repository's npm endpoint.
    NpmPublish,
    /// The same, for a package under a scope (an npm namespace).
    ScopedNpmPublish,
    /// `PublishPackageVersion`, which publishes a `generic` package version.
    GenericPublish,
    /// `UpdatePackageVersionsStatus` moving the version to `Unlisted`.
    StatusMove,
    /// `DeletePackageVersions` removing it.
    Removal,
    /// `CopyPackageVersions` into a second repository.
    Copy,
    /// A read through a DOWNSTREAM repository's endpoint, which retains the version the
    /// upstream holds.
    UpstreamRetention,
}

/// What one row does, and the announcement it must produce.
struct Row {
    case: &'static str,
    trigger: Trigger,
    /// The `packageFormat` the announced version carries.
    format: &'static str,
    /// The `packageNamespace` it carries, `None` for a package that has none.
    namespace: Option<&'static str>,
    /// The `operationType`, `packageVersionState` and `sequenceNumber` of the LAST
    /// announcement the row produces, which is the one it asserts.
    operation_type: &'static str,
    version_state: &'static str,
    sequence_number: i64,
    /// Its `changes` object.
    changes: Value,
}

/// RULE: every package version state change is announced once on the default bus, with the
/// revision, state and change facts of the record it left behind.
#[tokio::test]
async fn every_package_version_state_change_is_announced_on_the_default_bus() {
    let rows = [
        Row {
            case: "an npm publish over the repository endpoint",
            trigger: Trigger::NpmPublish,
            format: "npm",
            namespace: None,
            operation_type: "Created",
            version_state: "Published",
            sequence_number: 1,
            changes: json!({
                "assetsAdded": 1,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": true,
                "statusChanged": true,
            }),
        },
        Row {
            case: "an npm publish of a package under a scope",
            trigger: Trigger::ScopedNpmPublish,
            format: "npm",
            namespace: Some("acme"),
            operation_type: "Created",
            version_state: "Published",
            sequence_number: 1,
            changes: json!({
                "assetsAdded": 1,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": true,
                "statusChanged": true,
            }),
        },
        Row {
            case: "a generic publish through the control plane",
            trigger: Trigger::GenericPublish,
            format: "generic",
            namespace: Some("tools"),
            operation_type: "Created",
            version_state: "Published",
            sequence_number: 1,
            // A generic publish carries assets and no metadata document.
            changes: json!({
                "assetsAdded": 1,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": false,
                "statusChanged": true,
            }),
        },
        Row {
            case: "a status move of a published version",
            trigger: Trigger::StatusMove,
            format: "npm",
            namespace: None,
            operation_type: "Updated",
            version_state: "Unlisted",
            // The second change of the same version: the publish was the first.
            sequence_number: 2,
            changes: json!({
                "assetsAdded": 0,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": false,
                "statusChanged": true,
            }),
        },
        Row {
            case: "a removal of a published version",
            trigger: Trigger::Removal,
            format: "npm",
            namespace: None,
            operation_type: "Deleted",
            version_state: "Deleted",
            sequence_number: 2,
            changes: json!({
                "assetsAdded": 0,
                "assetsRemoved": 1,
                "assetsUpdated": 0,
                "metadataUpdated": false,
                "statusChanged": true,
            }),
        },
        Row {
            case: "a copy into another repository",
            trigger: Trigger::Copy,
            format: "npm",
            namespace: None,
            operation_type: "Created",
            version_state: "Published",
            sequence_number: 1,
            changes: json!({
                "assetsAdded": 1,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": true,
                "statusChanged": true,
            }),
        },
        Row {
            case: "a retention into a downstream repository",
            trigger: Trigger::UpstreamRetention,
            format: "npm",
            namespace: None,
            operation_type: "Created",
            version_state: "Published",
            sequence_number: 1,
            changes: json!({
                "assetsAdded": 1,
                "assetsRemoved": 0,
                "assetsUpdated": 0,
                "metadataUpdated": true,
                "statusChanged": true,
            }),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let domain = format!("evt-domain-{index}");
        let repository = format!("evt-repo-{index}");
        // The repository the LAST announcement of this row must name: the one the change
        // happened in.
        let second_repository = format!("evt-other-{index}");
        let package = format!("evt-pkg-{index}");
        let version = "1.0.0";

        let (url, queue_arn) = queue(&router, case, &format!("evt-queue-{index}")).await;
        control(
            &router,
            case,
            "POST",
            &format!("/v1/domain?domain={domain}"),
            Some(json!({})),
        )
        .await;
        rule(&router, case, &domain, &queue_arn).await;
        control(
            &router,
            case,
            "POST",
            &format!("/v1/repository?domain={domain}&repository={repository}"),
            Some(json!({})),
        )
        .await;

        let token = member(
            case,
            &control(
                &router,
                case,
                "POST",
                &format!("/v1/authorization-token?domain={domain}"),
                Some(json!({})),
            )
            .await,
            "/authorizationToken",
        );
        let display_name = match row.namespace {
            Some(scope) if row.format == "npm" => format!("@{scope}/{package}"),
            _ => package.clone(),
        };
        // The path an npm client addresses this package at: a scoped name travels in ONE
        // segment with its separator encoded.
        let registry_path = match row.namespace {
            Some(scope) if row.format == "npm" => format!("@{scope}%2f{package}"),
            _ => package.clone(),
        };

        // The repository whose change the row asserts, and how many announcements it
        // produces before that one.
        let (announced_in, expected_count) = match row.trigger {
            Trigger::NpmPublish | Trigger::ScopedNpmPublish | Trigger::GenericPublish => {
                (repository.clone(), 1)
            }
            Trigger::StatusMove | Trigger::Removal => (repository.clone(), 2),
            Trigger::Copy | Trigger::UpstreamRetention => (second_repository.clone(), 2),
        };

        match row.trigger {
            Trigger::GenericPublish => {
                publish_generic_version(
                    &router,
                    case,
                    &format!(
                        "/v1/package/version/publish?domain={domain}&repository={repository}\
                         &format=generic&namespace={}&package={package}&version={version}\
                         &asset=payload.bin",
                        row.namespace.unwrap_or_default()
                    ),
                    tarball(&package, version),
                )
                .await;
            }
            _ => {
                registry(
                    &router,
                    case,
                    "PUT",
                    &Addressed {
                        domain: &domain,
                        repository: &repository,
                        path: &registry_path,
                    },
                    &token,
                    Some(publish_document(&display_name, &package, version)),
                )
                .await;
            }
        }

        match row.trigger {
            Trigger::StatusMove => {
                control(
                    &router,
                    case,
                    "POST",
                    &format!(
                        "/v1/package/versions/update_status?domain={domain}\
                         &repository={repository}&format=npm&package={package}"
                    ),
                    Some(json!({"versions": [version], "targetStatus": "Unlisted"})),
                )
                .await;
            }
            Trigger::Removal => {
                control(
                    &router,
                    case,
                    "POST",
                    &format!(
                        "/v1/package/versions/delete?domain={domain}&repository={repository}\
                         &format=npm&package={package}"
                    ),
                    Some(json!({"versions": [version]})),
                )
                .await;
            }
            Trigger::Copy => {
                control(
                    &router,
                    case,
                    "POST",
                    &format!(
                        "/v1/repository?domain={domain}&repository={second_repository}"
                    ),
                    Some(json!({})),
                )
                .await;
                control(
                    &router,
                    case,
                    "POST",
                    &format!(
                        "/v1/package/versions/copy?domain={domain}\
                         &source-repository={repository}&destination-repository={second_repository}\
                         &format=npm&package={package}"
                    ),
                    Some(json!({"versions": [version]})),
                )
                .await;
            }
            Trigger::UpstreamRetention => {
                control(
                    &router,
                    case,
                    "POST",
                    &format!("/v1/repository?domain={domain}&repository={second_repository}"),
                    Some(json!({"upstreams": [{"repositoryName": repository}]})),
                )
                .await;
                // A READ through the downstream endpoint: the version it does not hold is
                // resolved from its upstream and retained in it.
                registry(
                    &router,
                    case,
                    "GET",
                    &Addressed {
                        domain: &domain,
                        repository: &second_repository,
                        path: &registry_path,
                    },
                    &token,
                    None,
                )
                .await;
            }
            Trigger::NpmPublish
            | Trigger::ScopedNpmPublish
            | Trigger::GenericPublish => {}
        }

        let arrived = collect(&router, case, &url, expected_count).await;
        assert_eq!(
            arrived.len(),
            expected_count,
            "case {case:?}: expected {expected_count} announcement(s) on the queue, got {} in \
             {arrived:?}",
            arrived.len()
        );
        let envelope = arrived
            .iter()
            .find(|envelope| {
                envelope.pointer("/detail/repositoryName") == Some(&json!(announced_in))
                    && envelope.pointer("/detail/operationType")
                        == Some(&json!(row.operation_type))
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an announcement of a {:?} change in {announced_in:?}, \
                     got {arrived:?}",
                    row.operation_type
                )
            });

        // The revision the announcement carries is the one a read of that version reports
        // — except for a removal, where the version is gone and the revision announced is
        // the one it last had.
        let expected_revision = match row.trigger {
            Trigger::Removal => member(case, envelope, "/detail/packageVersionRevision"),
            _ => {
                described_revision(
                    &router,
                    case,
                    &Coordinate {
                        domain: &domain,
                        repository: &announced_in,
                        format: row.format,
                        namespace: row.namespace,
                        package: &package,
                        version,
                    },
                )
                .await
            }
        };
        let expected_arn = format!(
            "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/{domain}/{announced_in}/{}/{}/{}",
            row.format,
            row.namespace.unwrap_or_default(),
            package
        );

        for (pointer, expected) in [
            ("/source", json!(SOURCE)),
            ("/detail-type", json!(STATE_CHANGE)),
            ("/account", json!(ACCOUNT)),
            ("/region", json!(REGION)),
            ("/resources", json!([expected_arn])),
            ("/detail/domainName", json!(&domain)),
            ("/detail/domainOwner", json!(ACCOUNT)),
            ("/detail/repositoryName", json!(&announced_in)),
            ("/detail/repositoryAdministrator", json!(ACCOUNT)),
            ("/detail/packageFormat", json!(row.format)),
            ("/detail/packageNamespace", json!(row.namespace)),
            ("/detail/packageName", json!(&package)),
            ("/detail/packageVersion", json!(version)),
            ("/detail/packageVersionState", json!(row.version_state)),
            ("/detail/packageVersionRevision", json!(expected_revision)),
            ("/detail/operationType", json!(row.operation_type)),
            ("/detail/sequenceNumber", json!(row.sequence_number)),
            ("/detail/changes", row.changes.clone()),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} to be {expected}, got {actual} in {envelope}"
            );
        }
        assert!(
            envelope
                .pointer("/detail/eventDeduplicationId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the announcement to carry a deduplication id, got \
             {envelope}"
        );
        assert!(
            envelope["time"].as_str().is_some_and(|at| at.ends_with('Z')),
            "case {case:?}: expected the envelope to state when the version changed, got \
             {envelope}"
        );
    }
}

/// RULE: a call that changes NO package version announces nothing — the announcement is of
/// a state change, not of a request.
///
/// The rows span the ways a request touches a version without changing it: a status move to
/// the status the version is already in, a status move the store refuses per version, and a
/// read of a packument the repository already holds.
#[tokio::test]
async fn a_call_that_changes_no_version_announces_nothing() {
    let rows: [(&str, &str, Value); 3] = [
        (
            "a status move to the status it already has",
            "/v1/package/versions/update_status",
            json!({"versions": ["1.0.0"], "targetStatus": "Published"}),
        ),
        (
            "a status move of a version nothing holds",
            "/v1/package/versions/update_status",
            json!({"versions": ["9.9.9"], "targetStatus": "Unlisted"}),
        ),
        (
            "a removal of a version nothing holds",
            "/v1/package/versions/delete",
            json!({"versions": ["9.9.9"]}),
        ),
    ];

    for (index, (case, route, body)) in rows.iter().enumerate() {
        let router = emulator();
        let domain = format!("quiet-domain-{index}");
        let repository = format!("quiet-repo-{index}");
        let package = format!("quiet-pkg-{index}");

        let (url, queue_arn) = queue(&router, case, &format!("quiet-queue-{index}")).await;
        control(
            &router,
            case,
            "POST",
            &format!("/v1/domain?domain={domain}"),
            Some(json!({})),
        )
        .await;
        rule(&router, case, &domain, &queue_arn).await;
        control(
            &router,
            case,
            "POST",
            &format!("/v1/repository?domain={domain}&repository={repository}"),
            Some(json!({})),
        )
        .await;
        let token = member(
            case,
            &control(
                &router,
                case,
                "POST",
                &format!("/v1/authorization-token?domain={domain}"),
                Some(json!({})),
            )
            .await,
            "/authorizationToken",
        );
        registry(
            &router,
            case,
            "PUT",
            &Addressed {
                domain: &domain,
                repository: &repository,
                path: &package,
            },
            &token,
            Some(publish_document(&package, &package, "1.0.0")),
        )
        .await;

        // The publish's own announcement, which must be the ONLY one.
        let published = collect(&router, case, &url, 1).await;
        assert_eq!(
            published.len(),
            1,
            "case {case:?}: the publish itself must be announced once, got {published:?}"
        );

        control(
            &router,
            case,
            "POST",
            &format!(
                "{route}?domain={domain}&repository={repository}&format=npm&package={package}"
            ),
            Some(body.clone()),
        )
        .await;
        // A read of what the repository already holds, which is not a change either.
        registry(
            &router,
            case,
            "GET",
            &Addressed {
                domain: &domain,
                repository: &repository,
                path: &package,
            },
            &token,
            None,
        )
        .await;

        // Longer than the span the publish's own announcement took to arrive above, so an
        // empty queue is evidence that nothing was announced rather than evidence of
        // having looked too early.
        tokio::time::sleep(Duration::from_millis(750)).await;
        let withheld = drain(&router, case, &url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a call that changed no version must announce nothing, got \
             {withheld:?}"
        );
    }
}

/// What the announcement of one [`Step`] must say about the change that step made.
struct Announced {
    operation_type: &'static str,
    version_state: &'static str,
    sequence_number: i64,
    changes: Value,
}

/// One publish into the SAME generic package version, and the announcement it must
/// produce — `None` for a publish that leaves the version exactly as it stood, which is
/// not a state change and so is announced not at all.
struct Step {
    case: &'static str,
    /// The asset this publish carries; its bytes are derived from the name, so two steps
    /// naming different assets publish different content.
    asset: &'static str,
    /// Whether the version stays open for further assets after this publish —
    /// `unfinished`, the member that keeps it in `Unfinished`
    /// (`smithy_query(query="operation",operation="PublishPackageVersion")`).
    unfinished: bool,
    announced: Option<Announced>,
}

/// RULE: a change to a version the repository ALREADY holds is announced in its own right
/// — as an `Updated` operation carrying what that change did to the version, the state the
/// version now has and the NEXT sequence number of that version, under a deduplication id
/// of its own — while a publish that changes nothing is announced not at all.
///
/// The rows walk one generic version through its whole life over the wire, which is the
/// dimension the `Created`-per-row table above does not span: the asset that creates it
/// while it stays open, that identical asset repeated, a second asset, and the final asset
/// that closes it at `Published`. Each row's `packageVersionRevision` is checked against
/// the revision `DescribePackageVersion` reports at that point, so an announcement cannot
/// carry a revision no read of the version agrees with.
#[tokio::test]
async fn each_change_of_a_held_version_is_announced_with_the_next_sequence_number() {
    let steps = [
        Step {
            case: "the asset that creates the version, left unfinished",
            asset: "part-one.bin",
            unfinished: true,
            announced: Some(Announced {
                operation_type: "Created",
                version_state: "Unfinished",
                sequence_number: 1,
                // A generic publish carries assets and no metadata document.
                changes: json!({
                    "assetsAdded": 1,
                    "assetsRemoved": 0,
                    "assetsUpdated": 0,
                    "metadataUpdated": false,
                    "statusChanged": true,
                }),
            }),
        },
        Step {
            case: "the same asset again, byte for byte, into the open version",
            asset: "part-one.bin",
            unfinished: true,
            announced: None,
        },
        Step {
            case: "a second asset into the open version",
            asset: "part-two.bin",
            unfinished: true,
            announced: Some(Announced {
                operation_type: "Updated",
                version_state: "Unfinished",
                sequence_number: 2,
                // The version gained an asset and stayed in the status it was in.
                changes: json!({
                    "assetsAdded": 1,
                    "assetsRemoved": 0,
                    "assetsUpdated": 0,
                    "metadataUpdated": false,
                    "statusChanged": false,
                }),
            }),
        },
        Step {
            case: "the final asset, which closes the version at Published",
            asset: "part-three.bin",
            unfinished: false,
            announced: Some(Announced {
                operation_type: "Updated",
                version_state: "Published",
                sequence_number: 3,
                changes: json!({
                    "assetsAdded": 1,
                    "assetsRemoved": 0,
                    "assetsUpdated": 0,
                    "metadataUpdated": false,
                    "statusChanged": true,
                }),
            }),
        },
    ];

    let router = emulator();
    let domain = "seq-domain";
    let repository = "seq-repo";
    let namespace = "tools";
    let package = "seq-pkg";
    let version = "2.1.0";
    let setup = "the version's own repository";

    let (url, queue_arn) = queue(&router, setup, "seq-queue").await;
    control(
        &router,
        setup,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(json!({})),
    )
    .await;
    rule(&router, setup, domain, &queue_arn).await;
    control(
        &router,
        setup,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(json!({})),
    )
    .await;

    let expected_arn = format!(
        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/{domain}/{repository}/generic/\
         {namespace}/{package}"
    );
    // Every deduplication id announced so far: one change, one id.
    let mut announced_ids: Vec<String> = Vec::new();

    for step in &steps {
        let case = step.case;
        publish_generic_version(
            &router,
            case,
            &format!(
                "/v1/package/version/publish?domain={domain}&repository={repository}\
                 &format=generic&namespace={namespace}&package={package}&version={version}\
                 &asset={}&unfinished={}",
                step.asset, step.unfinished
            ),
            tarball(step.asset, version),
        )
        .await;
        let revision = described_revision(
            &router,
            case,
            &Coordinate {
                domain,
                repository,
                format: "generic",
                namespace: Some(namespace),
                package,
                version,
            },
        )
        .await;

        let Some(expected) = &step.announced else {
            // Longer than the span the announcements of the other steps took to arrive, so
            // an empty queue is evidence that nothing was announced rather than evidence of
            // having looked too early.
            tokio::time::sleep(Duration::from_millis(750)).await;
            let withheld = drain(&router, case, &url).await;
            assert!(
                withheld.is_empty(),
                "case {case:?}: a publish that changed nothing must announce nothing, got \
                 {withheld:?}"
            );
            continue;
        };

        let arrived = collect(&router, case, &url, 1).await;
        assert_eq!(
            arrived.len(),
            1,
            "case {case:?}: expected exactly one announcement on the queue, got {} in \
             {arrived:?}",
            arrived.len()
        );
        let envelope = &arrived[0];
        for (pointer, wanted) in [
            ("/source", json!(SOURCE)),
            ("/detail-type", json!(STATE_CHANGE)),
            ("/resources", json!([&expected_arn])),
            ("/detail/domainName", json!(domain)),
            ("/detail/repositoryName", json!(repository)),
            ("/detail/packageFormat", json!("generic")),
            ("/detail/packageNamespace", json!(namespace)),
            ("/detail/packageName", json!(package)),
            ("/detail/packageVersion", json!(version)),
            ("/detail/packageVersionState", json!(expected.version_state)),
            ("/detail/packageVersionRevision", json!(&revision)),
            ("/detail/operationType", json!(expected.operation_type)),
            ("/detail/sequenceNumber", json!(expected.sequence_number)),
            ("/detail/changes", expected.changes.clone()),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected {pointer} to be {wanted}, got {actual} in {envelope}"
            );
        }

        let id = member(case, envelope, "/detail/eventDeduplicationId");
        assert!(
            !announced_ids.contains(&id),
            "case {case:?}: each change carries its own deduplication id, but {id:?} was \
             announced before, among {announced_ids:?}"
        );
        announced_ids.push(id);
    }

    let announced_count = steps.iter().filter(|step| step.announced.is_some()).count();
    assert_eq!(
        announced_ids.len(),
        announced_count,
        "case \"the whole progression\": expected {announced_count} announcement(s) in all, got \
         {announced_ids:?}"
    );
}

/// The SHA-256 of `content`, hex-encoded — the `assetSHA256` a generic publish declares
/// for the bytes it sends.
fn asset_sha256(content: &[u8]) -> String {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}
