// SPDX-License-Identifier: CC0-1.0

//! GrantFlowEntitlements, end to end over HTTP: the entitlements a grant mints are
//! PERSISTED on the flow, so the grant's own response and a later DescribeFlow report
//! the same records.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the same `MultiServiceRouter`
//! the emulator's front door uses, on the harness `flow_lifecycle` already owns, so what
//! these tests assert is what a MediaConnect client sees on the wire.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::flow_lifecycle::{Emulator, REGION};

/// The ceiling this service applies to the entitlements ONE FLOW holds.
const CEILING: usize = service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOW_ENTITLEMENTS;

/// An account id used as an entitlement subscriber: "the Amazon Web Services account IDs
/// that you want to share your content with"
/// (`smithy_query(query="shape",shape="GrantEntitlementRequest",member="Subscribers")`).
const SUBSCRIBER: &str = "111122223333";

/// Every entitlement ARN the service mints names this service, the request's Region, a
/// 12-digit account, the `entitlement` resource type and, last, the entitlement's name.
///
/// `Entitlement$EntitlementArn` targets `String`, so the model fixes no pattern; what is
/// asserted here is that the minted value is a well-formed ARN for this service's
/// namespace and that it carries the name the entitlement reports.
fn assert_minted_entitlement_arn(case: &str, arn: &str, name: &str) {
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: an entitlement ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "entitlement"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the entitlement name, got {arn:?}"
    );
}

/// The `entitlements` member of a response body, as the raw JSON entries.
fn entitlements(body: &Value) -> Vec<Value> {
    body["entitlements"].as_array().cloned().unwrap_or_default()
}

/// The `name` of each entry of an `entitlements` member.
fn entitlement_names(body: &Value) -> Vec<String> {
    entitlements(body)
        .iter()
        .map(|entitlement| entitlement["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A grant request body for `count` entitlements named `<prefix>-<n>`.
fn many(prefix: &str, count: usize) -> Value {
    json!({
        "entitlements": (0..count)
            .map(|index| json!({
                "name": format!("{prefix}-{index}"),
                "subscribers": [SUBSCRIBER],
            }))
            .collect::<Vec<Value>>()
    })
}

/// Send one GrantFlowEntitlements request.
async fn grant(
    emulator: &Emulator,
    flow_arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/entitlements"),
            Some(body),
        )
        .await
}

/// Create a flow carrying `created` entitlements and answer its ARN.
async fn flow_with(emulator: &Emulator, name: &str, created: &[Value]) -> String {
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "entitlements": created,
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating flow {name:?} expected 201, got {status} {code:?}: {body}"
    );
    body["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One grant: the entitlements the flow already carries, the entitlements the request
/// grants, and the names and statuses the service must give them.
struct GrantCase {
    case: &'static str,
    /// `Entitlements` of the CreateFlow request, i.e. what the flow carries already.
    created: Vec<Value>,
    /// `Entitlements` of the GrantFlowEntitlements request.
    granted: Vec<Value>,
    /// The `name` each granted entitlement must report, in request order.
    names: Vec<&'static str>,
    /// The `entitlementStatus` each granted entitlement must report, in request order.
    statuses: Vec<&'static str>,
}

/// The grants asserted below. The rows differ along what the rule generalises over: how
/// many entitlements one request grants, whether the flow already carries any, whether
/// the request names them, and which optional members it sets.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            case: "one entitlement with every optional member set, on a flow with none",
            created: vec![],
            granted: vec![json!({
                "name": "full-entitlement",
                "description": "every member set",
                // "Percentage from 0-100 of the data transfer cost to be billed to the
                // subscriber" (`GrantEntitlementRequest$DataTransferSubscriberFeePercent`).
                "dataTransferSubscriberFeePercent": 42,
                // `EntitlementStatus` (ENABLED | DISABLED), explicitly the non-default one.
                "entitlementStatus": "DISABLED",
                "encryption": {
                    "algorithm": "aes128",
                    "keyType": "static-key",
                    "roleArn": "arn:aws:iam::111122223333:role/MediaConnectRole",
                    "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:key",
                },
                "subscribers": [SUBSCRIBER],
            })],
            names: vec!["full-entitlement"],
            statuses: vec!["DISABLED"],
        },
        GrantCase {
            case: "two entitlements in one call, on a flow that already carries one",
            created: vec![json!({ "name": "created-entitlement", "subscribers": [SUBSCRIBER] })],
            granted: vec![
                json!({ "name": "second", "subscribers": [SUBSCRIBER] }),
                json!({
                    "name": "third",
                    "description": "the third one",
                    "subscribers": [SUBSCRIBER, "222233334444", "333344445555"],
                }),
            ],
            // `EntitlementStatus` omitted: "if you don't specify the entitlementStatus
            // field in your request, MediaConnect sets it to ENABLED".
            names: vec!["second", "third"],
            statuses: vec!["ENABLED", "ENABLED"],
        },
        GrantCase {
            case: "three entitlements the request does not name",
            created: vec![],
            granted: vec![
                json!({ "subscribers": [SUBSCRIBER] }),
                json!({ "subscribers": [SUBSCRIBER] }),
                json!({ "subscribers": [SUBSCRIBER], "entitlementStatus": "DISABLED" }),
            ],
            names: vec!["entitlement-1", "entitlement-2", "entitlement-3"],
            statuses: vec!["ENABLED", "ENABLED", "DISABLED"],
        },
        GrantCase {
            case: "an unnamed entitlement on a flow already holding the first default name",
            created: vec![json!({ "name": "entitlement-1", "subscribers": [SUBSCRIBER] })],
            granted: vec![json!({ "subscribers": [SUBSCRIBER] })],
            names: vec!["entitlement-2"],
            statuses: vec!["ENABLED"],
        },
        GrantCase {
            case: "the subscriber fee percent at both published bounds",
            created: vec![],
            granted: vec![
                json!({ "name": "fee-min", "dataTransferSubscriberFeePercent": 0, "subscribers": [SUBSCRIBER] }),
                json!({ "name": "fee-max", "dataTransferSubscriberFeePercent": 100, "subscribers": [SUBSCRIBER] }),
            ],
            names: vec!["fee-min", "fee-max"],
            statuses: vec!["ENABLED", "ENABLED"],
        },
        GrantCase {
            case: "an entitlement explicitly enabled, with an empty subscriber list",
            created: vec![json!({ "name": "created-entitlement", "subscribers": [SUBSCRIBER] })],
            granted: vec![json!({
                "name": "explicitly-enabled",
                "entitlementStatus": "ENABLED",
                "subscribers": [],
            })],
            names: vec!["explicitly-enabled"],
            statuses: vec!["ENABLED"],
        },
    ]
}

/// GrantFlowEntitlements mints each requested entitlement into the STORED flow record:
/// its response is that record (name, status, subscribers and every optional member it
/// was given, under an ARN this service minted), it APPENDS to the entitlements the flow
/// already carries, and DescribeFlow afterwards reports exactly the same entries.
#[tokio::test]
async fn grant_flow_entitlements_persists_what_it_returns() {
    let emulator = Emulator::new();

    for (row, case) in grant_cases().into_iter().enumerate() {
        let GrantCase {
            case,
            created,
            granted,
            names,
            statuses,
        } = case;
        let flow_arn = flow_with(&emulator, &format!("grant-flow-{row}"), &created).await;

        let (status, code, body) = grant(
            &emulator,
            &flow_arn,
            json!({ "entitlements": granted.clone() }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: granting expected 201, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the entitlements were granted to, got {body}"
        );

        let reported = entitlements(&body);
        assert_eq!(
            reported.len(),
            granted.len(),
            "case {case:?}: the response reports every granted entitlement, got {body}"
        );

        for (index, (request, entitlement)) in granted.iter().zip(reported.iter()).enumerate() {
            assert_eq!(
                entitlement["name"].as_str(),
                Some(names[index]),
                "case {case:?}: entitlement {index} name, got {entitlement}"
            );
            assert_eq!(
                entitlement["entitlementStatus"].as_str(),
                Some(statuses[index]),
                "case {case:?}: entitlement {index} status, got {entitlement}"
            );
            assert_eq!(
                entitlement["subscribers"], request["subscribers"],
                "case {case:?}: entitlement {index} subscribers, got {entitlement}"
            );
            // Every optional member the request set is reported back unchanged; one it
            // left out stays absent.
            for member in [
                "description",
                "dataTransferSubscriberFeePercent",
                "encryption",
            ] {
                assert_eq!(
                    entitlement[member], request[member],
                    "case {case:?}: entitlement {index} member {member:?}, got {entitlement}"
                );
            }
            assert_minted_entitlement_arn(
                case,
                entitlement["entitlementArn"].as_str().unwrap_or_default(),
                names[index],
            );
        }

        // The grant APPENDS: the flow reports the entitlements it already carried, then
        // the ones just granted, byte for byte as the grant reported them.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
        );
        let stored = entitlements(&described["flow"]);
        assert_eq!(
            stored.len(),
            created.len() + granted.len(),
            "case {case:?}: the flow carries the created and the granted entitlements, got {described}"
        );
        assert_eq!(
            &stored[created.len()..],
            reported.as_slice(),
            "case {case:?}: DescribeFlow must report exactly what the grant returned, got {described}"
        );
        let created_names: Vec<String> = created
            .iter()
            .map(|entitlement| entitlement["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            entitlement_names(&described["flow"])[..created.len()],
            created_names[..],
            "case {case:?}: the entitlements the flow already carried are untouched, got {described}"
        );

        // Every entitlement ARN on the flow is distinct: the ARN identifies one
        // entitlement.
        let arns: BTreeSet<&str> = stored
            .iter()
            .map(|entitlement| entitlement["entitlementArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            arns.len(),
            stored.len(),
            "case {case:?}: each entitlement has its own ARN, got {described}"
        );
    }
}

/// One refused grant: the flow ARN to address and the body to send, with the status and
/// error code the service answers with.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    body: Value,
    status: u16,
    code: &'static str,
}

/// GrantFlowEntitlements refuses an unknown flow with `NotFoundException` (404), and a
/// request that breaks a rule of the class with `BadRequestException` (400) — never with
/// a 501, never with a success, and never storing anything.
#[tokio::test]
async fn grant_flow_entitlements_refusals_name_the_modelled_error() {
    let emulator = Emulator::new();
    let created = vec![json!({ "name": "taken", "subscribers": [SUBSCRIBER] })];
    let flow_arn = flow_with(&emulator, "refusal-flow", &created).await;

    let cases = vec![
        RefusalCase {
            case: "grant on a flow ARN that is well formed but names no flow",
            flow_arn: "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:missing"
                .to_string(),
            body: many("ent", 1),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "grant on an ARN that fails the modelled pattern",
            flow_arn: "not-an-arn".to_string(),
            body: many("ent", 1),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "grant a name the flow already carries",
            flow_arn: flow_arn.clone(),
            body: json!({ "entitlements": [{ "name": "taken", "subscribers": [SUBSCRIBER] }] }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "grant the same name twice in one request",
            flow_arn: flow_arn.clone(),
            body: json!({ "entitlements": [
                { "name": "twice", "subscribers": [SUBSCRIBER] },
                { "name": "twice", "subscribers": [SUBSCRIBER] },
            ] }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "grant more entitlements than the flow's ceiling leaves room for",
            flow_arn: flow_arn.clone(),
            body: many("bulk", CEILING),
            status: 420,
            code: "GrantFlowEntitlements420Exception",
        },
        RefusalCase {
            case: "grant an empty list of entitlements",
            flow_arn: flow_arn.clone(),
            body: json!({ "entitlements": [] }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "grant an entitlement with no Subscribers, the required member",
            flow_arn: flow_arn.clone(),
            body: json!({ "entitlements": [{ "name": "subscriberless" }] }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "grant with no Entitlements member at all",
            flow_arn: flow_arn.clone(),
            body: json!({}),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        flow_arn: addressed,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, response) = grant(&emulator, &addressed, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: {response}"
        );
    }

    // Not one refusal stored an entitlement: the flow still carries exactly the one its
    // create request granted.
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describing after the refusals expected 200, got {status} {code:?}: {described}"
    );
    assert_eq!(
        entitlement_names(&described["flow"]),
        vec!["taken".to_string()],
        "a refused grant must store nothing, got {described}"
    );
}

/// A flow's entitlement list is BOUNDED: a grant that fills it to [`CEILING`] is stored,
/// and a later grant past that ceiling is refused with the modelled
/// `GrantFlowEntitlements420Exception` (HTTP 420) without storing anything, so the bound
/// is a ceiling on what is stored rather than a counter of calls.
#[tokio::test]
async fn grant_flow_entitlements_refuses_past_the_entitlement_ceiling() {
    let emulator = Emulator::new();
    // One entitlement comes from the create request, so the grant below fills the rest.
    let created = vec![json!({ "name": "created", "subscribers": [SUBSCRIBER] })];
    let flow_arn = flow_with(&emulator, "ceiling-flow", &created).await;

    let (status, code, body) = grant(&emulator, &flow_arn, many("fill", CEILING - 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "filling the flow to the ceiling expected 201, got {status} {code:?}: {body}"
    );
    assert_eq!(
        entitlements(&body).len(),
        CEILING - 1,
        "the grant at the ceiling reports every entitlement it granted, got {body}"
    );

    let (status, code, body) = grant(&emulator, &flow_arn, many("past", 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("GrantFlowEntitlements420Exception")),
        "granting past the ceiling expected 420, got {status} {code:?}: {body}"
    );

    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describing after the refusal expected 200, got {status} {code:?}: {described}"
    );
    let names = entitlement_names(&described["flow"]);
    assert_eq!(
        names.len(),
        CEILING,
        "the flow holds exactly the ceiling, got {} names",
        names.len()
    );
    assert!(
        names.iter().all(|name| !name.starts_with("past-")),
        "the refused grant stored nothing, got {names:?}"
    );
}

/// An entitlement name is unique within its flow, so a name the service generates is the
/// first free `entitlement-<n>` — across calls, and around names a caller chose itself.
#[tokio::test]
async fn generated_entitlement_names_stay_unique_across_calls() {
    let emulator = Emulator::new();
    let flow_arn = flow_with(&emulator, "generated-names-flow", &[]).await;

    // Each row is one grant on the same flow, and the names its entitlements must take.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "the first unnamed entitlement takes the first free name",
            json!({ "entitlements": [{ "subscribers": [SUBSCRIBER] }] }),
            vec!["entitlement-1"],
        ),
        (
            "a later call continues past the names already stored",
            json!({ "entitlements": [
                { "subscribers": [SUBSCRIBER] },
                { "subscribers": [SUBSCRIBER] },
            ] }),
            vec!["entitlement-2", "entitlement-3"],
        ),
        (
            "a caller-chosen name is kept as it is",
            json!({ "entitlements": [{ "name": "entitlement-5", "subscribers": [SUBSCRIBER] }] }),
            vec!["entitlement-5"],
        ),
        (
            "a generated name skips the caller-chosen one it would collide with",
            json!({ "entitlements": [
                { "subscribers": [SUBSCRIBER] },
                { "subscribers": [SUBSCRIBER] },
            ] }),
            vec!["entitlement-4", "entitlement-6"],
        ),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, body, names) in rows {
        let (status, code, response) = grant(&emulator, &flow_arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: granting expected 201, got {status} {code:?}: {response}"
        );
        assert_eq!(
            entitlement_names(&response),
            names
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: the names the grant reports, got {response}"
        );
        expected.extend(names.iter().map(ToString::to_string));

        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describing expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            entitlement_names(&described["flow"]),
            expected,
            "case {case:?}: the flow carries every entitlement granted so far, got {described}"
        );
    }
}
