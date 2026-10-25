// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteGateway`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * deleting a gateway removes it and everything its record owns — its
//!   capability configurations and its tags — so no later read resolves it: the
//!   gateway reader, the capability reader and the listing all stop reporting it;
//! * what the gateway reserved is released with it, so the platform device it ran
//!   on hosts a gateway again and its unique name is free for the next create;
//! * a `gatewayId` that resolves to no stored gateway is the modelled 404, which
//!   is what a second delete of the same gateway gets too, and a refused delete
//!   leaves every other gateway exactly as it was.
//!
//! The HTTP harness (`call`, `router`, `router_with_engine`, `error_code`,
//! `is_modelled_id`) and the gateway platform helpers are the ones the asset and
//! gateway suites already have; this suite reuses them rather than growing a
//! second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, RESOURCE_GATEWAY,
};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router, router_with_engine};
use crate::gateway_lifecycle::{greengrass, greengrass_v2, siemens_ie, GATEWAYS};

/// One row of the delete table: a platform type and the capability namespaces
/// configured on the gateway before it is deleted.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// Built from the row's label so each row takes its own device.
    platform: Value,
    /// Namespaces configured before the delete, in the form
    /// `capabilityNamespace` documents (`service:capability:version`).
    namespaces: Vec<&'static str>,
    tags: Option<Value>,
}

/// The rows: every platform member `GatewayPlatform` declares (smithy_query
/// query="shape" shape="GatewayPlatform": `greengrass`, `greengrassV2`,
/// `siemensIE`), crossed with how much the gateway holds when the delete lands —
/// no capability, one, several — and with / without tags.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "greengrassV2-bare",
            platform: greengrass_v2("delete-core-device-bare", None),
            namespaces: Vec::new(),
            tags: None,
        },
        DeleteCase {
            label: "greengrassV2-one-capability",
            platform: greengrass_v2("delete-core-device-one", Some("LINUX_AMD64")),
            namespaces: vec!["iotsitewise:opcuacollector:2"],
            tags: Some(json!({ "stage": "test" })),
        },
        DeleteCase {
            label: "greengrass-two-capabilities",
            platform: greengrass("delete-group"),
            namespaces: vec!["iotsitewise:opcuacollector:2", "iotsitewise:publisher:2"],
            tags: None,
        },
        DeleteCase {
            label: "siemensIE-three-capabilities",
            platform: siemens_ie("delete-siemens-thing"),
            namespaces: vec![
                "iotsitewise:opcuacollector:3",
                "iotsitewise:publisher:3",
                "iotsitewise:opcuacollector:2",
            ],
            tags: Some(json!({ "team": "ops", "env": "edge" })),
        },
    ]
}

/// Create the gateway a row describes and return its id.
async fn create(router: &MultiServiceRouter, case: &DeleteCase, name: &str) -> String {
    let mut body = json!({ "gatewayName": name });
    let object = body.as_object_mut().expect("an object body");
    object.insert("gatewayPlatform".to_string(), case.platform.clone());
    if case.platform["greengrassV2"]["coreDeviceOperatingSystem"].is_string() {
        object.insert("gatewayVersion".to_string(), json!("3"));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }

    let (status, created) = call(router, "POST", GATEWAYS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateGateway expected 201, got {status} {created}",
        case.label
    );
    let id = created["gatewayId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "case {:?}: gatewayId must be a modelled UUID, got {created}",
        case.label
    );
    id
}

/// Configure `namespace` on `gateway_id` with a document naming the row.
async fn configure(
    router: &MultiServiceRouter,
    case_label: &str,
    gateway_id: &str,
    namespace: &str,
) {
    let (status, written) = call(
        router,
        "POST",
        &format!("{GATEWAYS}/{gateway_id}/capability"),
        Some(json!({
            "capabilityNamespace": namespace,
            "capabilityConfiguration": format!("{{\"case\":\"{case_label}\"}}"),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case_label:?}: configuring {namespace:?} expected 201, got {status} {written}"
    );
}

/// Every `gatewayId` `ListGateways` reports, over as many pages as it takes.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    let mut uri = GATEWAYS.to_string();
    loop {
        let (status, page) = call(router, "GET", &uri, None).await;
        assert_eq!(status, 200, "ListGateways expected 200, got {status} {page}");
        for summary in page["gatewaySummaries"].as_array().into_iter().flatten() {
            ids.push(summary["gatewayId"].as_str().unwrap_or("").to_string());
        }
        match page["nextToken"].as_str() {
            Some(token) => uri = format!("{GATEWAYS}?nextToken={token}"),
            None => break,
        }
    }
    ids
}

#[tokio::test]
async fn deleting_a_gateway_forgets_it_and_frees_what_it_reserved() {
    let (router, engine) = router_with_engine();

    for case in delete_cases() {
        let name = format!("gateway-{}", case.label);
        let id = create(&router, &case, &name).await;
        for namespace in &case.namespaces {
            configure(&router, case.label, &id, namespace).await;
        }

        // The gateway is there before the delete: readable, and listed.
        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the gateway must be readable before the delete, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["gatewayCapabilitySummaries"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            case.namespaces.len(),
            "case {:?}: the gateway must hold the capabilities the row configured, got {described}",
            case.label
        );
        assert!(
            listed_ids(&router).await.contains(&id),
            "case {:?}: the gateway {id} must be listed before the delete",
            case.label
        );

        // The delete itself: the modelled 200 with no body of its own
        // (smithy_query query="operation" operation="DeleteGateway": output Unit,
        // code 200).
        let (status, deleted) = call(&router, "DELETE", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteGateway expected 200, got {status} {deleted}",
            case.label
        );

        // Nothing resolves the gateway any more.
        let (status, gone) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {:?}: DescribeGateway after the delete expected 404, got {status} {gone}",
            case.label
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code in {gone}",
            case.label
        );
        assert_eq!(
            gone["message"].as_str().unwrap_or(""),
            not_found_message(RESOURCE_GATEWAY, &id),
            "case {:?}: unexpected refusal message in {gone}",
            case.label
        );
        assert!(
            !listed_ids(&router).await.contains(&id),
            "case {:?}: the gateway {id} must not be listed after the delete",
            case.label
        );

        // Every capability the gateway held goes with it: a namespace that was
        // configured on it resolves to nothing, and the refusal is the absent
        // GATEWAY — the capability reader resolves the gateway first, and this one is
        // gone, so there is no gateway left to hold a configuration.
        for namespace in &case.namespaces {
            let (status, capability) = call(
                &router,
                "GET",
                &format!("{GATEWAYS}/{id}/capability/{namespace}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {:?}: the capability {namespace:?} must be gone, got {status} {capability}",
                case.label
            );
            assert_eq!(
                error_code(&capability),
                "ResourceNotFoundException",
                "case {:?}: unexpected error code in {capability}",
                case.label
            );
            assert_eq!(
                capability["message"].as_str().unwrap_or(""),
                not_found_message(RESOURCE_GATEWAY, &id),
                "case {:?}: unexpected refusal message in {capability}",
                case.label
            );
        }

        // The store holds neither the record nor the device reservation it took,
        // which is what frees the device for the create below. Asserted against the
        // store because the index has no reader on the wire.
        {
            let state = service_state(&*engine).expect("the iotsitewise store");
            let store = state.lock();
            assert!(
                !store.gateways.contains_key(&id),
                "case {:?}: the gateway record {id} survived the delete",
                case.label
            );
            let dangling: Vec<&String> = store
                .gateway_devices
                .iter()
                .filter(|(_, gateway)| *gateway == &id)
                .map(|(device, _)| device)
                .collect();
            assert!(
                dangling.is_empty(),
                "case {:?}: the deleted gateway {id} still reserves {dangling:?}",
                case.label
            );
        }

        // A second delete of the same gateway resolves nothing, exactly as the
        // reader does.
        let (status, again) = call(&router, "DELETE", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {:?}: a second DeleteGateway expected 404, got {status} {again}",
            case.label
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code in {again}",
            case.label
        );

        // Both things the deleted gateway held exclusively — its name and its
        // platform device — are free, so the very same create is accepted again and
        // mints a NEW gateway.
        let replacement = create(&router, &case, &name).await;
        assert_ne!(
            replacement, id,
            "case {:?}: the replacement gateway must be a new resource, not the deleted id",
            case.label
        );
        let (status, described) =
            call(&router, "GET", &format!("{GATEWAYS}/{replacement}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the replacement gateway must be readable, got {status} {described}",
            case.label
        );
        // The replacement starts with no capability: the configurations belonged to
        // the gateway that was deleted, not to its name or its device.
        assert_eq!(
            described["gatewayCapabilitySummaries"],
            json!([]),
            "case {:?}: the replacement must hold no capability, got {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn deleting_a_gateway_that_was_never_created_is_refused() {
    let (router, engine) = router_with_engine();

    // Two gateways the refusals below must leave untouched.
    let mut stored: Vec<String> = Vec::new();
    for case in ["survivor-one", "survivor-two"] {
        let (status, created) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({
                "gatewayName": case,
                "gatewayPlatform": greengrass_v2(&format!("core-device-{case}"), None),
            })),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");
        stored.push(created["gatewayId"].as_str().unwrap_or("").to_string());
    }

    // Well-formed ids no create ever minted: one all-f, one unrelated, and one
    // differing from a stored id in a single nibble. The mutated nibble is taken
    // from the FIRST hex block (positions 0..8), which carries no modelled
    // version or variant nibble, so the probe stays a modelled `ID` whichever id
    // the create minted — and it is flipped to a different hex digit rather than a
    // fixed one, so it cannot accidentally equal the stored id.
    let mut neighbour: Vec<char> = stored[0].chars().collect();
    let at = neighbour[..8]
        .iter()
        .position(|ch| *ch != '0')
        .unwrap_or(0_usize);
    neighbour[at] = if neighbour[at] == '1' { '2' } else { '1' };
    let neighbour: String = neighbour.into_iter().collect();
    assert_ne!(
        neighbour, stored[0],
        "the neighbour probe must differ from the stored id"
    );
    let unknown = [
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0123456b-89ab-4cde-8f01-23456789abcd",
        neighbour.as_str(),
    ];

    for case in unknown {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        let (status, body) = call(&router, "DELETE", &format!("{GATEWAYS}/{case}"), None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            not_found_message(RESOURCE_GATEWAY, case),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }

    // The refused deletes changed nothing: both gateways are still readable, still
    // listed, and still hold their device reservations.
    let listed = listed_ids(&router).await;
    for id in &stored {
        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "the gateway {id} must survive a refused delete, got {status} {described}"
        );
        assert!(
            listed.contains(id),
            "the gateway {id} must still be listed, got {listed:?}"
        );
    }
    let state = service_state(&*engine).expect("the iotsitewise store");
    let store = state.lock();
    assert_eq!(
        store.gateways.len(),
        stored.len(),
        "a refused delete must not remove a gateway, got {:?}",
        store.gateways.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        store.gateway_devices.len(),
        stored.len(),
        "a refused delete must not release a device, got {:?}",
        store.gateway_devices
    );
}

#[tokio::test]
async fn a_delete_touches_only_the_gateway_it_names() {
    let router = router();

    // Several gateways on different devices; one is deleted and the rest must be
    // reported exactly as before.
    let names = ["alpha", "beta", "gamma", "delta"];
    let mut ids: Vec<String> = Vec::new();
    for name in names {
        let (status, created) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({
                "gatewayName": name,
                "gatewayPlatform": greengrass_v2(&format!("core-device-{name}"), None),
            })),
        )
        .await;
        assert_eq!(status, 201, "case {name:?}: expected 201, got {status} {created}");
        ids.push(created["gatewayId"].as_str().unwrap_or("").to_string());
    }

    // Deleted one at a time, in the order they were created: each delete removes
    // exactly its own gateway and leaves every gateway not yet deleted reporting
    // exactly what it reported before.
    let mut remaining: Vec<String> = ids.clone();
    for (at, name) in names.iter().enumerate() {
        let doomed = ids[at].clone();
        remaining.retain(|id| *id != doomed);
        let mut before: Vec<(String, Value)> = Vec::new();
        for id in &remaining {
            let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {name:?}: the gateway {id} must be readable, got {status} {described}"
            );
            before.push((id.clone(), described));
        }

        let (status, deleted) =
            call(&router, "DELETE", &format!("{GATEWAYS}/{doomed}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteGateway expected 200, got {status} {deleted}"
        );

        for (id, described) in before {
            let (status, after) = call(&router, "GET", &format!("{GATEWAYS}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {name:?}: the gateway {id} must survive, got {status} {after}"
            );
            assert_eq!(
                after, described,
                "case {name:?}: the gateway {id} reads back differently after another delete: {after}"
            );
        }

        let listed = listed_ids(&router).await;
        assert!(
            !listed.contains(&doomed),
            "case {name:?}: the deleted gateway {doomed} is still listed, got {listed:?}"
        );
        for id in &remaining {
            assert!(
                listed.contains(id),
                "case {name:?}: the gateway {id} must still be listed, got {listed:?}"
            );
        }
    }

    // Everything has now been deleted, one at a time, so nothing is listed.
    let listed = listed_ids(&router).await;
    assert!(
        listed.is_empty(),
        "every gateway was deleted, so none may be listed, got {listed:?}"
    );
}
