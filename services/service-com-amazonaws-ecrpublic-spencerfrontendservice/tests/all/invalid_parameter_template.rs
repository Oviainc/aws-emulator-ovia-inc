// SPDX-License-Identifier: CC0-1.0

//! The `$.message` ECR Public states for a request the framework refused before any
//! handler ran, driven end to end over the wire.
//!
//! The rule under test: a request that falls outside a constraint the model declares is
//! refused with 400 `InvalidParameterException` — the one client-input error this model
//! declares — whose message is ECR Public's own template over the MEMBER and the RULE
//! alone: `Invalid parameter at '<member path>' failed to satisfy constraint:
//! '<requirement>'`, one clause per refused member (joined with `; ` when a request
//! refuses several), with the rejected value never interpolated and no "N validation
//! error(s) detected" preamble.
//!
//! Both halves of a clause are a deterministic function of the request and the model — the
//! member's dotted path, and the constraint the value fell outside of — so every row's
//! expectation is BUILT from the constraint the model declares (`smithy_query(query=
//! "shape", shape="RegistryDisplayName" | "RepositoryName" | "MaxResults" |
//! "ImageIdentifierList" | "ImageTag")`) rather than copied from a recording. The rows
//! span the class the template generalises over: each kind of constraint a member can
//! fail (a length floor and ceiling, a pattern, a numeric range, an absent required
//! member, a collection's entry count), at the top level and nested inside a list
//! element, over several operations.

use serde_json::{json, Value};

use crate::repository_image_lifecycle::{call, service};

/// The clause ECR Public states for one refused member.
fn clause(path: &str, requirement: &str) -> String {
    format!("Invalid parameter at '{path}' failed to satisfy constraint: '{requirement}'")
}

/// The `$.message` of a request refused with `InvalidParameterException`, asserting the
/// status and the wire code first.
async fn refusal_message(operation: &str, body: Value, case: &str) -> String {
    let router = service();
    let (status, parsed) = call(&router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
    assert!(
        wire_type.contains("InvalidParameterException"),
        "case {case:?}: expected InvalidParameterException, got {wire_type:?} in {parsed}"
    );
    parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {parsed}"))
        .to_string()
}

#[tokio::test]
async fn a_refused_member_is_stated_as_the_member_and_the_rule_it_failed() {
    // The modelled `RepositoryName` pattern, as the message states it: the expression a
    // value must satisfy, without the anchors the model spells it with.
    let repository_pattern = "(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*";

    // (case, operation, request, the ONE clause the message must be)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            // `RegistryDisplayName` is `@length(0..100)`.
            "a display name one character past the ceiling",
            "PutRegistryCatalogData",
            json!({"displayName": "x".repeat(101)}),
            clause(
                "displayName",
                "Member must have length less than or equal to 100",
            ),
        ),
        (
            "a display name far past the ceiling",
            "PutRegistryCatalogData",
            json!({"displayName": "d".repeat(4096)}),
            clause(
                "displayName",
                "Member must have length less than or equal to 100",
            ),
        ),
        (
            "a display name past the ceiling in multi-byte characters",
            "PutRegistryCatalogData",
            json!({"displayName": "名".repeat(101)}),
            clause(
                "displayName",
                "Member must have length less than or equal to 100",
            ),
        ),
        (
            // `RepositoryName` is `@length(2..205)`.
            "a repository name below the length floor",
            "CreateRepository",
            json!({"repositoryName": "a"}),
            clause(
                "repositoryName",
                "Member must have length greater than or equal to 2",
            ),
        ),
        (
            "a repository name past the length ceiling",
            "CreateRepository",
            json!({"repositoryName": "z".repeat(206)}),
            clause(
                "repositoryName",
                "Member must have length less than or equal to 205",
            ),
        ),
        (
            "a repository name outside the modelled pattern",
            "CreateRepository",
            json!({"repositoryName": "Not_A/Valid Name"}),
            clause(
                "repositoryName",
                &format!("must satisfy regular expression '{repository_pattern}'"),
            ),
        ),
        (
            "a repository name outside the pattern on a read",
            "DescribeImages",
            json!({"repositoryName": "UPPER"}),
            clause(
                "repositoryName",
                &format!("must satisfy regular expression '{repository_pattern}'"),
            ),
        ),
        (
            // `MaxResults` is `@range(1..1000)`.
            "a page size below the range floor",
            "DescribeRepositories",
            json!({"maxResults": 0}),
            clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "a page size past the range ceiling",
            "DescribeRepositories",
            json!({"maxResults": 1001}),
            clause(
                "maxResults",
                "Member must have value less than or equal to 1000",
            ),
        ),
        (
            // `ImageIdentifierList` is `@length(1..100)`.
            "more image identifiers than the list admits",
            "BatchDeleteImage",
            json!({
                "repositoryName": "probe",
                "imageIds": (0..101).map(|i| json!({"imageTag": format!("v{i}")})).collect::<Vec<Value>>(),
            }),
            clause(
                "imageIds",
                "Member must have length less than or equal to 100",
            ),
        ),
        (
            "an empty image identifier list",
            "BatchDeleteImage",
            json!({"repositoryName": "probe", "imageIds": []}),
            clause(
                "imageIds",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            // `ImageIdentifier$imageTag` is `@length(1..300)`: a nested member, whose
            // path names the list, the 1-based element and the member.
            "an empty image tag inside a list element",
            "BatchDeleteImage",
            json!({"repositoryName": "probe", "imageIds": [{"imageTag": ""}]}),
            clause(
                "imageIds.1.member.imageTag",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "an over-long image tag in the second list element",
            "BatchDeleteImage",
            json!({
                "repositoryName": "probe",
                "imageIds": [{"imageTag": "ok"}, {"imageTag": "t".repeat(301)}],
            }),
            clause(
                "imageIds.2.member.imageTag",
                "Member must have length less than or equal to 300",
            ),
        ),
        (
            // `RepositoryCatalogDataInput$description` is `@length(0..1024)`.
            "an over-long catalog description",
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": "probe",
                "catalogData": {"description": "c".repeat(1025)},
            }),
            clause(
                "catalogData.description",
                "Member must have length less than or equal to 1024",
            ),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let actual = refusal_message(operation, body, case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_names_the_member_and_never_the_value_it_rejected() {
    // Rows whose rejected value is a string a message could echo: each one is long or
    // distinctive enough that its presence in the message is unmistakable.
    let rows: Vec<(&str, &str, Value, String, &str)> = vec![
        (
            "a display name past the ceiling",
            "PutRegistryCatalogData",
            json!({"displayName": "secret-display-name-".repeat(10)}),
            "secret-display-name-".repeat(10),
            "displayName",
        ),
        (
            "a repository name outside the pattern",
            "CreateRepository",
            json!({"repositoryName": "Secret/Repository Name"}),
            "Secret/Repository Name".to_string(),
            "repositoryName",
        ),
        (
            "an over-long image tag",
            "BatchDeleteImage",
            json!({"repositoryName": "probe", "imageIds": [{"imageTag": "tag-secret".repeat(40)}]}),
            "tag-secret".repeat(40),
            "imageIds.1.member.imageTag",
        ),
        (
            "an over-long catalog description",
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": "probe",
                "catalogData": {"description": "description-secret".repeat(100)},
            }),
            "description-secret".repeat(100),
            "catalogData.description",
        ),
    ];

    for (case, operation, body, rejected, path) in rows {
        let actual = refusal_message(operation, body, case).await;
        assert!(
            !actual.contains(&rejected),
            "case {case:?}: the message must not echo the rejected value, got {actual:?}"
        );
        assert!(
            actual.starts_with(&format!("Invalid parameter at '{path}' ")),
            "case {case:?}: the message must open with the member at fault, got {actual:?}"
        );
        // The framework's own envelope is what this template replaces: neither its
        // preamble nor its `Value '<v>' at` clause may survive.
        assert!(
            !actual.contains("validation error"),
            "case {case:?}: the message must carry no validation-error preamble, got {actual:?}"
        );
        assert!(
            !actual.contains("Value "),
            "case {case:?}: the message must not state the value at all, got {actual:?}"
        );
        // The message is bounded by the member path and the rule, never by the request.
        assert!(
            actual.len() < 200,
            "case {case:?}: the message must not grow with the request, got {} characters \
             in {actual:?}",
            actual.len()
        );
    }
}

#[tokio::test]
async fn an_absent_required_member_is_stated_as_a_member_that_must_not_be_null() {
    // (case, operation, request, the member the refusal must name)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        ("no repositoryName at all", "CreateRepository", json!({}), "repositoryName"),
        (
            "no imageIds beside a repositoryName",
            "BatchDeleteImage",
            json!({"repositoryName": "probe"}),
            "imageIds",
        ),
        (
            "no layerDigests beside the upload it names",
            "CompleteLayerUpload",
            json!({
                "repositoryName": "probe",
                "uploadId": "0123abcd-4567-89ef-0123-456789abcdef",
            }),
            "layerDigests",
        ),
        (
            "no catalogData on a repository catalog write",
            "PutRepositoryCatalogData",
            json!({"repositoryName": "probe"}),
            "catalogData",
        ),
    ];

    for (case, operation, body, member) in rows {
        let actual = refusal_message(operation, body, case).await;
        let expected = clause(member, "Member must not be null");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_request_refusing_two_members_states_one_clause_for_each() {
    // A name that is both below the length floor and outside the pattern, plus a page
    // size outside the range: three refused members on one request, each stated once.
    let case = "a name failing two constraints beside a bad page size";
    let actual = refusal_message(
        "DescribeImages",
        json!({"repositoryName": "", "maxResults": 0, "imageIds": [{"imageTag": ""}]}),
        case,
    )
    .await;

    // The template is per MEMBER, so the message is the clauses joined — whatever order
    // the validating walk reports them in.
    let mut stated: Vec<&str> = actual.split("; ").collect();
    stated.sort_unstable();
    let mut expected = [
        clause(
            "repositoryName",
            "Member must have length greater than or equal to 2",
        ),
        clause(
            "repositoryName",
            "must satisfy regular expression \
             '(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*'",
        ),
        clause(
            "maxResults",
            "Member must have value greater than or equal to 1",
        ),
        clause(
            "imageIds.1.member.imageTag",
            "Member must have length greater than or equal to 1",
        ),
    ];
    expected.sort_unstable();
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(
        stated, expected_refs,
        "case {case:?}: expected one clause per refused member, got {actual:?}"
    );
}
