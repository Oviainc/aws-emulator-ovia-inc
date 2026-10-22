// SPDX-License-Identifier: CC0-1.0

//! UpdateFlowEntitlement, end to end over HTTP: the members a request carries are applied
//! to the STORED entitlement the `{FlowArn, EntitlementArn}` pair addresses, every member
//! the request omits keeps its stored value, and DescribeFlow afterwards reports exactly
//! the record the update returned.
//!
//! The rule is asserted over a TABLE spanning the class of partial updates rather than the
//! one request that exposed the stub: the rows differ along which members the request
//! carries (one, several, all, none), the SIZE of the subscriber list it replaces
//! (shorter, longer, empty), whether the entitlement held an encryption already, and both
//! directions of the status toggle.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Map, Value};

use crate::flow_lifecycle::Emulator;

/// Account ids used as entitlement subscribers: "the Amazon Web Services account IDs that
/// you want to share your content with"
/// (`smithy_query(query="shape",shape="Entitlement",member="Subscribers")`).
const SUBSCRIBER_A: &str = "111122223333";
/// See [`SUBSCRIBER_A`].
const SUBSCRIBER_B: &str = "222233334444";
/// See [`SUBSCRIBER_A`].
const SUBSCRIBER_C: &str = "333344445555";

/// "The ARN of the role that you created during setup (when you set up MediaConnect as a
/// trusted entity)" (`smithy_query(query="shape",shape="Encryption",member="RoleArn")`) —
/// the one member of `Encryption` the model makes required.
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/MediaConnectRole";

/// "The ARN of the secret that you created in Secrets Manager to store the encryption key"
/// (`Encryption$SecretArn`).
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-east-1:111122223333:secret:key";

/// The entitlement every row keeps BESIDE the one it updates, so each case also asserts
/// that updating one entitlement leaves its siblings alone.
fn sibling() -> Value {
    json!({ "name": "sibling", "subscribers": [SUBSCRIBER_C] })
}

/// Create a flow carrying `entitlements` and answer its ARN plus the entitlement records
/// the create response reported, in request order.
async fn flow_with(emulator: &Emulator, name: &str, entitlements: &[Value]) -> (String, Vec<Value>) {
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "entitlements": entitlements,
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating flow {name:?} expected 201, got {status} {code:?}: {body}"
    );
    let flow_arn = body["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let stored = body["flow"]["entitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        stored.len(),
        entitlements.len(),
        "creating flow {name:?} must store every entitlement it was given, got {body}"
    );
    (flow_arn, stored)
}

/// Send one UpdateFlowEntitlement request.
async fn update(
    emulator: &Emulator,
    flow_arn: &str,
    entitlement_arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}/entitlements/{entitlement_arn}"),
            Some(body),
        )
        .await
}

/// The `entitlements` of a DescribeFlow response.
async fn described_entitlements(emulator: &Emulator, flow_arn: &str) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describing {flow_arn} expected 200, got {status} {code:?}: {described}"
    );
    described["flow"]["entitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `expected` with the entitlement's own identity members added: neither `Name` nor
/// `EntitlementArn` has a counterpart in the request shape, so an update never moves them.
fn with_identity(expected: &Value, name: &str, entitlement_arn: &str) -> Value {
    let mut members: Map<String, Value> = expected
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    members.insert("name".to_string(), json!(name));
    members.insert("entitlementArn".to_string(), json!(entitlement_arn));
    Value::Object(members)
}

/// One partial update: the entitlement the flow carries, the request body, and the whole
/// entitlement record the stored flow must report afterwards (identity members aside).
struct UpdateCase {
    case: &'static str,
    /// The `Entitlements` entry of the CreateFlow request, i.e. the stored starting point.
    created: Value,
    /// The UpdateFlowEntitlement request body.
    body: Value,
    /// Every member the updated entitlement must report, and no other.
    expected: Value,
}

fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the status toggled off on an entitlement carrying only its required members",
            created: json!({ "name": "toggle-off", "subscribers": [SUBSCRIBER_A] }),
            body: json!({ "entitlementStatus": "DISABLED" }),
            expected: json!({
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
        },
        UpdateCase {
            case: "the status toggled back on, the other direction of the same member",
            created: json!({
                "name": "toggle-on",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
            body: json!({ "entitlementStatus": "ENABLED" }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
        },
        UpdateCase {
            case: "only the description: the stored status is left exactly as it was",
            created: json!({
                "name": "described",
                "description": "before",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
            body: json!({ "description": "after" }),
            expected: json!({
                "description": "after",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
        },
        UpdateCase {
            case: "the subscriber list replaced by a longer one",
            created: json!({ "name": "more-subscribers", "subscribers": [SUBSCRIBER_A] }),
            body: json!({ "subscribers": [SUBSCRIBER_A, SUBSCRIBER_B, SUBSCRIBER_C] }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [SUBSCRIBER_A, SUBSCRIBER_B, SUBSCRIBER_C],
            }),
        },
        UpdateCase {
            case: "the subscriber list replaced by a shorter one",
            created: json!({
                "name": "fewer-subscribers",
                "subscribers": [SUBSCRIBER_A, SUBSCRIBER_B, SUBSCRIBER_C],
            }),
            body: json!({ "subscribers": [SUBSCRIBER_B] }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [SUBSCRIBER_B],
            }),
        },
        UpdateCase {
            case: "the subscriber list replaced by the empty one",
            created: json!({
                "name": "no-subscribers",
                "subscribers": [SUBSCRIBER_A, SUBSCRIBER_B],
            }),
            body: json!({ "subscribers": [] }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [],
            }),
        },
        UpdateCase {
            case: "an encryption given to an entitlement that held none",
            created: json!({ "name": "new-encryption", "subscribers": [SUBSCRIBER_A] }),
            body: json!({ "encryption": {
                "algorithm": "aes256",
                "keyType": "static-key",
                "roleArn": ROLE_ARN,
                "secretArn": SECRET_ARN,
            } }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [SUBSCRIBER_A],
                "encryption": {
                    "algorithm": "aes256",
                    "keyType": "static-key",
                    "roleArn": ROLE_ARN,
                    "secretArn": SECRET_ARN,
                },
            }),
        },
        UpdateCase {
            case: "one encryption member replaced, the rest of the stored encryption kept",
            created: json!({
                "name": "merged-encryption",
                "subscribers": [SUBSCRIBER_A],
                "encryption": {
                    "algorithm": "aes128",
                    "keyType": "static-key",
                    "roleArn": ROLE_ARN,
                    "secretArn": SECRET_ARN,
                },
            }),
            body: json!({ "encryption": { "algorithm": "aes192" } }),
            expected: json!({
                "entitlementStatus": "ENABLED",
                "subscribers": [SUBSCRIBER_A],
                "encryption": {
                    "algorithm": "aes192",
                    "keyType": "static-key",
                    "roleArn": ROLE_ARN,
                    "secretArn": SECRET_ARN,
                },
            }),
        },
        UpdateCase {
            case: "every updatable member at once, the fee percent carried over untouched",
            created: json!({
                "name": "everything",
                "description": "before",
                // "Percentage from 0-100 of the data transfer cost to be billed to the
                // subscriber" (`Entitlement$DataTransferSubscriberFeePercent`) — a member
                // the update request shape has no counterpart for.
                "dataTransferSubscriberFeePercent": 17,
                "subscribers": [SUBSCRIBER_A],
                "encryption": {
                    "algorithm": "aes128",
                    "keyType": "static-key",
                    "roleArn": ROLE_ARN,
                    "secretArn": SECRET_ARN,
                },
            }),
            body: json!({
                "description": "after",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_B, SUBSCRIBER_C],
                // A SPEKE encryption: "this parameter is required for SPEKE encryption"
                // on `Url`, `Region`, `ResourceId` and `DeviceId`
                // (`smithy_query(query="shape",shape="UpdateEncryption")`).
                "encryption": {
                    "keyType": "speke",
                    "constantInitializationVector": "0123456789abcdef0123456789abcdef",
                    "deviceId": "device-1",
                    "region": "us-west-2",
                    "resourceId": "content-1",
                    "url": "https://example.com/speke",
                },
            }),
            expected: json!({
                "description": "after",
                "dataTransferSubscriberFeePercent": 17,
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_B, SUBSCRIBER_C],
                "encryption": {
                    "algorithm": "aes128",
                    "keyType": "speke",
                    "constantInitializationVector": "0123456789abcdef0123456789abcdef",
                    "deviceId": "device-1",
                    "region": "us-west-2",
                    "resourceId": "content-1",
                    "roleArn": ROLE_ARN,
                    "secretArn": SECRET_ARN,
                    "url": "https://example.com/speke",
                },
            }),
        },
        UpdateCase {
            case: "a request that carries no updatable member at all changes nothing",
            created: json!({
                "name": "untouched",
                "description": "kept",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
            body: json!({}),
            expected: json!({
                "description": "kept",
                "entitlementStatus": "DISABLED",
                "subscribers": [SUBSCRIBER_A],
            }),
        },
    ]
}

/// UpdateFlowEntitlement applies what the request carries to the STORED entitlement and
/// answers with that record: the response is the whole updated entitlement under its
/// unchanged ARN and name, the flow reports exactly the same entry afterwards, the
/// entitlement's siblings are untouched, and the operation is `@idempotent` — sending the
/// same request again answers the same record.
#[tokio::test]
async fn update_flow_entitlement_persists_what_it_returns() {
    let emulator = Emulator::new();

    for (row, case) in update_cases().into_iter().enumerate() {
        let UpdateCase {
            case,
            created,
            body,
            expected,
        } = case;
        let name = created["name"].as_str().unwrap_or_default().to_string();
        let (flow_arn, stored) = flow_with(
            &emulator,
            &format!("update-entitlement-flow-{row}"),
            &[created.clone(), sibling()],
        )
        .await;
        let entitlement_arn = stored[0]["entitlementArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = with_identity(&expected, &name, &entitlement_arn);

        let (status, code, response) =
            update(&emulator, &flow_arn, &entitlement_arn, body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: updating expected 202, got {status} {code:?}: {response}"
        );
        assert_eq!(
            response["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the entitlement belongs to, got {response}"
        );
        assert_eq!(
            response["entitlement"], expected,
            "case {case:?}: expected entitlement {expected}, got {response}"
        );

        // The STORE holds what the response reported, and only the addressed entitlement
        // moved: its sibling is byte for byte what the create response gave.
        let described = described_entitlements(&emulator, &flow_arn).await;
        assert_eq!(
            described.len(),
            2,
            "case {case:?}: the flow still carries both entitlements, got {described:?}"
        );
        assert_eq!(
            described[0], expected,
            "case {case:?}: DescribeFlow must report the updated record, got {:?}",
            described[0]
        );
        assert_eq!(
            described[1], stored[1],
            "case {case:?}: the sibling entitlement must be untouched, got {:?}",
            described[1]
        );

        // `@idempotent`: the same request applied to the record it already produced
        // answers that same record.
        let (status, code, again) = update(&emulator, &flow_arn, &entitlement_arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: re-applying the update expected 202, got {status} {code:?}: {again}"
        );
        assert_eq!(
            again["entitlement"], expected,
            "case {case:?}: re-applying the update must answer the same record, got {again}"
        );
    }
}

/// One refused update: the labels to address and the body to send, with the status and
/// error code this service answers with.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    entitlement_arn: String,
    body: Value,
    status: u16,
    code: &'static str,
}

/// UpdateFlowEntitlement refuses a pair of labels that resolves no stored entitlement with
/// `NotFoundException` (404) — the flow, the entitlement, or an entitlement of ANOTHER
/// flow — and a request it cannot express with `BadRequestException` (400), never with a
/// 501, never with a success, and never storing anything.
#[tokio::test]
async fn update_flow_entitlement_refusals_name_the_modelled_error() {
    let emulator = Emulator::new();
    let (flow_arn, stored) = flow_with(
        &emulator,
        "refusal-entitlement-flow",
        &[
            json!({ "name": "target", "subscribers": [SUBSCRIBER_A] }),
            sibling(),
        ],
    )
    .await;
    let entitlement_arn = stored[0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second flow with its own entitlement: its ARN names a real entitlement, just not
    // one of the flow addressed below.
    let (other_flow_arn, other_stored) = flow_with(
        &emulator,
        "other-entitlement-flow",
        &[json!({ "name": "elsewhere", "subscribers": [SUBSCRIBER_B] })],
    )
    .await;
    let other_entitlement_arn = other_stored[0]["entitlementArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let cases = vec![
        RefusalCase {
            case: "a flow ARN that is well formed but names no flow",
            flow_arn: "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:missing"
                .to_string(),
            entitlement_arn: entitlement_arn.clone(),
            body: json!({ "entitlementStatus": "DISABLED" }),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "a flow label that is not an ARN at all",
            flow_arn: "not-an-arn".to_string(),
            entitlement_arn: entitlement_arn.clone(),
            body: json!({ "entitlementStatus": "DISABLED" }),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "an entitlement ARN that is well formed but names no entitlement",
            flow_arn: flow_arn.clone(),
            entitlement_arn:
                "arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-deadbeef:missing"
                    .to_string(),
            body: json!({ "entitlementStatus": "DISABLED" }),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "an entitlement of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            entitlement_arn: other_entitlement_arn.clone(),
            body: json!({ "entitlementStatus": "DISABLED" }),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "an entitlement label outside the member's own ARN pattern",
            flow_arn: flow_arn.clone(),
            entitlement_arn: "not-an-entitlement-arn".to_string(),
            body: json!({ "entitlementStatus": "DISABLED" }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a status outside the EntitlementStatus value set",
            flow_arn: flow_arn.clone(),
            entitlement_arn: entitlement_arn.clone(),
            body: json!({ "entitlementStatus": "PAUSED" }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an encryption with no RoleArn on an entitlement that holds none",
            flow_arn: flow_arn.clone(),
            entitlement_arn: entitlement_arn.clone(),
            body: json!({ "encryption": { "algorithm": "aes128", "keyType": "static-key" } }),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        flow_arn: addressed_flow,
        entitlement_arn: addressed_entitlement,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, response) =
            update(&emulator, &addressed_flow, &addressed_entitlement, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: {response}"
        );
    }

    // Not one refusal wrote anything: both flows still report exactly the entitlements
    // their create requests stored.
    assert_eq!(
        described_entitlements(&emulator, &flow_arn).await,
        stored,
        "a refused update must store nothing on the flow it addressed"
    );
    assert_eq!(
        described_entitlements(&emulator, &other_flow_arn).await,
        other_stored,
        "a refused update must store nothing on any other flow either"
    );
}
