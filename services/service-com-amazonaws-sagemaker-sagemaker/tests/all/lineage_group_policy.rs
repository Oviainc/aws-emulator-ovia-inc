// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's lineage-group policy reader
//! (`GetLineageGroupPolicy`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! SageMaker models no `CreateLineageGroup` and no call that attaches a lineage-group
//! resource policy (a lineage group is shared with another account by a resource share,
//! not by a call on this service), so the two writes these suites need reach the shared
//! store directly: the crate's `ensure_lineage_group` hook, through the shared
//! `materialise_lineage_group` harness, and [`attach_policy`] below for the policy itself.
//! One suite does the whole round trip over the wire instead (`CreateArtifact` ->
//! `GetLineageGroupPolicy`), and everything the tests assert goes over the wire.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the stored
//! document and the stored group ARN a read replays for every document shape, name shape
//! and Region; the empty document a group nobody has shared carries; the
//! `ResourceNotFound` refusal for a group the Region does not hold; the name-or-ARN
//! addressing the member's own shape admits; and that reading changes nothing a later
//! caller sees.
//!
//! The wire harness is the crate's shared one, from `inference_deployment_lifecycle`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state::{
    self, default_lineage_group_name, EMPTY_RESOURCE_POLICY,
};

use super::inference_deployment_lifecycle::{
    error_code, materialise_lineage_group as materialise, message, str_field, Emulator, ACCOUNT,
};

/// An instant for the materialised groups: the suites here assert about the policy and the
/// ARN, never about the stored timestamps (`lineage_group_reporting` covers those).
fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// The ARN the lineage group `name` carries in `region`, as this service mints it.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{name}")
}

/// Attach `policy` to the lineage group `name` already materialised in `region`, on the
/// shared store the handlers read through `state::service_state`.
///
/// This is a direct write because no SageMaker operation attaches a lineage-group resource
/// policy: the group's record is where one lives, so seeding it there is how a group that
/// carries a non-empty document is reached at all.
fn attach_policy(emulator: &Emulator, region: &str, name: &str, policy: &str) {
    let store = state::service_state(&*emulator.engine)
        .expect("the service is mounted on the shared in-memory engine");
    let mut guard = store.lock();
    let group = guard
        .lineage_groups
        .get_mut(&(region.to_string(), name.to_string()))
        .unwrap_or_else(|| panic!("case {region}/{name}: the group must be materialised first"));
    group.resource_policy = policy.to_string();
}

/// `GetLineageGroupPolicy` for `identifier`, signed for `region`.
async fn get_policy(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "GetLineageGroupPolicy",
            &json!({ "LineageGroupName": identifier }),
        )
        .await
}

/// RULE: `GetLineageGroupPolicy` answers the policy document the addressed lineage group's
/// record holds, byte for byte, beside that stored group's own ARN — whatever the document's
/// shape and size, the group's name and the Region it was materialised in.
#[tokio::test]
async fn reports_the_policy_document_the_addressed_group_holds() {
    // Rows differ along the axes the rule generalises over: the document's form and size
    // (`ResourcePolicyString` is `len 0..20480` per
    // `smithy_query(query="operation",operation="GetLineageGroupPolicy")`, so a document of
    // a few characters and a multi-kilobyte one are both inside the model), the shape of
    // the group's name, and the Region.
    let indented = "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
         \"Effect\": \"Allow\",\n      \"Principal\": { \"AWS\": \"111122223333\" },\n      \
         \"Action\": \"sagemaker:QueryLineage\"\n    }\n  ]\n}";
    let bulky = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Sid\":\"{}\"}}]}}",
        "s".repeat(20_000)
    );
    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "default-group-no-statements",
            "us-east-1",
            default_lineage_group_name("us-east-1"),
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "one-character-name-indented-document",
            "eu-west-2",
            "a".to_string(),
            indented.to_string(),
        ),
        (
            "digits-name-tab-separated-document",
            "ap-southeast-1",
            "123".to_string(),
            "{\"Version\":\"2012-10-17\",\t\"Statement\":[]}".to_string(),
        ),
        (
            "hyphenated-name-smallest-document",
            "us-west-2",
            "my-lineage-group-2".to_string(),
            EMPTY_RESOURCE_POLICY.to_string(),
        ),
        (
            "maximum-length-name-bulky-document",
            "eu-central-1",
            "g".repeat(120),
            bulky,
        ),
    ];

    let emulator = Emulator::new();
    for (case, region, name, policy) in &cases {
        assert_eq!(
            materialise(&emulator, region, name, at()),
            Some(group_arn(region, name)),
            "case {case}: the store must materialise the group and answer its ARN"
        );
        attach_policy(&emulator, region, name, policy);
    }

    for (case, region, name, policy) in &cases {
        let (status, body) = get_policy(&emulator, region, name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            str_field(&body, "ResourcePolicy"),
            *policy,
            "case {case}: ResourcePolicy must be the stored document byte for byte, \
             got {body:?}"
        );
        assert_eq!(
            str_field(&body, "LineageGroupArn"),
            group_arn(region, name),
            "case {case}: LineageGroupArn must be the stored group's ARN, got {body:?}"
        );

        // A read changes nothing a later caller sees: the second answer is identical, and
        // the group's own record still reports the same properties.
        let (again_status, again) = get_policy(&emulator, region, name).await;
        assert_eq!(
            (again_status, &again),
            (status, &body),
            "case {case}: a second read must answer the same document"
        );
        let (described_status, described) = emulator
            .call_in(
                region,
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": name }),
            )
            .await;
        assert_eq!(
            described_status, 200,
            "case {case}: the group must still be reportable, got {described_status} \
             with {described}"
        );
        assert_eq!(
            str_field(&described, "LineageGroupArn"),
            group_arn(region, name),
            "case {case}: the describe must report the same ARN, got {described:?}"
        );
    }

    // And each read answers the document of the group ASKED FOR, not whichever group the
    // store holds first.
    for (case, region, name, policy) in &cases {
        let (_, body) = get_policy(&emulator, region, name).await;
        assert_eq!(
            str_field(&body, "ResourcePolicy"),
            *policy,
            "case {case}: each read must answer its own group's document, got {body:?}"
        );
    }
}

/// RULE: a lineage group nobody has granted access to still carries a policy — the IAM
/// document with no statements, `state::EMPTY_RESOURCE_POLICY` — so the read answers HTTP
/// 200 with that document and the group's ARN rather than a refusal or an absent member,
/// whatever materialised the group.
#[tokio::test]
async fn a_group_nobody_has_shared_carries_the_empty_document() {
    // Rows: every name shape and Region a group is materialised under — the document is
    // the group's, not one particular group's.
    let seeded: Vec<(&str, String)> = vec![
        ("us-east-1", default_lineage_group_name("us-east-1")),
        ("eu-west-2", "a".to_string()),
        ("ap-southeast-1", "123".to_string()),
        ("us-west-2", "my-lineage-group-2".to_string()),
        ("eu-central-1", "g".repeat(120)),
    ];

    let emulator = Emulator::new();
    for (region, name) in &seeded {
        assert!(
            materialise(&emulator, region, name, at()).is_some(),
            "case {region}/{name}: the store must materialise the group"
        );
    }

    for (region, name) in &seeded {
        let case = format!("{region}/{name}");
        let (status, body) = get_policy(&emulator, region, name).await;
        assert_eq!(
            status, 200,
            "case {case}: a group nobody has shared is still read, got {status} with {body}"
        );
        assert_eq!(
            str_field(&body, "ResourcePolicy"),
            EMPTY_RESOURCE_POLICY,
            "case {case}: ResourcePolicy, got {body:?}"
        );
        assert_eq!(
            str_field(&body, "LineageGroupArn"),
            group_arn(region, name),
            "case {case}: LineageGroupArn, got {body:?}"
        );
    }

    // The whole round trip over the wire, for the groups the service materialises itself:
    // each implemented lineage-entity create puts its Region's default group in the store,
    // and that group's policy is readable straight after. Rows differ along the entity
    // kind, the Region and the request's shape.
    let created: Vec<(&str, &str, &str, Value)> = vec![
        (
            "artifact",
            "us-east-2",
            "CreateArtifact",
            json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/one" },
            }),
        ),
        (
            "action",
            "ca-central-1",
            "CreateAction",
            json!({
                "ActionName": "action-one",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "s3://bucket/action-one" },
            }),
        ),
        (
            "context",
            "ap-northeast-1",
            "CreateContext",
            json!({
                "ContextName": "context-one",
                "ContextType": "Endpoint",
                "Source": { "SourceUri": "s3://bucket/context-one" },
                "Properties": { "stage": "prod" },
            }),
        ),
    ];

    for (case, region, target, body) in &created {
        // A fresh emulator per row: the Region already holds its default group, so its
        // policy — the empty document, since nothing has shared it — reads before and
        // after the create alike.
        let emulator = Emulator::new();
        let group_name = default_lineage_group_name(region);
        let (before_status, before) = get_policy(&emulator, region, &group_name).await;
        assert_eq!(
            before_status, 200,
            "case {case}: the Region's default group must be readable from the start, got \
             {before_status} with {before}"
        );
        assert_eq!(
            str_field(&before, "ResourcePolicy"),
            EMPTY_RESOURCE_POLICY,
            "case {case}: ResourcePolicy before the create, got {before:?}"
        );

        let (created_status, created_body) = emulator.call_in(region, target, body).await;
        assert_eq!(
            created_status, 200,
            "case {case}: {target}, got {created_status} with {created_body}"
        );

        let (status, policy_body) = get_policy(&emulator, region, &group_name).await;
        assert_eq!(
            status, 200,
            "case {case}: the materialised group must be read, got {status} with \
             {policy_body}"
        );
        assert_eq!(
            str_field(&policy_body, "ResourcePolicy"),
            EMPTY_RESOURCE_POLICY,
            "case {case}: ResourcePolicy, got {policy_body:?}"
        );
        assert_eq!(
            str_field(&policy_body, "LineageGroupArn"),
            group_arn(region, &group_name),
            "case {case}: LineageGroupArn, got {policy_body:?}"
        );
    }
}

/// RULE: a lineage group the caller's Region does not hold is refused with HTTP 400
/// `ResourceNotFound` naming the ARN the requested name resolves to, and the read mints
/// nothing — a group nothing has materialised is still absent afterwards.
#[tokio::test]
async fn a_group_the_region_does_not_hold_is_refused_and_nothing_is_minted() {
    let emulator = Emulator::new();

    // On a store that has materialised nothing, every name is absent EXCEPT the Region's
    // own default group, which the account holds with no call creating it.
    for name in ["absent", "a"] {
        let (status, body) = get_policy(&emulator, "us-east-1", name).await;
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
            message(&body).contains(&group_arn("us-east-1", name)),
            "case empty-store/{name}: the refusal must name the group it could not find, \
             got {body:?}"
        );
        assert!(
            body.get("ResourcePolicy").is_none(),
            "case empty-store/{name}: a refusal carries no document, got {body:?}"
        );

        // A report never mints the resource it was asked about: the group is still absent
        // from the describe and from the listing.
        let (described_status, described) = emulator
            .call(
                "DescribeLineageGroup",
                &json!({ "LineageGroupName": name }),
            )
            .await;
        assert_eq!(
            described_status, 400,
            "case empty-store/{name}: the read must not materialise the group, got \
             {described_status} with {described}"
        );
        let listed = emulator.ok("ListLineageGroups", &json!({})).await;
        let listed_names: Vec<&str> = listed
            .get("LineageGroupSummaries")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter_map(|summary| {
                        summary.get("LineageGroupName").and_then(Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed_names,
            vec![default_lineage_group_name("us-east-1")],
            "case empty-store/{name}: the read must put no group in the listing beside \
             the Region's own, got {listed:?}"
        );
    }

    // A lineage group is REGIONAL, so a policy attached in one Region is not this
    // caller's in another: the document is answered at home, while every other Region
    // answers its OWN default group — which nothing has shared, so the empty document.
    let home = "us-east-1";
    let home_group = default_lineage_group_name(home);
    let policy = "{\"Version\":\"2012-10-17\",\"Statement\":[]}";
    // The document lives on the group's record, so the home Region's default group is
    // materialised in the store before one is attached to it.
    assert!(
        materialise(&emulator, home, &home_group, at()).is_some(),
        "the home Region must materialise the group"
    );
    attach_policy(&emulator, home, &home_group, policy);

    let (home_status, home_body) = get_policy(&emulator, home, &home_group).await;
    assert_eq!(
        home_status, 200,
        "case {home}: expected HTTP 200, got {home_status} with {home_body}"
    );
    assert_eq!(
        str_field(&home_body, "ResourcePolicy"),
        policy,
        "case {home}: ResourcePolicy, got {home_body:?}"
    );

    for region in ["eu-west-2", "ap-southeast-1", "us-west-2", "eu-central-1"] {
        let its_group = default_lineage_group_name(region);
        let (status, body) = get_policy(&emulator, region, &its_group).await;
        assert_eq!(
            status, 200,
            "case {region}: that Region's own default group must be read, got {status} \
             with {body}"
        );
        assert_eq!(
            str_field(&body, "ResourcePolicy"),
            EMPTY_RESOURCE_POLICY,
            "case {region}: a document attached in {home} is not this Region's, got {body:?}"
        );
        assert_eq!(
            str_field(&body, "LineageGroupArn"),
            group_arn(region, &its_group),
            "case {region}: the ARN must name THIS Region's group, got {body:?}"
        );
    }
}

/// RULE: `LineageGroupName` is modelled `LineageGroupNameOrArn`
/// (`^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group\/)?([a-zA-Z0-9](-*[a-zA-Z0-9]){0,119})$`,
/// per `smithy_query(query="operation",operation="GetLineageGroupPolicy")`), so the group's
/// own ARN addresses the same group as its bare name — while an ARN naming another
/// partition, Region or account addresses no group of this account's and is refused.
#[tokio::test]
async fn a_group_is_addressable_by_its_name_or_by_its_own_arn() {
    let region = "us-east-1";
    let policy = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\"}]}";

    // Rows: the name shapes the member admits, each addressed both ways.
    let names = vec![
        default_lineage_group_name(region),
        "a".to_string(),
        "123".to_string(),
        "my-lineage-group-2".to_string(),
        "g".repeat(120),
    ];

    let emulator = Emulator::new();
    for name in &names {
        assert!(
            materialise(&emulator, region, name, at()).is_some(),
            "case {name}: the store must materialise the group"
        );
        attach_policy(&emulator, region, name, policy);
    }

    for name in &names {
        let (by_name_status, by_name) = get_policy(&emulator, region, name).await;
        assert_eq!(
            by_name_status, 200,
            "case by-name/{name}: expected HTTP 200, got {by_name_status} with {by_name}"
        );
        let (by_arn_status, by_arn) = get_policy(&emulator, region, &group_arn(region, name)).await;
        assert_eq!(
            by_arn_status, 200,
            "case by-arn/{name}: expected HTTP 200, got {by_arn_status} with {by_arn}"
        );
        assert_eq!(
            by_arn, by_name,
            "case {name}: the ARN must address the same group as the bare name"
        );
    }

    // An ARN this service did not mint names no group here, whatever local record shares
    // its trailing name: another Region, another account, another partition.
    let name = default_lineage_group_name(region);
    let name = name.as_str();
    let foreign: Vec<(&str, String)> = vec![
        ("another-region", group_arn("eu-west-2", name)),
        (
            "another-account",
            format!("arn:aws:sagemaker:{region}:111122223333:lineage-group/{name}"),
        ),
        (
            "another-partition",
            format!("arn:aws-cn:sagemaker:{region}:{ACCOUNT}:lineage-group/{name}"),
        ),
    ];
    for (case, identifier) in &foreign {
        let (status, body) = get_policy(&emulator, region, identifier).await;
        assert_eq!(
            status, 400,
            "case {case}: an ARN this service did not mint names no group, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case}: error code, got {body:?}"
        );
        assert!(
            message(&body).contains(identifier),
            "case {case}: the refusal must name the identifier the request sent, \
             got {body:?}"
        );
    }

    // The group itself still answers, so the refusals above are about the ARN and not
    // about the operation.
    let (status, body) = get_policy(&emulator, region, name).await;
    assert_eq!(
        status, 200,
        "the group must still answer its document, got {status} with {body}"
    );
}

/// RULE: a `LineageGroupName` outside the modelled constraints (length 1..256 and the
/// `LineageGroupNameOrArn` pattern) is refused at the boundary with HTTP 400
/// `ValidationException`, before the handler resolves anything — and a name AT the
/// published bound reaches the lookup, which answers it from the store.
#[tokio::test]
async fn an_identifier_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let emulator = Emulator::new();

    // The name portion of the pattern admits at most 120 characters, so one past it is
    // outside the model even though the member's length maximum is 256.
    let one_past_the_name_maximum = "g".repeat(121);
    let refused: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("one-past-the-name-maximum", one_past_the_name_maximum),
        ("leading-hyphen", "-group".to_string()),
        ("trailing-hyphen", "group-".to_string()),
        ("underscore", "my_group".to_string()),
        ("space", "my group".to_string()),
        (
            "another-resource-kind",
            format!(
                "arn:aws:sagemaker:us-east-1:{ACCOUNT}:context/{}",
                default_lineage_group_name("us-east-1")
            ),
        ),
    ];

    for (case, identifier) in &refused {
        let (status, body) = get_policy(&emulator, "us-east-1", identifier).await;
        assert_eq!(
            status, 400,
            "case {case}: an identifier outside the modelled constraints must be refused, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: an identifier outside the modelled constraints is refused by \
             the boundary validator, never with the scaffold's code, got {body:?}"
        );
    }

    // Each published bound of the name portion is INSIDE the constraint, so both reach the
    // lookup and are answered by the store rather than by the boundary.
    let policy = "{\"Version\":\"2012-10-17\",\"Statement\":[]}";
    for name in ["g".to_string(), "g".repeat(120)] {
        let case = format!("at-bound/{}", name.len());
        let (status, body) = get_policy(&emulator, "us-east-1", &name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the store's refusal, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {case}: a name at the bound must reach the lookup, got {body:?}"
        );

        assert!(
            materialise(&emulator, "us-east-1", &name, at()).is_some(),
            "case {case}: the store must accept the group"
        );
        attach_policy(&emulator, "us-east-1", &name, policy);
        let (ok_status, ok_body) = get_policy(&emulator, "us-east-1", &name).await;
        assert_eq!(
            ok_status, 200,
            "case {case}: expected HTTP 200, got {ok_status} with {ok_body}"
        );
        assert_eq!(
            str_field(&ok_body, "ResourcePolicy"),
            policy,
            "case {case}: ResourcePolicy, got {ok_body:?}"
        );
    }
}
