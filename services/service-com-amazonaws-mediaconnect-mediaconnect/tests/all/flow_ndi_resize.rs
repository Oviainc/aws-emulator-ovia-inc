// SPDX-License-Identifier: CC0-1.0

//! Resizing a flow that already INCLUDES NDI®, end to end over HTTP.
//!
//! The rule under test is the one `CreateFlowRequest$FlowSize` publishes
//! (`smithy_query(query="doc",shape="CreateFlowRequest",member="FlowSize")`: "set this
//! optional parameter to `LARGE` if you want to enable NDI sources or outputs on the flow")
//! and UpdateFlow's own documentation repeats ("if you have a MEDIUM flow and you want to
//! change the flow source to NDI®: first, use the UpdateFlow operation to upgrade the flow
//! size to LARGE… if you're switching from an NDI source to a transport stream (TS) source
//! and want to downgrade the flow size: first, use the UpdateFlowSource operation to change
//! the flow source type"): a flow that includes NDI exists only at a `FlowSize` whose
//! feature set carries NDI, so a request moving the size of a flow that includes NDI to a
//! size that does not carry it is refused with the modelled `BadRequestException` (HTTP
//! 400), writing nothing.
//!
//! "Includes NDI" is the same thing on every path: the flow's `NdiConfig` turns NDI on
//! ("this value must be set as ENABLED for your flow to support NDI sources or outputs",
//! `NdiConfig$NdiState`), or one of the SOURCES it holds is on the NDI transport, or one of
//! the OUTPUTS it holds is. The rows below therefore span all three markers — crossed with
//! the whole `FlowSize` catalog and with a request that does not touch the size at all —
//! rather than the one request an issue recorded, and each accepted update is read back to
//! assert the invariant the rule really states: a stored flow that includes NDI never
//! reports a size that cannot carry it.
//!
//! The add path's verdict on the very same stored flow is asserted beside it, so one state
//! has one answer: a flow UpdateFlow leaves is one AddFlowOutputs agrees with.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire. The `FlowSize` catalog and the request builders come from
//! `tests/all/flow_ndi_sizing.rs`, which judges the create and add halves of the same rule,
//! so the two suites read one catalog and one set of request forms.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;
use crate::flow_ndi_sizing::{flow_sizes, ndi_output, plain_output, plain_source, NDI_PROTOCOL};
use crate::flow_outputs::{add_flow_outputs, described_outputs};

/// The `FlowSize` values a resize can NAME, with whether each one's "processing capacity and
/// feature set" carries NDI®, read from the one catalog table this crate's NDI suites share
/// ([`crate::flow_ndi_sizing::flow_sizes`]): a resize always names a size, so that table's
/// "no flow size at all" row is not a resize target.
fn resize_targets() -> Vec<(&'static str, bool)> {
    flow_sizes()
        .into_iter()
        .filter_map(|(_, size, capable)| size.map(|size| (size, capable)))
        .collect()
}

/// One row: what the STORED flow holds before it is resized, and whether that flow includes
/// NDI®.
struct StoredForm {
    case: &'static str,
    /// Members merged into the base CreateFlow body (a row carrying `source` replaces the
    /// base one).
    patch: Value,
    /// Whether the flow this row stores includes NDI.
    includes_ndi: bool,
    /// The text the refusal names the NDI feature with, when this row includes NDI: the
    /// marker that makes the flow an NDI flow, as the refusal states it.
    feature: &'static str,
}

/// The forms NDI® can take on a STORED flow, each paired with whether that flow includes
/// NDI.
///
/// The rows differ along WHICH marker carries NDI (the flow's `NdiConfig`, a source's
/// protocol, a source's `NdiSourceSettings`, an output's protocol), how many outputs the
/// flow holds and WHERE the NDI one sits among them, how many markers at once, and the near
/// misses that are not NDI at all (an `NdiConfig` whose state is DISABLED, an output
/// carrying the NDI sender's name on a non-NDI transport).
fn stored_forms() -> Vec<StoredForm> {
    vec![
        StoredForm {
            case: "a flow with nothing NDI on it",
            patch: json!({}),
            includes_ndi: false,
            feature: "",
        },
        StoredForm {
            case: "a flow whose source is on the NDI protocol",
            patch: json!({
                "source": {
                    "name": "ndi-in",
                    "protocol": NDI_PROTOCOL,
                    // `NdiSourceSettings`: "the exact name of an existing NDI sender",
                    // whose audio reference level is bounded -40..=0 dBFS
                    // (`smithy_query(query="shape",shape="NdiSourceSettings")`).
                    "ndiSourceSettings": { "sourceName": "STUDIO (Cam1)", "audioReferenceLevel": -20 }
                }
            }),
            includes_ndi: true,
            feature: "The NDI source \"ndi-in\"",
        },
        StoredForm {
            // `NdiSourceSettings` is "the settings for the NDI® source", which only an NDI
            // source has, so it marks the source NDI on its own.
            case: "a flow whose source carries NdiSourceSettings on another protocol",
            patch: json!({
                "source": {
                    "name": "ndi-settings-only",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16",
                    "ndiSourceSettings": { "sourceName": "STUDIO (Cam2)" }
                }
            }),
            includes_ndi: true,
            feature: "The NDI source \"ndi-settings-only\"",
        },
        StoredForm {
            case: "a flow holding one output on the NDI protocol",
            patch: json!({ "outputs": [ndi_output("ndi-out")] }),
            includes_ndi: true,
            feature: "The NDI output \"ndi-out\"",
        },
        StoredForm {
            case: "a flow whose second of three outputs is on the NDI protocol",
            patch: json!({
                "outputs": [
                    plain_output("plain-out-1"),
                    ndi_output("ndi-out-middle"),
                    plain_output("plain-out-2")
                ]
            }),
            includes_ndi: true,
            feature: "The NDI output \"ndi-out-middle\"",
        },
        StoredForm {
            // `AddOutputRequest$NdiProgramName` is "a suffix for the name of the NDI®
            // sender that the flow creates", which an output only has when the NDI
            // transport is what carries it: the PROTOCOL is what makes an output NDI.
            case: "a flow holding an output naming an NDI sender on another protocol",
            patch: json!({
                "outputs": [{
                    "name": "sender-name-only",
                    "protocol": "rtp",
                    "destination": "198.51.100.8",
                    "port": 6102,
                    "ndiProgramName": "programme"
                }]
            }),
            includes_ndi: false,
            feature: "",
        },
        StoredForm {
            case: "a flow whose NdiConfig is ENABLED",
            patch: json!({ "ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-a" } }),
            includes_ndi: true,
            feature: "NdiConfig.NdiState ENABLED",
        },
        StoredForm {
            // The output marker stands on its own: the flow includes NDI because of what it
            // HOLDS, whatever the `NdiConfig` says.
            case: "a flow holding an NDI output while its NdiConfig is DISABLED",
            patch: json!({
                "ndiConfig": { "ndiState": "DISABLED", "machineName": "studio-a" },
                "outputs": [ndi_output("ndi-out-disabled-config")]
            }),
            includes_ndi: true,
            feature: "The NDI output \"ndi-out-disabled-config\"",
        },
        StoredForm {
            case: "a flow whose source and output are both on the NDI protocol",
            patch: json!({
                "source": {
                    "name": "ndi-in-both",
                    "protocol": NDI_PROTOCOL,
                    "ndiSourceSettings": { "sourceName": "STUDIO (Cam3)" }
                },
                "outputs": [ndi_output("ndi-out-both")]
            }),
            includes_ndi: true,
            // The source is the marker named first, exactly as the create path names it.
            feature: "The NDI source \"ndi-in-both\"",
        },
    ]
}

/// Create the flow one row describes, at the NDI-capable size every row is admissible at,
/// and return its ARN.
async fn create_row_flow(emulator: &Emulator, case: &str, name: &str, patch: &Value) -> String {
    let mut body = json!({ "name": name, "flowSize": "LARGE", "source": plain_source("src") });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in patch.as_object().expect("a patch is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The flow's stored size, as DescribeFlow reports it.
async fn described_flow_size(emulator: &Emulator, case: &str, flow_arn: &str) -> Option<String> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["flowSize"]
        .as_str()
        .map(ToString::to_string)
}

/// UpdateFlow moves a flow that includes NDI® only between sizes that carry NDI, whichever
/// marker — the flow's `NdiConfig`, a stored source or a stored output — is what makes it an
/// NDI flow, and refuses the move elsewhere with `BadRequestException` (400), writing
/// nothing.
#[tokio::test]
async fn update_flow_keeps_a_flow_that_includes_ndi_at_an_ndi_capable_size() {
    let emulator = Emulator::new();

    for (index, form) in stored_forms().into_iter().enumerate() {
        let StoredForm {
            case,
            patch,
            includes_ndi,
            feature,
        } = form;
        let flow_arn =
            create_row_flow(&emulator, case, &format!("ndi-resize-{index}"), &patch).await;

        // A request that touches no size is never refused for what the flow already holds,
        // whatever its markers: "the maintenance setting of the flow"
        // (`UpdateFlowRequest$Maintenance`) is one such setting.
        let case_untouched = format!("{case}, updated without touching the size");
        let (status, code, answered) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{flow_arn}"),
                Some(json!({ "maintenance": { "maintenanceDay": "Monday" } })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case_untouched:?}: a request that does not move the size is accepted, got \
             {status} {code:?}: {answered}"
        );

        for (size, capable) in resize_targets() {
            let case = format!("{case}, resized to {size}");
            let accepted = capable || !includes_ndi;
            let before = described_flow_size(&emulator, &case, &flow_arn).await;
            let (status, code, answered) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "flowSize": size })),
                )
                .await;
            let after = described_flow_size(&emulator, &case, &flow_arn).await;

            if accepted {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: this resize is one AWS accepts, got {status} {code:?}: \
                     {answered}"
                );
                // The answer is the stored record: read the size back rather than trusting
                // the 202 body.
                assert_eq!(
                    after.as_deref(),
                    Some(size),
                    "case {case:?}: the accepted resize stores its size, got {after:?}"
                );
            } else {
                assert_eq!(
                    (status, code.as_deref()),
                    (400, Some("BadRequestException")),
                    "case {case:?}: NDI needs a flow size that carries it, got {status} \
                     {code:?}: {answered}"
                );
                let message = answered["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(feature),
                    "case {case:?}: the refusal names what uses NDI ({feature:?}), got \
                     {message:?}"
                );
                assert!(
                    message.contains(size),
                    "case {case:?}: the refusal names the size it was asked for ({size:?}), \
                     got {message:?}"
                );
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused resize writes nothing, got {after:?}"
                );
            }

            // The invariant the rule states, read from the store either way: a flow that
            // includes NDI never reports a size that cannot carry it.
            let stored_is_capable = resize_targets()
                .into_iter()
                .any(|(value, capable)| Some(value) == after.as_deref() && capable);
            assert!(
                !includes_ndi || stored_is_capable,
                "case {case:?}: a stored flow that includes NDI stays on an NDI-capable \
                 size, got {after:?}"
            );
        }
    }
}

/// One stored flow has one answer: after UpdateFlow has settled a flow's size, AddFlowOutputs
/// accepts an output on the NDI® transport exactly where the stored size carries NDI — so a
/// state UpdateFlow leaves is never one the add path refuses to have made.
#[tokio::test]
async fn a_resized_flow_is_one_the_add_path_agrees_with() {
    let emulator = Emulator::new();

    for (index, form) in stored_forms().into_iter().enumerate() {
        let StoredForm {
            case, patch: form_patch, ..
        } = form;
        for (size, capable) in resize_targets() {
            let case = format!("{case}, resized to {size}, then given another NDI output");
            let flow_arn = create_row_flow(
                &emulator,
                &case,
                &format!("ndi-agree-{index}-{size}"),
                &form_patch,
            )
            .await;

            // Whatever the resize answers, the stored size below is the one the add path is
            // judged against.
            let (resize_status, _, resized) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "flowSize": size })),
                )
                .await;
            assert!(
                resize_status == 202 || resize_status == 400,
                "case {case:?}: a resize is accepted or refused, got {resize_status}: {resized}"
            );
            let stored = described_flow_size(&emulator, &case, &flow_arn).await;
            let stored_is_capable = resize_targets()
                .into_iter()
                .any(|(value, capable)| Some(value) == stored.as_deref() && capable);

            let before = described_outputs(&emulator, &case, &flow_arn).await;
            let (status, code, answered) = add_flow_outputs(
                &emulator,
                &flow_arn,
                json!([ndi_output(&format!("ndi-added-{index}-{size}"))]),
            )
            .await;
            let after = described_outputs(&emulator, &case, &flow_arn).await;

            if stored_is_capable {
                assert_eq!(
                    (status, code.as_deref()),
                    (201, None),
                    "case {case:?}: the stored size carries NDI (capable size requested: \
                     {capable}), so the add is accepted, got {status} {code:?}: {answered}"
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
                    "case {case:?}: the stored size does not carry NDI, so the add is \
                     refused, got {status} {code:?}: {answered}"
                );
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused add appends nothing, got {after:?}"
                );
            }
        }
    }
}
