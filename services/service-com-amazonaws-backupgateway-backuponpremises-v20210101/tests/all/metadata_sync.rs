// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartVirtualMachinesMetadataSync`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rules under test:
//!
//! 1. An operation that can only run under the IAM role a hypervisor's PROPERTY MAPPINGS
//!    name is refused with the modelled `AccessDeniedException` (HTTP 403, `ErrorCode`
//!    `MappingsNotConfigured`) whenever no mappings are set for the ARN it was asked for
//!    — before any resource-existence lookup, so an ARN naming no hypervisor at all is
//!    refused exactly like a hypervisor carrying no mappings, and never as a missing
//!    resource.
//! 2. Starting a metadata sync PERSISTS it on the hypervisor the ARN resolves to, and the
//!    `SyncMetadataStatus` a later read reports is a function of that record and the
//!    clock — `CREATED`, then `RUNNING`, then `SUCCEEDED` dated at the instant it fell
//!    due — so no call has to poll for the transition and no read moves it.
//!
//! The tables below span those classes: ARNs differing in resource type, partition, id
//! and length (including each published bound of `ServerArn`), mapping sets differing in
//! count and in the role they are set with, and every point of the sync's span.
//!
//! Where a case needs the clock to have moved on, the stored instant the sync started at
//! is moved BACK instead of sleeping ([`rewind_sync`]): nothing but the clock carries a
//! sync along, so a sync that started earlier and one observed later are the same
//! observation. One case waits for real ([`a_real_wait_alone_carries_the_sync_to_succeeded`])
//! to prove that no further call is what moves it.
//!
//! `PutHypervisorPropertyMappings` is the operation that SETS mappings and is still a
//! scaffold answering 501 under its own stub issue
//! (`issues/things-that-do-not-work/20261023T110543Z-backup-gateway-puthypervisorpropertymappings-stub.md`),
//! so the store is the only place mappings can come from until it is implemented —
//! seeded here through the same `property_mappings` field that handler will write, as the
//! sibling `hypervisor_property_mappings` suite does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, StoredPropertyMappings, VmwareToAwsTagMapping, HYPERVISOR_NOT_FOUND,
    MAPPINGS_NOT_CONFIGURED, METADATA_SYNC_CREATED_MESSAGE, METADATA_SYNC_RUNNING_AFTER,
    METADATA_SYNC_RUNNING_MESSAGE, METADATA_SYNC_SUCCEEDED_MESSAGE, METADATA_SYNC_SUCCEEDS_AFTER,
    SYNC_METADATA_CREATED, SYNC_METADATA_RUNNING, SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// A hypervisor ARN in `partition` naming `id`, so the rows span every partition the
/// modelled `ServerArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`).
fn hypervisor_arn_in(partition: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:hypervisor/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ServerArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="ServerArn")`).
fn arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:h/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// One modelled `VmwareToAwsTagMapping`
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`: every member required).
fn mapping(category: &str, tag_name: &str, key: &str, value: &str) -> VmwareToAwsTagMapping {
    VmwareToAwsTagMapping {
        vmware_category: category.to_string(),
        vmware_tag_name: tag_name.to_string(),
        aws_tag_key: key.to_string(),
        aws_tag_value: value.to_string(),
    }
}

/// Import a hypervisor over the wire and hand back the ARN the import minted.
async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

async fn start_sync(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "StartVirtualMachinesMetadataSync",
        &json!({ "HypervisorArn": arn }),
    )
    .await
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// Set `mappings` on the stored hypervisor — the one field
/// `PutHypervisorPropertyMappings` will write (see the module docs).
fn set_mappings(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    mappings: Option<StoredPropertyMappings>,
) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    state
        .lock()
        .hypervisor_mut(arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
        .property_mappings = mappings;
}

/// The instant the sync in flight on `arn` started at, as the store holds it.
fn sync_started(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<SystemTime> {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    store
        .hypervisor(arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
        .metadata_sync_started
}

/// Move the stored instant the sync on `arn` started at BACK by `by`, so a read observes
/// the point of the sync's span that lies `by` after it started.
///
/// The sync's progress is derived from that one instant and the clock, so this is the
/// same observation as waiting `by` — and it is what lets the table below span the whole
/// span without spending it.
fn rewind_sync(engine: &Arc<engine::InMemoryEngine>, arn: &str, by: Duration) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    let record = store
        .hypervisor_mut(arn)
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"));
    let started = record
        .metadata_sync_started
        .unwrap_or_else(|| panic!("no sync in flight on {arn}"));
    record.metadata_sync_started = Some(started - by);
}

/// The `LastSuccessfulMetadataSyncTime` a reply reports, in epoch seconds.
fn reported_success_time(reply: &Reply) -> Option<f64> {
    reply
        .body
        .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
        .and_then(Value::as_f64)
}

/// `time` as epoch seconds, the form the modelled `Time` timestamp takes on the wire.
fn epoch_secs(time: SystemTime) -> f64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// A started sync is persisted on the hypervisor the ARN resolves to, and the status a
/// read reports walks the sync's span on the clock alone: `CREATED`, `RUNNING`, then
/// `SUCCEEDED` dated at the instant it fell due.
///
/// The rows differ in the mappings the sync runs under — their count, the partition and
/// shape of the IAM role, an empty mapping list — none of which the sync's progress keys
/// on.
#[tokio::test]
async fn a_started_sync_is_persisted_and_the_clock_carries_it_to_succeeded() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Host, IamRoleArn, the mappings set for it)
    let rows: Vec<(&str, &str, String, Vec<VmwareToAwsTagMapping>)> = vec![
        (
            "one mapping, a plain role",
            "198.51.100.10",
            format!(
                "arn:aws:iam::{}:role/backup-gateway-sync",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![mapping("department", "finance", "Department", "Finance")],
        ),
        (
            "three mappings and a service-role path",
            "hypervisor.example.com",
            format!(
                "arn:aws:iam::{}:role/service-role/backup-gateway",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("department", "legal", "Department", "Legal"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        ),
        (
            "an empty mapping list, set in the aws-cn partition",
            "203.0.113.7",
            format!(
                "arn:aws-cn:iam::{}:role/cn-sync",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vec![],
        ),
        (
            "the shortest role the modelled IamRoleArn allows",
            "198.51.100.11",
            // `IamRoleArn` is `@length(min: 20, max: 2048)` with pattern
            // `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$`.
            "arn:aws:iam::1:role/r".to_string(),
            vec![mapping("c", "t", "k", "")],
        ),
    ];

    for (index, (case, host, iam_role_arn, mappings)) in rows.iter().enumerate() {
        let arn = import(&router, &format!("hv-{index}"), host).await;
        set_mappings(
            &engine,
            &arn,
            Some(StoredPropertyMappings {
                iam_role_arn: iam_role_arn.clone(),
                vmware_to_aws_tag_mappings: mappings.clone(),
            }),
        );

        // Nothing is in flight before the start call, so the record reports no sync at
        // all: the members below are what this operation wrote.
        let before = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (
                before.status,
                before.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                reported_success_time(&before)
            ),
            (200, None, None),
            "case {case:?}: a hypervisor no sync was started on must report none, got {} {:?}",
            before.status,
            before.body
        );

        let reply = start_sync(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: the start must answer 200 with the record's own ARN, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            sync_started(&engine, &arn).is_some(),
            "case {case:?}: the start must persist the instant the sync was requested"
        );

        // Straight after the start call the sync exists and has not begun.
        let created = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (
                created.status,
                created.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                created.str_at("/Hypervisor/LatestMetadataSyncStatusMessage"),
                reported_success_time(&created)
            ),
            (
                200,
                Some(SYNC_METADATA_CREATED),
                Some(METADATA_SYNC_CREATED_MESSAGE),
                None
            ),
            "case {case:?}: a sync just requested must read back as CREATED with no \
             successful sync yet, got {} {:?}",
            created.status,
            created.body
        );

        // ... at the instant it begins, it is RUNNING, and still nothing has succeeded.
        rewind_sync(&engine, &arn, METADATA_SYNC_RUNNING_AFTER);
        let running = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (
                running.status,
                running.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                running.str_at("/Hypervisor/LatestMetadataSyncStatusMessage"),
                reported_success_time(&running)
            ),
            (
                200,
                Some(SYNC_METADATA_RUNNING),
                Some(METADATA_SYNC_RUNNING_MESSAGE),
                None
            ),
            "case {case:?}: a sync past its start must read back as RUNNING, got {} {:?}",
            running.status,
            running.body
        );

        // ... and once it falls due it has SUCCEEDED, dated at that instant.
        rewind_sync(
            &engine,
            &arn,
            METADATA_SYNC_SUCCEEDS_AFTER - METADATA_SYNC_RUNNING_AFTER,
        );
        let succeeded = get_hypervisor(&router, &arn).await;
        let expected_success = epoch_secs(
            sync_started(&engine, &arn).expect("the sync must still be on the record")
                + METADATA_SYNC_SUCCEEDS_AFTER,
        );
        assert_eq!(
            (
                succeeded.status,
                succeeded.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                succeeded.str_at("/Hypervisor/LatestMetadataSyncStatusMessage"),
            ),
            (
                200,
                Some(SYNC_METADATA_SUCCEEDED),
                Some(METADATA_SYNC_SUCCEEDED_MESSAGE),
            ),
            "case {case:?}: a sync past its due instant must read back as SUCCEEDED, got {} {:?}",
            succeeded.status,
            succeeded.body
        );
        let reported = reported_success_time(&succeeded);
        assert!(
            reported.is_some_and(|secs| (secs - expected_success).abs() < 0.002),
            "case {case:?}: the successful sync must be dated at the instant it fell due \
             ({expected_success}), got {reported:?}"
        );
    }
}

/// Waiting for real, with no further call, is what carries the sync along: the status a
/// read reports changes on the clock alone.
#[tokio::test]
async fn a_real_wait_alone_carries_the_sync_to_succeeded() {
    let engine = build_engine();
    let router = build_router(&engine);

    let arn = import(&router, "hv-waited", "198.51.100.20").await;
    set_mappings(
        &engine,
        &arn,
        Some(StoredPropertyMappings {
            iam_role_arn: format!("arn:aws:iam::{}:role/waited", engine::EMULATOR_ACCOUNT_ID),
            vmware_to_aws_tag_mappings: vec![mapping("tier", "gold", "Tier", "Gold")],
        }),
    );

    let reply = start_sync(&router, &arn).await;
    assert_eq!(
        reply.status, 200,
        "the start must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let immediately = get_hypervisor(&router, &arn).await;
    assert_eq!(
        immediately.str_at("/Hypervisor/LatestMetadataSyncStatus"),
        Some(SYNC_METADATA_CREATED),
        "a caller reading straight after the start must still see the sync it requested, \
         got {:?}",
        immediately.body
    );

    // Blocking `std::thread::sleep` rather than `tokio::time::sleep`: this workspace pins
    // tokio with the `macros`/`rt` features only, so there is no `tokio::time` to await.
    std::thread::sleep(METADATA_SYNC_SUCCEEDS_AFTER + Duration::from_millis(100));

    let later = get_hypervisor(&router, &arn).await;
    assert_eq!(
        later.str_at("/Hypervisor/LatestMetadataSyncStatus"),
        Some(SYNC_METADATA_SUCCEEDED),
        "waiting alone must carry the sync to SUCCEEDED, got {:?}",
        later.body
    );
    assert!(
        reported_success_time(&later).is_some(),
        "a succeeded sync must report the time it succeeded at, got {:?}",
        later.body
    );
}

/// With no property mappings set for the ARN, the sync is refused with the modelled
/// `AccessDeniedException` — HTTP 403, `ErrorCode` `MappingsNotConfigured` — whatever the
/// ARN names: a hypervisor carrying no mappings, one whose mappings were cleared, a
/// gateway, or an ARN this store holds no record for at all, in any partition and at each
/// published bound of `ServerArn`.
#[tokio::test]
async fn a_sync_is_refused_when_no_mappings_are_configured() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Resources that really exist, so a refusal below is about the mappings and not about
    // an empty store.
    let live_gateway = create(&router, "gw-live").await;
    let no_mappings = import(&router, "hv-nomap", "198.51.100.30").await;
    let cleared = import(&router, "hv-cleared", "198.51.100.31").await;
    set_mappings(
        &engine,
        &cleared,
        Some(StoredPropertyMappings {
            iam_role_arn: format!(
                "arn:aws:iam::{}:role/temporary",
                engine::EMULATOR_ACCOUNT_ID
            ),
            vmware_to_aws_tag_mappings: vec![mapping("c", "t", "k", "v")],
        }),
    );
    set_mappings(&engine, &cleared, None);

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor this store holds, carrying no mappings",
            no_mappings.clone(),
        ),
        (
            "a hypervisor whose mappings were set and then cleared",
            cleared.clone(),
        ),
        (
            "a hypervisor ARN that was never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff01"),
        ),
        (
            "a never-imported ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff02"),
        ),
        (
            "a never-imported ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff03"),
        ),
        (
            "a gateway ARN that really names a gateway",
            live_gateway.clone(),
        ),
        (
            "the 50-character minimum of the modelled ServerArn",
            arn_of_length(50),
        ),
        (
            "the 500-character maximum of the modelled ServerArn",
            arn_of_length(500),
        ),
    ];

    for (case, arn) in &rows {
        let reply = start_sync(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode")
            ),
            (
                403,
                Some("AccessDeniedException"),
                Some(MAPPINGS_NOT_CONFIGURED)
            ),
            "case {case:?}: a sync with no mappings configured must be refused with 403 \
             AccessDeniedException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // A refused sync wrote nothing: the records this store does hold still report no sync.
    for (case, arn) in [
        ("a hypervisor carrying no mappings", &no_mappings),
        ("a hypervisor whose mappings were cleared", &cleared),
    ] {
        assert_eq!(
            sync_started(&engine, arn),
            None,
            "case {case:?}: a refused sync must not have been persisted"
        );
        let reply = get_hypervisor(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                reported_success_time(&reply)
            ),
            (200, None, None),
            "case {case:?}: a refused sync must leave the record reporting none, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// The permission gate fires BEFORE any resource-existence lookup: a well-formed ARN this
/// store holds no hypervisor under is refused exactly like a hypervisor that exists and
/// carries no mappings — and not with the missing-hypervisor refusal every operation that
/// does resolve the resource first answers with.
#[tokio::test]
async fn the_refusal_does_not_reveal_whether_the_arn_names_a_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    let existing = import(&router, "hv-existing", "198.51.100.40").await;
    let ghost = hypervisor_arn_in("aws", "hv-ffffffffffffff04");

    let for_existing = start_sync(&router, &existing).await;
    let for_ghost = start_sync(&router, &ghost).await;
    assert_eq!(
        (
            for_ghost.status,
            for_ghost.code.clone(),
            for_ghost.str_at("/ErrorCode").map(str::to_string),
            for_ghost.str_at("/Message").map(str::to_string),
        ),
        (
            for_existing.status,
            for_existing.code.clone(),
            for_existing.str_at("/ErrorCode").map(str::to_string),
            for_existing.str_at("/Message").map(str::to_string),
        ),
        "an ARN naming no hypervisor must be refused exactly like one carrying no \
         mappings, got {:?} for the ghost and {:?} for the existing record",
        for_ghost.body,
        for_existing.body
    );
    assert_eq!(
        for_ghost.status, 403,
        "the gate must answer 403, got {} {:?}",
        for_ghost.status, for_ghost.body
    );

    // An operation that DOES resolve the hypervisor first answers the same ghost ARN the
    // other way, so the 403 above is the gate's order and not this service refusing every
    // unknown ARN alike.
    let read = get_hypervisor(&router, &ghost).await;
    assert_eq!(
        (read.status, read.str_at("/ErrorCode")),
        (400, Some(HYPERVISOR_NOT_FOUND)),
        "GetHypervisor must still refuse the ghost ARN as a missing hypervisor, got {} {:?}",
        read.status,
        read.body
    );
}

/// Reading the hypervisor never moves the sync, and a second sync restarts it: the status
/// goes back to `CREATED` while the time of the sync that already succeeded stands until
/// the new one falls due.
#[tokio::test]
async fn reads_never_move_the_sync_and_a_second_start_restarts_it() {
    let engine = build_engine();
    let router = build_router(&engine);

    let arn = import(&router, "hv-stable", "198.51.100.50").await;
    set_mappings(
        &engine,
        &arn,
        Some(StoredPropertyMappings {
            iam_role_arn: format!("arn:aws:iam::{}:role/stable", engine::EMULATOR_ACCOUNT_ID),
            vmware_to_aws_tag_mappings: vec![mapping("tier", "gold", "Tier", "Gold")],
        }),
    );

    assert_eq!(
        start_sync(&router, &arn).await.status,
        200,
        "the first start must answer 200"
    );

    // Repeated reads of a sync in flight agree, and none of them advances it.
    let mut in_flight = Vec::new();
    for read in 0..3 {
        let reply = get_hypervisor(&router, &arn).await;
        assert_eq!(
            reply.str_at("/Hypervisor/LatestMetadataSyncStatus"),
            Some(SYNC_METADATA_CREATED),
            "read {read}: a read must not advance the sync, got {:?}",
            reply.body
        );
        in_flight.push(reply.body);
    }
    assert!(
        in_flight.windows(2).all(|pair| pair[0] == pair[1]),
        "repeated reads of a sync in flight must agree, got {in_flight:?}"
    );

    // Past the due instant the success time is stable however often it is read.
    rewind_sync(&engine, &arn, METADATA_SYNC_SUCCEEDS_AFTER);
    let first = get_hypervisor(&router, &arn).await;
    let again = get_hypervisor(&router, &arn).await;
    let first_success = reported_success_time(&first);
    assert_eq!(
        (
            first.str_at("/Hypervisor/LatestMetadataSyncStatus"),
            first_success
        ),
        (
            again.str_at("/Hypervisor/LatestMetadataSyncStatus"),
            reported_success_time(&again)
        ),
        "a succeeded sync must read the same every time, got {:?} then {:?}",
        first.body,
        again.body
    );
    assert_eq!(
        first.str_at("/Hypervisor/LatestMetadataSyncStatus"),
        Some(SYNC_METADATA_SUCCEEDED),
        "the first sync must have succeeded, got {:?}",
        first.body
    );

    // A second start begins a new sync: CREATED again, with the previous success still
    // standing because the new sync has not finished.
    assert_eq!(
        start_sync(&router, &arn).await.status,
        200,
        "the second start must answer 200"
    );
    let restarted = get_hypervisor(&router, &arn).await;
    assert_eq!(
        restarted.str_at("/Hypervisor/LatestMetadataSyncStatus"),
        Some(SYNC_METADATA_CREATED),
        "a second start must put the record back to CREATED, got {:?}",
        restarted.body
    );
    let restarted_success = reported_success_time(&restarted);
    assert!(
        restarted_success.is_some_and(|secs| first_success
            .is_some_and(|first| (secs - first).abs() < 0.002)),
        "a sync in flight must not touch the time the previous one succeeded at \
         ({first_success:?}), got {restarted_success:?}"
    );

    // And once the new sync falls due, the success time it reports is its own.
    rewind_sync(&engine, &arn, METADATA_SYNC_SUCCEEDS_AFTER);
    let finished = get_hypervisor(&router, &arn).await;
    let finished_success = reported_success_time(&finished);
    assert!(
        finished_success.is_some_and(|secs| first_success.is_some_and(|first| secs > first)),
        "the second sync must date its own success later than the first \
         ({first_success:?}), got {finished_success:?}"
    );
}

/// A `HypervisorArn` outside the modelled constraints never reaches the gate: it is
/// refused with HTTP 400 `ValidationException`, one past each published bound of
/// `ServerArn` (`@length(min: 50, max: 500)`, pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`).
#[tokio::test]
async fn a_sync_refuses_an_arn_outside_the_modelled_constraints() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one character under the 50-character minimum",
            json!({ "HypervisorArn": arn_of_length(49) }),
        ),
        (
            "one character over the 500-character maximum",
            json!({ "HypervisorArn": arn_of_length(501) }),
        ),
        (
            "a partition the pattern does not accept",
            json!({ "HypervisorArn": hypervisor_arn_in("aws-iso", "hv-0000000000000003") }),
        ),
        (
            "another service's ARN",
            json!({ "HypervisorArn": format!(
                "arn:aws:backup:{REGION}:{}:hypervisor/hv-0000000000000004",
                engine::EMULATOR_ACCOUNT_ID
            ) }),
        ),
        ("no HypervisorArn at all", json!({})),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "StartVirtualMachinesMetadataSync", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ARN outside the modelled constraints must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        0,
        "a refused request must not have created anything"
    );
}
