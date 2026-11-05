// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisassociateGatewayFromServer`: the removal of the link between a
//! gateway and the server (hypervisor) whose virtual machines it backs up.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the association helpers of
//! `gateway_server_association`, so what is asserted is the wire behaviour and the only way
//! a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: an operation that removes the link between two of this service's
//! resources resolves the record its ARN names in the persisted store, drops the link from
//! that record and nothing else, and every later read of it reports no link — so a cleared
//! gateway reports no `HypervisorId` to `GetGateway` or `ListGateways`, keeps every other
//! member it carried, leaves the server's own record alone (including one the store no
//! longer holds), and may be associated with another server afterwards; an ARN naming no
//! record is refused with the modelled `ResourceNotFoundException` and a record carrying no
//! link with the modelled `ConflictException`, both writing nothing.
//!
//! The tables below span that class — one and several gateways per server, every position
//! of the cleared gateway in a shared server's set, gateways carrying tags, a maintenance
//! window and a bandwidth schedule, the same request repeated, ARNs differing in resource
//! type, partition, id and length (each published bound of the modelled `GatewayArn` and one
//! past it), and every ordering of a three-server set a cleared gateway is moved across —
//! rather than one request.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, gateway_count, issued_activation_key,
    never_created_arn, Reply, REGION,
};
use crate::gateway_server_association::{
    arn_of, arn_of_length, associate, import, listed_hypervisor_ids, message_of,
    persisted_hypervisor_state, persisted_server_arn, reported_hypervisor_id, resource_id_of,
};

/// The `ErrorCode` this service answers with for a gateway that is associated with no
/// server. Spelled out here rather than reused from the crate, so the assertion is
/// independent of the code that produces it.
const GATEWAY_NOT_ASSOCIATED: &str = "GatewayNotAssociated";

async fn disassociate(router: &MultiServiceRouter, gateway_arn: &str) -> Reply {
    call(
        router,
        "DisassociateGatewayFromServer",
        &json!({ "GatewayArn": gateway_arn }),
    )
    .await
}

/// The `Gateway` details `GetGateway` reports for `arn`, as the whole serialised shape: a
/// disassociation has to leave every member of it but `HypervisorId` exactly as it was.
async fn gateway_details(router: &MultiServiceRouter, arn: &str) -> Value {
    let reply = call(router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "GetGateway on {arn} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("Gateway")
        .cloned()
        .unwrap_or_else(|| panic!("no Gateway in {:?}", reply.body))
}

/// The whole `GetBandwidthRateLimitSchedule` body for `arn` — the schedule a
/// disassociation must not touch.
async fn reported_schedule(router: &MultiServiceRouter, arn: &str) -> Value {
    let reply = call(
        router,
        "GetBandwidthRateLimitSchedule",
        &json!({ "GatewayArn": arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "GetBandwidthRateLimitSchedule on {arn} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.body.clone()
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn reported_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let reply = call(
        router,
        "ListTagsForResource",
        &json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ListTagsForResource on {arn} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.tags()
}

/// One server, the gateways associated with it, and which of them the disassociation
/// clears.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `Name` — len 1..100, pattern `^[a-zA-Z0-9-]*$`
    /// (`smithy_query(query="shape",shape="ImportHypervisorConfigurationInput")`).
    name: &'static str,
    /// `Host` — an IP address or an FQDN, len 3..128. Unique per row: one host, one
    /// hypervisor.
    host: &'static str,
    /// The display names of the gateways associated with this server.
    gateways: &'static [&'static str],
    /// The positions in `gateways` whose link is cleared: the rows differ in how many
    /// gateways share the server and in where the cleared ones sit in that set.
    cleared: &'static [usize],
    /// How many times each disassociation is sent. More than one asserts the SECOND call
    /// is refused — the operation is not `@idempotent`, where `DeleteGateway` is
    /// (`smithy_query(query="operation",operation="DisassociateGatewayFromServer,DeleteGateway")`)
    /// — and that the refusal changes nothing.
    sends: usize,
}

/// The rule: clearing a link resolves the gateway in the store, drops the server from that
/// record, and every later read of the gateway reports no `HypervisorId`.
#[tokio::test]
async fn disassociating_a_gateway_from_its_server_round_trips_through_the_store() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: [Row; 5] = [
        Row {
            case: "the only gateway of an IPv4 host",
            name: "hv-one",
            host: "198.51.100.51",
            gateways: &["gw-dis-one"],
            cleared: &[0],
            sends: 1,
        },
        Row {
            case: "the first of two gateways sharing an FQDN host",
            name: "hv-two",
            host: "hypervisor-fifty-two.example.com",
            gateways: &["gw-dis-two-a", "gw-dis-two-b"],
            cleared: &[0],
            sends: 1,
        },
        Row {
            case: "the middle of three gateways",
            name: "hv-three",
            host: "198.51.100.53",
            gateways: &["gw-dis-three-a", "gw-dis-three-b", "gw-dis-three-c"],
            cleared: &[1],
            sends: 1,
        },
        Row {
            case: "every gateway of one server",
            name: "hv-four",
            host: "vcenter-fifty-four.example.internal",
            gateways: &["gw-dis-four-a", "gw-dis-four-b", "gw-dis-four-c"],
            cleared: &[0, 1, 2],
            sends: 1,
        },
        Row {
            case: "a single-character server name, each request sent three times",
            name: "h",
            host: "198.51.100.55",
            gateways: &["gw-dis-five"],
            cleared: &[0],
            sends: 3,
        },
    ];

    // What every later read must report, accumulated across the rows: gateway ARN -> the
    // `HypervisorId` of its server, `None` once the link is cleared.
    let mut expected: BTreeMap<String, Option<String>> = BTreeMap::new();

    for row in &rows {
        let server_arn = import(&router, row.name, row.host).await;
        let server_id = resource_id_of(&server_arn).to_string();
        let state_before = persisted_hypervisor_state(&engine, &server_arn);

        let mut arns = Vec::new();
        for display_name in row.gateways {
            let gateway_arn = create(&router, display_name).await;
            let reply = associate(&router, &gateway_arn, &server_arn).await;
            assert_eq!(
                reply.status, 200,
                "case {:?} gateway {display_name:?}: the association must answer 200, got \
                 {} {:?}",
                row.case, reply.status, reply.body
            );
            expected.insert(gateway_arn.clone(), Some(server_id.clone()));
            arns.push(gateway_arn);
        }

        for position in row.cleared {
            let gateway_arn = &arns[*position];
            for send in 1..=row.sends {
                let reply = disassociate(&router, gateway_arn).await;
                if send == 1 {
                    assert_eq!(
                        (reply.status, reply.str_at("/GatewayArn")),
                        (200, Some(gateway_arn.as_str())),
                        "case {:?} gateway {position}: the disassociation must answer 200 \
                         with the gateway's ARN, got {} {:?}",
                        row.case,
                        reply.status,
                        reply.body
                    );
                } else {
                    // The link is already gone, so there is nothing left to clear.
                    assert_eq!(
                        (
                            reply.status,
                            reply.code.as_deref(),
                            reply.str_at("/ErrorCode"),
                        ),
                        (409, Some("ConflictException"), Some(GATEWAY_NOT_ASSOCIATED)),
                        "case {:?} gateway {position} send {send}: repeating the \
                         disassociation must be refused with 409 ConflictException / \
                         ErrorCode {GATEWAY_NOT_ASSOCIATED}, got {} {:?}",
                        row.case,
                        reply.status,
                        reply.body
                    );
                }
            }

            assert_eq!(
                persisted_server_arn(&engine, gateway_arn),
                None,
                "case {:?} gateway {position}: the store must hold no association for a \
                 cleared gateway",
                row.case
            );
            let reported = reported_hypervisor_id(&router, gateway_arn).await;
            assert_eq!(
                reported, None,
                "case {:?} gateway {position}: GetGateway must report no HypervisorId for \
                 a cleared gateway, got {reported:?}",
                row.case
            );
            expected.insert(gateway_arn.clone(), None);
        }

        // Every gateway the request did not name keeps its link: the clearing is a fact
        // about ONE record, not about the server.
        for (position, gateway_arn) in arns.iter().enumerate() {
            if row.cleared.contains(&position) {
                continue;
            }
            assert_eq!(
                persisted_server_arn(&engine, gateway_arn).as_deref(),
                Some(server_arn.as_str()),
                "case {:?} gateway {position}: a gateway the disassociation did not name \
                 must keep its association",
                row.case
            );
            assert_eq!(
                reported_hypervisor_id(&router, gateway_arn).await.as_deref(),
                Some(server_id.as_str()),
                "case {:?} gateway {position}: a gateway the disassociation did not name \
                 must still report its server",
                row.case
            );
        }

        // Clearing a link writes to the GATEWAY: the server's own record is neither
        // removed nor moved along by it.
        assert_eq!(
            persisted_hypervisor_state(&engine, &server_arn),
            state_before,
            "case {:?}: a disassociation must not change the server's persisted state",
            row.case
        );
        let server_still_there = call(
            &router,
            "GetHypervisor",
            &json!({ "HypervisorArn": server_arn }),
        )
        .await;
        assert_eq!(
            server_still_there.status, 200,
            "case {:?}: the server must still be in the store after a disassociation, got \
             {} {:?}",
            row.case, server_still_there.status, server_still_there.body
        );
    }

    // Every cleared and every kept link is visible to the listing too.
    assert_eq!(
        listed_hypervisor_ids(&router).await,
        expected,
        "ListGateways must report each gateway with the server it is associated with, and \
         none for a gateway whose link was cleared"
    );
}

/// One gateway's other members, which a disassociation must leave exactly as they were.
#[derive(Debug)]
struct ExtrasRow {
    case: &'static str,
    /// Tags to create the gateway with.
    tags: &'static [(&'static str, &'static str)],
    /// `(HourOfDay, MinuteOfHour, DayOfWeek)` for `PutMaintenanceStartTime`, or no window.
    window: Option<(i64, i64, i64)>,
    /// Whether a bandwidth rate-limit schedule is put on the gateway.
    schedule: bool,
}

/// The rule's "and nothing else" half: a disassociation drops the server's link from the
/// record and leaves every other member of it — display name, type, software version, last
/// seen time, maintenance window, bandwidth schedule and tags — exactly as it was.
#[tokio::test]
async fn a_disassociation_changes_only_the_servers_link() {
    let engine = build_engine();
    let router = build_router(&engine);

    let server_arn = import(&router, "hv-extras", "198.51.100.61").await;

    let rows: [ExtrasRow; 3] = [
        ExtrasRow {
            case: "a gateway carrying tags",
            tags: &[("env", "prod"), ("team", "backup")],
            window: None,
            schedule: false,
        },
        ExtrasRow {
            case: "a gateway carrying a weekly maintenance window",
            tags: &[],
            window: Some((3, 30, 2)),
            schedule: false,
        },
        ExtrasRow {
            case: "a gateway carrying tags, a window and a bandwidth schedule",
            tags: &[("env", "staging")],
            window: Some((23, 59, 6)),
            schedule: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let tags: BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let created = call(
            &router,
            "CreateGateway",
            &create_body(
                &format!("gw-extras-{index}"),
                &issued_activation_key(&router).await,
                &tags,
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateGateway must answer 200, got {} {:?}",
            row.case, created.status, created.body
        );
        let gateway_arn = created.gateway_arn().to_string();

        let associated = associate(&router, &gateway_arn, &server_arn).await;
        assert_eq!(
            associated.status, 200,
            "case {:?}: the association must answer 200, got {} {:?}",
            row.case, associated.status, associated.body
        );

        if let Some((hour, minute, day_of_week)) = row.window {
            let put = call(
                &router,
                "PutMaintenanceStartTime",
                &json!({
                    "GatewayArn": gateway_arn,
                    "HourOfDay": hour,
                    "MinuteOfHour": minute,
                    "DayOfWeek": day_of_week,
                }),
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
                row.case, put.status, put.body
            );
        }
        if row.schedule {
            // Every member of an interval is modelled required, and the rates are bounded
            // at `AverageUploadRateLimitInBitsPerSec` range 51200..8000000000000
            // (`smithy_query(query="shape",shape="BandwidthRateLimitInterval")`).
            let put = call(
                &router,
                "PutBandwidthRateLimitSchedule",
                &json!({
                    "GatewayArn": gateway_arn,
                    "BandwidthRateLimitIntervals": [{
                        "AverageUploadRateLimitInBitsPerSec": 51_200,
                        "StartHourOfDay": 1,
                        "StartMinuteOfHour": 0,
                        "EndHourOfDay": 5,
                        "EndMinuteOfHour": 59,
                        "DaysOfWeek": [1, 3, 5],
                    }],
                }),
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {:?}: PutBandwidthRateLimitSchedule must answer 200, got {} {:?}",
                row.case, put.status, put.body
            );
        }

        let details_before = gateway_details(&router, &gateway_arn).await;
        let schedule_before = reported_schedule(&router, &gateway_arn).await;
        let tags_before = reported_tags(&router, &gateway_arn).await;
        assert_eq!(
            tags_before, tags,
            "case {:?}: the gateway must carry the tags it was created with",
            row.case
        );

        let cleared = disassociate(&router, &gateway_arn).await;
        assert_eq!(
            (cleared.status, cleared.str_at("/GatewayArn")),
            (200, Some(gateway_arn.as_str())),
            "case {:?}: the disassociation must answer 200 with the gateway's ARN, got {} \
             {:?}",
            row.case,
            cleared.status,
            cleared.body
        );

        // The details afterwards are the details before, minus the server's id — every
        // other member, including the last seen time, is untouched.
        let mut expected_details = details_before.clone();
        expected_details
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {:?}: GetGateway must report a structure", row.case))
            .remove("HypervisorId")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: an associated gateway must report a HypervisorId, got \
                     {details_before:?}",
                    row.case
                )
            });
        let details_after = gateway_details(&router, &gateway_arn).await;
        assert_eq!(
            details_after, expected_details,
            "case {:?}: a disassociation must change nothing but the HypervisorId, got \
             {details_after:?}",
            row.case
        );
        assert_eq!(
            reported_schedule(&router, &gateway_arn).await,
            schedule_before,
            "case {:?}: a disassociation must leave the bandwidth schedule alone",
            row.case
        );
        assert_eq!(
            reported_tags(&router, &gateway_arn).await,
            tags_before,
            "case {:?}: a disassociation must leave the tags alone",
            row.case
        );
    }
}

/// The rule's refusal half: an ARN naming no gateway this store holds is refused with the
/// modelled `ResourceNotFoundException` (HTTP 400, `ErrorCode` `GatewayNotFound`), a
/// gateway associated with no server with the modelled `ConflictException` (HTTP 409)
/// naming that condition, and neither writes anything.
#[tokio::test]
async fn a_disassociation_naming_nothing_to_clear_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let server_arn = import(&router, "hv-refusals", "198.51.100.71").await;
    // The gateway whose link must survive every refusal below.
    let associated_arn = create(&router, "gw-refusals-linked").await;
    let linked = associate(&router, &associated_arn, &server_arn).await;
    assert_eq!(
        linked.status, 200,
        "the association must answer 200, got {} {:?}",
        linked.status, linked.body
    );

    let deleted_arn = create(&router, "gw-refusals-deleted").await;
    let deleted = call(
        &router,
        "DeleteGateway",
        &json!({ "GatewayArn": deleted_arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    // Every row is a well-formed `GatewayArn`, so it reaches the handler rather than the
    // boundary validator; the rows differ in the resource type the ARN names, in its
    // partition, in its id and in its length, including each published bound of the
    // modelled shape (len 50..180).
    let not_found: [(&str, String); 7] = [
        ("a gateway that was never created", never_created_arn()),
        ("a gateway that was deleted", deleted_arn.clone()),
        (
            "an ARN naming a hypervisor resource",
            server_arn.clone(),
        ),
        (
            "an ARN naming a virtual machine",
            arn_of("vm", "vm-2222222222222222"),
        ),
        (
            "an ARN in another partition",
            format!(
                "arn:aws-cn:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000888",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "an ARN at the modelled 50-character minimum",
            arn_of_length(50),
        ),
        (
            "an ARN at the modelled 180-character maximum",
            arn_of_length(180),
        ),
    ];

    for (case, arn) in &not_found {
        let refused = disassociate(&router, arn).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("GatewayNotFound"),
            ),
            "case {case:?}: an ARN with no gateway must be refused with 400 \
             ResourceNotFoundException / GatewayNotFound, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            message_of(&refused),
            "The specified gateway was not found.",
            "case {case:?}: the refusal must state the one fixed sentence this service \
             words a missing gateway with, got {:?}",
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused disassociation must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    // A gateway that exists but is associated with no server: there is no link to remove,
    // and no server the request could be naming.
    let never_associated = create(&router, "gw-refusals-unlinked").await;
    let once_associated = create(&router, "gw-refusals-cleared").await;
    let associated_once = associate(&router, &once_associated, &server_arn).await;
    assert_eq!(
        associated_once.status, 200,
        "the association must answer 200, got {} {:?}",
        associated_once.status, associated_once.body
    );
    let first = disassociate(&router, &once_associated).await;
    assert_eq!(
        first.status, 200,
        "the first disassociation must answer 200, got {} {:?}",
        first.status, first.body
    );

    let conflicts: [(&str, &String); 2] = [
        ("a gateway that was never associated", &never_associated),
        (
            "a gateway whose link was already cleared",
            &once_associated,
        ),
    ];
    for (case, arn) in &conflicts {
        let refused = disassociate(&router, arn).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (409, Some("ConflictException"), Some(GATEWAY_NOT_ASSOCIATED)),
            "case {case:?}: a gateway with no server must be refused with 409 \
             ConflictException / ErrorCode {GATEWAY_NOT_ASSOCIATED}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            !message_of(&refused).is_empty(),
            "case {case:?}: the refusal must state why it refused, got {:?}",
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused disassociation must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    // An ARN one character outside each published bound never reaches the handler: the
    // modelled constraint is refused at the boundary, which states no `ErrorCode`.
    let out_of_bounds: [(&str, String); 2] = [
        (
            "one character below the 50-character minimum",
            arn_of_length(49),
        ),
        (
            "one character past the 180-character maximum",
            arn_of_length(181),
        ),
    ];
    for (case, arn) in &out_of_bounds {
        let refused = disassociate(&router, arn).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a GatewayArn outside its modelled length must be refused with \
             400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            message_of(&refused).contains("at 'gatewayArn' failed to satisfy constraint: "),
            "case {case:?}: the refusal must name the member it refused, got {:?}",
            refused.body
        );
    }

    // Nothing was written by any refusal: the gateway that is associated still is, the
    // cleared one is still cleared, and the store holds exactly the gateways that were
    // created and not deleted.
    assert_eq!(
        persisted_server_arn(&engine, &associated_arn).as_deref(),
        Some(server_arn.as_str()),
        "a refused disassociation must leave another gateway's association in place"
    );
    assert_eq!(
        reported_hypervisor_id(&router, &associated_arn)
            .await
            .as_deref(),
        Some(resource_id_of(&server_arn)),
        "a refused disassociation must leave the read side reporting that association"
    );
    assert_eq!(
        persisted_server_arn(&engine, &once_associated),
        None,
        "a refused disassociation must leave a cleared gateway cleared"
    );
    assert_eq!(
        gateway_count(&engine),
        3,
        "a disassociation neither creates nor removes a gateway: the three gateways still \
         in the store are the four created minus the one deleted"
    );

    // And the operation succeeds for the gateway that does have a server, so the refusals
    // above are about what the store holds and not about the operation.
    let cleared = disassociate(&router, &associated_arn).await;
    assert_eq!(
        (cleared.status, cleared.str_at("/GatewayArn")),
        (200, Some(associated_arn.as_str())),
        "the disassociation of a gateway that has a server must answer 200, got {} {:?}",
        cleared.status,
        cleared.body
    );
    assert_eq!(
        reported_hypervisor_id(&router, &associated_arn).await,
        None,
        "the disassociation that succeeded must read back through GetGateway"
    );
}

/// What a cleared link makes possible: the conflict `AssociateGatewayToServer` answers for
/// an associated gateway keys on the LINK, so a gateway whose link was cleared may be
/// associated with another server — including when the server it was associated with is no
/// longer in the store, which does not clear the link by itself.
#[tokio::test]
async fn a_cleared_gateway_can_be_associated_with_another_server() {
    let engine = build_engine();
    let router = build_router(&engine);

    let servers = [
        import(&router, "hv-move-alpha", "198.51.100.81").await,
        import(&router, "hv-move-beta", "hypervisor-move-beta.example.com").await,
        import(&router, "hv-move-gamma", "198.51.100.83").await,
    ];

    // (case, the server the gateway starts on, the server it is moved to). Every ordering
    // of a three-server set, so what the move depends on is the cleared link and not which
    // server came first.
    let rows: [(&str, usize, usize); 3] = [
        ("alpha then beta", 0, 1),
        ("beta then gamma", 1, 2),
        ("gamma then alpha", 2, 0),
    ];

    for (case, first, second) in &rows {
        let gateway_arn = create(&router, &format!("gw-move-{first}{second}")).await;
        let held = &servers[*first];
        let other = &servers[*second];

        let associated = associate(&router, &gateway_arn, held).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: the first association must answer 200, got {} {:?}",
            associated.status, associated.body
        );
        // While the link is there, another server is refused.
        let refused = associate(&router, &gateway_arn, other).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: an associated gateway must be refused another server, got {} \
             {:?}",
            refused.status, refused.body
        );

        let cleared = disassociate(&router, &gateway_arn).await;
        assert_eq!(
            (cleared.status, cleared.str_at("/GatewayArn")),
            (200, Some(gateway_arn.as_str())),
            "case {case:?}: the disassociation must answer 200 with the gateway's ARN, got \
             {} {:?}",
            cleared.status,
            cleared.body
        );

        // With the link gone, the same request that was refused now succeeds.
        let moved = associate(&router, &gateway_arn, other).await;
        assert_eq!(
            (moved.status, moved.str_at("/GatewayArn")),
            (200, Some(gateway_arn.as_str())),
            "case {case:?}: a cleared gateway must be associable with another server, got \
             {} {:?}",
            moved.status,
            moved.body
        );
        assert_eq!(
            reported_hypervisor_id(&router, &gateway_arn).await.as_deref(),
            Some(resource_id_of(other)),
            "case {case:?}: the moved gateway must report its new server"
        );
    }

    // A gateway keeps naming a server that was deleted, and clearing that link is what the
    // disassociation does for it too: the record it writes to is the GATEWAY's.
    let doomed = import(&router, "hv-move-doomed", "198.51.100.84").await;
    let orphaned = create(&router, "gw-move-orphaned").await;
    let associated = associate(&router, &orphaned, &doomed).await;
    assert_eq!(
        associated.status, 200,
        "the association must answer 200, got {} {:?}",
        associated.status, associated.body
    );
    let removed = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": doomed }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        persisted_server_arn(&engine, &orphaned).as_deref(),
        Some(doomed.as_str()),
        "deleting the server must leave the gateway's link in place"
    );

    let cleared = disassociate(&router, &orphaned).await;
    assert_eq!(
        (cleared.status, cleared.str_at("/GatewayArn")),
        (200, Some(orphaned.as_str())),
        "a gateway naming a server the store no longer holds must still be disassociable, \
         got {} {:?}",
        cleared.status,
        cleared.body
    );
    assert_eq!(
        persisted_server_arn(&engine, &orphaned),
        None,
        "the disassociation must clear the link to the removed server"
    );
    let again = disassociate(&router, &orphaned).await;
    assert_eq!(
        (again.status, again.str_at("/ErrorCode")),
        (409, Some(GATEWAY_NOT_ASSOCIATED)),
        "with the link gone there is nothing left to clear, got {} {:?}",
        again.status,
        again.body
    );
}
