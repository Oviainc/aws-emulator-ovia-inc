// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rules `UpdateGatewayCapabilityConfiguration` follows
//! that the round-trip suite beside it (`gateway_capability_configuration.rs`)
//! does not reach, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rules these suites pin:
//!
//! * a capability configuration belongs to the GATEWAY it was written to, so the
//!   same namespace configured on several gateways holds one document per
//!   gateway and a write to one of them says nothing about any other;
//! * the write changes what a gateway reader reports, so it moves the gateway's
//!   `lastUpdateDate` and leaves its `creationDate` where the create put it;
//! * a request outside one of the operation's modelled constraints is refused as
//!   client input and persists nothing, while the value AT each published bound
//!   is accepted.
//!
//! The HTTP harness (`call`, `router`, `router_with_engine`, `error_code`) and the
//! gateway create / capability-write / capability-read helpers are the ones the
//! asset and capability suites already have; this suite reuses them rather than
//! growing a second copy.

use serde_json::json;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, CAPABILITY_SYNC_OUT_OF_SYNC,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::gateway_capability_configuration::{
    capability_read_uri, create_gateway, write_capability,
};
use crate::gateway_lifecycle::GATEWAYS;

/// The namespaces the isolation table configures on every gateway. They follow
/// the form `capabilityNamespace` documents — `service:capability:version` with
/// `opcuacollector` / `publisher` as the capability types and 2 (Classic streams,
/// V2 gateways) / 3 (MQTT-enabled, V3 gateways) as the versions (smithy_query
/// query="doc" shape="UpdateGatewayCapabilityConfigurationRequest"
/// member="capabilityNamespace").
const SHARED_NAMESPACES: [&str; 3] = [
    "iotsitewise:opcuacollector:3",
    "iotsitewise:opcuacollector:2",
    "iotsitewise:publisher:3",
];

#[tokio::test]
async fn a_capability_configuration_belongs_to_the_gateway_it_was_written_to() {
    let (router, engine) = router_with_engine();

    // Rows: the gateways the same namespaces are configured on, each with a
    // document of its own so no gateway can pass by reading another's. They
    // differ along the JSON form the document takes as well as the gateway.
    let gateways = ["windfarm", "boiler-house", "pump-station"];
    let mut ids = Vec::new();
    for name in gateways {
        ids.push(create_gateway(&router, name).await);
    }

    /// The document gateway `at` configures under namespace `which`.
    fn document(at: usize, which: usize) -> String {
        match which {
            0 => json!({ "sources": [{ "name": format!("source-{at}") }] }).to_string(),
            1 => json!([{ "capability": at }]).to_string(),
            _ => json!(format!("publisher-{at}")).to_string(),
        }
    }

    // Each gateway is written in turn, and after EVERY write every gateway
    // already written still answers with its own document: a write addresses one
    // gateway's record and leaves the others alone.
    for (at, gateway) in ids.iter().enumerate() {
        for (which, namespace) in SHARED_NAMESPACES.iter().enumerate() {
            let expected = document(at, which);
            let (status, written) = write_capability(&router, gateway, namespace, &expected).await;
            assert_eq!(
                status, 201,
                "case {:?}/{namespace:?}: the write expected 201, got {status} {written}",
                gateways[at]
            );
            assert_eq!(
                written["capabilitySyncStatus"].as_str().unwrap_or(""),
                CAPABILITY_SYNC_OUT_OF_SYNC,
                "case {:?}/{namespace:?}: a just-written capability is out of sync, got {written}",
                gateways[at]
            );
        }

        for (earlier, other) in ids.iter().enumerate().take(at + 1) {
            for (which, namespace) in SHARED_NAMESPACES.iter().enumerate() {
                let expected = document(earlier, which);
                let (status, read) =
                    call(&router, "GET", &capability_read_uri(other, namespace), None).await;
                assert_eq!(
                    status, 200,
                    "case {:?}/{namespace:?}: the read expected 200, got {status} {read}",
                    gateways[earlier]
                );
                assert_eq!(
                    read["gatewayId"].as_str().unwrap_or(""),
                    other.as_str(),
                    "case {:?}/{namespace:?}: the read must name the gateway it was \
                     addressed to, got {read}",
                    gateways[earlier]
                );
                assert_eq!(
                    read["capabilityConfiguration"].as_str().unwrap_or(""),
                    expected,
                    "case {:?}/{namespace:?}: a write to another gateway must not change \
                     this document, got {read}",
                    gateways[earlier]
                );
            }
        }
    }

    // The store holds one record per gateway, each with one entry per namespace:
    // the writes never grew a second entry for a namespace, on this gateway or
    // another.
    let state = service_state(&*engine).expect("the iotsitewise store");
    for (at, gateway) in ids.iter().enumerate() {
        let held = state
            .lock()
            .gateways
            .get(gateway)
            .map(|record| record.capabilities.len())
            .unwrap_or_default();
        assert_eq!(
            held,
            SHARED_NAMESPACES.len(),
            "case {:?}: expected {} stored capabilities, got {held}",
            gateways[at],
            SHARED_NAMESPACES.len()
        );
    }
}

#[tokio::test]
async fn a_write_moves_the_gateways_last_update_date_and_leaves_its_creation_date() {
    let router = router();
    let gateway = create_gateway(&router, "timestamps").await;

    let (status, created) = call(&router, "GET", &format!("{GATEWAYS}/{gateway}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeGateway before the write expected 200, got {status} {created}"
    );
    let creation = created["creationDate"].as_f64().unwrap_or(0.0);
    let mut previous = created["lastUpdateDate"].as_f64().unwrap_or(0.0);
    assert!(
        creation > 0.0 && creation == previous,
        "an untouched gateway's lastUpdateDate is its creationDate, got {created}"
    );

    // Rows: a write defining a namespace nothing has configured, a write
    // REPLACING the document under it, and a write defining a second namespace —
    // every form of the write, each of which changes the capability summaries a
    // gateway reader reports.
    let writes = [
        ("defines a namespace", "iotsitewise:opcuacollector:3", "{}"),
        (
            "replaces the document",
            "iotsitewise:opcuacollector:3",
            "{\"sources\":[]}",
        ),
        ("defines a second namespace", "iotsitewise:publisher:2", "[]"),
    ];

    for (case, namespace, document) in writes {
        // The clock must have moved between the writes for the comparison below
        // to say anything, and the emulator reports a real instant rather than a
        // counter.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (status, written) = write_capability(&router, &gateway, namespace, document).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the write expected 201, got {status} {written}"
        );

        let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{gateway}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGateway expected 200, got {status} {described}"
        );
        let last_update = described["lastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            last_update > previous,
            "case {case:?}: the write must move lastUpdateDate past {previous}, got {described}"
        );
        assert_eq!(
            described["creationDate"].as_f64().unwrap_or(0.0),
            creation,
            "case {case:?}: the write must leave creationDate where the create put it, \
             got {described}"
        );
        previous = last_update;
    }

    // A READ does not move it: only a write does.
    let (status, again) = call(&router, "GET", &format!("{GATEWAYS}/{gateway}"), None).await;
    assert_eq!(status, 200, "the second read expected 200, got {status}");
    assert_eq!(
        again["lastUpdateDate"].as_f64().unwrap_or(0.0),
        previous,
        "reading the gateway must not move its lastUpdateDate, got {again}"
    );
}

/// A `capabilityNamespace` of exactly `length` characters that its pattern
/// `^[a-zA-Z]+:[a-zA-Z]+:[0-9]+$` admits: letters, a colon, letters, a colon and
/// digits. Hand-rolled from the member's own constraints (smithy_query
/// query="operation" operation="UpdateGatewayCapabilityConfiguration":
/// `capabilityNamespace` targets `CapabilityNamespace`, len 1..512, pattern
/// `^[a-zA-Z]+:[a-zA-Z]+:[0-9]+$`).
///
/// `length` must leave room for the shortest value that pattern admits — one
/// letter, a colon, one letter, a colon and one digit.
fn namespace_of_length(length: usize) -> String {
    assert!(
        length >= 5,
        "a namespace the pattern admits is at least 5 characters, asked for {length}"
    );
    let digits = 1;
    let letters = length - digits - 2;
    let service = "s".repeat(letters - letters / 2);
    let capability = "C".repeat(letters / 2);
    let namespace = format!("{service}:{capability}:{}", "7".repeat(digits));
    assert_eq!(
        namespace.len(),
        length,
        "the probe namespace must be {length} characters, got {}",
        namespace.len()
    );
    namespace
}

#[tokio::test]
async fn a_write_outside_a_modelled_constraint_is_refused_and_persists_nothing() {
    let (router, engine) = router_with_engine();
    let gateway = create_gateway(&router, "bounds").await;

    // (case, namespace, configuration, accepted). Each row walks one modelled
    // bound of the operation's input: the value AT it, which must be accepted,
    // and the value just outside it, which must not. Sources: smithy_query
    // query="operation" operation="UpdateGatewayCapabilityConfiguration" —
    // `capabilityNamespace` len 1..512 pattern `^[a-zA-Z]+:[a-zA-Z]+:[0-9]+$`,
    // `capabilityConfiguration` len 1..10000000.
    let at_the_top = namespace_of_length(512);
    let past_the_top = namespace_of_length(513);
    let cases: Vec<(&str, String, String, bool)> = vec![
        (
            "a namespace at the top of its length",
            at_the_top.clone(),
            "{}".to_string(),
            true,
        ),
        (
            "a namespace one character past its length",
            past_the_top,
            "{}".to_string(),
            false,
        ),
        (
            "a namespace below its minimum length",
            String::new(),
            "{}".to_string(),
            false,
        ),
        (
            "a namespace with no version segment",
            "iotsitewise:opcuacollector".to_string(),
            "{}".to_string(),
            false,
        ),
        (
            "a namespace whose version is not digits",
            "iotsitewise:opcuacollector:v3".to_string(),
            "{}".to_string(),
            false,
        ),
        (
            "a namespace carrying a character outside its pattern",
            "iotsitewise:opcua-collector:3".to_string(),
            "{}".to_string(),
            false,
        ),
        (
            "a namespace with an empty capability segment",
            "iotsitewise::3".to_string(),
            "{}".to_string(),
            false,
        ),
        (
            "a configuration at the bottom of its length",
            "iotsitewise:publisher:2".to_string(),
            "0".to_string(),
            true,
        ),
        (
            "a configuration below its minimum length",
            "iotsitewise:publisher:3".to_string(),
            String::new(),
            false,
        ),
    ];

    let mut configured = 0usize;
    for (case, namespace, configuration, accepted) in cases {
        let (status, answer) = write_capability(&router, &gateway, &namespace, &configuration).await;
        if accepted {
            configured += 1;
            assert_eq!(
                status, 201,
                "case {case:?}: a value inside the bound is accepted, got {status} {answer}"
            );
            let (status, read) =
                call(&router, "GET", &capability_read_uri(&gateway, &namespace), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the accepted document must read back, got {status} {read}"
            );
            assert_eq!(
                read["capabilityConfiguration"].as_str().unwrap_or(""),
                configuration,
                "case {case:?}: the read must answer with the document written, got {read}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a value outside the bound must be refused, got {status} {answer}"
            );
        }
        // A refused write stores nothing: the gateway holds exactly the
        // capabilities the accepted rows configured.
        let state = service_state(&*engine).expect("the iotsitewise store");
        let held = state
            .lock()
            .gateways
            .get(&gateway)
            .map(|record| record.capabilities.len())
            .unwrap_or_default();
        assert_eq!(
            held, configured,
            "case {case:?}: expected {configured} stored capabilities, got {held}"
        );
    }
}
