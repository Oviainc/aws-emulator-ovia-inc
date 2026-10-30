// SPDX-License-Identifier: CC0-1.0

//! Creates sent while the record a replay would answer from is on its way OUT, end to
//! end over HTTP.
//!
//! The rule these suites pin: an idempotent create answers as a replay only from a
//! stored record that is still the one its original call left, so a record whose
//! removal the clock already owes (the transient `DELETING` every deletion in this
//! service writes) is never what a create is answered from — a create in that window
//! either refuses, where the resource is single-valued and the one it would create is
//! being deleted, or creates a new resource, where the resource is keyed or minted.
//! Nothing accepts the call and then leaves the store holding nothing for it.
//!
//! Each suite spans that class with a table: the rows differ in the idempotency token
//! the second call carries (the first call's, another, none), in whether the second call
//! asks for the same thing as the first, and across the resource kinds the rule covers —
//! the two single-valued workspace children (`CreateLoggingConfiguration`,
//! `CreateQueryLoggingConfiguration`) and the keyed or minted resources
//! (`CreateWorkspace`, `CreateRuleGroupsNamespace`, `CreateAnomalyDetector`,
//! `CreateScraper`).
//!
//! The shared harness ([`crate::workspace_lifecycle::Amp`]) drives the real request path
//! — `MultiServiceRouter::dispatch` over this service mounted on one shared engine — and
//! `Amp::store` reaches the SAME store the handlers read, which is how a test sees what a
//! refused or accepted create actually persisted.

use crate::anomaly_detector_lifecycle::{create_detector, describe_detector, minimal_configuration};
use crate::logging_configuration::log_group_arn;
use crate::logging_configuration_deletion::{create_logging, delete_logging, describe_logging};
use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::scraper_update::scrape_configuration_json;
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    LoggingConfiguration, LOGGING_CONFIGURATION_DELETING_MESSAGE, LOGGING_STATUS_ACTIVE,
    LOGGING_STATUS_CREATING, LOGGING_STATUS_DELETING, NAMESPACE_STATUS_DELETING,
    QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE, QUERY_LOGGING_STATUS_CREATING,
    QUERY_LOGGING_STATUS_DELETING, STATUS_ACTIVE, STATUS_DELETING,
};

/// The rules-and-alerting configuration the store holds for one workspace of `region`,
/// or `None` once it holds none.
fn stored_logging(amp: &Amp, region: &str, workspace_id: &str) -> Option<LoggingConfiguration> {
    amp.store()
        .lock()
        .region(region)
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.logging_configuration.clone())
}

/// `POST /workspaces/{workspaceId}/logging/query` — CreateQueryLoggingConfiguration.
///
/// `pub(crate)` with [`describe_query_logging`] and [`query_body`], so the suite that
/// pins the same rule for the PARENT's pending removal
/// ([`crate::create_on_a_workspace_whose_removal_is_owed`]) drives this operation through
/// the one copy of these helpers.
pub(crate) async fn create_query_logging(
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

/// `GET /workspaces/{workspaceId}/logging/query` — DescribeQueryLoggingConfiguration.
pub(crate) async fn describe_query_logging(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/logging/query"),
        region,
        "",
    )
    .await
}

/// `DELETE /workspaces/{workspaceId}/logging/query` — DeleteQueryLoggingConfiguration.
async fn delete_query_logging(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call(
        "DELETE",
        &format!("/workspaces/{workspace_id}/logging/query"),
        region,
        "",
    )
    .await
}

/// The `destinations` member of a query logging configuration: `LoggingDestinations` is
/// modelled `len 1..1`, so one `cloudWatchLogs` destination with its `filters`.
pub(crate) fn query_body(
    arn: &str,
    qsp_threshold: i64,
    client_token: Option<&str>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "destinations": [{
            "cloudWatchLogs": { "logGroupArn": arn },
            "filters": { "qspThreshold": qsp_threshold },
        }],
    });
    if let Some(token) = client_token {
        body["clientToken"] = serde_json::Value::String(token.to_string());
    }
    body
}

/// One row of the second call: the token it carries and whether it asks for the same
/// thing the first call did.
struct Second {
    name: &'static str,
    token: Option<&'static str>,
    same_target: bool,
}

/// The token the first create of each suite carries, so a row can replay exactly it.
const FIRST_TOKEN: &str = "first-create-token";

/// The rows every suite below walks: the second call carries the first call's token,
/// another token or none, and asks for the same thing or for something else.
fn second_calls() -> Vec<Second> {
    vec![
        Second {
            name: "the first call's token, the same target",
            token: Some(FIRST_TOKEN),
            same_target: true,
        },
        Second {
            name: "the first call's token, another target",
            token: Some(FIRST_TOKEN),
            same_target: false,
        },
        Second {
            name: "another token, the same target",
            // `IdempotencyToken` is modelled `len 1..64`: a token at that maximum is
            // carried the same way as a short one.
            token: Some("ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss"),
            same_target: true,
        },
        Second {
            name: "no token at all, the same target",
            token: None,
            same_target: true,
        },
    ]
}

/// A `CreateLoggingConfiguration` sent while the removal of the stored configuration is
/// owed is REFUSED — it is not a replay of the create that made that record, because the
/// clock is about to drop it — and the refused call leaves the pending removal exactly
/// where it was. Once the removal has come due, a create is accepted again and persists:
/// a describe reads that new record back.
#[tokio::test]
async fn a_logging_configuration_create_while_its_removal_is_owed_is_refused() {
    let region = "us-west-2";
    for second in second_calls() {
        let Second {
            name,
            token,
            same_target,
        } = second;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "logging-deleting").await;
        let first = log_group_arn("aws", region, "/aws/prometheus/first");
        let other = log_group_arn("aws", region, "/aws/prometheus/other");

        let created = create_logging(&amp, region, &workspace_id, &first, Some(FIRST_TOKEN)).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: the first create must be accepted, got {created:?}"
        );
        wait_for_due_transition();

        let deleted = delete_logging(&amp, region, &workspace_id, None).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: deleting an {LOGGING_STATUS_ACTIVE} configuration must be \
             accepted, got {deleted:?}"
        );
        let pending = stored_logging(&amp, region, &workspace_id).unwrap_or_else(|| {
            panic!("case {name:?}: the record is held while its removal is owed")
        });
        assert_eq!(
            pending.status_code, LOGGING_STATUS_DELETING,
            "case {name:?}: the delete must leave the record {LOGGING_STATUS_DELETING}, got {:?}",
            pending.status_code
        );

        let target = if same_target { &first } else { &other };
        let second_reply = create_logging(&amp, region, &workspace_id, target, token).await;
        assert_eq!(
            second_reply.status, 400,
            "case {name:?}: a create while the removal is owed must be refused with 400, \
             got {second_reply:?}"
        );
        assert_eq!(
            second_reply.error_code(),
            "ValidationException",
            "case {name:?}: expected the modelled ValidationException, got {second_reply:?}"
        );
        assert_eq!(
            text(&second_reply.body["message"]),
            LOGGING_CONFIGURATION_DELETING_MESSAGE,
            "case {name:?}: the refusal must say the deletion stands in the way, got \
             {second_reply:?}"
        );

        let after = stored_logging(&amp, region, &workspace_id)
            .unwrap_or_else(|| panic!("case {name:?}: a refused create removes nothing"));
        assert_eq!(
            after.status_code, LOGGING_STATUS_DELETING,
            "case {name:?}: the refused create leaves the status alone, got {:?}",
            after.status_code
        );
        assert_eq!(
            after.log_group_arn, first,
            "case {name:?}: the refused create changes nothing it names, got {:?}",
            after.log_group_arn
        );
        assert_eq!(
            after.transition_due, pending.transition_due,
            "case {name:?}: the refused create must not move the owed removal, expected \
             {:?}, got {:?}",
            pending.transition_due, after.transition_due
        );
        assert_eq!(
            after.modified_at, pending.modified_at,
            "case {name:?}: the refused create changes nothing, so modifiedAt stays, \
             expected {:?}, got {:?}",
            pending.modified_at, after.modified_at
        );

        wait_for_due_transition();
        let gone = describe_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: the owed removal must leave the workspace holding none, got \
             {gone:?}"
        );

        // With the removal done, the create the caller wanted is accepted and PERSISTS:
        // the describe reads that new record back.
        let again = create_logging(&amp, region, &workspace_id, target, token).await;
        assert_eq!(
            again.status, 202,
            "case {name:?}: a create once the removal is done must be accepted, got {again:?}"
        );
        assert_eq!(
            text(&again.body["status"]["statusCode"]),
            LOGGING_STATUS_CREATING,
            "case {name:?}: a new configuration starts {LOGGING_STATUS_CREATING}, got {again:?}"
        );
        let described = describe_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the accepted create must be readable back, got {described:?}"
        );
        assert_eq!(
            text(&described.body["loggingConfiguration"]["logGroupArn"]),
            *target,
            "case {name:?}: the stored configuration must publish where the accepted \
             create said, got {described:?}"
        );
    }
}

/// The same rule on the workspace's other single-valued logging child: a
/// `CreateQueryLoggingConfiguration` sent while the stored configuration's removal is
/// owed is refused, and once that removal is done a create is accepted and persists.
#[tokio::test]
async fn a_query_logging_configuration_create_while_its_removal_is_owed_is_refused() {
    let region = "eu-central-1";
    for second in second_calls() {
        let Second {
            name,
            token,
            same_target,
        } = second;
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "query-deleting").await;
        let first = log_group_arn("aws", region, "/aws/prometheus/query-first");
        let other = log_group_arn("aws", region, "/aws/prometheus/query-other");

        let created = create_query_logging(
            &amp,
            region,
            &workspace_id,
            &query_body(&first, 7, Some(FIRST_TOKEN)),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: the first create must be accepted, got {created:?}"
        );
        wait_for_due_transition();

        let deleted = delete_query_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: the delete must be accepted, got {deleted:?}"
        );
        let deleting = describe_query_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&deleting.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_DELETING,
            "case {name:?}: the delete must leave the record {QUERY_LOGGING_STATUS_DELETING}, \
             got {deleting:?}"
        );

        let (target, threshold) = if same_target {
            (&first, 7)
        } else {
            (&other, 99)
        };
        let second_reply =
            create_query_logging(&amp, region, &workspace_id, &query_body(target, threshold, token))
                .await;
        assert_eq!(
            second_reply.status, 400,
            "case {name:?}: a create while the removal is owed must be refused with 400, \
             got {second_reply:?}"
        );
        assert_eq!(
            second_reply.error_code(),
            "ValidationException",
            "case {name:?}: expected the modelled ValidationException, got {second_reply:?}"
        );
        assert_eq!(
            text(&second_reply.body["message"]),
            QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE,
            "case {name:?}: the refusal must say the deletion stands in the way, got \
             {second_reply:?}"
        );
        let untouched = describe_query_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            text(&untouched.body["queryLoggingConfiguration"]["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_DELETING,
            "case {name:?}: the refused create leaves the pending deletion alone, got \
             {untouched:?}"
        );

        wait_for_due_transition();
        let gone = describe_query_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {name:?}: the owed removal must leave the workspace holding none, got \
             {gone:?}"
        );

        let again =
            create_query_logging(&amp, region, &workspace_id, &query_body(target, threshold, token))
                .await;
        assert_eq!(
            again.status, 202,
            "case {name:?}: a create once the removal is done must be accepted, got {again:?}"
        );
        assert_eq!(
            text(&again.body["status"]["statusCode"]),
            QUERY_LOGGING_STATUS_CREATING,
            "case {name:?}: a new configuration starts {QUERY_LOGGING_STATUS_CREATING}, got \
             {again:?}"
        );
        let described = describe_query_logging(&amp, region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the accepted create must be readable back, got {described:?}"
        );
        assert_eq!(
            text(
                &described.body["queryLoggingConfiguration"]["destinations"][0]["cloudWatchLogs"]
                    ["logGroupArn"]
            ),
            *target,
            "case {name:?}: the stored configuration must carry the destination the \
             accepted create named, got {described:?}"
        );
    }
}

/// `CreateWorkspace` is keyed by nothing the caller names: the record a replayed token
/// would answer from is being deleted, so the create makes a NEW workspace — a different
/// `workspaceId` — rather than answering with one that is about to be gone.
#[tokio::test]
async fn a_workspace_create_replaying_a_token_of_a_deleting_workspace_makes_a_new_one() {
    let region = "us-east-1";
    for token in [Some(FIRST_TOKEN), Some("another-token"), None] {
        let case = format!("second create token {token:?}");
        let amp = Amp::new();
        let mut body = serde_json::json!({ "alias": "deleting-workspace" });
        body["clientToken"] = serde_json::Value::String(FIRST_TOKEN.to_string());
        let created = amp.call("POST", "/workspaces", region, &body.to_string()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the first create must be accepted, got {created:?}"
        );
        let first_id = text(&created.body["workspaceId"]);
        wait_for_due_transition();

        let deleted = amp.delete(region, &first_id, None).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: deleting an {STATUS_ACTIVE} workspace must be accepted, got \
             {deleted:?}"
        );
        assert_eq!(
            amp.store()
                .lock()
                .region(region)
                .workspaces
                .get(&first_id)
                .map(|workspace| workspace.status_code.clone()),
            Some(STATUS_DELETING.to_string()),
            "case {case:?}: the delete must leave the record {STATUS_DELETING}"
        );

        let mut second_body = serde_json::json!({ "alias": "after-deleting" });
        if let Some(token) = token {
            second_body["clientToken"] = serde_json::Value::String(token.to_string());
        }
        let second = amp
            .call("POST", "/workspaces", region, &second_body.to_string())
            .await;
        assert_eq!(
            second.status, 202,
            "case {case:?}: the second create must be accepted, got {second:?}"
        );
        let second_id = text(&second.body["workspaceId"]);
        assert_ne!(
            second_id, first_id,
            "case {case:?}: a workspace whose removal is owed is not what a create is \
             answered from, got {second:?}"
        );

        wait_for_due_transition();
        let old = amp
            .call("GET", &format!("/workspaces/{first_id}"), region, "")
            .await;
        assert_eq!(
            old.status, 404,
            "case {case:?}: the deleted workspace must be gone, got {old:?}"
        );
        let new = amp
            .call("GET", &format!("/workspaces/{second_id}"), region, "")
            .await;
        assert_eq!(
            new.status, 200,
            "case {case:?}: the created workspace must survive the other's removal, got \
             {new:?}"
        );
        assert_eq!(
            text(&new.body["workspace"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {case:?}: the created workspace must settle {STATUS_ACTIVE}, got {new:?}"
        );
        assert_eq!(
            text(&new.body["workspace"]["alias"]),
            "after-deleting",
            "case {case:?}: the created workspace must carry what its own create said, \
             got {new:?}"
        );
    }
}

/// A rule groups namespace is keyed by its NAME: a create carrying the token of a
/// namespace whose removal is owed is not a replay of it, so a create under another name
/// makes that namespace, and a create under the same name runs into the name that is
/// still taken (the modelled `ConflictException`, 409) instead of being accepted and
/// persisting nothing.
#[tokio::test]
async fn a_namespace_create_replaying_a_token_of_a_deleting_namespace_is_not_a_replay() {
    let region = "us-west-2";
    for (case, second_name) in [
        ("another name, the same token", "second-rules"),
        ("the name being deleted, the same token", "first-rules"),
    ] {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "namespace-deleting").await;
        let mut body = create_body("first-rules", b"groups: []\n", &[]);
        body["clientToken"] = serde_json::Value::String(FIRST_TOKEN.to_string());
        let created = create_namespace(&amp, region, &workspace_id, &body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the first create must be accepted, got {created:?}"
        );
        wait_for_due_transition();

        let deleted = amp
            .call(
                "DELETE",
                &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/first-rules"),
                region,
                "",
            )
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: the delete must be accepted, got {deleted:?}"
        );
        let pending = describe_namespace(&amp, region, &workspace_id, "first-rules").await;
        assert_eq!(
            text(&pending.body["ruleGroupsNamespace"]["status"]["statusCode"]),
            NAMESPACE_STATUS_DELETING,
            "case {case:?}: the delete must leave the record {NAMESPACE_STATUS_DELETING}, \
             got {pending:?}"
        );

        let mut second_body = create_body(second_name, b"groups:\n- name: cpu\n", &[]);
        second_body["clientToken"] = serde_json::Value::String(FIRST_TOKEN.to_string());
        let second = create_namespace(&amp, region, &workspace_id, &second_body).await;
        if second_name == "first-rules" {
            assert_eq!(
                second.status, 409,
                "case {case:?}: the name is still taken by the namespace being deleted, \
                 got {second:?}"
            );
            assert_eq!(
                second.error_code(),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {second:?}"
            );
            continue;
        }
        assert_eq!(
            second.status, 202,
            "case {case:?}: the second create must be accepted, got {second:?}"
        );
        assert_eq!(
            text(&second.body["name"]),
            second_name,
            "case {case:?}: the reply must name the namespace this call created, not the \
             one being deleted, got {second:?}"
        );
        let described = describe_namespace(&amp, region, &workspace_id, second_name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the accepted create must be readable back, got {described:?}"
        );
    }
}

/// An anomaly detector's id is minted: a create carrying the token of a detector whose
/// removal is owed makes a NEW detector rather than answering with the one that is about
/// to be gone.
#[tokio::test]
async fn a_detector_create_replaying_a_token_of_a_deleting_detector_makes_a_new_one() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "detector-deleting").await;
    let mut body = serde_json::json!({
        "alias": "first-detector",
        "configuration": minimal_configuration("sum(rate(requests_total[5m]))"),
        "clientToken": FIRST_TOKEN,
    });
    let created = create_detector(&amp, region, &workspace_id, &body).await;
    assert_eq!(
        created.status, 202,
        "the first create must be accepted, got {created:?}"
    );
    let first_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{first_id}"),
            region,
            "",
        )
        .await;
    assert_eq!(deleted.status, 202, "the delete must be accepted, got {deleted:?}");
    let pending = describe_detector(&amp, region, &workspace_id, &first_id).await;
    assert_eq!(
        text(&pending.body["anomalyDetector"]["status"]["statusCode"]),
        STATUS_DELETING,
        "the delete must leave the record {STATUS_DELETING}, got {pending:?}"
    );

    body["alias"] = serde_json::Value::String("second-detector".to_string());
    let second = create_detector(&amp, region, &workspace_id, &body).await;
    assert_eq!(
        second.status, 202,
        "the second create must be accepted, got {second:?}"
    );
    let second_id = text(&second.body["anomalyDetectorId"]);
    assert_ne!(
        second_id, first_id,
        "a detector whose removal is owed is not what a create is answered from, got \
         {second:?}"
    );

    wait_for_due_transition();
    let gone = describe_detector(&amp, region, &workspace_id, &first_id).await;
    assert_eq!(
        gone.status, 404,
        "the deleted detector must be gone, got {gone:?}"
    );
    let survivor = describe_detector(&amp, region, &workspace_id, &second_id).await;
    assert_eq!(
        survivor.status, 200,
        "the created detector must survive the other's removal, got {survivor:?}"
    );
    assert_eq!(
        text(&survivor.body["anomalyDetector"]["alias"]),
        "second-detector",
        "the created detector must carry what its own create said, got {survivor:?}"
    );
}

/// A scraper's id is minted too, so the same rule holds for it: a create carrying the
/// token of a scraper whose removal is owed makes a NEW scraper.
#[tokio::test]
async fn a_scraper_create_replaying_a_token_of_a_deleting_scraper_makes_a_new_one() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace = amp.call("POST", "/workspaces", region, "{}").await;
    assert_eq!(
        workspace.status, 202,
        "the destination workspace must be creatable, got {workspace:?}"
    );
    let workspace_arn = text(&workspace.body["arn"]);
    let account = engine::emulator_account_id();
    let mut body = serde_json::json!({
        "alias": "first-scraper",
        "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
        "source": {
            "eksConfiguration": {
                "clusterArn": format!("arn:aws:eks:{region}:{account}:cluster/metrics"),
                "subnetIds": ["subnet-0123456789abcdef0"],
            },
        },
        "destination": { "ampConfiguration": { "workspaceArn": workspace_arn } },
        "clientToken": FIRST_TOKEN,
    });
    let created = amp.call("POST", "/scrapers", region, &body.to_string()).await;
    assert_eq!(
        created.status, 202,
        "the first create must be accepted, got {created:?}"
    );
    let first_id = text(&created.body["scraperId"]);
    wait_for_due_transition();

    let deleted = amp
        .call("DELETE", &format!("/scrapers/{first_id}"), region, "")
        .await;
    assert_eq!(deleted.status, 202, "the delete must be accepted, got {deleted:?}");
    assert_eq!(
        text(&deleted.body["status"]["statusCode"]),
        STATUS_DELETING,
        "the delete must report the status it wrote, got {deleted:?}"
    );

    body["alias"] = serde_json::Value::String("second-scraper".to_string());
    let second = amp.call("POST", "/scrapers", region, &body.to_string()).await;
    assert_eq!(
        second.status, 202,
        "the second create must be accepted, got {second:?}"
    );
    let second_id = text(&second.body["scraperId"]);
    assert_ne!(
        second_id, first_id,
        "a scraper whose removal is owed is not what a create is answered from, got \
         {second:?}"
    );

    wait_for_due_transition();
    let gone = amp
        .call("GET", &format!("/scrapers/{first_id}"), region, "")
        .await;
    assert_eq!(gone.status, 404, "the deleted scraper must be gone, got {gone:?}");
    let survivor = amp
        .call("GET", &format!("/scrapers/{second_id}"), region, "")
        .await;
    assert_eq!(
        survivor.status, 200,
        "the created scraper must survive the other's removal, got {survivor:?}"
    );
    assert_eq!(
        text(&survivor.body["scraper"]["alias"]),
        "second-scraper",
        "the created scraper must carry what its own create said, got {survivor:?}"
    );
    assert_eq!(
        text(&survivor.body["scraper"]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "the created scraper must settle {STATUS_ACTIVE}, got {survivor:?}"
    );
}
