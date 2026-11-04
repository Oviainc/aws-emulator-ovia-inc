// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about how this service refuses an input-constraint
//! violation, over every member whose constraint a HANDLER checks rather than the boundary
//! validator.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The rule under test: a refusal of "this member failed a constraint" names the offending
//! member by its VALIDATION PATH — every model name in lowerCamel, and inside a list the
//! element's position followed by the step from the collection into its element — and
//! states the clause under the AWS envelope `N validation error(s) detected: `. That is
//! one condition class, so the wording cannot depend on whether the constraint was a
//! modelled trait the boundary validator refused or a rule stated only in the model's prose
//! that a handler refuses itself: the rules of `Tag$Key`, `Tag$Value` and
//! `CreateGatewayInput$Tags`' documented ceiling.
//!
//! A refusal that is NOT about a member failing a constraint is outside this class and is
//! worded as its own class is: an unresolvable pagination token, whose member constraints
//! all hold, is covered by `page_token_refusal_wording`.
//!
//! The tables below span that class rather than one request: every operation of this model
//! that carries an Amazon Web Services tag pair, both halves of a pair, the offending pair
//! at each position of a list of one, two and three, values differing in which space
//! character they carry and in the case of the reserved prefix, and the resource-level tag
//! ceiling reached in one request and across two.
//! `the_boundary_and_a_handler_name_one_member_alike` is the row that pins the
//! spelling to the boundary validator's own, by reading the path out of a refusal of a
//! MODELLED constraint on the very same member.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::MAX_TAGS_PER_RESOURCE;

use crate::aws_tag_rules_span_every_tag_pair::{role_arn, served_hypervisor};
use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, issued_activation_key, Reply,
};
use crate::gateway_server_association::message_of;

/// The operations of this model that carry an Amazon Web Services tag key/value pair in a
/// list member: the three that tag a resource through `Tags`, and the put that maps VMware
/// tags onto AWS ones
/// (`smithy_query(query="shape",shape="CreateGatewayInput,ImportHypervisorConfigurationInput,TagResourceInput,PutHypervisorPropertyMappingsInput")`).
const TAG_PAIR_OPERATIONS: [&str; 4] = [
    "TagResource",
    "CreateGateway",
    "ImportHypervisorConfiguration",
    "PutHypervisorPropertyMappings",
];

/// Which half of a pair a row refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    Key,
    Value,
}

/// The requirement the clause states for each rule, as the service words it.
const RESERVED_PREFIX: &str = "a tag key must not start with the reserved prefix 'aws:'";
const NO_SPACES: &str = "a tag value must not contain spaces";

/// The list member and the two members of its element that each operation carries a tag
/// pair in, spelled as a validation path spells them — lowerCamel, from
/// `smithy_query(query="shape",shape="Tag,VmwareToAwsTagMapping")`: `Tag$Key`/`Tag$Value`
/// for a resource tag, and `VmwareToAwsTagMapping$AwsTagKey`/`$AwsTagValue`, which target
/// the very same `TagKey` and `TagValue` shapes.
fn pair_members(operation: &str) -> (&'static str, &'static str, &'static str) {
    match operation {
        "PutHypervisorPropertyMappings" => {
            ("vmwareToAwsTagMappings", "awsTagKey", "awsTagValue")
        }
        _ => ("tags", "key", "value"),
    }
}

/// The validation path of a member inside a list: the list member, the element's position
/// counted from 1, the step from the collection into its element, then the element's own
/// member.
///
/// Spelled out here rather than reused from the crate, so the assertion is independent of
/// the code that builds the path; what pins it to the boundary validator's own spelling is
/// [`the_boundary_and_a_handler_name_one_member_alike`].
fn element_path(list_member: &str, position: usize, member: &str) -> String {
    format!("{list_member}.{position}.member.{member}")
}

/// The whole `ValidationException$Message` a refusal of ONE violation must state: the AWS
/// envelope over the clause, which names the member at `at` and the requirement it failed.
/// `value` is `None` where the clause names the member alone.
fn expected_message(value: Option<&str>, at: &str, requirement: &str) -> String {
    let subject = match value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value".to_string(),
    };
    format!("{subject} at '{at}' failed to satisfy constraint: {requirement}")
}

/// The one-violation form of [`expected_message`], under the envelope.
fn expected_one(value: Option<&str>, at: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: {}",
        expected_message(value, at, requirement)
    )
}

/// The member path a message NAMES, read back out of the clause.
fn named_member(message: &str) -> &str {
    let (_, rest) = message
        .split_once(" at '")
        .unwrap_or_else(|| panic!("no constraint clause in {message:?}"));
    let (member, _) = rest
        .split_once("' failed to satisfy constraint: ")
        .unwrap_or_else(|| panic!("no constraint clause in {message:?}"));
    member
}

/// The resources the tag-carrying operations are driven against: a live gateway to tag, and
/// a hypervisor a gateway is associated with, so a put reaches the same handler a served
/// one does.
struct Scene {
    gateway: String,
    hypervisor: String,
    role: String,
}

async fn scene(router: &MultiServiceRouter) -> Scene {
    Scene {
        gateway: create(router, "gw-wording").await,
        hypervisor: served_hypervisor(router, "hvwording", "198.51.100.120").await,
        role: role_arn("refusal-wording"),
    }
}

/// `Tags` as the wire carries it.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// `VmwareToAwsTagMappings` carrying the same pairs as their AWS half; the VMware half is
/// filled per element with values inside its modelled lengths, since it is required and
/// these rules do not reach it.
fn mapping_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .enumerate()
            .map(|(index, (key, value))| {
                json!({
                    "VmwareCategory": format!("category-{index}"),
                    "VmwareTagName": format!("tag-{index}"),
                    "AwsTagKey": key,
                    "AwsTagValue": value,
                })
            })
            .collect(),
    )
}

/// Send `operation` carrying `pairs` as the Amazon Web Services tag pairs it accepts.
///
/// `nonce` keeps the display name and the host of the creating operations distinct, so each
/// row is a request of its own rather than a repeat.
async fn call_with_pairs(
    router: &MultiServiceRouter,
    scene: &Scene,
    operation: &str,
    pairs: &[(&str, &str)],
    nonce: usize,
) -> Reply {
    match operation {
        "TagResource" => {
            call(
                router,
                operation,
                &json!({ "ResourceARN": scene.gateway, "Tags": tag_list(pairs) }),
            )
            .await
        }
        "CreateGateway" => {
            let activation_key = issued_activation_key(router).await;
            let mut body = create_body(
                &format!("gw-wording-{nonce}"),
                &activation_key,
                &BTreeMap::new(),
            );
            body["Tags"] = tag_list(pairs);
            call(router, operation, &body).await
        }
        "ImportHypervisorConfiguration" => {
            call(
                router,
                operation,
                &json!({
                    "Name": format!("hv-wording-{nonce}"),
                    "Host": format!("198.51.100.{}", nonce % 200 + 1),
                    "Tags": tag_list(pairs),
                }),
            )
            .await
        }
        "PutHypervisorPropertyMappings" => {
            call(
                router,
                operation,
                &json!({
                    "HypervisorArn": scene.hypervisor,
                    "IamRoleArn": scene.role,
                    "VmwareToAwsTagMappings": mapping_list(pairs),
                }),
            )
            .await
        }
        other => panic!("no request builder for {other}"),
    }
}

/// One row of the refusal table: the tag pairs a request carries, where in that list the
/// offending pair sits, which half of it breaks a rule, the value the clause must report
/// and the requirement it must state.
struct RefusedRow {
    case: &'static str,
    pairs: Vec<(&'static str, &'static str)>,
    position: usize,
    half: Half,
    offending: &'static str,
    requirement: &'static str,
}

/// Pairs a handler refuses.
///
/// The rows span both rules, both halves, lists of one, two and three with the offending
/// pair at each position, the case of the reserved prefix and what follows it, and the
/// space family — the separators `TagKey`'s pattern spells out as `\p{Z}`, where a key is
/// deliberately allowed them. The last row breaks both rules at once, so the clause has to
/// name the half that is checked first.
fn refused_rows() -> Vec<RefusedRow> {
    let row = |case, pairs, position, half, offending, requirement| RefusedRow {
        case,
        pairs,
        position,
        half,
        offending,
        requirement,
    };
    vec![
        row(
            "a reserved key, the only pair",
            vec![("aws:owner", "platform")],
            1,
            Half::Key,
            "aws:owner",
            RESERVED_PREFIX,
        ),
        row(
            "a reserved key in upper case, second of two",
            vec![("ok", "fine"), ("AWS:Owner", "platform")],
            2,
            Half::Key,
            "AWS:Owner",
            RESERVED_PREFIX,
        ),
        row(
            "a reserved key before a path, third of three",
            vec![
                ("first", "1"),
                ("second", "2"),
                ("aws:cloudformation/stack", "s"),
            ],
            3,
            Half::Key,
            "aws:cloudformation/stack",
            RESERVED_PREFIX,
        ),
        row(
            "a spaced value, the only pair",
            vec![("Owner", "two words")],
            1,
            Half::Value,
            "two words",
            NO_SPACES,
        ),
        row(
            "a leading space in the value, second of two",
            vec![("ok", "fine"), ("Owner", " leading")],
            2,
            Half::Value,
            " leading",
            NO_SPACES,
        ),
        row(
            "a horizontal tab in the value, third of three",
            vec![("first", "1"), ("second", "2"), ("Owner", "two\tfields")],
            3,
            Half::Value,
            "two\tfields",
            NO_SPACES,
        ),
        row(
            "an ideographic space (U+3000) in the value, second of three",
            vec![
                ("first", "1"),
                ("Owner", "wide\u{3000}space"),
                ("third", "3"),
            ],
            2,
            Half::Value,
            "wide\u{3000}space",
            NO_SPACES,
        ),
        row(
            "both rules broken by one pair",
            vec![("aws:owner", "two words")],
            1,
            Half::Key,
            "aws:owner",
            RESERVED_PREFIX,
        ),
    ]
}

/// A tag rule a handler refuses states the whole message the boundary form states: the
/// envelope, the member at its validation path with the offending element's position, and
/// the value refused — on every operation that carries a tag pair.
#[tokio::test]
async fn a_handler_refused_tag_rule_names_the_member_at_its_validation_path() {
    let engine = build_engine();
    let router = build_router(&engine);
    let scene = scene(&router).await;

    let mut nonce = 0;
    for operation in TAG_PAIR_OPERATIONS {
        let (list, key_member, value_member) = pair_members(operation);
        for row in refused_rows() {
            nonce += 1;
            let case = format!("{operation}: {}", row.case);
            let refused =
                call_with_pairs(&router, &scene, operation, &row.pairs, nonce).await;
            assert_eq!(
                (refused.status, refused.code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: the pair must be refused with 400 ValidationException, \
                 got {} {:?}",
                refused.status,
                refused.body
            );
            let member = match row.half {
                Half::Key => key_member,
                Half::Value => value_member,
            };
            let expected = expected_one(
                Some(row.offending),
                &element_path(list, row.position, member),
                row.requirement,
            );
            let stated = message_of(&refused);
            assert_eq!(
                stated, expected,
                "case {case:?}: the refusal must state {expected:?}, got {stated:?}"
            );
        }
    }
}

/// One row of the two-sided table: the half of a pair under test, the pair a HANDLER
/// refuses on it and the pair the BOUNDARY validator refuses on the very same member.
struct BothSidesRow<'a> {
    case: &'a str,
    half: Half,
    handler_pair: (&'a str, &'a str),
    boundary_pair: (&'a str, &'a str),
}

/// The row that pins the spelling: for the SAME member of the same element, the path a
/// handler's refusal names is the path the boundary validator names when a MODELLED
/// constraint on that member fails — and both carry the envelope.
///
/// The modelled violations are `TagKey`'s `@length(min: 1)` on an empty key and
/// `TagValue`'s `@length(max: 256)` on a 257-character value
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`), neither of which the handler
/// rules can reach.
#[tokio::test]
async fn the_boundary_and_a_handler_name_one_member_alike() {
    let engine = build_engine();
    let router = build_router(&engine);
    let scene = scene(&router).await;
    let too_long = "v".repeat(257);

    let mut nonce = 1_000;
    for operation in TAG_PAIR_OPERATIONS {
        let (list, key_member, value_member) = pair_members(operation);
        for position in [1usize, 2] {
            let rows = [
                BothSidesRow {
                    case: "the key half",
                    half: Half::Key,
                    handler_pair: ("aws:owner", "platform"),
                    boundary_pair: ("", "platform"),
                },
                BothSidesRow {
                    case: "the value half",
                    half: Half::Value,
                    handler_pair: ("Owner", "two words"),
                    boundary_pair: ("Owner", too_long.as_str()),
                },
            ];
            for BothSidesRow {
                case,
                half,
                handler_pair,
                boundary_pair,
            } in rows
            {
                nonce += 1;
                let case = format!("{operation}, element {position}: {case}");
                let member = match half {
                    Half::Key => key_member,
                    Half::Value => value_member,
                };
                let expected_path = element_path(list, position, member);

                let mut refused_paths = Vec::new();
                for (side, offending) in [("handler", handler_pair), ("boundary", boundary_pair)]
                {
                    // The offending pair at `position`, after as many pairs as it takes to
                    // put it there — so the path has to carry the element's place.
                    let mut pairs: Vec<(&str, &str)> =
                        (1..position).map(|_| ("filler", "fine")).collect();
                    pairs.push(offending);
                    nonce += 1;
                    let refused =
                        call_with_pairs(&router, &scene, operation, &pairs, nonce).await;
                    assert_eq!(
                        (refused.status, refused.code.as_deref()),
                        (400, Some("ValidationException")),
                        "case {case:?} ({side}): the pair must be refused with 400 \
                         ValidationException, got {} {:?}",
                        refused.status,
                        refused.body
                    );
                    let stated = message_of(&refused);
                    assert!(
                        stated.starts_with("1 validation error detected: "),
                        "case {case:?} ({side}): one violated constraint must state one \
                         validation error, got {stated:?}"
                    );
                    assert_eq!(
                        named_member(&stated),
                        expected_path,
                        "case {case:?} ({side}): the clause must name the member at its \
                         validation path, got {stated:?}"
                    );
                    refused_paths.push(named_member(&stated).to_string());
                }
                assert_eq!(
                    refused_paths[0], refused_paths[1],
                    "case {case:?}: the handler and the boundary must name one member one \
                     way, got {refused_paths:?}"
                );
            }
        }
    }
}

/// A rule about the MEMBER rather than about one element of it is reported at the member
/// itself: the resource tag ceiling, whether one request reaches it or two together do.
#[tokio::test]
async fn the_tag_ceiling_is_reported_at_the_tag_member_itself() {
    let engine = build_engine();
    let router = build_router(&engine);

    let pair = |index: usize| (format!("k{index}"), format!("v{index}"));
    let as_pairs = |rows: &[(String, String)]| -> Value {
        Value::Array(
            rows.iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        )
    };

    // One request past the ceiling, and a request that is only past it once merged with
    // the tags the resource already carries: one rule, one wording.
    let over_in_one: Vec<(String, String)> =
        (0..=MAX_TAGS_PER_RESOURCE).map(pair).collect();
    let up_to_the_ceiling: Vec<(String, String)> =
        (0..MAX_TAGS_PER_RESOURCE).map(pair).collect();
    let one_more = vec![pair(MAX_TAGS_PER_RESOURCE)];

    let expected = expected_one(
        Some(&(MAX_TAGS_PER_RESOURCE + 1).to_string()),
        "tags",
        &format!("a resource must carry at most {MAX_TAGS_PER_RESOURCE} tags"),
    );

    for (case, seed, refused_tags) in [
        ("in one request", Vec::new(), over_in_one),
        ("merged with the tags already carried", up_to_the_ceiling, one_more),
    ] {
        let gateway = create(&router, &format!("gw-ceiling-{}", seed.len())).await;
        if !seed.is_empty() {
            let accepted = call(
                &router,
                "TagResource",
                &json!({ "ResourceARN": gateway, "Tags": as_pairs(&seed) }),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: tagging up to the ceiling must be accepted, got {} {:?}",
                accepted.status, accepted.body
            );
        }
        let refused = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": gateway, "Tags": as_pairs(&refused_tags) }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: passing the ceiling must be refused with 400 \
             ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        let stated = message_of(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: the refusal must state {expected:?}, got {stated:?}"
        );
    }
}
