// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a gateway's CAPABILITY configurations
//! (`UpdateGatewayCapabilityConfiguration` -> `DescribeGatewayCapabilityConfiguration`,
//! and the summaries `DescribeGateway` reports), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a gateway holds one configuration per capability namespace and may hold
//!   several namespaces at once, so a read naming a namespace answers with the
//!   document the last write to THAT namespace supplied, byte for byte, and says
//!   nothing about any other namespace;
//! * a write is the last word on the namespace it names: it replaces the document
//!   already there rather than adding a second entry for it;
//! * after a write the capability is `OUT_OF_SYNC` until the gateway processes the
//!   configuration, and `IN_SYNC` afterwards — the same status whether it is read
//!   through the capability reader or through the gateway's capability summaries;
//! * the configuration is a JSON document, so a body that is not one is refused
//!   and nothing is stored for it;
//! * a gateway id no create minted, and a namespace no write configured, are
//!   modelled 404s rather than a fabricated configuration.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) and the
//! gateway create helpers are the ones the asset and gateway suites already have;
//! this suite reuses them rather than growing a second copy.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, CAPABILITY_SYNC_IN_SYNC, CAPABILITY_SYNC_OUT_OF_SYNC,
    MAX_CAPABILITIES_PER_GATEWAY,
};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router};
use crate::gateway_lifecycle::{greengrass_v2, GATEWAYS};

/// Comfortably past [`ACTIVATION_DELAY`], so the `OUT_OF_SYNC` -> `IN_SYNC`
/// transition the clock owes a written capability has fallen due.
fn past_the_owed_transition() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// Create a gateway named `name` and return its id.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        GATEWAYS,
        Some(json!({
            "gatewayName": name,
            "gatewayPlatform": greengrass_v2(&format!("core-device-{name}"), None),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create gateway {name:?}: expected 201, got {status} {created}"
    );
    let id = created["gatewayId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&id),
        "create gateway {name:?}: gatewayId must be a modelled UUID, got {created}"
    );
    id
}

/// `POST /20200301/gateways/{id}/capability` — the route
/// `UpdateGatewayCapabilityConfiguration` is bound to (smithy_query
/// query="operation" operation="UpdateGatewayCapabilityConfiguration").
fn capability_write_uri(gateway_id: &str) -> String {
    format!("{GATEWAYS}/{gateway_id}/capability")
}

/// `GET /20200301/gateways/{id}/capability/{namespace}` — the route
/// `DescribeGatewayCapabilityConfiguration` is bound to.
fn capability_read_uri(gateway_id: &str, namespace: &str) -> String {
    format!("{GATEWAYS}/{gateway_id}/capability/{namespace}")
}

/// Write `configuration` under `namespace` and return `(status, body)`.
async fn write_capability(
    router: &MultiServiceRouter,
    gateway_id: &str,
    namespace: &str,
    configuration: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &capability_write_uri(gateway_id),
        Some(json!({
            "capabilityNamespace": namespace,
            "capabilityConfiguration": configuration,
        })),
    )
    .await
}

/// One row of the round-trip table: the case name, the capability namespace and
/// the JSON document written under it.
#[derive(Debug)]
struct CapabilityCase {
    label: &'static str,
    namespace: &'static str,
    configuration: String,
}

/// The rows. The namespaces follow the form `capabilityNamespace` documents —
/// `service:capability:version`, with `opcuacollector` and `publisher` as the
/// capability types and 2 (Classic streams, V2 gateways) / 3 (MQTT-enabled, V3
/// gateways) as the versions (smithy_query query="doc"
/// shape="UpdateGatewayCapabilityConfigurationRequest"
/// member="capabilityNamespace") — and the documents differ along what the rule
/// generalises over: JSON form (object, nested object, array, scalar) and size,
/// from the shortest document the modelled 1..10000000 length admits to a long
/// one.
fn capability_cases() -> Vec<CapabilityCase> {
    vec![
        CapabilityCase {
            label: "opcuacollector-v3-empty-object",
            namespace: "iotsitewise:opcuacollector:3",
            configuration: "{}".to_string(),
        },
        CapabilityCase {
            label: "opcuacollector-v2-nested-sources",
            namespace: "iotsitewise:opcuacollector:2",
            configuration: json!({
                "sources": [{
                    "name": "Windfarm",
                    "endpoint": {
                        "endpointUri": "opc.tcp://203.0.113.10:49320",
                        "securityPolicy": "NONE",
                        "identityProvider": { "type": "Anonymous" },
                    },
                    "measurementDataStreamPrefix": "",
                }],
            })
            .to_string(),
        },
        CapabilityCase {
            label: "publisher-v2-array-document",
            namespace: "iotsitewise:publisher:2",
            configuration: json!([{ "SiteWisePublisherConfiguration": { "publishingOrder": "TIME_ORDER" } }])
                .to_string(),
        },
        CapabilityCase {
            label: "publisher-v3-scalar-document",
            namespace: "iotsitewise:publisher:3",
            // A JSON document need not be an object: a scalar is one too, and the
            // service stores whatever document the write supplied.
            configuration: "42".to_string(),
        },
        CapabilityCase {
            label: "opcuacollector-v4-single-character",
            namespace: "iotsitewise:opcuacollector:4",
            // The shortest document the modelled `@length(min: 1)` admits.
            configuration: "0".to_string(),
        },
        CapabilityCase {
            label: "opcuacollector-v5-long-document",
            namespace: "iotsitewise:opcuacollector:5",
            configuration: json!({ "note": "n".repeat(20_000) }).to_string(),
        },
    ]
}

#[tokio::test]
async fn a_written_capability_reads_back_for_every_namespace_and_document() {
    let router = router();
    let gateway = create_gateway(&router, "round-trip").await;

    for case in capability_cases() {
        let (status, written) =
            write_capability(&router, &gateway, case.namespace, &case.configuration).await;
        assert_eq!(
            status, 201,
            "case {:?}: the write expected 201, got {status} {written}",
            case.label
        );
        assert_eq!(
            written["capabilityNamespace"].as_str().unwrap_or(""),
            case.namespace,
            "case {:?}: the write must answer with the namespace it configured, got {written}",
            case.label
        );
        assert_eq!(
            written["capabilitySyncStatus"].as_str().unwrap_or(""),
            CAPABILITY_SYNC_OUT_OF_SYNC,
            "case {:?}: a just-written capability is out of sync, got {written}",
            case.label
        );

        let (status, read) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, case.namespace),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the read expected 200, got {status} {read}",
            case.label
        );
        assert_eq!(
            read["gatewayId"].as_str().unwrap_or(""),
            gateway,
            "case {:?}: the read must name the gateway it was addressed to, got {read}",
            case.label
        );
        assert_eq!(
            read["capabilityNamespace"].as_str().unwrap_or(""),
            case.namespace,
            "case {:?}: the read must name the namespace it was addressed to, got {read}",
            case.label
        );
        assert_eq!(
            read["capabilityConfiguration"].as_str().unwrap_or(""),
            case.configuration,
            "case {:?}: the read must answer with the document the write supplied, got {} characters",
            case.label,
            read["capabilityConfiguration"].as_str().unwrap_or("").len()
        );
        let sync_status = read["capabilitySyncStatus"].as_str().unwrap_or("");
        assert!(
            sync_status == CAPABILITY_SYNC_OUT_OF_SYNC || sync_status == CAPABILITY_SYNC_IN_SYNC,
            "case {:?}: a configured capability is out of sync or in sync, got {sync_status:?}",
            case.label
        );
    }

    // Every namespace is still readable once they are all configured: a write to
    // one says nothing about any other.
    for case in capability_cases() {
        let (status, read) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, case.namespace),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the namespace must survive the later writes, got {status} {read}",
            case.label
        );
        assert_eq!(
            read["capabilityConfiguration"].as_str().unwrap_or(""),
            case.configuration,
            "case {:?}: another namespace's write must not change this document",
            case.label
        );
    }

    // The gateway reports exactly the namespaces that were configured, once each
    // and in the order they were first configured in.
    let (status, described) = call(&router, "GET", &format!("{GATEWAYS}/{gateway}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeGateway expected 200, got {status} {described}"
    );
    let listed: Vec<&str> = described["gatewayCapabilitySummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["capabilityNamespace"].as_str().unwrap_or(""))
                .collect()
        })
        .unwrap_or_default();
    let expected: Vec<&str> = capability_cases()
        .iter()
        .map(|case| case.namespace)
        .collect();
    assert_eq!(
        listed, expected,
        "the gateway must summarise every configured namespace once, got {described}"
    );
}

#[tokio::test]
async fn a_write_replaces_the_document_under_the_namespace_it_names() {
    let router = router();
    let gateway = create_gateway(&router, "replacement").await;
    let namespace = "iotsitewise:opcuacollector:3";

    // Successive documents written to the SAME namespace, differing in form and
    // size so no row can pass by being the same as the previous one.
    let documents = [
        "{}".to_string(),
        json!({ "sources": [] }).to_string(),
        json!({ "sources": [{ "name": "First" }, { "name": "Second" }] }).to_string(),
        json!("a-string-document").to_string(),
        json!({ "note": "x".repeat(5_000) }).to_string(),
    ];

    for (at, document) in documents.iter().enumerate() {
        let (status, written) = write_capability(&router, &gateway, namespace, document).await;
        assert_eq!(
            status, 201,
            "write {at}: expected 201, got {status} {written}"
        );

        let (status, read) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, namespace),
            None,
        )
        .await;
        assert_eq!(status, 200, "read {at}: expected 200, got {status} {read}");
        assert_eq!(
            read["capabilityConfiguration"].as_str().unwrap_or(""),
            document.as_str(),
            "read {at}: the read must answer with the LAST document written"
        );

        // The replacement did not add a second entry for the namespace.
        let (status, described) =
            call(&router, "GET", &format!("{GATEWAYS}/{gateway}"), None).await;
        assert_eq!(
            status, 200,
            "read {at}: DescribeGateway expected 200, got {status} {described}"
        );
        let summaries = described["gatewayCapabilitySummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "read {at}: one namespace is summarised once, got {described}"
        );
        assert_eq!(
            summaries[0]["capabilityNamespace"].as_str().unwrap_or(""),
            namespace,
            "read {at}: the summary must name the configured namespace, got {described}"
        );
    }
}

#[tokio::test]
async fn a_written_capability_is_out_of_sync_until_the_gateway_processes_it() {
    let router = router();
    let gateway = create_gateway(&router, "sync-status").await;

    // One row per namespace, so the transition is pinned for more than the one
    // namespace an example happens to use.
    for namespace in [
        "iotsitewise:opcuacollector:3",
        "iotsitewise:publisher:2",
        "iotsitewise:opcuacollector:2",
    ] {
        let (status, written) = write_capability(&router, &gateway, namespace, "{}").await;
        assert_eq!(
            status, 201,
            "case {namespace:?}: the write expected 201, got {status} {written}"
        );
        assert_eq!(
            written["capabilitySyncStatus"].as_str().unwrap_or(""),
            CAPABILITY_SYNC_OUT_OF_SYNC,
            "case {namespace:?}: the write answers out of sync, got {written}"
        );

        // Straight after the write the gateway has not processed the configuration
        // yet, through either reader.
        let (status, read) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, namespace),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {namespace:?}: the read expected 200, got {status} {read}"
        );
        assert_eq!(
            read["capabilitySyncStatus"].as_str().unwrap_or(""),
            CAPABILITY_SYNC_OUT_OF_SYNC,
            "case {namespace:?}: the capability is out of sync until the gateway \
             processes it, got {read}"
        );
        assert_eq!(
            summary_status(&router, &gateway, namespace).await,
            CAPABILITY_SYNC_OUT_OF_SYNC,
            "case {namespace:?}: the gateway's summary must report the same status \
             as the capability reader"
        );

        // Once the transition the clock owes has fallen due, it is in sync — and
        // reading it again does not change what the next caller sees.
        tokio::time::sleep(past_the_owed_transition()).await;
        let (status, settled) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, namespace),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {namespace:?}: the settled read expected 200, got {status} {settled}"
        );
        assert_eq!(
            settled["capabilitySyncStatus"].as_str().unwrap_or(""),
            CAPABILITY_SYNC_IN_SYNC,
            "case {namespace:?}: a processed capability is in sync, got {settled}"
        );
        assert_eq!(
            summary_status(&router, &gateway, namespace).await,
            CAPABILITY_SYNC_IN_SYNC,
            "case {namespace:?}: the gateway's summary must report the settled status too"
        );
        let (status, again) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, namespace),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {namespace:?}: the second read expected 200, got {status} {again}"
        );
        assert_eq!(
            again, settled,
            "case {namespace:?}: reading a settled capability twice must answer the same, \
             got {again} then {settled}"
        );

        // A fresh write to the SAME namespace puts it back out of sync: the status
        // follows the last write, not the first.
        let (status, rewritten) =
            write_capability(&router, &gateway, namespace, "{\"sources\":[]}").await;
        assert_eq!(
            status, 201,
            "case {namespace:?}: the rewrite expected 201, got {status} {rewritten}"
        );
        assert_eq!(
            rewritten["capabilitySyncStatus"].as_str().unwrap_or(""),
            CAPABILITY_SYNC_OUT_OF_SYNC,
            "case {namespace:?}: a rewritten capability is out of sync again, got {rewritten}"
        );
    }
}

/// The `capabilitySyncStatus` the gateway's own summary reports for `namespace`.
async fn summary_status(
    router: &MultiServiceRouter,
    gateway_id: &str,
    namespace: &str,
) -> String {
    let (status, described) = call(router, "GET", &format!("{GATEWAYS}/{gateway_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeGateway expected 200, got {status} {described}"
    );
    described["gatewayCapabilitySummaries"]
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary["capabilityNamespace"].as_str() == Some(namespace))
                .map(|summary| {
                    summary["capabilitySyncStatus"]
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
        })
        .unwrap_or_else(|| panic!("the gateway must summarise {namespace:?}, got {described}"))
}

#[tokio::test]
async fn a_configuration_that_is_not_a_json_document_is_refused() {
    let router = router();
    let gateway = create_gateway(&router, "json-documents").await;

    // (case, document, accepted) — rows span what a JSON document is and is not:
    // every JSON form on the accepted side, and truncated, trailing-garbage and
    // non-JSON bodies on the refused one.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("an object", "{}", true),
        ("an array", "[]", true),
        ("a string", "\"configured\"", true),
        ("a number", "7", true),
        ("a boolean", "true", true),
        ("null", "null", true),
        ("an object with whitespace", " { \"a\": 1 } ", true),
        ("prose", "not a json document", false),
        ("a truncated object", "{\"sources\":", false),
        ("trailing garbage", "{} and then some", false),
        ("a trailing brace", "{}}", false),
        ("xml", "<capability><source/></capability>", false),
        ("a bare key", "sources", false),
    ];

    for (row, (case, document, accepted)) in rows.into_iter().enumerate() {
        // A namespace of its own per row, so a refused row cannot be hidden by the
        // document a previous row stored.
        let namespace = format!("iotsitewise:opcuacollector:{row}");
        let (status, answer) = write_capability(&router, &gateway, &namespace, document).await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a JSON document expected 201, got {status} {answer}"
            );
            let (status, read) = call(
                &router,
                "GET",
                &capability_read_uri(&gateway, &namespace),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the accepted document must read back, got {status} {read}"
            );
            assert_eq!(
                read["capabilityConfiguration"].as_str().unwrap_or(""),
                document,
                "case {case:?}: the read must answer with the document written, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a body that is not a JSON document expected 400, \
                 got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {answer}"
            );
            // Nothing was stored for the refused row.
            let (status, read) = call(
                &router,
                "GET",
                &capability_read_uri(&gateway, &namespace),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused write must store nothing, got {status} {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_gateway_or_namespace_is_refused() {
    let router = router();
    let gateway = create_gateway(&router, "refusals").await;
    let configured = "iotsitewise:opcuacollector:3";
    let (status, written) = write_capability(&router, &gateway, configured, "{}").await;
    assert_eq!(
        status, 201,
        "the configured capability: expected 201, got {status} {written}"
    );

    // Well-formed gateway ids no create ever minted: one all-f, one that differs
    // from the stored id in a single nibble.
    let mut neighbour: Vec<char> = gateway.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    for unknown in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(unknown),
            "case {unknown:?}: the probe id must itself be modelled"
        );
        let (status, body) = call(
            &router,
            "GET",
            &capability_read_uri(unknown, configured),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: an unknown gateway expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {body}"
        );
        // The refusal states the resource kind and the identifier the REQUEST
        // supplied, written out by hand here as an independent oracle.
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            format!("Gateway with ID {unknown} not found"),
            "case {unknown:?}: unexpected refusal message in {body}"
        );

        let (status, body) = write_capability(&router, unknown, configured, "{}").await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: a write to an unknown gateway expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            format!("Gateway with ID {unknown} not found"),
            "case {unknown:?}: unexpected refusal message in {body}"
        );
    }

    // Namespaces the gateway holds no configuration under: another capability
    // type, another version of the configured one, and another service.
    for unknown in [
        "iotsitewise:publisher:3",
        "iotsitewise:opcuacollector:2",
        "iotsitewise:opcuacollector:30",
        "other:opcuacollector:3",
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &capability_read_uri(&gateway, unknown),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: an unconfigured namespace expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            format!("Gateway Capability with ID {unknown} not found"),
            "case {unknown:?}: unexpected refusal message in {body}"
        );
    }

    // The refusals left the configured capability readable.
    let (status, read) = call(
        &router,
        "GET",
        &capability_read_uri(&gateway, configured),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the configured capability after the refusals: got {status} {read}"
    );
    assert_eq!(
        read["capabilityConfiguration"].as_str().unwrap_or(""),
        "{}",
        "the configured document must survive the refusals, got {read}"
    );
}

#[tokio::test]
async fn a_gateway_holds_at_most_the_bounded_number_of_capabilities() {
    let router = router();
    let gateway = create_gateway(&router, "bounded").await;

    for version in 1..=MAX_CAPABILITIES_PER_GATEWAY {
        let namespace = format!("iotsitewise:opcuacollector:{version}");
        let (status, written) = write_capability(&router, &gateway, &namespace, "{}").await;
        assert_eq!(
            status, 201,
            "namespace {namespace:?}: expected 201, got {status} {written}"
        );
    }

    // Past the cap a write naming a FRESH namespace is the modelled quota refusal.
    let beyond = format!(
        "iotsitewise:opcuacollector:{}",
        MAX_CAPABILITIES_PER_GATEWAY + 1
    );
    let (status, refused) = write_capability(&router, &gateway, &beyond, "{}").await;
    assert_eq!(
        status, 410,
        "namespace {beyond:?}: expected 410 past the cap, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "namespace {beyond:?}: unexpected error code in {refused}"
    );

    // A write REPLACING a namespace already configured adds nothing, so the cap
    // does not refuse it.
    let existing = "iotsitewise:opcuacollector:1";
    let (status, rewritten) = write_capability(&router, &gateway, existing, "[]").await;
    assert_eq!(
        status, 201,
        "namespace {existing:?}: a replacement must still be accepted, got {status} {rewritten}"
    );
    let (status, read) = call(
        &router,
        "GET",
        &capability_read_uri(&gateway, existing),
        None,
    )
    .await;
    assert_eq!(status, 200, "namespace {existing:?}: got {status} {read}");
    assert_eq!(
        read["capabilityConfiguration"].as_str().unwrap_or(""),
        "[]",
        "namespace {existing:?}: the replacement must read back, got {read}"
    );
}
