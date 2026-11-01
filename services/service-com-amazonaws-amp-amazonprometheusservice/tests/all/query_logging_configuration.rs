// SPDX-License-Identifier: CC0-1.0

//! A workspace's query logging configuration, end to end over HTTP: create, describe,
//! update and delete, each asserted against what the service PERSISTED rather than
//! against what the request carried.
//!
//! The rule these suites pin: a query logging configuration is a per-workspace resource
//! the service stores, so a create persists the destinations the request carries and
//! every later read answers from that stored record — whatever the destination's log
//! group, whatever its `qspThreshold`, and in whatever region the request was signed
//! for. Each suite spans that class with a table; the recorded case is one row.
//!
//! A create is only accepted for a workspace that will still be there to hold the record
//! it writes, so the last two suites pin the other half of that rule: the workspace's own
//! status decides whether it can take a configuration at all, and a create that arrives
//! while the workspace's removal is owed is refused rather than accepted and then dropped.
//!
//! The HTTP harness ([`crate::workspace_lifecycle::Amp`]) is shared with the workspace
//! suite, so every test in this crate reaches the service the same way: through
//! `MultiServiceRouter::dispatch`, exercising routing, the `restJson1` wire shapes, the
//! modelled response statuses and the modelled error envelopes.

use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    QUERY_LOGGING_CONFIGURATION_STATUS_CODE_VALUES, WORKSPACE_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE, QUERY_LOGGING_NOT_FOUND_MESSAGE,
    QUERY_LOGGING_STATUS_ACTIVE, QUERY_LOGGING_STATUS_CREATING, QUERY_LOGGING_STATUS_DELETING,
    QUERY_LOGGING_STATUS_UPDATING, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING,
    STATUS_UPDATING, WORKSPACE_DELETING_MESSAGE, WORKSPACE_NOT_FOUND_MESSAGE,
};

/// `CreateQueryLoggingConfiguration`: `POST /workspaces/{workspaceId}/logging/query`.
async fn create_configuration(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    body: &serde_json::Value,
) -> Reply {
    amp.call(
        "POST",
        &format!("/workspaces/{workspace_id}/logging/query"),
        region,
        &body.to_string(),
    )
    .await
}

/// `DescribeQueryLoggingConfiguration`: `GET /workspaces/{workspaceId}/logging/query`.
async fn describe_configuration(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/logging/query"),
        region,
        "",
    )
    .await
}

/// `UpdateQueryLoggingConfiguration`: `PUT /workspaces/{workspaceId}/logging/query`.
async fn update_configuration(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    body: &serde_json::Value,
) -> Reply {
    amp.call(
        "PUT",
        &format!("/workspaces/{workspace_id}/logging/query"),
        region,
        &body.to_string(),
    )
    .await
}

/// `DeleteQueryLoggingConfiguration`: `DELETE /workspaces/{workspaceId}/logging/query`,
/// whose `clientToken` is bound to the query string.
async fn delete_configuration(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    client_token: Option<&str>,
) -> Reply {
    let uri = match client_token {
        Some(token) => format!("/workspaces/{workspace_id}/logging/query?clientToken={token}"),
        None => format!("/workspaces/{workspace_id}/logging/query"),
    };
    amp.call("DELETE", &uri, region, "").await
}

/// A CloudWatch Logs log group ARN satisfying `CloudWatchLogDestination$logGroupArn`'s
/// pattern `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`.
fn log_group_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:logs:{region}:{}:log-group:{name}:*",
        engine::emulator_account_id()
    )
}

/// The `destinations` member: `LoggingDestinations` is modelled `len 1..1`, so one
/// `cloudWatchLogs` destination with its `filters`.
fn destinations_json(log_group_arn: &str, qsp_threshold: i64) -> serde_json::Value {
    serde_json::json!([{
        "cloudWatchLogs": { "logGroupArn": log_group_arn },
        "filters": { "qspThreshold": qsp_threshold },
    }])
}

/// The `(logGroupArn, qspThreshold)` pairs a reported `destinations` list carries.
fn destinations_of(value: &serde_json::Value) -> Vec<(String, i64)> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        text(&entry["cloudWatchLogs"]["logGroupArn"]),
                        entry["filters"]["qspThreshold"].as_i64().unwrap_or(-1),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The epoch seconds a `Timestamp` member reports (`restJson1` renders one as a JSON
/// number of seconds).
fn epoch_seconds(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// One row of the create/describe table: what varies is the destination and the region
/// the rule generalises over, never the operation.
struct Case {
    name: &'static str,
    region: &'static str,
    log_group: String,
    qsp_threshold: i64,
    client_token: Option<&'static str>,
}

fn round_trip_cases() -> Vec<Case> {
    vec![
        Case {
            name: "the published lower bound of qspThreshold",
            region: "us-west-2",
            log_group: "query-logs".to_string(),
            // `LoggingFilter$qspThreshold` is modelled `@range(min: 0)`, so 0 is the
            // lowest value the service accepts.
            qsp_threshold: 0,
            client_token: None,
        },
        Case {
            name: "one sample above the lower bound",
            region: "eu-west-1",
            log_group: "obs/query-logs".to_string(),
            qsp_threshold: 1,
            client_token: Some("token-one-sample"),
        },
        Case {
            name: "a mid-range threshold in another region",
            region: "ap-southeast-2",
            log_group: "aws/aps/query_logs.v1#1".to_string(),
            qsp_threshold: 1_234_567,
            client_token: Some("token-mid-range"),
        },
        Case {
            name: "the largest Long the member can hold",
            region: "us-east-1",
            // The pattern's log-group segment is `{1,512}` characters: a name at that
            // published bound is still accepted.
            log_group: "g".repeat(512),
            qsp_threshold: i64::MAX,
            client_token: None,
        },
    ]
}

/// Every status this crate reports for a query logging configuration is a value the
/// modelled `QueryLoggingConfigurationStatusCode` enum declares — the catalog comes from
/// the model, not from this test.
#[test]
fn reported_statuses_are_modelled_query_logging_status_codes() {
    for status in [
        QUERY_LOGGING_STATUS_CREATING,
        QUERY_LOGGING_STATUS_ACTIVE,
        QUERY_LOGGING_STATUS_UPDATING,
        QUERY_LOGGING_STATUS_DELETING,
    ] {
        assert!(
            QUERY_LOGGING_CONFIGURATION_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled QueryLoggingConfigurationStatusCode \
             ({QUERY_LOGGING_CONFIGURATION_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create persists the destinations it was given and a describe reads that same record
/// back: one row per thing the rule generalises over — the `qspThreshold` (each
/// published bound and the type's own), the log group name (short, pathed, punctuated,
/// at the pattern's 512-character bound), the region, and whether the call carried a
/// `clientToken`.
#[tokio::test]
async fn create_persists_the_destinations_and_describe_reads_them_back() {
    for case in round_trip_cases() {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "query-logging").await;
        let arn = log_group_arn(case.region, &case.log_group);
        let mut body = serde_json::json!({
            "destinations": destinations_json(&arn, case.qsp_threshold),
        });
        if let Some(token) = case.client_token {
            body["clientToken"] = serde_json::Value::String(token.to_string());
        }

        let created = create_configuration(&amp, case.region, &workspace_id, &body).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: create must answer the modelled 202, got {created:?}",
            case.name
        );
        assert_eq!(
            text(&created.body["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_CREATING,
            "case {:?}: a fresh configuration must report CREATING, got {created:?}",
            case.name
        );

        let described = describe_configuration(&amp, case.region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: describe must answer 200, got {described:?}",
            case.name
        );
        let metadata = &described.body["queryLoggingConfiguration"];
        assert_eq!(
            text(&metadata["workspace"]),
            workspace_id,
            "case {:?}: the configuration must name its workspace, got {described:?}",
            case.name
        );
        assert_eq!(
            destinations_of(&metadata["destinations"]),
            vec![(arn.clone(), case.qsp_threshold)],
            "case {:?}: describe must report the stored destinations, got {described:?}",
            case.name
        );
        assert_eq!(
            text(&metadata["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_CREATING,
            "case {:?}: describe must report the stored status, got {described:?}",
            case.name
        );
        assert!(
            metadata["status"].get("statusReason").is_none(),
            "case {:?}: nothing failed, so statusReason must be absent, got {described:?}",
            case.name
        );
        let created_at = epoch_seconds(&metadata["createdAt"]);
        let modified_at = epoch_seconds(&metadata["modifiedAt"]);
        assert!(
            created_at > 0.0 && (modified_at - created_at).abs() < f64::EPSILON,
            "case {:?}: a configuration nothing has changed must report \
             modifiedAt == createdAt, got createdAt {created_at} and modifiedAt {modified_at}",
            case.name
        );

        // The clock owes the move to ACTIVE; nothing else changes with it.
        wait_for_due_transition();
        let active = describe_configuration(&amp, case.region, &workspace_id).await;
        assert_eq!(
            text(&active.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_ACTIVE,
            "case {:?}: the configuration must become ACTIVE once due, got {active:?}",
            case.name
        );
        assert_eq!(
            destinations_of(&active.body["queryLoggingConfiguration"]["destinations"]),
            vec![(arn, case.qsp_threshold)],
            "case {:?}: the transition must not change the destinations, got {active:?}",
            case.name
        );
    }
}

/// An update REPLACES the stored destinations — `destinations` is the whole
/// configuration (`len 1..1`) — so every later read reports the new ones, `modifiedAt`
/// moves and `createdAt` does not. One row per kind of change.
#[tokio::test]
async fn update_replaces_the_destinations_that_later_reads_report() {
    struct Change {
        name: &'static str,
        log_group: &'static str,
        qsp_threshold: i64,
    }

    let changes = [
        Change {
            name: "a different threshold on the same log group",
            log_group: "query-logs",
            qsp_threshold: 9_000,
        },
        Change {
            name: "a different log group",
            log_group: "other/query-logs",
            qsp_threshold: 9_000,
        },
        Change {
            name: "back to the published lower bound",
            log_group: "other/query-logs",
            qsp_threshold: 0,
        },
    ];

    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "query-logging").await;
    let first = log_group_arn(region, "query-logs");
    let created = create_configuration(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "destinations": destinations_json(&first, 100) }),
    )
    .await;
    assert_eq!(created.status, 202, "create must succeed, got {created:?}");
    wait_for_due_transition();
    let before = describe_configuration(&amp, region, &workspace_id).await;
    let created_at = epoch_seconds(&before.body["queryLoggingConfiguration"]["createdAt"]);

    for (index, change) in changes.iter().enumerate() {
        let arn = log_group_arn(region, change.log_group);
        let updated = update_configuration(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "destinations": destinations_json(&arn, change.qsp_threshold),
                "clientToken": format!("update-{index}"),
            }),
        )
        .await;
        assert_eq!(
            updated.status, 202,
            "case {:?}: update must answer the modelled 202, got {updated:?}",
            change.name
        );
        assert_eq!(
            text(&updated.body["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_UPDATING,
            "case {:?}: an update in flight must report UPDATING, got {updated:?}",
            change.name
        );

        let described = describe_configuration(&amp, region, &workspace_id).await;
        let metadata = &described.body["queryLoggingConfiguration"];
        assert_eq!(
            destinations_of(&metadata["destinations"]),
            vec![(arn.clone(), change.qsp_threshold)],
            "case {:?}: the stored destinations must be the updated ones, got {described:?}",
            change.name
        );
        assert!(
            (epoch_seconds(&metadata["createdAt"]) - created_at).abs() < f64::EPSILON,
            "case {:?}: an update must not move createdAt, got {described:?}",
            change.name
        );
        assert!(
            epoch_seconds(&metadata["modifiedAt"]) >= created_at,
            "case {:?}: an update must move modifiedAt to the time of the change, got {described:?}",
            change.name
        );

        wait_for_due_transition();
        let settled = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&settled.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_ACTIVE,
            "case {:?}: the configuration must return to ACTIVE once due, got {settled:?}",
            change.name
        );
        assert_eq!(
            destinations_of(&settled.body["queryLoggingConfiguration"]["destinations"]),
            vec![(arn, change.qsp_threshold)],
            "case {:?}: the transition must not change the destinations, got {settled:?}",
            change.name
        );
    }
}

/// A delete starts the configuration's removal: it reports `DELETING` until the clock
/// owes the removal, after which a describe answers `ResourceNotFoundException` and the
/// workspace can take a new configuration. Rows: with and without a `clientToken`, and
/// the token replayed.
#[tokio::test]
async fn delete_removes_the_configuration_and_leaves_the_workspace() {
    for client_token in [None, Some("delete-token")] {
        let region = "eu-central-1";
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "query-logging").await;
        let arn = log_group_arn(region, "query-logs");
        let created = create_configuration(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "destinations": destinations_json(&arn, 7) }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {client_token:?}: create must succeed, got {created:?}"
        );
        wait_for_due_transition();

        let deleted = delete_configuration(&amp, region, &workspace_id, client_token).await;
        assert_eq!(
            deleted.status, 202,
            "case {client_token:?}: delete must answer the modelled 202, got {deleted:?}"
        );
        let deleting = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&deleting.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_DELETING,
            "case {client_token:?}: a pending deletion must report DELETING, got {deleting:?}"
        );

        // Re-sending the very call that started the deletion is that same call.
        if client_token.is_some() {
            let replayed = delete_configuration(&amp, region, &workspace_id, client_token).await;
            assert_eq!(
                replayed.status, 202,
                "case {client_token:?}: replaying the delete token must be accepted, got {replayed:?}"
            );
        }

        wait_for_due_transition();
        let gone = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {client_token:?}: a removed configuration must answer 404, got {gone:?}"
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {client_token:?}: unexpected error code, got {gone:?}"
        );
        assert_eq!(
            text(&gone.body["message"]),
            QUERY_LOGGING_NOT_FOUND_MESSAGE,
            "case {client_token:?}: a missing configuration reports one constant message, got {gone:?}"
        );

        // The workspace itself is untouched, and takes a new configuration.
        let workspace = amp
            .call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await;
        assert_eq!(
            workspace.status, 200,
            "case {client_token:?}: the workspace must outlive its configuration, got {workspace:?}"
        );
        let again = create_configuration(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "destinations": destinations_json(&arn, 11) }),
        )
        .await;
        assert_eq!(
            again.status, 202,
            "case {client_token:?}: a workspace with no configuration takes a new one, got {again:?}"
        );
    }
}

/// `clientToken` carries `@idempotencyToken`: replaying a create answers from the record
/// that token already created and stores nothing further, while a second create under
/// any other token is refused — a workspace holds at most one configuration.
#[tokio::test]
async fn replaying_a_client_token_creates_no_second_configuration() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "query-logging").await;
    let first = log_group_arn(region, "first-logs");
    let second = log_group_arn(region, "second-logs");

    let body = serde_json::json!({
        "destinations": destinations_json(&first, 42),
        "clientToken": "create-token",
    });
    let created = create_configuration(&amp, region, &workspace_id, &body).await;
    assert_eq!(created.status, 202, "create must succeed, got {created:?}");

    // The same token with DIFFERENT destinations is still the same request: it answers
    // from the stored record and changes nothing.
    let replayed = create_configuration(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({
            "destinations": destinations_json(&second, 99),
            "clientToken": "create-token",
        }),
    )
    .await;
    assert_eq!(
        replayed.status, 202,
        "case \"replayed token\": must be accepted, got {replayed:?}"
    );
    let described = describe_configuration(&amp, region, &workspace_id).await;
    assert_eq!(
        destinations_of(&described.body["queryLoggingConfiguration"]["destinations"]),
        vec![(first.clone(), 42)],
        "case \"replayed token\": the stored destinations must be the first ones, got {described:?}"
    );

    // Any other create against a workspace that already holds one is refused, and the
    // refusal stores nothing.
    for (case, token) in [
        ("no token", None),
        ("a different token", Some("other-token")),
    ] {
        let mut body = serde_json::json!({ "destinations": destinations_json(&second, 99) });
        if let Some(token) = token {
            body["clientToken"] = serde_json::Value::String(token.to_string());
        }
        let refused = create_configuration(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a second configuration must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: unexpected error code, got {refused:?}"
        );
        let unchanged = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            destinations_of(&unchanged.body["queryLoggingConfiguration"]["destinations"]),
            vec![(first.clone(), 42)],
            "case {case:?}: the refusal must store nothing, got {unchanged:?}"
        );
    }

    // A later update under its own token does not unseat the create's token: replaying
    // the create is still that same create, answered from the record as it now stands.
    let third = log_group_arn(region, "third-logs");
    let updated = update_configuration(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({
            "destinations": destinations_json(&third, 7),
            "clientToken": "update-token",
        }),
    )
    .await;
    assert_eq!(
        updated.status, 202,
        "case \"update between replays\": must succeed, got {updated:?}"
    );
    let replayed_after_update = create_configuration(&amp, region, &workspace_id, &body).await;
    assert_eq!(
        replayed_after_update.status, 202,
        "case \"create token replayed after an update\": must still be accepted, \
         got {replayed_after_update:?}"
    );
    let after = describe_configuration(&amp, region, &workspace_id).await;
    assert_eq!(
        destinations_of(&after.body["queryLoggingConfiguration"]["destinations"]),
        vec![(third, 7)],
        "case \"create token replayed after an update\": the replay must store nothing, \
         got {after:?}"
    );
}

/// Which earlier call recorded the token the update under test carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    /// No call has used this token yet.
    Nobody,
    /// `CreateQueryLoggingConfiguration` carried it.
    Create,
    /// An earlier `UpdateQueryLoggingConfiguration` carried it.
    Update,
    /// `DeleteQueryLoggingConfiguration` carried it.
    Delete,
}

/// What the update under test must do with the token it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// A request this operation has not carried out: the destinations it names become
    /// the stored ones.
    Applies,
    /// The same update again: 202 from the stored record, which stands unchanged.
    Replays,
    /// Refused with the modelled `ConflictException` (409); nothing changes.
    Conflicts,
}

/// One row of the replay table: who used the token before, what the update carries, and
/// what it must then do.
struct Replay {
    name: &'static str,
    recorded: Recorded,
    /// Spans the `IdempotencyToken` shape (`len 1..64`, pattern `^[!-~]+$`): a plain
    /// name, a UUID, punctuation, the 64-character bound — and no token at all.
    token: Option<&'static str>,
    expected: Expected,
}

/// `UpdateQueryLoggingConfiguration`'s `clientToken` is an `@idempotencyToken`, so a
/// replay is recognised against the token THIS operation's own last call recorded: a
/// token some other operation recorded names a request this one never carried out, so
/// the update applies (or is refused on the state that other call left) rather than
/// being answered from the stored record. One row per operation that can have recorded
/// the token.
#[tokio::test]
async fn an_update_replays_only_the_token_its_own_last_call_recorded() {
    let rows = [
        Replay {
            name: "a token nothing has used",
            recorded: Recorded::Nobody,
            token: Some("fresh-token"),
            expected: Expected::Applies,
        },
        Replay {
            name: "no token at all",
            recorded: Recorded::Nobody,
            token: None,
            expected: Expected::Applies,
        },
        Replay {
            name: "the token the create carried",
            recorded: Recorded::Create,
            token: Some("7f1c9a60-2b4e-4a51-9f2f-8c7d6e5b4a30"),
            expected: Expected::Applies,
        },
        Replay {
            name: "the punctuated token the create carried",
            recorded: Recorded::Create,
            token: Some("!~#$%&*+/=?^_`{|}"),
            expected: Expected::Applies,
        },
        Replay {
            name: "the token this operation's own last call carried",
            recorded: Recorded::Update,
            token: Some("update-token"),
            expected: Expected::Replays,
        },
        Replay {
            name: "a 64-character token this operation's own last call carried",
            recorded: Recorded::Update,
            token: Some("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
            expected: Expected::Replays,
        },
        Replay {
            name: "the token the delete carried",
            recorded: Recorded::Delete,
            token: Some("delete-token"),
            expected: Expected::Conflicts,
        },
    ];

    let region = "us-west-2";
    for row in rows {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "query-logging").await;
        let stored = log_group_arn(region, "stored-logs");
        let target = log_group_arn(region, "target-logs");

        // The create: it carries the row's token only where the row says the CREATE is
        // what recorded it.
        let mut create_body = serde_json::json!({ "destinations": destinations_json(&stored, 100) });
        if row.recorded == Recorded::Create {
            create_body["clientToken"] = serde_json::json!(row.token);
        }
        let created = create_configuration(&amp, region, &workspace_id, &create_body).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: create must succeed, got {created:?}",
            row.name
        );
        wait_for_due_transition();

        // The earlier call that records the token, where the row names one. An earlier
        // update leaves the SAME destinations the create stored, so only the update
        // under test can change them.
        if row.recorded == Recorded::Update {
            let earlier = update_configuration(
                &amp,
                region,
                &workspace_id,
                &serde_json::json!({
                    "destinations": destinations_json(&stored, 100),
                    "clientToken": row.token,
                }),
            )
            .await;
            assert_eq!(
                earlier.status, 202,
                "case {:?}: the earlier update must succeed, got {earlier:?}",
                row.name
            );
        }
        if row.recorded == Recorded::Delete {
            let deleted = delete_configuration(&amp, region, &workspace_id, row.token).await;
            assert_eq!(
                deleted.status, 202,
                "case {:?}: the delete must be accepted, got {deleted:?}",
                row.name
            );
        }

        let before = describe_configuration(&amp, region, &workspace_id).await;
        let mut update_body = serde_json::json!({ "destinations": destinations_json(&target, 7) });
        if let Some(token) = row.token {
            update_body["clientToken"] = serde_json::json!(token);
        }
        let updated = update_configuration(&amp, region, &workspace_id, &update_body).await;

        match row.expected {
            Expected::Conflicts => {
                assert_eq!(
                    updated.status, 409,
                    "case {:?}: the update must answer the modelled 409, got {updated:?}",
                    row.name
                );
                assert_eq!(
                    updated.error_code(),
                    "ConflictException",
                    "case {:?}: unexpected error code, got {updated:?}",
                    row.name
                );
            }
            Expected::Applies | Expected::Replays => {
                assert_eq!(
                    updated.status, 202,
                    "case {:?}: the update must answer the modelled 202, got {updated:?}",
                    row.name
                );
                assert_eq!(
                    text(&updated.body["status"]["statusCode"]),
                    QUERY_LOGGING_STATUS_UPDATING,
                    "case {:?}: the reported status must be the stored one, got {updated:?}",
                    row.name
                );
            }
        }

        let described = describe_configuration(&amp, region, &workspace_id).await;
        let metadata = &described.body["queryLoggingConfiguration"];
        let expected_destinations = match row.expected {
            Expected::Applies => vec![(target.clone(), 7)],
            Expected::Replays | Expected::Conflicts => vec![(stored.clone(), 100)],
        };
        assert_eq!(
            destinations_of(&metadata["destinations"]),
            expected_destinations,
            "case {:?}: expected the stored destinations to be {expected_destinations:?}, \
             got {described:?}",
            row.name
        );
        let expected_status = match row.expected {
            Expected::Applies | Expected::Replays => QUERY_LOGGING_STATUS_UPDATING,
            Expected::Conflicts => QUERY_LOGGING_STATUS_DELETING,
        };
        assert_eq!(
            text(&metadata["status"]["statusCode"]),
            expected_status,
            "case {:?}: expected the stored status to be {expected_status}, got {described:?}",
            row.name
        );
        if row.expected == Expected::Applies {
            assert!(
                epoch_seconds(&metadata["modifiedAt"])
                    >= epoch_seconds(&before.body["queryLoggingConfiguration"]["modifiedAt"]),
                "case {:?}: an applied update must move modifiedAt to the time of the \
                 change, got {described:?}",
                row.name
            );
        } else {
            assert_eq!(
                metadata["modifiedAt"], before.body["queryLoggingConfiguration"]["modifiedAt"],
                "case {:?}: a call that changed nothing must leave modifiedAt standing, \
                 got {described:?}",
                row.name
            );
        }

        // The create's own token still answers as a replay of that create, whatever the
        // update did with a token of its own.
        if row.recorded == Recorded::Create {
            let replayed_create =
                create_configuration(&amp, region, &workspace_id, &create_body).await;
            assert_eq!(
                replayed_create.status, 202,
                "case {:?}: replaying the create must still be accepted, \
                 got {replayed_create:?}",
                row.name
            );
            let after = describe_configuration(&amp, region, &workspace_id).await;
            assert_eq!(
                destinations_of(&after.body["queryLoggingConfiguration"]["destinations"]),
                vec![(target.clone(), 7)],
                "case {:?}: the replayed create must store nothing, got {after:?}",
                row.name
            );
        }
    }
}

/// Every one of the four operations answers `ResourceNotFoundException` for a workspace
/// that holds no such resource — an unknown workspace, and a known workspace with no
/// configuration.
#[tokio::test]
async fn a_missing_resource_is_a_resource_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let known = amp.active_workspace(region, "query-logging").await;
    let unknown = "ws-00000000-0000-4000-8000-000000000000";
    let arn = log_group_arn(region, "query-logs");
    let body = serde_json::json!({ "destinations": destinations_json(&arn, 5) });

    for (case, workspace_id) in [
        ("unknown workspace", unknown),
        ("workspace with no configuration", known.as_str()),
    ] {
        let mut replies = vec![
            (
                "DescribeQueryLoggingConfiguration",
                describe_configuration(&amp, region, workspace_id).await,
            ),
            (
                "UpdateQueryLoggingConfiguration",
                update_configuration(&amp, region, workspace_id, &body).await,
            ),
            (
                "DeleteQueryLoggingConfiguration",
                delete_configuration(&amp, region, workspace_id, None).await,
            ),
        ];
        // A create is refused only for a workspace that does not exist; a workspace
        // without a configuration is exactly what a create is for.
        if workspace_id == unknown {
            replies.push((
                "CreateQueryLoggingConfiguration",
                create_configuration(&amp, region, workspace_id, &body).await,
            ));
        }

        for (operation, reply) in replies {
            assert_eq!(
                reply.status, 404,
                "case {case:?}/{operation}: must answer 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: unexpected error code, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                workspace_id,
                "case {case:?}/{operation}: the refusal must name the resource, got {reply:?}"
            );
            if workspace_id != unknown {
                assert_eq!(
                    text(&reply.body["message"]),
                    QUERY_LOGGING_NOT_FOUND_MESSAGE,
                    "case {case:?}/{operation}: a missing configuration reports one constant \
                     message, got {reply:?}"
                );
            }
        }
    }
}

/// A query logging configuration belongs to its workspace, so deleting the workspace
/// takes the configuration with it.
#[tokio::test]
async fn deleting_the_workspace_drops_its_query_logging_configuration() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "query-logging").await;
    let arn = log_group_arn(region, "query-logs");
    let created = create_configuration(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "destinations": destinations_json(&arn, 3) }),
    )
    .await;
    assert_eq!(created.status, 202, "create must succeed, got {created:?}");
    wait_for_due_transition();

    let deleted = amp.delete(region, &workspace_id, None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the workspace must be accepted, got {deleted:?}"
    );
    wait_for_due_transition();

    let gone = describe_configuration(&amp, region, &workspace_id).await;
    assert_eq!(
        gone.status, 404,
        "case \"workspace deleted\": the configuration must be gone, got {gone:?}"
    );
    assert_eq!(
        gone.error_code(),
        "ResourceNotFoundException",
        "case \"workspace deleted\": unexpected error code, got {gone:?}"
    );
}

/// A request outside a published constraint is refused before any state changes: one row
/// per constraint the model states for `destinations`.
#[tokio::test]
async fn requests_outside_a_published_constraint_are_refused() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "query-logging").await;
    let arn = log_group_arn(region, "query-logs");

    let cases = vec![
        // `LoggingDestinations` is `len 1..1`.
        ("no destination", serde_json::json!({ "destinations": [] })),
        (
            "two destinations",
            serde_json::json!({ "destinations": [
                destinations_json(&arn, 1)[0].clone(),
                destinations_json(&log_group_arn(region, "second"), 2)[0].clone(),
            ]}),
        ),
        // `LoggingFilter$qspThreshold` is `@range(min: 0)`, so one below the bound.
        (
            "a qspThreshold one below the published bound",
            serde_json::json!({ "destinations": destinations_json(&arn, -1) }),
        ),
        // `CloudWatchLogDestination$logGroupArn` carries a pattern: this one names no
        // log group ARN at all.
        (
            "a logGroupArn outside the published pattern",
            serde_json::json!({ "destinations": destinations_json("not-an-arn", 1) }),
        ),
        // The pattern's log-group segment is `{1,512}` characters: one past that bound.
        (
            "a log group name one past the published length bound",
            serde_json::json!({
                "destinations": destinations_json(&log_group_arn(region, &"g".repeat(513)), 1)
            }),
        ),
    ];

    for (case, body) in cases {
        let refused = create_configuration(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: create must be refused with 400, got {refused:?}"
        );
        let after = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a refused create must store nothing, got {after:?}"
        );
    }
}

/// The state the WORKSPACE is in when the create arrives. The catalog is the modelled
/// `WorkspaceStatusCode` — `CREATING`, `ACTIVE`, `UPDATING`, `DELETING`,
/// `CREATION_FAILED` — of which every status this emulator can put a workspace in is a
/// row here, plus the workspace being gone once its owed removal came due.
/// `CREATION_FAILED` has no row because nothing in this emulator fails a create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A create whose owed move to `ACTIVE` has not come due yet.
    Creating,
    /// A settled workspace.
    Active,
    /// A rename whose owed settling has not come due yet.
    Updating,
    /// A delete whose owed removal has not come due yet.
    Deleting,
    /// The same delete, once that removal came due: the workspace is gone.
    Removed,
}

impl Parent {
    /// The status a workspace in this state REPORTS, or `None` once it holds no record
    /// to report at all.
    fn reported(self) -> Option<&'static str> {
        match self {
            Parent::Creating => Some(STATUS_CREATING),
            Parent::Active => Some(STATUS_ACTIVE),
            Parent::Updating => Some(STATUS_UPDATING),
            Parent::Deleting => Some(STATUS_DELETING),
            Parent::Removed => None,
        }
    }

    /// Put a workspace of `region` into this state and answer its id.
    async fn workspace(self, amp: &Amp, region: &str, case: &str) -> String {
        if self == Parent::Creating {
            // The owed move to ACTIVE is not waited for, so the workspace is still
            // CREATING when the create under test arrives.
            let created = amp
                .call(
                    "POST",
                    "/workspaces",
                    region,
                    &serde_json::json!({ "alias": "query-logging" }).to_string(),
                )
                .await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: the workspace create must be accepted, got {created:?}"
            );
            return text(&created.body["workspaceId"]);
        }

        let workspace_id = amp.active_workspace(region, "query-logging").await;
        match self {
            Parent::Active => {}
            Parent::Updating => {
                let renamed = amp
                    .rename(
                        region,
                        &workspace_id,
                        &serde_json::json!({ "alias": "query-logging-renamed" }),
                    )
                    .await;
                assert_eq!(
                    renamed.status, 204,
                    "case {case:?}: the rename must be accepted, got {renamed:?}"
                );
            }
            Parent::Deleting | Parent::Removed => {
                let deleted = amp.delete(region, &workspace_id, None).await;
                assert_eq!(
                    deleted.status, 202,
                    "case {case:?}: deleting an {STATUS_ACTIVE} workspace must be accepted, \
                     got {deleted:?}"
                );
                if self == Parent::Removed {
                    wait_for_due_transition();
                }
            }
            Parent::Creating => unreachable!("handled above"),
        }
        workspace_id
    }
}

/// One row of the parent-status table: the state the workspace is in, the region the
/// calls are signed for, and the destination the create carries — the rule turns on the
/// workspace's state alone, whatever the request asks for.
struct ParentCase {
    name: &'static str,
    region: &'static str,
    parent: Parent,
    log_group: &'static str,
    qsp_threshold: i64,
    client_token: Option<&'static str>,
}

fn parent_cases() -> Vec<ParentCase> {
    vec![
        ParentCase {
            name: "a workspace still being created",
            region: "us-west-2",
            parent: Parent::Creating,
            log_group: "query-logs",
            // `LoggingFilter$qspThreshold` is modelled `@range(min: 0)`: the published
            // lower bound.
            qsp_threshold: 0,
            client_token: None,
        },
        ParentCase {
            name: "a settled workspace",
            region: "eu-west-1",
            parent: Parent::Active,
            log_group: "obs/query-logs",
            qsp_threshold: 5,
            client_token: Some("token-on-an-active-workspace"),
        },
        ParentCase {
            name: "a workspace whose rename has not settled",
            region: "ap-southeast-2",
            parent: Parent::Updating,
            log_group: "aws/aps/query_logs.v1#1",
            qsp_threshold: 1_234_567,
            client_token: None,
        },
        ParentCase {
            name: "a workspace whose deletion has not settled",
            region: "us-east-1",
            parent: Parent::Deleting,
            log_group: "query-logs",
            qsp_threshold: 7,
            client_token: None,
        },
        ParentCase {
            name: "a workspace whose deletion has not settled, under a token",
            region: "us-gov-west-1",
            parent: Parent::Deleting,
            log_group: "aws/aps/deleting-workspace",
            qsp_threshold: i64::MAX,
            client_token: Some("token-on-a-deleting-workspace"),
        },
        ParentCase {
            name: "a workspace whose deletion came due",
            region: "eu-central-1",
            parent: Parent::Removed,
            log_group: "query-logs",
            qsp_threshold: 3,
            client_token: None,
        },
    ]
}

/// A create is accepted only for a workspace that will still be there to hold the record
/// it writes: while the workspace's own removal is owed (`DELETING`, "Workspace is being
/// deleted") the create is refused with the modelled `ValidationException` (400) and
/// writes nothing, because the owed removal drops the configuration along with the
/// workspace — a create accepted in that window could only leave the store holding
/// nothing for it. Every other status the workspace can report is not on its way out, so
/// the same create is accepted and persists; once the removal has come due there is no
/// workspace at all and the refusal is `ResourceNotFoundException` (404).
#[tokio::test]
async fn a_create_is_refused_exactly_while_the_workspace_removal_is_owed() {
    for case in parent_cases() {
        let ParentCase {
            name,
            region,
            parent,
            log_group,
            qsp_threshold,
            client_token,
        } = case;
        let amp = Amp::new();
        let workspace_id = parent.workspace(&amp, region, name).await;

        let reported = amp.describe(region, &workspace_id).await;
        match parent.reported() {
            Some(status) => {
                assert!(
                    WORKSPACE_STATUS_CODE_VALUES.contains(&status),
                    "case {name:?}: {status:?} is not a modelled WorkspaceStatusCode \
                     ({WORKSPACE_STATUS_CODE_VALUES:?})"
                );
                assert_eq!(
                    text(&reported.body["workspace"]["status"]["statusCode"]),
                    status,
                    "case {name:?}: the workspace must be in the status under test, got \
                     {reported:?}"
                );
            }
            None => assert_eq!(
                reported.status, 404,
                "case {name:?}: the owed removal must leave the workspace gone, got \
                 {reported:?}"
            ),
        }

        let arn = log_group_arn(region, log_group);
        let mut body = serde_json::json!({ "destinations": destinations_json(&arn, qsp_threshold) });
        if let Some(token) = client_token {
            body["clientToken"] = serde_json::Value::String(token.to_string());
        }
        let reply = create_configuration(&amp, region, &workspace_id, &body).await;

        match parent {
            Parent::Deleting => {
                assert_eq!(
                    reply.status, 400,
                    "case {name:?}: a create on a workspace being deleted must be refused \
                     with 400, got {reply:?}"
                );
                assert_eq!(
                    reply.error_code(),
                    "ValidationException",
                    "case {name:?}: expected the modelled ValidationException, got {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["message"]),
                    WORKSPACE_DELETING_MESSAGE,
                    "case {name:?}: the refusal must say the workspace's deletion stands in \
                     the way, got {reply:?}"
                );
            }
            Parent::Removed => {
                assert_eq!(
                    reply.status, 404,
                    "case {name:?}: a create for a workspace that is gone must be refused \
                     with 404, got {reply:?}"
                );
                assert_eq!(
                    reply.error_code(),
                    "ResourceNotFoundException",
                    "case {name:?}: expected the modelled ResourceNotFoundException, got \
                     {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["message"]),
                    WORKSPACE_NOT_FOUND_MESSAGE,
                    "case {name:?}: unexpected refusal message, got {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["resourceId"]),
                    workspace_id,
                    "case {name:?}: the refusal must name the resource asked for, got {reply:?}"
                );
            }
            Parent::Creating | Parent::Active | Parent::Updating => {
                assert_eq!(
                    reply.status, 202,
                    "case {name:?}: a workspace that is not on its way out must take a \
                     configuration, got {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["status"]["statusCode"]),
                    QUERY_LOGGING_STATUS_CREATING,
                    "case {name:?}: a fresh configuration must report \
                     {QUERY_LOGGING_STATUS_CREATING}, got {reply:?}"
                );
            }
        }

        // What the STORE holds is what decides it: an accepted create is readable back
        // through the describe, and a refused one left nothing for a reader to find.
        let described = describe_configuration(&amp, region, &workspace_id).await;
        if matches!(parent, Parent::Deleting | Parent::Removed) {
            assert_eq!(
                described.status, 404,
                "case {name:?}: the refused create must leave the store holding nothing \
                 for it, got {described:?}"
            );
        } else {
            assert_eq!(
                described.status, 200,
                "case {name:?}: the accepted create must be readable back, got {described:?}"
            );
            assert_eq!(
                destinations_of(&described.body["queryLoggingConfiguration"]["destinations"]),
                vec![(arn.clone(), qsp_threshold)],
                "case {name:?}: describe must report the stored destinations, got \
                 {described:?}"
            );
        }
    }
}

/// The token the create of the next suite carries, so its refused calls can replay
/// exactly it.
const BOTH_OWED_TOKEN: &str = "create-before-both-removals";

/// When BOTH removals are owed — the configuration's own and its workspace's — the
/// refusal names the WORKSPACE's, because the workspace's removal takes the record with
/// it whatever status that record is in. Neither refusal writes anything: the record on
/// its way out stays the one the create left, and once the removals come due nothing is
/// left behind. Rows: the token the refused call carries (none, the create's own,
/// another).
#[tokio::test]
async fn the_workspace_removal_is_what_a_refusal_names_when_both_are_owed() {
    for token in [None, Some(BOTH_OWED_TOKEN), Some("another-token")] {
        let name = format!("token {token:?}");
        let region = "us-west-2";
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "query-logging").await;
        let first = log_group_arn(region, "first-logs");
        let created = create_configuration(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "destinations": destinations_json(&first, 13),
                "clientToken": BOTH_OWED_TOKEN,
            }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: the first create must be accepted, got {created:?}"
        );
        wait_for_due_transition();
        let settled = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&settled.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_ACTIVE,
            "case {name:?}: the first configuration must settle first, got {settled:?}"
        );

        let other = log_group_arn(region, "other-logs");
        let mut body = serde_json::json!({ "destinations": destinations_json(&other, 17) });
        if let Some(token) = token {
            body["clientToken"] = serde_json::Value::String(token.to_string());
        }

        // Only the CONFIGURATION's removal is owed yet, so that is what the refusal
        // names.
        let deleted = delete_configuration(&amp, region, &workspace_id, None).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: deleting the configuration must be accepted, got {deleted:?}"
        );
        let refused = create_configuration(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a create while the configuration's removal is owed must be \
             refused with 400, got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["message"]),
            QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE,
            "case {name:?}: the refusal must name the configuration's own deletion, got \
             {refused:?}"
        );

        // Now the WORKSPACE's removal is owed too, and it is the one that stands in the
        // way: it drops the record whatever status that record is in.
        let deleted_workspace = amp.delete(region, &workspace_id, None).await;
        assert_eq!(
            deleted_workspace.status, 202,
            "case {name:?}: deleting the workspace must be accepted, got \
             {deleted_workspace:?}"
        );
        let refused_again = create_configuration(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            refused_again.status, 400,
            "case {name:?}: a create on a workspace being deleted must be refused with \
             400, got {refused_again:?}"
        );
        assert_eq!(
            refused_again.error_code(),
            "ValidationException",
            "case {name:?}: expected the modelled ValidationException, got {refused_again:?}"
        );
        assert_eq!(
            text(&refused_again.body["message"]),
            WORKSPACE_DELETING_MESSAGE,
            "case {name:?}: the refusal must name the workspace's deletion, got \
             {refused_again:?}"
        );

        // Neither refusal wrote anything: the record on its way out is still the one the
        // first create left.
        let unchanged = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            destinations_of(&unchanged.body["queryLoggingConfiguration"]["destinations"]),
            vec![(first.clone(), 13)],
            "case {name:?}: the refusals must store nothing, got {unchanged:?}"
        );
        assert_eq!(
            text(&unchanged.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_DELETING,
            "case {name:?}: the record must still be the one on its way out, got \
             {unchanged:?}"
        );

        wait_for_due_transition();
        let gone = describe_configuration(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: once the removals come due nothing is held for the workspace, \
             got {gone:?}"
        );
        let workspace_gone = amp.describe(region, &workspace_id).await;
        assert_eq!(
            workspace_gone.status, 404,
            "case {name:?}: the workspace itself must be gone, got {workspace_gone:?}"
        );
    }
}
