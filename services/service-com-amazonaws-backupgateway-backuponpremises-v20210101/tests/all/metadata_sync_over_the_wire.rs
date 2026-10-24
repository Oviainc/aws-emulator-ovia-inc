// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `StartVirtualMachinesMetadataSync` WORKFLOW, driven with wire
//! calls only.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle` and the helpers of
//! `hypervisor_property_mappings`, so what is asserted is the wire behaviour and the only
//! way a later request can see what an earlier one wrote is the shared store.
//!
//! Where the sibling `metadata_sync` suite seeds the hypervisor's property mappings
//! straight into the store, every record this suite's cases need is built by CALLING the
//! service: `ImportHypervisorConfiguration`, `CreateGateway`, `AssociateGatewayToServer`
//! and `PutHypervisorPropertyMappings`. So these cases pin that the documented workflow is
//! reachable for a client that only has the API — nothing here reaches into
//! [`backup_gateway::state`] to arrange a scene.
//!
//! The rules under test:
//!
//! 1. A metadata sync of a hypervisor's virtual-machine metadata runs under the IAM role
//!    that hypervisor's PROPERTY MAPPINGS name, so with mappings set for the ARN the sync
//!    answers 200 with the ARN of the record the store holds, and the
//!    `SyncMetadataStatus` a later read reports is a function of that record and the clock
//!    (`CREATED`, then `RUNNING`, then `SUCCEEDED` dated at the instant it fell due).
//!    Starting a sync again on the same hypervisor is answered the same way.
//! 2. With NO mappings set for the ARN the sync is refused with the modelled
//!    `AccessDeniedException` (HTTP 403, `ErrorCode` `MappingsNotConfigured`), and that
//!    refusal comes BEFORE any resource-existence lookup: an ARN this store holds no
//!    hypervisor under is refused exactly like a hypervisor that carries no mappings,
//!    where an operation that does resolve the resource first (`GetHypervisor`) answers
//!    the same ARN as a missing hypervisor. That ordering is the behaviour the recorded
//!    unit `backup-gateway.start-metadata-sync-state-transition` pins — see
//!    `issues/things-that-now-work/20261023T110543Z-backup-gateway-startvirtualmachinesmetadatasync-stub.md`,
//!    whose check asserts an `AccessDeniedException`/`MappingsNotConfigured` answer for a
//!    nonexistent `HypervisorArn`.
//!
//! The tables below span those classes rather than one request: hypervisors differing in
//! host form, mapping sets differing in count (including an empty list), IAM roles
//! differing in partition, path and length, and ARNs differing in resource type,
//! partition, id and length, including each published bound of `ServerArn`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    VmwareToAwsTagMapping, HYPERVISOR_NOT_FOUND, MAPPINGS_NOT_CONFIGURED,
    METADATA_SYNC_CREATED_MESSAGE, METADATA_SYNC_RUNNING_MESSAGE, METADATA_SYNC_SUCCEEDED_MESSAGE,
    METADATA_SYNC_SUCCEEDS_AFTER, SYNC_METADATA_CREATED, SYNC_METADATA_RUNNING,
    SYNC_METADATA_SUCCEEDED,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply};
use crate::hypervisor_property_mappings::{
    arn_of_length, expected_mappings, get_mappings, hypervisor_arn_in, import, mapping,
};

/// An `IamRoleArn` of this account in `partition`, naming `path_and_name`. The modelled
/// member is `@length(min: 20, max: 2048)` with pattern
/// `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$`
/// (`smithy_query(query="shape",shape="IamRoleArn")`).
fn role_arn(partition: &str, path_and_name: &str) -> String {
    format!(
        "arn:{partition}:iam::{}:role/{path_and_name}",
        engine::EMULATOR_ACCOUNT_ID
    )
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

/// Associate a freshly created gateway with the hypervisor `hypervisor_arn` names, over
/// the wire: `PutHypervisorPropertyMappings` is carried out by the appliance serving the
/// hypervisor, so a client reaches the mappings through this call.
async fn associate_gateway(router: &MultiServiceRouter, display_name: &str, hypervisor_arn: &str) {
    let gateway = create(router, display_name).await;
    let reply = call(
        router,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway, "ServerArn": hypervisor_arn }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.str_at("/GatewayArn")),
        (200, Some(gateway.as_str())),
        "case {display_name:?}: AssociateGatewayToServer must answer 200 with the gateway's \
         own ARN, got {} {:?}",
        reply.status,
        reply.body
    );
}

/// Put `mappings` on `hypervisor_arn` over the wire, asserting the put answered with the
/// stored record's ARN.
async fn put_mappings(
    router: &MultiServiceRouter,
    hypervisor_arn: &str,
    iam_role_arn: &str,
    mappings: &[VmwareToAwsTagMapping],
) {
    let reply = call(
        router,
        "PutHypervisorPropertyMappings",
        &json!({
            "HypervisorArn": hypervisor_arn,
            "IamRoleArn": iam_role_arn,
            "VmwareToAwsTagMappings": expected_mappings(mappings),
        }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.str_at("/HypervisorArn")),
        (200, Some(hypervisor_arn)),
        "PutHypervisorPropertyMappings must answer 200 with the record's own ARN, got {} {:?}",
        reply.status,
        reply.body
    );
}

/// Walk the whole documented workflow for one hypervisor with API calls only, and hand
/// back the ARN the import minted: import, create and associate a gateway, put the
/// mappings the sync runs under.
async fn served_hypervisor_with_mappings(
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
    iam_role_arn: &str,
    mappings: &[VmwareToAwsTagMapping],
) -> String {
    let hypervisor = import(router, name, host).await;
    associate_gateway(router, &format!("gw-{name}"), &hypervisor).await;
    put_mappings(router, &hypervisor, iam_role_arn, mappings).await;
    hypervisor
}

/// The `LatestMetadataSyncStatus`, its message and the `LastSuccessfulMetadataSyncTime` a
/// `GetHypervisor` reply reports, as one tuple so a case names what it compares.
fn reported_sync(reply: &Reply) -> (u16, Option<&str>, Option<&str>, bool) {
    (
        reply.status,
        reply.str_at("/Hypervisor/LatestMetadataSyncStatus"),
        reply.str_at("/Hypervisor/LatestMetadataSyncStatusMessage"),
        reply
            .body
            .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
            .is_some(),
    )
}

/// The documented workflow — import a hypervisor, have a gateway serve it, set its
/// property mappings, start the metadata sync, read the hypervisor back — is reachable
/// with wire calls only, and the sync is refused until the mappings it runs under are set.
///
/// The rows differ in the host's form, in the number of mappings put (including an empty
/// list) and in the partition, path and length of the IAM role they are put with — none of
/// which the workflow keys on.
#[tokio::test]
async fn the_documented_metadata_sync_workflow_is_reachable_over_the_wire() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Name, Host, IamRoleArn, the mappings the put carries)
    let rows: Vec<(&str, &str, &str, String, Vec<VmwareToAwsTagMapping>)> = vec![
        (
            "one mapping, a plain role, an IPv4 host",
            "hv-wire-0",
            "192.0.2.70",
            role_arn("aws", "backup-gateway-wire"),
            vec![mapping("department", "finance", "Department", "Finance")],
        ),
        (
            "three mappings, a service-role path, a DNS host",
            "hv-wire-1",
            "hv-wire.example.com",
            role_arn("aws", "service-role/backup-gateway"),
            vec![
                mapping("department", "finance", "Department", "Finance"),
                mapping("department", "legal", "Department", "Legal"),
                mapping("tier", "gold", "Tier", "Gold"),
            ],
        ),
        (
            "an empty mapping list, a role in the aws-cn partition",
            "hv-wire-2",
            "203.0.113.70",
            role_arn("aws-cn", "cn-wire"),
            vec![],
        ),
        (
            "the shortest role the modelled IamRoleArn allows",
            "hv-wire-3",
            "192.0.2.71",
            "arn:aws:iam::1:role/r".to_string(),
            vec![mapping("c", "t", "k", "")],
        ),
    ];

    for (case, name, host, iam_role_arn, mappings) in &rows {
        let hypervisor = import(&router, name, host).await;

        // Before the mappings are set there is no role the sync could run under, so it is
        // refused and nothing is written.
        let refused = start_sync(&router, &hypervisor).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode")
            ),
            (
                403,
                Some("AccessDeniedException"),
                Some(MAPPINGS_NOT_CONFIGURED)
            ),
            "case {case:?}: a sync before the mappings are put must be refused 403 \
             AccessDeniedException, got {} {:?}",
            refused.status,
            refused.body
        );
        let unchanged = get_hypervisor(&router, &hypervisor).await;
        assert_eq!(
            reported_sync(&unchanged),
            (200, None, None, false),
            "case {case:?}: a refused sync must leave the record reporting none, got {} {:?}",
            unchanged.status,
            unchanged.body
        );

        // The appliance that serves the hypervisor is what applies the mappings, so the
        // client creates a gateway, associates it and then puts them — every step a call.
        associate_gateway(&router, &format!("gw-{name}"), &hypervisor).await;
        put_mappings(&router, &hypervisor, iam_role_arn, mappings).await;
        let read_back = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            (
                read_back.status,
                read_back.str_at("/IamRoleArn"),
                read_back
                    .body
                    .get("VmwareToAwsTagMappings")
                    .cloned()
                    .unwrap_or(Value::Null)
            ),
            (200, Some(iam_role_arn.as_str()), expected_mappings(mappings)),
            "case {case:?}: the mappings the sync runs under must read back as they were \
             put, got {} {:?}",
            read_back.status,
            read_back.body
        );

        // ... and now the same sync call is served.
        let started = start_sync(&router, &hypervisor).await;
        assert_eq!(
            (started.status, started.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {case:?}: the sync must answer 200 with the record's own ARN, got {} {:?}",
            started.status,
            started.body
        );

        // A caller reading straight after the start still sees the sync it asked for, at a
        // point of its span where nothing has succeeded yet. WHICH point is the clock's
        // answer and not this suite's: nothing here arranges the stored instant, so a
        // loaded machine can be past the CREATED span (`METADATA_SYNC_RUNNING_AFTER`) by
        // the time the read lands. Either point is accepted, with the message of the
        // status reported beside it; the sibling `metadata_sync` suite pins each point
        // exactly by moving that instant.
        let in_flight = get_hypervisor(&router, &hypervisor).await;
        let reported = reported_sync(&in_flight);
        let in_flight_points = [
            (
                200,
                Some(SYNC_METADATA_CREATED),
                Some(METADATA_SYNC_CREATED_MESSAGE),
                false,
            ),
            (
                200,
                Some(SYNC_METADATA_RUNNING),
                Some(METADATA_SYNC_RUNNING_MESSAGE),
                false,
            ),
        ];
        assert!(
            in_flight_points.contains(&reported),
            "case {case:?}: a sync just started must read back as CREATED or RUNNING with no \
             successful sync yet, got {reported:?} in {:?}",
            in_flight.body
        );

        // Asking again is served again: a sync is started, not a resource minted.
        let restarted = start_sync(&router, &hypervisor).await;
        assert_eq!(
            (restarted.status, restarted.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {case:?}: starting the sync again must answer 200 with the same ARN, got \
             {} {:?}",
            restarted.status,
            restarted.body
        );
    }
}

/// Waiting for real, with no further call, carries a sync started over the wire to
/// `SUCCEEDED`, dated at the instant it fell due.
#[tokio::test]
async fn a_real_wait_carries_a_sync_started_over_the_wire_to_succeeded() {
    let engine = build_engine();
    let router = build_router(&engine);

    let hypervisor = served_hypervisor_with_mappings(
        &router,
        "hv-wire-waited",
        "192.0.2.80",
        &role_arn("aws", "waited-over-the-wire"),
        &[mapping("tier", "gold", "Tier", "Gold")],
    )
    .await;

    let started = start_sync(&router, &hypervisor).await;
    assert_eq!(
        started.status,
        200,
        "the sync must answer 200, got {} {:?}",
        started.status,
        started.body
    );

    // Blocking `std::thread::sleep` rather than `tokio::time::sleep`: this workspace pins
    // tokio with the `macros`/`rt` features only, so there is no `tokio::time` to await.
    std::thread::sleep(METADATA_SYNC_SUCCEEDS_AFTER + Duration::from_millis(100));

    let later = get_hypervisor(&router, &hypervisor).await;
    assert_eq!(
        reported_sync(&later),
        (
            200,
            Some(SYNC_METADATA_SUCCEEDED),
            Some(METADATA_SYNC_SUCCEEDED_MESSAGE),
            true
        ),
        "waiting alone must carry the sync to SUCCEEDED with the time it succeeded at, got \
         {} {:?}",
        later.status,
        later.body
    );
}

/// The mappings gate is asked about the ARN the request carries and is answered before any
/// resource-existence lookup: only an ARN mappings are set for syncs, and every other ARN
/// — a hypervisor carrying none, one whose appliance never put any, a gateway, an ARN this
/// store holds no record for at all, in any partition and at each published bound of
/// `ServerArn` — takes the same `AccessDeniedException`.
#[tokio::test]
async fn only_an_arn_with_mappings_syncs_and_the_gate_precedes_the_existence_lookup() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The one record the gate passes: its mappings were put over the wire.
    let with_mappings = served_hypervisor_with_mappings(
        &router,
        "hv-gate-mapped",
        "192.0.2.90",
        &role_arn("aws-us-gov", "gov-wire"),
        &[mapping("tier", "silver", "Tier", "Silver")],
    )
    .await;
    // Records that really exist and carry no mappings, so a refusal below is about the
    // mappings and not about an empty store.
    let imported_only = import(&router, "hv-gate-bare", "192.0.2.91").await;
    let served_only = import(&router, "hv-gate-served", "192.0.2.92").await;
    associate_gateway(&router, "gw-hv-gate-served", &served_only).await;
    let live_gateway = create(&router, "gw-gate-live").await;
    let ghost = hypervisor_arn_in("aws", "hv-ffffffffffffff10");

    let served = start_sync(&router, &with_mappings).await;
    assert_eq!(
        (served.status, served.str_at("/HypervisorArn")),
        (200, Some(with_mappings.as_str())),
        "an ARN mappings were put for must sync, got {} {:?}",
        served.status,
        served.body
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor that was only imported",
            imported_only.clone(),
        ),
        (
            "a hypervisor a gateway serves, with no mappings put",
            served_only.clone(),
        ),
        ("a gateway ARN that really names a gateway", live_gateway),
        ("a hypervisor ARN that was never imported", ghost.clone()),
        (
            "a never-imported ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff11"),
        ),
        (
            "a never-imported ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff12"),
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
            "case {case:?}: a sync for an ARN no mappings are set for must be refused 403 \
             AccessDeniedException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // The refusals wrote nothing: the records this store does hold still report no sync.
    for (case, arn) in [
        ("a hypervisor that was only imported", &imported_only),
        ("a hypervisor a gateway serves", &served_only),
    ] {
        let reply = get_hypervisor(&router, arn).await;
        assert_eq!(
            reported_sync(&reply),
            (200, None, None, false),
            "case {case:?}: a refused sync must leave the record reporting none, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // An operation that DOES resolve the hypervisor first answers the same never-imported
    // ARN the other way, so the 403 above is the gate's order and not this service
    // refusing every unknown ARN alike.
    let read = get_hypervisor(&router, &ghost).await;
    assert_eq!(
        (read.status, read.str_at("/ErrorCode")),
        (400, Some(HYPERVISOR_NOT_FOUND)),
        "GetHypervisor must refuse a never-imported ARN as a missing hypervisor, got {} {:?}",
        read.status,
        read.body
    );
}
