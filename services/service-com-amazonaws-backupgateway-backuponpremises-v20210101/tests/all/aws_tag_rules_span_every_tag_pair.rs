// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two tag rules this model states only in prose, over EVERY
//! member that carries an Amazon Web Services tag key/value pair — a resource's `Tags`
//! and a hypervisor property mapping's `AwsTagKey`/`AwsTagValue`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the mapping helpers of
//! `hypervisor_property_mappings`, so what is asserted is the wire behaviour and the only
//! way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: a pair whose KEY starts with the reserved `aws:` prefix, or whose
//! VALUE carries a space, is refused with the modelled `ValidationException` at HTTP 400
//! and nothing is written — whichever member carried it, because
//! `VmwareToAwsTagMapping$AwsTagKey` and `$AwsTagValue` target the very `TagKey` and
//! `TagValue` shapes `Tag$Key` and `Tag$Value` do
//! (`smithy_query(query="shape",shape="VmwareToAwsTagMapping,Tag")`), and a pair that
//! satisfies both rules is stored and reads back unchanged through either member. The
//! rules are the model's own sentences: "The key can't start with `aws:`" (`Tag$Key`) and
//! "Spaces are not allowed in tag values" (the `Tag` shape), neither of which the members'
//! `@pattern`s refuse.
//!
//! The tables below span that class rather than one request: reserved keys differing in
//! case and in what follows the prefix, spaced values differing in which space character
//! they carry and where in the value it sits (between words, leading, trailing, alone, at
//! the length maximum), the offending pair sitting at a different position in the mapping
//! list, and accepted pairs differing in character class, script and length, including
//! both published bounds of `TagValue`'s `@length(min: 0, max: 256)`.
//!
//! `tag_value_spaces` covers the `Tags` member on its own; this suite is what pins the two
//! members to ONE answer.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{VmwareToAwsTagMapping, MAPPINGS_NOT_FOUND};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply};
use crate::gateway_server_association::{associate, message_of};
use crate::hypervisor_property_mappings::{
    expected_mappings, get_mappings, import, mapping, persisted_mappings,
};

/// An `IamRoleArn` of this account, inside the modelled pattern
/// `^arn:(aws|aws-cn|aws-us-gov):iam::([0-9]+):role/(\S+)$`.
pub(crate) fn role_arn(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A hypervisor a gateway is associated with: the scene a put can succeed in, driven
/// entirely over the wire (the gate `PutHypervisorPropertyMappings` applies is the
/// association `AssociateGatewayToServer` writes).
pub(crate) async fn served_hypervisor(
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
) -> String {
    let hypervisor = import(router, name, host).await;
    let gateway = create(router, &format!("gw-{name}")).await;
    let associated = associate(router, &gateway, &hypervisor).await;
    assert_eq!(
        associated.status, 200,
        "the gateway must associate with {hypervisor}, got {} {:?}",
        associated.status, associated.body
    );
    hypervisor
}

async fn put(
    router: &MultiServiceRouter,
    arn: &str,
    role: &str,
    mappings: &[VmwareToAwsTagMapping],
) -> Reply {
    call(
        router,
        "PutHypervisorPropertyMappings",
        &json!({
            "HypervisorArn": arn,
            "IamRoleArn": role,
            "VmwareToAwsTagMappings": expected_mappings(mappings),
        }),
    )
    .await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, key: &str, value: &str) -> Reply {
    call(
        router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": [{ "Key": key, "Value": value }] }),
    )
    .await
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let reply = call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "ListTagsForResource must answer 200 for {arn}, got {} {:?}",
        reply.status, reply.body
    );
    reply.tags()
}

/// Assert one refusal of a mapping: 400, the modelled `ValidationException`, and a message
/// naming the member that failed its constraint — at the validation path of the AWS half of
/// the mapping at `position` in the list, the way the boundary validator names a member it
/// refuses itself (`refusal_names_the_member_one_way`) — and the value it refused.
fn assert_mapping_refused(case: &str, position: usize, offending: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: the put must be refused with 400 ValidationException, got {} {:?}",
        reply.status,
        reply.body
    );
    let stated = message_of(reply);
    assert!(
        stated.starts_with("1 validation error detected: "),
        "case {case:?}: one violated constraint must state one validation error, got \
         {stated:?}"
    );
    let at = format!("at 'vmwareToAwsTagMappings.{position}.member.awsTag");
    assert!(
        stated.contains(&at),
        "case {case:?}: the refusal must state which member of which element failed its \
         constraint ({at:?}), got {stated:?}"
    );
    assert!(
        stated.contains(offending),
        "case {case:?}: the refusal must report the {offending:?} it refused, got {stated:?}"
    );
}

/// Pairs no member of this service may persist, with the half that breaks a rule.
///
/// The reserved-prefix rows span the case of the prefix (the reservation is
/// case-insensitive) and what follows it, including the bare prefix as the whole key; the
/// space rows span the Unicode space family — the separators the modelled `TagKey` pattern
/// spells out as `\p{Z}`, where a KEY is deliberately allowed them — the tab and line
/// forms, and where in the value the space sits. The last space row puts one at
/// `TagValue`'s `@length(max: 256)` boundary, so a value the length constraint accepts is
/// still refused for the space it ends on.
fn refused_pairs() -> Vec<(String, String, String, String)> {
    // (case, key, value, the half the refusal must report)
    let mut rows: Vec<(String, String, String, String)> = [
        ("the reserved prefix in lower case", "aws:created-by", "me", "aws:created-by"),
        ("the reserved prefix in upper case", "AWS:Owner", "me", "AWS:Owner"),
        ("the reserved prefix in mixed case", "Aws:Owner", "me", "Aws:Owner"),
        ("nothing but the reserved prefix", "aws:", "me", "aws:"),
        ("the reserved prefix before a path", "aws:cloudformation/stack", "s", "aws:cloudformation/stack"),
        ("an ASCII space between words", "Owner", "two words", "two words"),
        ("a leading ASCII space", "Owner", " leading", " leading"),
        ("a trailing ASCII space", "Owner", "trailing ", "trailing "),
        ("nothing but one ASCII space", "Owner", " ", " "),
        ("a horizontal tab", "Owner", "two\tfields", "two\tfields"),
        ("a line feed", "Owner", "first\nsecond", "first\nsecond"),
        ("a no-break space (U+00A0)", "Owner", "non\u{00a0}breaking", "non\u{00a0}breaking"),
        ("an ideographic space (U+3000)", "Owner", "wide\u{3000}space", "wide\u{3000}space"),
    ]
    .into_iter()
    .map(|(case, key, value, offending)| {
        (
            case.to_string(),
            key.to_string(),
            value.to_string(),
            offending.to_string(),
        )
    })
    .collect();
    let at_the_maximum = format!("{} ", "v".repeat(255));
    rows.push((
        "a space at the 256-character value maximum".to_string(),
        "Owner".to_string(),
        at_the_maximum.clone(),
        at_the_maximum,
    ));
    rows.push((
        "both rules broken at once".to_string(),
        "aws:owner".to_string(),
        "two words".to_string(),
        "aws:owner".to_string(),
    ));
    rows
}

/// Pairs every member of this service persists unchanged: the rules must refuse nothing
/// else.
///
/// The character catalog is the `Tag` doc's own list — "UTF-8 letters, numbers, and the
/// following characters: + - = . _ : /" — and the lengths are both bounds of `TagValue`'s
/// `@length(min: 0, max: 256)`. The `aws:`-later-on row is what pins the rule to the
/// PREFIX rather than to the substring, and the spaced-key row to the VALUE rather than to
/// spaces in general.
fn accepted_pairs() -> Vec<(String, String, String)> {
    // (case, key, value)
    let mut rows: Vec<(String, String, String)> = [
        ("a plain pair", "Department", "Finance"),
        ("every character the Tag doc lists beside letters and digits", "k+-=._:/", "+-=._:/"),
        ("the reserved prefix later in the key", "owner-aws:core", "platform"),
        ("a key carrying spaces, which its pattern admits", "cost center", "ops"),
        ("letters of a non-latin script", "Ключ", "значение"),
        ("the empty value, TagValue's modelled length minimum", "Empty", ""),
    ]
    .into_iter()
    .map(|(case, key, value)| (case.to_string(), key.to_string(), value.to_string()))
    .collect();
    rows.push((
        "a 256-character value, TagValue's modelled length maximum".to_string(),
        "Longest".to_string(),
        "v".repeat(256),
    ));
    rows
}

/// A mapping whose AWS half breaks either rule is refused, wherever in the list it sits,
/// and the refusal writes nothing: the hypervisor still carries no mappings at all.
#[tokio::test]
async fn a_mapping_whose_aws_tag_pair_breaks_a_tag_rule_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&router, "hvmaprules", "198.51.100.110").await;
    let role = role_arn("mapping-rules");

    for (case, key, value, offending) in refused_pairs() {
        // The offending mapping alone, and the same mapping as the middle one of three —
        // the rule is about the pair, not about which element carries it.
        let alone = vec![mapping("env", "prod", &key, &value)];
        let among_others = vec![
            mapping("tier", "gold", "Tier", "Gold"),
            mapping("env", "prod", &key, &value),
            mapping("owner", "platform", "Owner", "Platform"),
        ];
        for (where_it_sits, position, mappings) in
            [("alone", 1, alone), ("second of three", 2, among_others)]
        {
            let case = format!("{case} ({where_it_sits})");
            let reply = put(&router, &hypervisor, &role, &mappings).await;
            assert_mapping_refused(&case, position, &offending, &reply);
            assert_eq!(
                persisted_mappings(&engine, &hypervisor),
                "None",
                "case {case:?}: a refused put must write no mappings"
            );
            let read = get_mappings(&router, &hypervisor).await;
            assert_eq!(
                (read.status, read.str_at("/ErrorCode")),
                (400, Some(MAPPINGS_NOT_FOUND)),
                "case {case:?}: the hypervisor must still carry no mappings, got {} {:?}",
                read.status,
                read.body
            );
        }
    }

    // A mapping set that breaks neither rule is accepted on the very same hypervisor, so
    // the refusals above are about the pairs and not about the operation.
    let accepted = vec![mapping("env", "prod", "Environment", "Production")];
    let reply = put(&router, &hypervisor, &role, &accepted).await;
    assert_eq!(
        (reply.status, reply.str_at("/HypervisorArn")),
        (200, Some(hypervisor.as_str())),
        "a mapping satisfying both rules must be accepted, got {} {:?}",
        reply.status,
        reply.body
    );
}

/// One pair, one answer: a key/value pair sent as a resource tag and as a mapping's AWS
/// half is accepted by both members or refused by both — and an accepted pair reads back
/// unchanged through each of them.
#[tokio::test]
async fn one_tag_pair_is_answered_the_same_way_as_a_tag_and_as_a_mapping() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&router, "hvonepair", "198.51.100.111").await;
    let role = role_arn("one-answer");

    for (case, key, value, offending) in refused_pairs() {
        let tagged = tag_resource(&router, &hypervisor, &key, &value).await;
        assert_eq!(
            (tagged.status, tagged.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: TagResource must refuse the pair, got {} {:?}",
            tagged.status,
            tagged.body
        );
        let reply = put(
            &router,
            &hypervisor,
            &role,
            &[mapping("env", "prod", &key, &value)],
        )
        .await;
        assert_mapping_refused(&case, 1, &offending, &reply);
    }
    assert!(
        listed_tags(&router, &hypervisor).await.is_empty(),
        "no refused pair may have been stored as a tag"
    );
    assert_eq!(
        persisted_mappings(&engine, &hypervisor),
        "None",
        "no refused pair may have been stored as a mapping"
    );

    for (case, key, value) in accepted_pairs() {
        let tagged = tag_resource(&router, &hypervisor, &key, &value).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must accept the pair, got {} {:?}",
            tagged.status, tagged.body
        );
        let listed = listed_tags(&router, &hypervisor).await;
        assert_eq!(
            listed.get(&key),
            Some(&value),
            "case {case:?}: the tag must read back unchanged, ListTagsForResource \
             reported {listed:?}"
        );

        let mappings = vec![mapping("env", "prod", &key, &value)];
        let reply = put(&router, &hypervisor, &role, &mappings).await;
        assert_eq!(
            (reply.status, reply.str_at("/HypervisorArn")),
            (200, Some(hypervisor.as_str())),
            "case {case:?}: the put must accept the pair, got {} {:?}",
            reply.status,
            reply.body
        );
        let read = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            read.body
                .get("VmwareToAwsTagMappings")
                .cloned()
                .unwrap_or(Value::Null),
            expected_mappings(&mappings),
            "case {case:?}: the mapping must read back unchanged, got {:?}",
            read.body
        );
    }
}

/// A refused put leaves the mappings an earlier put stored exactly as they were: the
/// refusal happens before anything is written, however often it is repeated.
#[tokio::test]
async fn a_refused_mapping_leaves_the_stored_mappings_alone() {
    let engine = build_engine();
    let router = build_router(&engine);
    let hypervisor = served_hypervisor(&router, "hvkeeps", "198.51.100.112").await;
    let role = role_arn("keeps");

    let stored = vec![
        mapping("department", "finance", "Department", "Finance"),
        mapping("tier", "gold", "Tier", "Gold"),
    ];
    let accepted = put(&router, &hypervisor, &role, &stored).await;
    assert_eq!(
        accepted.status, 200,
        "the seed put must be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let before = persisted_mappings(&engine, &hypervisor);

    for (case, key, value, offending) in refused_pairs() {
        // Twice, so a refusal is as inert the second time as the first.
        for attempt in 1..=2 {
            let reply = put(
                &router,
                &hypervisor,
                &role_arn("refused"),
                &[mapping("env", "prod", &key, &value)],
            )
            .await;
            assert_mapping_refused(
                &format!("{case}, attempt {attempt}"),
                1,
                &offending,
                &reply,
            );
        }
        assert_eq!(
            persisted_mappings(&engine, &hypervisor),
            before,
            "case {case:?}: a refused put must leave the stored mappings alone"
        );
        let read = get_mappings(&router, &hypervisor).await;
        assert_eq!(
            (read.status, read.str_at("/IamRoleArn")),
            (200, Some(role.as_str())),
            "case {case:?}: the role of the accepted put must still be reported, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.body
                .get("VmwareToAwsTagMappings")
                .cloned()
                .unwrap_or(Value::Null),
            expected_mappings(&stored),
            "case {case:?}: the stored mappings must be the ones the accepted put wrote, \
             got {:?}",
            read.body
        );
    }
}
