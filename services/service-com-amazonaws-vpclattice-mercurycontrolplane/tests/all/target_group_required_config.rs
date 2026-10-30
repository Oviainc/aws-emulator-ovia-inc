// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the config members a target-group TYPE cannot be created
//! without (`CreateTargetGroup` + `GetTargetGroup`).
//!
//! The rule under test: a target group of a type that routes to targets inside a VPC
//! cannot be created unless its config supplies every member the type demands — each
//! either sent or filled in from the default the member's own documentation states —
//! and a create that leaves any of them null is refused with HTTP 400
//! `ValidationException` over the `config` member, naming the members that type demands
//! and the type; a type that demands none of them is created without a config at all.
//!
//! Every row is generated from the service's own catalog
//! (`state::REQUIRED_CONFIG_MEMBERS`), so no row can drift from the rule the handler
//! applies, and the rows differ along the type, which demanded members the config
//! sends, whether the config is sent at all, and the protocol (one whose port has a
//! documented default and one whose port has none). The recorded case — type `IP` with
//! an incomplete config — is ONE row of that table.
//!
//! The harness (`Lattice`, `Answer`, `message_of`, …) is the one the service-slice suite
//! already builds, and the request helpers are the target-group suite's.

use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

use crate::target_groups::{create_target_group, get_target_group, member_value, A_VPC};
use crate::vpclattice_service_slice::{message_of, Answer, Lattice};

/// The config members `group_type` cannot be created without, read from the service's
/// own catalog so this suite and the handler share one source.
pub(crate) fn required_members(group_type: &str) -> &'static [&'static str] {
    assert!(
        state::REQUIRED_CONFIG_MEMBERS
            .iter()
            .any(|(name, _)| *name == group_type),
        "the type catalog has no required-member row for {group_type:?}"
    );
    state::required_config_members(group_type)
}

/// The `TargetGroupType` values the model enumerates, read from the service's constants.
const ALL_TYPES: [&str; 4] = [
    state::TYPE_IP,
    state::TYPE_LAMBDA,
    state::TYPE_INSTANCE,
    state::TYPE_ALB,
];

/// The protocols a row is tried under: `HTTP`, for which the `port` member documents a
/// default ("For HTTP, the default is 80"), and `TCP`, for which it documents none — so
/// an omitted port is supplied in the first and null in the second.
///
/// SOURCE: `smithy_query(query="shape",shape="TargetGroupProtocol")` for the value set
/// (`HTTP | HTTPS | TCP`) and `TargetGroupConfig$port` for the defaults.
const PROTOCOLS: [&str; 2] = ["HTTP", "TCP"];

/// The value this suite sends for `member` when a row sends it: the target-group
/// suite's own per-member table, with `protocol` taken from the row, since which port
/// default applies depends on it.
fn value_for(member: &str, protocol: &str) -> serde_json::Value {
    match member {
        "protocol" => serde_json::json!(protocol),
        other => member_value(other),
    }
}

/// Assert the refusal of `answer`: the modelled 400 `ValidationException`, its wire
/// error code, its `reason`, and the sentence over `config` naming the members the type
/// demands and the type itself.
fn assert_refused(case: &str, answer: &Answer, group_type: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected the wire error code ValidationException, got {:?}",
        answer.error_code
    );
    assert_eq!(
        answer.body["reason"],
        serde_json::json!(state::VALIDATION_REASON_FIELD),
        "case {case:?}: expected the modelled field-validation reason, got {}",
        answer.body
    );
    let expected = state::required_config_members_message(group_type);
    let message = message_of(case, answer);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    assert_eq!(
        answer.body["fieldList"],
        serde_json::json!([{ "name": "config", "message": expected }]),
        "case {case:?}: expected one fieldList entry naming config, got {}",
        answer.body
    );
}

/// The rule over every subset of the demanded members, for every type, under a protocol
/// whose port defaults and one whose port does not.
#[tokio::test]
async fn a_create_is_refused_unless_its_config_supplies_every_member_its_type_demands() {
    let lattice = Lattice::new();
    let mut names = (0..).map(|n: u32| format!("required-case-{n}"));

    for group_type in ALL_TYPES {
        let required = required_members(group_type);
        for protocol in PROTOCOLS {
            // Every subset of the demanded members, plus the row that sends no `config`
            // member at all (`None`).
            let subsets = (0..(1_u32 << required.len())).map(Some).chain([None]);
            for subset in subsets {
                let sent: Vec<&str> = match subset {
                    Some(mask) => required
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| mask & (1 << index) != 0)
                        .map(|(_, member)| *member)
                        .collect(),
                    None => Vec::new(),
                };
                // The protocol dimension only says anything about a row that sends one.
                if !sent.contains(&"protocol") && protocol != PROTOCOLS[0] {
                    continue;
                }
                let config = subset.map(|_| {
                    let mut config = serde_json::Map::new();
                    for member in &sent {
                        config.insert((*member).to_string(), value_for(member, protocol));
                    }
                    serde_json::Value::Object(config)
                });
                // A member is supplied when it is sent, or — for `port` — when the sent
                // protocol documents a default for it.
                let port_defaults =
                    sent.contains(&"protocol") && state::default_port_for(protocol).is_some();
                let complete = required
                    .iter()
                    .all(|member| sent.contains(member) || (*member == "port" && port_defaults));

                let name = names.next().expect("the name generator is unbounded");
                let case = format!(
                    "{group_type} with config {} over {protocol}",
                    config
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "absent".to_string())
                );
                let mut body = serde_json::Map::new();
                body.insert("name".to_string(), serde_json::json!(name));
                body.insert("type".to_string(), serde_json::json!(group_type));
                if let Some(config) = config.clone() {
                    body.insert("config".to_string(), config);
                }
                let answer =
                    create_target_group(&lattice, serde_json::Value::Object(body.clone())).await;

                if complete {
                    assert_eq!(
                        answer.status, 201,
                        "case {case:?}: every demanded member is supplied, expected 201, got {} / {}",
                        answer.status, answer.body
                    );
                    // Read back through the implemented reader: what was accepted is
                    // what the store now holds.
                    let id = answer.str_field("id");
                    let read = get_target_group(&lattice, &id).await;
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: expected the created target group to read back, got {} / {}",
                        read.status, read.body
                    );
                    for member in required {
                        assert!(
                            !read.body["config"][member].is_null(),
                            "case {case:?}: expected config.{member} persisted, got {}",
                            read.body["config"]
                        );
                    }
                    continue;
                }

                assert_refused(&case, &answer, group_type);

                // A refused create persists nothing: the account-unique name it asked
                // for is still free, so a complete create can take it.
                let mut retry = body;
                let mut config = serde_json::Map::new();
                for member in required {
                    config.insert((*member).to_string(), value_for(member, PROTOCOLS[0]));
                }
                retry.insert("config".to_string(), serde_json::Value::Object(config));
                let retried = create_target_group(&lattice, serde_json::Value::Object(retry)).await;
                assert_eq!(
                    retried.status, 201,
                    "case {case:?}: the refused create must have taken no name, got {} / {}",
                    retried.status, retried.body
                );
            }
        }
    }
}

/// The refusal's sentence, pinned literally over rows that vary which members are left
/// null and which type demands them. The first row is the recorded one.
#[tokio::test]
async fn the_refusal_names_the_config_members_the_type_demands_and_the_type() {
    let lattice = Lattice::new();

    // (case, request body, the sentence AWS answers with)
    let rows: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "IP with no config at all",
            serde_json::json!({ "name": "ip-no-config", "type": "IP" }),
            "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group",
        ),
        (
            "IP with an empty config",
            serde_json::json!({ "name": "ip-empty-config", "type": "IP", "config": {} }),
            "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group",
        ),
        (
            "IP missing only the VPC",
            serde_json::json!({
                "name": "ip-no-vpc",
                "type": "IP",
                "config": { "protocol": "HTTP", "port": 8080 },
            }),
            "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group",
        ),
        (
            "INSTANCE missing only the VPC",
            serde_json::json!({
                "name": "instance-no-vpc",
                "type": "INSTANCE",
                "config": { "protocol": "HTTP", "port": 8080 },
            }),
            "Config fields port|protocol|vpcIdentifier must not be null for INSTANCE Target Group",
        ),
        (
            "ALB missing the port and the protocol",
            serde_json::json!({
                "name": "alb-no-port",
                "type": "ALB",
                "config": { "vpcIdentifier": A_VPC },
            }),
            "Config fields port|protocol|vpcIdentifier must not be null for ALB Target Group",
        ),
        (
            // `port` documents a default for HTTP and HTTPS only, so under TCP an
            // omitted port is still null.
            "IP over TCP with no port",
            serde_json::json!({
                "name": "ip-tcp-no-port",
                "type": "IP",
                "config": { "protocol": "TCP", "vpcIdentifier": A_VPC },
            }),
            "Config fields port|protocol|vpcIdentifier must not be null for IP Target Group",
        ),
    ];

    for (case, body, expected) in rows {
        let answer = create_target_group(&lattice, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        let message = message_of(case, &answer);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            answer.body["fieldList"],
            serde_json::json!([{ "name": "config", "message": expected }]),
            "case {case:?}: expected one fieldList entry naming config, got {}",
            answer.body
        );
    }

    // The same rule the other way round: a type that demands no config member at all is
    // created without a config, and reads back.
    let created = create_target_group(
        &lattice,
        serde_json::json!({ "name": "lambda-no-config", "type": "LAMBDA" }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"LAMBDA with no config\": expected 201, got {} / {}",
        created.status, created.body
    );
    let read = get_target_group(&lattice, &created.str_field("id")).await;
    assert_eq!(
        read.status, 200,
        "case \"LAMBDA with no config\": expected the created target group to read back, got {} / {}",
        read.status, read.body
    );
}
