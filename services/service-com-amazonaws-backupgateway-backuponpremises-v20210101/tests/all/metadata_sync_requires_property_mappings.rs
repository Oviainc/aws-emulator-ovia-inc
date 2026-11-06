// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PRECONDITION of a `StartVirtualMachinesMetadataSync`: the
//! property mappings of the hypervisor its `HypervisorArn` names.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the workflow helpers of
//! `metadata_sync_over_the_wire`, so what is asserted is the wire behaviour and nothing
//! here reaches into `backup_gateway::state` to arrange a scene.
//!
//! The rule under test: a metadata sync of a hypervisor's virtual machines requires that
//! hypervisor's property mappings to be CONFIGURED — a `HypervisorArn` this service holds
//! no mappings under is refused and nothing is written, with the modelled
//! `AccessDeniedException`, `ErrorCode` `MappingsNotConfigured` and the sentence AWS states
//! for the condition, answered at `MAPPINGS_NOT_CONFIGURED_STATUS`. The gate is asked
//! BEFORE any resource-existence lookup, so a record whose mappings were never put and an
//! ARN naming no record at all take the one answer and it reveals neither. What the
//! mappings SAY conditions nothing — that is the sibling
//! `metadata_sync_is_not_gated_on_property_mappings` suite's rule.
//!
//! The table below spans that class rather than one request: records differing in how far
//! along the workflow they were taken (never imported, imported only, served by a gateway
//! with no mappings put) and ARNs differing in resource type, partition, id and length,
//! including each published bound of `ServerArn`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    MAPPINGS_NOT_CONFIGURED, MAPPINGS_NOT_CONFIGURED_MESSAGE, MAPPINGS_NOT_CONFIGURED_STATUS,
    METADATA_SYNC_SUCCEEDS_AFTER,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply};
use crate::hypervisor_property_mappings::{arn_of_length, hypervisor_arn_in, import, mapping};
use crate::metadata_sync_over_the_wire::{
    associate_gateway, role_arn, served_hypervisor_with_mappings, start_sync,
};

/// Long enough past [`METADATA_SYNC_SUCCEEDS_AFTER`] that a sync started before the wait
/// would have fallen due on a loaded machine too.
const PAST_THE_SYNC: Duration =
    METADATA_SYNC_SUCCEEDS_AFTER.saturating_add(Duration::from_millis(150));

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// The `ResourceArn`s `ListVirtualMachines` reports, for one hypervisor when `filter` is
/// set and for the whole account when it is not.
pub(crate) async fn listed_arns(
    router: &MultiServiceRouter,
    filter: Option<&str>,
) -> (u16, Vec<String>) {
    let mut body = json!({});
    if let Some(arn) = filter {
        body["HypervisorArn"] = json!(arn);
    }
    let reply = call(router, "ListVirtualMachines", &body).await;
    let arns = reply
        .body
        .get("VirtualMachines")
        .and_then(Value::as_array)
        .map(|machines| {
            machines
                .iter()
                .map(|machine| {
                    machine
                        .get("ResourceArn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    (reply.status, arns)
}

/// Everything a refusal of this precondition states: the status, the error shape, the
/// required `ErrorCode` and the sentence.
///
/// Shared with the sibling `fixed_refusal_sentences` suite, so one refusal is read one way
/// in both.
pub(crate) fn refusal(reply: &Reply) -> (u16, Option<&str>, Option<&str>, Option<&str>) {
    (
        reply.status,
        reply.code.as_deref(),
        reply.str_at("/ErrorCode"),
        reply.str_at("/Message"),
    )
}

/// Every `HypervisorArn` this service holds no property mappings under takes the one
/// refusal — status, shape, `ErrorCode` and sentence alike — whether it names a record
/// whose mappings were never put or no record at all, and the refusal writes nothing: no
/// sync, and so no inventory, anywhere.
#[tokio::test]
async fn an_arn_with_no_configured_mappings_is_refused_one_way() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Records that really exist, so a refusal below is about the mappings the sync needs
    // and not about an empty store.
    let live_gateway = create(&router, "gw-gated-live").await;
    let imported_only = import(&router, "hv-gated-bare", "192.0.2.150").await;
    let served_only = import(&router, "hv-gated-served", "192.0.2.151").await;
    associate_gateway(&router, "gw-hv-gated-served", &served_only).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor that was only imported, so no mappings were put",
            imported_only,
        ),
        (
            "a hypervisor a gateway serves, with no mappings put",
            served_only,
        ),
        (
            "a hypervisor ARN that was never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff20"),
        ),
        (
            "a never-imported ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff21"),
        ),
        (
            "a never-imported ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff22"),
        ),
        ("a gateway ARN that really names a gateway", live_gateway),
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
            refusal(&reply),
            (
                MAPPINGS_NOT_CONFIGURED_STATUS,
                Some("AccessDeniedException"),
                Some(MAPPINGS_NOT_CONFIGURED),
                Some(MAPPINGS_NOT_CONFIGURED_MESSAGE)
            ),
            "case {case:?}: a sync for an ARN with no configured mappings must be refused \
             {MAPPINGS_NOT_CONFIGURED_STATUS} AccessDeniedException, got {} {:?}",
            reply.status,
            reply.body
        );

        // Nothing about the record moved: a refused sync was never started, so a
        // hypervisor the service does report reports no sync at all.
        let read = get_hypervisor(&router, arn).await;
        if read.status == 200 {
            assert_eq!(
                read.str_at("/Hypervisor/LatestMetadataSyncStatus"),
                None,
                "case {case:?}: a refused sync must leave the record reporting none, got {:?}",
                read.body
            );
        }
    }

    // Waiting changes nothing: a refused sync was never started, so it never falls due.
    std::thread::sleep(PAST_THE_SYNC);
    assert_eq!(
        listed_arns(&router, None).await,
        (200, Vec::new()),
        "a refused sync must materialise no inventory"
    );
}

/// The precondition is settled by the mappings the service HOLDS at the moment of the
/// call, so a hypervisor whose mappings were never put is refused where one whose gateway
/// put them is served — and the refusal wrote nothing that the later sync of another
/// record depends on.
#[tokio::test]
async fn putting_the_mappings_is_what_turns_the_refusal_into_a_sync() {
    let engine = build_engine();
    let router = build_router(&engine);

    let unconfigured = import(&router, "hv-gated-turned", "192.0.2.160").await;
    let before = start_sync(&router, &unconfigured).await;
    assert_eq!(
        refusal(&before),
        (
            MAPPINGS_NOT_CONFIGURED_STATUS,
            Some("AccessDeniedException"),
            Some(MAPPINGS_NOT_CONFIGURED),
            Some(MAPPINGS_NOT_CONFIGURED_MESSAGE)
        ),
        "case \"no mappings put\": the sync must be refused, got {} {:?}",
        before.status,
        before.body
    );

    // The two values AWS is recorded stating for this refusal, written out rather than read
    // off the constants the handler uses, so a change to either is caught here: the status
    // is 400 and NOT the 403 `AccessDeniedException`'s `@httpError` declares, and the
    // sentence is entirely fixed text with no part taken from the request.
    assert_eq!(
        (before.status, before.str_at("/Message")),
        (
            400,
            Some("HypervisorPropertyMappings is not configured for the Hypervisor")
        ),
        "case \"no mappings put\": the refusal must state the recorded status and sentence, \
         got {} {:?}",
        before.status,
        before.body
    );

    // The appliance that serves the hypervisor is what applies the mappings, so the client
    // creates a gateway, associates it and puts them — every step a call.
    let configured = served_hypervisor_with_mappings(
        &router,
        "hv-gated-turned-2",
        "192.0.2.161",
        &role_arn("aws", "turned-gated"),
        &[mapping("tier", "silver", "Tier", "Silver")],
    )
    .await;
    let after = start_sync(&router, &configured).await;
    assert_eq!(
        (after.status, after.str_at("/HypervisorArn")),
        (200, Some(configured.as_str())),
        "case \"mappings put\": the sync must answer 200 with the record's own ARN, got \
         {} {:?}",
        after.status,
        after.body
    );

    // The hypervisor whose mappings were never put still has no sync and no inventory,
    // however many other records have one.
    std::thread::sleep(PAST_THE_SYNC);
    assert_eq!(
        listed_arns(&router, Some(&unconfigured)).await,
        (200, Vec::new()),
        "case \"no mappings put\": the refused hypervisor must still hold no inventory"
    );
}
