// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AssociateGatewayToServer`: the link between a gateway and the
//! server (hypervisor) whose virtual machines it backs up.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour, and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: an operation that links two of this service's resources resolves
//! BOTH ARNs in the persisted store, persists the link on the record it writes, and every
//! later read of that record reports it — so an association is visible to `GetGateway`
//! and `ListGateways` as the server's `HypervisorId`, asserting the link that already
//! exists changes nothing, a link to a second server is refused with the modelled
//! `ConflictException`, an ARN naming a resource the store does not hold is refused as the
//! missing resource it is and writes nothing, and the link is a fact about the
//! record it was written on rather than about the pair, so removing the other end does not
//! clear it.
//!
//! The tables below span that class — servers differing in name and host form, one and
//! several gateways per server, the same association sent once and repeated, both ends of
//! the link missing, ARNs differing in resource type, id and length (each published bound
//! of the modelled `ServerArn` and one past it), and every ordering of two servers in a
//! conflict — rather than one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, GATEWAY_NOT_FOUND_MESSAGE, HYPERVISOR_NOT_FOUND_MESSAGE, HYPERVISOR_PENDING,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, never_created_arn, Reply, REGION,
};

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
pub(crate) fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ServerArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="ServerArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`, and the resource-type
/// segment is a single character so that even the 50-character minimum fits.
///
/// `GatewayArn` carries the SAME pattern with `@length(min: 50, max: 180)`, so the sibling
/// suites build their gateway-ARN length cases from here too.
pub(crate) fn arn_of_length(len: usize) -> String {
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

/// The resource id an ARN of this service names — the `hv-0000…` of
/// `arn:aws:backup-gateway:<region>:<account>:hypervisor/hv-0000…`.
///
/// Spelled out here rather than reused from the crate, so the assertion is independent of
/// the code that reports the id.
pub(crate) fn resource_id_of(arn: &str) -> &str {
    arn.rsplit_once('/')
        .map(|(_, id)| id)
        .unwrap_or_else(|| panic!("{arn} names no resource id"))
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned
/// in the `Message` member.
pub(crate) fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

pub(crate) async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &json!({ "Name": name, "Host": host }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

pub(crate) async fn associate(
    router: &MultiServiceRouter,
    gateway_arn: &str,
    server_arn: &str,
) -> Reply {
    call(
        router,
        "AssociateGatewayToServer",
        &json!({ "GatewayArn": gateway_arn, "ServerArn": server_arn }),
    )
    .await
}

/// The `HypervisorId` `GetGateway` reports for `arn`, `None` for a gateway associated with
/// no server.
pub(crate) async fn reported_hypervisor_id(
    router: &MultiServiceRouter,
    arn: &str,
) -> Option<String> {
    let reply = call(router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "GetGateway on {arn} must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/Gateway/HypervisorId")
        .map(str::to_string)
}

/// Every gateway `ListGateways` reports, by ARN, with the `HypervisorId` of the server it
/// is associated with.
pub(crate) async fn listed_hypervisor_ids(
    router: &MultiServiceRouter,
) -> BTreeMap<String, Option<String>> {
    let reply = call(router, "ListGateways", &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "ListGateways must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("Gateways")
        .and_then(Value::as_array)
        .map(|gateways| {
            gateways
                .iter()
                .map(|gateway| {
                    (
                        gateway["GatewayArn"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        gateway
                            .get("HypervisorId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The server ARN the STORE holds the gateway's association under, read out of the store
/// rather than off the wire: this is what an association writes and what a refused one
/// must leave alone.
pub(crate) fn persisted_server_arn(
    engine: &Arc<engine::InMemoryEngine>,
    gateway_arn: &str,
) -> Option<String> {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .gateway(gateway_arn)
        .unwrap_or_else(|| panic!("no gateway stored under {gateway_arn}"))
        .hypervisor_arn
        .clone()
}

/// The persisted `HypervisorState` of the record stored under `arn` — the value an
/// association must not have touched (what a read REPORTS is derived from it and the
/// clock).
pub(crate) fn persisted_hypervisor_state(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> String {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor(arn)
        .map(|record| record.state.clone())
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
}

/// One server to import, and the gateways to associate with it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `Name` — len 1..100, pattern `^[a-zA-Z0-9-]*$`
    /// (`smithy_query(query="shape",shape="ImportHypervisorConfigurationInput")`).
    name: &'static str,
    /// `Host` — an IP address or an FQDN, len 3..128. Unique per row: one host, one
    /// hypervisor.
    host: &'static str,
    /// The display names of the gateways to associate with this server: the rows differ in
    /// how many gateways share one server.
    gateways: &'static [&'static str],
    /// How many times each association is sent: more than once asserts that asserting the
    /// link that already exists succeeds and changes nothing.
    sends: usize,
}

/// The rule: an association resolves both ARNs in the store, persists the link on the
/// gateway record, and every later read of that gateway reports the server's
/// `HypervisorId`.
#[tokio::test]
async fn associating_a_gateway_with_a_server_round_trips_through_the_store() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: [Row; 5] = [
        Row {
            case: "one gateway on an IPv4 host",
            name: "hv-one",
            host: "198.51.100.11",
            gateways: &["gw-one"],
            sends: 1,
        },
        Row {
            case: "two gateways sharing one FQDN host",
            name: "hv-two",
            host: "hypervisor-two.example.com",
            gateways: &["gw-two-a", "gw-two-b"],
            sends: 1,
        },
        Row {
            case: "the same association sent three times",
            name: "hv-three",
            host: "198.51.100.13",
            gateways: &["gw-three"],
            sends: 3,
        },
        Row {
            case: "three gateways, each association sent twice",
            name: "hv-four",
            host: "vcenter-four.example.internal",
            gateways: &["gw-four-a", "gw-four-b", "gw-four-c"],
            sends: 2,
        },
        Row {
            case: "a single-character server name",
            name: "h",
            host: "198.51.100.15",
            gateways: &["gw-five"],
            sends: 1,
        },
    ];

    // What every later read must report, accumulated across the rows: gateway ARN ->
    // the `HypervisorId` of its server.
    let mut expected: BTreeMap<String, Option<String>> = BTreeMap::new();

    for row in &rows {
        let server_arn = import(&router, row.name, row.host).await;
        let server_id = resource_id_of(&server_arn).to_string();
        let state_before = persisted_hypervisor_state(&engine, &server_arn);

        for display_name in row.gateways {
            let gateway_arn = create(&router, display_name).await;
            assert_eq!(
                reported_hypervisor_id(&router, &gateway_arn).await,
                None,
                "case {:?} gateway {display_name:?}: a freshly created gateway is \
                 associated with no server",
                row.case
            );

            for send in 1..=row.sends {
                let reply = associate(&router, &gateway_arn, &server_arn).await;
                assert_eq!(
                    (reply.status, reply.str_at("/GatewayArn")),
                    (200, Some(gateway_arn.as_str())),
                    "case {:?} gateway {display_name:?} send {send}: the association must \
                     answer 200 with the gateway's ARN, got {} {:?}",
                    row.case,
                    reply.status,
                    reply.body
                );
            }

            // The link is persisted on the gateway record and reported by the read side.
            assert_eq!(
                persisted_server_arn(&engine, &gateway_arn).as_deref(),
                Some(server_arn.as_str()),
                "case {:?} gateway {display_name:?}: the store must hold the association \
                 to {server_arn}, got {:?}",
                row.case,
                persisted_server_arn(&engine, &gateway_arn)
            );
            let reported = reported_hypervisor_id(&router, &gateway_arn).await;
            assert_eq!(
                reported.as_deref(),
                Some(server_id.as_str()),
                "case {:?} gateway {display_name:?}: GetGateway must report the server's \
                 HypervisorId {server_id:?}, got {reported:?}",
                row.case
            );
            expected.insert(gateway_arn.clone(), Some(server_id.clone()));
        }

        // Associating a gateway writes to the GATEWAY: the server's own record is not
        // moved along by it.
        assert_eq!(
            persisted_hypervisor_state(&engine, &server_arn),
            state_before,
            "case {:?}: an association must not change the server's persisted state",
            row.case
        );
        assert_eq!(
            state_before, HYPERVISOR_PENDING,
            "case {:?}: an imported server starts PENDING, so the check above compares \
             against the state an import left",
            row.case
        );
    }

    // Every association is visible to the listing too, and no gateway carries another
    // gateway's server.
    assert_eq!(
        listed_hypervisor_ids(&router).await,
        expected,
        "ListGateways must report each gateway with the server it was associated with"
    );
}

/// The rule's refusal half: an ARN naming a resource the store does not hold is refused as
/// the MISSING RESOURCE it is — the modelled `ResourceNotFoundException` at HTTP 400,
/// carrying the `ErrorCode` that names which end of the link is missing and the fixed
/// sentence this service states for that resource type — and nothing is written. This
/// operation declares no `ResourceNotFoundException` of its own
/// (`smithy_query(query="operation",operation="AssociateGatewayToServer")`), and still
/// answers the condition in the shape every other operation of this service answers it in:
/// an ARN that resolves to nothing is a not-found condition, not an invalid request.
#[tokio::test]
async fn an_association_naming_a_resource_the_store_does_not_hold_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let gateway_arn = create(&router, "gw-live").await;
    let server_arn = import(&router, "hv-live", "198.51.100.21").await;
    let deleted_server_arn = import(&router, "hv-deleted", "198.51.100.22").await;
    let deleted = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": deleted_server_arn }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    // (case, GatewayArn, ServerArn, the ErrorCode the refusal must carry, its sentence).
    // Every row is a
    // well-formed ARN of this service, so it reaches the handler rather than the boundary
    // validator; the rows differ in which end of the link is missing, in the resource type
    // the ARN names, in the id, and — for the ServerArn — in length, including each
    // published bound of the modelled shape (len 50..500).
    let rows: [(&str, String, String, &str, &str); 9] = [
        (
            "a gateway that was never created",
            never_created_arn(),
            server_arn.clone(),
            "GatewayNotFound",
            GATEWAY_NOT_FOUND_MESSAGE,
        ),
        (
            "a GatewayArn naming a hypervisor resource",
            server_arn.clone(),
            server_arn.clone(),
            "GatewayNotFound",
            GATEWAY_NOT_FOUND_MESSAGE,
        ),
        (
            "a GatewayArn naming a virtual machine",
            arn_of("vm", "vm-1111111111111111"),
            server_arn.clone(),
            "GatewayNotFound",
            GATEWAY_NOT_FOUND_MESSAGE,
        ),
        (
            "a GatewayArn in another partition",
            format!(
                "arn:aws-us-gov:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000777",
                engine::EMULATOR_ACCOUNT_ID
            ),
            server_arn.clone(),
            "GatewayNotFound",
            GATEWAY_NOT_FOUND_MESSAGE,
        ),
        (
            "a server that was never imported",
            gateway_arn.clone(),
            arn_of("hypervisor", "hv-1111111111111111"),
            "HypervisorNotFound",
            HYPERVISOR_NOT_FOUND_MESSAGE,
        ),
        (
            "a server that was deleted",
            gateway_arn.clone(),
            deleted_server_arn.clone(),
            "HypervisorNotFound",
            HYPERVISOR_NOT_FOUND_MESSAGE,
        ),
        (
            "a ServerArn naming a gateway resource",
            gateway_arn.clone(),
            gateway_arn.clone(),
            "HypervisorNotFound",
            HYPERVISOR_NOT_FOUND_MESSAGE,
        ),
        (
            "a ServerArn at the modelled 50-character minimum",
            gateway_arn.clone(),
            arn_of_length(50),
            "HypervisorNotFound",
            HYPERVISOR_NOT_FOUND_MESSAGE,
        ),
        (
            "a ServerArn at the modelled 500-character maximum",
            gateway_arn.clone(),
            arn_of_length(500),
            "HypervisorNotFound",
            HYPERVISOR_NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, gateway, server, error_code, message) in &rows {
        let refused = associate(&router, gateway, server).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
                message_of(&refused).as_str(),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some(*error_code),
                *message,
            ),
            "case {case:?}: an ARN with no record must be refused with 400 \
             ResourceNotFoundException / ErrorCode {error_code:?} and the sentence \
             {message:?}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused association must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    // A ServerArn one character outside each published bound never reaches the handler:
    // the modelled constraint is refused at the boundary, which states no `ErrorCode`.
    let out_of_bounds: [(&str, String); 2] = [
        ("one character below the 50-character minimum", arn_of_length(49)),
        ("one character past the 500-character maximum", arn_of_length(501)),
    ];
    for (case, server) in &out_of_bounds {
        let refused = associate(&router, &gateway_arn, server).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a ServerArn outside its modelled length must be refused with \
             400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            message_of(&refused).contains("at 'serverArn' failed to satisfy constraint: "),
            "case {case:?}: the refusal must name the member it refused, got {:?}",
            refused.body
        );
    }

    // Nothing was written: the gateway that exists is still associated with no server,
    // whichever way the refusals above were spelled.
    assert_eq!(
        persisted_server_arn(&engine, &gateway_arn),
        None,
        "a refused association must leave the gateway unassociated in the store"
    );
    assert_eq!(
        reported_hypervisor_id(&router, &gateway_arn).await,
        None,
        "a refused association must leave the read side reporting no HypervisorId"
    );

    // And the same request succeeds for the pair that does exist, so the refusals above
    // are about the ARNs and not about the operation.
    let associated = associate(&router, &gateway_arn, &server_arn).await;
    assert_eq!(
        (associated.status, associated.str_at("/GatewayArn")),
        (200, Some(gateway_arn.as_str())),
        "the association of two records that exist must answer 200, got {} {:?}",
        associated.status,
        associated.body
    );
    assert_eq!(
        reported_hypervisor_id(&router, &gateway_arn).await.as_deref(),
        Some(resource_id_of(&server_arn)),
        "the association that succeeded must read back through GetGateway"
    );
}

/// The rule's conflict half: a gateway backs up ONE server — `GatewayDetails` carries a
/// single `HypervisorId` — so associating an associated gateway with a different server is
/// the modelled `ConflictException` (HTTP 409) and leaves the existing link in place.
#[tokio::test]
async fn a_gateway_already_associated_with_another_server_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    let servers = [
        import(&router, "hv-alpha", "198.51.100.31").await,
        import(&router, "hv-beta", "hypervisor-beta.example.com").await,
        import(&router, "hv-gamma", "198.51.100.33").await,
    ];

    // (case, the server the gateway is associated with, the server the second request
    // names). Every ordering of a three-server set, so the refusal keys on the LINK and
    // not on which server came first.
    let rows: [(&str, usize, usize); 3] = [
        ("alpha then beta", 0, 1),
        ("beta then gamma", 1, 2),
        ("gamma then alpha", 2, 0),
    ];

    for (case, first, second) in &rows {
        let gateway_arn = create(&router, &format!("gw-conflict-{first}{second}")).await;
        let held = &servers[*first];
        let other = &servers[*second];

        let associated = associate(&router, &gateway_arn, held).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: the first association must answer 200, got {} {:?}",
            associated.status, associated.body
        );

        let refused = associate(&router, &gateway_arn, other).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                409,
                Some("ConflictException"),
                Some("GatewayAlreadyAssociated"),
            ),
            "case {case:?}: associating an associated gateway with another server must be \
             refused with 409 ConflictException / ErrorCode GatewayAlreadyAssociated, got \
             {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            message_of(&refused).contains(held.as_str()),
            "case {case:?}: the refusal must name the server the gateway is already \
             associated with, got {:?}",
            refused.body
        );
        // The link that was already there is untouched.
        assert_eq!(
            persisted_server_arn(&engine, &gateway_arn).as_deref(),
            Some(held.as_str()),
            "case {case:?}: a refused association must leave the existing link in place"
        );
        assert_eq!(
            reported_hypervisor_id(&router, &gateway_arn).await.as_deref(),
            Some(resource_id_of(held)),
            "case {case:?}: the read side must still report the server the gateway is \
             associated with"
        );

        // Asserting the link that already exists is the same link again: it succeeds and
        // changes nothing.
        let again = associate(&router, &gateway_arn, held).await;
        assert_eq!(
            (again.status, again.str_at("/GatewayArn")),
            (200, Some(gateway_arn.as_str())),
            "case {case:?}: re-asserting the existing association must answer 200, got {} \
             {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            reported_hypervisor_id(&router, &gateway_arn).await.as_deref(),
            Some(resource_id_of(held)),
            "case {case:?}: re-asserting the existing association must change nothing"
        );
    }
}

/// The link is a fact about the GATEWAY, not about the pair: removing the other end does
/// not clear it.
///
/// The link belongs to the gateway and is reported as the gateway's own `HypervisorId`, so
/// a gateway keeps naming the server it was associated with after `DeleteHypervisor`, and
/// `DisassociateGatewayFromServer` is the operation that clears it
/// (`gateway_server_disassociation`). The leftover link resolves no hypervisor: a request
/// naming that ARN is refused as a missing hypervisor like any other
/// (`put_property_mappings_one_answer_for_a_missing_hypervisor`).
#[tokio::test]
async fn deleting_a_server_leaves_the_association_on_the_gateway() {
    let engine = build_engine();
    let router = build_router(&engine);

    let doomed = import(&router, "hv-doomed", "198.51.100.41").await;
    let survivor = import(&router, "hv-survivor", "198.51.100.42").await;

    // Two gateways on the server that goes away and one on the server that stays, so what
    // the deletion does — and does not do — is shown to span every gateway of the removed
    // server and no other.
    let on_doomed = [
        create(&router, "gw-doomed-a").await,
        create(&router, "gw-doomed-b").await,
    ];
    let on_survivor = create(&router, "gw-survivor").await;

    for gateway_arn in on_doomed.iter().chain([&on_survivor]) {
        let server = if gateway_arn == &on_survivor {
            &survivor
        } else {
            &doomed
        };
        let reply = associate(&router, gateway_arn, server).await;
        assert_eq!(
            reply.status, 200,
            "case {gateway_arn}: the association must answer 200, got {} {:?}",
            reply.status, reply.body
        );
    }

    let deleted = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": doomed }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteHypervisor must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    // The server really is gone: the hypervisor read answers as this service answers for a
    // hypervisor it holds no record for.
    let gone = call(
        &router,
        "GetHypervisor",
        &json!({ "HypervisorArn": doomed }),
    )
    .await;
    assert_eq!(
        (gone.status, gone.code.as_deref(), gone.str_at("/ErrorCode")),
        (
            400,
            Some("ResourceNotFoundException"),
            Some("HypervisorNotFound")
        ),
        "the deleted server must be gone from the store, got {} {:?}",
        gone.status,
        gone.body
    );

    for gateway_arn in &on_doomed {
        assert_eq!(
            persisted_server_arn(&engine, gateway_arn).as_deref(),
            Some(doomed.as_str()),
            "case {gateway_arn}: the store must still hold the gateway's association to \
             the server it was associated with"
        );
        assert_eq!(
            reported_hypervisor_id(&router, gateway_arn).await.as_deref(),
            Some(resource_id_of(&doomed)),
            "case {gateway_arn}: the gateway must still report the server it was \
             associated with"
        );
    }
    assert_eq!(
        reported_hypervisor_id(&router, &on_survivor).await.as_deref(),
        Some(resource_id_of(&survivor)),
        "the gateway on the server that stayed must keep its association"
    );

    // And the link still refuses a second server: it is the gateway's association that
    // decides that, which the deletion did not touch.
    let refused = associate(&router, &on_doomed[0], &survivor).await;
    assert_eq!(
        (
            refused.status,
            refused.code.as_deref(),
            refused.str_at("/ErrorCode"),
        ),
        (
            409,
            Some("ConflictException"),
            Some("GatewayAlreadyAssociated"),
        ),
        "an associated gateway must still be refused a second server, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        reported_hypervisor_id(&router, &on_doomed[0])
            .await
            .as_deref(),
        Some(resource_id_of(&doomed)),
        "the refused association must change nothing"
    );
}
