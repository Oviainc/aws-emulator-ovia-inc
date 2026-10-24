// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's lineage-group reporting surface
//! (`DescribeLineageGroup`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! SageMaker models no `CreateLineageGroup`: the service materialises a lineage group
//! when the account's lineage entities put something in it, which in this crate is
//! `ComAmazonawsSagemakerSagemakerState::ensure_lineage_group` — the hook the
//! lineage-entity creates call. `CreateArtifact` is implemented, so one suite below does
//! the whole round trip over the wire (`CreateArtifact` -> `DescribeLineageGroup`); the
//! others materialise groups through that same state API, which is how they reach name
//! shapes, regions and stored instants no single create can produce.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! stored record a describe replays for every name shape, region and timestamp; the
//! not-found refusal for every name the store does not hold; the boundary refusal for a
//! name outside the modelled constraints; and the bound on the store.
//!
//! The wire harness is the crate's shared one, from `inference_deployment_lifecycle`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state::{
    DEFAULT_LINEAGE_GROUP, LINEAGE_GROUP_QUOTA,
};

use super::inference_deployment_lifecycle::{
    epoch_seconds, error_code, f64_field, materialise_lineage_group as materialise, message,
    str_field, Emulator, ACCOUNT,
};

/// `DescribeLineageGroup` for `name`, signed for `region`, requiring a 200.
async fn describe_ok(emulator: &Emulator, region: &str, name: &str) -> Value {
    let (status, body) = emulator
        .call_in(region, "DescribeLineageGroup", &json!({ "LineageGroupName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {region}/{name}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// RULE: `DescribeLineageGroup` reports the lineage-group record the store holds under
/// the requested name — its name, the ARN minted in the region the group was materialised
/// in, its display name and its stored creation and last-modified instants — whatever the
/// name's shape and the region, and reading it leaves the record unchanged.
#[tokio::test]
async fn describe_lineage_group_reports_the_stored_record() {
    // Rows differ along the axes the rule generalises over: the name's shape (the default
    // group AWS materialises, a one-character name, digits, hyphens and the 120-character
    // modelled maximum), the region the group was materialised in, and the stored
    // instant (including a sub-second component).
    let cases: Vec<(&str, String, Duration)> = vec![
        (
            "us-east-1",
            DEFAULT_LINEAGE_GROUP.to_string(),
            Duration::from_secs(1_700_000_000),
        ),
        (
            "eu-west-2",
            "a".to_string(),
            Duration::from_secs(1_700_000_001) + Duration::from_nanos(250_000_000),
        ),
        (
            "ap-southeast-1",
            "123".to_string(),
            Duration::from_secs(1_600_000_000),
        ),
        (
            "us-west-2",
            "my-lineage-group-2".to_string(),
            Duration::from_secs(1_500_000_000) + Duration::from_nanos(500_000_000),
        ),
        (
            "eu-central-1",
            "g".repeat(120),
            Duration::from_secs(1_400_000_000),
        ),
    ];

    let emulator = Emulator::new();
    for (region, name, offset) in &cases {
        let at = SystemTime::UNIX_EPOCH + *offset;
        assert_eq!(
            materialise(&emulator, region, name, at),
            Some(format!(
                "arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{name}"
            )),
            "case {region}/{name}: the store must materialise the group and answer its ARN"
        );
    }

    for (region, name, offset) in &cases {
        let at = SystemTime::UNIX_EPOCH + *offset;
        let case = format!("{region}/{name}");
        let described = describe_ok(&emulator, region, name).await;

        assert_eq!(
            str_field(&described, "LineageGroupName"),
            *name,
            "case {case}: LineageGroupName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "LineageGroupArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{name}"),
            "case {case}: LineageGroupArn, got {described:?}"
        );
        // Nothing in the API supplies a display name for a group the service materialises
        // itself, so the stored display name is the group's own name.
        assert_eq!(
            str_field(&described, "DisplayName"),
            *name,
            "case {case}: DisplayName, got {described:?}"
        );
        assert!(
            (f64_field(&described, "CreationTime") - epoch_seconds(at)).abs() < 1e-6,
            "case {case}: CreationTime must be the stored instant {}, got {described:?}",
            epoch_seconds(at)
        );
        assert!(
            (f64_field(&described, "LastModifiedTime") - epoch_seconds(at)).abs() < 1e-6,
            "case {case}: LastModifiedTime must be the stored instant {}, got {described:?}",
            epoch_seconds(at)
        );
        // Members the service has no value for stay ABSENT rather than being invented:
        // a materialised group has no description and no Studio user context behind it.
        for member in ["Description", "CreatedBy", "LastModifiedBy"] {
            assert!(
                described.get(member).is_none(),
                "case {case}: {member} must be absent, got {described:?}"
            );
        }

        // A read changes nothing a later caller sees: the second answer is identical.
        let again = describe_ok(&emulator, region, name).await;
        assert_eq!(
            again, described,
            "case {case}: a second describe must report the same record"
        );
    }

    // And the describe reports the group ASKED FOR, not whichever one is first in the
    // store: every row answers with its own name.
    let reported: Vec<String> = {
        let mut names = Vec::new();
        for (region, name, _) in &cases {
            names.push(str_field(
                &describe_ok(&emulator, region, name).await,
                "LineageGroupName",
            ));
        }
        names
    };
    let expected: Vec<String> = cases.iter().map(|(_, name, _)| name.clone()).collect();
    assert_eq!(
        reported, expected,
        "each describe must report the name it was asked about"
    );
}

/// RULE: a lineage group belongs to the Region the entity that materialised it was created
/// in — `state::lineage_group_arn`'s own doc calls it "the ARN of the Region's default
/// lineage group" — so a caller signing for another Region does not find it, and each
/// Region's group carries its own ARN and its own stored instants.
#[tokio::test]
async fn a_lineage_group_belongs_to_the_region_it_was_materialised_in() {
    let emulator = Emulator::new();
    let home = "us-east-1";
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert!(
        materialise(&emulator, home, DEFAULT_LINEAGE_GROUP, at).is_some(),
        "the home Region must materialise the group"
    );

    // Rows: every other Region the group was NOT materialised in, including an unsigned
    // request (which falls back to the emulator's default Region, the home one here is
    // us-east-1 — so that case is covered by the home read below instead).
    for region in ["eu-west-2", "ap-southeast-1", "us-west-2", "eu-central-1"] {
        let (status, body) = emulator
            .call_in(
                region,
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": DEFAULT_LINEAGE_GROUP }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {region}: a group materialised in {home} must not be reported in \
             {region}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {region}: error code, got {body:?}"
        );
    }

    // The home Region still reports it, with its own ARN.
    let described = describe_ok(&emulator, home, DEFAULT_LINEAGE_GROUP).await;
    assert_eq!(
        str_field(&described, "LineageGroupArn"),
        format!("arn:aws:sagemaker:{home}:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"),
        "case {home}: LineageGroupArn, got {described:?}"
    );

    // Materialising the same name in a second Region is a DIFFERENT group: its own ARN
    // and its own instants, and the home Region's record does not move.
    let elsewhere = "eu-west-2";
    let later = at + Duration::from_secs(3_600);
    assert_eq!(
        materialise(&emulator, elsewhere, DEFAULT_LINEAGE_GROUP, later),
        Some(format!(
            "arn:aws:sagemaker:{elsewhere}:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"
        )),
        "case {elsewhere}: a second Region materialises its own group"
    );
    let there = describe_ok(&emulator, elsewhere, DEFAULT_LINEAGE_GROUP).await;
    assert_eq!(
        str_field(&there, "LineageGroupArn"),
        format!("arn:aws:sagemaker:{elsewhere}:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"),
        "case {elsewhere}: LineageGroupArn, got {there:?}"
    );
    assert!(
        (f64_field(&there, "CreationTime") - epoch_seconds(later)).abs() < 1e-6,
        "case {elsewhere}: CreationTime must be this Region's own instant, got {there:?}"
    );
    assert_eq!(
        describe_ok(&emulator, home, DEFAULT_LINEAGE_GROUP).await,
        described,
        "case {home}: another Region's group must not change this one's record"
    );
}

/// RULE: creating a lineage entity materialises its Region's default lineage group, so
/// `DescribeLineageGroup` reports the group the entity's own `LineageGroupArn` names —
/// the whole round trip over the wire, through every implemented lineage-entity create.
#[tokio::test]
async fn creating_a_lineage_entity_makes_the_default_group_reportable() {
    // Rows differ along the entity kind (`CreateArtifact`, `CreateAction` and
    // `CreateContext`, the lineage-entity creates implemented today), the signed region
    // (so the group's ARN is checked per region) and the request's shape, since the group
    // is materialised by ANY lineage entity rather than by one particular request.
    //
    // Each row: case, region, target, the entity's ARN member and kind, the create body,
    // and a second create whose entity joins the same group.
    let cases: Vec<(&str, &str, &str, &str, &str, Value, Value)> = vec![
        (
            "artifact-minimal",
            "us-east-1",
            "CreateArtifact",
            "ArtifactArn",
            "artifact",
            json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/one" },
            }),
            json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/one-again" },
            }),
        ),
        (
            "artifact-named-with-properties",
            "eu-west-2",
            "CreateArtifact",
            "ArtifactArn",
            "artifact",
            json!({
                "ArtifactName": "artifact-two",
                "ArtifactType": "Model",
                "Source": { "SourceUri": "s3://bucket/two" },
                "Properties": { "framework": "xgboost" },
            }),
            json!({
                "ArtifactType": "Image",
                "Source": { "SourceUri": "s3://bucket/two-again" },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
        (
            "action-minimal",
            "ap-southeast-1",
            "CreateAction",
            "ActionArn",
            "action",
            json!({
                "ActionName": "action-one",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "s3://bucket/action-one" },
            }),
            json!({
                "ActionName": "action-two",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "s3://bucket/action-two" },
            }),
        ),
        (
            "action-with-properties",
            "us-west-2",
            "CreateAction",
            "ActionArn",
            "action",
            json!({
                "ActionName": "action-three",
                "ActionType": "Approval",
                "Source": { "SourceUri": "s3://bucket/action-three" },
                "Properties": { "approver": "ml-team" },
                "Description": "an approval step",
            }),
            json!({
                "ActionName": "action-four",
                "ActionType": "Approval",
                "Source": { "SourceUri": "s3://bucket/action-four" },
            }),
        ),
        (
            "context-minimal",
            "eu-central-1",
            "CreateContext",
            "ContextArn",
            "context",
            json!({
                "ContextName": "context-one",
                "ContextType": "DemoType",
                "Source": { "SourceUri": "s3://bucket/context-one" },
            }),
            json!({
                "ContextName": "context-two",
                "ContextType": "DemoType",
                "Source": { "SourceUri": "s3://bucket/context-two" },
            }),
        ),
        (
            "context-with-properties",
            "us-east-2",
            "CreateContext",
            "ContextArn",
            "context",
            json!({
                "ContextName": "context-three",
                "ContextType": "Endpoint",
                "Source": { "SourceUri": "s3://bucket/context-three" },
                "Properties": { "stage": "prod" },
                "Description": "a deployment stage",
            }),
            json!({
                "ContextName": "context-four",
                "ContextType": "Endpoint",
                "Source": { "SourceUri": "s3://bucket/context-four" },
            }),
        ),
    ];

    for (case, region, target, arn_member, kind, body, second_body) in cases {
        // A fresh emulator per row: the group must be materialised by THIS create, in the
        // region this request signed for.
        let emulator = Emulator::new();
        let (absent_status, absent) = emulator
            .call_in(
                region,
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": DEFAULT_LINEAGE_GROUP }),
            )
            .await;
        assert_eq!(
            absent_status, 400,
            "case {case}: before any lineage entity the group must be absent, \
             got {absent_status} with {absent}"
        );

        let (created_status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            created_status, 200,
            "case {case}: {target}, got {created_status} with {created}"
        );
        assert!(
            str_field(&created, arn_member)
                .starts_with(&format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/")),
            "case {case}: {arn_member}, got {created:?}"
        );

        let described = describe_ok(&emulator, region, DEFAULT_LINEAGE_GROUP).await;
        assert_eq!(
            str_field(&described, "LineageGroupName"),
            DEFAULT_LINEAGE_GROUP,
            "case {case}: LineageGroupName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "LineageGroupArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"),
            "case {case}: LineageGroupArn, got {described:?}"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be the instant the group was materialised, \
             got {described:?}"
        );

        // A second entity does not re-materialise the group: its record is unchanged.
        let (second_status, second) = emulator.call_in(region, target, &second_body).await;
        assert_eq!(
            second_status, 200,
            "case {case}: the second {target}, got {second_status} with {second}"
        );
        assert_eq!(
            describe_ok(&emulator, region, DEFAULT_LINEAGE_GROUP).await,
            described,
            "case {case}: a second lineage entity must leave the group's record unchanged"
        );

        // And only the default group exists: no entity put anything in another one.
        let (other_status, other) = emulator
            .call_in(
                region,
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": "some-other-group" }),
            )
            .await;
        assert_eq!(
            other_status, 400,
            "case {case}: no other group may appear, got {other_status} with {other}"
        );
        assert_eq!(
            error_code(&other),
            "ResourceNotFound",
            "case {case}: error code for another group, got {other:?}"
        );
    }
}

/// RULE: a lineage group the store does not hold is refused with HTTP 400
/// `ResourceNotFound` naming the ARN the requested name would have — never a 200 with
/// empty members, and never the scaffold's 501.
#[tokio::test]
async fn an_unmaterialised_lineage_group_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    // On a store that has materialised nothing, EVERY name is absent — including the
    // name AWS gives the group it materialises for an account that has lineage entities.
    for name in [DEFAULT_LINEAGE_GROUP, "absent", "a"] {
        let (status, body) = emulator
            .call("DescribeLineageGroup", &json!({ "LineageGroupName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case empty-store/{name}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case empty-store/{name}: error code, got {body:?}"
        );
        assert!(
            message(&body).contains(&format!("{ACCOUNT}:lineage-group/{name}")),
            "case empty-store/{name}: the refusal must name the group it could not find, \
             got {body:?}"
        );
    }

    // With one group materialised, every name that is not EXACTLY it is still absent:
    // the lookup is by the whole name, case-sensitively, not by prefix or suffix.
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(
        materialise(&emulator, "us-east-1", "lineage-alpha", at),
        Some(format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/lineage-alpha"
        )),
        "the store must materialise the group and answer its ARN"
    );
    for name in [
        "Lineage-Alpha",
        "LINEAGE-ALPHA",
        "lineage",
        "lineage-alpha2",
        "alpha",
        DEFAULT_LINEAGE_GROUP,
    ] {
        let (status, body) = emulator
            .call("DescribeLineageGroup", &json!({ "LineageGroupName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case near-miss/{name}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case near-miss/{name}: error code, got {body:?}"
        );
    }

    // The one name that IS in the store still answers, so the refusals above are about
    // the name and not about the operation.
    let described = describe_ok(&emulator, "us-east-1", "lineage-alpha").await;
    assert_eq!(
        str_field(&described, "LineageGroupName"),
        "lineage-alpha",
        "the materialised group must still be reported, got {described:?}"
    );
}

/// RULE: a `LineageGroupName` outside the modelled constraints (`ExperimentEntityName`,
/// length 1..120, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, as
/// `smithy_query(query="operation",operation="DescribeLineageGroup")` reports them) is
/// refused at the boundary with HTTP 400, before the handler resolves any name — and a
/// name AT each published bound is accepted through to the lookup.
#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let emulator = Emulator::new();

    let too_long = "g".repeat(121);
    let refused: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("one-past-the-length-maximum", too_long),
        ("leading-hyphen", "-group".to_string()),
        ("trailing-hyphen", "group-".to_string()),
        ("underscore", "my_group".to_string()),
        ("space", "my group".to_string()),
        ("slash", "my/group".to_string()),
        (
            "an-arn-rather-than-a-name",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/{DEFAULT_LINEAGE_GROUP}"),
        ),
    ];

    for (case, name) in &refused {
        let (status, body) = emulator
            .call("DescribeLineageGroup", &json!({ "LineageGroupName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a name outside the modelled constraints must be refused, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: a name outside the modelled constraints is refused by the \
             boundary validator, never by the store and never with the scaffold's \
             code, got {body:?}"
        );
    }

    // Each published length bound is inside the constraint, so both reach the lookup and
    // are answered by the store rather than by the boundary: absent before, present after.
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for name in ["g".to_string(), "g".repeat(120)] {
        let (status, body) = emulator
            .call("DescribeLineageGroup", &json!({ "LineageGroupName": &name }))
            .await;
        assert_eq!(
            status, 400,
            "case at-bound/{}: expected the store's refusal, got {status} with {body}",
            name.len()
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case at-bound/{}: a name at the bound must reach the lookup, got {body:?}",
            name.len()
        );

        assert!(
            materialise(&emulator, "us-east-1", &name, at).is_some(),
            "case at-bound/{}: the store must accept the group",
            name.len()
        );
        let described = describe_ok(&emulator, "us-east-1", &name).await;
        assert_eq!(
            str_field(&described, "LineageGroupName"),
            name,
            "case at-bound/{}: LineageGroupName, got {described:?}",
            name.len()
        );
    }
}

/// RULE: the lineage-group store is bounded, so materialising groups cannot grow it
/// without end — at the cap a new name is refused and stays unknown to the describe,
/// while every group already in the store is still reported.
#[tokio::test]
async fn the_lineage_group_store_is_bounded() {
    let emulator = Emulator::new();
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    for index in 0..LINEAGE_GROUP_QUOTA {
        let name = format!("group-{index}");
        assert!(
            materialise(&emulator, "us-east-1", &name, at).is_some(),
            "case {name}: the store must accept group {index} of {LINEAGE_GROUP_QUOTA}"
        );
    }

    // One past the cap: refused, and never materialised.
    assert!(
        materialise(&emulator, "us-east-1", "one-too-many", at).is_none(),
        "case one-too-many: the store must refuse a new group at the cap"
    );
    let (status, body) = emulator
        .call(
            "DescribeLineageGroup",
            &json!({ "LineageGroupName": "one-too-many" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one-too-many: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFound",
        "case one-too-many: error code, got {body:?}"
    );

    // A name already in the store is NOT materialised again — nothing changed, so there
    // is no new ARN to answer — and it is still reported.
    assert!(
        materialise(&emulator, "us-east-1", "group-0", at).is_none(),
        "case group-0: a group already in the store must not be materialised again"
    );
    let described = describe_ok(&emulator, "us-east-1", "group-0").await;
    assert_eq!(
        str_field(&described, "LineageGroupName"),
        "group-0",
        "case group-0: LineageGroupName, got {described:?}"
    );
}
