// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's lineage **context** entity:
//! `CreateContext` -> `DescribeContext` -> `UpdateContext` -> `DeleteContext`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` already defines for this crate.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! optional member; name-or-ARN resolution; the modelled `ResourceNotFound` for every
//! reader and mutator of an absent context; and the documented overwrite/remove
//! semantics of `UpdateContext`'s `Properties` and `PropertiesToRemove`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator};

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Spacing between a create and the update that follows it, so `LastModifiedTime`
/// moving past `CreationTime` is unambiguous at the resolution the wire carries.
const EDIT_SPACING: Duration = Duration::from_millis(5);

/// A minimal well-formed `CreateContext` body: the three required members.
fn create_body(name: &str, context_type: &str, source_uri: &str) -> Value {
    json!({
        "ContextName": name,
        "ContextType": context_type,
        "Source": { "SourceUri": source_uri },
    })
}

/// `count` distinct property entries, as the wire map `LineageEntityParameters` carries.
fn properties(count: usize) -> Value {
    let map: BTreeMap<String, String> = (0..count)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    json!(map)
}

/// The timestamp member `member`, as the epoch-seconds number the JSON protocol carries.
fn instant(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a timestamp member {member} in {body}"))
}

/// RULE: `CreateContext` mints `arn:aws:sagemaker:<signed region>:<account>:context/<name>`
/// — whatever the name's shape and whatever region the caller signed for, and the
/// emulator default when the request is unsigned.
#[tokio::test]
async fn create_context_mints_an_arn_naming_the_signed_region_and_the_context_name() {
    // `ContextName` is len 1..120, pattern ^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,119}$
    // (smithy_query operation=CreateContext), so the rows span a single character, digits,
    // the hyphen and underscore separators the pattern allows, and the 120-character
    // upper bound.
    let at_the_upper_bound = format!("a{}", "b".repeat(119));
    let names = [
        "c".to_string(),
        "context1".to_string(),
        "demo-context-with-hyphens".to_string(),
        "demo_context_with_underscores".to_string(),
        "mix-ed_1".to_string(),
        at_the_upper_bound.clone(),
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in &names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let (status, body) = emulator
                .call_in(
                    region,
                    "CreateContext",
                    &create_body(name, "DemoType", "s3://bucket/key"),
                )
                .await;
            assert_eq!(status, 200, "case {case}: CreateContext, got {status} {body}");
            assert_eq!(
                str_field(&body, "ContextArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:context/{name}"),
                "case {case}: ContextArn, got {body:?}"
            );

            // The ARN the create answered is the one the reader replays.
            let described = emulator
                .call_in(region, "DescribeContext", &json!({ "ContextName": name }))
                .await;
            assert_eq!(
                str_field(&described.1, "ContextArn"),
                str_field(&body, "ContextArn"),
                "case {case}: DescribeContext must replay the minted ARN, got {:?}",
                described.1
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateContext",
            &create_body("unsigned-context", "DemoType", "s3://bucket/key"),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: CreateContext, got {status} {body}");
    assert_eq!(
        str_field(&body, "ContextArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:context/unsigned-context"),
        "case unsigned: ContextArn must fall back to the default region, got {body:?}"
    );
}

/// RULE: `DescribeContext` replays every member `CreateContext` stored — present members
/// and absent ones alike — and reports `CreationTime` and `LastModifiedTime` as the one
/// instant the create recorded.
#[tokio::test]
async fn describe_context_replays_exactly_what_create_context_stored() {
    // Rows differ along which optional members the request carries and along the size of
    // each: the source's optional type and id, description length (the modelled bound is
    // len 0..3072), property count (the modelled bound is len 0..30), context-type length
    // (String256, len 0..256) and tag count.
    let long_description = "d".repeat(3072);
    let long_type = "t".repeat(256);
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", json!({})),
        ("empty-properties", json!({ "Properties": {} })),
        (
            "full-source",
            json!({ "Source": {
                "SourceUri": "arn:aws:sagemaker:us-east-1:000000000000:endpoint/some-endpoint",
                "SourceType": "ARN",
                "SourceId": "some-endpoint",
            }}),
        ),
        ("description", json!({ "Description": "a lineage context" })),
        ("empty-description", json!({ "Description": "" })),
        ("description-at-the-upper-bound", json!({ "Description": long_description })),
        ("one-property", json!({ "Properties": properties(1) })),
        ("thirty-properties", json!({ "Properties": properties(30) })),
        ("context-type-at-the-upper-bound", json!({ "ContextType": long_type })),
        (
            "tags",
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }] }),
        ),
        (
            "every-optional-member",
            json!({
                "Description": "everything at once",
                "Properties": properties(3),
                "Source": { "SourceUri": "s3://bucket/key", "SourceType": "S3", "SourceId": "key" },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = create_body(case, "DemoType", "s3://bucket/key");
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateContext", &body).await;
        let described = emulator
            .ok("DescribeContext", &json!({ "ContextName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ContextName"),
            case,
            "case {case}: ContextName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ContextArn"),
            str_field(&created, "ContextArn"),
            "case {case}: DescribeContext must replay the ARN CreateContext minted"
        );

        // Every member the request carried must come back unchanged; `Tags` is stored but
        // is not part of `DescribeContext`'s modelled output, so it is excluded here.
        // `Properties` is the map the context HOLDS, so a create that named none reads
        // back as the empty map rather than as a missing member.
        for member in ["ContextType", "Source", "Description"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must round-trip, got {described:?}"
            );
        }
        assert_eq!(
            described.get("Properties"),
            Some(&body.get("Properties").cloned().unwrap_or_else(|| json!({}))),
            "case {case}: Properties must be the map the context holds, got {described:?}"
        );

        // A context nobody has updated reports one instant for both stamps.
        let created_at = instant(&described, "CreationTime");
        let modified_at = instant(&described, "LastModifiedTime");
        assert_eq!(
            modified_at, created_at,
            "case {case}: an untouched context must report LastModifiedTime \
             {modified_at} equal to CreationTime {created_at}"
        );

        // `CreatedBy`/`LastModifiedBy` name the IAM identity the request signed with; an
        // untouched context reports the creator in both.
        for member in ["CreatedBy", "LastModifiedBy"] {
            let identity = described
                .get(member)
                .unwrap_or_else(|| panic!("case {case}: {member}, got {described:?}"));
            assert_eq!(
                identity
                    .get("IamIdentity")
                    .and_then(|iam| iam.get("Arn"))
                    .and_then(Value::as_str),
                Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
                "case {case}: {member}.IamIdentity.Arn, got {identity:?}"
            );
        }
        // Every lineage entity belongs to its Region's one default lineage group
        // (`crate::state::DEFAULT_LINEAGE_GROUP`), whose ARN this member carries.
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            Some(format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/sagemaker-default-lineage-group").as_str()),
            "case {case}: LineageGroupArn must name the Region's default lineage group, \
             got {described:?}"
        );

        // A read changes nothing a later reader sees.
        let again = emulator
            .ok("DescribeContext", &json!({ "ContextName": case }))
            .await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeContext must answer the same, got {again:?}"
        );
    }
}

/// RULE: a `*NameOrArn` input member resolves the bare name and the resource's ARN to the
/// same stored record (`DescribeContext`'s `ContextName` is a `ContextNameOrArn`).
#[tokio::test]
async fn describe_context_accepts_the_name_or_the_context_arn() {
    // Rows span the name shapes the `ContextNameOrArn` pattern allows on either side of
    // the optional ARN prefix.
    let names = ["c", "ctx1", "demo-context", "demo_context", "mix-ed_1"];

    for region in ["us-east-1", "eu-central-1"] {
        let emulator = Emulator::new();
        for name in names {
            let case = format!("region={region} name={name}");
            let created = emulator
                .call_in(
                    region,
                    "CreateContext",
                    &create_body(name, "DemoType", "s3://bucket/key"),
                )
                .await;
            assert_eq!(created.0, 200, "case {case}: CreateContext {:?}", created.1);
            let arn = str_field(&created.1, "ContextArn");

            let by_name = emulator
                .call_in(region, "DescribeContext", &json!({ "ContextName": name }))
                .await;
            let by_arn = emulator
                .call_in(region, "DescribeContext", &json!({ "ContextName": arn }))
                .await;
            assert_eq!(by_name.0, 200, "case {case}: by name {:?}", by_name.1);
            assert_eq!(by_arn.0, 200, "case {case}: by ARN {:?}", by_arn.1);
            assert_eq!(
                by_arn.1, by_name.1,
                "case {case}: the ARN and the name must resolve to one record, got {:?}",
                by_arn.1
            );
        }
    }
}

/// RULE: every operation that reads or edits a context refuses one that was never
/// created with the modelled `ResourceNotFound` at HTTP 400 — never a 200 with empty
/// members, and never a 501.
#[tokio::test]
async fn an_absent_context_is_refused_with_the_modelled_resource_not_found() {
    // Rows: each of the three operations that take an existing context, over several
    // name shapes, including an ARN spelling for the reader that accepts one.
    let cases: Vec<(&str, Value)> = vec![
        ("DescribeContext", json!({ "ContextName": "never-created" })),
        ("DescribeContext", json!({ "ContextName": "other_name" })),
        (
            "DescribeContext",
            json!({ "ContextName": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:context/never-created") }),
        ),
        ("DeleteContext", json!({ "ContextName": "never-created" })),
        ("DeleteContext", json!({ "ContextName": "other_name" })),
        ("UpdateContext", json!({ "ContextName": "never-created" })),
        (
            "UpdateContext",
            json!({ "ContextName": "other_name", "Description": "d", "Properties": properties(2) }),
        ),
    ];

    for (target, body) in cases {
        let emulator = Emulator::new();
        // A context exists under a different name, so the refusal is about the named one.
        emulator
            .ok(
                "CreateContext",
                &create_body("a-real-context", "DemoType", "s3://bucket/key"),
            )
            .await;

        let (status, parsed) = emulator.call(target, &body).await;
        let case = format!("{target} {body}");
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: expected the modelled ResourceNotFound, got {parsed:?}"
        );

        // The refusal wrote nothing: the sibling context is untouched.
        let survivor = emulator
            .ok("DescribeContext", &json!({ "ContextName": "a-real-context" }))
            .await;
        assert_eq!(
            str_field(&survivor, "ContextName"),
            "a-real-context",
            "case {case}: the sibling context must survive, got {survivor:?}"
        );
    }
}

/// RULE: `ContextName` is unique per account and Region, so a create naming one already
/// in use is refused at HTTP 400 and the stored context is kept, never overwritten.
#[tokio::test]
async fn creating_a_context_name_already_in_use_is_refused_and_keeps_the_stored_context() {
    // Rows differ in how much of the second request differs from the first: everything,
    // only the type, only the description, or nothing at all.
    let cases: Vec<(&str, Value)> = vec![
        ("identical-request", json!({})),
        ("different-type", json!({ "ContextType": "OtherType" })),
        ("different-source", json!({ "Source": { "SourceUri": "s3://other/key" } })),
        ("added-description", json!({ "Description": "a second description" })),
        ("added-properties", json!({ "Properties": properties(4) })),
    ];

    for (case, overrides) in cases {
        let emulator = Emulator::new();
        let first = create_body("taken", "FirstType", "s3://bucket/first");
        let created = emulator.ok("CreateContext", &first).await;

        let mut second = first.clone();
        for (key, value) in overrides.as_object().expect("a JSON object of overrides") {
            second[key] = value.clone();
        }
        let (status, parsed) = emulator.call("CreateContext", &second).await;
        assert_eq!(
            status, 400,
            "case {case}: the second CreateContext must be refused, got {status} {parsed}"
        );

        // The first context is exactly as it was created.
        let described = emulator
            .ok("DescribeContext", &json!({ "ContextName": "taken" }))
            .await;
        assert_eq!(
            str_field(&described, "ContextArn"),
            str_field(&created, "ContextArn"),
            "case {case}: the stored context must keep its ARN, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ContextType"),
            "FirstType",
            "case {case}: the stored context must keep its type, got {described:?}"
        );
        assert_eq!(
            described.get("Source"),
            first.get("Source"),
            "case {case}: the stored context must keep its source, got {described:?}"
        );
        assert_eq!(
            described.get("Description"),
            None,
            "case {case}: the refused request must not add a description, got {described:?}"
        );
    }
}

/// RULE: `DeleteContext` removes exactly the named context, answers its stored ARN, and
/// leaves every sibling readable; a second delete of the same name is refused.
#[tokio::test]
async fn deleting_a_context_removes_only_that_entry() {
    // Rows: delete the first, a middle and the last of a seeded set, so position in the
    // store is not what decides the outcome.
    let seeded = ["alpha", "beta_two", "gamma-three"];

    for victim in seeded {
        let emulator = Emulator::new();
        let mut arns = BTreeMap::new();
        for name in seeded {
            let created = emulator
                .ok(
                    "CreateContext",
                    &create_body(name, "DemoType", "s3://bucket/key"),
                )
                .await;
            arns.insert(name, str_field(&created, "ContextArn"));
        }

        let deleted = emulator
            .ok("DeleteContext", &json!({ "ContextName": victim }))
            .await;
        assert_eq!(
            str_field(&deleted, "ContextArn"),
            arns[victim],
            "case {victim}: DeleteContext must answer the stored ARN, got {deleted:?}"
        );

        let (status, parsed) = emulator
            .call("DescribeContext", &json!({ "ContextName": victim }))
            .await;
        assert_eq!(
            status, 400,
            "case {victim}: the deleted context must be gone, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {victim}: expected ResourceNotFound after the delete, got {parsed:?}"
        );

        // A second delete is refused: the entry is really gone, not merely hidden.
        let (again_status, again) = emulator
            .call("DeleteContext", &json!({ "ContextName": victim }))
            .await;
        assert_eq!(
            again_status, 400,
            "case {victim}: a second DeleteContext must be refused, got {again_status} {again}"
        );

        for survivor in seeded.iter().filter(|name| **name != victim) {
            let described = emulator
                .ok("DescribeContext", &json!({ "ContextName": survivor }))
                .await;
            assert_eq!(
                str_field(&described, "ContextArn"),
                arns[*survivor],
                "case {victim}: sibling {survivor} must survive, got {described:?}"
            );
        }

        // The freed name can be created again, and the new context is a fresh record.
        let recreated = emulator
            .ok(
                "CreateContext",
                &create_body(victim, "OtherType", "s3://bucket/again"),
            )
            .await;
        assert_eq!(
            str_field(&recreated, "ContextArn"),
            arns[victim],
            "case {victim}: recreating the freed name must mint the same ARN, got {recreated:?}"
        );
    }
}

/// One row of the `UpdateContext` table: the case name, the `Properties` the create
/// stored, the editing members the update carries, and the `Properties` and
/// `Description` the following describe must report.
type UpdateCase = (
    &'static str,
    Value,
    Value,
    Option<Value>,
    Option<&'static str>,
);

/// RULE: `UpdateContext` replaces the description when the request carries one, replaces
/// the whole property map when it carries `Properties` ("the new list of properties.
/// Overwrites the current property list" — smithy_query doc=UpdateContextRequest$Properties),
/// then drops the keys named in `PropertiesToRemove`, and moves `LastModifiedTime` while
/// leaving `CreationTime` where the create put it.
#[tokio::test]
async fn update_context_overwrites_the_properties_and_removes_the_listed_keys() {
    // Rows differ along which editing members the request carries, the direction of the
    // property change (grow, shrink, replace, no-op) and the count — including the
    // modelled upper bound of 30 entries.
    let thirty = properties(30);
    let cases: Vec<UpdateCase> = vec![
        (
            "description-only-leaves-the-properties",
            json!({ "key0": "value0" }),
            json!({ "Description": "an edited description" }),
            Some(json!({ "key0": "value0" })),
            Some("an edited description"),
        ),
        (
            "properties-replace-the-whole-map",
            json!({ "key0": "value0", "key1": "value1" }),
            json!({ "Properties": { "fresh": "1" } }),
            Some(json!({ "fresh": "1" })),
            Some("the original description"),
        ),
        (
            "properties-to-remove-drops-one-key",
            json!({ "key0": "value0", "key1": "value1" }),
            json!({ "PropertiesToRemove": ["key0"] }),
            Some(json!({ "key1": "value1" })),
            Some("the original description"),
        ),
        (
            "properties-to-remove-can-empty-the-map",
            json!({ "key0": "value0" }),
            json!({ "PropertiesToRemove": ["key0"] }),
            Some(json!({})),
            Some("the original description"),
        ),
        (
            "removing-from-a-context-that-has-no-properties-leaves-it-empty",
            json!({}),
            json!({ "PropertiesToRemove": ["key0"] }),
            Some(json!({})),
            Some("the original description"),
        ),
        (
            "removing-a-key-that-is-not-stored-changes-nothing",
            json!({ "key0": "value0" }),
            json!({ "PropertiesToRemove": ["absent"] }),
            Some(json!({ "key0": "value0" })),
            Some("the original description"),
        ),
        (
            "the-overwrite-happens-before-the-removal",
            json!({ "key0": "value0" }),
            json!({ "Properties": { "a": "1", "b": "2" }, "PropertiesToRemove": ["a"] }),
            Some(json!({ "b": "2" })),
            Some("the original description"),
        ),
        (
            "an-update-carrying-no-edit-changes-no-value",
            json!({}),
            json!({}),
            Some(json!({})),
            Some("the original description"),
        ),
        (
            "an-update-carrying-no-edit-keeps-the-stored-properties",
            json!({ "key0": "value0" }),
            json!({}),
            Some(json!({ "key0": "value0" })),
            Some("the original description"),
        ),
        (
            "properties-at-the-modelled-upper-bound",
            json!({}),
            json!({ "Properties": thirty.clone() }),
            Some(thirty.clone()),
            Some("the original description"),
        ),
        (
            "an-empty-description-is-an-edit",
            json!({ "key0": "value0" }),
            json!({ "Description": "" }),
            Some(json!({ "key0": "value0" })),
            Some(""),
        ),
    ];

    for (case, initial, edit, expected_properties, expected_description) in cases {
        let emulator = Emulator::new();
        let mut body = create_body(case, "DemoType", "s3://bucket/key");
        body["Description"] = json!("the original description");
        if initial.as_object().is_some_and(|map| !map.is_empty()) {
            body["Properties"] = initial;
        }
        let created = emulator.ok("CreateContext", &body).await;
        let before = emulator
            .ok("DescribeContext", &json!({ "ContextName": case }))
            .await;

        // Spaced so the stamp the update moves is distinguishable from the create's.
        tokio::time::sleep(EDIT_SPACING).await;

        let mut update = json!({ "ContextName": case });
        for (key, value) in edit.as_object().expect("a JSON object of edit members") {
            update[key] = value.clone();
        }
        let updated = emulator.ok("UpdateContext", &update).await;
        assert_eq!(
            str_field(&updated, "ContextArn"),
            str_field(&created, "ContextArn"),
            "case {case}: UpdateContext must answer the stored ARN, got {updated:?}"
        );

        let after = emulator
            .ok("DescribeContext", &json!({ "ContextName": case }))
            .await;
        assert_eq!(
            after.get("Properties"),
            expected_properties.as_ref(),
            "case {case}: Properties after the update, got {after:?}"
        );
        assert_eq!(
            after.get("Description").and_then(Value::as_str),
            expected_description,
            "case {case}: Description after the update, got {after:?}"
        );
        // The members no editing member names are untouched.
        for member in ["ContextName", "ContextArn", "ContextType", "Source"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must not move on an update, got {after:?}"
            );
        }
        assert_eq!(
            instant(&after, "CreationTime"),
            instant(&before, "CreationTime"),
            "case {case}: CreationTime must not move, got {after:?}"
        );
        let created_at = instant(&after, "CreationTime");
        let modified_at = instant(&after, "LastModifiedTime");
        assert!(
            modified_at > created_at,
            "case {case}: LastModifiedTime {modified_at} must move past CreationTime \
             {created_at} on an update, got {after:?}"
        );
    }
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary with
/// HTTP 400, before any state is written.
#[tokio::test]
async fn context_requests_outside_the_modelled_constraints_are_refused() {
    // Rows: one past each published bound these operations state — `ContextName`
    // len 1..120 with pattern ^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,119}$, `ContextType`
    // (String256) len 0..256, `Description` len 0..3072, `Properties` len 0..30, and
    // `Source`/`ContextType`/`ContextName` all being required.
    let name_past_120 = "a".repeat(121);
    let type_past_256 = "t".repeat(257);
    let description_past_3072 = "d".repeat(3073);
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "context-name-past-120",
            "CreateContext",
            create_body(&name_past_120, "DemoType", "s3://bucket/key"),
        ),
        (
            "context-name-with-an-illegal-character",
            "CreateContext",
            create_body("not a context name", "DemoType", "s3://bucket/key"),
        ),
        (
            "empty-context-name",
            "CreateContext",
            create_body("", "DemoType", "s3://bucket/key"),
        ),
        (
            "context-type-past-256",
            "CreateContext",
            create_body("ok-name", &type_past_256, "s3://bucket/key"),
        ),
        (
            "missing-source",
            "CreateContext",
            json!({ "ContextName": "ok-name", "ContextType": "DemoType" }),
        ),
        (
            "missing-context-type",
            "CreateContext",
            json!({ "ContextName": "ok-name", "Source": { "SourceUri": "s3://bucket/key" } }),
        ),
        (
            "missing-source-uri",
            "CreateContext",
            json!({ "ContextName": "ok-name", "ContextType": "DemoType", "Source": {} }),
        ),
        (
            "description-past-3072",
            "CreateContext",
            json!({
                "ContextName": "ok-name",
                "ContextType": "DemoType",
                "Source": { "SourceUri": "s3://bucket/key" },
                "Description": description_past_3072,
            }),
        ),
        (
            "thirty-one-properties",
            "CreateContext",
            json!({
                "ContextName": "ok-name",
                "ContextType": "DemoType",
                "Source": { "SourceUri": "s3://bucket/key" },
                "Properties": properties(31),
            }),
        ),
        (
            "update-with-thirty-one-properties",
            "UpdateContext",
            json!({ "ContextName": "seeded-context", "Properties": properties(31) }),
        ),
        (
            "update-with-a-description-past-3072",
            "UpdateContext",
            json!({ "ContextName": "seeded-context", "Description": description_past_3072 }),
        ),
        (
            "describe-with-an-illegal-name",
            "DescribeContext",
            json!({ "ContextName": "not a context name" }),
        ),
        (
            "delete-with-an-arn-its-member-does-not-admit",
            "DeleteContext",
            json!({ "ContextName": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:context/seeded-context") }),
        ),
    ];

    for (case, target, body) in cases {
        let emulator = Emulator::new();
        // A context already exists, so a refusal below is about the violated constraint
        // and not about a resource that was never created.
        emulator
            .ok(
                "CreateContext",
                &create_body("seeded-context", "DemoType", "s3://bucket/key"),
            )
            .await;

        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: {target} must be refused by the boundary validator, got {parsed:?}"
        );

        // Nothing above reached state: the name it tried to use is still absent, and the
        // context that did exist is untouched.
        let (describe_status, described) = emulator
            .call("DescribeContext", &json!({ "ContextName": "ok-name" }))
            .await;
        assert_eq!(
            describe_status, 400,
            "case {case}: a refused request must write nothing, got {describe_status} {described}"
        );
        let survivor = emulator
            .ok("DescribeContext", &json!({ "ContextName": "seeded-context" }))
            .await;
        assert_eq!(
            survivor.get("Properties"),
            Some(&json!({})),
            "case {case}: the refused request must not edit the stored context, got {survivor:?}"
        );
        assert_eq!(
            str_field(&survivor, "ContextType"),
            "DemoType",
            "case {case}: the stored context must be unchanged, got {survivor:?}"
        );
    }
}

/// RULE: a context name identifies one context PER REGION — `ContextName` "must be unique
/// to your account in an Amazon Web Services Region" (smithy_query
/// doc=CreateContextRequest member=ContextName) — so the same name in two Regions is two
/// contexts, and an ARN naming a Region the name was never created in addresses neither.
#[tokio::test]
async fn a_context_name_is_unique_per_region_not_across_regions() {
    // One emulator for every row: the rule is about Regions inside one account's state.
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    // The same name is accepted in each Region, and each mints its own ARN. `ContextType`
    // carries the creating Region, so a read proves which record answered.
    for region in regions {
        let (status, body) = emulator
            .call_in(
                region,
                "CreateContext",
                &create_body("shared-name", region, "s3://bucket/key"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case region={region}: CreateContext must be accepted, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ContextArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:context/shared-name"),
            "case region={region}: ContextArn, got {body:?}"
        );
    }

    for region in regions {
        let (status, body) = emulator
            .call_in(region, "DescribeContext", &json!({ "ContextName": "shared-name" }))
            .await;
        assert_eq!(
            status, 200,
            "case region={region}: DescribeContext, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ContextType"),
            region,
            "case region={region}: the Region's own context must answer, got {body:?}"
        );
        assert_eq!(
            str_field(&body, "ContextArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:context/shared-name"),
            "case region={region}: ContextArn, got {body:?}"
        );
        assert_eq!(
            str_field(&body, "LineageGroupArn"),
            format!(
                "arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/sagemaker-default-lineage-group"
            ),
            "case region={region}: LineageGroupArn must name that Region's group, got {body:?}"
        );
    }

    // Within one Region the name is taken.
    let (status, parsed) = emulator
        .call_in(
            "us-east-1",
            "CreateContext",
            &create_body("shared-name", "second-try", "s3://bucket/key"),
        )
        .await;
    assert_eq!(
        status, 400,
        "case same-region-again: the name is taken, got {status} {parsed}"
    );

    // An ARN naming a Region the name was never created in addresses no context.
    let (missing_status, missing) = emulator
        .call_in(
            "us-east-1",
            "DescribeContext",
            &json!({ "ContextName": format!("arn:aws:sagemaker:us-west-2:{ACCOUNT}:context/shared-name") }),
        )
        .await;
    assert_eq!(
        missing_status, 400,
        "case foreign-region-arn: must be refused, got {missing_status} {missing}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFound",
        "case foreign-region-arn: expected ResourceNotFound, got {missing:?}"
    );

    // A delete in one Region leaves the other Regions' contexts standing.
    let (deleted_status, deleted) = emulator
        .call_in("eu-west-1", "DeleteContext", &json!({ "ContextName": "shared-name" }))
        .await;
    assert_eq!(
        deleted_status, 200,
        "case delete-one-region: got {deleted_status} {deleted}"
    );
    let (gone_status, gone) = emulator
        .call_in("eu-west-1", "DescribeContext", &json!({ "ContextName": "shared-name" }))
        .await;
    assert_eq!(
        gone_status, 400,
        "case delete-one-region: the deleted context must be gone, got {gone_status} {gone}"
    );
    for region in ["us-east-1", "ap-southeast-2"] {
        let (status, body) = emulator
            .call_in(region, "DescribeContext", &json!({ "ContextName": "shared-name" }))
            .await;
        assert_eq!(
            status, 200,
            "case delete-one-region: {region} must survive, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "ContextType"),
            region,
            "case delete-one-region: {region} must be unchanged, got {body:?}"
        );
    }
}

/// RULE: a context `CreateContext` stored is a lineage entity an association may point at,
/// at either end, while a `context/...` ARN nothing created names no entity and is refused
/// with the modelled `ResourceNotFound`.
///
/// `AssociationEntityArn`'s modelled pattern admits the `context` kind
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial-component|artifact|action|context)/`,
/// `smithy_query(query="shape",shape="AssociationEntityArn")`), so `AddAssociation` has to
/// decide a context end from the store `CreateContext` writes to, exactly as it does for
/// the artifact and action kinds.
#[tokio::test]
async fn a_stored_context_is_an_association_end_and_an_unstored_one_is_not() {
    // Rows differ along the Region, along which END of the edge the context sits at, and
    // along whether that context was ever created: the rule is about the store deciding
    // the end, so it holds at both ends and in every Region.
    for region in ["us-east-1", "eu-central-1"] {
        for context_at_source in [true, false] {
            for created in [true, false] {
                let emulator = Emulator::new();
                let case =
                    format!("region={region} context_at_source={context_at_source} created={created}");

                // The other end is always a stored artifact, so the context end alone
                // decides the outcome.
                let (artifact_status, artifact_body) = emulator
                    .call_in(
                        region,
                        "CreateArtifact",
                        &json!({
                            "ArtifactName": "association-partner",
                            "Source": { "SourceUri": "s3://bucket/partner.csv" },
                            "ArtifactType": "DataSet",
                        }),
                    )
                    .await;
                assert_eq!(
                    artifact_status, 200,
                    "case {case}: CreateArtifact, got {artifact_status} {artifact_body}"
                );
                let artifact = str_field(&artifact_body, "ArtifactArn");

                let context = if created {
                    let (status, body) = emulator
                        .call_in(
                            region,
                            "CreateContext",
                            &create_body("association-end", "Endpoint", "s3://bucket/endpoint"),
                        )
                        .await;
                    assert_eq!(status, 200, "case {case}: CreateContext, got {status} {body}");
                    str_field(&body, "ContextArn")
                } else {
                    // The shape this service mints, for a context no create ever stored.
                    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:context/never-created")
                };

                let (source, destination) = if context_at_source {
                    (context.clone(), artifact.clone())
                } else {
                    (artifact.clone(), context.clone())
                };
                let (status, body) = emulator
                    .call_in(
                        region,
                        "AddAssociation",
                        &json!({
                            "SourceArn": source,
                            "DestinationArn": destination,
                            "AssociationType": "ContributedTo",
                        }),
                    )
                    .await;

                if !created {
                    assert_eq!(
                        status, 400,
                        "case {case}: an unstored context end must be refused, got {status} {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "ResourceNotFound",
                        "case {case}: expected ResourceNotFound, got {body:?}"
                    );
                    continue;
                }

                assert_eq!(
                    status, 200,
                    "case {case}: a stored context must be an association end, got {status} {body}"
                );
                assert_eq!(
                    (str_field(&body, "SourceArn"), str_field(&body, "DestinationArn")),
                    (source.clone(), destination.clone()),
                    "case {case}: the response echoes both ends, got {body:?}"
                );

                // The edge is PERSISTED: the delete that releases it answers 200, which it
                // only does for an association the store holds.
                let (released_status, released) = emulator
                    .call_in(
                        region,
                        "DeleteAssociation",
                        &json!({ "SourceArn": source, "DestinationArn": destination }),
                    )
                    .await;
                assert_eq!(
                    released_status, 200,
                    "case {case}: the stored association must be releasable, got \
                     {released_status} {released}"
                );
            }
        }
    }
}
