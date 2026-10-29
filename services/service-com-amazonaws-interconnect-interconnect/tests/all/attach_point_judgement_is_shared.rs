// SPDX-License-Identifier: CC0-1.0

//! The ONE attach-point judgement both writers of a connection's attach point are held
//! to, driven over the wire through the shared `crate::emulator` harness.
//!
//! RULE: an `attachPoint` naming a Direct Connect gateway is judged against the attach
//! points of the account by one rule, whichever operation writes it — `CreateConnection`
//! allocating the first one or `AcceptConnectionProposal` associating one: a gateway the
//! account holds ready is accepted, a gateway the account's attach points do not include
//! is the modelled `ResourceNotFoundException` (404) naming the identifier and the
//! Environment, and an account whose attach points cannot be read at all is the modelled
//! `InterconnectServerException` (500) saying so without naming the identifier, because
//! then nothing is known about it either way.
//!
//! The rows differ along what the rule generalises over: the operation that writes the
//! attach point, the shape of the identifier, the verdict the resolution reaches, the
//! environment (and so the provider kind), the bandwidth and the Region the request is
//! signed for.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text, Emulator,
};

/// The two Regions the rows sign their requests for, so the rule is checked across the
/// Region the request addresses as well as across the identifier.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The gateways the rows below attach to, so the account holds attach points and what a
/// refusal refuses is the identifier rather than an empty account.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [
    ("ic-shared-judgement-a", None),
    ("ic-shared-judgement-b", Some(64_512)),
];

/// How a request whose attach points could not be read AT ALL is refused — written out
/// here rather than imported, so this suite is a second witness to the wording both
/// writers answer with.
const ATTACH_POINTS_UNRESOLVABLE: &str =
    "the attach points of this account could not be resolved, so the requested attach point \
     could not be validated";

/// `CreateConnection`, as a `(status, body)` pair so a refusal is readable.
async fn create(
    emulator: &Emulator,
    region: &str,
    environment: &Value,
    bandwidth: &str,
    attach_point: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert(
        "environmentId".to_string(),
        json!(text(environment, "environmentId")),
    );
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) = remote_account_for(environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A proposal awaiting acceptance, created on an attach point the account holds — the
/// record whose `activationKey` the acceptance below is made with.
async fn proposal(
    emulator: &Emulator,
    region: &str,
    environment: &Value,
    bandwidth: &str,
    gateway: &str,
) -> Value {
    let (status, body) = create(
        emulator,
        region,
        environment,
        bandwidth,
        &json!({ "directConnectGateway": gateway }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {region:?}: expected the proposal to be created on {gateway:?}, got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// `AcceptConnectionProposal`, as a `(status, body)` pair.
async fn accept(
    emulator: &Emulator,
    region: &str,
    activation_key: &str,
    attach_point: &Value,
) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "AcceptConnectionProposal",
            json!({ "activationKey": activation_key, "attachPoint": attach_point }),
        )
        .await
}

/// The `(environment, bandwidth, region)` rows both writers are driven over: every
/// bandwidth of every environment that takes a connection, alternating the Region.
///
/// Each row carries the Environment ITS OWN Region's endpoint offers for that partner
/// offering, because an Environment belongs to the Region whose connections it
/// produces.
async fn rows(emulator: &Emulator) -> Vec<(Value, String, &'static str)> {
    let catalog = emulator.environments().await;
    let mut out = Vec::new();
    for environment in creatable_environments(&catalog) {
        for bandwidth in available_bandwidths(&environment) {
            let region = REGIONS[out.len() % REGIONS.len()];
            out.push((
                emulator.counterpart_in(region, &environment).await,
                bandwidth,
                region,
            ));
        }
    }
    assert!(
        !out.is_empty(),
        "case \"catalog\": expected at least one environment that takes connections"
    );
    out
}

/// The identifier an attach point names, whichever member carries it.
fn identifier_of(attach_point: &Value) -> String {
    attach_point
        .as_object()
        .and_then(|members| members.values().next())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The verdict a gateway identifier resolves to
// ---------------------------------------------------------------------------

/// RULE: both writers of a connection's attach point reach the SAME verdict on the same
/// gateway identifier and answer it with the same status, the same modelled error and
/// the same wording — a gateway the account holds ready is accepted by both, and one the
/// account's attach points do not include is the modelled 404 naming the identifier and
/// the Environment from both.
#[tokio::test]
async fn both_attach_point_writers_answer_one_gateway_verdict_the_same_way() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let held = gateways.first().expect("the account holds a gateway").clone();

    for (environment, bandwidth, region) in rows(&emulator).await {
        let environment_id = text(&environment, "environmentId");

        // Rows differ in the SHAPE of the identifier that is not an attach point of the
        // account: each UUID flavour the member's pattern allows, and a case variant of
        // a gateway the account does hold.
        let absent: Vec<(&str, Value)> = vec![
            (
                "a v4 gateway id the account does not hold",
                json!({ "directConnectGateway": "00000000-0000-4000-8000-000000000000" }),
            ),
            (
                "a gateway id of all digits",
                json!({ "directConnectGateway": "12345678-1234-1234-1234-123456789012" }),
            ),
            (
                "a gateway id of all hex letters",
                json!({ "directConnectGateway": "ffffffff-ffff-ffff-ffff-ffffffffffff" }),
            ),
            (
                "an upper-cased variant of a gateway the account holds",
                json!({ "directConnectGateway": held.to_ascii_uppercase() }),
            ),
        ];

        for (shape, attach_point) in &absent {
            let case = format!("{environment_id} / {bandwidth} / {region} / {shape}");

            // The create that allocates the first attach point...
            let (created_status, created_body) =
                create(&emulator, region, &environment, &bandwidth, attach_point).await;
            // ...and the acceptance that associates one, on a proposal of the same
            // Environment, so the two refusals are comparable word for word.
            let awaiting =
                proposal(&emulator, region, &environment, &bandwidth, &held).await;
            let (accepted_status, accepted_body) = accept(
                &emulator,
                region,
                &text(&awaiting, "activationKey"),
                attach_point,
            )
            .await;

            for (operation, status, body) in [
                ("CreateConnection", created_status, &created_body),
                ("AcceptConnectionProposal", accepted_status, &accepted_body),
            ] {
                assert_eq!(
                    status, 404,
                    "case {case:?} / {operation}: expected the modelled 404 for an identifier that is not an attach point, got {status} with {body:?}"
                );
                assert!(
                    code(body).contains("ResourceNotFoundException"),
                    "case {case:?} / {operation}: expected ResourceNotFoundException, got {:?}",
                    code(body)
                );
                let worded = message(body);
                assert!(
                    worded.contains(&identifier_of(attach_point)) && worded.contains(&environment_id),
                    "case {case:?} / {operation}: expected the refusal to name the identifier and the environment, got {worded:?}"
                );
            }
            assert_eq!(
                message(&created_body),
                message(&accepted_body),
                "case {case:?}: expected both writers to word the refusal identically, got {:?} vs {:?}",
                message(&created_body),
                message(&accepted_body)
            );
        }

        // Positive control: the gateway the account DOES hold ready is accepted by both
        // writers, so what the rows above refuse is the identifier and not the rule.
        let valid = json!({ "directConnectGateway": held });
        let (status, body) = create(&emulator, region, &environment, &bandwidth, &valid).await;
        assert_eq!(
            status, 200,
            "case {environment_id:?}: expected a gateway of the account to be accepted by the create, got {status} with {body:?}"
        );
        let awaiting = proposal(&emulator, region, &environment, &bandwidth, &held).await;
        let (status, body) = accept(
            &emulator,
            region,
            &text(&awaiting, "activationKey"),
            &valid,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {environment_id:?}: expected a gateway of the account to be accepted by the acceptance, got {status} with {body:?}"
        );
        assert_eq!(
            body["connection"]["attachPoint"], valid,
            "case {environment_id:?}: expected the accepted connection to carry the attach point selected, got {:?}",
            body["connection"]["attachPoint"]
        );
    }
}

// ---------------------------------------------------------------------------
// An account whose attach points cannot be read at all
// ---------------------------------------------------------------------------

/// RULE: when the account's attach points cannot be read at all, both writers answer the
/// modelled `InterconnectServerException` (500) with one wording that names no
/// identifier — a different answer from the 404 above, because nothing is known about the
/// identifier either way — and neither writes anything.
#[tokio::test]
async fn both_attach_point_writers_report_an_unreadable_account_the_same_way() {
    // Two front doors over ONE store: the fully mounted emulator, which owns the
    // account's gateways, and one with Interconnect alone behind it, which can reach no
    // service holding them.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mounted = Emulator::on_engine(Arc::clone(&engine));
    let degraded = Emulator::interconnect_only(engine);

    let gateways = mounted.ready_gateways(&GATEWAY_ROWS).await;
    let held = gateways.first().expect("the account holds a gateway").clone();

    let table = rows(&mounted).await;
    for (environment, bandwidth, region) in table.clone() {
        let environment_id = text(&environment, "environmentId");
        let case = format!("{environment_id} / {bandwidth} / {region}");
        let attach_point = json!({ "directConnectGateway": held });

        // The proposal is made through the mounted door, so what the degraded door
        // cannot judge below is the attach point and not the proposal.
        let awaiting = proposal(&mounted, region, &environment, &bandwidth, &held).await;

        let (created_status, created_body) =
            create(&degraded, region, &environment, &bandwidth, &attach_point).await;
        let (accepted_status, accepted_body) = accept(
            &degraded,
            region,
            &text(&awaiting, "activationKey"),
            &attach_point,
        )
        .await;

        for (operation, status, body) in [
            ("CreateConnection", created_status, &created_body),
            ("AcceptConnectionProposal", accepted_status, &accepted_body),
        ] {
            assert_eq!(
                status, 500,
                "case {case:?} / {operation}: expected the modelled 500 when the account's attach points cannot be read, got {status} with {body:?}"
            );
            assert!(
                code(body).contains("InterconnectServerException"),
                "case {case:?} / {operation}: expected InterconnectServerException, got {:?}",
                code(body)
            );
            assert_eq!(
                message(body),
                ATTACH_POINTS_UNRESOLVABLE,
                "case {case:?} / {operation}: expected the unreadable-account wording, got {:?}",
                message(body)
            );
            assert!(
                !message(body).contains(held.as_str()),
                "case {case:?} / {operation}: expected the refusal not to judge the identifier it could not read, got {:?}",
                message(body)
            );
        }

        // Neither refusal wrote anything: the proposal still carries the attach point it
        // was created on, and no connection was allocated for the refused create.
        let read_back = mounted
            .ic_ok_in(
                region,
                "DescribeConnectionProposal",
                json!({ "activationKey": text(&awaiting, "activationKey") }),
            )
            .await;
        assert_eq!(
            text(&read_back, "environmentId"),
            environment_id,
            "case {case:?}: expected the proposal to survive the refusals, got {read_back:?}"
        );
        let stored = mounted
            .ic_ok_in(region, "GetConnection", json!({ "identifier": text(&awaiting, "id") }))
            .await;
        assert_eq!(
            stored["connection"]["attachPoint"], awaiting["attachPoint"],
            "case {case:?}: expected the refused acceptance to leave the proposal's attach point alone, got {:?}",
            stored["connection"]["attachPoint"]
        );
    }

    // Nothing the degraded door refused is in the store: every connection it holds is
    // one the mounted door created above.
    let mut counted = BTreeMap::new();
    for region in REGIONS {
        let listed = mounted
            .ic_ok_in(region, "ListConnections", json!({ "maxResults": 100 }))
            .await;
        counted.insert(
            region,
            listed["connections"].as_array().map(Vec::len).unwrap_or(0),
        );
    }
    let total: usize = counted.values().sum();
    assert_eq!(
        total,
        table.len(),
        "case \"store\": expected only the proposals the mounted door created, got {counted:?}"
    );
}
