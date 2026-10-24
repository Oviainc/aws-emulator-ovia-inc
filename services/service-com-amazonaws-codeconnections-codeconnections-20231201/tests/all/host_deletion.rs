// SPDX-License-Identifier: CC0-1.0

//! `DeleteHost` over the wire: the host a delete removed is gone from every reader, and
//! a delete the documented preconditions forbid removes nothing.
//!
//! THE RULE these tests assert, over tables rather than over one recorded request: a
//! delete of a resource this service addresses by ARN removes exactly that stored row —
//! so every later reader reports it ABSENT and its ARN is never resolved again — and
//! otherwise refuses, with the modeled `ResourceNotFoundException` (404) when the ARN
//! resolves to no such resource and the modeled `ResourceUnavailableException` (400) when
//! the resource IS there but a documented precondition of its deletion does not hold
//! (`DeleteHost`: "Before you delete a host, all connections associated to the host must
//! be deleted"; "A host cannot be deleted if it is in the VPC_CONFIG_INITIALIZING or
//! VPC_CONFIG_DELETING state"). A refused delete persists nothing.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing
//! `connection_lifecycle`'s and `host_association`'s helpers, so these tests exercise the
//! same path a signed SDK request takes. The recorded unit the issue cited — a host
//! deleted and then reported not found — is ONE row of the first table below.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeconnections_codeconnections_20231201::ComAmazonawsCodeconnectionsCodeconnections20231201State as CodeConnectionsState;

use super::connection_lifecycle::{REGION, call, create, router, router_with_engine};
use super::git_sync::error_code;
use super::host_association::{create_host, create_plain_host, host_cases, plain_host};

/// The sentence this service answers for an ARN that resolves to no host, written out as
/// the LITERAL its host readers return rather than read off the implementation's own
/// constant — the same literal `refusal_messages.rs` pins.
fn host_not_found_message(host_arn: &str) -> String {
    format!("No host exists with the ARN: {host_arn}")
}

/// Every value the `Host` shape's `Status` member documents, from
/// `smithy_query(query="doc", shape="Host", member="Status")`: "The status of the host,
/// such as PENDING, AVAILABLE, VPC_CONFIG_DELETING, VPC_CONFIG_INITIALIZING, and
/// VPC_CONFIG_FAILED_INITIALIZATION."
const HOST_STATUS_VALUES: &[&str] = &[
    "PENDING",
    "AVAILABLE",
    "VPC_CONFIG_DELETING",
    "VPC_CONFIG_INITIALIZING",
    "VPC_CONFIG_FAILED_INITIALIZATION",
];

/// The statuses `DeleteHost`'s own documentation forbids deleting from: "A host cannot be
/// deleted if it is in the VPC_CONFIG_INITIALIZING or VPC_CONFIG_DELETING state."
const STATUSES_BLOCKING_DELETE: &[&str] = &["VPC_CONFIG_INITIALIZING", "VPC_CONFIG_DELETING"];

/// The message a refused response carries.
fn refusal_message(case: &str, body: &serde_json::Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message, got {body}"))
        .to_string()
}

/// Delete one host and return the status and body.
async fn delete_host(
    router: &MultiServiceRouter,
    host_arn: &str,
) -> (u16, serde_json::Value) {
    call(router, "DeleteHost", json!({ "HostArn": host_arn })).await
}

/// Assert a `DeleteHost` succeeded: HTTP 200 and the modeled EMPTY output, which
/// awsJson1_0 renders as an empty body or an empty object.
fn assert_deleted(case: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DeleteHost, got {status} {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: DeleteHost models no output members, got {body}"
    );
}

/// Assert a refusal is the modeled absence answer, worded as this service's host readers
/// word it.
fn assert_absent(case: &str, status: u16, body: &serde_json::Value, host_arn: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for a host ARN that resolves to nothing, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        refusal_message(case, body),
        host_not_found_message(host_arn),
        "case {case:?}: expected the host-absence sentence, got {body}"
    );
}

/// Assert a refusal is the modeled "the host is there but is not usable" answer.
fn assert_unavailable(case: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a precondition the delete breaks, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceUnavailableException",
        "case {case:?}: expected ResourceUnavailableException, got {body}"
    );
}

/// The `HostArn`s `ListHosts` reports, walking every page.
async fn listed_host_arns(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut arns = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let (status, page) = call(router, "ListHosts", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListHosts, got {status} {page}"
        );
        for host in page["Hosts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Hosts must be a list, got {page}"))
        {
            arns.push(
                host["HostArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {page}"))
                    .to_string(),
            );
        }
        match page["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    arns
}

/// A host a delete removed is gone from every reader of it, its ARN resolves to nothing
/// for any operation that names one, and deleting it again is the same absence. Rows are
/// the host table `host_association.rs` spans (provider type, endpoint form and length,
/// VPC configuration present or not and at each published list bound, tag count), so what
/// the delete removes varies along everything a host varies along.
#[tokio::test]
async fn a_deleted_host_is_reported_absent_by_every_reader() {
    let router = router();

    // Every row is created first, so each delete happens with siblings in the store: a
    // delete that removed more than its own row would show up as a missing sibling.
    let cases = host_cases();
    let mut remaining: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        remaining.push((case.label, create_host(&router, case).await));
    }

    while let Some((label, arn)) = remaining.first().cloned() {
        remaining.remove(0);

        let (status, body) = delete_host(&router, &arn).await;
        assert_deleted(label, status, &body);

        // GetHost: the single-resource reader reports the host as absent.
        let (status, body) = call(&router, "GetHost", json!({ "HostArn": arn })).await;
        assert_absent(label, status, &body, &arn);

        // ListHosts: the report holds every sibling and not the deleted row.
        let listed = listed_host_arns(&router, label).await;
        assert!(
            !listed.contains(&arn),
            "case {label:?}: a deleted host is listed by no report, got {listed:?}"
        );
        let mut expected: Vec<String> =
            remaining.iter().map(|(_, arn)| arn.clone()).collect();
        expected.sort();
        let mut got = listed.clone();
        got.sort();
        assert_eq!(
            got, expected,
            "case {label:?}: ListHosts must report exactly the hosts still there, got {listed:?}"
        );

        // The ARN-addressed tagging surface stops resolving it too: the tags went with
        // the row.
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            json!({ "ResourceArn": arn }),
        )
        .await;
        assert_absent(label, status, &body, &arn);

        // A host a connection could be created against: the reference no longer resolves.
        let (status, body) = call(
            &router,
            "CreateConnection",
            json!({
                "ConnectionName": format!("on-deleted-{}", listed.len()),
                "ProviderType": "GitHubEnterpriseServer",
                "HostArn": arn,
            }),
        )
        .await;
        assert_absent(label, status, &body, &arn);

        // And the delete is not idempotent for a host addressed by ARN: a second delete
        // is the same absence the readers report.
        let (status, body) = delete_host(&router, &arn).await;
        assert_absent(label, status, &body, &arn);
    }

    let listed = listed_host_arns(&router, "all deleted").await;
    assert!(
        listed.is_empty(),
        "after deleting every host the report is empty, got {listed:?}"
    );
}

/// A delete of a host-shaped ARN this account holds no host for is the same absence, and
/// it removes nothing. Rows are values the modeled `HostArn` pattern admits
/// (`^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$`)
/// that this store does not hold.
#[tokio::test]
async fn deleting_a_host_that_is_not_there_refuses_and_removes_nothing() {
    let router = router();
    let account = engine::emulator_account_id();

    // A host that IS there, so "unresolvable" cannot mean "the store is empty".
    let live = create_plain_host(&router, "live", "GitHubEnterpriseServer").await;
    let live_id = live
        .rsplit_once("host/")
        .expect("a host ARN carries a host/ resource")
        .1
        .to_string();

    let unresolvable = [
        (
            "an id this store never minted",
            format!(
                "arn:aws:codeconnections:{REGION}:{account}:host/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"
            ),
        ),
        (
            "the codestar-connections namespace the pattern also admits",
            format!("arn:aws:codestar-connections:{REGION}:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in another region",
            format!("arn:aws:codeconnections:us-east-1:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in the aws-cn partition",
            format!("arn:aws-cn:codeconnections:cn-north-1:{account}:host/{live_id}"),
        ),
        (
            "the live host's id in another account",
            format!("arn:aws:codeconnections:{REGION}:210987654321:host/{live_id}"),
        ),
        (
            "a resource id that is not a host id at all",
            format!("arn:aws:codeconnections:{REGION}:{account}:host/x"),
        ),
    ];

    for (label, host_arn) in &unresolvable {
        let (status, body) = delete_host(&router, host_arn).await;
        assert_absent(label, status, &body, host_arn);

        // The live host is untouched by every refused delete.
        let listed = listed_host_arns(&router, label).await;
        assert_eq!(
            listed,
            vec![live.clone()],
            "case {label:?}: a refused delete removes nothing, got {listed:?}"
        );
    }

    let (status, body) = delete_host(&router, &live).await;
    assert_deleted("the live host", status, &body);
}

/// A host a connection is still associated with is refused — `DeleteHost`: "Before you
/// delete a host, all connections associated to the host must be deleted" — and the host
/// stays exactly as it was; once the last associated connection is deleted the host goes.
/// Rows differ in how many connections are associated and in provider type.
#[tokio::test]
async fn a_host_with_associated_connections_is_refused_until_they_are_gone() {
    let router = router();

    // (case, provider type, how many connections name the host)
    let rows: &[(&str, &str, usize)] = &[
        ("one connection", "GitHubEnterpriseServer", 1),
        ("two connections", "GitLabSelfManaged", 2),
        ("three connections", "Bitbucket", 3),
    ];

    // Noise the rule must ignore: a connection with no host at all, and a host nothing
    // is associated with, which must stay deletable throughout.
    let hostless = create(&router, "no-host", "GitHub").await;
    let unassociated = create_plain_host(&router, "unassociated", "GitHub").await;

    for (label, provider, count) in rows {
        let host = create_host(&router, &plain_host(&format!("host-{label}"), "GitHub")).await;
        let mut connections: Vec<String> = Vec::new();
        for index in 0..*count {
            let (status, created) = call(
                &router,
                "CreateConnection",
                json!({
                    "ConnectionName": format!("conn-{label}-{index}"),
                    "ProviderType": provider,
                    "HostArn": host,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200 from CreateConnection, got {status} {created}"
            );
            connections.push(
                created["ConnectionArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: no ConnectionArn: {created}"))
                    .to_string(),
            );
        }

        // Every associated connection but the last one is deleted, and the host is
        // refused at each step: one connection left is enough to block it.
        for (index, connection) in connections.iter().enumerate() {
            let step = format!("{label}: {} still associated", connections.len() - index);
            let (status, body) = delete_host(&router, &host).await;
            assert_unavailable(&step, status, &body);
            assert!(
                refusal_message(&step, &body).contains(&host),
                "case {step:?}: the refusal names the host it refused, got {body}"
            );

            // Nothing was removed: the host is still reported, and so is every
            // connection on it.
            let (status, got) = call(&router, "GetHost", json!({ "HostArn": host })).await;
            assert_eq!(
                status, 200,
                "case {step:?}: a refused delete leaves the host in place, got {status} {got}"
            );
            let (status, listed) = call(
                &router,
                "ListConnections",
                json!({ "HostArnFilter": host }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {step:?}: expected 200 from ListConnections, got {status} {listed}"
            );
            assert_eq!(
                listed["Connections"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {step:?}: Connections is not a list: {listed}"))
                    .len(),
                connections.len() - index,
                "case {step:?}: a refused delete removes no connection either, got {listed}"
            );

            let (status, body) = call(
                &router,
                "DeleteConnection",
                json!({ "ConnectionArn": connection }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {step:?}: expected 200 from DeleteConnection, got {status} {body}"
            );
        }

        // With the last association gone the host deletes, and is then absent.
        let (status, body) = delete_host(&router, &host).await;
        assert_deleted(label, status, &body);
        let (status, body) = call(&router, "GetHost", json!({ "HostArn": host })).await;
        assert_absent(label, status, &body, &host);

        // The connection that names no host was never a reason to refuse anything.
        let (status, got) = call(
            &router,
            "GetConnection",
            json!({ "ConnectionArn": hostless }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the hostless connection is untouched, got {status} {got}"
        );
    }

    // A host no connection ever named is deletable at the end, exactly as it was at the
    // start: the refusal is keyed to the host's own associations, not to the store
    // holding connections at all.
    let (status, body) = delete_host(&router, &unassociated).await;
    assert_deleted("a host nothing is associated with", status, &body);
}

/// A host in a status `DeleteHost` cannot delete from is refused and stays; a host in any
/// other documented status deletes. Rows are every value the `Host` shape's `Status`
/// member documents, including the two the operation names and the three it does not.
///
/// No implemented operation of this service moves a host out of `PENDING` yet
/// (`UpdateHost` is still a scaffold), so each row's status is seeded into the shared
/// engine's store — the same engine every handler of this router reads — the way
/// `absent_resource_answers.rs` seeds the sync state only the service records.
#[tokio::test]
async fn a_host_in_a_status_that_forbids_deletion_is_refused() {
    let (router, engine) = router_with_engine();

    for status_value in HOST_STATUS_VALUES {
        let host = create_plain_host(
            &router,
            &format!("host-{}", status_value.to_lowercase()),
            "GitHubEnterpriseServer",
        )
        .await;
        seed_host_status(&engine, &host, status_value);

        let (status, body) = delete_host(&router, &host).await;
        if STATUSES_BLOCKING_DELETE.contains(status_value) {
            assert_unavailable(status_value, status, &body);
            assert!(
                refusal_message(status_value, &body).contains(status_value),
                "case {status_value:?}: the refusal names the status it refused, got {body}"
            );

            // Nothing was removed: the host is still there, still in that status.
            let (status, got) = call(&router, "GetHost", json!({ "HostArn": host })).await;
            assert_eq!(
                status, 200,
                "case {status_value:?}: a refused delete leaves the host in place, got {status} {got}"
            );
            assert_eq!(
                got["Status"],
                json!(status_value),
                "case {status_value:?}: the host keeps its status, got {got}"
            );
        } else {
            assert_deleted(status_value, status, &body);
            let (status, body) = call(&router, "GetHost", json!({ "HostArn": host })).await;
            assert_absent(status_value, status, &body, &host);
        }
    }
}

/// A deleted host's ARN is never resolved again, even by a host created with the same
/// name: the id is minted from a counter the store never rewinds. Rows are repeated
/// create/delete cycles under ONE name, so a reused ARN would show up as a collision.
#[tokio::test]
async fn a_deleted_hosts_arn_is_never_reused() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    for cycle in 0..4 {
        let label = format!("cycle {cycle}");
        let host = create_plain_host(&router, "recycled-name", "GitHub").await;
        assert!(
            !seen.contains(&host),
            "case {label:?}: a deleted host's ARN must never come back, saw {host:?} twice"
        );
        seen.push(host.clone());

        let (status, body) = delete_host(&router, &host).await;
        assert_deleted(&label, status, &body);

        // Every earlier cycle's ARN is still absent, not just the one just deleted.
        for (index, earlier) in seen.iter().enumerate() {
            let (status, body) = call(&router, "GetHost", json!({ "HostArn": earlier })).await;
            assert_absent(&format!("{label}: cycle {index}'s ARN"), status, &body, earlier);
        }
    }
}

/// Put one stored host into `status`, in the engine every handler of the router reads:
/// the state only `UpdateHost` and the VPC provisioning the console drives would write.
fn seed_host_status(engine: &Arc<engine::InMemoryEngine>, host_arn: &str, status: &str) {
    let store = engine.state::<CodeConnectionsState>();
    let mut guard = store.lock();
    let host = guard
        .hosts
        .get_mut(host_arn)
        .unwrap_or_else(|| panic!("the seeded host {host_arn} must be stored"));
    host.status = status.to_string();
}
