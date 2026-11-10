// SPDX-License-Identifier: CC0-1.0

//! The NDI® source-name prefix a flow's `NdiConfig` is stored with, end to end over HTTP.
//!
//! The rule under test is published by the member itself
//! (`smithy_query(query="doc",shape="NdiConfig",member="MachineName")`): "a prefix for the
//! names of the NDI sources that the flow creates. If a custom name isn't specified,
//! MediaConnect generates a unique 12-character ID as the prefix". So `MachineName` is a
//! SERVICE-ASSIGNED member of a stored flow, not an echo of the request: a config stored
//! without a custom prefix carries the generated 12-character ID, every later read reports
//! it, and a prefix the caller named is kept exactly as given.
//!
//! The rule binds every operation that writes a flow's `NdiConfig` — CreateFlow and
//! UpdateFlow — so each has its own table below. The rows span what the rule generalises
//! over: whether the request names a prefix at all (absent, empty, custom), the FORM of a
//! custom prefix (short, exactly the generated width, longer), and the rest of the config
//! around it (`NdiState` ENABLED / DISABLED / unset, with and without a discovery server),
//! since the prefix is assigned whatever the config says. The generated prefix is also
//! asserted as an identity: unique per flow, and the same value on every read of one flow,
//! including after an update rewrites its config.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_ndi_sizing::plain_source;

/// The width the documentation states for the prefix the service generates, written out
/// here rather than read from the crate so these cases check the rule and not the code's
/// own idea of it: "MediaConnect generates a unique 12-character ID as the prefix".
const DOCUMENTED_ID_LEN: usize = 12;

/// What the rule says a row's stored `MachineName` is.
#[derive(Debug)]
enum Prefix {
    /// The request named a custom prefix, which is kept as given.
    Kept(&'static str),
    /// The request named none, so the service assigns the generated ID.
    Generated,
}

/// One row of the prefix table: the `NdiConfig` a request carries, and what the rule says
/// the stored config's `MachineName` is.
struct PrefixCase {
    case: &'static str,
    /// `None` = the request carries no `machineName` member at all.
    machine_name: Option<&'static str>,
    /// `None` = the config names no `ndiState`.
    ndi_state: Option<&'static str>,
    /// Whether the config also declares a discovery server, so the prefix rule is asserted
    /// on configs that are not all the same shape.
    discovery_server: bool,
    expected: Prefix,
}

/// The ways a request can speak (or not speak) about the prefix, crossed with the rest of
/// the config around it.
fn prefix_cases() -> Vec<PrefixCase> {
    vec![
        PrefixCase {
            case: "no machineName, ndiState ENABLED",
            machine_name: None,
            ndi_state: Some("ENABLED"),
            discovery_server: false,
            expected: Prefix::Generated,
        },
        PrefixCase {
            case: "no machineName, ndiState DISABLED",
            machine_name: None,
            ndi_state: Some("DISABLED"),
            discovery_server: false,
            expected: Prefix::Generated,
        },
        PrefixCase {
            case: "no machineName and no ndiState at all",
            machine_name: None,
            ndi_state: None,
            discovery_server: false,
            expected: Prefix::Generated,
        },
        PrefixCase {
            case: "no machineName, with a discovery server declared",
            machine_name: None,
            ndi_state: Some("ENABLED"),
            discovery_server: true,
            expected: Prefix::Generated,
        },
        PrefixCase {
            case: "an empty machineName, which names no custom prefix",
            machine_name: Some(""),
            ndi_state: Some("ENABLED"),
            discovery_server: false,
            expected: Prefix::Generated,
        },
        PrefixCase {
            case: "a short custom machineName",
            machine_name: Some("studio-a"),
            ndi_state: Some("ENABLED"),
            discovery_server: false,
            expected: Prefix::Kept("studio-a"),
        },
        PrefixCase {
            // Exactly as wide as the generated ID, so width alone cannot tell the two apart.
            case: "a custom machineName of the generated width",
            machine_name: Some("cam-control1"),
            ndi_state: Some("DISABLED"),
            discovery_server: false,
            expected: Prefix::Kept("cam-control1"),
        },
        PrefixCase {
            case: "a custom machineName longer than the generated width",
            machine_name: Some("north-studio-gallery-b"),
            ndi_state: None,
            discovery_server: true,
            expected: Prefix::Kept("north-studio-gallery-b"),
        },
    ]
}

/// The `NdiConfig` one row sends.
fn ndi_config(row: &PrefixCase) -> Value {
    let mut config = json!({});
    if let Some(machine_name) = row.machine_name {
        config["machineName"] = json!(machine_name);
    }
    if let Some(ndi_state) = row.ndi_state {
        config["ndiState"] = json!(ndi_state);
    }
    if row.discovery_server {
        // `NdiDiscoveryServerConfig`'s two required members
        // (`smithy_query(query="shape",shape="NdiDiscoveryServerConfig")`).
        config["ndiDiscoveryServers"] = json!([{
            "discoveryServerAddress": "198.51.100.9",
            "vpcInterfaceAdapter": "adapter-0"
        }]);
    }
    config
}

/// The flow size a row needs: a config that turns NDI on exists only at an NDI-capable
/// size (`CreateFlowRequest$FlowSize`), while any other config is stored at any size — so
/// the rows that leave the feature off also exercise a flow of no stated size.
fn flow_size(row: &PrefixCase) -> Option<&'static str> {
    (row.ndi_state == Some("ENABLED")).then_some("LARGE")
}

/// Why `prefix` is not the ID the service generates, or `None` when it is one: the
/// documented width, and an ID's own characters.
///
/// `pub(crate)` because `tests/all/update_flow.rs` asserts the same rule about the config
/// its own NDI table stores, and one rule is one judgement.
pub(crate) fn generated_id_rejection(prefix: &str) -> Option<String> {
    if prefix.chars().count() != DOCUMENTED_ID_LEN {
        return Some(format!(
            "a generated prefix is a {DOCUMENTED_ID_LEN}-character ID, and {prefix:?} is \
             {} characters",
            prefix.chars().count()
        ));
    }
    if !prefix.chars().all(|character| character.is_alphanumeric()) {
        return Some(format!("{prefix:?} is not an ID"));
    }
    None
}

/// The `machineName` of the `NdiConfig` the flow `flow_arn` is STORED with, read back
/// through DescribeFlow rather than from the response of the call that wrote it.
async fn stored_machine_name(emulator: &Emulator, case: &str, flow_arn: &str) -> String {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["ndiConfig"]["machineName"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the stored NdiConfig carries a machineName, got {:?}",
                described["flow"]["ndiConfig"]
            )
        })
        .to_string()
}

/// The width the crate publishes for the generated prefix is the documented one.
#[tokio::test]
async fn the_published_generated_prefix_width_is_the_documented_one() {
    assert_eq!(
        mc::NDI_MACHINE_NAME_LEN, DOCUMENTED_ID_LEN,
        "case \"published width\": MediaConnect generates a unique {DOCUMENTED_ID_LEN}-\
         character ID as the prefix, got {}",
        mc::NDI_MACHINE_NAME_LEN
    );
}

/// CreateFlow stores — and reports — the prefix the request named, and the generated
/// 12-character ID for a request that named none. The generated ID is unique per flow.
#[tokio::test]
async fn create_flow_assigns_the_ndi_machine_name_prefix() {
    let emulator = Emulator::new();
    let mut generated: BTreeSet<String> = BTreeSet::new();

    for (index, row) in prefix_cases().into_iter().enumerate() {
        let case = row.case;
        let name = format!("ndi-prefix-create-{index}");
        let mut body = json!({
            "name": name,
            "source": plain_source("src"),
            "ndiConfig": ndi_config(&row)
        });
        if let Some(size) = flow_size(&row) {
            body["flowSize"] = json!(size);
        }

        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: this config is one AWS stores, got {status} {code:?}: {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The create's own answer is the stored record, so the prefix is reported there too.
        let answered = created["flow"]["ndiConfig"]["machineName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stored = stored_machine_name(&emulator, case, &flow_arn).await;
        assert_eq!(
            answered, stored,
            "case {case:?}: CreateFlow reports the prefix the flow is stored with, got \
             {answered:?} then {stored:?}"
        );

        match row.expected {
            Prefix::Kept(expected) => assert_eq!(
                stored, expected,
                "case {case:?}: a prefix the request named is kept as given, got {stored:?}"
            ),
            Prefix::Generated => {
                assert!(
                    generated_id_rejection(&stored).is_none(),
                    "case {case:?}: the service assigns a generated prefix: {}, got \
                     {stored:?}",
                    generated_id_rejection(&stored).unwrap_or_default()
                );
                assert!(
                    generated.insert(stored.clone()),
                    "case {case:?}: a generated prefix is unique per flow, yet {stored:?} \
                     was already assigned to another flow"
                );
            }
        }

        // A read reports the same prefix every time: it is the flow's identity, not a value
        // minted per call.
        let again = stored_machine_name(&emulator, case, &flow_arn).await;
        assert_eq!(
            again, stored,
            "case {case:?}: every read reports one prefix, got {stored:?} then {again:?}"
        );
    }
}

/// UpdateFlow stores a replacement `NdiConfig` under the same rule: the prefix the request
/// names, or the generated ID of the flow it is updating — which is the SAME ID the create
/// assigned, so a flow that never names a custom prefix keeps one for its whole life.
#[tokio::test]
async fn update_flow_assigns_the_ndi_machine_name_prefix() {
    let emulator = Emulator::new();

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "ndi-prefix-update",
            "flowSize": "LARGE",
            "source": plain_source("src"),
            "ndiConfig": { "ndiState": "ENABLED" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"the flow under test\": expected 201, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let assigned = stored_machine_name(&emulator, "the flow under test", &flow_arn).await;
    assert!(
        generated_id_rejection(&assigned).is_none(),
        "case \"the flow under test\": the create assigns a generated prefix: {}, got \
         {assigned:?}",
        generated_id_rejection(&assigned).unwrap_or_default()
    );

    // Each row updates the SAME flow in order, so the rows also assert that the flow's own
    // generated prefix survives a config that named a custom one and was then replaced.
    // `None` as the config means the request does not carry the member at all.
    let rows: Vec<(&str, Option<Value>, Prefix)> = vec![
        (
            "a replacement config naming no machineName",
            Some(json!({ "ndiState": "ENABLED" })),
            Prefix::Generated,
        ),
        (
            "a replacement config naming a custom machineName",
            Some(json!({ "ndiState": "ENABLED", "machineName": "gallery-b" })),
            Prefix::Kept("gallery-b"),
        ),
        (
            "a request that does not touch the config at all",
            None,
            Prefix::Kept("gallery-b"),
        ),
        (
            "a replacement config whose machineName is empty",
            Some(json!({ "ndiState": "DISABLED", "machineName": "" })),
            Prefix::Generated,
        ),
        (
            "a replacement config naming no machineName again",
            Some(json!({ "ndiState": "ENABLED" })),
            Prefix::Generated,
        ),
    ];

    for (case, config, expected) in rows {
        let body = match config {
            Some(config) => json!({ "ndiConfig": config }),
            // A request that touches another member only, so the stored config stands.
            None => json!({ "flowSize": "LARGE_4X" }),
        };
        let (status, code, updated) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202, got {status} {code:?}: {updated}"
        );

        let answered = updated["flow"]["ndiConfig"]["machineName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stored = stored_machine_name(&emulator, case, &flow_arn).await;
        assert_eq!(
            answered, stored,
            "case {case:?}: UpdateFlow reports the prefix the flow is stored with, got \
             {answered:?} then {stored:?}"
        );
        match expected {
            Prefix::Kept(custom) => assert_eq!(
                stored, custom,
                "case {case:?}: a prefix the request named is kept as given, got {stored:?}"
            ),
            Prefix::Generated => assert_eq!(
                stored, assigned,
                "case {case:?}: the flow keeps the generated prefix it was assigned, got \
                 {stored:?}"
            ),
        }
    }
}
