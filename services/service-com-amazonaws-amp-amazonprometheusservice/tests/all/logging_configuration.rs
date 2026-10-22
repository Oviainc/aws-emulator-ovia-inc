// SPDX-License-Identifier: CC0-1.0

//! The workspace's rules-and-alerting logging configuration, end to end over HTTP:
//! `CreateLoggingConfiguration` persists it and `DescribeLoggingConfiguration` reads
//! that same record back.
//!
//! The rule under test: a workspace holds at most ONE rules-and-alerting logging
//! configuration, created against `/workspaces/{workspaceId}/logging` and reported from
//! what was PERSISTED — whatever `logGroupArn` the request carried — starting in a
//! transient `CREATING` status the clock owes a move to `ACTIVE`, and gone with the
//! workspace it belongs to. Each test spans a SET of inputs: the rows differ in the log
//! group's partition, region, name form and name length, and in whether the call carries
//! an idempotency token.
//!
//! Like `workspace_lifecycle.rs`, every suite drives the real request path —
//! `MultiServiceRouter::dispatch` over this service mounted on one shared engine — so
//! routing, the `restJson1` wire shapes, the modelled response statuses and the modelled
//! error envelopes are all exercised. The harness is suite-local for the same reason it
//! is there: each suite stands alone on the pieces it needs.

use bytes::Bytes;
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    LOGGING_STATUS_ACTIVE, LOGGING_STATUS_CREATING, TRANSITION_DELAY,
};
use service_com_amazonaws_amp_amazonprometheusservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One response, as a test reads it: the HTTP status and the parsed body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: serde_json::Value,
}

impl Reply {
    /// The error code a modelled failure renders as the JSON envelope's `__type`.
    fn error_code(&self) -> String {
        text(&self.body["__type"])
    }
}

/// The emulator under test: this service on its own shared engine.
struct Amp {
    router: MultiServiceRouter,
}

impl Amp {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one request, signed (unverified — authentication is off here) for
    /// `region`.
    async fn call(&self, method: &str, uri: &str, region: &str, body: &str) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("aps.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/aps/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body.to_owned()))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// A fresh workspace, as every logging configuration needs one to hang off.
    async fn workspace(&self, region: &str, alias: &str) -> String {
        let created = self
            .call(
                "POST",
                "/workspaces",
                region,
                &serde_json::json!({ "alias": alias }).to_string(),
            )
            .await;
        assert_eq!(created.status, 202, "create workspace {alias}: {created:?}");
        text(&created.body["workspaceId"])
    }

    async fn create_logging(
        &self,
        region: &str,
        workspace_id: &str,
        log_group_arn: &str,
        client_token: Option<&str>,
    ) -> Reply {
        let mut body = serde_json::json!({ "logGroupArn": log_group_arn });
        if let Some(token) = client_token {
            body["clientToken"] = serde_json::Value::String(token.to_string());
        }
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/logging"),
            region,
            &body.to_string(),
        )
        .await
    }

    async fn describe_logging(&self, region: &str, workspace_id: &str) -> Reply {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/logging"),
            region,
            "",
        )
        .await
    }

    async fn delete_workspace(&self, region: &str, workspace_id: &str) -> Reply {
        self.call("DELETE", &format!("/workspaces/{workspace_id}"), region, "")
            .await
    }
}

fn text(value: &serde_json::Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// A CloudWatch log group ARN satisfying `LogGroupArn`'s pattern
/// `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`
/// — the account segment comes from the engine, never from a literal.
fn log_group_arn(partition: &str, region: &str, log_group_name: &str) -> String {
    format!(
        "arn:{partition}:logs:{region}:{}:log-group:{log_group_name}:*",
        engine::emulator_account_id()
    )
}

/// Sleep past the span the emulator gives a pending lifecycle transition. Blocking is
/// fine: each suite drives one request at a time.
fn wait_for_due_transition() {
    std::thread::sleep(TRANSITION_DELAY + std::time::Duration::from_millis(250));
}

/// One row of the input class a logging configuration spans: the log group it publishes
/// to, and whether the call carries an idempotency token.
struct Case {
    name: &'static str,
    region: &'static str,
    partition: &'static str,
    /// `LogGroupArn` admits `[A-Za-z0-9.\-_#/]{1,512}` as the log group name, so the rows
    /// walk that character set and both of its published length bounds.
    log_group_name: String,
    client_token: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "shortest log group name",
            region: "us-east-1",
            partition: "aws",
            log_group_name: "a".to_string(),
            client_token: None,
        },
        Case {
            name: "slashed path, with a token",
            region: "us-west-2",
            partition: "aws",
            log_group_name: "/aws/prometheus/rules".to_string(),
            client_token: Some("token-slashed"),
        },
        Case {
            name: "every admitted punctuation",
            region: "eu-central-1",
            partition: "aws",
            log_group_name: "metrics.rules-log_group#1/nested".to_string(),
            client_token: Some("token-punctuation"),
        },
        Case {
            name: "longest log group name",
            region: "ap-southeast-2",
            partition: "aws",
            log_group_name: "l".repeat(512),
            client_token: None,
        },
        Case {
            name: "another partition",
            region: "us-gov-west-1",
            partition: "aws-us-gov",
            log_group_name: "gov/rules".to_string(),
            client_token: Some("token-gov"),
        },
    ]
}

/// The statuses this emulator reports are entries of the enum the model declares — the
/// catalog comes from the model, not from this test.
#[test]
fn reported_statuses_are_modelled_logging_configuration_status_codes() {
    for status in [LOGGING_STATUS_CREATING, LOGGING_STATUS_ACTIVE] {
        assert!(
            LOGGING_CONFIGURATION_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled LoggingConfigurationStatusCode \
             ({LOGGING_CONFIGURATION_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create persists the configuration and a describe reads that same record back: one
/// row per thing the rule generalises over — the log group's partition, region, name form
/// and name length, and the presence of an idempotency token.
#[tokio::test]
async fn create_persists_the_configuration_and_describe_reads_it_back() {
    for case in cases() {
        let amp = Amp::new();
        let workspace_id = amp.workspace(case.region, "logging").await;
        let arn = log_group_arn(case.partition, case.region, &case.log_group_name);

        let created = amp
            .create_logging(case.region, &workspace_id, &arn, case.client_token)
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: CreateLoggingConfiguration must answer 202, got {created:?}",
            case.name
        );
        assert_eq!(
            text(&created.body["status"]["statusCode"]),
            LOGGING_STATUS_CREATING,
            "case {:?}: a new configuration starts CREATING, got {created:?}",
            case.name
        );
        assert!(
            created.body["status"]["statusReason"].is_null(),
            "case {:?}: nothing failed, so statusReason must be absent, got {created:?}",
            case.name
        );

        let described = amp.describe_logging(case.region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeLoggingConfiguration must answer 200, got {described:?}",
            case.name
        );
        let metadata = &described.body["loggingConfiguration"];
        assert_eq!(
            text(&metadata["workspace"]),
            workspace_id,
            "case {:?}: the configuration is reported for its own workspace, got {described:?}",
            case.name
        );
        assert_eq!(
            text(&metadata["logGroupArn"]),
            arn,
            "case {:?}: logGroupArn must be the persisted value, got {described:?}",
            case.name
        );
        assert_eq!(
            text(&metadata["status"]["statusCode"]),
            LOGGING_STATUS_CREATING,
            "case {:?}: the describe reports the stored status, got {described:?}",
            case.name
        );
        let created_at = metadata["createdAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: createdAt must be a timestamp, got {described:?}",
                case.name
            )
        });
        let modified_at = metadata["modifiedAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: modifiedAt must be a timestamp, got {described:?}",
                case.name
            )
        });
        assert!(
            created_at > 0.0,
            "case {:?}: createdAt must be populated, got {created_at}",
            case.name
        );
        assert_eq!(
            modified_at, created_at,
            "case {:?}: a configuration nothing has changed was modified when it was created",
            case.name
        );

        // A read changes nothing a later caller sees.
        let again = amp.describe_logging(case.region, &workspace_id).await;
        assert_eq!(
            again.body, described.body,
            "case {:?}: a describe must not change what the next describe reports",
            case.name
        );
    }
}

/// `CREATING` is transient: the clock owes the move to `ACTIVE`, and nothing else about
/// the configuration changes with it.
#[tokio::test]
async fn a_creating_configuration_becomes_active_once_the_transition_is_due() {
    let amp = Amp::new();
    let workspace_id = amp.workspace("us-west-2", "transition").await;
    let arn = log_group_arn("aws", "us-west-2", "/aws/prometheus/transition");
    amp.create_logging("us-west-2", &workspace_id, &arn, None)
        .await;
    let before = amp.describe_logging("us-west-2", &workspace_id).await;
    assert_eq!(
        text(&before.body["loggingConfiguration"]["status"]["statusCode"]),
        LOGGING_STATUS_CREATING,
        "case \"straight after create\": the transient status must still be observable, \
         got {before:?}"
    );

    wait_for_due_transition();

    let after = amp.describe_logging("us-west-2", &workspace_id).await;
    let metadata = &after.body["loggingConfiguration"];
    assert_eq!(
        text(&metadata["status"]["statusCode"]),
        LOGGING_STATUS_ACTIVE,
        "case \"once due\": the configuration must become ACTIVE, got {after:?}"
    );
    assert_eq!(
        text(&metadata["logGroupArn"]),
        arn,
        "case \"once due\": the transition must not change logGroupArn, got {after:?}"
    );
    assert_eq!(
        metadata["createdAt"], before.body["loggingConfiguration"]["createdAt"],
        "case \"once due\": the transition must not change createdAt, got {after:?}"
    );
    assert_eq!(
        metadata["modifiedAt"], before.body["loggingConfiguration"]["modifiedAt"],
        "case \"once due\": the clock moving the status on is not a change to the \
         configuration, got {after:?}"
    );
}

/// `CreateLoggingConfiguration` is `@idempotent` and its `clientToken` is an
/// `@idempotencyToken`: re-sending the call that created the stored configuration answers
/// with that configuration, while a create naming a different log group is refused —
/// a workspace holds only one. One row per form the second call takes.
#[tokio::test]
async fn re_creating_a_configuration_is_idempotent_or_refused() {
    /// Each row: the second call's token and log group, and whether it is the same call.
    struct Replay {
        name: &'static str,
        token: Option<&'static str>,
        same_log_group: bool,
        replays: bool,
    }

    let first_token = "first-token";
    for row in [
        Replay {
            name: "same token, same log group",
            token: Some(first_token),
            same_log_group: true,
            replays: true,
        },
        Replay {
            name: "same token, different log group",
            token: Some(first_token),
            same_log_group: false,
            replays: true,
        },
        Replay {
            name: "no token, same log group",
            token: None,
            same_log_group: true,
            replays: true,
        },
        Replay {
            name: "different token, same log group",
            token: Some("second-token"),
            same_log_group: true,
            replays: true,
        },
        Replay {
            name: "different token, different log group",
            token: Some("second-token"),
            same_log_group: false,
            replays: false,
        },
        Replay {
            name: "no token, different log group",
            token: None,
            same_log_group: false,
            replays: false,
        },
    ] {
        let amp = Amp::new();
        let workspace_id = amp.workspace("us-west-2", "idempotent").await;
        let first = log_group_arn("aws", "us-west-2", "/aws/prometheus/first");
        let other = log_group_arn("aws", "us-west-2", "/aws/prometheus/second");
        let created = amp
            .create_logging("us-west-2", &workspace_id, &first, Some(first_token))
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the first create must succeed, got {created:?}",
            row.name
        );

        let log_group = if row.same_log_group { &first } else { &other };
        let second = amp
            .create_logging("us-west-2", &workspace_id, log_group, row.token)
            .await;
        if row.replays {
            assert_eq!(
                second.status, 202,
                "case {:?}: replaying the create must answer 202, got {second:?}",
                row.name
            );
            assert_eq!(
                text(&second.body["status"]["statusCode"]),
                LOGGING_STATUS_CREATING,
                "case {:?}: the replay reports the stored status, got {second:?}",
                row.name
            );
        } else {
            assert_eq!(
                second.status, 400,
                "case {:?}: a second configuration must be refused with 400, got {second:?}",
                row.name
            );
            assert_eq!(
                second.error_code(),
                "ValidationException",
                "case {:?}: the refusal must be a ValidationException, got {second:?}",
                row.name
            );
        }

        // Either way the workspace still holds exactly the configuration it was created
        // with: a replay creates nothing new, and a refusal changes nothing.
        let described = amp.describe_logging("us-west-2", &workspace_id).await;
        assert_eq!(
            text(&described.body["loggingConfiguration"]["logGroupArn"]),
            first,
            "case {:?}: the stored configuration must still be the first one, got {described:?}",
            row.name
        );
    }
}

/// The configuration belongs to ONE workspace: each workspace reports its own, and a
/// workspace that has none is a `ResourceNotFoundException` (404) rather than an empty
/// 200 — as is any call naming a workspace that does not exist.
#[tokio::test]
async fn a_configuration_is_reported_only_for_the_workspace_that_holds_it() {
    let amp = Amp::new();
    let mut expected = Vec::new();
    for index in 0..3 {
        let workspace_id = amp.workspace("us-west-2", &format!("ws-{index}")).await;
        // The middle workspace deliberately gets no logging configuration.
        let arn = (index != 1).then(|| {
            log_group_arn(
                "aws",
                "us-west-2",
                &format!("/aws/prometheus/ws-{index}"),
            )
        });
        if let Some(arn) = arn.as_deref() {
            let created = amp
                .create_logging("us-west-2", &workspace_id, arn, None)
                .await;
            assert_eq!(
                created.status, 202,
                "case {index}: the create must succeed, got {created:?}"
            );
        }
        expected.push((workspace_id, arn));
    }

    for (workspace_id, arn) in &expected {
        let described = amp.describe_logging("us-west-2", workspace_id).await;
        match arn {
            Some(arn) => {
                assert_eq!(
                    described.status, 200,
                    "case {workspace_id:?}: a stored configuration must be described, \
                     got {described:?}"
                );
                assert_eq!(
                    text(&described.body["loggingConfiguration"]["logGroupArn"]),
                    *arn,
                    "case {workspace_id:?}: each workspace reports its OWN log group, \
                     got {described:?}"
                );
            }
            None => {
                assert_eq!(
                    described.status, 404,
                    "case {workspace_id:?}: a workspace with no configuration is a 404, \
                     got {described:?}"
                );
                assert_eq!(
                    described.error_code(),
                    "ResourceNotFoundException",
                    "case {workspace_id:?}: the refusal must be a ResourceNotFoundException, \
                     got {described:?}"
                );
            }
        }
    }

    // An id naming no stored workspace: both operations answer the modelled 404, whatever
    // the id's form.
    for unknown in ["ws-00000000-0000-0000-0000-000000000000", "nope", "0"] {
        let described = amp.describe_logging("us-west-2", unknown).await;
        assert_eq!(
            described.status, 404,
            "case {unknown:?}: describing an unknown workspace is a 404, got {described:?}"
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: describing an unknown workspace is a ResourceNotFoundException, \
             got {described:?}"
        );
        let created = amp
            .create_logging(
                "us-west-2",
                unknown,
                &log_group_arn("aws", "us-west-2", "/aws/prometheus/orphan"),
                None,
            )
            .await;
        assert_eq!(
            created.status, 404,
            "case {unknown:?}: creating against an unknown workspace is a 404, got {created:?}"
        );
        assert_eq!(
            created.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: creating against an unknown workspace is a \
             ResourceNotFoundException, got {created:?}"
        );
    }
}

/// The configuration lives on its workspace, so deleting the workspace takes it with it.
#[tokio::test]
async fn deleting_the_workspace_takes_its_configuration_with_it() {
    let amp = Amp::new();
    let workspace_id = amp.workspace("us-west-2", "doomed").await;
    let arn = log_group_arn("aws", "us-west-2", "/aws/prometheus/doomed");
    amp.create_logging("us-west-2", &workspace_id, &arn, None)
        .await;
    // `WorkspaceStatusCode` allows a deletion only from ACTIVE.
    wait_for_due_transition();

    let deleted = amp.delete_workspace("us-west-2", &workspace_id).await;
    assert_eq!(
        deleted.status, 202,
        "case \"delete workspace\": the deletion must be accepted, got {deleted:?}"
    );
    wait_for_due_transition();

    let described = amp.describe_logging("us-west-2", &workspace_id).await;
    assert_eq!(
        described.status, 404,
        "case \"after the workspace is gone\": its configuration must be gone too, \
         got {described:?}"
    );
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "case \"after the workspace is gone\": the refusal must be a \
         ResourceNotFoundException, got {described:?}"
    );
}
