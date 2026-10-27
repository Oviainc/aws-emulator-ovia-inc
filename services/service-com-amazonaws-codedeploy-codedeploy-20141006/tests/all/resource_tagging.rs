// SPDX-License-Identifier: CC0-1.0

//! Tagging a CodeDeploy resource by its ARN, driven over the wire.
//!
//! RULE under test: `TagResource` reads its `ResourceArn` as the resource that
//! ARN identifies — an application, a deployment group or a deployment
//! configuration, by the names the ARN's resource part carries — and merges the
//! request's key/value tags onto that resource's own record, replacing the value
//! of a key it already carries rather than duplicating the key, so
//! `ListTagsForResource` reports exactly those tags; a value that is not in the
//! ARN format is refused with `InvalidArnException`, a well-formed ARN naming
//! anything else with `ArnNotSupportedException`, an ARN whose resource the
//! account does not hold with that resource kind's own `…DoesNotExistException`,
//! and a `Tags` list the service will not apply with `TagRequiredException` or
//! `InvalidTagsToAddException` — each BEFORE anything is written.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, seeded_group, Emulator};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state;

/// The region the harness's unsigned calls are served in, which is the one the
/// service builds an ARN with when a request carries no credential scope.
const REGION: &str = "us-east-1";

/// One row of the tagging table: the case name, the ARN of the resource it tags,
/// and the `(key, value)` tags it applies to it.
type TagCase<'a> = (&'a str, String, &'a [(&'a str, &'a str)]);

/// One row of the re-tagging table: the case name, the `(key, value)` tags the
/// call applies, and the tags the resource must report afterwards.
type RetagCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);

/// A `tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// The `$.message` of a failure body: the sentence the refusal carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// The `(key, value)` pairs `ListTagsForResource` reports for `resource_arn`, in
/// report order, requiring the read to succeed.
async fn tags_of(emulator: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let listed = emulator
        .ok("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    listed["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Key"].as_str().unwrap_or_default().to_string(),
                entry["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create a `Server` deployment configuration under `name`.
async fn create_config(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": name,
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
        )
        .await;
}

/// RULE: the tags a call applies are merged onto the resource its ARN names, and
/// `ListTagsForResource` reports exactly those tags — whatever the resource
/// kind, the number of tags or the form of a tag's value.
#[tokio::test]
async fn tags_applied_by_arn_read_back_from_the_resource() {
    let emulator = Emulator::new();
    // One resource of every taggable kind, each created through the API a client
    // uses, so the ARN under test names a record the store really holds.
    emulator
        .ok("CreateApplication", json!({"applicationName": "tagged-app"}))
        .await;
    let group = seeded_group(&emulator, "grouped-app", "Server").await;
    create_config(&emulator, "tagged-config").await;

    // Rows differ along the resource KIND the ARN names, the number of tags
    // applied, and the form of the tag values (ordinary, empty — the service
    // forbids an empty KEY only — and one carrying punctuation and spaces).
    let cases: &[TagCase<'_>] = &[
        (
            "application, one tag",
            state::application_arn(REGION, "tagged-app"),
            &[("Env", "prod")],
        ),
        (
            "deployment group, three tags",
            state::deployment_group_arn(REGION, "grouped-app", &group),
            &[("Env", "dev"), ("Owner", "ana"), ("Tier", "api")],
        ),
        (
            "deployment configuration, empty tag value",
            state::deployment_config_arn(REGION, "tagged-config"),
            &[("Marker", ""), ("Env", "staging")],
        ),
        (
            "predefined deployment configuration, punctuated value",
            state::deployment_config_arn(REGION, "CodeDeployDefault.OneAtATime"),
            &[("Owner", "team platform/eu, on-call")],
        ),
    ];

    for (case, resource_arn, tags) in cases {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": tags.iter().map(|(k, v)| tag(k, v)).collect::<Vec<_>>(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must succeed, got {body:?}"
        );

        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let reported = tags_of(&emulator, resource_arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the resource reports the tags it was given, got {reported:?}"
        );
    }

    // Tagging one resource tags no other: each ARN above still reports only its
    // own tags.
    let application = tags_of(&emulator, &state::application_arn(REGION, "tagged-app")).await;
    assert_eq!(
        application,
        vec![("Env".to_string(), "prod".to_string())],
        "case \"tags stay on their own resource\": expected only the application's own tag, got \
         {application:?}"
    );
    let untagged = tags_of(&emulator, &state::application_arn(REGION, "grouped-app")).await;
    assert!(
        untagged.is_empty(),
        "case \"an untagged resource\": an application whose GROUP was tagged carries no tags, \
         got {untagged:?}"
    );
}

/// RULE: an ARN identifies its resource by the names it carries, not by the
/// partition, region or account segments — a call naming the same resource
/// through any of those reaches the same record.
#[tokio::test]
async fn an_arn_identifies_its_resource_by_name() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "by-name"}))
        .await;

    // Rows differ along the ARN segments that are not part of the resource's
    // identity: the region, the partition, and the account.
    let cases: &[(&str, String)] = &[
        (
            "the emulator's own region",
            state::application_arn(REGION, "by-name"),
        ),
        (
            "another region",
            state::application_arn("eu-west-1", "by-name"),
        ),
        (
            "another partition",
            "arn:aws-cn:codedeploy:cn-north-1:000000000000:application:by-name".to_string(),
        ),
        (
            "another account",
            "arn:aws:codedeploy:us-east-1:111122223333:application:by-name".to_string(),
        ),
    ];

    for (case, resource_arn) in cases {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [tag("Reached", case)]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ARN names a stored application, got {body:?}"
        );
        let reported = tags_of(&emulator, resource_arn).await;
        assert_eq!(
            reported,
            vec![("Reached".to_string(), (*case).to_string())],
            "case {case:?}: every form reaches the same record, got {reported:?}"
        );
    }
}

/// RULE: a tag whose key the resource already carries REPLACES that key's value;
/// it never adds a second entry for the key.
#[tokio::test]
async fn re_tagging_a_key_replaces_its_value() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "retagged"}))
        .await;
    let resource_arn = state::application_arn(REGION, "retagged");

    // Rows are successive calls on ONE resource: a first tagging, a replacement
    // of one key, an added key, and a replacement by a tag carrying an empty
    // value.
    let cases: &[RetagCase<'_>] = &[
        (
            "first tagging",
            &[("Env", "dev"), ("Owner", "ana")],
            &[("Env", "dev"), ("Owner", "ana")],
        ),
        (
            "replace one key",
            &[("Env", "prod")],
            &[("Env", "prod"), ("Owner", "ana")],
        ),
        (
            "add a key",
            &[("Tier", "api")],
            &[("Env", "prod"), ("Owner", "ana"), ("Tier", "api")],
        ),
        (
            "replace with an empty value",
            &[("Owner", "")],
            &[("Env", "prod"), ("Owner", ""), ("Tier", "api")],
        ),
    ];

    for (case, applied, expected) in cases {
        emulator
            .ok(
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": applied.iter().map(|(k, v)| tag(k, v)).collect::<Vec<_>>(),
                }),
            )
            .await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let reported = tags_of(&emulator, &resource_arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a key is replaced, never duplicated, got {reported:?}"
        );
    }
}

/// RULE: a `ResourceArn` the service cannot resolve to a resource it tags is
/// refused with HTTP 400 and the modelled error for THAT failure — not in the
/// ARN format, naming a resource these operations do not act on, or naming a
/// resource the account does not hold — and nothing is written.
#[tokio::test]
async fn a_resource_arn_is_refused_by_what_is_wrong_with_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "present"}))
        .await;
    let group = seeded_group(&emulator, "present-group-app", "Server").await;
    let _ = &group;
    let good_tags = json!([tag("Env", "prod")]);

    // Rows differ along WHAT is wrong with the ARN: its form, the service or
    // resource type it names, and whether the resource it names exists. The
    // resource types are exercised one per modelled "does not exist" error. The
    // last column is what the refusal has to NAME — the ARN as sent where the
    // ARN itself is at fault, the resource's name where it is the resource that
    // is missing — so a caller can tell from the message what to change.
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "not an ARN at all",
            "tagged-app".to_string(),
            "InvalidArnException",
            "tagged-app".to_string(),
        ),
        (
            "too few ARN fields",
            "arn:aws:codedeploy:us-east-1:000000000000".to_string(),
            "InvalidArnException",
            "arn:aws:codedeploy:us-east-1:000000000000".to_string(),
        ),
        (
            "an empty partition",
            "arn::codedeploy:us-east-1:000000000000:application:present".to_string(),
            "InvalidArnException",
            "arn::codedeploy:us-east-1:000000000000:application:present".to_string(),
        ),
        (
            "an application ARN with no name",
            "arn:aws:codedeploy:us-east-1:000000000000:application:".to_string(),
            "InvalidArnException",
            "arn:aws:codedeploy:us-east-1:000000000000:application:".to_string(),
        ),
        (
            "a deployment group ARN with no group name",
            "arn:aws:codedeploy:us-east-1:000000000000:deploymentgroup:present".to_string(),
            "InvalidArnException",
            "arn:aws:codedeploy:us-east-1:000000000000:deploymentgroup:present".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:iam::000000000000:role/CodeDeployRole".to_string(),
            "ArnNotSupportedException",
            "arn:aws:iam::000000000000:role/CodeDeployRole".to_string(),
        ),
        (
            "an S3 bucket ARN",
            "arn:aws:s3:::deployment-bundles".to_string(),
            "ArnNotSupportedException",
            "arn:aws:s3:::deployment-bundles".to_string(),
        ),
        (
            "a CodeDeploy on-premises instance ARN",
            state::instance_arn(REGION, "host-01", "registration"),
            "ArnNotSupportedException",
            state::instance_arn(REGION, "host-01", "registration"),
        ),
        (
            "a CodeDeploy resource type that is not tagged here",
            "arn:aws:codedeploy:us-east-1:000000000000:deployment:d-ABCDEFGHI".to_string(),
            "ArnNotSupportedException",
            "arn:aws:codedeploy:us-east-1:000000000000:deployment:d-ABCDEFGHI".to_string(),
        ),
        (
            "an application that does not exist",
            state::application_arn(REGION, "absent-app"),
            "ApplicationDoesNotExistException",
            "absent-app".to_string(),
        ),
        (
            "a deployment group whose application does not exist",
            state::deployment_group_arn(REGION, "absent-app", "any-group"),
            "ApplicationDoesNotExistException",
            "absent-app".to_string(),
        ),
        (
            "a deployment group that does not exist",
            state::deployment_group_arn(REGION, "present-group-app", "absent-group"),
            "DeploymentGroupDoesNotExistException",
            "absent-group".to_string(),
        ),
        (
            "a deployment configuration that does not exist",
            state::deployment_config_arn(REGION, "absent-config"),
            "DeploymentConfigDoesNotExistException",
            "absent-config".to_string(),
        ),
    ];

    for (case, resource_arn, expected, named) in cases {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": good_tags}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: every one of these is a 400 client error, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
        let message = message(case, &body);
        assert!(
            message.contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {message:?}"
        );

        // The same ARN is refused by the READER with the same code wherever the
        // failure is about the ARN itself; a resource the account does not hold
        // carries no tags, which the reader models no error for.
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
            .await;
        let reader_expected = match expected {
            "InvalidArnException" | "ArnNotSupportedException" => expected,
            _ => "",
        };
        if reader_expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: the reader models no does-not-exist error, got {body:?}"
            );
            assert_eq!(
                body["Tags"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: a resource the account does not hold carries no tags, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the reader refuses the same ARN, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                reader_expected,
                "case {case:?}: expected {reader_expected} from the reader, got {body:?}"
            );
        }
    }

    // None of the refusals above tagged the one application that does exist.
    let reported = tags_of(&emulator, &state::application_arn(REGION, "present")).await;
    assert!(
        reported.is_empty(),
        "a refused request writes nothing, got {reported:?}"
    );
}

/// RULE: a `Tags` list the service will not apply is refused with the modelled
/// error for that failure — no tag at all is `TagRequiredException`, a tag
/// missing a key or a value and a list that would take the resource past the tag
/// bound are `InvalidTagsToAddException` — and the resource is left as it was.
#[tokio::test]
async fn a_tags_list_is_refused_by_what_is_wrong_with_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "tag-gated"}))
        .await;
    let resource_arn = state::application_arn(REGION, "tag-gated");

    // The bound this service enforces, read from the service itself rather than
    // restated here (`crate::state::MAX_TAGS_PER_RESOURCE`).
    let bound = state::MAX_TAGS_PER_RESOURCE;
    let past_bound: Vec<Value> = (0..=bound)
        .map(|index| tag(&format!("Key{index}"), "v"))
        .collect();

    let cases: Vec<(&str, Value, &str)> = vec![
        ("no tags", json!([]), "TagRequiredException"),
        (
            "a tag with neither key nor value",
            json!([{}]),
            "InvalidTagsToAddException",
        ),
        (
            "a value-only tag",
            json!([{"Value": "prod"}]),
            "InvalidTagsToAddException",
        ),
        (
            "a key-only tag",
            json!([{"Key": "Env"}]),
            "InvalidTagsToAddException",
        ),
        (
            "a tag with an empty key",
            json!([{"Key": "", "Value": "prod"}]),
            "InvalidTagsToAddException",
        ),
        (
            "one tag more than a resource may carry",
            Value::Array(past_bound),
            "InvalidTagsToAddException",
        ),
    ];

    for (case, tags, expected) in cases {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": tags}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: every one of these is a 400 client error, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
        let reported = tags_of(&emulator, &resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused call tags nothing, got {reported:?}"
        );
    }

    // AT the bound the call is accepted, and the resource carries every tag.
    let at_bound: Vec<Value> = (0..bound)
        .map(|index| tag(&format!("Key{index}"), &format!("value-{index}")))
        .collect();
    emulator
        .ok(
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": Value::Array(at_bound)}),
        )
        .await;
    let reported = tags_of(&emulator, &resource_arn).await;
    assert_eq!(
        reported.len(),
        bound,
        "case \"at the bound\": the resource carries every tag, got {reported:?}"
    );

    // Replacing an existing key at the bound adds no key, so it is accepted;
    // one NEW key past the bound is refused and changes nothing.
    emulator
        .ok(
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [tag("Key0", "replaced")]}),
        )
        .await;
    let reported = tags_of(&emulator, &resource_arn).await;
    assert_eq!(
        reported.len(),
        bound,
        "case \"replace at the bound\": no key was added, got {reported:?}"
    );
    assert_eq!(
        reported.first().map(|(_, value)| value.as_str()),
        Some("replaced"),
        "case \"replace at the bound\": the value was replaced, got {reported:?}"
    );

    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [tag("OneTooMany", "x")]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"past the bound\": the tag bound is a client error, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidTagsToAddException",
        "case \"past the bound\": expected InvalidTagsToAddException, got {body:?}"
    );
    let reported = tags_of(&emulator, &resource_arn).await;
    assert_eq!(
        reported.len(),
        bound,
        "case \"past the bound\": the refused call changed nothing, got {reported:?}"
    );
}

/// RULE: reading a resource's tags changes nothing — the same list is reported
/// however many times it is read, and the resource's own record is untouched.
#[tokio::test]
async fn reading_the_tags_of_a_resource_changes_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "stable"}))
        .await;
    let resource_arn = state::application_arn(REGION, "stable");
    emulator
        .ok(
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [tag("Env", "prod")]}),
        )
        .await;

    let first = tags_of(&emulator, &resource_arn).await;
    for read in 1..=3 {
        let again = tags_of(&emulator, &resource_arn).await;
        assert_eq!(
            again, first,
            "case \"read {read}\": a read reports the same tags, got {again:?}"
        );
    }
    let application = emulator
        .ok("GetApplication", json!({"applicationName": "stable"}))
        .await;
    assert_eq!(
        application["application"]["applicationName"].as_str(),
        Some("stable"),
        "reading tags leaves the application itself alone, got {application:?}"
    );
}
