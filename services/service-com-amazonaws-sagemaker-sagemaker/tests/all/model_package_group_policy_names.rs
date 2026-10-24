// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the NAME a model-registry resource policy is addressed by.
//!
//! The sibling suite `model_package_group_policy` asserts the policy round trip over the
//! shapes a policy DOCUMENT takes; this one varies the other axis of the same rule — the
//! `ModelPackageGroupName` the document is stored under — because that name is the KEY of
//! the record, not a value the service reformats:
//!
//! * a put/get/delete round trip holds for every name the modelled `EntityName` admits,
//!   including both published length bounds, and each name is its own key: a group's
//!   policy is never reported for a different name;
//! * a name OUTSIDE the modelled constraints is refused by every policy operation with
//!   HTTP 400 `ValidationException`, reports no document, and changes nothing a later
//!   caller sees.
//!
//! The constraints both tables are built from are the modelled ones for
//! `GetModelPackageGroupPolicy$ModelPackageGroupName` /
//! `PutModelPackageGroupPolicy$ModelPackageGroupName`:
//! `EntityName`, `@length(min: 1, max: 63)`, `@pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! (`smithy_query(query="operation", operation="GetModelPackageGroupPolicy")`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) on the harness the `inference_deployment_lifecycle` suite stands up, so
//! what these tests exercise is what a real SDK client gets.

use serde_json::json;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The modelled `EntityName` length bounds for `ModelPackageGroupName`
/// (`@length(min: 1, max: 63)`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 63;

/// Group names spanning what the modelled `EntityName` admits: both published length
/// bounds, digits, mixed case, and the runs of hyphens the pattern's `-*` allows between
/// two alphanumerics.
fn accepted_names() -> Vec<(&'static str, String)> {
    let shortest = "g".to_string();
    assert_eq!(
        shortest.len(),
        NAME_MIN,
        "the shortest row must sit on the published minimum, got {shortest:?}"
    );
    let longest = format!("a{}z", "b".repeat(NAME_MAX - 2));
    assert_eq!(
        longest.len(),
        NAME_MAX,
        "the longest row must sit on the published maximum, got {} characters",
        longest.len()
    );
    vec![
        ("at-the-length-minimum", shortest),
        ("single-digit", "7".to_string()),
        ("mixed-case-with-hyphens", "Registry-Group-1".to_string()),
        ("runs-of-hyphens", "a--b---c".to_string()),
        ("digits-only", "0123456789".to_string()),
        ("at-the-length-maximum", longest),
    ]
}

/// Names the modelled constraints do NOT admit: one past each length bound, a hyphen
/// where the pattern requires an alphanumeric, characters the pattern's class does not
/// contain, and the group's own ARN (this member is `EntityName`, not `ArnOrName`).
fn refused_names() -> Vec<(&'static str, String)> {
    vec![
        ("empty-below-the-length-minimum", String::new()),
        ("one-past-the-length-maximum", "a".repeat(NAME_MAX + 1)),
        ("leading-hyphen", "-group".to_string()),
        ("trailing-hyphen", "group-".to_string()),
        ("underscore", "group_name".to_string()),
        ("dot", "group.name".to_string()),
        ("space", "group name".to_string()),
        (
            // `DescribeModelPackageGroup` takes `ArnOrName`, so this row names a group
            // that does not exist either way: the policy operations refuse the ARN on its
            // SHAPE, before any lookup.
            "an-arn-rather-than-a-name",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/absent-group"),
        ),
    ]
}

/// A policy document naming `owner`, so a document read back identifies the group it was
/// stored on and a document reported for the wrong key is visible as such.
fn policy_for(owner: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"Policy{}","Effect":"Allow","Principal":{{"AWS":"*"}},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}}]}}"#,
        owner.replace(['-', '_'], "")
    )
}

/// RULE: a model group's resource policy is stored under the group's NAME and handed back
/// byte for byte for every name the modelled `EntityName` admits — and only for that name,
/// since the name is the key of the record.
#[tokio::test]
async fn the_policy_round_trip_holds_for_every_accepted_name_shape() {
    let emulator = Emulator::new();
    let names = accepted_names();

    for (case, name) in &names {
        let document = policy_for(name);

        let created = emulator
            .ok(
                "CreateModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        let arn = str_field(&created, "ModelPackageGroupArn");
        assert!(
            arn.ends_with(&format!("model-package-group/{name}")),
            "case {case:?}: the group's ARN must carry the name it was created under, got \
             {arn:?}"
        );

        let put = emulator
            .ok(
                "PutModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": name, "ResourcePolicy": document }),
            )
            .await;
        assert_eq!(
            str_field(&put, "ModelPackageGroupArn"),
            arn,
            "case {case:?}: the put must answer the ARN the create minted"
        );

        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            document,
            "case {case:?}: the document must come back byte for byte"
        );

        // A read reports: the next reader sees the same document.
        let again = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            str_field(&again, "ResourcePolicy"),
            document,
            "case {case:?}: reading the document must not change it"
        );
    }

    // Every name still reports ITS OWN document after all the others were written, so the
    // name is the key: no later put reached an earlier group.
    for (case, name) in &names {
        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            policy_for(name),
            "case {case:?}: each name must report the document stored under it"
        );
    }

    // Deleting one group's policy releases that key alone.
    let (removed_case, removed_name) = &names[0];
    let (status, body) = emulator
        .call(
            "DeleteModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": removed_name }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {removed_case:?}: expected HTTP 200 from DeleteModelPackageGroupPolicy, got \
         {status} with {body}"
    );
    let (status, body) = emulator
        .call(
            "GetModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": removed_name }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {removed_case:?}: the released key must report no document, got {status} with \
         {body}"
    );
    for (case, name) in &names[1..] {
        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            policy_for(name),
            "case {case:?}: another key's delete must leave this document in place"
        );
    }
}

/// RULE: a `ModelPackageGroupName` outside the modelled `EntityName` constraints is
/// refused by every model-registry policy operation with HTTP 400 `ValidationException`,
/// reports no document, and leaves the policies that ARE stored untouched.
#[tokio::test]
async fn a_name_outside_the_modelled_entity_name_is_refused_by_every_policy_operation() {
    let emulator = Emulator::new();

    // A group that does exist, so every refusal can be shown to have changed nothing.
    let existing = "named-group";
    let stored = policy_for(existing);
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": existing }),
        )
        .await;
    emulator
        .ok(
            "PutModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": existing, "ResourcePolicy": stored }),
        )
        .await;

    for (case, name) in refused_names() {
        for (operation, body) in [
            (
                "GetModelPackageGroupPolicy",
                json!({ "ModelPackageGroupName": name }),
            ),
            (
                "PutModelPackageGroupPolicy",
                json!({ "ModelPackageGroupName": name, "ResourcePolicy": stored }),
            ),
            (
                "DeleteModelPackageGroupPolicy",
                json!({ "ModelPackageGroupName": name }),
            ),
        ] {
            let (status, refused) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case:?} {operation}: expected a 400 ValidationException for {name:?}, \
                 got {status} with {refused}"
            );
            assert!(
                refused.get("ResourcePolicy").is_none(),
                "case {case:?} {operation}: a refusal must report no document, got {refused}"
            );
            // The refusal is the CONSTRAINT's, not the one a name no group is stored
            // under gets: it names the member and the constraint it failed, so these rows
            // would not pass if no pattern/length validation ran at all.
            let reason = message(&refused);
            assert!(
                reason.contains("modelPackageGroupName") && reason.contains("constraint"),
                "case {case:?} {operation}: the refusal must name the member and the \
                 constraint it failed, got {reason:?}"
            );
            assert!(
                !reason.contains("does not exist")
                    && !reason.contains("Cannot find resource policy"),
                "case {case:?} {operation}: a name the model does not admit must be refused \
                 on its shape, not reported as a group that is merely absent, got {reason:?}"
            );
        }

        // The refusals created nothing and disturbed nothing.
        let (status, described) = emulator
            .call(
                "DescribeModelPackageGroup",
                &json!({ "ModelPackageGroupName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused name must not become a group, got {status} with \
             {described}"
        );
        let read = emulator
            .ok(
                "GetModelPackageGroupPolicy",
                &json!({ "ModelPackageGroupName": existing }),
            )
            .await;
        assert_eq!(
            str_field(&read, "ResourcePolicy"),
            stored,
            "case {case:?}: the stored document must be untouched by the refusals"
        );
    }
}
