// SPDX-License-Identifier: CC0-1.0

//! Managed endpoints end to end: `CreateManagedEndpoint`, `DescribeManagedEndpoint` and
//! `ListManagedEndpoints`.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A create persists the endpoint and every reader answers from THAT record: the
//!    members the request carried come back, the ones it omitted stay absent, and a listed
//!    endpoint is byte-for-byte the record the describe answers with.
//! 2. An endpoint is provisioned, so it is reported `CREATING` right after the create and
//!    `ACTIVE` once the transition the clock owes has come due — in both readers.
//! 3. A list reports the endpoints of the NAMED virtual cluster, newest first; another
//!    cluster's endpoints are neither listed nor describable under it.
//! 4. `ListManagedEndpoints` is `@paginated`, so `maxResults` + `nextToken` walk the stored
//!    set with no repeats and no gaps, every page is at most `maxResults` long, and a
//!    `nextToken` comes back only while endpoints remain.
//! 5. `states`, `types`, `createdAfter` and `createdBefore` filter the listing; a filter
//!    the request omits (or sends empty) constrains nothing, and the bounds are strict.
//! 6. A list is a read: it does not change what the next caller sees.
//! 7. `clientToken` is `@idempotencyToken`: the same token replayed names the endpoint the
//!    first call minted, so the listing still holds exactly one.
//! 8. A request the operation cannot answer is refused with the modelled error (HTTP 400
//!    `ValidationException` for an unanswerable listing, `ResourceNotFoundException` for a
//!    resource that is not there) and changes nothing.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::ENDPOINT_STATE_VALUES;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{
    assert_resource_id, field, instant_millis, query_instant, text, Answer, Emr, REGION,
};

/// `POST /virtualclusters/{cluster}/endpoints` — `CreateManagedEndpoint`.
///
/// `pub(crate)` because the describe-focused suite beside this one drives the service
/// through the SAME helpers, so the two cannot drift into two request paths.
pub(crate) async fn create_endpoint(emr: &Emr, cluster: &str, body: Value) -> Answer {
    emr.send(
        "POST",
        &format!("/virtualclusters/{cluster}/endpoints"),
        Some(body),
    )
    .await
}

/// `GET /virtualclusters/{cluster}/endpoints/{id}` — `DescribeManagedEndpoint`.
pub(crate) async fn describe_endpoint(emr: &Emr, cluster: &str, id: &str) -> Answer {
    emr.send(
        "GET",
        &format!("/virtualclusters/{cluster}/endpoints/{id}"),
        None,
    )
    .await
}

/// `GET /virtualclusters/{cluster}/endpoints{query}` — `ListManagedEndpoints`.
pub(crate) async fn list_endpoints(emr: &Emr, cluster: &str, query: &str) -> Answer {
    emr.send(
        "GET",
        &format!("/virtualclusters/{cluster}/endpoints{query}"),
        None,
    )
    .await
}

/// Wait until the transition the service owes a fresh endpoint has come due, with margin,
/// so the next call materialises it. The service stores the due instant and never sleeps;
/// this is the test's own clock.
pub(crate) fn wait_for_the_clock() {
    std::thread::sleep(state::TRANSITION_SPAN + Duration::from_millis(250));
}

/// The `endpoints` a list answer carries, in the order it reported them.
pub(crate) fn listed(body: &Value, case: &str) -> Vec<Value> {
    field(body, "/endpoints", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: endpoints is not a list: {body}"))
        .clone()
}

/// The ids a list answer carries, in the order it reported them.
pub(crate) fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    listed(body, case)
        .iter()
        .map(|endpoint| text(endpoint, "/id", case))
        .collect()
}

/// The `endpoint` a describe answer carries.
pub(crate) fn described(body: &Value, case: &str) -> Value {
    field(body, "/endpoint", case).clone()
}

/// The whole record `DescribeManagedEndpoint` answers with for one endpoint.
///
/// This and the three helpers below are the ones every endpoint suite reads a stored
/// endpoint through, so no suite carries its own copy of them.
pub(crate) async fn endpoint_record(emr: &Emr, cluster: &str, id: &str, case: &str) -> Value {
    let answer = describe_endpoint(emr, cluster, id).await;
    described(answer.ok(case), case)
}

/// The `state` `DescribeManagedEndpoint` reports for one endpoint.
pub(crate) async fn endpoint_state(emr: &Emr, cluster: &str, id: &str, case: &str) -> String {
    text(&endpoint_record(emr, cluster, id, case).await, "/state", case)
}

/// The record the listing of `cluster` reports for `id`: the copy the describe has to
/// agree with, because both readers answer from one stored endpoint.
pub(crate) async fn listed_record(emr: &Emr, cluster: &str, id: &str, case: &str) -> Value {
    let answer = list_endpoints(emr, cluster, "").await;
    let body = answer.ok(case).clone();
    listed(&body, case)
        .into_iter()
        .find(|candidate| text(candidate, "/id", case) == id)
        .unwrap_or_else(|| panic!("case {case:?}: the listing does not report {id}: {body}"))
}

/// The record without its `state`, so two reads taken in different phases of a transition
/// can be compared on everything else.
pub(crate) fn without_state(endpoint: &Value, case: &str) -> Value {
    let mut copy = endpoint.clone();
    copy.as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: the endpoint is not an object: {endpoint}"))
        .remove("state");
    copy
}

/// One acceptable `CreateManagedEndpoint` body: the five members the model marks
/// `@required` beside `virtualClusterId`, which is an `@httpLabel`
/// (smithy_query: operation `CreateManagedEndpoint`).
pub(crate) fn endpoint_body(name: &str, token: &str, endpoint_type: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "type": endpoint_type,
        "releaseLabel": "emr-6.10.0-latest",
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
    })
}

/// The `CreateManagedEndpoint` bodies the round-trip suite stores, spanning what an
/// endpoint carries: which optional members are present, how deep
/// `configurationOverrides` goes, the form of the name, and how many tags there are.
pub(crate) fn stored_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            endpoint_body(
                "minimal",
                "ep-tok-minimal",
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        ),
        (
            "a deprecated certificateArn and two tags",
            json!({
                "name": "endpoint.with_cert",
                "clientToken": "ep-tok-cert",
                "type": "JUPYTER_ENTERPRISE_GATEWAY",
                "releaseLabel": "emr-7.0.0-latest",
                "executionRoleArn": "arn:aws:iam::000000000000:role/path/EMRStudioRole",
                // `ACMCertArn` is len 44..2048, pattern
                // ^arn:(aws[a-zA-Z0-9-]*):acm:.+:(\d{12}):certificate/.+$
                "certificateArn":
                    "arn:aws:acm:us-west-2:000000000000:certificate/abcdefgh-1234-5678-9012-abcdefghijkl",
                "tags": {"team": "studio", "env": "test"},
            }),
        ),
        (
            "an application configuration and monitoring overrides",
            json!({
                "name": "endpoint/overrides#1",
                "clientToken": "ep-tok-overrides",
                "type": "JUPYTER_ENTERPRISE_GATEWAY",
                "releaseLabel": "emr-6.11.0-latest",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
                "configurationOverrides": {
                    "applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor.instances": "2"},
                        "configurations": [{
                            "classification": "spark-env",
                            "properties": {"PYSPARK_PYTHON": "python3"},
                        }],
                    }],
                    "monitoringConfiguration": {
                        // `PersistentAppUI` is an enum: PERSISTENT_APP_U_I_VALUES.
                        "persistentAppUI": "ENABLED",
                        "s3MonitoringConfiguration": {"logUri": "s3://bucket/endpoint-logs"},
                    },
                },
                "sessionIdleTimeoutInMinutes": 15,
                "tags": {"only": "one"},
            }),
        ),
        (
            "a name at the ResourceNameString length bound and a session timeout",
            json!({
                // `ResourceNameString` is len 1..64: the upper bound.
                "name": "n".repeat(64),
                "clientToken": "t".repeat(64),
                "type": "JUPYTER_SERVER",
                "releaseLabel": "emr-6.10.0-latest",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
                "sessionIdleTimeoutInMinutes": 480,
            }),
        ),
    ]
}

/// RULES 1 and 6: a created endpoint is read back out of state by both readers, with the
/// members the request carried and without the ones it omitted.
#[tokio::test]
async fn a_created_managed_endpoint_is_read_back_from_state() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-roundtrip", "vc-ep-roundtrip").await;

    for (label, body) in stored_bodies() {
        let created = create_endpoint(&emr, &cluster, body.clone()).await;
        let created = created.ok(label).clone();
        let id = text(&created, "/id", label);
        assert_resource_id(label, "the endpoint id", &id);
        assert_eq!(
            text(&created, "/name", label),
            body["name"].as_str().unwrap_or_default(),
            "case {label:?}: the create reported another name: {created}"
        );
        assert_eq!(
            text(&created, "/virtualClusterId", label),
            cluster,
            "case {label:?}: the create reported another virtual cluster: {created}"
        );
        // The `EndpointArn` is built from the cluster, the id and the region the caller
        // signed for, never a hardcoded one.
        let arn = text(&created, "/arn", label);
        assert!(
            arn.contains(&format!(":{REGION}:"))
                && arn.ends_with(&format!("/virtualclusters/{cluster}/endpoints/{id}")),
            "case {label:?}: arn {arn:?} is not this endpoint's EndpointArn in {REGION}"
        );

        let answer = describe_endpoint(&emr, &cluster, &id).await;
        let endpoint = described(answer.ok(label), label);
        for (pointer, expected) in [
            ("/id", json!(id)),
            ("/name", body["name"].clone()),
            ("/virtualClusterId", json!(cluster)),
            ("/arn", json!(arn)),
            ("/type", body["type"].clone()),
            ("/releaseLabel", body["releaseLabel"].clone()),
            ("/executionRoleArn", body["executionRoleArn"].clone()),
        ] {
            assert_eq!(
                field(&endpoint, pointer, label),
                &expected,
                "case {label:?}: {pointer} is not the value the create persisted: {endpoint}"
            );
        }
        // The reported state is one the model declares, and `createdAt` is the record's
        // own instant rather than a canned one.
        let reported_state = text(&endpoint, "/state", label);
        assert!(
            ENDPOINT_STATE_VALUES.contains(&reported_state.as_str()),
            "case {label:?}: state {reported_state:?} is not an EndpointState"
        );
        let created_at = instant_millis(&endpoint, "/createdAt", label);
        let now = state::now_millis();
        assert!(
            created_at <= now && created_at > now - 60_000,
            "case {label:?}: createdAt {created_at} is not the instant the endpoint was created (now {now})"
        );

        // The members the request omitted stay ABSENT; the ones it carried come back.
        for member in ["certificateArn", "configurationOverrides"] {
            let pointer = format!("/{member}");
            match body.get(member) {
                Some(submitted) => assert_eq!(
                    field(&endpoint, &pointer, label),
                    submitted,
                    "case {label:?}: {member} is not what the request carried: {endpoint}"
                ),
                None => assert!(
                    endpoint.get(member).is_none(),
                    "case {label:?}: {member} was reported for a request that omitted it: {endpoint}"
                ),
            }
        }
        let expected_tags = body.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            field(&endpoint, "/tags", label),
            &expected_tags,
            "case {label:?}: the reported tags are not the stored ones: {endpoint}"
        );

        // RULE 1: the listed record IS the described record, so the two readers answer
        // from one copy of the endpoint.
        let listing = list_endpoints(&emr, &cluster, "").await;
        let listing = listing.ok(label).clone();
        let listed_record = listed(&listing, label)
            .into_iter()
            .find(|candidate| text(candidate, "/id", label) == id)
            .unwrap_or_else(|| panic!("case {label:?}: the list does not report {id}: {listing}"));
        assert_eq!(
            listed_record, endpoint,
            "case {label:?}: the listed endpoint differs from the described record"
        );
    }
}

/// RULE 2: the `CREATING` -> `ACTIVE` transition the clock owes is observable in both
/// phases, through both readers.
#[tokio::test]
async fn a_managed_endpoint_reaches_active_on_the_clock() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-clock", "vc-ep-clock").await;
    let case = "a fresh endpoint";

    let created = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("clock", "ep-tok-clock", "JUPYTER_ENTERPRISE_GATEWAY"),
    )
    .await;
    let id = text(created.ok(case), "/id", case);

    // The span is never zero: a caller reading straight back sees the transient state.
    for (reader, state_now) in [
        ("describe", {
            let answer = describe_endpoint(&emr, &cluster, &id).await;
            text(&described(answer.ok(case), case), "/state", case)
        }),
        ("list", {
            let answer = list_endpoints(&emr, &cluster, "").await;
            text(&listed(answer.ok(case), case)[0], "/state", case)
        }),
    ] {
        assert_eq!(
            state_now, state::ENDPOINT_CREATING,
            "case {case:?}: {reader} reported {state_now:?} before the transition came due"
        );
    }

    wait_for_the_clock();

    for (reader, state_then) in [
        ("describe", {
            let answer = describe_endpoint(&emr, &cluster, &id).await;
            text(&described(answer.ok(case), case), "/state", case)
        }),
        ("list", {
            let answer = list_endpoints(&emr, &cluster, "").await;
            text(&listed(answer.ok(case), case)[0], "/state", case)
        }),
    ] {
        assert_eq!(
            state_then, state::ENDPOINT_ACTIVE,
            "case {case:?}: {reader} reported {state_then:?} after the transition came due"
        );
    }
}

/// Create `count` endpoints of `endpoint_type` in `cluster`, returning their ids in
/// creation order.
async fn create_many(
    emr: &Emr,
    cluster: &str,
    endpoint_type: &str,
    prefix: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let label = format!("{prefix} {index}");
        let created = create_endpoint(
            emr,
            cluster,
            endpoint_body(
                &format!("{prefix}-{index}"),
                &format!("ep-tok-{prefix}-{index}"),
                endpoint_type,
            ),
        )
        .await;
        ids.push(text(created.ok(&label), "/id", &label));
    }
    ids
}

/// RULES 3 and 6: a list reports the named cluster's endpoints newest first, another
/// cluster's endpoints are neither listed nor describable under it, and listing twice
/// answers the same thing.
#[tokio::test]
async fn listing_managed_endpoints_is_scoped_to_one_virtual_cluster() {
    let emr = Emr::new();
    let first = emr.cluster("endpoint-scope-a", "vc-ep-scope-a").await;
    let second = emr.cluster("endpoint-scope-b", "vc-ep-scope-b").await;

    let in_first = create_many(&emr, &first, "JUPYTER_ENTERPRISE_GATEWAY", "scope-a", 4).await;
    let in_second = create_many(&emr, &second, "JUPYTER_SERVER", "scope-b", 2).await;

    // Rows differ in which cluster is listed and how many endpoints it holds — including
    // a cluster holding none.
    let third = emr.cluster("endpoint-scope-c", "vc-ep-scope-c").await;
    for (label, cluster, created) in [
        ("the cluster with four endpoints", &first, &in_first),
        ("the cluster with two endpoints", &second, &in_second),
        ("a cluster with no endpoint", &third, &Vec::new()),
    ] {
        let answer = list_endpoints(&emr, cluster, "").await;
        let body = answer.ok(label).clone();
        let mut newest_first = created.clone();
        newest_first.reverse();
        assert_eq!(
            listed_ids(&body, label),
            newest_first,
            "case {label:?}: the list must report exactly this cluster's endpoints, newest first: {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {label:?}: an unpaged list that reported everything must carry no nextToken: {body}"
        );

        // RULE 6: the list changed nothing a later reader sees.
        let again = list_endpoints(&emr, cluster, "").await;
        assert_eq!(
            again.ok(label),
            &body,
            "case {label:?}: listing twice answered two different sets"
        );
    }

    // An endpoint id under the WRONG cluster is not found, not the other cluster's
    // endpoint.
    for (label, cluster, id) in [
        ("the second cluster's endpoint under the first", &first, &in_second[0]),
        ("the first cluster's endpoint under the second", &second, &in_first[0]),
        ("an endpoint under a cluster holding none", &third, &in_first[1]),
    ] {
        describe_endpoint(&emr, cluster, id)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }
}

/// RULE 4: `maxResults` + `nextToken` walk one cluster's endpoints with no repeats and no
/// gaps, and a `nextToken` is reported only while endpoints remain.
#[tokio::test]
async fn listing_managed_endpoints_pages_the_stored_set() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-paging", "vc-ep-paging").await;
    let created = create_many(&emr, &cluster, "JUPYTER_ENTERPRISE_GATEWAY", "page", 5).await;
    let stored: BTreeSet<String> = created.iter().cloned().collect();
    assert_eq!(
        stored.len(),
        created.len(),
        "every create must have minted its own endpoint, got {created:?}"
    );

    // Rows differ in the page size: one item at a time, divisors and non-divisors of the
    // stored count, exactly the stored count, and one past it.
    for page_size in [1usize, 2, 3, created.len(), created.len() + 2] {
        let label = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let answer = list_endpoints(&emr, &cluster, &query).await;
            let body = answer.ok(&label).clone();
            let page = listed_ids(&body, &label);
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page carried {} endpoints, more than maxResults",
                page.len()
            );
            walked.extend(page);
            match body.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        walked.len() < created.len(),
                        "case {label:?}: a nextToken was reported after all {} endpoints",
                        created.len()
                    );
                    query = format!("?maxResults={page_size}&nextToken={token}");
                }
                None => break,
            }
        }

        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {label:?}: paging repeated an endpoint, walked {walked:?}"
        );
        assert_eq!(
            unique.into_iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {label:?}: paging did not walk every stored endpoint, walked {walked:?}"
        );
    }
}

/// RULE 5: `states` and `types` keep the endpoints a request names, and an empty filter
/// constrains nothing.
#[tokio::test]
async fn listing_managed_endpoints_honours_the_state_and_type_filters() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-filters", "vc-ep-filters").await;

    // Rows differ along the `EndpointType` value, which the model leaves free-form
    // (smithy_query: shape `EndpointType` — len 1..64, pattern \S), and along how many
    // endpoints share one type.
    let gateways = create_many(&emr, &cluster, "JUPYTER_ENTERPRISE_GATEWAY", "filter-jeg", 3).await;
    let servers = create_many(&emr, &cluster, "JUPYTER_SERVER", "filter-js", 2).await;
    let custom = create_many(&emr, &cluster, "CUSTOM.endpoint-type_1", "filter-custom", 1).await;

    let everything: BTreeSet<String> = gateways
        .iter()
        .chain(servers.iter())
        .chain(custom.iter())
        .cloned()
        .collect();
    let gateway_set: BTreeSet<String> = gateways.iter().cloned().collect();
    let server_set: BTreeSet<String> = servers.iter().cloned().collect();
    let custom_set: BTreeSet<String> = custom.iter().cloned().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();
    let mut gateways_and_custom = gateway_set.clone();
    gateways_and_custom.extend(custom_set.iter().cloned());

    // Every endpoint is CREATING until the clock settles it, so the state rows are
    // asserted in BOTH phases: first against CREATING, then against ACTIVE.
    // An EMPTY `states`/`types` list (the model allows len 0) cannot be expressed on the
    // query string: `?states=` is one member whose value is the empty string, which the
    // enum and `EndpointType`'s `\S` pattern both refuse before any handler runs. So the
    // "a filter the request omits constrains nothing" row is the no-filter one.
    let creating_cases: Vec<(&str, String, &BTreeSet<String>)> = vec![
        ("no filter", String::new(), &everything),
        (
            "states naming the state every endpoint is in",
            format!("?states={}", state::ENDPOINT_CREATING),
            &everything,
        ),
        (
            "states naming a state no endpoint is in",
            format!("?states={}", state::ENDPOINT_TERMINATED),
            &nothing,
        ),
        (
            "states naming two states, one of them the current one",
            format!(
                "?states={}&states={}",
                state::ENDPOINT_TERMINATING,
                state::ENDPOINT_CREATING
            ),
            &everything,
        ),
        (
            "one type",
            "?types=JUPYTER_ENTERPRISE_GATEWAY".to_string(),
            &gateway_set,
        ),
        ("another type", "?types=JUPYTER_SERVER".to_string(), &server_set),
        (
            "a type only one endpoint carries",
            "?types=CUSTOM.endpoint-type_1".to_string(),
            &custom_set,
        ),
        (
            "two types",
            "?types=JUPYTER_ENTERPRISE_GATEWAY&types=CUSTOM.endpoint-type_1".to_string(),
            &gateways_and_custom,
        ),
        (
            "a type no endpoint carries",
            "?types=NOT_AN_ENDPOINT_TYPE".to_string(),
            &nothing,
        ),
        (
            "a type and the current state together",
            format!(
                "?types=JUPYTER_SERVER&states={}",
                state::ENDPOINT_CREATING
            ),
            &server_set,
        ),
        (
            "a type and a state nothing is in together",
            format!(
                "?types=JUPYTER_SERVER&states={}",
                state::ENDPOINT_ACTIVE
            ),
            &nothing,
        ),
    ];
    for (label, query, expected) in &creating_cases {
        let answer = list_endpoints(&emr, &cluster, query).await;
        let body = answer.ok(label).clone();
        let reported: BTreeSet<String> = listed_ids(&body, label).into_iter().collect();
        assert_eq!(
            &reported, *expected,
            "case {label:?}: {query} reported the wrong endpoints: {body}"
        );
    }

    wait_for_the_clock();

    // The same rule once the clock has moved every endpoint to ACTIVE: the filter follows
    // the stored state, it does not remember the state at creation.
    for (label, query, expected) in [
        (
            "states naming the settled state",
            format!("?states={}", state::ENDPOINT_ACTIVE),
            &everything,
        ),
        (
            "states naming the state the endpoints have left",
            format!("?states={}", state::ENDPOINT_CREATING),
            &nothing,
        ),
        (
            "the settled state and one type together",
            format!("?states={}&types=JUPYTER_SERVER", state::ENDPOINT_ACTIVE),
            &server_set,
        ),
    ] {
        let answer = list_endpoints(&emr, &cluster, &query).await;
        let body = answer.ok(label).clone();
        let reported: BTreeSet<String> = listed_ids(&body, label).into_iter().collect();
        assert_eq!(
            &reported, expected,
            "case {label:?}: {query} reported the wrong endpoints: {body}"
        );
    }
}

/// RULE 5 for the instant bounds: `createdAfter` and `createdBefore` bound the stored
/// `createdAt` strictly, so a bound keeps all, some or none of the cluster's endpoints.
///
/// The fixture creates one group, lets the clock move past it, takes a MIDPOINT, lets the
/// clock move again and creates a second group, so a bound that falls between the two
/// groups has endpoints on either side of it.
#[tokio::test]
async fn listing_managed_endpoints_honours_the_created_at_bounds() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-bounds", "vc-ep-bounds").await;
    // Wider than the one-second resolution an ISO-8601 instant can be rendered at, so a
    // bound taken between the two groups stays between them however the wire renders it.
    let gap = Duration::from_millis(1_100);

    let before_everything = state::now_millis() - 2_000;
    let early = create_many(&emr, &cluster, "JUPYTER_ENTERPRISE_GATEWAY", "bound-early", 3).await;
    tokio::time::sleep(gap).await;
    let midpoint = state::now_millis();
    tokio::time::sleep(gap).await;
    let late = create_many(&emr, &cluster, "JUPYTER_SERVER", "bound-late", 2).await;
    let after_everything = state::now_millis() + 2_000;

    let everything: BTreeSet<String> = early.iter().chain(late.iter()).cloned().collect();
    let early_set: BTreeSet<String> = early.iter().cloned().collect();
    let late_set: BTreeSet<String> = late.iter().cloned().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();

    // Rows differ along which bound is applied and whether it keeps all, some or none.
    let cases: Vec<(&str, String, &BTreeSet<String>)> = vec![
        ("no bound", String::new(), &everything),
        (
            "createdAfter before every endpoint",
            format!("?createdAfter={}", query_instant(before_everything)),
            &everything,
        ),
        (
            "createdAfter after every endpoint",
            format!("?createdAfter={}", query_instant(after_everything)),
            &nothing,
        ),
        (
            "createdAfter at the midpoint",
            format!("?createdAfter={}", query_instant(midpoint)),
            &late_set,
        ),
        (
            "createdBefore after every endpoint",
            format!("?createdBefore={}", query_instant(after_everything)),
            &everything,
        ),
        (
            "createdBefore before every endpoint",
            format!("?createdBefore={}", query_instant(before_everything)),
            &nothing,
        ),
        (
            "createdBefore at the midpoint",
            format!("?createdBefore={}", query_instant(midpoint)),
            &early_set,
        ),
        (
            "a window around every endpoint",
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(before_everything),
                query_instant(after_everything)
            ),
            &everything,
        ),
        (
            "a window holding only the later group",
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(midpoint),
                query_instant(after_everything)
            ),
            &late_set,
        ),
        (
            "a window holding only the earlier group",
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(before_everything),
                query_instant(midpoint)
            ),
            &early_set,
        ),
        (
            "an empty window",
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(after_everything),
                query_instant(before_everything)
            ),
            &nothing,
        ),
    ];

    for (label, query, expected) in &cases {
        let answer = list_endpoints(&emr, &cluster, query).await;
        let body = answer.ok(label).clone();
        let reported: BTreeSet<String> = listed_ids(&body, label).into_iter().collect();
        assert_eq!(
            &reported, *expected,
            "case {label:?}: {query} reported the wrong endpoints: {body}"
        );
        // Whatever a bound kept really is inside it: the reported `createdAt` values are
        // the records' own instants, not the bounds.
        for endpoint in listed(&body, label) {
            let created_at = instant_millis(&endpoint, "/createdAt", label);
            assert!(
                created_at >= before_everything && created_at <= after_everything,
                "case {label:?}: createdAt {created_at} is outside [{before_everything}, {after_everything}]"
            );
        }
    }
}

/// RULE 7: a replayed `clientToken` names the endpoint the first call minted, so the
/// listing still holds exactly one; a different token with the same parameters mints
/// another.
#[tokio::test]
async fn a_replayed_client_token_mints_no_second_managed_endpoint() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-idempotency", "vc-ep-idem").await;
    let other = emr.cluster("endpoint-idempotency-2", "vc-ep-idem-2").await;
    let case = "the first create";

    let body = endpoint_body("idem", "ep-tok-idem", "JUPYTER_ENTERPRISE_GATEWAY");
    let first = create_endpoint(&emr, &cluster, body.clone()).await;
    let first_id = text(first.ok(case), "/id", case);

    // Rows: the same token replayed, then a different token with the same parameters, then
    // the same token in ANOTHER cluster — a token is scoped to the cluster it created in.
    let replayed = create_endpoint(&emr, &cluster, body.clone()).await;
    assert_eq!(
        text(replayed.ok("the replay"), "/id", "the replay"),
        first_id,
        "a replayed clientToken must name the endpoint the first call minted"
    );

    let fresh_token = create_endpoint(
        &emr,
        &cluster,
        endpoint_body("idem", "ep-tok-idem-2", "JUPYTER_ENTERPRISE_GATEWAY"),
    )
    .await;
    let second_id = text(fresh_token.ok("a fresh token"), "/id", "a fresh token");
    assert_ne!(
        second_id, first_id,
        "a fresh clientToken must mint its own endpoint"
    );

    let in_other_cluster = create_endpoint(&emr, &other, body).await;
    let other_id = text(
        in_other_cluster.ok("the same token in another cluster"),
        "/id",
        "the same token in another cluster",
    );
    assert_ne!(
        other_id, first_id,
        "a clientToken is scoped to one virtual cluster, so it must mint an endpoint there"
    );

    let listing = list_endpoints(&emr, &cluster, "").await;
    let body = listing.ok("after the replays").clone();
    assert_eq!(
        listed_ids(&body, "after the replays")
            .into_iter()
            .collect::<BTreeSet<String>>(),
        [first_id, second_id].into_iter().collect::<BTreeSet<String>>(),
        "the replay must not have stored a second endpoint: {body}"
    );
}

/// RULE 8: a request the operation cannot answer is refused with the modelled error and
/// changes nothing.
#[tokio::test]
async fn unanswerable_managed_endpoint_requests_are_refused() {
    let emr = Emr::new();
    let cluster = emr.cluster("endpoint-refusals", "vc-ep-refusals").await;
    let other = emr.cluster("endpoint-refusals-2", "vc-ep-refusals-2").await;
    let stored = create_many(&emr, &cluster, "JUPYTER_ENTERPRISE_GATEWAY", "refuse", 2).await;

    // A cluster this store does not hold SCOPES the listing to no endpoint rather than
    // making the request an error, so it answers an empty page — the rule
    // `crate::cluster_scoped_listing_scope` holds over both cluster-scoped listings.
    let unknown = list_endpoints(&emr, "nosuchcluster", "").await;
    assert_eq!(
        listed(unknown.ok("listing an unknown virtual cluster"), "listing an unknown virtual cluster"),
        Vec::<Value>::new(),
        "case {:?}: expected an empty page, got {}",
        "listing an unknown virtual cluster",
        unknown.body
    );
    // That same cluster cannot host an endpoint — and THAT operation models the not-found
    // error, so it uses it.
    create_endpoint(
        &emr,
        "nosuchcluster",
        endpoint_body("orphan", "ep-tok-orphan", "JUPYTER_ENTERPRISE_GATEWAY"),
    )
    .await
    .refused(
        "creating an endpoint in an unknown virtual cluster",
        400,
        "ResourceNotFoundException",
    );
    describe_endpoint(&emr, &cluster, "nosuchendpoint")
        .await
        .refused("describing an unknown endpoint", 400, "ResourceNotFoundException");

    // A page of fewer than one endpoint is not a page, and a `nextToken` this listing
    // never issued is not a position in its order. The foreign cursors come from the one
    // encoder every paginated list here uses, so these rows cannot drift from the format.
    let job_run_cursor =
        state::encode_list_cursor(&state::list_job_runs_scope(REGION, &cluster), 1, 2);
    let other_cluster_cursor = state::encode_list_cursor(
        &state::list_managed_endpoints_scope(REGION, &other),
        1,
        2,
    );
    for (label, query) in [
        ("maxResults below one", "?maxResults=0".to_string()),
        ("a negative maxResults", "?maxResults=-3".to_string()),
        ("a forged nextToken", "?nextToken=not-a-cursor".to_string()),
        ("a nextToken with one component", "?nextToken=12345".to_string()),
        ("a nextToken carrying no scope", "?nextToken=1:2".to_string()),
        (
            "a nextToken whose parts are not numbers",
            "?nextToken=endpoints:a:b".to_string(),
        ),
        (
            "the cursor of this cluster's job-run listing",
            format!("?nextToken={job_run_cursor}"),
        ),
        (
            "the cursor of another cluster's endpoint listing",
            format!("?nextToken={other_cluster_cursor}"),
        ),
    ] {
        list_endpoints(&emr, &cluster, &query)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals changed nothing: the cluster still lists exactly what it held.
    let after = list_endpoints(&emr, &cluster, "").await;
    let body = after.ok("after the refusals").clone();
    assert_eq!(
        listed_ids(&body, "after the refusals")
            .into_iter()
            .collect::<BTreeSet<String>>(),
        stored.iter().cloned().collect::<BTreeSet<String>>(),
        "a refused request must leave the stored set untouched: {body}"
    );
}
