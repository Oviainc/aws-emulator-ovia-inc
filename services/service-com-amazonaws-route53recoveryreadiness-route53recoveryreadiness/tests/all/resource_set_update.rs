// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateResourceSet`.
//!
//! The rule under test: an update of a resource set is a WHOLE-RESOURCE replacement —
//! the submitted resource type and resource list become the stored ones exactly (no
//! merge with what was there), the record's identity (its ARN and its name) and the
//! tags it carries are untouched, and every read surface afterwards reports the
//! replacement; an update addressed to a set that does not exist changes nothing and is
//! refused with HTTP 404 `ResourceNotFoundException`, and an update whose body breaks a
//! rule a create is held to is refused with HTTP 400 `ValidationException` with the
//! previous record left intact.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with `readiness_check_lifecycle.rs`
//! rather than copied), so only the HTTP status, the `x-amzn-errortype` header and the
//! JSON body are inspected: an answer that is not computed from persisted state cannot
//! pass.
//!
//! The tests are tables whose rows differ along what the rule generalises over: the
//! resource COUNT before and after (growing, shrinking, emptying, up to the stored
//! ceiling), which `Resource` members each entry carries and how deep its
//! `DnsTargetResource` subtree goes, which catalog entry the type is moved to, the
//! number of updates applied in a row, and the tag count the record carries while being
//! updated.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    MAX_READINESS_SCOPES_PER_RESOURCE, MAX_RESOURCES_PER_SET,
};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The constant sentence this service answers a not-found with. Written out here rather
/// than imported, so the test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// Resource types the `ResourceSetType` member's own documentation enumerates (model:
/// `smithy_query(query="doc", shape="UpdateResourceSetRequest",
/// member="ResourceSetType")`). These are entries the issue cited none of; the update
/// table moves a stored set onto each of them in turn.
const DOCUMENTED_RESOURCE_SET_TYPES: &[&str] = &[
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::EC2::CustomerGateway",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::RDS::DBCluster",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
    "AWS::SQS::Queue",
    "AWS::EC2::VPC",
];

/// The ARN AWS documents for a resource set. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty:
/// `arn:aws:route53-recovery-readiness::<account>:resource-set/<name>`. Written out here
/// rather than taken from the handler's helper, so the test pins the format.
fn resource_set_arn(name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:resource-set/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// `POST /resourcesets` (model: `CreateResourceSet`).
async fn create_resource_set(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/resourcesets", Some(body)).await
}

/// `GET /resourcesets/{ResourceSetName}` (model: `GetResourceSet`).
async fn get_resource_set(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/resourcesets/{name}"), None)
        .await
}

/// `PUT /resourcesets/{ResourceSetName}` (model: `UpdateResourceSet`).
async fn update_resource_set(
    emulator: &Emulator,
    name: &str,
    resource_set_type: &str,
    resources: &Value,
) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/resourcesets/{name}"),
            Some(json!({
                "resourceSetType": resource_set_type,
                "resources": resources,
            })),
        )
        .await
}

/// The entry `ListResourceSets` reports for `name`, or `None` when the listing does not
/// hold it.
async fn listed_entry(emulator: &Emulator, name: &str) -> Option<Value> {
    let listed = emulator.send("GET", "/resourcesets?maxResults=1000", None).await;
    listed
        .field("resourceSets")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("resourceSetName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

/// A reply's `resources` member, with an omitted member read as the empty list: the wire
/// may carry `[]` or nothing at all for a set that holds none.
fn resources_of(value: &Value) -> Value {
    match value.get("resources") {
        Some(Value::Array(items)) => Value::Array(items.clone()),
        _ => Value::Array(Vec::new()),
    }
}

/// `value` with every null member and every absent-as-null entry removed, so a submitted
/// resource list can be compared to the one reported back without the comparison
/// depending on whether the serializer writes an omitted member as `null` or leaves it
/// out.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), without_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// One resource carrying only an ARN.
fn arn_resource(index: usize) -> Value {
    json!({ "resourceArn": format!("arn:aws:dynamodb:us-east-1:111122223333:table/table_{index}") })
}

/// One resource carrying an ARN and `count` readiness scopes.
fn scoped_resource(index: usize, count: usize) -> Value {
    let scopes: Vec<String> = (0..count)
        .map(|scope| {
            format!("arn:aws:route53-recovery-readiness::111122223333:cell/cell_{index}_{scope}")
        })
        .collect();
    json!({
        "resourceArn": format!("arn:aws:sqs:us-east-1:111122223333:queue_{index}"),
        "readinessScopes": scopes,
    })
}

/// One resource carrying the whole `DnsTargetResource` subtree (model:
/// `smithy_query(query="shape", shape="Resource,DNSTargetResource,TargetResource")`), so
/// a replacement is exercised on a deep entry and not only on a flat one.
fn dns_target_resource(index: usize) -> Value {
    json!({
        "componentId": format!("component_{index}"),
        "dnsTargetResource": {
            "domainName": format!("app-{index}.example.com"),
            "hostedZoneArn": "arn:aws:route53:::hostedzone/Z1EXAMPLE1",
            "recordSetId": format!("record_{index}"),
            "recordType": "A",
            "targetResource": {
                "nLBResource": {
                    "arn": format!("arn:aws:elasticloadbalancing:us-east-1:111122223333:loadbalancer/net/nlb-{index}/0123456789abcdef"),
                },
            },
        },
        "readinessScopes": [
            format!("arn:aws:route53-recovery-readiness::111122223333:recovery-group/group_{index}"),
        ],
    })
}

/// A resource list of `count` entries, cycling the three entry FORMS above so a row's
/// list is not uniform.
fn mixed_resources(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| match index % 3 {
                0 => arn_resource(index),
                1 => scoped_resource(index, 1 + index % 3),
                _ => dns_target_resource(index),
            })
            .collect(),
    )
}

/// Store a resource set and assert the create succeeded.
async fn seed(emulator: &Emulator, name: &str, kind: &str, resources: &Value, tags: Option<Value>) {
    let mut body = json!({
        "resourceSetName": name,
        "resourceSetType": kind,
        "resources": resources,
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let reply = create_resource_set(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected CreateResourceSet to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

/// Every row replaces a stored set's type and resource list, and the replacement is what
/// the update reports, what `GetResourceSet` reads back and what `ListResourceSets`
/// reports — while the ARN and the name stay the record's identity.
#[tokio::test]
async fn an_update_replaces_the_type_and_the_resource_list_everywhere_the_record_is_read() {
    // Rows differ along the resource count BEFORE and AFTER the update (growing,
    // shrinking, emptying, filling an empty set) and along which catalog entry the type
    // is moved to.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("empty to one", 0, 1),
        ("one to empty", 1, 0),
        ("one to one", 1, 1),
        ("one to several", 1, 5),
        ("several to one", 6, 1),
        ("several to several", 4, 7),
        ("empty to empty", 0, 0),
        ("several to the stored ceiling", 3, MAX_RESOURCES_PER_SET),
        ("the stored ceiling to one", MAX_RESOURCES_PER_SET, 1),
    ];

    let emulator = Emulator::new();
    for (index, (case, before, after)) in rows.into_iter().enumerate() {
        let name = format!("update_set_{index}");
        // The type before and the type after are different catalog entries, so the row
        // proves the type moved rather than happening to agree.
        let before_type = DOCUMENTED_RESOURCE_SET_TYPES[index % DOCUMENTED_RESOURCE_SET_TYPES.len()];
        let after_type =
            DOCUMENTED_RESOURCE_SET_TYPES[(index + 1) % DOCUMENTED_RESOURCE_SET_TYPES.len()];
        seed(&emulator, &name, before_type, &mixed_resources(before), None).await;

        let replacement = mixed_resources(after);
        let updated = update_resource_set(&emulator, &name, after_type, &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateResourceSet to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("resourceSetType").as_deref(),
            Some(after_type),
            "case {case:?}: expected the replacement type back, got {:?}",
            updated.string("resourceSetType")
        );
        assert_eq!(
            without_nulls(&resources_of(&updated.body)),
            without_nulls(&replacement),
            "case {case:?}: expected the replacement list back, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("resourceSetArn"),
            Some(resource_set_arn(&name)),
            "case {case:?}: expected the record's ARN to be unchanged, got {:?}",
            updated.string("resourceSetArn")
        );
        assert_eq!(
            updated.string("resourceSetName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the record's name to be unchanged, got {:?}",
            updated.string("resourceSetName")
        );

        // The replacement is PERSISTED: the reader sees exactly what the update reported.
        let fetched = get_resource_set(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the updated record to be readable, got {} / {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            without_nulls(&fetched.body),
            without_nulls(&updated.body),
            "case {case:?}: expected GetResourceSet to report the update, got {}",
            fetched.body
        );

        let entry = listed_entry(&emulator, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the record to be listed"));
        assert_eq!(
            entry.get("resourceSetType").and_then(Value::as_str),
            Some(after_type),
            "case {case:?}: expected the listing to report the replacement type, got {entry}"
        );
        assert_eq!(
            without_nulls(&resources_of(&entry)),
            without_nulls(&replacement),
            "case {case:?}: expected the listing to report the replacement list, got {entry}"
        );
    }
}

/// An update does not merge: whatever was stored is gone, member by member. Rows differ
/// in how the two lists OVERLAP, which is the way a merge would show itself.
#[tokio::test]
async fn an_update_replaces_rather_than_merges_the_resource_list() {
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a disjoint replacement",
            json!([arn_resource(1), arn_resource(2)]),
            json!([arn_resource(9)]),
        ),
        (
            "a replacement that keeps one entry",
            json!([arn_resource(1), arn_resource(2)]),
            json!([arn_resource(2)]),
        ),
        (
            "the same entries in the opposite order",
            json!([arn_resource(1), arn_resource(2), arn_resource(3)]),
            json!([arn_resource(3), arn_resource(2), arn_resource(1)]),
        ),
        (
            "an entry that loses its readiness scopes",
            json!([scoped_resource(4, 3)]),
            json!([arn_resource(4)]),
        ),
        (
            "an entry that gains readiness scopes",
            json!([arn_resource(5)]),
            json!([scoped_resource(5, MAX_READINESS_SCOPES_PER_RESOURCE)]),
        ),
        (
            "a flat entry replaced by a DNS target entry",
            json!([arn_resource(6)]),
            json!([dns_target_resource(6)]),
        ),
        (
            "a DNS target entry replaced by a flat entry",
            json!([dns_target_resource(7)]),
            json!([arn_resource(7)]),
        ),
        (
            "an emptied list",
            json!([arn_resource(8), dns_target_resource(8)]),
            json!([]),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, before, after)) in rows.into_iter().enumerate() {
        let name = format!("replace_set_{index}");
        seed(&emulator, &name, "AWS::DynamoDB::Table", &before, None).await;

        let updated = update_resource_set(&emulator, &name, "AWS::DynamoDB::Table", &after).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );

        let fetched = get_resource_set(&emulator, &name).await;
        let stored = without_nulls(&resources_of(&fetched.body));
        assert_eq!(
            stored,
            without_nulls(&after),
            "case {case:?}: expected the stored list to be exactly the replacement, got {stored}"
        );
        let stored_count = stored.as_array().map_or(0, Vec::len);
        let expected_count = after.as_array().map_or(0, Vec::len);
        assert_eq!(
            stored_count, expected_count,
            "case {case:?}: expected {expected_count} stored resources, got {stored_count}"
        );
    }
}

/// Updating is idempotent and repeatable: applying the same body twice leaves the same
/// record, and a chain of updates leaves the LAST one. Rows differ in how many updates
/// are applied in a row.
#[tokio::test]
async fn repeated_updates_leave_the_last_one_stored() {
    let emulator = Emulator::new();
    for rounds in [1usize, 2, 3, 5] {
        let name = format!("repeat_set_{rounds}");
        seed(
            &emulator,
            &name,
            "AWS::Lambda::Function",
            &mixed_resources(2),
            None,
        )
        .await;

        let mut last: Option<(String, Value)> = None;
        for round in 0..rounds {
            let kind = DOCUMENTED_RESOURCE_SET_TYPES[round % DOCUMENTED_RESOURCE_SET_TYPES.len()];
            let resources = mixed_resources(round + 1);
            // The same body is applied twice per round, so an update that accumulated
            // state would show up as a difference between the two answers.
            let first = update_resource_set(&emulator, &name, kind, &resources).await;
            let again = update_resource_set(&emulator, &name, kind, &resources).await;
            assert_eq!(
                (first.status, again.status),
                (200, 200),
                "case {rounds:?} rounds, round {round}: expected both updates to answer HTTP 200, got {} / {}",
                first.status,
                again.status
            );
            assert_eq!(
                without_nulls(&again.body),
                without_nulls(&first.body),
                "case {rounds:?} rounds, round {round}: expected an idempotent update, got {} then {}",
                first.body,
                again.body
            );
            last = Some((kind.to_string(), resources));
        }

        let (expected_type, expected_resources) =
            last.expect("every row applies at least one update");
        let fetched = get_resource_set(&emulator, &name).await;
        assert_eq!(
            fetched.string("resourceSetType").as_deref(),
            Some(expected_type.as_str()),
            "case {rounds:?} rounds: expected the last type stored, got {:?}",
            fetched.string("resourceSetType")
        );
        assert_eq!(
            without_nulls(&resources_of(&fetched.body)),
            without_nulls(&expected_resources),
            "case {rounds:?} rounds: expected the last list stored, got {}",
            fetched.body
        );
    }
}

/// The tags a record carries are not part of an update's request, so an update leaves
/// them exactly as they were. Rows differ in the tag count.
#[tokio::test]
async fn an_update_leaves_the_records_tags_alone() {
    let emulator = Emulator::new();
    for tag_count in [0usize, 1, 2, 7] {
        let name = format!("tagged_set_{tag_count}");
        let tags: serde_json::Map<String, Value> = (0..tag_count)
            .map(|index| (format!("key_{index}"), json!(format!("value_{index}"))))
            .collect();
        let tags = Value::Object(tags);
        seed(
            &emulator,
            &name,
            "AWS::SQS::Queue",
            &mixed_resources(2),
            Some(tags.clone()),
        )
        .await;

        let before = get_resource_set(&emulator, &name).await;
        let updated =
            update_resource_set(&emulator, &name, "AWS::SNS::Topic", &mixed_resources(3)).await;
        assert_eq!(
            updated.status, 200,
            "case {tag_count} tags: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );

        let expected = if tag_count == 0 {
            // An untagged record reports no tags member at all.
            None
        } else {
            Some(tags.clone())
        };
        assert_eq!(
            updated.field("tags").cloned(),
            expected,
            "case {tag_count} tags: expected the stored tags reported unchanged, got {:?}",
            updated.field("tags")
        );
        assert_eq!(
            get_resource_set(&emulator, &name).await.field("tags").cloned(),
            before.field("tags").cloned(),
            "case {tag_count} tags: expected the stored tags untouched by the update"
        );
    }
}

/// An update addressed to a set no record holds is refused 404, and nothing is stored
/// for it. Rows differ in the FORM of the missing name.
#[tokio::test]
async fn updating_a_set_that_does_not_exist_is_refused_and_stores_nothing() {
    let rows: Vec<(&str, String)> = vec![
        ("a plain absent name", "absent_set".to_string()),
        ("a name that differs only in case", "PRESENT_SET".to_string()),
        (
            "a name that is a prefix of a stored one",
            "present".to_string(),
        ),
        (
            "a name with a trailing underscore",
            "present_set_".to_string(),
        ),
        // `__stringMax64PatternAAZAZ09Z`, the shape a reported name targets, is
        // `length 0..64` (model: `smithy_query(query="shape",
        // shape="__stringMax64PatternAAZAZ09Z")`): a name AT the published bound is
        // still just a name no record holds.
        ("a 64-character name", "n".repeat(64)),
    ];

    let emulator = Emulator::new();
    seed(
        &emulator,
        "present_set",
        "AWS::DynamoDB::Table",
        &mixed_resources(2),
        None,
    )
    .await;

    for (case, missing) in rows {
        let missing = missing.as_str();
        let refused = update_resource_set(
            &emulator,
            missing,
            "AWS::DynamoDB::Table",
            &mixed_resources(1),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?} ({missing:?}): expected HTTP 404, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?} ({missing:?}): expected ResourceNotFoundException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.string("message").as_deref(),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?} ({missing:?}): expected the service's not-found sentence, got {:?}",
            refused.string("message")
        );
        assert_eq!(
            get_resource_set(&emulator, missing).await.status,
            404,
            "case {case:?} ({missing:?}): a refused update must not create the record"
        );
        assert!(
            listed_entry(&emulator, missing).await.is_none(),
            "case {case:?} ({missing:?}): a refused update must not be listed"
        );
    }

    // The set that DOES exist is untouched by every refusal above.
    let survivor = get_resource_set(&emulator, "present_set").await;
    assert_eq!(
        without_nulls(&resources_of(&survivor.body)),
        without_nulls(&mixed_resources(2)),
        "case {:?}: expected the stored record untouched by the refusals, got {}",
        "survivor",
        survivor.body
    );
}

/// An update is held to the rules a create is held to, and a refused update leaves the
/// previous record exactly as it was. Rows differ in which rule the body breaks.
#[tokio::test]
async fn a_refused_update_leaves_the_previous_record_intact() {
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a real AWS type outside the audited catalog",
            "AWS::S3::Bucket",
            json!([arn_resource(1)]),
        ),
        (
            "an invented type inside the member pattern",
            "AWS::Foo::Bar",
            json!([arn_resource(1)]),
        ),
        (
            "a catalog entry mis-cased",
            "AWS::Dynamodb::Table",
            json!([arn_resource(1)]),
        ),
        (
            "one resource past the stored ceiling",
            "AWS::DynamoDB::Table",
            mixed_resources(MAX_RESOURCES_PER_SET + 1),
        ),
        (
            "one readiness scope past the stored ceiling",
            "AWS::DynamoDB::Table",
            json!([scoped_resource(1, MAX_READINESS_SCOPES_PER_RESOURCE + 1)]),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, kind, resources)) in rows.into_iter().enumerate() {
        let name = format!("refused_set_{index}");
        let original = mixed_resources(2);
        seed(&emulator, &name, "AWS::Lambda::Function", &original, None).await;

        let refused = update_resource_set(&emulator, &name, kind, &resources).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );

        let fetched = get_resource_set(&emulator, &name).await;
        assert_eq!(
            fetched.string("resourceSetType").as_deref(),
            Some("AWS::Lambda::Function"),
            "case {case:?}: expected the previous type to survive the refusal, got {:?}",
            fetched.string("resourceSetType")
        );
        assert_eq!(
            without_nulls(&resources_of(&fetched.body)),
            without_nulls(&original),
            "case {case:?}: expected the previous list to survive the refusal, got {}",
            fetched.body
        );
    }
}
