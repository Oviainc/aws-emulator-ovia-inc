// SPDX-License-Identifier: CC0-1.0

//! What `CreateConnection` accepts as an `attachPoint`, and what identifying
//! information it asks the caller for, driven over the wire through the shared
//! `crate::emulator` harness.
//!
//! Two RULES are under test, each over a TABLE whose rows differ along what the rule
//! generalises over rather than along one recorded request:
//!
//! * `CreateConnection$attachPoint` has to identify an attach point that EXISTS and is
//!   valid for the named Environment — the set `ListAttachPoints` reports for it for
//!   the `directConnectGateway` member, a Connection of the account for the `arn` one —
//!   and one that resolves to nothing is refused with the operation's modelled 404,
//!   having provisioned nothing.
//! * An Environment that declares a `remoteIdentifierType` asks every create on it for
//!   a `remoteAccount` carrying identifying information of THAT kind; one that declares
//!   none asks for nothing.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text,
    Emulator, PARTNER_EMAIL,
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

/// The `identifier`s `ListAttachPoints` reports for `environment_id`.
async fn listed_identifiers(emulator: &Emulator, environment_id: &str) -> Vec<String> {
    emulator
        .attach_points(environment_id)
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
        environments.len() >= 2,
        "case \"catalog\": expected at least two environments that take connections, got {}",
        environments.len()
    );

    // Rows differ in the attach-point MEMBER and in the shape of the identifier that
    // resolves to nothing: a gateway id of each UUID flavour the member's pattern
    // allows, a case variant of a gateway the account does hold, and connection ARNs
    // that name no connection — one in the Region the request addresses, one in another
    // Region, one in another account.
    let account = engine::EMULATOR_ACCOUNT_ID;
    for (index, environment) in environments.iter().enumerate() {
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[index % bandwidths.len()].clone();
        let region = REGIONS[index % REGIONS.len()];
        let listed = listed_identifiers(&emulator, &environment_id).await;
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
        let environment_id = text(environment, "environmentId");
        let region = REGIONS[index % REGIONS.len()];
        let listed = listed_identifiers(&emulator, &environment_id).await;
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
        let environment = &environments[index % environments.len()];
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

// ---------------------------------------------------------------------------
// The identifying information an Environment asks for
// ---------------------------------------------------------------------------

/// The `remoteAccount` values that are NOT of the kind `declared` names.
///
/// Rows differ in how the value misses: the other kind's shape, and — for the account
/// number, which is twelve digits — one digit short and one digit past that width; for
/// the email kind, each part of the `@`-separated shape in turn.
fn wrong_kind_identifiers(declared: &str) -> Vec<(String, String)> {
    let rows: Vec<(&str, String)> = match declared {
        "account" => vec![
            ("an email address", PARTNER_EMAIL.to_string()),
            ("a word", "not-an-account".to_string()),
            ("one digit short of an account number", "1".repeat(11)),
            ("one digit past an account number", "1".repeat(13)),
        ],
        "email" => vec![
            ("an account number", engine::EMULATOR_ACCOUNT_ID.to_string()),
            ("no at sign at all", "partner.example.test".to_string()),
            ("an empty local part", "@example.test".to_string()),
            ("an empty domain", "partner@".to_string()),
            ("two at signs", "partner@example@test".to_string()),
        ],
        other => panic!("case {other:?}: no RemoteAccountIdentifierType is modelled"),
    };
    rows.into_iter()
        .map(|(shape, value)| (shape.to_string(), value))
        .collect()
}

/// The `remoteAccount` values that ARE of the kind `declared` names, beyond the one the
/// harness picks.
///
/// The `email` rows span what the kind means rather than one address shape: a dotted
/// domain, a sub-domain, and a domain with no dot at all — the service states no email
/// grammar, so a plain host name is identifying information of that kind too.
fn right_kind_identifiers(declared: &str) -> Vec<String> {
    match declared {
        "account" => vec![
            engine::EMULATOR_ACCOUNT_ID.to_string(),
            "123456789012".to_string(),
        ],
        "email" => vec![
            "a@b.test".to_string(),
            "first.last@sub.example.test".to_string(),
            "partner@localhost".to_string(),
        ],
        other => panic!("case {other:?}: no RemoteAccountIdentifierType is modelled"),
    }
}

/// RULE: an Environment that declares a `remoteIdentifierType` asks every create on it
/// for a `remoteAccount` carrying identifying information of that kind — a create that
/// omits it, or carries another kind, is refused with the modelled
/// `InterconnectValidationException` (400) and provisions nothing; an Environment that
/// declares no type asks for nothing.
#[tokio::test]
async fn an_environment_asks_a_create_for_the_identifying_information_it_declares() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator
        .ready_gateways(&[("ic-remote-account", None)])
        .await
        .remove(0);
    let attach_point = json!({ "directConnectGateway": gateway });

    // Rows span the catalog: environments declaring each modelled type, and one
    // declaring none.
    let mut declared_types: BTreeMap<String, usize> = BTreeMap::new();
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let bandwidth = available_bandwidths(&environment)[0].clone();
        let declared = environment["remoteIdentifierType"].as_str().map(str::to_string);
        *declared_types
            .entry(declared.clone().unwrap_or_else(|| "none".to_string()))
            .or_default() += 1;

        let before = stored_connections(&emulator).await.len();

        match &declared {
            // An Environment that declares a type asks for it: absent is refused, the
            // wrong kind is refused, and every value of the right kind is accepted.
            Some(declared) => {
                let case = format!("{environment_id} / {declared} / absent");
                let (status, body) = create(
                    &emulator,
                    REGIONS[0],
                    &environment_id,
                    &bandwidth,
                    &attach_point,
                    None,
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a create with no remoteAccount to be refused, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains("InterconnectValidationException"),
                    "case {case:?}: expected InterconnectValidationException, got {:?}",
                    code(&body)
                );
                assert!(
                    message(&body).contains(&environment_id) && message(&body).contains(declared),
                    "case {case:?}: expected the refusal to name the environment and the kind it asks for, got {:?}",
                    message(&body)
                );

                for (shape, value) in wrong_kind_identifiers(declared) {
                    let case = format!("{environment_id} / {declared} / {shape}");
                    let (status, body) = create(
                        &emulator,
                        REGIONS[0],
                        &environment_id,
                        &bandwidth,
                        &attach_point,
                        Some(json!({ "identifier": value })),
                    )
                    .await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected {value:?} to be refused as not the declared kind, got {status} with {body:?}"
                    );
                    assert!(
                        code(&body).contains("InterconnectValidationException"),
                        "case {case:?}: expected InterconnectValidationException, got {:?}",
                        code(&body)
                    );
                }

                for value in right_kind_identifiers(declared) {
                    let case = format!("{environment_id} / {declared} / {value}");
                    let (status, body) = create(
                        &emulator,
                        REGIONS[0],
                        &environment_id,
                        &bandwidth,
                        &attach_point,
                        Some(json!({ "identifier": value })),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected identifying information of the declared kind to be accepted, got {status} with {body:?}"
                    );
                }

                let after = stored_connections(&emulator).await.len();
                assert_eq!(
                    after,
                    before + right_kind_identifiers(declared).len(),
                    "case {environment_id:?}: expected only the accepted creates to have provisioned a connection, got {after} for {before} before"
                );
            }
            // An Environment that declares no type asks for nothing: the create is
            // accepted with the member absent, and with any identifier supplied.
            None => {
                for (shape, remote_account) in [
                    ("absent".to_string(), None),
                    (
                        "an account number".to_string(),
                        Some(json!({ "identifier": engine::EMULATOR_ACCOUNT_ID })),
                    ),
                    (
                        "an email address".to_string(),
                        Some(json!({ "identifier": PARTNER_EMAIL })),
                    ),
                ] {
                    let case = format!("{environment_id} / declares nothing / {shape}");
                    let (status, body) = create(
                        &emulator,
                        REGIONS[0],
                        &environment_id,
                        &bandwidth,
                        &attach_point,
                        remote_account,
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected an environment that declares no type to ask for nothing, got {status} with {body:?}"
                    );
                }
            }
        }
    }

    // The table really did span both modelled types and the absent one, so neither
    // branch above went unchecked.
    for expected in ["account", "email", "none"] {
        assert!(
            declared_types.contains_key(expected),
            "case \"coverage\": expected the catalog to offer an environment declaring {expected:?}, got {declared_types:?}"
        );
    }
}
