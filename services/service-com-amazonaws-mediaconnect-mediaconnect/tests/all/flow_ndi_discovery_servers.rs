// SPDX-License-Identifier: CC0-1.0

//! How many NDI® discovery servers a flow's `NdiConfig` may declare, end to end over HTTP.
//!
//! The rule under test is the count the model publishes twice — on the member
//! (`smithy_query(query="doc",shape="NdiConfig",member="NdiDiscoveryServers")`: "a list of
//! up to three NDI discovery server configurations") and on the member's target shape
//! (`smithy_query(query="shape",shape="NdiDiscoveryServerConfig")`: "a maximum of 3 servers
//! is allowed"). The config is kept verbatim on the flow record, so a request declaring
//! more than that maximum is refused with the modelled `BadRequestException` (HTTP 400) and
//! stores nothing, on every operation that writes a flow's `NdiConfig`: CreateFlow and
//! UpdateFlow.
//!
//! The rows span the COUNT the rule generalises over — the member absent, an empty list,
//! one, two and three servers (each inside the bound), the published maximum itself, one
//! past it and far past it — crossed with whether the config turns NDI on, since the bound
//! is on what is stored rather than on whether the feature is enabled. A server's own
//! members vary too (with and without the optional `DiscoveryServerPort`), so what is
//! asserted is the count rule and not one recorded body.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_ndi_sizing::plain_source;

/// The documented maximum, written out here rather than read from the crate so these cases
/// check the rule and not the code's own idea of it: "a list of up to three NDI discovery
/// server configurations" / "a maximum of 3 servers is allowed".
const DOCUMENTED_MAX_SERVERS: usize = 3;

/// `count` discovery server configurations, each carrying the two required members
/// (`DiscoveryServerAddress`, `VpcInterfaceAdapter`) and, on every other entry, the optional
/// `DiscoveryServerPort` — so a row's entries are not all the same shape.
fn discovery_servers(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let mut server = json!({
                    "discoveryServerAddress": format!("198.51.100.{}", index + 1),
                    "vpcInterfaceAdapter": format!("adapter-{index}")
                });
                if index % 2 == 0 {
                    // "The port for the NDI discovery server. Defaults to 5959 if a custom
                    // port isn't specified."
                    server["discoveryServerPort"] = json!(5959 + index);
                }
                server
            })
            .collect(),
    )
}

/// One row of the count table: how many servers the request declares (`None` = the member is
/// absent altogether) and whether the rule says a flow may hold it.
struct CountCase {
    case: &'static str,
    servers: Option<usize>,
    accepted: bool,
}

/// The counts the rule generalises over: every count inside the published maximum, the
/// maximum itself, one past it and far past it.
fn count_cases() -> Vec<CountCase> {
    vec![
        CountCase {
            case: "no ndiDiscoveryServers member at all",
            servers: None,
            accepted: true,
        },
        CountCase {
            case: "an empty ndiDiscoveryServers list",
            servers: Some(0),
            accepted: true,
        },
        CountCase {
            case: "one discovery server",
            servers: Some(1),
            accepted: true,
        },
        CountCase {
            case: "two discovery servers",
            servers: Some(2),
            accepted: true,
        },
        CountCase {
            case: "three discovery servers, the published maximum",
            servers: Some(DOCUMENTED_MAX_SERVERS),
            accepted: true,
        },
        CountCase {
            case: "four discovery servers, one past the published maximum",
            servers: Some(DOCUMENTED_MAX_SERVERS + 1),
            accepted: false,
        },
        CountCase {
            case: "twenty discovery servers",
            servers: Some(20),
            accepted: false,
        },
    ]
}

/// Whether the config turns NDI on, and the flow size that pairing needs: NDI ENABLED needs
/// a size that carries NDI (`CreateFlowRequest$FlowSize`), while a config that leaves the
/// feature off is stored at any size — the discovery-server count rule is the same either
/// way.
fn ndi_states() -> Vec<(&'static str, Option<&'static str>, Option<&'static str>)> {
    vec![
        ("ndiState ENABLED on a LARGE flow", Some("ENABLED"), Some("LARGE")),
        ("ndiState DISABLED on a flow of no stated size", Some("DISABLED"), None),
        ("an ndiConfig that names no state", None, None),
    ]
}

/// The `NdiConfig` member for one row: the state the row names, plus the servers it declares.
fn ndi_config(ndi_state: Option<&str>, servers: Option<usize>) -> Value {
    let mut config = json!({ "machineName": "studio-a" });
    if let Some(ndi_state) = ndi_state {
        config["ndiState"] = json!(ndi_state);
    }
    if let Some(servers) = servers {
        config["ndiDiscoveryServers"] = discovery_servers(servers);
    }
    config
}

/// Whether a flow named `name` is in the store, as ListFlows reports it.
async fn is_listed(emulator: &Emulator, case: &str, name: &str) -> bool {
    let (status, code, listed) = emulator.list_flows("").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: ListFlows must answer 200, got {status} {code:?}: {listed}"
    );
    listed["flows"]
        .as_array()
        .is_some_and(|flows| flows.iter().any(|flow| flow["name"].as_str() == Some(name)))
}

/// The bound the crate publishes is the one the model documents, and the refusal it words
/// resolves every count the same way.
#[tokio::test]
async fn the_published_discovery_server_bound_is_the_documented_one() {
    assert_eq!(
        mc::MAX_NDI_DISCOVERY_SERVERS, DOCUMENTED_MAX_SERVERS,
        "case \"published bound\": a maximum of {DOCUMENTED_MAX_SERVERS} servers is allowed, \
         got {}",
        mc::MAX_NDI_DISCOVERY_SERVERS
    );
    assert!(
        mc::ndi_discovery_servers_rejection("NdiConfig", None).is_none(),
        "case \"no config at all\": a request carrying no NdiConfig breaks no count rule"
    );
}

/// CreateFlow stores an `NdiConfig` declaring at most the published maximum of discovery
/// servers, reports exactly what it stored, and refuses a longer list with
/// `BadRequestException` (400), storing no flow.
#[tokio::test]
async fn create_flow_bounds_the_discovery_servers_it_stores() {
    let emulator = Emulator::new();

    for (index, row) in count_cases().into_iter().enumerate() {
        for (state_case, ndi_state, flow_size) in ndi_states() {
            let CountCase {
                case,
                servers,
                accepted,
            } = &row;
            let case = format!("{case} with {state_case}");
            let name = format!("ndi-servers-create-{index}-{}", ndi_state.unwrap_or("unset"));

            let mut body = json!({
                "name": name,
                "source": plain_source("src"),
                "ndiConfig": ndi_config(ndi_state, *servers)
            });
            if let Some(flow_size) = flow_size {
                body["flowSize"] = json!(flow_size);
            }

            let (status, code, created) = emulator.create_flow(body.clone()).await;
            if *accepted {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: this config is one AWS stores, got {status} {code:?}: \
                     {created}"
                );
                // The stored record is what the create described: read it back through
                // DescribeFlow rather than trusting the 201 body.
                let flow_arn = created["flow"]["flowArn"].as_str().unwrap_or_default();
                let (status, code, described) = emulator.describe_flow(flow_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: \
                     {described}"
                );
                let stored = described["flow"]["ndiConfig"]["ndiDiscoveryServers"].clone();
                match servers {
                    Some(count) if *count > 0 => assert_eq!(
                        stored,
                        discovery_servers(*count),
                        "case {case:?}: the stored flow reports the {count} servers the \
                         create declared, got {stored:?}"
                    ),
                    // An absent member stays absent and an empty list carries nothing:
                    // either way the stored flow reports no server.
                    _ => assert!(
                        stored.as_array().is_none_or(|servers| servers.is_empty()),
                        "case {case:?}: the stored flow reports no discovery server, got \
                         {stored:?}"
                    ),
                }
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: a list past the published maximum is refused, got \
                     {status} {code:?}: {created}"
                );
                assert!(
                    !is_listed(&emulator, &case, &name).await,
                    "case {case:?}: a refused create stores no flow, yet {name:?} is listed"
                );
            }
        }
    }
}

/// UpdateFlow replaces a flow's `NdiConfig` only with one declaring at most the published
/// maximum of discovery servers, and refuses a longer list with `BadRequestException` (400),
/// leaving the stored config as it was.
#[tokio::test]
async fn update_flow_bounds_the_discovery_servers_it_stores() {
    let emulator = Emulator::new();

    for (index, row) in count_cases().into_iter().enumerate() {
        let CountCase {
            case,
            servers,
            accepted,
        } = &row;
        let case = case.to_string();

        // The flow under test starts NDI-enabled at a size that carries NDI, holding one
        // discovery server — so what each row changes is only the count.
        let held = ndi_config(Some("ENABLED"), Some(1));
        let (status, code, created) = emulator
            .create_flow(json!({
                "name": format!("ndi-servers-update-{index}"),
                "flowSize": "LARGE",
                "source": plain_source("src"),
                "ndiConfig": held.clone()
            }))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the flow under test must be created, got {status} {code:?}: \
             {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, code, updated) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{flow_arn}"),
                Some(json!({ "ndiConfig": ndi_config(Some("ENABLED"), *servers) })),
            )
            .await;
        if *accepted {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: this config is one AWS stores, got {status} {code:?}: \
                 {updated}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a list past the published maximum is refused, got {status} \
                 {code:?}: {updated}"
            );
        }

        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
        );
        let stored = described["flow"]["ndiConfig"]["ndiDiscoveryServers"].clone();
        if *accepted {
            match servers {
                Some(count) if *count > 0 => assert_eq!(
                    stored,
                    discovery_servers(*count),
                    "case {case:?}: the stored flow reports the {count} servers the update \
                     declared, got {stored:?}"
                ),
                _ => assert!(
                    stored.as_array().is_none_or(|servers| servers.is_empty()),
                    "case {case:?}: the update replaced the config, so the stored flow \
                     reports no discovery server, got {stored:?}"
                ),
            }
        } else {
            assert_eq!(
                stored,
                discovery_servers(1),
                "case {case:?}: a refused update stores nothing, so the one server the flow \
                 was created with is still what it reports, got {stored:?}"
            );
        }
    }
}
