// SPDX-License-Identifier: CC0-1.0

//! Whether a flow may include NDI®, end to end over HTTP.
//!
//! The rule under test is published by `CreateFlowRequest$FlowSize`
//! (`smithy_query(query="doc",shape="CreateFlowRequest",member="FlowSize")`): "Determines
//! the processing capacity and feature set of the flow. Set this optional parameter to
//! `LARGE` if you want to enable NDI sources or outputs on the flow", which UpdateFlow's
//! own documentation repeats ("if you have a MEDIUM flow and you want to change the flow
//! source to NDI: first, use the UpdateFlow operation to upgrade the flow size to LARGE").
//! So a flow that INCLUDES NDI — `NdiConfig$NdiState` ENABLED ("this value must be set as
//! ENABLED for your flow to support NDI sources or outputs"), or a source or an output on
//! the NDI transport — exists only at a `FlowSize` whose feature set carries NDI, and every
//! request that would leave a flow outside that rule is refused with the modelled
//! `BadRequestException` (HTTP 400), storing nothing.
//!
//! The rule binds every operation that can put NDI on a flow, so each has its own table
//! below: CreateFlow (the flow, its `Source`/`Sources` and its `Outputs`), AddFlowSources
//! and AddFlowOutputs (which append one), and UpdateFlowSource / UpdateFlowOutput (which
//! move an existing one onto the NDI transport). UpdateFlow's own half of the rule — the
//! `NdiConfig` / `FlowSize` pair on an existing flow — is asserted in
//! `tests/all/update_flow.rs`.
//!
//! The rows span the WHOLE `FlowSize` catalog (`smithy_query(query="shape",
//! shape="FlowSize")`: MEDIUM | LARGE | LARGE_4X) plus the flow whose size was never set,
//! crossed with the forms NDI can take in a request, so what is asserted is the rule rather
//! than the one request the issue recorded.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_outputs::{add_flow_outputs, described_outputs};
use crate::flow_sources::{add_flow_sources, described_sources, failover_active};

/// The `Protocol` value that names the NDI® transport
/// (`smithy_query(query="shape",shape="Protocol")`: … | ndi-speed-hq), written out here
/// rather than read from the crate so these cases check the rule and not the code's own
/// idea of it.
const NDI_PROTOCOL: &str = "ndi-speed-hq";

/// The whole `FlowSize` catalog (`smithy_query(query="shape",shape="FlowSize")`: MEDIUM |
/// LARGE | LARGE_4X), plus the flow whose size the request never set, with whether each
/// one's "feature set" carries NDI®: `CreateFlowRequest$FlowSize` names `LARGE` as the size
/// that enables NDI, so `MEDIUM` — and a size never set at all — does not, and `LARGE_4X`,
/// which is larger than `LARGE`, does.
fn flow_sizes() -> Vec<(&'static str, Option<&'static str>, bool)> {
    vec![
        ("no flow size at all", None, false),
        ("MEDIUM", Some("MEDIUM"), false),
        ("LARGE", Some("LARGE"), true),
        ("LARGE_4X", Some("LARGE_4X"), true),
    ]
}

/// A source request on a protocol that is not the NDI transport.
fn plain_source(name: &str) -> Value {
    json!({ "name": name, "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" })
}

/// A source request on the NDI® transport.
fn ndi_source(name: &str) -> Value {
    json!({
        "name": name,
        "protocol": NDI_PROTOCOL,
        // `NdiSourceSettings`: "the exact name of an existing NDI sender", whose audio
        // reference level is bounded -40..=0 dBFS
        // (`smithy_query(query="shape",shape="NdiSourceSettings")`).
        "ndiSourceSettings": { "sourceName": "STUDIO (Cam1)", "audioReferenceLevel": -20 }
    })
}

/// An output request on a protocol that is not the NDI transport.
fn plain_output(name: &str) -> Value {
    json!({ "name": name, "protocol": "rtp", "destination": "198.51.100.7", "port": 6100 })
}

/// An output request on the NDI® transport.
fn ndi_output(name: &str) -> Value {
    json!({ "name": name, "protocol": NDI_PROTOCOL, "ndiProgramName": "programme" })
}

/// One row of the CreateFlow table: the members a request adds to the base body to describe
/// its NDI (or non-NDI) flow, and whether the flow it describes INCLUDES NDI.
struct NdiForm {
    case: &'static str,
    /// Members merged into the base CreateFlow body. A row that carries `sources` drops the
    /// base `source`, since the two name the same thing.
    patch: Value,
    includes_ndi: bool,
}

/// The forms NDI can take in a CreateFlow request, each paired with whether the flow it
/// describes includes NDI.
///
/// The rows differ along WHICH marker carries NDI (the flow's `NdiConfig`, a source's
/// protocol, a source's `NdiSourceSettings`, an output's protocol), how many markers at
/// once, WHERE the NDI source sits (the single `Source` member, the first or the second
/// entry of `Sources`), and the near misses that are not NDI at all (`NdiState` DISABLED,
/// an `NdiConfig` that names no state, an output carrying the NDI sender's name on a
/// non-NDI transport).
fn ndi_forms() -> Vec<NdiForm> {
    vec![
        NdiForm {
            case: "nothing NDI at all",
            patch: json!({}),
            includes_ndi: false,
        },
        NdiForm {
            // "This value must be set as ENABLED for your flow to support NDI sources or
            // outputs" (`NdiConfig$NdiState`).
            case: "ndiConfig with ndiState ENABLED",
            patch: json!({ "ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-a" } }),
            includes_ndi: true,
        },
        NdiForm {
            // "The default value is DISABLED" — the other value of the `NdiState` enum
            // (`smithy_query(query="shape",shape="NdiState")`: ENABLED | DISABLED) turns
            // nothing on.
            case: "ndiConfig with ndiState DISABLED",
            patch: json!({ "ndiConfig": { "ndiState": "DISABLED", "machineName": "studio-a" } }),
            includes_ndi: false,
        },
        NdiForm {
            // `NdiState` is optional on `NdiConfig`, and a setting nothing turned on is off.
            case: "an ndiConfig that names no state",
            patch: json!({ "ndiConfig": { "machineName": "studio-a" } }),
            includes_ndi: false,
        },
        NdiForm {
            case: "the single Source on the NDI transport",
            patch: json!({ "source": ndi_source("ndi-in") }),
            includes_ndi: true,
        },
        NdiForm {
            // `NdiSourceSettings` is "the settings for the NDI® source", which only an NDI
            // source has, so it marks the source NDI on its own.
            case: "a source carrying NdiSourceSettings on another protocol",
            patch: json!({
                "source": {
                    "name": "ndi-settings-only",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16",
                    "ndiSourceSettings": { "sourceName": "STUDIO (Cam2)" }
                }
            }),
            includes_ndi: true,
        },
        NdiForm {
            case: "the first entry of a Sources pair on the NDI transport",
            patch: json!({
                "sources": [ndi_source("ndi-first"), plain_source("plain-second")],
                "sourceFailoverConfig": failover_active()
            }),
            includes_ndi: true,
        },
        NdiForm {
            case: "the second entry of a Sources pair on the NDI transport",
            patch: json!({
                "sources": [plain_source("plain-first"), ndi_source("ndi-second")],
                "sourceFailoverConfig": failover_active()
            }),
            includes_ndi: true,
        },
        NdiForm {
            case: "a Sources pair with no NDI entry",
            patch: json!({
                "sources": [plain_source("plain-first"), plain_source("plain-second")],
                "sourceFailoverConfig": failover_active()
            }),
            includes_ndi: false,
        },
        NdiForm {
            case: "an Output on the NDI transport",
            patch: json!({ "outputs": [ndi_output("ndi-out")] }),
            includes_ndi: true,
        },
        NdiForm {
            case: "the second of two Outputs on the NDI transport",
            patch: json!({ "outputs": [plain_output("plain-out"), ndi_output("ndi-out")] }),
            includes_ndi: true,
        },
        NdiForm {
            // `AddOutputRequest$NdiProgramName` is "a suffix for the name of the NDI®
            // sender that the flow creates", which an output only has when the NDI
            // transport is what carries it: the PROTOCOL is what makes an output NDI.
            case: "an output naming an NDI sender on another protocol",
            patch: json!({
                "outputs": [{
                    "name": "sender-name-only",
                    "protocol": "rtp",
                    "destination": "198.51.100.8",
                    "port": 6101,
                    "ndiProgramName": "programme"
                }]
            }),
            includes_ndi: false,
        },
        NdiForm {
            case: "an enabled ndiConfig, an NDI source and an NDI output at once",
            patch: json!({
                "ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-a" },
                "source": ndi_source("ndi-in"),
                "outputs": [ndi_output("ndi-out")]
            }),
            includes_ndi: true,
        },
    ]
}

/// The CreateFlow body for one row at one flow size.
fn create_body(name: &str, flow_size: Option<&str>, patch: &Value) -> Value {
    let mut body = json!({ "name": name, "source": plain_source("src") });
    if let Some(flow_size) = flow_size {
        body["flowSize"] = json!(flow_size);
    }
    let object = body.as_object_mut().expect("the body is a JSON object");
    if patch.get("sources").is_some() {
        object.remove("source");
    }
    for (member, value) in patch.as_object().expect("a patch is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
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

/// The capable sizes the crate publishes are the ones the model's documentation names, and
/// the predicate the handlers read resolves every value of the catalog the same way.
#[tokio::test]
async fn the_published_ndi_capable_sizes_are_the_documented_ones() {
    assert_eq!(
        mc::NDI_CAPABLE_FLOW_SIZES.to_vec(),
        vec!["LARGE", "LARGE_4X"],
        "case \"published sizes\": NDI runs on the LARGE sizes, got {:?}",
        mc::NDI_CAPABLE_FLOW_SIZES
    );
    for (case, flow_size, capable) in flow_sizes() {
        assert_eq!(
            mc::flow_size_supports_ndi(flow_size),
            capable,
            "case {case:?}: flow_size_supports_ndi({flow_size:?}) must be {capable}, got {}",
            mc::flow_size_supports_ndi(flow_size)
        );
    }
}

/// CreateFlow stores a flow that includes NDI® only at a flow size that carries NDI, and
/// refuses one at any other size with `BadRequestException` (400), storing no flow.
#[tokio::test]
async fn create_flow_stores_an_ndi_flow_only_at_an_ndi_capable_size() {
    let emulator = Emulator::new();

    for (index, form) in ndi_forms().into_iter().enumerate() {
        for (size_case, flow_size, capable) in flow_sizes() {
            let NdiForm {
                case,
                patch,
                includes_ndi,
            } = &form;
            let case = format!("{case} at {size_case}");
            let name = format!("ndi-create-{index}-{}", flow_size.unwrap_or("unset"));
            let accepted = !includes_ndi || capable;

            let body = create_body(&name, flow_size, patch);
            let (status, code, created) = emulator.create_flow(body.clone()).await;
            if accepted {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: this flow is one AWS creates, got {status} {code:?}: \
                     {created}"
                );
                // The stored record is what the create described: read back through
                // DescribeFlow, not from the 201 body.
                let flow_arn = created["flow"]["flowArn"].as_str().unwrap_or_default();
                let (status, code, described) = emulator.describe_flow(flow_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: \
                     {described}"
                );
                assert_eq!(
                    described["flow"]["flowSize"].as_str(),
                    flow_size,
                    "case {case:?}: the stored flow size, got {:?}",
                    described["flow"]["flowSize"]
                );
                if let Some(ndi_config) = body.get("ndiConfig") {
                    assert_eq!(
                        described["flow"]["ndiConfig"]["ndiState"],
                        ndi_config["ndiState"],
                        "case {case:?}: the stored NdiConfig state, got {:?}",
                        described["flow"]["ndiConfig"]
                    );
                }
                let stored_sources = described["flow"]["sources"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let requested_sources = body
                    .get("sources")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| vec![body["source"].clone()]);
                assert_eq!(
                    stored_sources.len(),
                    requested_sources.len(),
                    "case {case:?}: the stored flow holds the sources the create named, got \
                     {stored_sources:?}"
                );
                for (offset, requested) in requested_sources.iter().enumerate() {
                    assert_eq!(
                        stored_sources[offset]["transport"]["protocol"],
                        requested["protocol"],
                        "case {case:?}: sources[{offset}].transport.protocol, got {:?}",
                        stored_sources[offset]["transport"]
                    );
                }
                let stored_outputs = described["flow"]["outputs"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let requested_outputs = body
                    .get("outputs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    stored_outputs.len(),
                    requested_outputs.len(),
                    "case {case:?}: the stored flow holds the outputs the create named, got \
                     {stored_outputs:?}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: NDI needs a flow size that carries it, got {status} \
                     {code:?}: {created}"
                );
                assert!(
                    !is_listed(&emulator, &case, &name).await,
                    "case {case:?}: a refused create stores no flow, yet {name:?} is listed"
                );
            }
        }
    }
}

/// AddFlowSources appends a source on the NDI® transport only to a flow whose size carries
/// NDI, and refuses it elsewhere with `BadRequestException` (400), appending nothing.
#[tokio::test]
async fn add_flow_sources_appends_an_ndi_source_only_to_an_ndi_capable_flow() {
    let emulator = Emulator::new();

    for (size_case, flow_size, capable) in flow_sizes() {
        // The flow starts on one non-NDI source with failover ACTIVE, so the pair the add
        // would leave is inside the bound `FailoverConfig$State` sets and the only rule
        // under test is the NDI one.
        let mut body = json!({
            "name": format!("ndi-add-source-{}", flow_size.unwrap_or("unset")),
            "source": plain_source("src"),
            "sourceFailoverConfig": failover_active()
        });
        if let Some(flow_size) = flow_size {
            body["flowSize"] = json!(flow_size);
        }
        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {size_case:?}: the flow under test must be created, got {status} {code:?}: \
             {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let before = described_sources(&emulator, size_case, &flow_arn).await;

        for (form, source) in [
            ("an NDI source", ndi_source("ndi-added")),
            (
                "a source carrying NdiSourceSettings only",
                json!({
                    "name": "ndi-settings-added",
                    "protocol": "rtp",
                    "whitelistCidr": "10.3.0.0/16",
                    "ndiSourceSettings": { "sourceName": "STUDIO (Cam3)" }
                }),
            ),
            ("a source that is not NDI", plain_source("plain-added")),
        ] {
            let case = format!("{form} added to a flow at {size_case}");
            let is_ndi = form != "a source that is not NDI";
            let (status, code, answered) =
                add_flow_sources(&emulator, &flow_arn, json!([source])).await;
            let after = described_sources(&emulator, &case, &flow_arn).await;
            if capable || !is_ndi {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: this add is one AWS accepts, got {status} {code:?}: \
                     {answered}"
                );
                assert_eq!(
                    after.len(),
                    before.len() + 1,
                    "case {case:?}: the accepted add appends its source, got {after:?}"
                );
                // Leave the flow as the next row found it: the failover bound allows one
                // pair, not one pair per row.
                let added = after
                    .last()
                    .and_then(|source| source["sourceArn"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let (status, _, removed) =
                    crate::flow_sources::remove_flow_source(&emulator, &flow_arn, &added).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: the fixture removal must succeed, got {status}: {removed}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: NDI needs a flow size that carries it, got {status} \
                     {code:?}: {answered}"
                );
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused add appends nothing, got {after:?}"
                );
            }
        }
    }
}

/// AddFlowOutputs appends an output on the NDI® transport only to a flow whose size carries
/// NDI, and refuses it elsewhere with `BadRequestException` (400), appending nothing.
#[tokio::test]
async fn add_flow_outputs_appends_an_ndi_output_only_to_an_ndi_capable_flow() {
    let emulator = Emulator::new();

    for (size_case, flow_size, capable) in flow_sizes() {
        let mut body = json!({
            "name": format!("ndi-add-output-{}", flow_size.unwrap_or("unset")),
            "source": plain_source("src")
        });
        if let Some(flow_size) = flow_size {
            body["flowSize"] = json!(flow_size);
        }
        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {size_case:?}: the flow under test must be created, got {status} {code:?}: \
             {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        for (form, output, is_ndi) in [
            ("an NDI output", ndi_output("ndi-added"), true),
            (
                "an output naming an NDI sender on another protocol",
                json!({
                    "name": "sender-name-added",
                    "protocol": "rtp",
                    "destination": "198.51.100.9",
                    "port": 6102,
                    "ndiProgramName": "programme"
                }),
                false,
            ),
            ("an output that is not NDI", plain_output("plain-added"), false),
        ] {
            let case = format!("{form} added to a flow at {size_case}");
            let before = described_outputs(&emulator, &case, &flow_arn).await;
            let (status, code, answered) =
                add_flow_outputs(&emulator, &flow_arn, json!([output])).await;
            let after = described_outputs(&emulator, &case, &flow_arn).await;
            if capable || !is_ndi {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: this add is one AWS accepts, got {status} {code:?}: \
                     {answered}"
                );
                assert_eq!(
                    after.len(),
                    before.len() + 1,
                    "case {case:?}: the accepted add appends its output, got {after:?}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: NDI needs a flow size that carries it, got {status} \
                     {code:?}: {answered}"
                );
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused add appends nothing, got {after:?}"
                );
            }
        }
    }
}

/// UpdateFlowSource and UpdateFlowOutput move a stored record onto the NDI® transport only
/// on a flow whose size carries NDI, and refuse it elsewhere with `BadRequestException`
/// (400), writing nothing.
#[tokio::test]
async fn an_update_moves_a_record_onto_the_ndi_transport_only_on_an_ndi_capable_flow() {
    let emulator = Emulator::new();

    for (size_case, flow_size, capable) in flow_sizes() {
        let mut body = json!({
            "name": format!("ndi-update-{}", flow_size.unwrap_or("unset")),
            "source": plain_source("src"),
            "outputs": [plain_output("out")]
        });
        if let Some(flow_size) = flow_size {
            body["flowSize"] = json!(flow_size);
        }
        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {size_case:?}: the flow under test must be created, got {status} {code:?}: \
             {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let source_arn = described_sources(&emulator, size_case, &flow_arn).await[0]["sourceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let output_arn = described_outputs(&emulator, size_case, &flow_arn).await[0]["outputArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The two markers that make a SOURCE NDI, and the one that makes an OUTPUT NDI,
        // each through the update that introduces it.
        let rows = vec![
            (
                "the source moved onto the NDI protocol",
                format!("/v1/flows/{flow_arn}/source/{source_arn}"),
                json!({ "protocol": NDI_PROTOCOL }),
                true,
            ),
            (
                "the source given NdiSourceSettings",
                format!("/v1/flows/{flow_arn}/source/{source_arn}"),
                json!({ "ndiSourceSettings": { "sourceName": "STUDIO (Cam4)" } }),
                true,
            ),
            (
                "the source moved onto another protocol",
                format!("/v1/flows/{flow_arn}/source/{source_arn}"),
                json!({ "protocol": "udp" }),
                false,
            ),
            (
                "the output moved onto the NDI protocol",
                format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
                json!({ "protocol": NDI_PROTOCOL }),
                true,
            ),
            (
                "the output given the NDI sender's name on another protocol",
                format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
                json!({ "ndiProgramName": "programme" }),
                false,
            ),
        ];

        for (form, uri, update, is_ndi) in rows {
            let case = format!("{form} on a flow at {size_case}");
            let (_, _, before) = emulator.describe_flow(&flow_arn).await;
            let (status, code, answered) = emulator.send("PUT", &uri, Some(update)).await;
            let (_, _, after) = emulator.describe_flow(&flow_arn).await;
            if capable || !is_ndi {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: this update is one AWS accepts, got {status} {code:?}: \
                     {answered}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: NDI needs a flow size that carries it, got {status} \
                     {code:?}: {answered}"
                );
                assert_eq!(
                    after["flow"], before["flow"],
                    "case {case:?}: a refused update writes nothing, got {:?}",
                    after["flow"]
                );
            }
        }
    }
}
