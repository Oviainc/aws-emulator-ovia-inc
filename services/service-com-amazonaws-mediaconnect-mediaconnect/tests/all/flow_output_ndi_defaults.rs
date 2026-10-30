// SPDX-License-Identifier: CC0-1.0

//! The NDI® sender settings a flow output carries when the request that wrote it named
//! none, end to end over HTTP.
//!
//! The rule under test: a transport member whose documentation names the value MediaConnect
//! uses when the request omits it is MATERIALISED on the stored record under the condition
//! that documentation attaches it to, so every later read reports it. For the three NDI
//! sender members that condition is "for NDI outputs", i.e. an output on the NDI transport
//! (`smithy_query(query="shape",shape="Protocol")`: … | ndi-speed-hq):
//!
//! * `NdiOutputTimecodeSource` — "if you don't specify this field, MediaConnect uses
//!   `EMBEDDED_TIMECODE`" (`AddOutputRequest$NdiOutputTimecodeSource`), stated on the
//!   reported shape as "for NDI outputs, this field is always present and defaults to
//!   `EMBEDDED_TIMECODE`" (`Transport$NdiOutputTimecodeSource`);
//! * `NdiAudioReferenceLevel` — "if you don't specify this field, MediaConnect uses `-20`"
//!   (`AddOutputRequest$NdiAudioReferenceLevel`), and "for NDI outputs, this field is always
//!   present and defaults to `-20`" (`Transport$NdiAudioReferenceLevel`);
//! * `NdiProgramName` — "a suffix for the name of the NDI® sender that the flow creates. If
//!   a custom name isn't specified, MediaConnect uses the output name"
//!   (`AddOutputRequest$NdiProgramName`).
//!
//! A value the request DID name is kept as it came, and an output on any other protocol is
//! stored exactly as the request described it: the documentation attaches these values to
//! NDI outputs and to nothing else.
//!
//! The rule binds every path that writes an output's transport, so each has its own table
//! below: CreateFlow's `Outputs`, AddFlowOutputs, and UpdateFlowOutput — which can move a
//! stored output ONTO the NDI transport without naming any of the three.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;
use crate::flow_ndi_sizing::NDI_PROTOCOL;
use crate::flow_outputs::{add_flow_outputs, described_outputs};
use crate::update_flow_output::{at_path, update_flow_output};

/// The value `Transport$NdiOutputTimecodeSource` documents for an NDI output whose request
/// named no timecode source, written out here rather than read from the crate so these
/// cases check the documented rule and not the code's own idea of it.
const DEFAULT_TIMECODE_SOURCE: &str = "EMBEDDED_TIMECODE";

/// The other value of the `NdiOutputTimecodeSource` catalog
/// (`smithy_query(query="shape",shape="NdiOutputTimecodeSource")`: EMBEDDED_TIMECODE |
/// UTC_SYSTEM_TIME) — the one a request has to name to get it.
const OTHER_TIMECODE_SOURCE: &str = "UTC_SYSTEM_TIME";

/// The value `Transport$NdiAudioReferenceLevel` documents for an NDI output whose request
/// named no audio reference level, in dBFS.
const DEFAULT_AUDIO_REFERENCE_LEVEL: i64 = -20;

/// The published bounds of `NdiAudioReferenceLevel`
/// (`smithy_query(query="shape",shape="AddOutputRequest")`: range -40..0), as `(lowest,
/// highest)` acceptable level in dBFS.
const AUDIO_REFERENCE_LEVEL_RANGE: (i64, i64) = (-40, 0);

/// A flow the outputs under test can live on: NDI needs a flow size that carries it
/// (`CreateFlowRequest$FlowSize`, asserted over the whole size catalog in
/// `crate::flow_ndi_sizing`), so every flow here is created at one.
fn ndi_capable_flow(name: &str, outputs: Value) -> Value {
    json!({
        "name": name,
        "flowSize": "LARGE",
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        "outputs": outputs,
    })
}

/// Create `flow` and return its ARN.
async fn create_flow(emulator: &Emulator, case: &str, flow: Value) -> (String, Value) {
    let (status, code, created) = emulator.create_flow(flow).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (flow_arn, created)
}

/// The three NDI sender members an output request writes, as the stored record must report
/// them for an output named `name` — the documented value for a member an NDI output's
/// request left unset, the request's own value otherwise, and nothing at all on an output
/// that is not an NDI output.
fn expected_ndi_members(request: &Value, name: &str) -> Vec<(&'static str, Value)> {
    let requested = |member: &str| request[member].clone();
    if request["protocol"].as_str() != Some(NDI_PROTOCOL) {
        return vec![
            (
                "ndiOutputTimecodeSource",
                requested("ndiOutputTimecodeSource"),
            ),
            ("ndiAudioReferenceLevel", requested("ndiAudioReferenceLevel")),
            ("ndiProgramName", requested("ndiProgramName")),
        ];
    }
    let or_documented = |member: &str, documented: Value| match requested(member) {
        Value::Null => documented,
        named => named,
    };
    vec![
        (
            "ndiOutputTimecodeSource",
            or_documented("ndiOutputTimecodeSource", json!(DEFAULT_TIMECODE_SOURCE)),
        ),
        (
            "ndiAudioReferenceLevel",
            or_documented(
                "ndiAudioReferenceLevel",
                json!(DEFAULT_AUDIO_REFERENCE_LEVEL),
            ),
        ),
        // "MediaConnect uses the output name" — the name the output is STORED under, which
        // for a request that named none is the one the service generated.
        ("ndiProgramName", or_documented("ndiProgramName", json!(name))),
    ]
}

/// Assert `output` — a stored output answered by `path_label`'s operation — reports the
/// three NDI sender members the rule gives the request it was written from.
fn assert_ndi_members(case: &str, path_label: &str, request: &Value, output: &Value) {
    let name = output["name"].as_str().unwrap_or_default();
    for (member, expected) in expected_ndi_members(request, name) {
        let actual = &output["transport"][member];
        assert_eq!(
            actual, &expected,
            "case {case:?} ({path_label}): transport.{member}: expected {expected:?}, got \
             {actual:?} in {output:?}"
        );
    }
}

/// One row of the write table: an output request, judged by the rule above whichever
/// operation stores it.
struct OutputCase {
    case: &'static str,
    request: Value,
}

/// The rows differ along what the rule generalises over: whether the output is on the NDI
/// transport at all, which of the three members the request names (none, each one on its
/// own, all three), the value named for each — including the catalog's other timecode
/// source, each published bound of the audio reference level and the documented level named
/// explicitly — and whether the output carries a name of its own, since the program name
/// defaults to the name the output is STORED under.
fn output_cases() -> Vec<OutputCase> {
    vec![
        OutputCase {
            case: "an NDI output naming none of the three",
            request: json!({ "name": "ndi-bare", "protocol": NDI_PROTOCOL }),
        },
        OutputCase {
            case: "an NDI output with no name of its own",
            request: json!({ "protocol": NDI_PROTOCOL }),
        },
        OutputCase {
            case: "an NDI output naming only its program name",
            request: json!({
                "name": "ndi-program",
                "protocol": NDI_PROTOCOL,
                "ndiProgramName": "programme"
            }),
        },
        OutputCase {
            case: "an NDI output naming only the catalog's other timecode source",
            request: json!({
                "name": "ndi-timecode",
                "protocol": NDI_PROTOCOL,
                "ndiOutputTimecodeSource": OTHER_TIMECODE_SOURCE
            }),
        },
        OutputCase {
            case: "an NDI output naming only the timecode source the rule would default to",
            request: json!({
                "name": "ndi-timecode-same",
                "protocol": NDI_PROTOCOL,
                "ndiOutputTimecodeSource": DEFAULT_TIMECODE_SOURCE
            }),
        },
        OutputCase {
            case: "an NDI output at the lowest published audio reference level",
            request: json!({
                "name": "ndi-level-low",
                "protocol": NDI_PROTOCOL,
                "ndiAudioReferenceLevel": AUDIO_REFERENCE_LEVEL_RANGE.0
            }),
        },
        OutputCase {
            case: "an NDI output at the highest published audio reference level",
            request: json!({
                "name": "ndi-level-high",
                "protocol": NDI_PROTOCOL,
                "ndiAudioReferenceLevel": AUDIO_REFERENCE_LEVEL_RANGE.1
            }),
        },
        OutputCase {
            case: "an NDI output naming the audio reference level the rule would default to",
            request: json!({
                "name": "ndi-level-same",
                "protocol": NDI_PROTOCOL,
                "ndiAudioReferenceLevel": DEFAULT_AUDIO_REFERENCE_LEVEL
            }),
        },
        OutputCase {
            case: "an NDI output naming all three",
            request: json!({
                "name": "ndi-all",
                "protocol": NDI_PROTOCOL,
                "ndiProgramName": "programme",
                "ndiOutputTimecodeSource": OTHER_TIMECODE_SOURCE,
                "ndiAudioReferenceLevel": -18,
                "ndiSpeedHqQuality": 80
            }),
        },
        OutputCase {
            case: "an output on another protocol naming none of the three",
            request: json!({
                "name": "plain-bare",
                "protocol": "rtp",
                "destination": "198.51.100.7",
                "port": 6100
            }),
        },
        OutputCase {
            case: "an output on another protocol naming an NDI sender name",
            request: json!({
                "name": "plain-program",
                "protocol": "udp",
                "destination": "198.51.100.8",
                "port": 6101,
                "ndiProgramName": "programme"
            }),
        },
        OutputCase {
            case: "an output that names no protocol at all",
            request: json!({ "name": "no-protocol", "destination": "198.51.100.9" }),
        },
    ]
}

/// CreateFlow's `Outputs` store each output with the NDI sender members the rule gives it,
/// and DescribeFlow reports the same record.
#[tokio::test]
async fn create_flow_stores_the_documented_ndi_output_members() {
    let emulator = Emulator::new();

    let rows = output_cases();
    let requested: Vec<Value> = rows.iter().map(|row| row.request.clone()).collect();
    let (flow_arn, created) = create_flow(
        &emulator,
        "CreateFlow with every output form",
        ndi_capable_flow("ndi-defaults-create", json!(requested)),
    )
    .await;

    let answered = created["flow"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        answered.len(),
        rows.len(),
        "CreateFlow answers one output per requested output, got {answered:?}"
    );
    for (row, output) in rows.iter().zip(answered.iter()) {
        assert_ndi_members(row.case, "CreateFlow", &row.request, output);
        // An output that names no protocol has no transport at all
        // (`Transport$Protocol` is required), so nothing is materialised onto one.
        if row.request["protocol"].is_null() {
            assert!(
                output["transport"].is_null(),
                "case {:?}: an output with no protocol has no transport, got {output:?}",
                row.case
            );
        }
    }

    let stored = described_outputs(&emulator, "CreateFlow with every output form", &flow_arn).await;
    assert_eq!(
        stored, answered,
        "DescribeFlow reports the outputs the create answered, got {stored:?}"
    );
}

/// AddFlowOutputs stores each appended output with the NDI sender members the rule gives
/// it, and DescribeFlow reports the same record.
#[tokio::test]
async fn add_flow_outputs_stores_the_documented_ndi_output_members() {
    let emulator = Emulator::new();

    for (index, OutputCase { case, request }) in output_cases().into_iter().enumerate() {
        let (flow_arn, _) = create_flow(
            &emulator,
            case,
            ndi_capable_flow(&format!("ndi-defaults-add-{index}"), json!([])),
        )
        .await;

        let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, json!([request])).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
        );
        let answered = body["outputs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: one Output per requested output, got {answered:?}"
        );
        assert_ndi_members(case, "AddFlowOutputs", &request, &answered[0]);

        let stored = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            stored,
            answered,
            "case {case:?}: DescribeFlow reports the output the add answered, got {stored:?}"
        );
    }
}

/// One row of the UpdateFlowOutput sequence: the request body, and the paths of the stored
/// output it must leave at the given values.
struct MoveCase {
    case: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// The sequence applied to ONE output that starts on a protocol that is not the NDI
/// transport: the update that moves it onto NDI naming none of the three members, then the
/// updates that name one of them, then an update that touches another transport member, and
/// finally the update that moves the output back off NDI — so what is asserted is the rule
/// across the move, and not one request.
///
/// `OUTPUT_NAME` is the name the output is stored under, which is what an NDI output's
/// program name defaults to.
const OUTPUT_NAME: &str = "moving";

fn move_cases() -> Vec<MoveCase> {
    vec![
        MoveCase {
            case: "moved onto the NDI transport naming none of the three",
            body: json!({ "protocol": NDI_PROTOCOL }),
            expect: vec![
                ("transport.protocol", json!(NDI_PROTOCOL)),
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(DEFAULT_TIMECODE_SOURCE),
                ),
                (
                    "transport.ndiAudioReferenceLevel",
                    json!(DEFAULT_AUDIO_REFERENCE_LEVEL),
                ),
                ("transport.ndiProgramName", json!(OUTPUT_NAME)),
            ],
        },
        MoveCase {
            case: "the timecode source named on an output already on NDI",
            body: json!({ "ndiOutputTimecodeSource": OTHER_TIMECODE_SOURCE }),
            expect: vec![
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(OTHER_TIMECODE_SOURCE),
                ),
                (
                    "transport.ndiAudioReferenceLevel",
                    json!(DEFAULT_AUDIO_REFERENCE_LEVEL),
                ),
                ("transport.ndiProgramName", json!(OUTPUT_NAME)),
            ],
        },
        MoveCase {
            case: "a custom program name named on an output already on NDI",
            body: json!({ "ndiProgramName": "programme" }),
            expect: vec![
                ("transport.ndiProgramName", json!("programme")),
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(OTHER_TIMECODE_SOURCE),
                ),
                (
                    "transport.ndiAudioReferenceLevel",
                    json!(DEFAULT_AUDIO_REFERENCE_LEVEL),
                ),
            ],
        },
        MoveCase {
            case: "the audio reference level named on an output already on NDI",
            body: json!({ "ndiAudioReferenceLevel": -18 }),
            expect: vec![
                ("transport.ndiAudioReferenceLevel", json!(-18)),
                ("transport.ndiProgramName", json!("programme")),
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(OTHER_TIMECODE_SOURCE),
                ),
            ],
        },
        MoveCase {
            // A transport member that is not an NDI member: the partial update writes it
            // and the NDI members keep what the rows above left on the record.
            case: "another transport member on an output already on NDI",
            body: json!({ "minLatency": 120 }),
            expect: vec![
                ("transport.minLatency", json!(120)),
                ("transport.ndiAudioReferenceLevel", json!(-18)),
                ("transport.ndiProgramName", json!("programme")),
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(OTHER_TIMECODE_SOURCE),
                ),
            ],
        },
        MoveCase {
            // "If you don't specify this field, MediaConnect leaves the value unchanged"
            // (`UpdateFlowOutputRequest`): moving the output onto another protocol is a
            // partial update like any other, so the settings it does not name survive.
            case: "moved back off the NDI transport",
            body: json!({ "protocol": "udp" }),
            expect: vec![
                ("transport.protocol", json!("udp")),
                ("transport.ndiAudioReferenceLevel", json!(-18)),
                ("transport.ndiProgramName", json!("programme")),
                (
                    "transport.ndiOutputTimecodeSource",
                    json!(OTHER_TIMECODE_SOURCE),
                ),
            ],
        },
    ]
}

/// UpdateFlowOutput materialises the documented NDI sender members on an output it moves
/// onto the NDI transport, keeps a value the request names, and answers with the stored
/// record DescribeFlow then reports.
#[tokio::test]
async fn update_flow_output_materialises_the_documented_ndi_output_members() {
    let emulator = Emulator::new();

    let (flow_arn, created) = create_flow(
        &emulator,
        "the output moved onto NDI",
        ndi_capable_flow(
            "ndi-defaults-update",
            json!([{
                "name": OUTPUT_NAME,
                "protocol": "udp",
                "destination": "198.51.100.11",
                "port": 6200
            }]),
        ),
    )
    .await;
    let output = &created["flow"]["outputs"][0];
    assert!(
        output["transport"]["ndiProgramName"].is_null()
            && output["transport"]["ndiOutputTimecodeSource"].is_null()
            && output["transport"]["ndiAudioReferenceLevel"].is_null(),
        "the output starts on a protocol that is not NDI, so it carries no NDI sender \
         settings, got {output:?}"
    );
    let output_arn = output["outputArn"].as_str().unwrap_or_default().to_string();

    for MoveCase { case, body, expect } in move_cases() {
        let (status, code, answered) =
            update_flow_output(&emulator, &flow_arn, &output_arn, body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {answered}"
        );
        for (path, expected) in &expect {
            let actual = at_path(&answered["output"], path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path}: expected {expected:?}, got {actual:?} in {:?}",
                answered["output"]
            );
        }
        let stored = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            stored.first(),
            Some(&answered["output"]),
            "case {case:?}: DescribeFlow reports the record the update answered, got {stored:?}"
        );
    }
}

/// An audio reference level outside the published range is refused with the operation's own
/// modelled `BadRequestException` (400) and appends nothing — the documented default fills
/// only an omitted member, it never rescues a value the model will not take.
#[tokio::test]
async fn an_audio_reference_level_past_the_published_range_is_refused() {
    let emulator = Emulator::new();
    let (flow_arn, _) = create_flow(
        &emulator,
        "a level past the range",
        ndi_capable_flow("ndi-defaults-range", json!([])),
    )
    .await;

    for level in [
        AUDIO_REFERENCE_LEVEL_RANGE.0 - 1,
        AUDIO_REFERENCE_LEVEL_RANGE.1 + 1,
    ] {
        let case = format!("an NDI output at an audio reference level of {level} dBFS");
        let (status, code, body) = add_flow_outputs(
            &emulator,
            &flow_arn,
            json!([{
                "name": format!("ndi-out-{level}"),
                "protocol": NDI_PROTOCOL,
                "ndiAudioReferenceLevel": level
            }]),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a level outside the published range is refused, got {status} \
             {code:?}: {body}"
        );
        let stored = described_outputs(&emulator, &case, &flow_arn).await;
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused add appends nothing, got {stored:?}"
        );
    }
}
