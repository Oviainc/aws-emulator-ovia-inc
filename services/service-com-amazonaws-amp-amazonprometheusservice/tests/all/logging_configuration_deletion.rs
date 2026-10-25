// SPDX-License-Identifier: CC0-1.0

//! `DeleteLoggingConfiguration`, end to end over HTTP: the deletion of the
//! rules-and-alerting logging configuration a workspace holds.
//!
//! The rule under test: deleting the configuration is accepted (the modelled 202) only
//! while that configuration and its workspace are both `ACTIVE` — the lifecycle rule the
//! modelled `LoggingConfigurationStatusCode` states, "Update/Deletion is disallowed until
//! logging configuration is ACTIVE and workspace status is ACTIVE", otherwise the modelled
//! `ConflictException` (409) — it moves the STORED record to `DELETING` with the removal
//! owed on the clock, and once that removal has come due the workspace holds no
//! configuration at all, so every reader of one answers the modelled 404. The operation is
//! `@idempotent`, so a delete sent while the removal is still owed is the same request and
//! does not push it out.
//!
//! Each suite spans a SET of inputs: the rows differ in the log group's partition, region,
//! name form and name length, in whether the call carries an idempotency token, and — for
//! the refusals — over the model's OWN status catalogs
//! (`crate::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES` and
//! `WORKSPACE_STATUS_CODE_VALUES`), so a status this file never names by hand is covered
//! too.
//!
//! The shared harness (`Amp`, `Reply`, [`text`], [`wait_for_due_transition`]) drives the
//! real request path — `MultiServiceRouter::dispatch` over this service mounted on one
//! shared engine — and `Amp::store` reaches the SAME store the handlers read, which is how
//! a status the emulator's own calls never reach is put in front of this operation.

use crate::logging_configuration::log_group_arn;
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    LOGGING_CONFIGURATION_STATUS_CODE_VALUES, WORKSPACE_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    LoggingConfiguration, LOGGING_CONFIGURATION_RESOURCE_TYPE, LOGGING_NOT_FOUND_MESSAGE,
    LOGGING_STATUS_ACTIVE, LOGGING_STATUS_DELETING, STATUS_ACTIVE, WORKSPACE_NOT_FOUND_MESSAGE,
    WORKSPACE_RESOURCE_TYPE,
};
use std::time::{Duration, SystemTime};

/// The instant a record forced into a given status was last modified: a fixed past one,
/// so a call that writes the record is visible as a `modifiedAt` that MOVED, and a call
/// that was refused as one that did not.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// `POST /workspaces/{workspaceId}/logging`.
async fn create_logging(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    arn: &str,
    client_token: Option<&str>,
) -> Reply {
    let mut body = serde_json::json!({ "logGroupArn": arn });
    if let Some(token) = client_token {
        body["clientToken"] = serde_json::Value::String(token.to_string());
    }
    amp.call(
        "POST",
        &format!("/workspaces/{workspace_id}/logging"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /workspaces/{workspaceId}/logging`.
async fn describe_logging(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call("GET", &format!("/workspaces/{workspace_id}/logging"), region, "")
        .await
}

/// `DELETE /workspaces/{workspaceId}/logging`, with the optional `clientToken` query
/// member when one is given.
async fn delete_logging(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    client_token: Option<&str>,
) -> Reply {
    let uri = match client_token {
        Some(token) => format!("/workspaces/{workspace_id}/logging?clientToken={token}"),
        None => format!("/workspaces/{workspace_id}/logging"),
    };
    amp.call("DELETE", &uri, region, "").await
}

/// The configuration the store holds for one workspace, or `None` once it holds none.
fn stored(amp: &Amp, workspace_id: &str) -> Option<LoggingConfiguration> {
    amp.store()
        .lock()
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.logging_configuration.clone())
}

/// A workspace whose configuration is `ACTIVE`, created through the service's own
/// operations and settled by the clock.
async fn active_configuration(amp: &Amp, region: &str, alias: &str, arn: &str) -> String {
    let workspace_id = amp.active_workspace(region, alias).await;
    let created = create_logging(amp, region, &workspace_id, arn, None).await;
    assert_eq!(
        created.status, 202,
        "case {alias:?}: create the configuration to delete, got {created:?}"
    );
    wait_for_due_transition();
    workspace_id
}

/// Put the workspace and its configuration in the statuses named, with nothing owed by
/// the clock. Written straight into the store the handlers read, because `CREATION_FAILED`
/// and the rest are statuses this emulator's own calls never reach.
fn force_statuses(amp: &Amp, workspace_id: &str, workspace_status: &str, logging_status: &str) {
    let store = amp.store();
    let mut guard = store.lock();
    let workspace = guard
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the store holds workspace {workspace_id:?}"));
    workspace.status_code = workspace_status.to_string();
    workspace.transition_due = None;
    let logging = workspace
        .logging_configuration
        .as_mut()
        .unwrap_or_else(|| panic!("workspace {workspace_id:?} holds a configuration"));
    logging.status_code = logging_status.to_string();
    logging.status_reason = None;
    logging.transition_due = None;
    logging.modified_at = seeded_at();
}

/// One row of the input class this deletion spans: the log group the configuration
/// publishes to, and whether the delete carries an idempotency token.
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
            name: "shortest log group name, no token",
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
            client_token: Some("delete-logging-slashed"),
        },
        Case {
            name: "every admitted punctuation",
            region: "eu-central-1",
            partition: "aws",
            log_group_name: "metrics.rules-log_group#1/nested".to_string(),
            client_token: Some("delete-logging-punctuation"),
        },
        Case {
            name: "longest log group name",
            region: "ap-southeast-2",
            partition: "aws",
            log_group_name: "l".repeat(512),
            client_token: None,
        },
        Case {
            name: "another partition, token at the top of its modelled length",
            region: "us-gov-west-1",
            partition: "aws-us-gov",
            log_group_name: "gov/rules".to_string(),
            // `IdempotencyToken` is modelled `len 1..64`.
            client_token: Some(
                "tttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttt",
            ),
        },
    ]
}

/// Deleting an `ACTIVE` configuration is accepted, moves the stored record to `DELETING`
/// with the removal owed on the clock, and the clock then drops it — after which the
/// workspace holds none at all and a reader answers the modelled 404, while the workspace
/// itself is untouched. One row per thing the rule generalises over.
#[tokio::test]
async fn deleting_a_configuration_moves_it_to_deleting_then_the_clock_removes_it() {
    for case in cases() {
        let Case {
            name,
            region,
            partition,
            log_group_name,
            client_token,
        } = case;
        let arn = log_group_arn(partition, region, &log_group_name);
        let amp = Amp::new();
        let workspace_id = active_configuration(&amp, region, "to-delete", &arn).await;
        force_statuses(&amp, &workspace_id, STATUS_ACTIVE, LOGGING_STATUS_ACTIVE);

        let reply = delete_logging(&amp, region, &workspace_id, client_token).await;
        assert_eq!(
            reply.status, 202,
            "case {name:?}: DeleteLoggingConfiguration is modelled 202, got {reply:?}"
        );

        let pending = stored(&amp, &workspace_id).unwrap_or_else(|| {
            panic!("case {name:?}: the record must still be held while the removal is owed")
        });
        assert_eq!(
            pending.status_code, LOGGING_STATUS_DELETING,
            "case {name:?}: expected the status the call wrote, got {:?}",
            pending.status_code
        );
        assert!(
            LOGGING_CONFIGURATION_STATUS_CODE_VALUES.contains(&pending.status_code.as_str()),
            "case {name:?}: the reported status must be a modelled \
             LoggingConfigurationStatusCode ({LOGGING_CONFIGURATION_STATUS_CODE_VALUES:?}), \
             got {:?}",
            pending.status_code
        );
        assert!(
            pending.transition_due.is_some(),
            "case {name:?}: the delete must leave a removal owed, got {:?}",
            pending.transition_due
        );
        assert!(
            pending.modified_at > seeded_at(),
            "case {name:?}: the delete must move modifiedAt past {:?}, got {:?}",
            seeded_at(),
            pending.modified_at
        );
        assert_eq!(
            pending.log_group_arn, arn,
            "case {name:?}: a pending deletion still publishes where it did, got {:?}",
            pending.log_group_arn
        );

        // While the removal is owed the configuration is still reported, in the transient
        // status the call wrote.
        let described = describe_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: a configuration whose removal is owed is still described, \
             got {described:?}"
        );
        assert_eq!(
            text(&described.body["loggingConfiguration"]["status"]["statusCode"]),
            LOGGING_STATUS_DELETING,
            "case {name:?}: expected the status the delete wrote, got {described:?}"
        );

        wait_for_due_transition();

        // The removal the clock owed has come due, so the configuration is gone: the
        // reader answers the modelled 404 and the workspace holds no record at all.
        let gone = describe_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: a removed configuration must answer 404, got {gone:?}"
        );
        assert_eq!(
            stored(&amp, &workspace_id).map(|record| record.status_code),
            None,
            "case {name:?}: the workspace must hold no configuration once the removal is \
             due, got {:?}",
            stored(&amp, &workspace_id)
        );
        assert_eq!(
            text(&gone.body["message"]),
            LOGGING_NOT_FOUND_MESSAGE,
            "case {name:?}: expected the configuration not-found sentence, got {gone:?}"
        );

        // Deleting the configuration is not deleting the workspace.
        let workspace = amp
            .call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await;
        assert_eq!(
            workspace.status, 200,
            "case {name:?}: the workspace itself must survive, got {workspace:?}"
        );
        assert_eq!(
            text(&workspace.body["workspace"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {name:?}: the workspace must keep its status, got {workspace:?}"
        );

        // The removal has come due, so the very same call is now a not-found rather than
        // a second deletion.
        let repeated = delete_logging(&amp, region, &workspace_id, client_token).await;
        assert_eq!(
            repeated.status, 404,
            "case {name:?}: deleting a configuration the store no longer holds must \
             answer 404, got {repeated:?}"
        );
        assert_eq!(
            text(&repeated.body["resourceType"]),
            LOGGING_CONFIGURATION_RESOURCE_TYPE,
            "case {name:?}: the refusal must name the resource type, got {repeated:?}"
        );
    }
}

/// The operation is `@idempotent`: a delete sent while the removal of that same record is
/// still owed is the SAME request, so it is accepted again — with the token the first call
/// carried, with none, or with another — and does NOT push the owed removal out.
#[tokio::test]
async fn a_delete_while_the_removal_is_owed_is_the_same_request() {
    let region = "us-west-2";
    let token = "delete-logging-replay";
    let arn = log_group_arn("aws", region, "/aws/prometheus/replay");
    let amp = Amp::new();
    let workspace_id = active_configuration(&amp, region, "replayed-delete", &arn).await;

    let first = delete_logging(&amp, region, &workspace_id, Some(token)).await;
    assert_eq!(
        first.status, 202,
        "the first delete must be accepted, got {first:?}"
    );
    let owed = stored(&amp, &workspace_id).and_then(|record| record.transition_due);
    assert!(
        owed.is_some(),
        "the delete must leave a removal owed, got {owed:?}"
    );
    let modified = stored(&amp, &workspace_id).map(|record| record.modified_at);

    for repeat_token in [Some(token), None, Some("delete-logging-other")] {
        let repeat = delete_logging(&amp, region, &workspace_id, repeat_token).await;
        assert_eq!(
            repeat.status, 202,
            "case {repeat_token:?}: a deletion already under way must still be accepted, \
             got {repeat:?}"
        );
        let record = stored(&amp, &workspace_id).unwrap_or_else(|| {
            panic!("case {repeat_token:?}: the record is held until the removal is due")
        });
        assert_eq!(
            record.transition_due, owed,
            "case {repeat_token:?}: the repeat must not move the instant the removal is \
             owed, expected {owed:?}, got {:?}",
            record.transition_due
        );
        assert_eq!(
            Some(record.modified_at),
            modified,
            "case {repeat_token:?}: the repeat changes nothing, so modifiedAt stays, \
             expected {modified:?}, got {:?}",
            record.modified_at
        );
        assert_eq!(
            record.status_code, LOGGING_STATUS_DELETING,
            "case {repeat_token:?}: the record stays in the status the first call wrote, \
             got {:?}",
            record.status_code
        );
    }
}

/// A deletion is refused with the modelled `ConflictException` (409) unless the
/// configuration AND its workspace are both `ACTIVE`, and the refused call leaves the
/// stored record exactly where it was. The rows come from the model's own status catalogs,
/// so a status this test never names by hand is covered too.
#[tokio::test]
async fn a_delete_is_refused_unless_the_configuration_and_its_workspace_are_active() {
    let region = "eu-west-1";
    let arn = log_group_arn("aws", region, "/aws/prometheus/conflict");

    // Every pair of statuses the two catalogs admit, minus the one pair the rule allows
    // and the pair a deletion already under way is (which is the idempotent replay, not a
    // refusal).
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for workspace_status in WORKSPACE_STATUS_CODE_VALUES {
        for logging_status in LOGGING_CONFIGURATION_STATUS_CODE_VALUES {
            if *workspace_status == STATUS_ACTIVE && *logging_status == LOGGING_STATUS_ACTIVE {
                continue;
            }
            if *logging_status == LOGGING_STATUS_DELETING {
                continue;
            }
            pairs.push((workspace_status, logging_status));
        }
    }
    assert!(
        !pairs.is_empty(),
        "the model's status catalogs must yield refusable pairs, got {pairs:?}"
    );

    for (workspace_status, logging_status) in pairs {
        let case = format!("workspace {workspace_status} / configuration {logging_status}");
        let amp = Amp::new();
        let workspace_id = active_configuration(&amp, region, "conflicted", &arn).await;
        force_statuses(&amp, &workspace_id, workspace_status, logging_status);

        let reply = delete_logging(&amp, region, &workspace_id, Some("delete-conflict")).await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: the delete must be refused with the modelled 409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            workspace_id,
            "case {case:?}: the refusal must name the workspace asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            LOGGING_CONFIGURATION_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
        let message = text(&reply.body["message"]);
        assert!(
            message.contains(workspace_status) && message.contains(logging_status),
            "case {case:?}: the refusal must say which statuses stand in the way, got \
             {message:?}"
        );

        let record = stored(&amp, &workspace_id)
            .unwrap_or_else(|| panic!("case {case:?}: a refused delete removes nothing"));
        assert_eq!(
            record.status_code, logging_status,
            "case {case:?}: a refused delete leaves the status alone, got {:?}",
            record.status_code
        );
        assert_eq!(
            record.transition_due, None,
            "case {case:?}: a refused delete owes nothing, got {:?}",
            record.transition_due
        );
        assert_eq!(
            record.modified_at,
            seeded_at(),
            "case {case:?}: a refused delete leaves modifiedAt alone, expected {:?}, got \
             {:?}",
            seeded_at(),
            record.modified_at
        );
    }
}

/// A workspace the store does not hold is the modelled `ResourceNotFoundException` (404)
/// naming the WORKSPACE, whatever the id looks like; a workspace that holds no
/// configuration is that same status naming the CONFIGURATION, whatever status the
/// workspace itself is in.
#[tokio::test]
async fn an_absent_workspace_or_configuration_is_not_found() {
    let region = "us-east-2";
    let amp = Amp::new();
    let held = active_configuration(
        &amp,
        region,
        "bystander",
        &log_group_arn("aws", region, "/aws/prometheus/bystander"),
    )
    .await;

    let absent = [
        ("a minted-looking id nothing holds", "ws-99999999-9999-9999-9999-999999999999".to_string()),
        ("a one-character id", "z".to_string()),
        // `WorkspaceId` is modelled `len 1..64`.
        ("an id of the maximum modelled length", "w".repeat(64)),
        ("an id that differs from a held one only in case", held.to_uppercase()),
    ];

    for (case, workspace_id) in absent {
        let reply = delete_logging(&amp, region, &workspace_id, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent workspace must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            WORKSPACE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the workspace not-found sentence, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            workspace_id,
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            WORKSPACE_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
    }

    assert_eq!(
        stored(&amp, &held).map(|record| record.status_code),
        Some(LOGGING_STATUS_ACTIVE.to_string()),
        "a delete that named another workspace must leave this configuration where it \
         was, got {:?}",
        stored(&amp, &held)
    );

    for workspace_status in WORKSPACE_STATUS_CODE_VALUES {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "unlogged").await;
        {
            let store = amp.store();
            let mut guard = store.lock();
            let workspace = guard
                .workspaces
                .get_mut(&workspace_id)
                .expect("the store holds the workspace just created");
            workspace.status_code = (*workspace_status).to_string();
            workspace.transition_due = None;
        }

        let reply = delete_logging(&amp, region, &workspace_id, Some("delete-unlogged")).await;
        assert_eq!(
            reply.status, 404,
            "case {workspace_status:?}: a workspace holding no configuration must answer \
             404, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            LOGGING_NOT_FOUND_MESSAGE,
            "case {workspace_status:?}: expected the configuration not-found sentence, \
             got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            workspace_id,
            "case {workspace_status:?}: the refusal must name the workspace asked for, \
             got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            LOGGING_CONFIGURATION_RESOURCE_TYPE,
            "case {workspace_status:?}: the refusal must name the resource type, got \
             {reply:?}"
        );
    }
}

/// Deleting one workspace's configuration is a change to that record only: every other
/// workspace keeps its configuration, its `modifiedAt` and the fact that nothing is owed
/// on it.
#[tokio::test]
async fn a_delete_touches_only_the_configuration_it_names() {
    let region = "us-west-2";
    let amp = Amp::new();
    let mut workspace_ids = Vec::new();
    for alias in ["fleet-a", "fleet-b", "fleet-c"] {
        let workspace_id = active_configuration(
            &amp,
            region,
            alias,
            &log_group_arn("aws", region, &format!("/aws/prometheus/{alias}")),
        )
        .await;
        force_statuses(&amp, &workspace_id, STATUS_ACTIVE, LOGGING_STATUS_ACTIVE);
        workspace_ids.push(workspace_id);
    }

    let deleted = &workspace_ids[1];
    let reply = delete_logging(&amp, region, deleted, None).await;
    assert_eq!(
        reply.status, 202,
        "case {deleted:?}: the delete must be accepted, got {reply:?}"
    );

    for workspace_id in &workspace_ids {
        let record = stored(&amp, workspace_id).unwrap_or_else(|| {
            panic!("case {workspace_id:?}: every configuration is still held")
        });
        if workspace_id == deleted {
            assert_eq!(
                record.status_code, LOGGING_STATUS_DELETING,
                "case {workspace_id:?}: expected the status the call wrote, got {:?}",
                record.status_code
            );
            continue;
        }
        assert_eq!(
            record.status_code, LOGGING_STATUS_ACTIVE,
            "case {workspace_id:?}: a configuration nobody deleted keeps its status, got \
             {:?}",
            record.status_code
        );
        assert_eq!(
            record.modified_at,
            seeded_at(),
            "case {workspace_id:?}: a configuration nobody deleted keeps its modifiedAt, \
             got {:?}",
            record.modified_at
        );
        assert_eq!(
            record.transition_due, None,
            "case {workspace_id:?}: a configuration nobody deleted owes nothing, got {:?}",
            record.transition_due
        );
    }

    wait_for_due_transition();

    for workspace_id in &workspace_ids {
        // Reading over HTTP is what makes the store apply what the clock already owed.
        let described = describe_logging(&amp, region, workspace_id).await;
        let expected_status = if workspace_id == deleted { 404 } else { 200 };
        assert_eq!(
            described.status, expected_status,
            "case {workspace_id:?}: expected {expected_status} once the owed removal is \
             due, got {described:?}"
        );
        let held = stored(&amp, workspace_id).is_some();
        assert_eq!(
            held,
            workspace_id != deleted,
            "case {workspace_id:?}: only the configuration the delete named is removed, \
             got held={held}"
        );
    }
}
