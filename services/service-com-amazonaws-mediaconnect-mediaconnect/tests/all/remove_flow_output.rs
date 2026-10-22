// SPDX-License-Identifier: CC0-1.0

//! RemoveFlowOutput, end to end over HTTP: one call drops exactly the output whose minted
//! ARN it names from the flow it names, and nothing else about that flow changes.
//!
//! The rule these cases assert, over a table rather than over one request: a remove
//! answers 202 with the stored flow's ARN and the removed output's ARN, the flow then
//! holds the outputs it held minus that one IN THE SAME ORDER, the output's name is free
//! again, and a request naming a flow the store does not hold or an output that flow does
//! not hold is refused `NotFoundException` (404) and removes nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_outputs::{
    add_flow_outputs, assert_minted_output_arn, create_flow_with_outputs, described_outputs,
    unknown_flow_arn,
};

/// `DELETE /v1/flows/{FlowArn}/outputs/{OutputArn}` — the RemoveFlowOutput binding
/// (`smithy_query(query="operation",operation="RemoveFlowOutput")`).
async fn remove_flow_output(
    emulator: &Emulator,
    flow_arn: &str,
    output_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
            None,
        )
        .await
}

/// `count` output requests named `out-<n>` from `first`, so a row can build a flow with a
/// known number of distinguishable outputs.
fn numbered_outputs(first: usize, count: usize) -> Value {
    (first..first + count)
        .map(|index| {
            json!({
                "name": format!("out-{index}"),
                "protocol": "udp",
                "destination": "203.0.113.60",
                "port": 8000 + i32::try_from(index).unwrap_or(0) * 2
            })
        })
        .collect()
}

/// The flow's status, as DescribeFlow reports it.
async fn described_status(emulator: &Emulator, flow_arn: &str) -> String {
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    described["flow"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Remove the output at `position` of the flow's CURRENT output list and assert the whole
/// rule for that one call: the 202 answer names the flow and that output, the flow then
/// holds the other outputs unchanged and in order, and its status is left alone.
async fn remove_at(emulator: &Emulator, case: &str, flow_arn: &str, position: usize) {
    let before = described_outputs(emulator, case, flow_arn).await;
    let status_before = described_status(emulator, flow_arn).await;
    let target = before
        .get(position)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the flow must hold an output at {position}"));
    let output_arn = target["outputArn"].as_str().unwrap_or_default().to_string();

    let (status, code, body) = remove_flow_output(emulator, flow_arn, &output_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: removing output {position} expected 202 with no error, got {status} \
         {code:?}: {body}"
    );
    assert_eq!(
        (body["flowArn"].as_str(), body["outputArn"].as_str()),
        (Some(flow_arn), Some(output_arn.as_str())),
        "case {case:?}: the response names the flow and the removed output, got {body}"
    );

    let after = described_outputs(emulator, case, flow_arn).await;
    let expected: Vec<Value> = before
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, output)| output.clone())
        .collect();
    assert_eq!(
        after, expected,
        "case {case:?}: the flow holds the other outputs, in order, got {after:?}"
    );
    assert_eq!(
        described_status(emulator, flow_arn).await,
        status_before,
        "case {case:?}: removing an output does not move the flow's status"
    );
}

/// One row of the RemoveFlowOutput table: the outputs the flow is created with, the
/// outputs AddFlowOutputs then appends, and the positions removed by successive calls
/// (each one into the list as it stands when that call is made).
struct RemoveCase {
    case: &'static str,
    created: Value,
    added: Value,
    remove: &'static [usize],
}

/// The rows differ along what the rule generalises over: how many outputs the flow holds,
/// where in its list the removed one sits (only, first, middle, last), how many calls
/// remove outputs, and whether the removed output was stored by CreateFlow or by
/// AddFlowOutputs.
fn remove_cases() -> Vec<RemoveCase> {
    vec![
        RemoveCase {
            case: "the only output of a flow",
            created: numbered_outputs(0, 1),
            added: json!([]),
            remove: &[0],
        },
        RemoveCase {
            case: "the first of three",
            created: numbered_outputs(0, 3),
            added: json!([]),
            remove: &[0],
        },
        RemoveCase {
            case: "the middle of three",
            created: numbered_outputs(0, 3),
            added: json!([]),
            remove: &[1],
        },
        RemoveCase {
            case: "the last of three",
            created: numbered_outputs(0, 3),
            added: json!([]),
            remove: &[2],
        },
        RemoveCase {
            case: "an output AddFlowOutputs appended, not CreateFlow",
            created: numbered_outputs(0, 1),
            added: numbered_outputs(1, 2),
            remove: &[2],
        },
        RemoveCase {
            case: "two outputs of five, over two calls",
            created: numbered_outputs(0, 2),
            added: numbered_outputs(2, 3),
            remove: &[4, 0],
        },
        RemoveCase {
            case: "every output of a flow, one call at a time",
            created: numbered_outputs(0, 2),
            added: numbered_outputs(2, 1),
            remove: &[1, 1, 0],
        },
        RemoveCase {
            case: "an output carrying encryption and a VPC interface attachment",
            created: json!([
                {
                    "name": "enc",
                    "protocol": "srt-listener",
                    "port": 8100,
                    "minLatency": 120,
                    "encryption": {
                        "algorithm": "aes256",
                        "keyType": "static-key",
                        "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                        "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-abc"
                    },
                    "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" }
                },
                { "name": "plain", "protocol": "udp", "destination": "203.0.113.61", "port": 8102 }
            ]),
            added: json!([]),
            remove: &[0],
        },
    ]
}

/// RemoveFlowOutput drops exactly the output its `OutputArn` names from the flow its
/// `FlowArn` names, and leaves every other output of that flow exactly as it was.
#[tokio::test]
async fn remove_flow_output_drops_only_the_output_it_names() {
    let emulator = Emulator::new();

    for (index, row) in remove_cases().into_iter().enumerate() {
        let RemoveCase {
            case,
            created,
            added,
            remove,
        } = row;
        let flow_arn =
            create_flow_with_outputs(&emulator, case, &format!("flow-remove-{index}"), created)
                .await;
        if added.as_array().is_some_and(|outputs| !outputs.is_empty()) {
            let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, added).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the fixture outputs must be added, got {status} {code:?}: {body}"
            );
        }

        for position in remove {
            remove_at(&emulator, case, &flow_arn, *position).await;
        }
    }
}

/// One row of the refusal table: a RemoveFlowOutput request the service must refuse, the
/// two ARNs it carries, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    output_arn: String,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A RemoveFlowOutput request that resolves no output removes nothing: it is refused with
/// the operation's own modelled error, naming the ARN that resolved nothing, and the
/// flow still holds exactly the outputs it held.
#[tokio::test]
async fn remove_flow_output_refuses_what_it_cannot_remove() {
    let emulator = Emulator::new();

    let flow_arn = create_flow_with_outputs(
        &emulator,
        "refusals",
        "flow-remove-refusals",
        numbered_outputs(0, 2),
    )
    .await;
    let other_flow = create_flow_with_outputs(
        &emulator,
        "refusals",
        "flow-remove-other",
        numbered_outputs(10, 1),
    )
    .await;
    let before = described_outputs(&emulator, "refusals", &flow_arn).await;
    let other_before = described_outputs(&emulator, "refusals", &other_flow).await;
    let held_output = before[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other_output = other_before[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // One output removed for real, so a second call on its ARN addresses an output that
    // no longer exists.
    let gone = before[1]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _, body) = remove_flow_output(&emulator, &flow_arn, &gone).await;
    assert_eq!(
        status, 202,
        "the fixture removal must succeed, got {status}: {body}"
    );
    let before = described_outputs(&emulator, "refusals", &flow_arn).await;

    // An output ARN of the form this service mints, for an output it never minted.
    let never_minted =
        format!("arn:aws:mediaconnect:{REGION}:111122223333:output:1-0000dead:never-minted");

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted, with an output ARN it did mint",
            flow_arn: unknown_flow_arn(),
            output_arn: held_output.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow_arn()),
        },
        RefusalCase {
            case: "an output ARN this service never minted",
            flow_arn: flow_arn.clone(),
            output_arn: never_minted.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(never_minted),
        },
        RefusalCase {
            case: "an output of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            output_arn: other_output.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(other_output.clone()),
        },
        RefusalCase {
            case: "an output already removed",
            flow_arn: flow_arn.clone(),
            output_arn: gone.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(gone),
        },
        RefusalCase {
            // `OutputArn` keeps its own modelled pattern `^arn:.+:mediaconnect.+:output:.+$`
            // (`smithy_query(query="operation",operation="RemoveFlowOutput")`), so a label
            // outside it is refused by the boundary walk in this service's
            // `BadRequestException` envelope before the handler runs.
            case: "an output label that is no output ARN at all",
            flow_arn: flow_arn.clone(),
            output_arn: "not-an-output-arn".to_string(),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "an output ARN of another service",
            flow_arn: flow_arn.clone(),
            output_arn: format!("arn:aws:medialive:{REGION}:111122223333:output:1-0000beef:x"),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        output_arn: target_output,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, body) = remove_flow_output(&emulator, &target_flow, &target_output).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        match &not_found_identifier {
            Some(identifier) => assert_eq!(
                body["message"].as_str(),
                Some(mc::not_found_message(identifier).as_str()),
                "case {case:?}: the envelope names the identifier that resolved nothing, got \
                 {body}"
            ),
            None => assert!(
                body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: a refused request keeps this service's validation envelope, got \
                 {body}"
            ),
        }

        let after = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused remove stores nothing, got {after:?}"
        );
        let other_after = described_outputs(&emulator, case, &other_flow).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: a refused remove leaves every other flow alone, got {other_after:?}"
        );
    }
}

/// An output name is unique within its flow only while the output is there: once it is
/// removed the name is free again, for a caller that chooses it and for the service's own
/// generated `output-<n>`, and the output added in its place is a NEW resource with its
/// own ARN.
#[tokio::test]
async fn a_removed_output_frees_its_name_for_the_flow() {
    let emulator = Emulator::new();
    let case = "a removed output's name is free again";
    let flow_arn = create_flow_with_outputs(
        &emulator,
        case,
        "flow-remove-names",
        json!([
            { "name": "output-1", "protocol": "udp", "destination": "203.0.113.70", "port": 8200 },
            { "name": "recycled", "protocol": "udp", "destination": "203.0.113.71", "port": 8202 }
        ]),
    )
    .await;
    let held = described_outputs(&emulator, case, &flow_arn).await;
    let recycled_arn = held[1]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The precondition: while the output is there, its name is taken.
    let fresh =
        json!([{ "name": "recycled", "protocol": "udp", "destination": "203.0.113.72", "port": 8204 }]);
    let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, fresh.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a name the flow still uses is refused, got {status} {code:?}: {body}"
    );

    let (status, code, body) = remove_flow_output(&emulator, &flow_arn, &recycled_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the remove expected 202, got {status} {code:?}: {body}"
    );

    // The chosen name is free, and what takes it is a new resource with its own ARN.
    let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, fresh).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the freed name is accepted again, got {status} {code:?}: {body}"
    );
    let re_added = body["outputs"][0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_minted_output_arn(case, &re_added, "recycled");
    assert_ne!(
        re_added, recycled_arn,
        "case {case:?}: the output added in its place is a new resource"
    );

    // And the generated name the service hands an unnamed output is the first free one,
    // which the removal put back in play.
    let outputs = described_outputs(&emulator, case, &flow_arn).await;
    let generated_arn = outputs[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, body) = remove_flow_output(&emulator, &flow_arn, &generated_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: removing the generated-name output expected 202, got {status} {code:?}: \
         {body}"
    );
    let (status, code, body) =
        add_flow_outputs(&emulator, &flow_arn, json!([{ "protocol": "udp" }])).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: an unnamed output is accepted, got {status} {code:?}: {body}"
    );
    assert_eq!(
        body["outputs"][0]["name"].as_str(),
        Some("output-1"),
        "case {case:?}: the generated name is the first free one, got {body}"
    );
}

/// A flow in a transient status still serves a remove: the removal materialises only the
/// status change the CLOCK already owed, never one of its own, so the caller sees the
/// transient status before the span is up and the settled one after it — with the output
/// gone either way.
#[tokio::test]
async fn removing_an_output_leaves_the_clock_to_move_the_status() {
    let emulator = Emulator::new();
    let case = "a remove while the flow is starting";
    let flow_arn = create_flow_with_outputs(
        &emulator,
        case,
        "flow-remove-transient",
        numbered_outputs(0, 2),
    )
    .await;

    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref(), body["status"].as_str()),
        (202, None, Some(mc::STATUS_STARTING)),
        "case {case:?}: StartFlow expected 202 STARTING, got {status} {code:?}: {body}"
    );

    // Inside the transition span: the remove is served and the flow is still STARTING.
    remove_at(&emulator, case, &flow_arn, 0).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_STARTING,
        "case {case:?}: the flow is still in its transient status"
    );

    // Past the span: the clock settles the status, and the output is still gone.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_ACTIVE,
        "case {case:?}: the clock settles the status the start owed"
    );
    let outputs = described_outputs(&emulator, case, &flow_arn).await;
    let names: Vec<&str> = outputs
        .iter()
        .map(|output| output["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["out-1"],
        "case {case:?}: the removed output stays removed, got {names:?}"
    );
}
