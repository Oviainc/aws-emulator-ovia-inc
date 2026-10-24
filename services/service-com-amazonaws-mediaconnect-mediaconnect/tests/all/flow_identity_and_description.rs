// SPDX-License-Identifier: CC0-1.0

//! What identifies a flow, what its `description` is, and the 2xx a create answers with.
//!
//! Three rules, each asserted over a TABLE that spans its class rather than over one
//! request:
//!
//! 1. A flow is identified by the ARN the service mints for it, which carries a per-flow
//!    generated id as well as the name (`flow:1-<id>:<name>`), so a flow NAME is not an
//!    identity: the model declares no uniqueness rule on `Flow.Name`
//!    (`smithy_query(query="shape",shape="Flow")` — `Name` is `required`, with no
//!    `@unique`-style constraint and no documented rule), and two flows may carry one
//!    name and still be distinct resources.
//! 2. A flow's `description` is SERVICE-DERIVED when the flow is created — CreateFlow
//!    models no `Description` member — from the flow's own source: the source's
//!    description when the request gives a non-blank one, otherwise a sentence naming the
//!    flow and that source. It is reported, non-empty, on `Flow.Description` (DescribeFlow,
//!    CreateFlow, UpdateFlow) and on the REQUIRED `ListedFlow.Description` (ListFlows), and
//!    both report the SAME value.
//! 3. A POST that CREATES something on this service answers HTTP 201, entitlements
//!    included.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the same `MultiServiceRouter`
//! the emulator's front door uses, on the harness `flow_lifecycle` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;

/// A subscriber account id for the entitlement grants below.
const SUBSCRIBER: &str = "111122223333";

/// Create one flow and return `(status, error code, body)`.
async fn create(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.create_flow(body).await
}

/// The `flowArn` of a created flow.
fn created_arn(case: &str, body: &Value) -> String {
    body["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created flow must carry an ARN, got {body}"))
        .to_string()
}

/// The eight colon-separated segments of an ARN this service mints.
fn segments(arn: &str) -> Vec<String> {
    arn.split(':').map(ToString::to_string).collect()
}

/// Every flow ListFlows reports, as `(flowArn, name, description)`.
async fn listed(emulator: &Emulator) -> Vec<(String, String, Value)> {
    let (status, code, body) = emulator.list_flows("?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "ListFlows expected 200 with no error, got {status} {code:?}: {body}"
    );
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| {
            (
                flow["flowArn"].as_str().unwrap_or_default().to_string(),
                flow["name"].as_str().unwrap_or_default().to_string(),
                flow["description"].clone(),
            )
        })
        .collect()
}

/// One row of the flow-name table: a name, and how many flows to create under it.
struct NameCase {
    case: &'static str,
    name: String,
    copies: usize,
}

/// The names asserted below. The rows differ along what the rule generalises over: the
/// FORM of the name (single character, digits only, mixed case, the underscore and hyphen
/// the modelled `FlowArn` segment allows, at either end, a long name) and the NUMBER of
/// flows sharing it (two, three).
///
/// Every name is a legal last ARN segment per the modelled `FlowArn` pattern
/// (`smithy_query(query="shape",shape="FlowArn")`:
/// `…:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`), which is the only rule this service applies
/// to a flow name.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            case: "the plain name of the report, created twice",
            name: "dup".to_string(),
            copies: 2,
        },
        NameCase {
            case: "a one-character name",
            name: "a".to_string(),
            copies: 2,
        },
        NameCase {
            case: "mixed case with both separators, three flows deep",
            name: "Mixed_Case-Flow-9".to_string(),
            copies: 3,
        },
        NameCase {
            case: "digits only",
            name: "0123456789".to_string(),
            copies: 2,
        },
        NameCase {
            case: "separators at both ends",
            name: "_leading-and-trailing_".to_string(),
            copies: 2,
        },
        NameCase {
            case: "a long name",
            name: "x".repeat(128),
            copies: 2,
        },
    ]
}

/// A flow NAME is not a flow's identity: creating a second (and third) flow under the
/// name of a LIVE flow succeeds with HTTP 201, and the flows are distinct resources —
/// their ARNs differ only in the generated id segment, each is readable on its own ARN,
/// and ListFlows reports every one of them.
#[tokio::test]
async fn a_flow_name_may_be_shared_because_the_minted_arn_is_the_identity() {
    let emulator = Emulator::new();
    let mut expected_total = 0_usize;

    for NameCase { case, name, copies } in name_cases() {
        let mut arns: Vec<String> = Vec::new();
        for copy in 0..copies {
            let (status, code, body) = create(
                &emulator,
                json!({
                    "name": name,
                    "source": {
                        "name": format!("{name}-src"),
                        "protocol": "rtp",
                        "whitelistCidr": "10.0.0.0/16"
                    }
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: copy {copy} of name {name:?} expected 201 with no error, got \
                 {status} {code:?}: {body}"
            );
            assert_eq!(
                body["flow"]["name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: copy {copy} keeps the requested name, got {body}"
            );
            arns.push(created_arn(case, &body));
        }
        expected_total += copies;

        // The ARNs are distinct, and they differ ONLY in the generated id segment: same
        // partition, service, Region, account, resource type and name.
        let distinct: BTreeSet<&String> = arns.iter().collect();
        assert_eq!(
            distinct.len(),
            copies,
            "case {case:?}: each flow has its own ARN, got {arns:?}"
        );
        let first = segments(&arns[0]);
        for (copy, arn) in arns.iter().enumerate() {
            let parts = segments(arn);
            assert_eq!(
                parts.len(),
                8,
                "case {case:?}: copy {copy} ARN has 8 segments, got {arn:?}"
            );
            let differing: Vec<usize> = (0..8).filter(|&i| parts[i] != first[i]).collect();
            assert!(
                differing.iter().all(|&i| i == 6),
                "case {case:?}: copy {copy} may differ from {:?} only in the generated id \
                 segment, got {arn:?} (segments {differing:?} differ)",
                arns[0]
            );
            assert_eq!(
                parts[7], name,
                "case {case:?}: copy {copy} ARN still ends with the flow name, got {arn:?}"
            );

            // Each one is a resource of its own: DescribeFlow on its ARN answers it.
            let (status, code, described) = emulator.describe_flow(arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: copy {copy} must be readable on its own ARN, got {status} \
                 {code:?}: {described}"
            );
            assert_eq!(
                (
                    described["flow"]["flowArn"].as_str(),
                    described["flow"]["name"].as_str()
                ),
                (Some(arn.as_str()), Some(name.as_str())),
                "case {case:?}: copy {copy} reports its own ARN and the shared name, got \
                 {described}"
            );
        }

        // And ListFlows reports every flow under the name, not just the first.
        let page = listed(&emulator).await;
        let under_name: Vec<&String> = page
            .iter()
            .filter(|(_, listed_name, _)| listed_name == &name)
            .map(|(arn, ..)| arn)
            .collect();
        assert_eq!(
            under_name.len(),
            copies,
            "case {case:?}: ListFlows must report all {copies} flows named {name:?}, got \
             {under_name:?}"
        );
    }

    // Nothing was dropped or overwritten along the way: the store holds one flow per
    // create that answered 201.
    let page = listed(&emulator).await;
    assert_eq!(
        page.len(),
        expected_total,
        "every created flow is still stored, got {} entries",
        page.len()
    );
}

/// One row of the description table: a CreateFlow body, and the description the service
/// must derive for the flow it creates.
struct DescriptionCase {
    case: &'static str,
    body: Value,
    expected: &'static str,
}

/// The descriptions asserted below. The rows differ along what the rule generalises over:
/// whether the source carries a description at all, whether that description is blank
/// (which is not a description), how many sources the flow has and which of them carries
/// one, and whether the source is named by the request or by the service.
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        DescriptionCase {
            case: "the source carries a description, which the flow takes",
            body: json!({
                "name": "desc-echo",
                "source": {
                    "name": "probe-src",
                    "description": "probe source",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16"
                }
            }),
            expected: "probe source",
        },
        DescriptionCase {
            case: "the source carries none, so the service generates the sentence",
            body: json!({
                "name": "desc-generated",
                "source": {
                    "name": "desc-generated-src",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16"
                }
            }),
            expected: "desc-generated using source desc-generated-src",
        },
        DescriptionCase {
            case: "the source is not named either, so it takes the flow's name",
            body: json!({
                "name": "desc-unnamed-source",
                "source": { "protocol": "udp", "whitelistCidr": "192.0.2.0/24" }
            }),
            expected: "desc-unnamed-source using source desc-unnamed-source",
        },
        DescriptionCase {
            case: "a blank source description is not a description",
            body: json!({
                "name": "desc-blank",
                "source": {
                    "name": "desc-blank-src",
                    "description": "   ",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16"
                }
            }),
            expected: "desc-blank using source desc-blank-src",
        },
        DescriptionCase {
            case: "a failover pair whose first source carries the description",
            body: json!({
                "name": "desc-pair-first",
                // A flow holding a pair of sources has source failover ACTIVE: "if the
                // state is active, the flow can have one or two sources"
                // (`FailoverConfig$State`).
                "sourceFailoverConfig": { "state": "ENABLED" },
                "sources": [
                    {
                        "name": "primary",
                        "description": "the primary feed",
                        "protocol": "zixi-push",
                        "whitelistCidr": "10.1.0.0/16"
                    },
                    {
                        "name": "secondary",
                        "description": "the standby feed",
                        "protocol": "zixi-push",
                        "whitelistCidr": "10.2.0.0/16"
                    }
                ]
            }),
            expected: "the primary feed",
        },
        DescriptionCase {
            case: "a failover pair where only the second source carries one",
            body: json!({
                "name": "desc-pair-second",
                "sourceFailoverConfig": { "state": "ENABLED" },
                "sources": [
                    { "name": "primary", "protocol": "rist", "whitelistCidr": "10.1.0.0/16" },
                    {
                        "name": "secondary",
                        "description": "the standby feed",
                        "protocol": "rist",
                        "whitelistCidr": "10.2.0.0/16"
                    }
                ]
            }),
            expected: "desc-pair-second using source primary",
        },
        DescriptionCase {
            case: "an entitled source's description, carried verbatim",
            body: json!({
                "name": "desc-entitled",
                "source": {
                    "name": "from-partner",
                    "description": "Zuliefer-Feed \u{2014} 24/7 \u{1f3a5}",
                    "entitlementArn":
                        "arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-0000000f:shared"
                }
            }),
            expected: "Zuliefer-Feed \u{2014} 24/7 \u{1f3a5}",
        },
    ]
}

/// A flow's `description` is derived from its source when the flow is created, and the
/// SAME non-empty value is reported by CreateFlow, by DescribeFlow and by ListFlows —
/// where the member is required, so it is never the empty string.
#[tokio::test]
async fn a_flow_description_is_derived_from_its_source_and_reported_everywhere() {
    let emulator = Emulator::new();
    let mut created_arns: Vec<(&str, String, &str)> = Vec::new();

    for DescriptionCase {
        case,
        body,
        expected,
    } in description_cases()
    {
        let (status, code, created) = create(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {created}"
        );
        assert_eq!(
            created["flow"]["description"].as_str(),
            Some(expected),
            "case {case:?}: CreateFlow reports the derived description, got {:?}",
            created["flow"]["description"]
        );

        let flow_arn = created_arn(case, &created);
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: DescribeFlow expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["flow"]["description"].as_str(),
            Some(expected),
            "case {case:?}: DescribeFlow reports the stored description, got {:?}",
            described["flow"]["description"]
        );

        created_arns.push((case, flow_arn, expected));
    }

    // ListFlows reports the same value on every entry, as a present, non-empty string.
    let page = listed(&emulator).await;
    assert_eq!(
        page.len(),
        created_arns.len(),
        "every flow created above is listed, got {} entries",
        page.len()
    );
    for (case, flow_arn, expected) in &created_arns {
        let entry = page
            .iter()
            .find(|(arn, ..)| arn == flow_arn)
            .unwrap_or_else(|| panic!("case {case:?}: flow {flow_arn} must be listed"));
        assert_eq!(
            entry.2.as_str(),
            Some(*expected),
            "case {case:?}: ListFlows reports the same description DescribeFlow does, got {:?}",
            entry.2
        );
    }
    for (arn, name, description) in &page {
        assert!(
            description.as_str().is_some_and(|value| !value.is_empty()),
            "flow {name:?} ({arn}): ListedFlow.Description is required and non-empty, got \
             {description:?}"
        );
    }
}

/// One row of the success-status table: a POST that creates something on a flow.
struct CreatedCase {
    case: &'static str,
    /// The path under `/v1/flows/{FlowArn}`, empty for the flow itself.
    suffix: &'static str,
    body: Value,
}

/// A POST that CREATES resources on this service answers HTTP 201 Created — the status
/// the recorded wire responses carry — whether what it creates is a flow, its outputs,
/// its media streams or its ENTITLEMENTS. A read of the same flow stays 200.
#[tokio::test]
async fn a_post_that_creates_answers_201_created() {
    let emulator = Emulator::new();
    let (status, code, created) = create(
        &emulator,
        json!({
            "name": "created-status-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateFlow expected 201 Created, got {status} {code:?}: {created}"
    );
    let flow_arn = created_arn("the flow under test", &created);

    let cases = vec![
        CreatedCase {
            case: "GrantFlowEntitlements, one named entitlement",
            suffix: "/entitlements",
            body: json!({ "entitlements": [{ "name": "e1", "subscribers": [SUBSCRIBER] }] }),
        },
        CreatedCase {
            case: "GrantFlowEntitlements, three entitlements the request does not name",
            suffix: "/entitlements",
            body: json!({ "entitlements": [
                { "subscribers": [SUBSCRIBER] },
                { "subscribers": [SUBSCRIBER] },
                { "subscribers": [SUBSCRIBER] },
            ] }),
        },
        CreatedCase {
            case: "AddFlowOutputs, the sibling that adds outputs",
            suffix: "/outputs",
            body: json!({ "outputs": [
                { "name": "out-a", "protocol": "udp", "destination": "203.0.113.10", "port": 5000 },
            ] }),
        },
        CreatedCase {
            case: "AddFlowMediaStreams, the sibling that adds media streams",
            suffix: "/mediaStreams",
            body: json!({ "mediaStreams": [
                { "mediaStreamId": 1, "mediaStreamName": "video-1", "mediaStreamType": "video" },
            ] }),
        },
    ];

    for CreatedCase { case, suffix, body } in cases {
        let (status, code, response) = emulator
            .send("POST", &format!("/v1/flows/{flow_arn}{suffix}"), Some(body))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 Created with no error, got {status} {code:?}: {response}"
        );
    }

    // Reading the flow back is not a create: it stays 200, and it reports everything the
    // calls above created.
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DescribeFlow expected 200, got {status} {code:?}: {described}"
    );
    assert_eq!(
        (
            described["flow"]["entitlements"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            described["flow"]["outputs"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            described["flow"]["mediaStreams"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
        ),
        (4, 1, 1),
        "the 201s above persisted what they created, got {described}"
    );
}
