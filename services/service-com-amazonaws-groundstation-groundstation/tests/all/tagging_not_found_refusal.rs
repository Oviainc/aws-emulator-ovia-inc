// SPDX-License-Identifier: CC0-1.0

//! Ground Station's TAGGING PLANE refusing an ARN that resolves to nothing, end to end
//! over HTTP.
//!
//! The RULE under test: `TagResource`, `UntagResource` and `ListTagsForResource` are one
//! ARN-addressed plane, so when `resourceArn` resolves to no resource of this account —
//! whether the resource was deleted, was never created, belongs to another service, or the
//! ARN is one this service never mints at all — every one of the three answers the SAME
//! refusal: the wire code [`EXPECTED_CODE`], HTTP [`EXPECTED_STATUS`], and the whole
//! message [`EXPECTED_MESSAGE`] — one FIXED sentence, with nothing from the
//! request interpolated into it, so the ARN, the account id and the region the caller sent
//! appear nowhere in it.
//!
//! The suite asserts that over a TABLE rather than over the one recorded call, its rows
//! differing along what the rule generalises over — how the ARN came to resolve to nothing
//! (deleted vs never created vs never mintable) and which kind of resource it names (a
//! config of every `ConfigCapabilityType`, a mission profile, a dataflow endpoint group, a
//! contact, an agent, another service's resource, another region's, another account's, and
//! the shortest ARN the model admits) — crossed with all three operations of the plane.
//!
//! The harness, the signed `restJson1` requests, the creates and the tag plumbing are
//! [`crate::scheduling_loop`]'s, [`crate::resource_tagging`]'s and
//! [`crate::tag_removal`]'s, reused rather than rebuilt.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::security_details;
use crate::resource_tagging::{agent_arn, contact_arn, list_tags, tag_resource, tags_of};
use crate::scheduling_loop::{
    antenna_uplink_config_data, config_data_table, create_config, create_mission_profile,
    harness_with_engine, provision_uplink_approval, send, Reply, ABSENT_CONFIG_ARN,
    ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID, REGION,
};
use crate::tag_removal::untag_resource;

/// The wire error code AWS's Ground Station tagging plane names a resource it cannot find
/// by. Spelled here as the requirement rather than read from this crate, so a change to
/// what the emulator answers is what this suite catches.
const EXPECTED_CODE: &str = "NotFoundException";

/// The WHOLE message that refusal states: one fixed sentence, carrying nothing from the
/// request. It is the wording the model gives the shape whose payload the refusal carries,
/// "Resource was not found." (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), as the tagging plane states it.
const EXPECTED_MESSAGE: &str = "Resource was not found";

/// The HTTP status of that refusal: the `@httpError` of `ResourceNotFoundException`, the
/// shape these operations declare for it (`smithy_query(query="operation",
/// operation="TagResource")`).
const EXPECTED_STATUS: u16 = 434;

/// The shortest `resourceArn` the model admits: `AnyArn` is `@length(min: 5, max: 1024)`
/// with `@pattern("^(arn:aws:)[\s\S]{0,1024}$")` (`smithy_query(query="operation",
/// operation="TagResource")`), so the prefix alone is a legal value — one this service could
/// never have minted, and so one more way for an ARN to resolve to nothing.
const SHORTEST_ADMITTED_ARN: &str = "arn:aws:";

/// Every ARN of the table, each paired with how it came to resolve to no resource.
///
/// The deleted rows are created and then deleted here, so the suite asserts the refusal a
/// client meets after a delete — the case the plane is most often asked about — beside the
/// ARNs that never resolved at all.
async fn arns_resolving_to_nothing(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    // `uplinkEchoConfig` must point at an antenna-uplink config, so one exists first and is
    // NOT a row: the configs that reference it are deleted, it is not.
    let (_, _, antenna_uplink_arn) = create_config(
        router,
        "not-found echo source",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    // A deleted config of EVERY capability type (source:
    // `crate::scheduling_loop::config_data_table`, built from `smithy_query(query="shape",
    // shape="ConfigTypeData")`), so the rule is asserted over the whole union rather than
    // over the one type the recorded call carried. Deleted in reverse creation order, so a
    // config another one references is gone before the one it points at.
    let mut created: Vec<(String, String, String)> = Vec::new();
    for (config_type, config_data) in config_data_table(&antenna_uplink_arn) {
        let (config_id, _, config_arn) = create_config(
            router,
            &format!("not-found {config_type}"),
            config_data,
            Some(json!({"stage": "created"})),
        )
        .await;
        created.push((config_type.to_string(), config_id, config_arn));
    }
    while let Some((config_type, config_id, config_arn)) = created.pop() {
        let deleted = send(
            router,
            "DELETE",
            &format!("/config/{config_type}/{config_id}"),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case a deleted {config_type} config: expected DeleteConfig 200, got {} {}",
            deleted.status, deleted.body
        );
        rows.push((format!("a deleted {config_type} config"), config_arn));
    }

    // A deleted mission profile.
    let profile_arn =
        create_mission_profile(router, "not-found profile", 180, Some(120), None).await;
    let profile_id = profile_arn
        .rsplit('/')
        .next()
        .expect("a mission profile ARN ends in its id")
        .to_string();
    let deleted = send(
        router,
        "DELETE",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case a deleted mission profile: expected DeleteMissionProfile 200, got {} {}",
        deleted.status, deleted.body
    );
    rows.push(("a deleted mission profile".to_string(), profile_arn));

    // A deleted dataflow endpoint group.
    let group = send(
        router,
        "POST",
        "/dataflowEndpointGroup",
        Some(json!({
            "endpointDetails": [{
                "securityDetails": security_details(),
                "endpoint": {
                    "name": "not-found-endpoint",
                    "address": {"name": "172.31.0.11", "port": 55889},
                    "mtu": 1500,
                },
            }],
        })),
    )
    .await;
    assert_eq!(
        group.status, 200,
        "case a deleted dataflow endpoint group: expected CreateDataflowEndpointGroup 200, \
         got {} {}",
        group.status, group.body
    );
    let group_id = group.string("dataflowEndpointGroupId");
    let group_read = send(
        router,
        "GET",
        &format!("/dataflowEndpointGroup/{group_id}"),
        None,
    )
    .await;
    let group_arn = group_read.string("dataflowEndpointGroupArn");
    let deleted = send(
        router,
        "DELETE",
        &format!("/dataflowEndpointGroup/{group_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case a deleted dataflow endpoint group: expected DeleteDataflowEndpointGroup 200, \
         got {} {}",
        deleted.status, deleted.body
    );
    rows.push(("a deleted dataflow endpoint group".to_string(), group_arn));

    // ARNs that never resolved: a resource kind this service mints under an id it never
    // held, another service's resource, another region's and another account's, and the
    // shortest ARN the model admits.
    let account = engine::emulator_account_id();
    rows.extend([
        (
            "a config that was never created".to_string(),
            ABSENT_CONFIG_ARN.to_string(),
        ),
        (
            "a mission profile that was never created".to_string(),
            ABSENT_MISSION_PROFILE_ARN.to_string(),
        ),
        (
            "a contact that was never reserved".to_string(),
            contact_arn(ABSENT_UUID),
        ),
        (
            "an agent that was never registered".to_string(),
            agent_arn(ABSENT_UUID),
        ),
        (
            "an ephemeris that was never created".to_string(),
            format!("arn:aws:groundstation:{REGION}:{account}:ephemeris/{ABSENT_UUID}"),
        ),
        (
            "a resource of another service".to_string(),
            "arn:aws:s3:::ground-station-downlink".to_string(),
        ),
        (
            "this account's config in another region".to_string(),
            format!("arn:aws:groundstation:eu-west-1:{account}:config/tracking/{ABSENT_UUID}"),
        ),
        (
            "another account's config in this region".to_string(),
            format!("arn:aws:groundstation:{REGION}:210987654321:config/tracking/{ABSENT_UUID}"),
        ),
        (
            "the shortest ARN the model admits".to_string(),
            SHORTEST_ADMITTED_ARN.to_string(),
        ),
    ]);

    rows
}

/// Assert one refusal of the plane: the code, the status and the whole message, plus that
/// nothing the request carried was interpolated into it.
fn assert_not_found(case: &str, operation: &str, arn: &str, reply: &Reply) {
    assert_eq!(
        reply.status, EXPECTED_STATUS,
        "case {case}: {operation} must answer {EXPECTED_STATUS}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        EXPECTED_CODE,
        "case {case}: {operation} must answer {EXPECTED_CODE}, got {}",
        reply.error_code()
    );
    let message = reply.field("message").as_str().unwrap_or_default();
    assert_eq!(
        message, EXPECTED_MESSAGE,
        "case {case}: {operation} must state the one fixed sentence, got {message:?}"
    );
    assert!(
        !message.contains(arn)
            && !message.contains(engine::emulator_account_id())
            && !message.contains(REGION),
        "case {case}: {operation} must interpolate nothing from the request, got {message:?}"
    );
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_resolving_to_nothing_the_same_way() {
    let (router, engine) = harness_with_engine();
    // One row's config transmits on an uplink band, which this account must be approved for
    // before it can be created (and then deleted).
    provision_uplink_approval(&engine);

    // A live resource, so the suite also shows the refusals persisted nothing anywhere.
    let (_, _, live_arn) = create_config(
        &router,
        "untouched by the refusals",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        Some(json!({"stage": "created"})),
    )
    .await;

    for (case, arn) in arns_resolving_to_nothing(&router).await {
        let tagged = tag_resource(&router, &arn, &tags_of(&[("post-delete", "x")])).await;
        assert_not_found(&case, "TagResource", &arn, &tagged);

        let removed = untag_resource(&router, &arn, &["stage"]).await;
        assert_not_found(&case, "UntagResource", &arn, &removed);

        let listed = list_tags(&router, &arn).await;
        assert_not_found(&case, "ListTagsForResource", &arn, &listed);

        // The refused write stored nothing under that ARN: the read of it still refuses
        // rather than reporting a map the plane never accepted.
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.status, EXPECTED_STATUS,
            "case {case}: a refused write must store nothing, got {} {}",
            again.status, again.body
        );
    }

    // The live resource is untouched by every refusal, and the plane still serves it.
    let listed = list_tags(&router, &live_arn).await;
    assert_eq!(
        listed.status, 200,
        "case the live resource: expected ListTagsForResource 200, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.field("tags"),
        &json!({"stage": "created"}),
        "case the live resource: the refusals must leave its tags exactly as they were, got {}",
        listed.body
    );
}
