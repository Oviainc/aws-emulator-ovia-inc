// SPDX-License-Identifier: CC0-1.0

//! The `$.message` ECR Public states for a `nextToken` it never issued, over every
//! paginated reader this service has, driven end to end over the wire.
//!
//! The rule under test: a pagination token this service did not mint is refused with HTTP
//! 400 `InvalidParameterException` — the one client-input error this model declares — and
//! the refusal is this service's parameter-constraint template over the member and the
//! rule alone, `Invalid parameter at 'NextToken' failed to satisfy constraint: 'Invalid
//! next token provided'`: the member path is the service's own PascalCase parameter name
//! for the token (the WIRE member is lower-camel `nextToken`, as
//! `smithy_query(query="operation", operation="DescribeImageTags,DescribeImages,
//! DescribeRepositories,DescribeRegistries")` reports for every `@paginated` reader of
//! this service, whose `inputToken` is `nextToken` -> shape `NextToken`), and the
//! constraint clause is the VERDICT on the token supplied, not a prerequisite on how the
//! caller should have come by it. The clause states neither the rejected token nor
//! anything else the request carried, and the refused call changes nothing a later caller
//! sees.
//!
//! Each row differs along what the rule generalises over: which paginated operation is
//! read, and the form a never-issued token takes (not hex at all, odd-length hex, hex
//! that decodes to bytes that are not text, a base64 cursor from some other service, one
//! carrying punctuation or whitespace, one far longer than any this service mints, and —
//! where a well-formed token can still name a key the listing could never have ended on —
//! that case too). The token's form is exactly what the rule generalises over, so one
//! message is expected for all of them.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_message;
use crate::repository_image_lifecycle::{call, digest_of, ok, refused, service, REGISTRY_ID};

/// The repository the image-scoped readers are pointed at.
const REPOSITORY: &str = "team/cursors";

/// The one clause ECR Public states for a token it did not issue, built from the member
/// and the rule rather than copied from a recording.
fn expected_message() -> String {
    format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: '{requirement}'",
        member = "NextToken",
        requirement = "Invalid next token provided",
    )
}

/// Every form a token this service never minted can take, for a reader whose listing
/// accepts any well-formed cursor.
fn never_issued_tokens() -> Vec<(String, String)> {
    vec![
        ("not hex at all".to_string(), "not-hex".to_string()),
        ("odd-length hex".to_string(), "abc".to_string()),
        (
            "hex that decodes to bytes that are not text".to_string(),
            "ff".to_string(),
        ),
        (
            "hex that decodes to a truncated multi-byte sequence".to_string(),
            "e298".to_string(),
        ),
        (
            "a base64 cursor from some other service".to_string(),
            "bmV2ZXItaXNzdWVkLWN1cnNvcg==".to_string(),
        ),
        (
            "a token carrying whitespace and punctuation".to_string(),
            "page 2 of 5?".to_string(),
        ),
        (
            "a token far longer than any this service mints".to_string(),
            "zq".repeat(512),
        ),
    ]
}

/// The paginated readers of this service, each with the request members it needs beside
/// the token.
fn paginated_readers() -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeImageTags", json!({"repositoryName": REPOSITORY})),
        ("DescribeImages", json!({"repositoryName": REPOSITORY})),
        ("DescribeRepositories", json!({})),
        ("DescribeRegistries", json!({})),
    ]
}

/// Seed one repository holding three tagged manifests, so every reader under test has a
/// non-empty listing to be refused a cursor into.
async fn seed(router: &MultiServiceRouter) -> Vec<String> {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": REPOSITORY}),
        "seed the repository",
    )
    .await;
    let mut digests = Vec::new();
    for index in 0..3 {
        let manifest = json!({"schemaVersion": 2, "name": format!("cursor-{index}")}).to_string();
        ok(
            router,
            "PutImage",
            json!({
                "repositoryName": REPOSITORY,
                "imageManifest": manifest,
                "imageTag": format!("v{index}"),
            }),
            "seed an image",
        )
        .await;
        digests.push(digest_of(&manifest));
    }
    digests
}

/// The tags `DescribeImageTags` reports for the seeded repository, unpaged.
async fn tags(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = ok(
        router,
        "DescribeImageTags",
        json!({"repositoryName": REPOSITORY}),
        case,
    )
    .await;
    body["imageTagDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageTagDetails array, got {body}"))
        .iter()
        .map(|entry| entry["imageTag"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_never_issued_token_is_refused_as_the_token_parameter_and_the_token_rule() {
    let router = service();
    seed(&router).await;
    let expected = expected_message();

    for (operation, base) in paginated_readers() {
        for (form, token) in never_issued_tokens() {
            // The same refusal whether or not the request also asks for a page size: the
            // page size is not what the token clause is about.
            let variants: Vec<(String, Option<i64>)> = vec![
                (format!("{operation}, {form}"), None),
                (
                    format!("{operation}, {form}, with maxResults"),
                    Some(2),
                ),
            ];
            for (case, max_results) in variants {
                let mut body = base.clone();
                body["nextToken"] = json!(token);
                if let Some(max_results) = max_results {
                    body["maxResults"] = json!(max_results);
                }
                let parsed =
                    refused(&router, operation, body, "InvalidParameterException", &case).await;
                let actual = error_message(&parsed);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_token_refusal_states_the_service_parameter_and_never_the_token_itself() {
    let router = service();
    seed(&router).await;

    let mut stated: BTreeSet<String> = BTreeSet::new();
    for (operation, base) in paginated_readers() {
        for (form, token) in never_issued_tokens() {
            let case = format!("{operation}, {form}");
            let mut body = base.clone();
            body["nextToken"] = json!(token);
            let parsed =
                refused(&router, operation, body, "InvalidParameterException", &case).await;
            let actual = error_message(&parsed);

            // The member path is the service's parameter name, not the wire member name.
            assert!(
                actual.starts_with("Invalid parameter at 'NextToken' "),
                "case {case:?}: expected the clause to open with the token parameter, \
                 got {actual:?}"
            );
            assert!(
                !actual.contains("'nextToken'"),
                "case {case:?}: expected the service parameter name rather than the wire \
                 member name, got {actual:?}"
            );
            // The clause is a verdict on the token, not a prerequisite on the caller.
            assert!(
                !actual.contains("Member must"),
                "case {case:?}: expected a verdict on the supplied token rather than a \
                 caller prerequisite, got {actual:?}"
            );
            // Nothing the request carried is quoted back, and no framework envelope
            // survives.
            if !token.is_empty() {
                assert!(
                    !actual.contains(token.as_str()),
                    "case {case:?}: the message must not echo the rejected token, \
                     got {actual:?}"
                );
            }
            for leaked in [REPOSITORY, REGISTRY_ID] {
                assert!(
                    !actual.contains(leaked),
                    "case {case:?}: the message must not state {leaked:?}, got {actual:?}"
                );
            }
            assert!(
                !actual.contains("validation error"),
                "case {case:?}: the message must carry no validation-error preamble, \
                 got {actual:?}"
            );
            stated.insert(actual);
        }
    }

    // The token's form and the operation read are what the rule generalises over, so the
    // whole table states ONE message.
    assert_eq!(
        stated.len(),
        1,
        "expected one message across every reader and token form, got {stated:?}"
    );
}

#[tokio::test]
async fn a_well_formed_token_naming_no_page_of_the_listing_is_refused_the_same_way() {
    // `DescribeRegistries` lists exactly one registry, so the registry id is the only key
    // a page of it can end on: a well-formed token naming anything else is a cursor this
    // service never issued, and must be refused with the same clause as a malformed one.
    let router = service();
    let expected = expected_message();
    for key in ["not-the-registry", "999999999999", "emulator"] {
        let case = format!("a well-formed token naming {key:?}");
        let parsed = refused(
            &router,
            "DescribeRegistries",
            json!({"nextToken": hex::encode(key)}),
            "InvalidParameterException",
            &case,
        )
        .await;
        let actual = error_message(&parsed);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_token_leaves_every_listing_as_it_was() {
    let router = service();
    let digests = seed(&router).await;
    let before = tags(&router, "before the refused tokens").await;
    assert_eq!(
        before,
        vec!["v0".to_string(), "v1".to_string(), "v2".to_string()],
        "expected the seeded tag set, got {before:?}"
    );

    for (operation, base) in paginated_readers() {
        for (form, token) in never_issued_tokens() {
            let case = format!("{operation}, {form}");
            let mut body = base.clone();
            body["nextToken"] = json!(token);
            refused(&router, operation, body, "InvalidParameterException", &case).await;
        }
    }

    // Every listing still reports exactly what it held before the refusals, and the
    // cursor of a fresh walk has not advanced.
    let after = tags(&router, "after the refused tokens").await;
    assert_eq!(
        after, before,
        "expected the refused tokens to change no tag, got {after:?}"
    );

    let case = "the first page after the refused tokens";
    let first_page = ok(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": REPOSITORY, "maxResults": 2}),
        case,
    )
    .await;
    let page_tags: Vec<String> = first_page["imageTagDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an array, got {first_page}"))
        .iter()
        .map(|entry| entry["imageTag"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        page_tags,
        vec!["v0".to_string(), "v1".to_string()],
        "case {case:?}: expected the walk to restart at the first page, got {page_tags:?}"
    );
    assert!(
        first_page["nextToken"].as_str().is_some(),
        "case {case:?}: expected a continuation token for the remaining tag, \
         got {first_page}"
    );

    let case = "the images after the refused tokens";
    let images = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": REPOSITORY}),
        case,
    )
    .await;
    let described: BTreeSet<String> = images["imageDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an array, got {images}"))
        .iter()
        .map(|entry| entry["imageDigest"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        described,
        digests.iter().cloned().collect::<BTreeSet<String>>(),
        "case {case:?}: expected every pushed manifest to survive the refusals, \
         got {described:?}"
    );

    // And a token refusal is decided before anything the request names is looked up, so a
    // repository this registry never held is still answered with the token clause.
    let case = "a never-issued token on a repository that does not exist";
    let (status, parsed) = call(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": "no-such-repository", "nextToken": "not-hex"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed}"
    );
    let actual = error_message(&parsed);
    assert_eq!(
        actual,
        expected_message(),
        "case {case:?}: expected {:?}, got {actual:?}",
        expected_message()
    );
}
