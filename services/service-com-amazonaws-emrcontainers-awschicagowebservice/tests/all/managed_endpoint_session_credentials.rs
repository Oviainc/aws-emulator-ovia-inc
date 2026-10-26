// SPDX-License-Identifier: CC0-1.0

//! `GetManagedEndpointSessionCredentials`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. The endpoint fixtures are the ones [`crate::managed_endpoint_listing`] drives,
//! so the suites cannot drift into two request paths.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The call MINTS a session for an endpoint the store holds and answers out of the
//!    stored record: a `ResourceIdString` `id`, one session token reported by both
//!    `credentials` and `endpointCredentials`, and an `expiresAt` of the instant the call
//!    was served plus the `durationInSeconds` it asked for — the documented 15-minute
//!    default when it asked for none. Every session's token is distinct and none of them is
//!    any value the request carried.
//! 2. The endpoint is resolved by the pair (`virtualClusterIdentifier`,
//!    `endpointIdentifier`), each written as the resource's id or as its ARN; a pair the
//!    store does not hold — in this region — is refused with the modelled
//!    `ResourceNotFoundException` (HTTP 400).
//! 3. A session connects a client through the endpoint's gateway, so only an `ACTIVE`
//!    endpoint has one: an endpoint still `CREATING`, or one torn down with its virtual
//!    cluster, is refused with the modelled `ValidationException` (HTTP 400).
//! 4. A request outside what the operation issues is refused with the modelled
//!    `ValidationException`: a `credentialType` other than the one supported type, and a
//!    `durationInSeconds` outside 1..=12 hours.
//! 5. `clientToken` is `@idempotencyToken`: the same request replayed answers with the
//!    SAME session rather than minting a second credential, the same token carrying a
//!    different request is refused, and a request naming no token mints its own session
//!    every time.
//! 6. A session's expiry is owed to the clock: once it has passed, the same token mints a
//!    fresh session instead of handing back an expired one.
//! 7. Issuing a session is not a write to the endpoint: however many are issued, every
//!    reader answers the same endpoint record, and a refused request changes nothing.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::created_metadata_wire_form::assert_date_time_string;
use crate::managed_endpoint_listing::{
    create_endpoint, describe_endpoint, described, endpoint_body, wait_for_the_clock,
};
use crate::virtual_cluster_job_run_loop::{
    assert_resource_id, field, instant_millis, text, Answer, Emr, REGION,
};

/// `POST /virtualclusters/{virtualClusterIdentifier}/endpoints/{endpointIdentifier}/credentials`
/// — `GetManagedEndpointSessionCredentials`.
async fn issue_session(emr: &Emr, cluster: &str, endpoint: &str, body: Value) -> Answer {
    emr.send(
        "POST",
        &format!("/virtualclusters/{cluster}/endpoints/{endpoint}/credentials"),
        Some(body),
    )
    .await
}

/// An identifier as one URI path segment: a `/` inside an ARN is sent `%2F`, which is how
/// an SDK encodes a value bound to a non-greedy `@httpLabel`.
fn path_segment(identifier: &str) -> String {
    identifier.replace('/', "%2F")
}

/// One acceptable request body: the two members the model marks `@required` beside the two
/// `@httpLabel` identifiers (smithy_query: operation
/// `GetManagedEndpointSessionCredentials`), plus whatever a row adds.
fn request_body(extra: Value) -> Value {
    let mut body = json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
        "credentialType": state::SESSION_CREDENTIAL_TYPE_TOKEN,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// A virtual cluster holding one managed endpoint that has reached `ACTIVE`, which is the
/// state a session is issued for.
async fn active_endpoint(emr: &Emr, label: &str) -> (String, String) {
    let cluster = emr
        .cluster(&format!("vc-{label}"), &format!("vc-tok-{label}"))
        .await;
    let created = create_endpoint(
        emr,
        &cluster,
        endpoint_body(label, &format!("ep-tok-{label}"), "JUPYTER_SERVER"),
    )
    .await;
    let endpoint = text(created.ok(label), "/id", label);
    // The endpoint is accepted CREATING and reaches ACTIVE on the clock.
    wait_for_the_clock();
    (cluster, endpoint)
}

/// The one session token a successful answer reports, read off BOTH credential members the
/// model declares: `credentials` ("The structure containing the session credentials") and
/// `endpointCredentials` ("The session credentials that the operation returns") name the
/// same session, so a client reading either reaches it.
fn reported_token(body: &Value, case: &str) -> String {
    let token = text(body, "/credentials/token", case);
    assert_eq!(
        text(body, "/endpointCredentials/token", case),
        token,
        "case {case:?}: the two credential members must report one session token: {body}"
    );
    assert!(
        !token.trim().is_empty(),
        "case {case:?}: the session token is blank: {body}"
    );
    token
}

/// RULES 1 and 2: a session is minted for an `ACTIVE` endpoint and answered out of the
/// stored record, over rows spanning what a request may ask for and how it may name the
/// endpoint.
#[tokio::test]
async fn a_session_is_issued_for_an_active_endpoint_and_reports_what_was_stored() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "issue").await;

    // The rows differ along the duration asked for — absent, both published bounds of the
    // documented 1..=12-hour window, and values inside it — along the optional members
    // carried, and along the FORM each identifier takes (the resource's id, or its ARN).
    let default_seconds = state::DEFAULT_SESSION_DURATION_SECONDS;
    let max_seconds = state::MAX_SESSION_DURATION_SECONDS;
    let cluster_arn = state::virtual_cluster_arn(REGION, &cluster);
    let endpoint_arn = state::managed_endpoint_arn(REGION, &cluster, &endpoint);
    let rows: Vec<(String, String, String, Value, i32)> = vec![
        (
            "no duration named, so the documented default".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({}),
            default_seconds,
        ),
        (
            "the shortest duration a token can be valid for".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({"durationInSeconds": 1}),
            1,
        ),
        (
            "a minute".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({"durationInSeconds": 60}),
            60,
        ),
        (
            "the documented default named explicitly".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({"durationInSeconds": default_seconds}),
            default_seconds,
        ),
        (
            "the documented maximum of 12 hours".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({"durationInSeconds": max_seconds}),
            max_seconds,
        ),
        (
            "a log context and a client token".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({"logContext": "studio-logs", "clientToken": "issue-tok-logctx"}),
            default_seconds,
        ),
        (
            "an execution role with a path".to_string(),
            cluster.clone(),
            endpoint.clone(),
            json!({
                "executionRoleArn": "arn:aws:iam::000000000000:role/service-role/EMRStudio_Role",
                "durationInSeconds": 3_600,
            }),
            3_600,
        ),
        (
            "the endpoint named by its ARN".to_string(),
            cluster.clone(),
            path_segment(&endpoint_arn),
            json!({}),
            default_seconds,
        ),
        (
            "the virtual cluster named by its ARN".to_string(),
            path_segment(&cluster_arn),
            endpoint.clone(),
            json!({"durationInSeconds": 120}),
            120,
        ),
        (
            "both named by their ARNs".to_string(),
            path_segment(&cluster_arn),
            path_segment(&endpoint_arn),
            json!({}),
            default_seconds,
        ),
    ];

    let mut ids = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    for (case, cluster_identifier, endpoint_identifier, extra, duration_seconds) in &rows {
        let body = request_body(extra.clone());
        let before = state::now_millis();
        let answer = issue_session(&emr, cluster_identifier, endpoint_identifier, body.clone()).await;
        let after = state::now_millis();
        let session = answer.ok(case).clone();

        let id = text(&session, "/id", case);
        assert_resource_id(case, "the session id", &id);
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: session id {id} was already minted for another session"
        );

        let token = reported_token(&session, case);
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: the session token was already issued to another session"
        );
        // The token is a minted secret, not a value echoed out of the request.
        for echoed in [
            cluster.as_str(),
            endpoint.as_str(),
            id.as_str(),
            "arn:aws:iam::000000000000:role/EMRStudioRole",
            "issue-tok-logctx",
        ] {
            assert!(
                token != echoed && !token.contains(echoed),
                "case {case:?}: the session token {token:?} carries request value {echoed:?}"
            );
        }

        // `expiresAt` is the instant the call was served plus the duration it asked for,
        // on the RFC 3339 wire form `Date` carries (`@timestampFormat("date-time")`).
        let validity_millis = i64::from(*duration_seconds) * 1_000;
        assert_date_time_string(
            case,
            &session,
            "/expiresAt",
            before + validity_millis,
            after + validity_millis,
        );
        let expires_at = instant_millis(&session, "/expiresAt", case);
        assert!(
            expires_at > after,
            "case {case:?}: a session expiring at {expires_at} is already expired at {after}"
        );
    }

    assert_eq!(
        ids.len(),
        rows.len(),
        "every accepted request must mint its own session, got {} for {} rows",
        ids.len(),
        rows.len()
    );
}

/// RULE 2: a pair the store does not hold is refused with `ResourceNotFoundException`,
/// including a stored endpoint offered under another cluster, an ARN of another resource,
/// and the right pair asked for at another region's endpoint.
#[tokio::test]
async fn a_pair_the_store_does_not_hold_is_not_found() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "notfound").await;
    let (other_cluster, other_endpoint) = active_endpoint(&emr, "notfoundtwo").await;

    let rows: Vec<(String, String, String)> = vec![
        (
            "an id nothing was minted for".to_string(),
            cluster.clone(),
            "doesnotexist1".to_string(),
        ),
        (
            "the shortest id the model allows".to_string(),
            cluster.clone(),
            "a".to_string(),
        ),
        (
            "the longest id the model allows".to_string(),
            cluster.clone(),
            "z".repeat(64),
        ),
        (
            "an endpoint of the other cluster".to_string(),
            cluster.clone(),
            other_endpoint.clone(),
        ),
        (
            "a stored endpoint under a cluster nothing was minted for".to_string(),
            "nosuchcluster".to_string(),
            endpoint.clone(),
        ),
        (
            "the cluster's own id as an endpoint id".to_string(),
            cluster.clone(),
            cluster.clone(),
        ),
        (
            "the endpoint ARN of the other cluster".to_string(),
            cluster.clone(),
            path_segment(&state::managed_endpoint_arn(
                REGION,
                &other_cluster,
                &other_endpoint,
            )),
        ),
        (
            "an endpoint ARN of another region".to_string(),
            cluster.clone(),
            path_segment(&state::managed_endpoint_arn("eu-west-1", &cluster, &endpoint)),
        ),
        (
            "a job-run ARN as the endpoint identifier".to_string(),
            cluster.clone(),
            path_segment(&state::job_run_arn(REGION, &cluster, &endpoint)),
        ),
        (
            "a job-run ARN as the cluster identifier".to_string(),
            path_segment(&state::job_run_arn(REGION, &cluster, &endpoint)),
            endpoint.clone(),
        ),
        (
            "a virtual-cluster ARN as the endpoint identifier".to_string(),
            cluster.clone(),
            path_segment(&state::virtual_cluster_arn(REGION, &cluster)),
        ),
    ];

    for (case, cluster_identifier, endpoint_identifier) in &rows {
        issue_session(
            &emr,
            cluster_identifier,
            endpoint_identifier,
            request_body(json!({})),
        )
        .await
        .refused(case, 400, "ResourceNotFoundException");
    }

    // The SAME pair, asked for at another region's endpoint: EMR on EKS is regional, so the
    // endpoint exists only where it was created.
    let case = "the stored pair at another region's endpoint";
    let answer = emr
        .send_in(
            "eu-west-1",
            "POST",
            &format!("/virtualclusters/{cluster}/endpoints/{endpoint}/credentials"),
            Some(request_body(json!({}))),
        )
        .await;
    answer.refused(case, 400, "ResourceNotFoundException");

    // The refusals minted nothing and broke nothing: the pair still issues a session.
    let case = "the stored pair after the refusals";
    let answer = issue_session(&emr, &cluster, &endpoint, request_body(json!({}))).await;
    reported_token(answer.ok(case), case);
}

/// RULE 3: only an `ACTIVE` endpoint has a gateway to connect to, so an endpoint in any
/// other state is refused — in the `CREATING` phase it has not reached yet, and in the
/// terminal state its virtual cluster's teardown takes it to.
#[tokio::test]
async fn an_endpoint_that_is_not_active_issues_no_session() {
    let emr = Emr::new();
    let cluster = emr.cluster("vc-states", "vc-tok-states").await;
    let created = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("states", "ep-tok-states", "JUPYTER_ENTERPRISE_GATEWAY"),
    )
    .await;
    let endpoint = text(created.ok("the created endpoint"), "/id", "states");

    // The endpoint is CREATING until the clock makes its step due; a caller acting straight
    // after the create sees that transient state and its state error.
    for attempt in 0..3 {
        let case = format!("attempt {attempt} while the endpoint is still CREATING");
        issue_session(&emr, &cluster, &endpoint, request_body(json!({})))
            .await
            .refused(&case, 400, "ValidationException");
    }
    let case = "the endpoint while it is still CREATING";
    let answer = describe_endpoint(&emr, &cluster, &endpoint).await;
    assert_eq!(
        text(&described(answer.ok(case), case), "/state", case),
        state::ENDPOINT_CREATING,
        "case {case:?}: the fixture must still be in its transient state"
    );

    wait_for_the_clock();
    let case = "the endpoint once it has reached ACTIVE";
    let answer = issue_session(&emr, &cluster, &endpoint, request_body(json!({}))).await;
    reported_token(answer.ok(case), case);

    // Its virtual cluster's teardown takes the endpoint with it, and a torn-down endpoint
    // accepts no client either.
    emr.delete_cluster(&cluster).await.ok("the cluster teardown");
    let case = "the endpoint while its cluster is tearing down";
    issue_session(&emr, &cluster, &endpoint, request_body(json!({})))
        .await
        .refused(case, 400, "ValidationException");
    wait_for_the_clock();
    let case = "the endpoint once its cluster has terminated";
    issue_session(&emr, &cluster, &endpoint, request_body(json!({})))
        .await
        .refused(case, 400, "ValidationException");
}

/// RULE 4: a request asking for something the operation does not issue is refused with the
/// modelled `ValidationException`.
///
/// The `credentialType` rows walk the forms a value that is not the one supported type
/// takes — another case, another word, the same word with surrounding space — and the
/// duration rows walk the published window's bounds and one past each
/// (`GetManagedEndpointSessionCredentialsRequest$durationInSeconds`: "The default duration
/// is 15 minutes and the maximum is 12 hours").
#[tokio::test]
async fn a_request_outside_what_the_operation_issues_is_refused() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "refusals").await;

    let max_seconds = state::MAX_SESSION_DURATION_SECONDS;
    let mut rows: Vec<(String, Value)> = vec![
        (
            "a lower-case credential type".to_string(),
            json!({"credentialType": "token"}),
        ),
        (
            "a mixed-case credential type".to_string(),
            json!({"credentialType": "Token"}),
        ),
        (
            "the supported type with trailing space".to_string(),
            json!({"credentialType": "TOKEN "}),
        ),
        (
            "a credential type the service does not issue".to_string(),
            json!({"credentialType": "SESSION"}),
        ),
        (
            "a credential type at the model's length maximum".to_string(),
            json!({"credentialType": "T".repeat(64)}),
        ),
        (
            "a zero duration".to_string(),
            json!({"durationInSeconds": 0}),
        ),
        (
            "a negative duration".to_string(),
            json!({"durationInSeconds": -1}),
        ),
        (
            "one second past the documented maximum".to_string(),
            json!({"durationInSeconds": max_seconds + 1}),
        ),
        (
            "a duration of a whole day".to_string(),
            json!({"durationInSeconds": 24 * 60 * 60}),
        ),
    ];
    rows.push((
        "the largest duration an integer can carry".to_string(),
        json!({"durationInSeconds": i32::MAX}),
    ));

    for (case, extra) in &rows {
        issue_session(&emr, &cluster, &endpoint, request_body(extra.clone()))
            .await
            .refused(case, 400, "ValidationException");
    }

    // Every bound the rows step past is itself accepted, so the refusals are about the
    // window and not about the member being named at all.
    for (case, extra) in [
        ("the lowest accepted duration", json!({"durationInSeconds": 1})),
        (
            "the highest accepted duration",
            json!({"durationInSeconds": max_seconds}),
        ),
    ] {
        let answer = issue_session(&emr, &cluster, &endpoint, request_body(extra)).await;
        reported_token(answer.ok(case), case);
    }
}

/// RULE 5: `clientToken` is `@idempotencyToken`, so a replay names the session the first
/// call issued, the same token with different parameters is refused, and a request naming
/// no token mints its own session each time.
#[tokio::test]
async fn a_replayed_client_token_names_the_session_the_first_call_issued() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "idempotency").await;

    // The rows differ along the request the token stands for, so a replay is compared for
    // more than one shape of request.
    let rows: Vec<(String, Value)> = vec![
        (
            "a token on the smallest acceptable request".to_string(),
            json!({"clientToken": "idem-plain"}),
        ),
        (
            "a token on a request that sizes its session".to_string(),
            json!({"clientToken": "idem-sized", "durationInSeconds": 1_800}),
        ),
        (
            "a token on a request carrying a log context".to_string(),
            json!({"clientToken": "idem-logs", "logContext": "idem-logs-bucket"}),
        ),
    ];

    let mut issued: Vec<(String, Value)> = Vec::new();
    for (case, extra) in &rows {
        let body = request_body(extra.clone());
        let first = issue_session(&emr, &cluster, &endpoint, body.clone()).await;
        let session = first.ok(case).clone();
        // Replayed more than once: the second and third calls answer the first session
        // rather than minting a second credential.
        for replay in 0..3 {
            let case = format!("{case}, replay {replay}");
            let again = issue_session(&emr, &cluster, &endpoint, body.clone()).await;
            assert_eq!(
                again.ok(&case),
                &session,
                "case {case:?}: a replayed clientToken must answer the first session"
            );
        }
        issued.push((text(&session, "/id", case), session));
    }

    // Distinct tokens are distinct requests: each mints its own session.
    let ids: BTreeSet<String> = issued.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        ids.len(),
        rows.len(),
        "each clientToken must name its own session, got {ids:?}"
    );
    let tokens: BTreeSet<String> = issued
        .iter()
        .map(|(id, session)| reported_token(session, id))
        .collect();
    assert_eq!(
        tokens.len(),
        rows.len(),
        "each session must carry its own token"
    );

    // The SAME token carrying a DIFFERENT request is not a replay: answering it with the
    // first session would drop what this call asked for.
    for (case, extra) in [
        (
            "a spent token with another duration",
            json!({"clientToken": "idem-plain", "durationInSeconds": 60}),
        ),
        (
            "a spent token with another execution role",
            json!({
                "clientToken": "idem-plain",
                "executionRoleArn": "arn:aws:iam::000000000000:role/OtherStudioRole",
            }),
        ),
        (
            "a spent token with a log context the first call did not carry",
            json!({"clientToken": "idem-plain", "logContext": "other-logs"}),
        ),
    ] {
        issue_session(&emr, &cluster, &endpoint, request_body(extra))
            .await
            .refused(case, 400, "ValidationException");
    }

    // And the refusals left the first session replayable, unchanged.
    let case = "the first session after the refused replays";
    let again = issue_session(
        &emr,
        &cluster,
        &endpoint,
        request_body(json!({"clientToken": "idem-plain"})),
    )
    .await;
    assert_eq!(
        again.ok(case),
        &issued[0].1,
        "case {case:?}: a refused replay must not have changed the stored session"
    );

    // A request naming no `clientToken` is not replayable: each call mints its own session.
    let mut untokened = BTreeSet::new();
    for attempt in 0..3 {
        let case = format!("an untokened request, attempt {attempt}");
        let answer = issue_session(&emr, &cluster, &endpoint, request_body(json!({}))).await;
        let session = answer.ok(&case).clone();
        assert!(
            untokened.insert(reported_token(&session, &case)),
            "case {case:?}: a request carrying no clientToken must mint its own session"
        );
    }
}

/// RULE 6: a session's expiry is owed to the clock. Once `expiresAt` has passed the record
/// is gone, so the same `clientToken` mints a FRESH session instead of handing back a token
/// that no longer authenticates anything.
#[tokio::test]
async fn an_expired_session_is_not_handed_out_again() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "expiry").await;
    let body = request_body(json!({"clientToken": "expiry-tok", "durationInSeconds": 1}));

    let first = issue_session(&emr, &cluster, &endpoint, body.clone()).await;
    let first = first.ok("the session issued for one second").clone();
    let case = "a replay while the session is still valid";
    let replayed = issue_session(&emr, &cluster, &endpoint, body.clone()).await;
    assert_eq!(
        replayed.ok(case),
        &first,
        "case {case:?}: a valid session must still be replayable"
    );

    // Past its own `expiresAt`; the service stores the instant and never sleeps, so this is
    // the test's own clock.
    std::thread::sleep(Duration::from_millis(1_300));

    let case = "the same token once the session has expired";
    let fresh = issue_session(&emr, &cluster, &endpoint, body.clone()).await;
    let fresh = fresh.ok(case).clone();
    assert_ne!(
        text(&fresh, "/id", case),
        text(&first, "/id", case),
        "case {case:?}: an expired session must not be handed out again: {fresh}"
    );
    assert_ne!(
        reported_token(&fresh, case),
        reported_token(&first, case),
        "case {case:?}: the fresh session must carry its own token"
    );
    assert!(
        instant_millis(&fresh, "/expiresAt", case) > instant_millis(&first, "/expiresAt", case),
        "case {case:?}: the fresh session must expire later than the one it replaced"
    );
}

/// RULE 7: issuing a session writes the session and nothing else. The endpoint reads back
/// identically however many sessions it has issued, and a refused request changes nothing a
/// later caller sees.
#[tokio::test]
async fn issuing_a_session_does_not_change_the_endpoint() {
    let emr = Emr::new();
    let (cluster, endpoint) = active_endpoint(&emr, "noop").await;

    let case = "the endpoint before any session";
    let answer = describe_endpoint(&emr, &cluster, &endpoint).await;
    let before = described(answer.ok(case), case);

    // The rows differ in how many sessions are issued between two reads, which is what an
    // issue that consumed or moved something on the endpoint would be caught by.
    for issues in [1_usize, 2, 5] {
        let case = format!("{issues} session(s) issued");
        for index in 0..issues {
            let answer = issue_session(
                &emr,
                &cluster,
                &endpoint,
                request_body(json!({"durationInSeconds": 60 + index as i64})),
            )
            .await;
            reported_token(answer.ok(&case), &case);
        }
        // And a refusal in the middle of them.
        issue_session(
            &emr,
            &cluster,
            &endpoint,
            request_body(json!({"credentialType": "NOTTOKEN"})),
        )
        .await
        .refused(&case, 400, "ValidationException");

        let answer = describe_endpoint(&emr, &cluster, &endpoint).await;
        let after = described(answer.ok(&case), &case);
        assert_eq!(
            after, before,
            "case {case:?}: issuing a session must leave the endpoint record untouched"
        );
        assert_eq!(
            text(&after, "/state", &case),
            state::ENDPOINT_ACTIVE,
            "case {case:?}: the endpoint must still be ACTIVE: {after}"
        );
    }

    // The virtual cluster is untouched too: nothing about a session is a cluster write.
    let case = "the virtual cluster after every session";
    let answer = emr.describe_cluster(&cluster).await;
    assert_eq!(
        text(
            field(answer.ok(case), "/virtualCluster", case),
            "/state",
            case
        ),
        state::CLUSTER_RUNNING,
        "case {case:?}: the cluster must still be RUNNING"
    );
}
