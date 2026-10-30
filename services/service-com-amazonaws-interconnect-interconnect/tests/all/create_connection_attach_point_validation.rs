// SPDX-License-Identifier: CC0-1.0

//! What `CreateConnection` accepts as an `attachPoint`, driven over the wire through
//! the shared `crate::emulator` harness.
//!
//! RULE: `CreateConnection$attachPoint` has to identify an attach point that EXISTS and
//! is valid for the named Environment — the set `ListAttachPoints` reports for it for
//! the `directConnectGateway` member, a Connection of the account for the `arn` one —
//! and one that resolves to nothing is refused with the operation's modelled 404,
//! having provisioned nothing.
//!
//! The rows differ along what the rule generalises over (the attach-point member, the
//! shape of the identifier, the Environment and the Region) rather than along one
//! recorded request.

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text,
    Emulator,
};

/// The two Regions the rows below sign their requests for, so a rule is checked across
/// the Region an ARN carries as well as across the resource it names.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// Long enough to cover a connection's tear-down span (the handler's own 400 ms) with
/// margin, so a record a row deleted has reached its terminal state.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_millis(700);

/// `CreateConnection` with every member a row may vary, as a `(status, body)` pair so a
/// refusal is readable.
async fn create(
    emulator: &Emulator,
    region: &str,
    environment_id: &str,
    bandwidth: &str,
    attach_point: &Value,
    remote_account: Option<Value>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) = remote_account {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// Create on `environment`, taking the `remoteAccount` that Environment asks for from
/// its own record, so a row about the attach point is not also a row about the
/// identifier.
async fn create_on(
    emulator: &Emulator,
    region: &str,
    environment: &Value,
    bandwidth: &str,
    attach_point: &Value,
) -> (u16, Value) {
    create(
        emulator,
        region,
        &text(environment, "environmentId"),
        bandwidth,
        attach_point,
        remote_account_for(environment),
    )
    .await
}

/// Every connection the store holds, walked page by page so the count is the whole
/// store however it is windowed.
async fn stored_connections(emulator: &Emulator) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok("ListConnections", Value::Object(request))
            .await;
        all.extend(
            listed["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// The `identifier`s `ListAttachPoints` reports for `environment_id` in `region`.
async fn listed_identifiers(emulator: &Emulator, region: &str, environment_id: &str) -> Vec<String> {
    emulator
        .attach_points_in(region, environment_id)
        .await
        .iter()
        .map(|descriptor| text(descriptor, "identifier"))
        .collect()
}

// ---------------------------------------------------------------------------
// An attach point the Environment does not offer
// ---------------------------------------------------------------------------

/// RULE: an `attachPoint` that resolves to no attach point valid for the named
/// Environment is refused with the operation's modelled `ResourceNotFoundException`
/// (404), worded from the request, and nothing is provisioned for it.
#[tokio::test]
async fn an_attach_point_that_resolves_to_nothing_is_refused_and_provisions_nothing() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    // One gateway of the account, so what is refused below is the identifier and not an
    // account that holds no attach point at all.
    let owned = emulator
        .ready_gateways(&[("ic-validation-owned", None)])
        .await
        .remove(0);

    let environments = creatable_environments(&catalog);
    assert!(
        !environments.is_empty(),
        "case \"catalog\": expected an environment that takes connections, got {}",
        environments.len()
    );

    // Rows differ in the attach-point MEMBER and in the shape of the identifier that
    // resolves to nothing: a gateway id of each UUID flavour the member's pattern
    // allows, a case variant of a gateway the account does hold, and connection ARNs
    // that name no connection — one in the Region the request addresses, one in another
    // Region, one in another account. They also differ in the Region the create
    // addresses, since the catalog publishes one Environment per regional endpoint.
    let account = engine::EMULATOR_ACCOUNT_ID;
    for (index, region) in REGIONS.iter().enumerate() {
        let region = *region;
        // The Environment this row's Region offers for that partner offering: an
        // Environment belongs to the Region whose connections it produces.
        let environment = &emulator
            .counterpart_in(region, &environments[index % environments.len()])
            .await;
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[index % bandwidths.len()].clone();
        let listed = listed_identifiers(&emulator, region, &environment_id).await;
        assert!(
            listed.contains(&owned),
            "case {environment_id:?}: expected the account's gateway to be a listed attach point, got {listed:?}"
        );

        let unresolvable: Vec<(String, Value)> = vec![
            (
                "a v4 gateway id the account does not hold".to_string(),
                json!({ "directConnectGateway": "00000000-0000-4000-8000-000000000000" }),
            ),
            (
                "a gateway id of all digits".to_string(),
                json!({ "directConnectGateway": "12345678-1234-1234-1234-123456789012" }),
            ),
            (
                "a gateway id of all hex letters".to_string(),
                json!({ "directConnectGateway": "ffffffff-ffff-ffff-ffff-ffffffffffff" }),
            ),
            (
                "an upper-cased variant of a gateway the account holds".to_string(),
                json!({ "directConnectGateway": owned.to_ascii_uppercase() }),
            ),
            (
                "an ARN naming no connection of this Region".to_string(),
                json!({ "arn": format!("arn:aws:interconnect:{region}:{account}:connection/mcc-00000000") }),
            ),
            (
                "an ARN naming no connection of another Region".to_string(),
                json!({ "arn": format!("arn:aws:interconnect:ap-south-1:{account}:connection/lmcc-zzzzzzzz") }),
            ),
            (
                "an ARN naming a connection of another account".to_string(),
                json!({ "arn": format!("arn:aws:interconnect:{region}:123456789012:connection/mcc-0a1b2c3d") }),
            ),
        ];

        let before = stored_connections(&emulator).await;
        for (shape, attach_point) in &unresolvable {
            let case = format!("{environment_id} / {bandwidth} / {shape}");
            let (status, body) =
                create_on(&emulator, region, environment, &bandwidth, attach_point).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the modelled 404 for an attach point that resolves to nothing, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                code(&body)
            );
            let identifier = attach_point
                .as_object()
                .and_then(|members| members.values().next())
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                message(&body).contains(&identifier) && message(&body).contains(&environment_id),
                "case {case:?}: expected the refusal to name the attach point and the environment, got {:?}",
                message(&body)
            );
        }

        // Not one of those refusals provisioned anything.
        let after = stored_connections(&emulator).await;
        assert_eq!(
            after, before,
            "case {environment_id:?}: expected the refused creates to have provisioned nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An attach point the Environment does offer
// ---------------------------------------------------------------------------

/// RULE: every attach point `ListAttachPoints` reports for an Environment is one a
/// create on that Environment accepts, and the stored Connection reports it back.
#[tokio::test]
async fn every_listed_attach_point_is_accepted_and_read_back() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    // Rows differ in how many gateways the account holds and in each gateway's name
    // and `amazonSideAsn`.
    let gateways = emulator
        .ready_gateways(&[
            ("ic-accepted-a", None),
            ("ic accepted spaced", Some(64_512)),
            ("ic-accepted-c", Some(4_294_967_294)),
        ])
        .await;
    assert_eq!(
        gateways.len(),
        3,
        "setup: expected three gateways, got {gateways:?}"
    );

    let mut rows = 0usize;
    for (index, environment) in creatable_environments(&catalog).iter().enumerate() {
        let region = REGIONS[index % REGIONS.len()];
        let environment = &emulator.counterpart_in(region, environment).await;
        let environment_id = text(environment, "environmentId");
        let listed = listed_identifiers(&emulator, region, &environment_id).await;
        for bandwidth in available_bandwidths(environment) {
            for identifier in &listed {
                let case = format!("{environment_id} / {bandwidth} / {identifier} / {region}");
                let attach_point = json!({ "directConnectGateway": identifier });
                let (status, body) =
                    create_on(&emulator, region, environment, &bandwidth, &attach_point).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a listed attach point to be accepted, got {status} with {body:?}"
                );
                assert_eq!(
                    body["connection"]["attachPoint"], attach_point,
                    "case {case:?}: expected the connection to carry the attach point it was created on, got {:?}",
                    body["connection"]["attachPoint"]
                );

                // Read back through the implemented reader, in the Region the create
                // addressed: a connection belongs to the Region it was created in.
                let identifier_of = text(&body["connection"], "id");
                let (status, fetched) = emulator
                    .ic_in(
                        region,
                        "GetConnection",
                        json!({ "identifier": identifier_of }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the connection to be readable, got {status} with {fetched:?}"
                );
                assert_eq!(
                    fetched["connection"]["attachPoint"], attach_point,
                    "case {case:?}: expected GetConnection to report the same attach point, got {:?}",
                    fetched["connection"]["attachPoint"]
                );
                rows += 1;
            }
        }
    }
    assert!(
        rows >= 6,
        "case \"coverage\": expected the table to span several environments and gateways, got {rows} rows"
    );
}

/// RULE: the `arn` member names an attach point by full ARN, so a Connection the
/// account HOLDS is one — while the same ARN, once that connection is gone, no longer
/// resolves to anything.
#[tokio::test]
async fn a_connection_of_the_account_is_an_attach_point_by_arn() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator
        .ready_gateways(&[("ic-arn-attach-point", None)])
        .await
        .remove(0);
    let environments = creatable_environments(&catalog);

    // Rows differ in the Region the ARN is minted in and addressed from, in the
    // environment (and so in the `ConnectionId` prefix the ARN carries) and in the
    // bandwidth.
    for (index, region) in REGIONS.iter().enumerate() {
        let environment = &emulator
            .counterpart_in(region, &environments[index % environments.len()])
            .await;
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[index % bandwidths.len()].clone();
        let case = format!("{environment_id} / {bandwidth} / {region}");

        let (status, base) = create_on(
            &emulator,
            region,
            environment,
            &bandwidth,
            &json!({ "directConnectGateway": gateway }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the connection the ARN will name to be created, got {status} with {base:?}"
        );
        let base_arn = text(&base["connection"], "arn");
        let attach_point = json!({ "arn": base_arn });

        let (status, attached) =
            create_on(&emulator, region, environment, &bandwidth, &attach_point).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an ARN naming a connection of the account to be accepted, got {status} with {attached:?}"
        );
        let attached_id = text(&attached["connection"], "id");
        let (status, fetched) = emulator
            .ic_in(region, "GetConnection", json!({ "identifier": attached_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the connection to be readable, got {status} with {fetched:?}"
        );
        assert_eq!(
            fetched["connection"]["attachPoint"], attach_point,
            "case {case:?}: expected GetConnection to report the ARN attach point, got {:?}",
            fetched["connection"]["attachPoint"]
        );

        // The SAME ARN, once the connection it names is gone, resolves to nothing.
        let base_id = text(&base["connection"], "id");
        emulator
            .ic_ok_in(region, "DeleteConnection", json!({ "identifier": base_id }))
            .await;
        tokio::time::sleep(SETTLE_WAIT).await;
        let (status, body) =
            create_on(&emulator, region, environment, &bandwidth, &attach_point).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the ARN of a gone connection to resolve to nothing, got {status} with {body:?}"
        );
    }
}

// What identifying information a create needs is its own rule, and its own suite:
// `tests/all/create_connection_remote_account.rs`.
