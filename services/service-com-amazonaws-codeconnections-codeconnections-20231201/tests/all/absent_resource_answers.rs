// SPDX-License-Identifier: CC0-1.0

//! What this service answers when the thing a request names is NOT THERE.
//!
//! Two rules, each asserted over a table rather than over one recorded request.
//!
//! RULE 1 — an identifier that resolves to no resource this service holds is reported
//! with the modeled `ResourceNotFoundException` (HTTP 404), wherever that identifier is
//! named: as the addressed resource (`GetHost`, `GetConnection`), as a reference another
//! resource is created against (`CreateConnection`'s `HostArn`), or as a filter
//! (`ListConnections`' `HostArnFilter`). `GetHost` and `CreateConnection` also model
//! `ResourceUnavailableException` (HTTP 400), which is the "the resource IS there but is
//! not usable" signal and never the answer for absence — a client branching on 404
//! (create-if-missing, cleanup that tolerates a 404) must not see a 400 for a resource
//! that simply does not exist.
//!
//! RULE 2 — `GetResourceSyncStatus` REPORTS the sync state this service has recorded
//! for the `(ResourceName, SyncType)` pair: a pair it has recorded none for succeeds
//! (HTTP 200) with none of `DesiredState`, `LatestSync`, `LatestSuccessfulSync`
//! populated, whether or not the resource has a sync configuration, and a pair that has
//! recorded state reports exactly that state. Only the service records a sync attempt
//! (nothing in this emulator fetches a remote repository's commits), so the rows a row
//! of this table needs are seeded into the shared engine's store the way
//! `sync_blocker_summary.rs` seeds the blockers `GetSyncBlockerSummary` reports.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a
//! signed SDK request takes.

use std::sync::Arc;

use service_com_amazonaws_codeconnections_codeconnections_20231201::ComAmazonawsCodeconnectionsCodeconnections20231201State as CodeConnectionsState;
use service_com_amazonaws_codeconnections_codeconnections_20231201::ops::types::{
    RESOURCE_SYNC_STATUS_VALUES, ResourceSyncAttempt, ResourceSyncEvent, Revision,
    SYNC_CONFIGURATION_TYPE_VALUES,
};
use service_com_amazonaws_codeconnections_codeconnections_20231201::state::{
    StoredResourceSyncState, sync_configuration_key,
};

use super::connection_lifecycle::{REGION, call, create, router, router_with_engine};
use super::git_sync::error_code;
use super::host_association::create_plain_host;

/// The only value the modeled `SyncConfigurationType` enum declares, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")` (also carried by the
/// crate's `SYNC_CONFIGURATION_TYPE_VALUES`).
const SYNC_TYPE: &str = "CFN_STACK_SYNC";

/// The published bounds of the modeled `ResourceName` member (`length 1..100`,
/// `pattern ^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`), from
/// `smithy_query(query="operation", operation="GetResourceSyncStatus")`.
const RESOURCE_NAME_MIN_LEN: usize = 1;
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// The members a `GetResourceSyncStatus` response can carry.
const SYNC_STATE_MEMBERS: &[&str] = &["DesiredState", "LatestSync", "LatestSuccessfulSync"];

/// A fixed instant the seeded sync attempts are placed around, so every expected
/// timestamp is an exact number rather than "about now".
const BASE_EPOCH_SECS: i64 = 1_700_000_000;

/// Host ARNs the modeled `HostArn` pattern admits —
/// `^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$` —
/// that name no host this store holds. `live_id` is the id of a host that IS there, so
/// a row can differ from a real host in exactly one ARN field.
fn unresolvable_host_arns(live_id: &str) -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        (
            "a host id this store never minted",
            format!(
                "arn:aws:codeconnections:{REGION}:{account}:host/2b7d1c0a-9e8f-4a3b-8c5d-6e7f8a9b0c1d"
            ),
        ),
        (
            "the codestar-connections namespace the pattern also admits",
            format!("arn:aws:codestar-connections:{REGION}:{account}:host/{live_id}"),
        ),
        (
            "a live host's id in another region",
            format!("arn:aws:codeconnections:ap-southeast-2:{account}:host/{live_id}"),
        ),
        (
            "a live host's id in the aws-us-gov partition",
            format!("arn:aws-us-gov:codeconnections:us-gov-west-1:{account}:host/{live_id}"),
        ),
        (
            "a live host's id in another account",
            format!("arn:aws:codeconnections:{REGION}:314159265358:host/{live_id}"),
        ),
        (
            "a resource id that is not a host id at all",
            format!("arn:aws:codeconnections:{REGION}:{account}:host/-"),
        ),
    ]
}

/// Assert one answer is the modeled `ResourceNotFoundException` at HTTP 404, carrying a
/// message that names the identifier that resolved to nothing.
fn assert_not_found(case: &str, needle: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for an absent resource, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains(needle),
        "case {case:?}: the refusal must name {needle:?}, got {body}"
    );
}

/// RULE 1, over every form of unresolvable host ARN and every place a host ARN is
/// named: `GetHost` (the addressed resource), `CreateConnection` (a reference) and
/// `ListConnections` (a filter) all answer 404 `ResourceNotFoundException`, and never
/// the 400 `ResourceUnavailableException` that belongs to a host which exists but is
/// unusable.
#[tokio::test]
async fn a_host_arn_that_resolves_to_nothing_is_reported_as_not_found() {
    let router = router();

    // A host that IS there, so "resolves to nothing" can never mean "the store is
    // empty", and a connection on it, so the refusals below are not a blanket refusal.
    let live_host = create_plain_host(&router, "live-host", "GitHubEnterpriseServer").await;
    let live_id = live_host
        .rsplit_once("host/")
        .unwrap_or_else(|| panic!("CreateHost returned no host ARN: {live_host:?}"))
        .1
        .to_string();
    let (status, body) = call(
        &router,
        "CreateConnection",
        serde_json::json!({
            "ConnectionName": "on-the-live-host",
            "ProviderType": "GitHubEnterpriseServer",
            "HostArn": live_host,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live host must resolve, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "GetHost",
        serde_json::json!({ "HostArn": live_host }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetHost on the live host must answer 200, got {status} {body}"
    );

    for (index, (label, host_arn)) in unresolvable_host_arns(&live_id).iter().enumerate() {
        let (status, body) = call(
            &router,
            "GetHost",
            serde_json::json!({ "HostArn": host_arn }),
        )
        .await;
        assert_not_found(&format!("GetHost: {label}"), host_arn, status, &body);
        assert!(
            body["Name"].is_null(),
            "case {label:?}: a refused GetHost reports no host, got {body}"
        );

        let (status, body) = call(
            &router,
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": format!("refused-{index}"),
                "ProviderType": "GitHubEnterpriseServer",
                "HostArn": host_arn,
            }),
        )
        .await;
        assert_not_found(&format!("CreateConnection: {label}"), host_arn, status, &body);
        assert!(
            body["ConnectionArn"].is_null(),
            "case {label:?}: a refused CreateConnection mints nothing, got {body}"
        );

        let (status, body) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "HostArnFilter": host_arn }),
        )
        .await;
        assert_not_found(&format!("ListConnections: {label}"), host_arn, status, &body);
    }

    // Every refused create persisted nothing: only the one accepted connection is there.
    let (status, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "ListConnections: expected 200, got {status} {listed}"
    );
    let names: Vec<&str> = listed["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("Connections is not a list: {listed}"))
        .iter()
        .map(|connection| connection["ConnectionName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["on-the-live-host"],
        "a refused create must persist nothing, got {names:?}"
    );
}

/// RULE 1 for a connection ARN too: the same absence, reported the same way by the
/// operation that addresses a connection. Rows are ARNs the modeled `ConnectionArn`
/// pattern `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$` admits.
#[tokio::test]
async fn a_connection_arn_that_resolves_to_nothing_is_reported_as_not_found() {
    let router = router();
    let live = create(&router, "live-connection", "GitHub").await;
    let account = engine::emulator_account_id();

    let rows = [
        (
            "a connection id this store never minted",
            format!("arn:aws:codeconnections:{REGION}:{account}:connection/00000000-1111-2222-3333-444444444444"),
        ),
        (
            "a live connection's id in another region",
            live.replace(REGION, "ca-central-1"),
        ),
        (
            "a live connection's id in another account",
            live.replace(account, "314159265358"),
        ),
    ];

    for (label, connection_arn) in &rows {
        let (status, body) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_not_found(label, connection_arn, status, &body);
        assert!(
            body["Connection"].is_null(),
            "case {label:?}: a refused GetConnection reports no connection, got {body}"
        );
    }

    // And the live connection still reads back, so the rows above are absence and not a
    // blanket refusal.
    let (status, body) = call(
        &router,
        "GetConnection",
        serde_json::json!({ "ConnectionArn": live }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetConnection on the live connection must answer 200, got {status} {body}"
    );
}

/// Resource names a `GetResourceSyncStatus` row asks about: they differ in length (each
/// published bound of `ResourceName` 1..100) and in form (mixed case, digits, and the
/// `_` and `-` the published pattern admits after the first character) — the dimensions
/// RULE 2 generalises over. None of them is the name the issue's recording cited.
fn resource_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain mixed-case stack name", "OrdersStack".to_string()),
        (
            "a name at the published minimum length",
            "Z".repeat(RESOURCE_NAME_MIN_LEN),
        ),
        (
            "a name at the published maximum length",
            "q".repeat(RESOURCE_NAME_MAX_LEN),
        ),
        (
            "a name carrying the hyphen the pattern admits",
            "orders-stack-2".to_string(),
        ),
        (
            "a name carrying the underscore the pattern admits",
            "orders_stack_3".to_string(),
        ),
        ("a name of digits and mixed case", "Stack42Beta".to_string()),
    ]
}

/// Assert one `GetResourceSyncStatus` answer is the empty success: HTTP 200, no error
/// code, and not one of the sync-state members present.
fn assert_empty_sync_status(case: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetResourceSyncStatus, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "",
        "case {case:?}: expected no error code, got {body}"
    );
    for member in SYNC_STATE_MEMBERS {
        assert!(
            body[member].is_null(),
            "case {case:?}: no {member} may be reported for a sync nothing ran, got {body}"
        );
    }
    assert_eq!(
        body,
        &serde_json::json!({}),
        "case {case:?}: a pair with no recorded sync state reports an empty payload, got {body}"
    );
}

/// Create a sync configuration for `resource_name` over the wire, so the resource is
/// one the store really holds a configuration for.
async fn configure_sync(
    router: &smithy_server_runtime::MultiServiceRouter,
    index: usize,
    resource_name: &str,
) {
    let connection_arn = create(router, &format!("ars-conn-{index}"), "GitHub").await;
    let (status, body) = call(
        router,
        "CreateRepositoryLink",
        serde_json::json!({
            "ConnectionArn": connection_arn,
            "OwnerId": format!("owner-{index}"),
            "RepositoryName": format!("repo-{index}"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_name:?}: expected 200 from CreateRepositoryLink, got {status} {body}"
    );
    let link_id = body["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {resource_name:?}: CreateRepositoryLink returned no id: {body}")
        })
        .to_string();

    let (status, body) = call(
        router,
        "CreateSyncConfiguration",
        serde_json::json!({
            "Branch": "main",
            "ConfigFile": "stack.yaml",
            "RepositoryLinkId": link_id,
            "ResourceName": resource_name,
            "RoleArn": format!("arn:aws:iam::{}:role/git-sync", engine::emulator_account_id()),
            "SyncType": SYNC_TYPE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_name:?}: expected 200 from CreateSyncConfiguration, got {status} {body}"
    );
}

/// RULE 2, first half: a `(ResourceName, SyncType)` pair with no recorded sync state
/// succeeds with an empty payload — for a resource with no sync configuration at all
/// AND for one that is configured, because the report is of SYNC STATE and not of the
/// configuration. A polling client must be able to tell "never synced" from "bad
/// request", so neither case is a 400.
#[tokio::test]
async fn a_pair_with_no_recorded_sync_state_reports_an_empty_status() {
    let router = router();

    for (index, (label, resource_name)) in resource_names().iter().enumerate() {
        // Unconfigured first: nothing in the store mentions this resource.
        let (status, body) = call(
            &router,
            "GetResourceSyncStatus",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_empty_sync_status(&format!("unconfigured: {label}"), status, &body);

        // Then configured, which changes nothing about the SYNC STATE: still nothing ran.
        configure_sync(&router, index, resource_name).await;
        let (status, configured) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the configuration must really be there, got {status} {configured}"
        );

        let (status, body) = call(
            &router,
            "GetResourceSyncStatus",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_empty_sync_status(&format!("configured: {label}"), status, &body);

        // A report changes nothing a later caller sees: the same answer every time.
        let (again_status, again) = call(
            &router,
            "GetResourceSyncStatus",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_eq!(
            (again_status, &again),
            (status, &body),
            "case {label:?}: a repeated read must answer exactly as the first did, got {again}"
        );
    }
}

/// RULE 2, second half: the empty payload above is what the STORE holds, not a constant.
/// A sync state seeded against a pair — the only way one appears, since only the service
/// records an attempt — is reported member for member, and only for the pair it was
/// recorded against. Rows differ in which members are recorded (latest only; latest plus
/// a successful one; a desired state too), in status value and in event count.
#[tokio::test]
async fn a_recorded_sync_state_is_reported_member_for_member() {
    let (router, engine) = router_with_engine();

    // (case, resource name, status for the latest attempt, event count, whether a
    //  successful attempt and a desired state are recorded too)
    let rows = [
        ("an initiated attempt, no events", "InitiatedStack", "INITIATED", 0usize, false),
        ("an in-progress attempt with one event", "ProgressStack", "IN_PROGRESS", 1, false),
        ("a failed attempt with two events", "FailedStack", "FAILED", 2, false),
        (
            "a succeeded attempt, also the latest successful one, with a desired state",
            "SucceededStack",
            "SUCCEEDED",
            3,
            true,
        ),
    ];

    for (label, resource_name, status_value, events, with_successful) in rows {
        assert!(
            RESOURCE_SYNC_STATUS_VALUES.contains(&status_value),
            "case {label:?}: {status_value:?} is not a modeled ResourceSyncStatus"
        );
        assert!(
            SYNC_CONFIGURATION_TYPE_VALUES.contains(&SYNC_TYPE),
            "case {label:?}: {SYNC_TYPE:?} is not a modeled SyncConfigurationType"
        );

        let attempt = sync_attempt(resource_name, status_value, events);
        seed_sync_state(
            &engine,
            resource_name,
            StoredResourceSyncState {
                desired_state: with_successful.then(|| revision(resource_name, "desired")),
                latest_sync: Some(attempt.clone()),
                latest_successful_sync: with_successful.then(|| attempt.clone()),
            },
        );

        let (status, body) = call(
            &router,
            "GetResourceSyncStatus",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetResourceSyncStatus, got {status} {body}"
        );
        assert_eq!(
            body["LatestSync"],
            expected_attempt(resource_name, status_value, events),
            "case {label:?}: LatestSync must be the recorded attempt, got {body}"
        );
        if with_successful {
            assert_eq!(
                body["LatestSuccessfulSync"],
                expected_attempt(resource_name, status_value, events),
                "case {label:?}: LatestSuccessfulSync must be the recorded attempt, got {body}"
            );
            assert_eq!(
                body["DesiredState"],
                expected_revision(resource_name, "desired"),
                "case {label:?}: DesiredState must be the recorded revision, got {body}"
            );
        } else {
            assert!(
                body["LatestSuccessfulSync"].is_null() && body["DesiredState"].is_null(),
                "case {label:?}: only recorded members may be reported, got {body}"
            );
        }

        // The state belongs to the pair it was recorded against and to no other: a
        // neighbouring resource name still reports the empty status.
        let (status, other) = call(
            &router,
            "GetResourceSyncStatus",
            serde_json::json!({
                "SyncType": SYNC_TYPE,
                "ResourceName": format!("{resource_name}Neighbour"),
            }),
        )
        .await;
        assert_empty_sync_status(&format!("neighbour of {label}"), status, &other);
    }
}

/// Seed one pair's sync state into the engine every handler of the router reads — the
/// way the service would record it once a Git sync had run.
fn seed_sync_state(
    engine: &Arc<engine::InMemoryEngine>,
    resource_name: &str,
    state: StoredResourceSyncState,
) {
    let store = engine.state::<CodeConnectionsState>();
    store
        .lock()
        .resource_sync_states
        .insert(sync_configuration_key(SYNC_TYPE, resource_name), state);
}

/// A `Revision` for `resource_name`, distinguished by `marker`.
fn revision(resource_name: &str, marker: &str) -> Revision {
    Revision {
        branch: format!("{marker}-branch"),
        directory: format!("stacks/{resource_name}"),
        owner_id: "owner-seed".to_string(),
        repository_name: format!("repo-{marker}"),
        provider_type: "GitHub".to_string(),
        sha: format!("{:0>40}", format!("{marker}{resource_name}")),
    }
}

/// The JSON a seeded [`revision`] must come back as.
fn expected_revision(resource_name: &str, marker: &str) -> serde_json::Value {
    let revision = revision(resource_name, marker);
    serde_json::json!({
        "Branch": revision.branch,
        "Directory": revision.directory,
        "OwnerId": revision.owner_id,
        "RepositoryName": revision.repository_name,
        "ProviderType": revision.provider_type,
        "Sha": revision.sha,
    })
}

/// A `ResourceSyncAttempt` with `events` events, as the service would record one.
fn sync_attempt(resource_name: &str, status: &str, events: usize) -> ResourceSyncAttempt {
    ResourceSyncAttempt {
        events: (0..events)
            .map(|index| ResourceSyncEvent {
                event: format!("event-{index}"),
                external_id: (index % 2 == 1).then(|| format!("external-{index}")),
                time: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS + index as i64),
                r#type: format!("type-{index}"),
            })
            .collect(),
        initial_revision: revision(resource_name, "initial"),
        started_at: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS),
        status: status.to_string(),
        target_revision: revision(resource_name, "target"),
        target: resource_name.to_string(),
    }
}

/// The JSON a seeded [`sync_attempt`] must come back as. awsJson1_0 renders a timestamp
/// as epoch seconds.
fn expected_attempt(resource_name: &str, status: &str, events: usize) -> serde_json::Value {
    let expected_events: Vec<serde_json::Value> = (0..events)
        .map(|index| {
            let mut event = serde_json::json!({
                "Event": format!("event-{index}"),
                "Time": BASE_EPOCH_SECS + index as i64,
                "Type": format!("type-{index}"),
            });
            if index % 2 == 1 {
                event["ExternalId"] = serde_json::json!(format!("external-{index}"));
            }
            event
        })
        .collect();
    serde_json::json!({
        "Events": expected_events,
        "InitialRevision": expected_revision(resource_name, "initial"),
        "StartedAt": BASE_EPOCH_SECS,
        "Status": status,
        "TargetRevision": expected_revision(resource_name, "target"),
        "Target": resource_name,
    })
}
