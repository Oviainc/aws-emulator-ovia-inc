// SPDX-License-Identifier: CC0-1.0

//! The workspace's rules-and-alerting logging configuration, end to end over HTTP:
//! `CreateLoggingConfiguration` persists it, `UpdateLoggingConfiguration` points it at
//! another log group, and `DescribeLoggingConfiguration` reads that same record back.
//!
//! The rule under test: a workspace holds at most ONE rules-and-alerting logging
//! configuration, created against `/workspaces/{workspaceId}/logging` and reported from
//! what was PERSISTED — whatever `logGroupArn` the request carried — starting in a
//! transient `CREATING` status the clock owes a move to `ACTIVE`, pointed at another log
//! group by an update that is accepted only while the configuration and its workspace are
//! `ACTIVE` (and that reports the transient `UPDATING` the clock owes a move back from),
//! and gone with the workspace it belongs to. Each test spans a SET of inputs: the rows
//! differ in the log group's partition, region, name form and name length, and in whether
//! the call carries an idempotency token.
//!
//! Like `workspace_lifecycle.rs`, every suite drives the real request path —
//! `MultiServiceRouter::dispatch` over this service mounted on one shared engine — so
//! routing, the `restJson1` wire shapes, the modelled response statuses and the modelled
//! error envelopes are all exercised. The harness is suite-local for the same reason it
//! is there: each suite stands alone on the pieces it needs.

use bytes::Bytes;
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    LOGGING_STATUS_ACTIVE, LOGGING_STATUS_CREATING, LOGGING_STATUS_UPDATING, TRANSITION_DELAY,
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

    /// `UpdateLoggingConfiguration`: `PUT /workspaces/{workspaceId}/logging`.
    async fn update_logging(
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
            "PUT",
            &format!("/workspaces/{workspace_id}/logging"),
            region,
            &body.to_string(),
        )
        .await
    }

    /// A workspace holding an ACTIVE logging configuration on `log_group_arn` — the state
    /// every update starts from, since an update is allowed only from there.
    async fn active_logging(&self, region: &str, alias: &str, log_group_arn: &str) -> String {
        let workspace_id = self.workspace(region, alias).await;
        let created = self
            .create_logging(region, &workspace_id, log_group_arn, None)
            .await;
        assert_eq!(
            created.status, 202,
            "case {alias:?}: the create must succeed, got {created:?}"
        );
        wait_for_due_transition();
        let described = self.describe_logging(region, &workspace_id).await;
        assert_eq!(
            text(&described.body["loggingConfiguration"]["status"]["statusCode"]),
            LOGGING_STATUS_ACTIVE,
            "case {alias:?}: the configuration must settle ACTIVE before an update, \
             got {described:?}"
        );
        workspace_id
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
    for status in [
        LOGGING_STATUS_CREATING,
        LOGGING_STATUS_ACTIVE,
        LOGGING_STATUS_UPDATING,
    ] {
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

/// The timestamp a reader reports, as epoch seconds.
fn epoch_seconds(value: &serde_json::Value, case: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp, got {value:?}"))
}

/// `UpdateLoggingConfiguration` points the PERSISTED configuration at the log group the
/// request names and reports the stored record: one row per thing the rule generalises
/// over — the target log group's partition, region, name form and name length, and the
/// presence of an idempotency token. The update is transient (`UPDATING`) and the clock
/// owes the move back to `ACTIVE`, which leaves the new log group in place.
#[tokio::test]
async fn an_update_points_the_stored_configuration_at_the_named_log_group() {
    for case in cases() {
        let amp = Amp::new();
        let first = log_group_arn("aws", case.region, "/aws/prometheus/before-update");
        let workspace_id = amp.active_logging(case.region, "updated", &first).await;
        let target = log_group_arn(case.partition, case.region, &case.log_group_name);

        let before = amp.describe_logging(case.region, &workspace_id).await;
        let created_at = epoch_seconds(
            &before.body["loggingConfiguration"]["createdAt"],
            case.name,
        );

        let updated = amp
            .update_logging(case.region, &workspace_id, &target, case.client_token)
            .await;
        assert_eq!(
            updated.status, 202,
            "case {:?}: UpdateLoggingConfiguration must answer 202, got {updated:?}",
            case.name
        );
        assert_eq!(
            text(&updated.body["status"]["statusCode"]),
            LOGGING_STATUS_UPDATING,
            "case {:?}: an accepted update reports UPDATING, got {updated:?}",
            case.name
        );
        assert!(
            updated.body["status"]["statusReason"].is_null(),
            "case {:?}: nothing failed, so statusReason must be absent, got {updated:?}",
            case.name
        );

        let described = amp.describe_logging(case.region, &workspace_id).await;
        let metadata = &described.body["loggingConfiguration"];
        assert_eq!(
            text(&metadata["logGroupArn"]),
            target,
            "case {:?}: the stored configuration must publish to the named log group, \
             got {described:?}",
            case.name
        );
        assert_eq!(
            text(&metadata["status"]["statusCode"]),
            LOGGING_STATUS_UPDATING,
            "case {:?}: the describe reports the stored transient status, got {described:?}",
            case.name
        );
        assert_eq!(
            epoch_seconds(&metadata["createdAt"], case.name),
            created_at,
            "case {:?}: an update must not change createdAt, got {described:?}",
            case.name
        );
        assert!(
            epoch_seconds(&metadata["modifiedAt"], case.name) >= created_at,
            "case {:?}: the update changed the record, so modifiedAt must not go \
             backwards, got {described:?}",
            case.name
        );

        wait_for_due_transition();

        let settled = amp.describe_logging(case.region, &workspace_id).await;
        let settled_metadata = &settled.body["loggingConfiguration"];
        assert_eq!(
            text(&settled_metadata["status"]["statusCode"]),
            LOGGING_STATUS_ACTIVE,
            "case {:?}: the configuration must become ACTIVE once due, got {settled:?}",
            case.name
        );
        assert_eq!(
            text(&settled_metadata["logGroupArn"]),
            target,
            "case {:?}: settling must not change logGroupArn, got {settled:?}",
            case.name
        );
        assert_eq!(
            settled_metadata["modifiedAt"], metadata["modifiedAt"],
            "case {:?}: the clock moving the status on is not a change to the \
             configuration, got {settled:?}",
            case.name
        );
    }
}

/// `UpdateLoggingConfiguration` is `@idempotent` and its `clientToken` is an
/// `@idempotencyToken`: re-sending the call that last updated the configuration answers
/// from the stored record rather than applying a second change — whatever log group the
/// replay names. A call carrying a token nothing has used is a new request.
#[tokio::test]
async fn replaying_an_update_answers_from_the_stored_record() {
    /// Each row: the log group the second call names, and whether it is the same call.
    struct Replay {
        name: &'static str,
        token: Option<&'static str>,
        same_log_group: bool,
        replays: bool,
    }

    let region = "us-west-2";
    let update_token = "update-token";
    for row in [
        Replay {
            name: "same token, same log group",
            token: Some(update_token),
            same_log_group: true,
            replays: true,
        },
        Replay {
            name: "same token, another log group",
            token: Some(update_token),
            same_log_group: false,
            replays: true,
        },
        Replay {
            name: "a token nothing has used",
            token: Some("other-token"),
            same_log_group: false,
            replays: false,
        },
        Replay {
            name: "no token at all",
            token: None,
            same_log_group: false,
            replays: false,
        },
    ] {
        let amp = Amp::new();
        let first = log_group_arn("aws", region, "/aws/prometheus/first");
        let workspace_id = amp.active_logging(region, "replay", &first).await;
        let second = log_group_arn("aws", region, "/aws/prometheus/second");
        let third = log_group_arn("aws", region, "/aws/prometheus/third");

        let updated = amp
            .update_logging(region, &workspace_id, &second, Some(update_token))
            .await;
        assert_eq!(
            updated.status, 202,
            "case {:?}: the first update must succeed, got {updated:?}",
            row.name
        );
        // A replay must not restart the span, so the stored record is read before and
        // after; a NEW request is allowed to act only from ACTIVE.
        if !row.replays {
            wait_for_due_transition();
        }
        let before = amp.describe_logging(region, &workspace_id).await;

        let target = if row.same_log_group { &second } else { &third };
        let again = amp
            .update_logging(region, &workspace_id, target, row.token)
            .await;
        assert_eq!(
            again.status, 202,
            "case {:?}: the second update must answer 202, got {again:?}",
            row.name
        );

        let described = amp.describe_logging(region, &workspace_id).await;
        let metadata = &described.body["loggingConfiguration"];
        let expected_log_group = if row.replays { &second } else { target };
        assert_eq!(
            text(&metadata["logGroupArn"]),
            *expected_log_group,
            "case {:?}: a replay must change nothing and a new request must apply, \
             got {described:?}",
            row.name
        );
        assert_eq!(
            text(&metadata["status"]["statusCode"]),
            LOGGING_STATUS_UPDATING,
            "case {:?}: the stored status must be the transient UPDATING, got {described:?}",
            row.name
        );
        if row.replays {
            assert_eq!(
                metadata["modifiedAt"], before.body["loggingConfiguration"]["modifiedAt"],
                "case {:?}: a replay is not a change, so modifiedAt must stand, \
                 got {described:?}",
                row.name
            );
        }
    }
}

/// "Update/Deletion is disallowed until logging configuration is ACTIVE and workspace
/// status is ACTIVE" (`LoggingConfigurationStatusCode`): an update from any other pair of
/// statuses is the modelled `ConflictException` (409) and changes nothing. One row per way
/// the pair can fail that rule.
#[tokio::test]
async fn an_update_is_refused_unless_the_configuration_and_its_workspace_are_active() {
    let region = "eu-west-1";
    for case in [
        "the configuration is still CREATING",
        "the configuration is still UPDATING",
        "the workspace is DELETING",
    ] {
        let amp = Amp::new();
        let first = log_group_arn("aws", region, "/aws/prometheus/gated");
        let target = log_group_arn("aws", region, "/aws/prometheus/elsewhere");
        let workspace_id = match case {
            "the configuration is still CREATING" => {
                // No wait: the create's span has not elapsed, so the configuration is
                // still settling.
                let workspace_id = amp.workspace(region, "gated").await;
                let created = amp
                    .create_logging(region, &workspace_id, &first, None)
                    .await;
                assert_eq!(
                    created.status, 202,
                    "case {case:?}: the create must succeed, got {created:?}"
                );
                workspace_id
            }
            "the configuration is still UPDATING" => {
                let workspace_id = amp.active_logging(region, "gated", &first).await;
                let updated = amp
                    .update_logging(region, &workspace_id, &first, Some("settling"))
                    .await;
                assert_eq!(
                    updated.status, 202,
                    "case {case:?}: the first update must succeed, got {updated:?}"
                );
                workspace_id
            }
            _ => {
                let workspace_id = amp.active_logging(region, "gated", &first).await;
                let deleted = amp.delete_workspace(region, &workspace_id).await;
                assert_eq!(
                    deleted.status, 202,
                    "case {case:?}: the workspace deletion must be accepted, got {deleted:?}"
                );
                workspace_id
            }
        };

        let before = amp.describe_logging(region, &workspace_id).await;
        let refused = amp
            .update_logging(region, &workspace_id, &target, Some("refused-token"))
            .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: the update must be refused with 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: the refusal must be a ConflictException, got {refused:?}"
        );

        let after = amp.describe_logging(region, &workspace_id).await;
        assert_eq!(
            text(&after.body["loggingConfiguration"]["logGroupArn"]),
            text(&before.body["loggingConfiguration"]["logGroupArn"]),
            "case {case:?}: a refused update must change nothing, got {after:?}"
        );
    }
}

/// An update needs a configuration to act on: a workspace that holds none, and an id
/// naming no workspace at all, are both the modelled `ResourceNotFoundException` (404)
/// rather than a configuration created out of an update.
#[tokio::test]
async fn an_update_without_a_stored_configuration_is_not_found() {
    let region = "ap-northeast-1";
    let amp = Amp::new();
    let target = log_group_arn("aws", region, "/aws/prometheus/absent");

    let empty = amp.workspace(region, "no-configuration").await;
    wait_for_due_transition();
    let refused = amp.update_logging(region, &empty, &target, None).await;
    assert_eq!(
        refused.status, 404,
        "case \"workspace holds no configuration\": the update must answer 404, \
         got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ResourceNotFoundException",
        "case \"workspace holds no configuration\": the refusal must be a \
         ResourceNotFoundException, got {refused:?}"
    );
    // The update created nothing: the workspace still holds no configuration.
    let described = amp.describe_logging(region, &empty).await;
    assert_eq!(
        described.status, 404,
        "case \"workspace holds no configuration\": a refused update must persist \
         nothing, got {described:?}"
    );

    for unknown in ["ws-00000000-0000-0000-0000-000000000000", "nope", "0"] {
        let refused = amp.update_logging(region, unknown, &target, None).await;
        assert_eq!(
            refused.status, 404,
            "case {unknown:?}: updating against an unknown workspace is a 404, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: updating against an unknown workspace is a \
             ResourceNotFoundException, got {refused:?}"
        );
    }
}
