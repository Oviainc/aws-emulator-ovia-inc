// SPDX-License-Identifier: CC0-1.0

//! RevokeFlowEntitlement, end to end over HTTP: one call drops exactly the entitlement
//! whose ARN it names from the flow it names, and nothing else about that flow changes.
//!
//! The rule is asserted over a TABLE spanning the class rather than over the one request
//! that exposed the stub: a revoke answers 202 with the stored flow's ARN and the revoked
//! entitlement's ARN, the flow then carries the entitlements it carried MINUS that one IN
//! THE SAME ORDER, the entitlement's name is free again, and a request naming a flow the
//! store does not hold or an entitlement that flow does not carry is refused
//! `NotFoundException` (404) and revokes nothing.
//!
//! The rows differ along what the rule generalises over: how many entitlements the flow
//! carries, where in its list the revoked one sits (only, first, middle, last), how many
//! calls revoke one, whether the revoked entitlement was stored by CreateFlow or granted
//! later by GrantFlowEntitlements, and which optional members it carried.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_entitlements::{entitlement_names, entitlements, flow_with, grant};
use crate::flow_lifecycle::{Emulator, REGION};

/// An account id used as an entitlement subscriber: "the Amazon Web Services account IDs
/// that you want to share your content with"
/// (`smithy_query(query="shape",shape="GrantEntitlementRequest",member="Subscribers")`).
const SUBSCRIBER: &str = "111122223333";

/// `DELETE /v1/flows/{FlowArn}/entitlements/{EntitlementArn}` — the RevokeFlowEntitlement
/// binding (`smithy_query(query="operation",operation="RevokeFlowEntitlement")`).
async fn revoke(
    emulator: &Emulator,
    flow_arn: &str,
    entitlement_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/entitlements/{entitlement_arn}"),
            None,
        )
        .await
}

/// The `entitlements` of a DescribeFlow response — the stored record every case reads the
/// flow back through.
async fn described_entitlements(emulator: &Emulator, case: &str, flow_arn: &str) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describing {flow_arn} expected 200, got {status} {code:?}: {described}"
    );
    entitlements(&described["flow"])
}

/// The flow's status, as DescribeFlow reports it.
async fn described_status(emulator: &Emulator, flow_arn: &str) -> String {
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    described["flow"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `count` entitlement requests named `<prefix>-<n>` from `first`, so a row can build a
/// flow with a known number of distinguishable entitlements.
fn numbered_entitlements(prefix: &str, first: usize, count: usize) -> Vec<Value> {
    (first..first + count)
        .map(|index| json!({ "name": format!("{prefix}-{index}"), "subscribers": [SUBSCRIBER] }))
        .collect()
}

/// Revoke the entitlement at `position` of the flow's CURRENT entitlement list and assert
/// the whole rule for that one call: the 202 answer names the flow and that entitlement,
/// the flow then carries the other entitlements unchanged and in order, and its status is
/// left alone.
async fn revoke_at(emulator: &Emulator, case: &str, flow_arn: &str, position: usize) {
    let before = described_entitlements(emulator, case, flow_arn).await;
    let status_before = described_status(emulator, flow_arn).await;
    let target = before.get(position).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the flow must carry an entitlement at {position}")
    });
    let entitlement_arn = target["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, body) = revoke(emulator, flow_arn, &entitlement_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: revoking entitlement {position} expected 202 with no error, got {status} \
         {code:?}: {body}"
    );
    assert_eq!(
        (body["flowArn"].as_str(), body["entitlementArn"].as_str()),
        (Some(flow_arn), Some(entitlement_arn.as_str())),
        "case {case:?}: the response names the flow and the revoked entitlement, got {body}"
    );

    let after = described_entitlements(emulator, case, flow_arn).await;
    let expected: Vec<Value> = before
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, entitlement)| entitlement.clone())
        .collect();
    assert_eq!(
        after, expected,
        "case {case:?}: the flow carries the other entitlements, in order, got {after:?}"
    );
    assert_eq!(
        described_status(emulator, flow_arn).await,
        status_before,
        "case {case:?}: revoking an entitlement does not move the flow's status"
    );
}

/// One row of the RevokeFlowEntitlement table: the entitlements the flow is created with,
/// the entitlements GrantFlowEntitlements then appends, and the positions revoked by
/// successive calls (each one into the list as it stands when that call is made).
struct RevokeCase {
    case: &'static str,
    created: Vec<Value>,
    granted: Vec<Value>,
    revoke: &'static [usize],
}

/// The rows the rule is asserted over.
fn revoke_cases() -> Vec<RevokeCase> {
    vec![
        RevokeCase {
            case: "the only entitlement of a flow",
            created: numbered_entitlements("ent", 0, 1),
            granted: vec![],
            revoke: &[0],
        },
        RevokeCase {
            case: "the first of three",
            created: numbered_entitlements("ent", 0, 3),
            granted: vec![],
            revoke: &[0],
        },
        RevokeCase {
            case: "the middle of three",
            created: numbered_entitlements("ent", 0, 3),
            granted: vec![],
            revoke: &[1],
        },
        RevokeCase {
            case: "the last of three",
            created: numbered_entitlements("ent", 0, 3),
            granted: vec![],
            revoke: &[2],
        },
        RevokeCase {
            case: "an entitlement GrantFlowEntitlements appended, not CreateFlow",
            created: numbered_entitlements("ent", 0, 1),
            granted: numbered_entitlements("granted", 1, 2),
            revoke: &[2],
        },
        RevokeCase {
            case: "two entitlements of five, over two calls",
            created: numbered_entitlements("ent", 0, 2),
            granted: numbered_entitlements("granted", 2, 3),
            revoke: &[4, 0],
        },
        RevokeCase {
            case: "every entitlement of a flow, one call at a time",
            created: numbered_entitlements("ent", 0, 2),
            granted: numbered_entitlements("granted", 2, 1),
            revoke: &[1, 1, 0],
        },
        RevokeCase {
            case: "an entitlement carrying every optional member",
            created: vec![
                json!({
                    "name": "full",
                    "description": "every member set",
                    // "Percentage from 0-100 of the data transfer cost to be billed to the
                    // subscriber"
                    // (`GrantEntitlementRequest$DataTransferSubscriberFeePercent`).
                    "dataTransferSubscriberFeePercent": 100,
                    // `EntitlementStatus` (ENABLED | DISABLED), explicitly the non-default.
                    "entitlementStatus": "DISABLED",
                    "encryption": {
                        "algorithm": "aes256",
                        "keyType": "static-key",
                        "roleArn": "arn:aws:iam::111122223333:role/MediaConnectRole",
                        "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:key",
                    },
                    "subscribers": [SUBSCRIBER, "222233334444"],
                }),
                json!({ "name": "plain", "subscribers": [] }),
            ],
            granted: vec![],
            revoke: &[0],
        },
    ]
}

/// RevokeFlowEntitlement drops exactly the entitlement its `EntitlementArn` names from the
/// flow its `FlowArn` names, and leaves every other entitlement of that flow exactly as it
/// was.
#[tokio::test]
async fn revoke_flow_entitlement_drops_only_the_entitlement_it_names() {
    let emulator = Emulator::new();

    for (row, case) in revoke_cases().into_iter().enumerate() {
        let RevokeCase {
            case,
            created,
            granted,
            revoke: positions,
        } = case;
        let flow_arn = flow_with(&emulator, &format!("revoke-flow-{row}"), &created).await;
        if !granted.is_empty() {
            let (status, code, body) =
                grant(&emulator, &flow_arn, json!({ "entitlements": granted })).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the fixture grant must succeed, got {status} {code:?}: {body}"
            );
        }

        for position in positions {
            revoke_at(&emulator, case, &flow_arn, *position).await;
        }
    }
}

/// One row of the refusal table: a RevokeFlowEntitlement request the service must refuse,
/// the two labels it carries, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    entitlement_arn: String,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A RevokeFlowEntitlement request that resolves no stored entitlement revokes nothing: it
/// is refused with the operation's own modelled error, naming the ARN that resolved
/// nothing, and every flow still carries exactly the entitlements it carried.
#[tokio::test]
async fn revoke_flow_entitlement_refuses_what_it_cannot_revoke() {
    let emulator = Emulator::new();
    let case = "refusals";
    let flow_arn = flow_with(
        &emulator,
        "revoke-refusal-flow",
        &numbered_entitlements("ent", 0, 2),
    )
    .await;
    let other_flow = flow_with(
        &emulator,
        "revoke-refusal-other-flow",
        &numbered_entitlements("elsewhere", 0, 1),
    )
    .await;

    let held = described_entitlements(&emulator, case, &flow_arn).await;
    let other_held = described_entitlements(&emulator, case, &other_flow).await;
    let held_entitlement = held[0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other_entitlement = other_held[0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // One entitlement revoked for real, so a second call on its ARN addresses an
    // entitlement that no longer exists.
    let gone = held[1]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _, body) = revoke(&emulator, &flow_arn, &gone).await;
    assert_eq!(
        status, 202,
        "the fixture revocation must succeed, got {status}: {body}"
    );
    let before = described_entitlements(&emulator, case, &flow_arn).await;

    // An entitlement ARN of the form this service mints, for one it never minted.
    let never_minted = format!(
        "arn:aws:mediaconnect:{REGION}:111122223333:entitlement:1-0000dead:never-minted"
    );
    // A flow ARN of the form this service mints, for a flow it never minted.
    let unknown_flow =
        format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-minted");

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted, with an entitlement ARN it did mint",
            flow_arn: unknown_flow.clone(),
            entitlement_arn: held_entitlement.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow),
        },
        RefusalCase {
            // The `FlowArn` label carries no pattern the boundary walk enforces, so a label
            // that is no ARN at all simply resolves no flow.
            case: "a flow label that is not an ARN at all",
            flow_arn: "not-an-arn".to_string(),
            entitlement_arn: held_entitlement.clone(),
            status: 404,
            code: "NotFoundException",
            // No flow ARN, so no flow lookup was reached: the envelope carries the generic
            // reason rather than the caller's own text.
            not_found_identifier: Some(mc::UNRESOLVED_PATH_NOT_FOUND_REASON.to_string()),
        },
        RefusalCase {
            case: "an entitlement ARN this service never minted",
            flow_arn: flow_arn.clone(),
            entitlement_arn: never_minted.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(never_minted),
        },
        RefusalCase {
            case: "an entitlement of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            entitlement_arn: other_entitlement.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(other_entitlement.clone()),
        },
        RefusalCase {
            case: "an entitlement already revoked",
            flow_arn: flow_arn.clone(),
            entitlement_arn: gone.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(gone),
        },
        RefusalCase {
            // `EntitlementArn` keeps its own modelled pattern
            // `^arn:.+:mediaconnect.+:entitlement:.+$`
            // (`smithy_query(query="operation",operation="RevokeFlowEntitlement")`), so a
            // label outside it is refused by the boundary walk in this service's
            // `BadRequestException` envelope before the handler runs.
            case: "an entitlement label that is no entitlement ARN at all",
            flow_arn: flow_arn.clone(),
            entitlement_arn: "not-an-entitlement-arn".to_string(),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "an entitlement ARN of another service",
            flow_arn: flow_arn.clone(),
            entitlement_arn: format!(
                "arn:aws:medialive:{REGION}:111122223333:entitlement:1-0000beef:x"
            ),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
    ];

    for RefusalCase {
        case,
        flow_arn: addressed_flow,
        entitlement_arn: addressed_entitlement,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, body) = revoke(&emulator, &addressed_flow, &addressed_entitlement).await;
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

        let after = described_entitlements(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused revoke stores nothing, got {after:?}"
        );
        let other_after = described_entitlements(&emulator, case, &other_flow).await;
        assert_eq!(
            other_after, other_held,
            "case {case:?}: a refused revoke leaves every other flow alone, got {other_after:?}"
        );
    }
}

/// An entitlement name is unique within its flow only while the entitlement is there: once
/// it is revoked the name is free again, for a caller that chooses it and for the service's
/// own generated `entitlement-<n>`, and the entitlement granted in its place is a NEW
/// resource with its own ARN.
#[tokio::test]
async fn a_revoked_entitlement_frees_its_name_for_the_flow() {
    let emulator = Emulator::new();
    let case = "a revoked entitlement's name is free again";
    let flow_arn = flow_with(
        &emulator,
        "revoke-names-flow",
        &[
            json!({ "name": "entitlement-1", "subscribers": [SUBSCRIBER] }),
            json!({ "name": "recycled", "subscribers": [SUBSCRIBER] }),
        ],
    )
    .await;
    let held = described_entitlements(&emulator, case, &flow_arn).await;
    let recycled_arn = held[1]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The precondition: while the entitlement is there, its name is taken.
    let fresh = json!({ "entitlements": [{ "name": "recycled", "subscribers": [SUBSCRIBER] }] });
    let (status, code, body) = grant(&emulator, &flow_arn, fresh.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a name the flow still carries is refused, got {status} {code:?}: {body}"
    );

    let (status, code, body) = revoke(&emulator, &flow_arn, &recycled_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the revoke expected 202, got {status} {code:?}: {body}"
    );

    // The chosen name is free, and what takes it is a new resource with its own ARN.
    let (status, code, body) = grant(&emulator, &flow_arn, fresh).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the freed name is accepted again, got {status} {code:?}: {body}"
    );
    let re_granted = body["entitlements"][0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        re_granted, recycled_arn,
        "case {case:?}: the entitlement granted in its place is a new resource"
    );

    // And the generated name the service hands an unnamed entitlement is the first free
    // one, which the revocation put back in play.
    let generated_arn = described_entitlements(&emulator, case, &flow_arn).await[0]
        ["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, body) = revoke(&emulator, &flow_arn, &generated_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: revoking the generated-name entitlement expected 202, got {status} \
         {code:?}: {body}"
    );
    let (status, code, body) = grant(
        &emulator,
        &flow_arn,
        json!({ "entitlements": [{ "subscribers": [SUBSCRIBER] }] }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: an unnamed entitlement is accepted, got {status} {code:?}: {body}"
    );
    assert_eq!(
        entitlement_names(&body),
        vec!["entitlement-1".to_string()],
        "case {case:?}: the generated name is the first free one, got {body}"
    );
}

/// A flow in a transient status still serves a revoke: the revocation materialises only the
/// status change the CLOCK already owed, never one of its own, so the caller sees the
/// transient status before the span is up and the settled one after it — with the
/// entitlement gone either way.
#[tokio::test]
async fn revoking_an_entitlement_leaves_the_clock_to_move_the_status() {
    let emulator = Emulator::new();
    let case = "a revoke while the flow is starting";
    let flow_arn = flow_with(
        &emulator,
        "revoke-transient-flow",
        &numbered_entitlements("ent", 0, 2),
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

    // Inside the transition span: the revoke is served and the flow is still STARTING.
    revoke_at(&emulator, case, &flow_arn, 0).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_STARTING,
        "case {case:?}: the flow is still in its transient status"
    );

    // Past the span: the clock settles the status, and the entitlement is still gone.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_ACTIVE,
        "case {case:?}: the clock settles the status the start owed"
    );
    let (_, _, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        entitlement_names(&described["flow"]),
        vec!["ent-1".to_string()],
        "case {case:?}: the revoked entitlement stays revoked, got {described}"
    );
}
