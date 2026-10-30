// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the target-group slice (`CreateTargetGroup` + `GetTargetGroup`).
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router that mounts it — and reads the answer back out of a LATER
//! request, so nothing here can pass on a canned body: an id that is never stored
//! fails the read-back, and a response echoed from the request fails the ARN,
//! defaulting and status assertions.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, `expected_arn`, …) is the one the
//! service-slice suite already builds; nothing is duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. `CreateTargetGroup` mints a target group for the account: an id of
//!    `^tg-[0-9a-z]{17}$` and an ARN built from the request's region and the emulator's
//!    account; the record is persisted, and `GetTargetGroup` by the bare id and by the
//!    ARN both report exactly what was stored.
//! 2. A config is persisted with every value its MEMBERS document a default for filled
//!    in, for the members the target group's TYPE supports — and only those.
//! 3. A config member is supported only for the target-group types its own
//!    documentation names; sent for any other type it is a 400 `ValidationException`.
//! 4. A health-check member outside the range its own documentation states is a 400
//!    `ValidationException`; each published bound itself is accepted.
//! 5. A target-group name is unique within the ACCOUNT: a second create under a taken
//!    name is a 409 `ConflictException`, and a `clientToken` retry answers with the
//!    first target group instead of a second one.
//! 6. An identifier naming no persisted target group is a 404
//!    `ResourceNotFoundException`, and a name the modelled pattern forbids is a 400
//!    `ValidationException`.
//! 7. A create answers with the non-terminal `CREATE_IN_PROGRESS`, and the target group
//!    reaches `ACTIVE` once the span the clock owes it has passed.
//! 8. `serviceArns` is derived from the stored listener and rule records: a service
//!    whose forward action names the target group is reported, one that does not is not.

use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

use crate::listeners::{a_service, create_listener};
use crate::rules::{a_listener, create_rule};
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_lattice_id, is_timestamp, message_of, Answer, Lattice,
};

/// The ARN resource kind of a target group (`TargetGroupArn`'s modelled pattern).
const TARGET_GROUP_KIND: &str = "targetgroup";

/// A VPC id of the shape `VpcId` demands (`^vpc-(([0-9a-z]{8})|([0-9a-z]{17}))$`). Which
/// VPC it is is EC2's concern, not this slice's: the member is stored as sent.
pub(crate) const A_VPC: &str = "vpc-0a1b2c3d4e5f60718";

/// `POST /targetgroups`.
pub(crate) async fn create_target_group(lattice: &Lattice, body: serde_json::Value) -> Answer {
    lattice.call("POST", "/targetgroups", Some(body)).await
}

/// `GET /targetgroups/{targetGroupIdentifier}`.
pub(crate) async fn get_target_group(lattice: &Lattice, identifier: &str) -> Answer {
    lattice
        .call("GET", &format!("/targetgroups/{identifier}"), None)
        .await
}

/// Create a target group that must succeed, answering its id and ARN.
async fn a_target_group(lattice: &Lattice, body: serde_json::Value) -> (String, String) {
    let created = create_target_group(lattice, body.clone()).await;
    assert_eq!(
        created.status, 201,
        "case fixture target group {body}: expected 201, got {} / {}",
        created.status, created.body
    );
    (created.str_field("id"), created.str_field("arn"))
}

/// Rules 1 and 2, over a table that varies the type, which config members are sent,
/// the name's shape and the tags. The recorded LAMBDA case is one row of it.
#[tokio::test]
async fn creating_target_groups_persists_them_and_both_identifier_forms_read_them_back() {
    let lattice = Lattice::new();

    // (case, request body, the config the service must PERSIST)
    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "LAMBDA with no config at all",
            serde_json::json!({ "name": "lambda-plain", "type": "LAMBDA" }),
            // `lambdaEventStructureVersion` documents "The default is V1"; every other
            // member of the config is unsupported for a LAMBDA target group.
            serde_json::json!({ "lambdaEventStructureVersion": "V1" }),
        ),
        (
            "LAMBDA carrying the non-default event structure version",
            serde_json::json!({
                "name": "lambda-v2",
                "type": "LAMBDA",
                "config": { "lambdaEventStructureVersion": "V2" },
                "tags": { "team": "payments" },
            }),
            serde_json::json!({ "lambdaEventStructureVersion": "V2" }),
        ),
        (
            "IP over HTTP with no port",
            serde_json::json!({
                "name": "ip-http",
                "type": "IP",
                "config": { "protocol": "HTTP", "vpcIdentifier": A_VPC },
            }),
            // `port` documents "For HTTP, the default is 80", `protocolVersion` "The
            // default is HTTP1" and `ipAddressType` "The default is IPV4".
            serde_json::json!({
                "protocol": "HTTP",
                "port": 80,
                "protocolVersion": "HTTP1",
                "ipAddressType": "IPV4",
                "vpcIdentifier": A_VPC,
            }),
        ),
        (
            "IP over HTTPS with no port, explicit IPV6 and GRPC",
            serde_json::json!({
                "name": "ip-https-v6",
                "type": "IP",
                "config": {
                    "protocol": "HTTPS",
                    "protocolVersion": "GRPC",
                    "ipAddressType": "IPV6",
                    "vpcIdentifier": A_VPC,
                },
            }),
            // "For HTTPS, the default is 443"; a value the caller sent is kept.
            serde_json::json!({
                "protocol": "HTTPS",
                "port": 443,
                "protocolVersion": "GRPC",
                "ipAddressType": "IPV6",
                "vpcIdentifier": A_VPC,
            }),
        ),
        (
            "INSTANCE with an explicit port and a half-filled health check",
            serde_json::json!({
                "name": "instance-hc",
                "type": "INSTANCE",
                "config": {
                    "protocol": "HTTPS",
                    "port": 8443,
                    "vpcIdentifier": A_VPC,
                    "healthCheck": {
                        "enabled": true,
                        "healthCheckIntervalSeconds": 60,
                        "matcher": { "httpCode": "200-299" },
                    },
                },
            }),
            // The health check keeps what the caller sent and takes the defaults its
            // members document for the rest ("The default path is /", protocol HTTP,
            // timeout 5 s, healthy 5, unhealthy 2); no `ipAddressType` is filled in,
            // because that member is supported only for an IP target group.
            serde_json::json!({
                "protocol": "HTTPS",
                "port": 8443,
                "protocolVersion": "HTTP1",
                "vpcIdentifier": A_VPC,
                "healthCheck": {
                    "enabled": true,
                    "protocol": "HTTP",
                    "path": "/",
                    "healthCheckIntervalSeconds": 60,
                    "healthCheckTimeoutSeconds": 5,
                    "healthyThresholdCount": 5,
                    "unhealthyThresholdCount": 2,
                    "matcher": { "httpCode": "200-299" },
                },
            }),
        ),
        (
            // The shortest name `TargetGroupName`'s `@length(3, 128)` admits.
            "ALB under the shortest legal name",
            serde_json::json!({
                "name": "alb",
                "type": "ALB",
                "config": { "protocol": "HTTP", "port": 80, "vpcIdentifier": A_VPC },
            }),
            serde_json::json!({
                "protocol": "HTTP",
                "port": 80,
                "protocolVersion": "HTTP1",
                "vpcIdentifier": A_VPC,
            }),
        ),
    ];

    for (case, body, expected_config) in cases {
        let created = create_target_group(&lattice, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");
        assert!(
            is_lattice_id(&id, "tg-"),
            "case {case:?}: expected an id of ^tg-[0-9a-z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            arn,
            expected_arn(TARGET_GROUP_KIND, &id),
            "case {case:?}: expected the ARN of the minted id, got {arn:?}"
        );
        assert_eq!(
            created.body["name"], body["name"],
            "case {case:?}: expected the submitted name, got {}",
            created.body
        );
        assert_eq!(
            created.body["type"], body["type"],
            "case {case:?}: expected the submitted type, got {}",
            created.body
        );
        assert_eq!(
            created.body["status"], "CREATE_IN_PROGRESS",
            "case {case:?}: a create answers the non-terminal status, got {}",
            created.body
        );
        assert_eq!(
            created.body["config"], expected_config,
            "case {case:?}: expected the config {expected_config}, got {}",
            created.body["config"]
        );

        // Both modelled identifier forms read the SAME stored record back.
        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = get_target_group(&lattice, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                id,
                "case {case:?} read by {form}: expected the stored id, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("arn"),
                arn,
                "case {case:?} read by {form}: expected the stored ARN, got {}",
                read.body
            );
            assert_eq!(
                read.body["name"], body["name"],
                "case {case:?} read by {form}: expected the stored name, got {}",
                read.body
            );
            assert_eq!(
                read.body["type"], body["type"],
                "case {case:?} read by {form}: expected the stored type, got {}",
                read.body
            );
            assert_eq!(
                read.body["config"], expected_config,
                "case {case:?} read by {form}: expected the persisted config, got {}",
                read.body["config"]
            );
            assert!(
                is_timestamp(&read.body["createdAt"]) && is_timestamp(&read.body["lastUpdatedAt"]),
                "case {case:?} read by {form}: expected RFC 3339 timestamps, got {}",
                read.body
            );
            assert_eq!(
                read.body["serviceArns"],
                serde_json::json!([]),
                "case {case:?} read by {form}: nothing routes to it yet, got {}",
                read.body["serviceArns"]
            );
        }
    }
}

/// One value for each config member, so a member can be sent on its own.
pub(crate) fn member_value(member: &str) -> serde_json::Value {
    match member {
        "port" => serde_json::json!(8080),
        "protocol" => serde_json::json!("HTTP"),
        "protocolVersion" => serde_json::json!("HTTP2"),
        "vpcIdentifier" => serde_json::json!(A_VPC),
        "ipAddressType" => serde_json::json!("IPV6"),
        "healthCheck" => serde_json::json!({ "enabled": true }),
        "lambdaEventStructureVersion" => serde_json::json!("V2"),
        other => panic!("the support table grew a member this test has no value for: {other}"),
    }
}

/// Rule 3, over every (type, member) pair the support table forbids, plus the pairs it
/// allows. The table is read from the service's own catalog, so a row cannot drift from
/// the member documentation it was built from.
///
/// Each row sends the member under test on top of the members its TYPE cannot do
/// without, so the only rule it can fail is the support one; the presence rule those
/// members come from has its own suite (`target_group_required_config`).
#[tokio::test]
async fn a_config_member_is_refused_for_a_type_that_does_not_support_it() {
    let lattice = Lattice::new();

    let all_types = [
        state::TYPE_IP,
        state::TYPE_LAMBDA,
        state::TYPE_INSTANCE,
        state::TYPE_ALB,
    ];
    let mut names = (0..).map(|n: u32| format!("support-case-{n}"));

    for (member, supported_for) in state::CONFIG_MEMBER_SUPPORT {
        for group_type in all_types {
            let name = names.next().expect("the name generator is unbounded");
            let case = format!("{member} sent for a {group_type} target group");
            let mut config = serde_json::Map::new();
            for required in crate::target_group_required_config::required_members(group_type) {
                config.insert((*required).to_string(), member_value(required));
            }
            config.insert((*member).to_string(), member_value(member));
            let answer = create_target_group(
                &lattice,
                serde_json::json!({
                    "name": name,
                    "type": group_type,
                    "config": serde_json::Value::Object(config),
                }),
            )
            .await;
            if supported_for.contains(&group_type) {
                assert_eq!(
                    answer.status, 201,
                    "case {case:?}: the member is supported for this type, expected 201, got {} / {}",
                    answer.status, answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?}",
                answer.error_code
            );
            let message = message_of(&case, &answer);
            assert_eq!(
                message,
                state::unsupported_config_member_message(member, group_type),
                "case {case:?}: the refusal must name the member and the type, got {message:?}"
            );
            assert_eq!(
                answer.body["fieldList"][0]["name"], *member,
                "case {case:?}: the refusal must name the member it constrains, got {}",
                answer.body
            );
        }
    }
}

/// Rule 4: each documented health-check range is enforced at its published bounds and
/// one past each of them, over the service's own catalog of those ranges.
#[tokio::test]
async fn health_check_members_are_refused_outside_their_documented_ranges() {
    let lattice = Lattice::new();
    let mut names = (0..).map(|n: u32| format!("health-case-{n}"));

    for &(member, min, max, _) in state::HEALTH_CHECK_RANGES {
        // (what the value is, the value, whether the service must accept it)
        let rows = [
            ("the published minimum", min, true),
            ("the published maximum", max, true),
            ("one below the published minimum", min - 1, false),
            ("one above the published maximum", max + 1, false),
        ];
        for (what, value, accepted) in rows {
            let name = names.next().expect("the name generator is unbounded");
            let case = format!("{member} at {what} ({value})");
            let mut health_check = serde_json::Map::new();
            health_check.insert(member.to_string(), serde_json::json!(value));
            let answer = create_target_group(
                &lattice,
                serde_json::json!({
                    "name": name,
                    "type": "INSTANCE",
                    "config": {
                        "protocol": "HTTP",
                        "vpcIdentifier": A_VPC,
                        "healthCheck": serde_json::Value::Object(health_check),
                    },
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    answer.status, 201,
                    "case {case:?}: a published bound is inside the range, expected 201, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["config"]["healthCheck"][member],
                    serde_json::json!(value),
                    "case {case:?}: expected the submitted value persisted, got {}",
                    answer.body["config"]["healthCheck"]
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?}",
                answer.error_code
            );
            let message = message_of(&case, &answer);
            let path = format!("config.healthCheck.{member}");
            assert!(
                message.contains(&path) && message.contains(&value.to_string()),
                "case {case:?}: the refusal must name where the value was sent and echo it, got {message:?}"
            );
            if value < min {
                // Below the documented minimum the modelled `@range` is satisfied (it
                // admits 0), so this refusal is the handler's own and carries the
                // violated member in `fieldList`; above the documented maximum, which
                // the modelled `@range` shares, the boundary refuses it first.
                assert_eq!(
                    answer.body["fieldList"][0]["name"], path,
                    "case {case:?}: the refusal must name the member it constrains, got {}",
                    answer.body
                );
            }
        }
    }
}

/// Rules 5 and 6, over a table of name shapes, token replays and identifiers that name
/// nothing.
#[tokio::test]
async fn names_are_unique_per_account_tokens_replay_and_unknown_identifiers_are_not_found() {
    let lattice = Lattice::new();

    let (first_id, first_arn) = a_target_group(
        &lattice,
        serde_json::json!({
            "name": "unique-name",
            "type": "LAMBDA",
            "clientToken": "token-one",
        }),
    )
    .await;

    // Rule 5: the name is taken for the whole account, whatever the second create's
    // type or config is.
    for (case, body) in [
        (
            "the same name and type",
            serde_json::json!({ "name": "unique-name", "type": "LAMBDA" }),
        ),
        (
            "the same name under another type",
            serde_json::json!({
                "name": "unique-name",
                "type": "IP",
                "config": { "protocol": "HTTP", "vpcIdentifier": A_VPC },
            }),
        ),
    ] {
        let answer = create_target_group(&lattice, body).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected a ConflictException, got {:?}",
            answer.error_code
        );
        let message = message_of(case, &answer);
        assert!(
            message.contains("unique-name") && message.contains(state::account_id()),
            "case {case:?}: the refusal must name the name and the account it is taken in, got {message:?}"
        );
        assert_eq!(
            answer.body["resourceId"], first_id,
            "case {case:?}: the refusal must name the target group holding the name, got {}",
            answer.body
        );
    }

    // Rule 5, second half: a retry carrying the first create's token answers with that
    // same target group rather than minting a second one.
    let replayed = create_target_group(
        &lattice,
        serde_json::json!({
            "name": "unique-name",
            "type": "LAMBDA",
            "clientToken": "token-one",
        }),
    )
    .await;
    assert_eq!(
        replayed.status, 201,
        "case a clientToken retry: expected 201, got {} / {}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.str_field("id"),
        first_id,
        "case a clientToken retry: expected the first target group, got {}",
        replayed.body
    );

    // A name is free again under a different name, and a token reused for different
    // parameters conflicts — whichever parameter differs.
    for (case, body) in [
        (
            "a token reused for a different name",
            serde_json::json!({
                "name": "other-name",
                "type": "LAMBDA",
                "clientToken": "token-one",
            }),
        ),
        (
            "a token reused with a different config",
            serde_json::json!({
                "name": "unique-name",
                "type": "LAMBDA",
                "config": { "lambdaEventStructureVersion": "V2" },
                "clientToken": "token-one",
            }),
        ),
        (
            "a token reused with different tags",
            serde_json::json!({
                "name": "unique-name",
                "type": "LAMBDA",
                "tags": { "team": "payments" },
                "clientToken": "token-one",
            }),
        ),
    ] {
        let reused_token = create_target_group(&lattice, body).await;
        assert_eq!(
            reused_token.status, 409,
            "case {case:?}: expected 409, got {} / {}",
            reused_token.status, reused_token.body
        );
    }

    // A retry that spells a documented default out explicitly is the SAME request, so it
    // still replays rather than conflicting.
    let explicit_default = create_target_group(
        &lattice,
        serde_json::json!({
            "name": "unique-name",
            "type": "LAMBDA",
            "config": { "lambdaEventStructureVersion": "V1" },
            "clientToken": "token-one",
        }),
    )
    .await;
    assert_eq!(
        explicit_default.status, 201,
        "case a retry spelling the documented default out: expected 201, got {} / {}",
        explicit_default.status, explicit_default.body
    );
    assert_eq!(
        explicit_default.str_field("id"),
        first_id,
        "case a retry spelling the documented default out: expected the first target group, got {}",
        explicit_default.body
    );

    // Rule 6: names the modelled pattern forbids, each refused with the construction
    // the pattern violation reports.
    for (case, name) in [
        ("the reserved id prefix", "tg-reserved-prefix"),
        ("a leading hyphen", "-leading"),
        ("a trailing hyphen", "trailing-"),
        ("two hyphens in a row", "double--hyphen"),
        ("an upper-case letter", "NotLower"),
    ] {
        let answer = create_target_group(
            &lattice,
            serde_json::json!({ "name": name, "type": "LAMBDA" }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({name:?}): expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({name:?}): expected a ValidationException, got {:?}",
            answer.error_code
        );
        let message = message_of(case, &answer);
        assert!(
            message.contains(name) && message.contains("name"),
            "case {case:?}: the refusal must echo the submitted name at its member, got {message:?}"
        );
    }

    // Rule 6, second half: every identifier form that names no stored target group.
    let absent_id = format!("tg-{:0>17}", "0");
    let absent_arn = expected_arn(TARGET_GROUP_KIND, &absent_id);
    for (case, identifier) in [
        ("an id no create ever minted", absent_id.clone()),
        ("an ARN no create ever minted", as_label(&absent_arn)),
    ] {
        let answer = get_target_group(&lattice, &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        let submitted = identifier.replace("%2F", "/");
        assert_eq!(
            message_of(case, &answer),
            format!("TargetGroup {submitted} cannot be found."),
            "case {case:?}: the refusal must name the identifier the caller sent, got {}",
            answer.body
        );
    }

    // The stored one is still readable after all of those refusals.
    let read = get_target_group(&lattice, &as_label(&first_arn)).await;
    assert_eq!(
        read.str_field("id"),
        first_id,
        "case the first target group after the refusals: expected it intact, got {}",
        read.body
    );
}

/// Rule 7: the create answers the non-terminal status and the target group reaches
/// `ACTIVE` once the span the clock owes it has passed, with nothing but the clock
/// moving.
#[tokio::test]
async fn a_created_target_group_reaches_active_after_the_transition_the_clock_owes_it() {
    let lattice = Lattice::new();

    let created = create_target_group(
        &lattice,
        serde_json::json!({ "name": "clock-tg", "type": "LAMBDA" }),
    )
    .await;
    let id = created.str_field("id");
    assert_eq!(
        created.str_field("status"),
        "CREATE_IN_PROGRESS",
        "case immediately after the create: expected CREATE_IN_PROGRESS, got {}",
        created.body
    );
    let straight_back = get_target_group(&lattice, &id).await;
    assert_eq!(
        straight_back.str_field("status"),
        "CREATE_IN_PROGRESS",
        "case read inside the transition window: expected CREATE_IN_PROGRESS, got {}",
        straight_back.body
    );

    tokio::time::sleep(state::CREATE_TRANSITION + std::time::Duration::from_millis(300)).await;

    let after = get_target_group(&lattice, &id).await;
    assert_eq!(
        after.str_field("status"),
        "ACTIVE",
        "case read after the transition: expected ACTIVE, got {}",
        after.body
    );
}

/// Rule 8: `serviceArns` follows the stored listener and rule records, so it names the
/// services that route to this target group and no others.
#[tokio::test]
async fn service_arns_report_the_services_whose_actions_forward_to_the_target_group() {
    let lattice = Lattice::new();

    let (routed_id, routed_arn) =
        a_target_group(&lattice, serde_json::json!({ "name": "routed-tg", "type": "LAMBDA" })).await;
    let (idle_id, _) =
        a_target_group(&lattice, serde_json::json!({ "name": "idle-tg", "type": "LAMBDA" })).await;

    let (listener_service_id, listener_service_arn) = a_service(&lattice, "forwarding-svc").await;
    let listener = create_listener(
        &lattice,
        &listener_service_id,
        serde_json::json!({
            "name": "forwarding-listener",
            "protocol": "HTTP",
            "port": 80,
            "defaultAction": {
                "forward": { "targetGroups": [{ "targetGroupIdentifier": routed_id }] },
            },
        }),
    )
    .await;
    assert_eq!(
        listener.status, 201,
        "case a listener forwarding to the target group: expected 201, got {} / {}",
        listener.status, listener.body
    );

    // A second service reaches the same target group through a RULE rather than through
    // its listener's default action, and names it by its ARN.
    let (rule_service_id, rule_service_arn) = a_service(&lattice, "rule-svc").await;
    let rule_listener = a_listener(&lattice, &rule_service_id, "rule-listener").await;
    let rule = create_rule(
        &lattice,
        &rule_service_id,
        &rule_listener,
        serde_json::json!({
            "name": "to-target-group",
            "priority": 10,
            "match": { "httpMatch": { "pathMatch": { "match": { "prefix": "/x" } } } },
            "action": {
                "forward": { "targetGroups": [{ "targetGroupIdentifier": routed_arn }] },
            },
        }),
    )
    .await;
    assert_eq!(
        rule.status, 201,
        "case a rule forwarding to the target group: expected 201, got {} / {}",
        rule.status, rule.body
    );

    let mut expected = vec![listener_service_arn, rule_service_arn];
    expected.sort();
    let read = get_target_group(&lattice, &routed_id).await;
    assert_eq!(
        read.body["serviceArns"],
        serde_json::json!(expected),
        "case the routed target group: expected the forwarding services' ARNs, got {}",
        read.body["serviceArns"]
    );

    let idle = get_target_group(&lattice, &idle_id).await;
    assert_eq!(
        idle.body["serviceArns"],
        serde_json::json!([]),
        "case the target group nothing forwards to: expected no service ARNs, got {}",
        idle.body["serviceArns"]
    );
}
