// SPDX-License-Identifier: CC0-1.0

//! The Spark Connect session surface of a cluster, end to end over the wire.
//!
//! THE RULE: the session surface exists only on a cluster whose LAUNCH enabled it — "set
//! `SessionEnabled` to true when creating the cluster" — so every operation of that surface
//! refuses a cluster without it, with the client error it models and Amazon EMR's own
//! sentence for the class, and `ListSessions` refuses rather than reporting an empty list.
//! On a cluster that does carry it, `StartSession` records a session — "when a session is
//! first created, it enters the `SUBMITTED` state" (`StartSessionOutput$State` doc, Smithy
//! model) — which `GetSession` and `ListSessions` then report, newest first, and which the
//! clock walks on to `STARTING` and `STARTED`.
//!
//! The rows span what the rule generalises over: the operation of the surface that is called
//! and the request it carries, whether the cluster's launch enabled sessions (absent, false,
//! true), the state the cluster is in, and the filters and page sizes a listing is given.
//! Every session is read back over the wire through `GetSession` and `ListSessions`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    spark_release_labels, SESSION_ID_PREFIX, SESSION_PHASE_SPAN,
};

use crate::job_flow_lifecycle::{
    assert_minted_id, at, Emr, PAST_CLUSTER_READY, INSTANCE_PROFILE, SERVICE_ROLE,
};
use crate::refusal_messages::message;

/// The sentence Amazon EMR refuses the session surface of a cluster without sessions with:
/// fixed text apart from the cluster id the request sent, quoted.
fn not_enabled_sentence(cluster_id: &str) -> String {
    format!(
        "Sessions are not enabled on cluster '{cluster_id}'. Set SessionEnabled to true \
         when creating the cluster to enable sessions."
    )
}

/// The Amazon EMR Spark release the session surface is documented for: "this operation is
/// supported in Amazon EMR Spark 8.0.0 and later" (`StartSession` doc, Smithy model).
const SPARK_RELEASE: &str = "emr-spark-8.0.0";

/// A launch whose `SessionEnabled` is `enabled` when the row states one at all.
fn launch_request(name: &str, enabled: Option<bool>) -> Value {
    let mut request = json!({
        "Name": name,
        "ReleaseLabel": SPARK_RELEASE,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Applications": [{ "Name": "Spark" }],
        "Instances": {
            "InstanceCount": 2,
            "MasterInstanceType": "m5.xlarge",
            "SlaveInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    });
    if let Some(enabled) = enabled {
        request["SessionEnabled"] = json!(enabled);
    }
    request
}

/// THE RULE's refusal half: on a cluster whose launch did not enable sessions, BOTH
/// `StartSession` and `ListSessions` answer HTTP 400 `InvalidRequestException` with the
/// sentence above — the listing too, rather than an empty `Sessions` list.
#[tokio::test]
async fn the_session_surface_refuses_a_cluster_that_was_not_launched_with_sessions() {
    struct Case {
        label: &'static str,
        /// What the launch said about `SessionEnabled`.
        enabled: Option<bool>,
    }

    let cases = [
        Case {
            label: "a launch that said nothing about sessions",
            enabled: None,
        },
        Case {
            label: "a launch that disabled sessions explicitly",
            enabled: Some(false),
        },
    ];

    let emr = Emr::new();
    tokio::time::sleep(PAST_CLUSTER_READY).await;

    for case in cases {
        let label = case.label;
        let cluster_id = emr.run_job_flow(launch_request(label, case.enabled)).await;
        tokio::time::sleep(PAST_CLUSTER_READY).await;

        // Every request shape of the surface, including a listing with filters and a page
        // size, is refused the same way: the gate is about the CLUSTER, not the request.
        let requests = [
            ("StartSession", json!({ "ClusterId": cluster_id.clone() })),
            (
                "StartSession",
                json!({ "ClusterId": cluster_id.clone(), "Name": "analysis" }),
            ),
            ("ListSessions", json!({ "ClusterId": cluster_id.clone() })),
            (
                "ListSessions",
                json!({
                    "ClusterId": cluster_id.clone(),
                    "SessionStates": ["STARTED"],
                    "MaxResults": 10,
                }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{label}: {operation}");
            let (status, payload) = emr.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
            assert!(
                code.contains("InvalidRequestException"),
                "case {case:?}: expected InvalidRequestException, got {code:?} in {payload}"
            );
            assert_eq!(
                message(&case, &payload),
                not_enabled_sentence(&cluster_id),
                "case {case:?}: expected the sessions-not-enabled sentence, got {payload}"
            );
        }
    }
}

/// THE RULE's positive half: a cluster launched with sessions starts them, reports each one
/// through `GetSession` and `ListSessions` newest first, walks it from `SUBMITTED` to
/// `STARTED`, and answers a repeat of the same `ClientRequestToken` with the session the
/// first request created.
#[tokio::test]
async fn a_session_enabled_cluster_starts_sessions_and_reports_them() {
    let emr = Emr::new();
    let cluster_id = emr
        .run_job_flow(launch_request("sessions", Some(true)))
        .await;
    tokio::time::sleep(PAST_CLUSTER_READY).await;

    // The requests span what `StartSession` accepts: a bare one, a named one, one with an
    // execution role, engine configuration overrides, an idle timeout, tags and a client
    // request token.
    let requests = vec![
        ("bare", json!({ "ClusterId": cluster_id.clone() })),
        (
            "named",
            json!({ "ClusterId": cluster_id.clone(), "Name": "analysis" }),
        ),
        (
            "configured",
            json!({
                "ClusterId": cluster_id.clone(),
                "Name": "configured",
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMR_Session_Role",
                "EngineConfigurations": [{
                    "Classification": "spark-defaults",
                    "Properties": { "spark.executor.memory": "2g" },
                }],
                "SessionIdleTimeoutInMinutes": 30,
                "Tags": [{ "Key": "team", "Value": "data" }],
                "ClientRequestToken": "token-configured",
            }),
        ),
    ];

    let mut started: Vec<(String, String)> = Vec::new();
    for (label, request) in &requests {
        let answer = emr.ok("StartSession", request.clone()).await;
        let id = answer["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: StartSession answers an Id, got {answer}"))
            .to_string();
        assert_minted_id(label, SESSION_ID_PREFIX, &id);
        assert_eq!(
            answer["ClusterId"].as_str(),
            Some(cluster_id.as_str()),
            "case {label:?}: the answer names the cluster the session runs on, got {answer}"
        );
        assert_eq!(
            answer["State"].as_str(),
            Some("SUBMITTED"),
            "case {label:?}: a session first created is SUBMITTED, got {answer}"
        );
        let arn = answer["Arn"].as_str().unwrap_or_default();
        assert!(
            arn.starts_with("arn:aws:elasticmapreduce:") && arn.ends_with(&format!("session/{id}")),
            "case {label:?}: the answer names the session's own ARN, got {arn:?}"
        );
        assert!(
            answer["AccountId"].as_str().is_some_and(|account| {
                account.len() == 12 && account.chars().all(|digit| digit.is_ascii_digit())
            }),
            "case {label:?}: the answer names the owning account, got {answer}"
        );
        started.push(((*label).to_string(), id));
    }

    // Every session is read back through `GetSession`, with the configuration its own
    // request supplied and the cluster's release beside it.
    for (label, id) in &started {
        let session = emr
            .ok(
                "GetSession",
                json!({ "ClusterId": cluster_id.clone(), "SessionId": id.clone() }),
            )
            .await["Session"]
            .clone();
        assert_eq!(
            session["Id"].as_str(),
            Some(id.as_str()),
            "case {label:?}: GetSession reports the session that was started, got {session}"
        );
        assert_eq!(
            session["ReleaseLabel"].as_str(),
            Some(SPARK_RELEASE),
            "case {label:?}: a session reports the release its CLUSTER runs, got {session}"
        );
        assert!(
            session["CreatedAt"].as_f64().is_some(),
            "case {label:?}: a session reports when it was created, got {session}"
        );
        // No Spark Connect server is served for a session, so no URL is reported for one.
        assert!(
            session.get("ServerUrl").is_none_or(Value::is_null),
            "case {label:?}: no session reports a Spark Connect URL, got {session}"
        );
    }

    let configured = emr
        .ok(
            "GetSession",
            json!({
                "ClusterId": cluster_id.clone(),
                "SessionId": started[2].1.clone(),
            }),
        )
        .await["Session"]
        .clone();
    assert_eq!(
        configured["Name"].as_str(),
        Some("configured"),
        "the session reports the name its request gave it, got {configured}"
    );
    assert_eq!(
        configured["SessionIdleTimeoutInMinutes"].as_i64(),
        Some(30),
        "the session reports the idle timeout its request gave it, got {configured}"
    );
    assert_eq!(
        configured["EngineConfigurations"][0]["Classification"].as_str(),
        requests[2].1["EngineConfigurations"][0]["Classification"].as_str(),
        "the session reports the engine configuration its request gave it, got {configured}"
    );
    assert_eq!(
        configured["Tags"][0]["Key"].as_str(),
        Some("team"),
        "the session reports the tags its request gave it, got {configured}"
    );

    // "Newer sessions are returned first." (`ListSessions` doc.)
    let listed = emr
        .ok("ListSessions", json!({ "ClusterId": cluster_id.clone() }))
        .await;
    let ids: Vec<&str> = at(&listed, &["Sessions"])
        .as_array()
        .map(|sessions| {
            sessions
                .iter()
                .filter_map(|session| session["Id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    let newest_first: Vec<&str> = started
        .iter()
        .rev()
        .map(|(_, id)| id.as_str())
        .collect();
    assert_eq!(
        ids, newest_first,
        "ListSessions reports every session newest first, got {listed}"
    );

    // One page per session: the token is the first session the page did not report.
    let mut paged: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "ClusterId": cluster_id.clone(), "MaxResults": 1 });
        if let Some(token) = token.as_ref() {
            request["NextToken"] = json!(token);
        }
        let page = emr.ok("ListSessions", request).await;
        let batch: Vec<String> = at(&page, &["Sessions"])
            .as_array()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter_map(|session| session["Id"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            batch.len() <= 1,
            "a page of one holds at most one session, got {page}"
        );
        paged.extend(batch);
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        paged,
        newest_first
            .iter()
            .map(|id| (*id).to_string())
            .collect::<Vec<String>>(),
        "paging reports every session exactly once, in the same order"
    );

    // A token the listing did not mint is refused.
    let (status, payload) = emr
        .call(
            "ListSessions",
            json!({ "ClusterId": cluster_id.clone(), "NextToken": "is-NOTATOKEN" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an unminted NextToken is refused, got {status} with {payload}"
    );

    // The same `ClientRequestToken` answers the session the first request created, and
    // starts no second one.
    let repeated = emr.ok("StartSession", requests[2].1.clone()).await;
    assert_eq!(
        repeated["Id"].as_str(),
        Some(started[2].1.as_str()),
        "a repeated ClientRequestToken answers the first session, got {repeated}"
    );
    let listed = emr
        .ok("ListSessions", json!({ "ClusterId": cluster_id.clone() }))
        .await;
    assert_eq!(
        at(&listed, &["Sessions"]).as_array().map(Vec::len),
        Some(started.len()),
        "a repeated ClientRequestToken starts no second session, got {listed}"
    );

    // The clock walks a session on: `STARTING` after one span, `STARTED` after two. The
    // state filter partitions the sessions by the state they have reached.
    tokio::time::sleep(SESSION_PHASE_SPAN.saturating_mul(2) + Duration::from_millis(900)).await;
    for (label, id) in &started {
        let session = emr
            .ok(
                "GetSession",
                json!({ "ClusterId": cluster_id.clone(), "SessionId": id.clone() }),
            )
            .await["Session"]
            .clone();
        assert_eq!(
            session["State"].as_str(),
            Some("STARTED"),
            "case {label:?}: the clock takes a session to STARTED, got {session}"
        );
        assert!(
            session["StartedAt"].as_f64().is_some(),
            "case {label:?}: a STARTED session reports when it entered that state, got \
             {session}"
        );
    }
    for (states, expected) in [
        (json!(["STARTED"]), started.len()),
        (json!(["SUBMITTED"]), 0),
        (json!(["SUBMITTED", "STARTED"]), started.len()),
        (json!(["TERMINATED"]), 0),
        // A filter that names no state narrows nothing, the way an absent one does.
        (json!([]), started.len()),
    ] {
        let filtered = emr
            .ok(
                "ListSessions",
                json!({ "ClusterId": cluster_id.clone(), "SessionStates": states.clone() }),
            )
            .await;
        assert_eq!(
            at(&filtered, &["Sessions"]).as_array().map(Vec::len),
            Some(expected),
            "case {states:?}: expected {expected} sessions, got {filtered}"
        );
    }
}

/// The release family the session surface needs: a cluster launches on an Amazon EMR Spark
/// release, and `DescribeReleaseLabel` reports what one bundles. The rows span the spellings
/// around the one release the family offers, so a label the family does not publish is
/// refused rather than launched.
#[tokio::test]
async fn a_cluster_launches_on_an_amazon_emr_spark_release() {
    let emr = Emr::new();

    // `(the label, whether a launch may name it)`. The offered release comes from the
    // service's own family (`state::spark_release_labels`), and the rows around it are the
    // spellings next to it: another patch, another line, another case.
    let offered = spark_release_labels();
    assert_eq!(
        offered,
        vec![SPARK_RELEASE.to_string()],
        "the Spark family offers the release the session surface needs, got {offered:?}"
    );
    let rows: Vec<(&str, bool)> = vec![
        (SPARK_RELEASE, true),
        ("emr-spark-8.0.1", false),
        ("emr-spark-7.0.0", false),
        ("emr-spark-8.0", false),
        ("emr-SPARK-8.0.0", false),
        ("emr-spark-8.0.0.0", false),
        // The series that is not this family is unaffected: it resolves as it always did.
        ("emr-7.1.0", true),
    ];

    for (label, launchable) in rows {
        let mut request = launch_request(label, Some(true));
        request["ReleaseLabel"] = json!(label);
        let (status, payload) = emr.call("RunJobFlow", request).await;
        if !launchable {
            assert_eq!(
                status, 400,
                "case {label:?}: a label the catalog does not offer launches nothing, got \
                 {status} with {payload}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {label:?}: expected the launch to be accepted, got {status} with {payload}"
        );
        let cluster_id = payload["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: RunJobFlow answers an id"))
            .to_string();
        let described = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            described["ReleaseLabel"].as_str(),
            Some(label),
            "case {label:?}: the cluster reports the release it was launched on, got \
             {described}"
        );
    }

    // What the family's release bundles, read back through the operation that reports it.
    let described = emr
        .ok(
            "DescribeReleaseLabel",
            json!({ "ReleaseLabel": SPARK_RELEASE }),
        )
        .await;
    assert_eq!(
        described["ReleaseLabel"].as_str(),
        Some(SPARK_RELEASE),
        "the description names the release asked for, got {described}"
    );
    let applications: Vec<&str> = described["Applications"]
        .as_array()
        .map(|applications| {
            applications
                .iter()
                .filter_map(|application| application["Name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        applications,
        vec!["Spark"],
        "an Amazon EMR Spark release bundles Spark, got {described}"
    );
}

/// A cluster that carries the surface but is not up: "the cluster must be in the RUNNING or
/// WAITING state" (`StartSession` doc), so a session cannot be started on one that is
/// shutting down — and the refusal names the state it is in.
#[tokio::test]
async fn a_session_is_not_started_on_a_cluster_that_is_not_running() {
    let emr = Emr::new();
    let cluster_id = emr
        .run_job_flow(launch_request("terminating", Some(true)))
        .await;
    tokio::time::sleep(PAST_CLUSTER_READY).await;
    emr.ok(
        "TerminateJobFlows",
        json!({ "JobFlowIds": [cluster_id.clone()] }),
    )
    .await;

    let (status, payload) = emr
        .call("StartSession", json!({ "ClusterId": cluster_id.clone() }))
        .await;
    assert_eq!(
        status, 400,
        "a cluster that is not up starts no session, got {status} with {payload}"
    );
    let said = message("a terminating cluster", &payload);
    assert!(
        said.contains(&cluster_id) && said.contains("RUNNING") && said.contains("WAITING"),
        "the refusal names the cluster and the states a session needs, got {said:?}"
    );

    // The listing still answers for that cluster: its sessions are a list, and it has none.
    let listed = emr
        .ok("ListSessions", json!({ "ClusterId": cluster_id.clone() }))
        .await;
    assert_eq!(
        at(&listed, &["Sessions"]).as_array().map(Vec::len),
        Some(0),
        "a session-enabled cluster with no sessions lists none, got {listed}"
    );
}
