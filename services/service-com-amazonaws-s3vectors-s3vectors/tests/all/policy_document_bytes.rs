// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bytes GetVectorBucketPolicy reports, served over the real
//! `restJson1` wire through the router production uses.
//!
//! The rule under test: an IAM policy document this service takes as an opaque string
//! member is not echoed in the formatting the caller submitted — it is reported as the
//! service renders the policy, which differs from the submitted text in exactly two
//! ways: the insignificant whitespace between its JSON tokens is dropped, and an
//! element the policy language types as "String or Array of strings" (a statement's
//! `Action`/`NotAction` and `Resource`/`NotResource`, and the principal lists inside
//! `Principal`/`NotPrincipal`) is reported as the STRING when the caller submitted a
//! one-element array holding one string. Nothing else moves: the member order, the
//! statement count, the `Statement` array itself, the string literals and the number
//! literals of the submitted document all survive, and the reported form is a fixed
//! point of a get-modify-put round trip.
//!
//! Each table row states the case it covers, spells out the bytes a client must read
//! back, and every assertion reports the actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, ok, router};

/// A fresh router with one empty vector bucket of this name.
async fn with_bucket(bucket: &str) -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    router
}

/// Store `policy` on `bucket` and return the bytes a get reports back.
async fn put_then_get(router: &MultiServiceRouter, bucket: &str, policy: &str) -> String {
    ok(
        router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket, "policy": policy }),
    )
    .await;
    let body = ok(
        router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    body["policy"]
        .as_str()
        .unwrap_or_else(|| panic!("GetVectorBucketPolicy must report a policy string, got {body}"))
        .to_string()
}

/// One row: what a failing assertion calls it, the document submitted, and the exact
/// bytes a get must report back.
type Row = (&'static str, &'static str, &'static str);

/// Documents that differ along what the rule generalises over: WHERE the submitted
/// whitespace sits (between the separators, as indentation across lines, around the
/// document), whether it is inside a string literal (where it is part of the value),
/// how a string's escapes are spelled, how a number is spelled, in which order the
/// members arrive — the last two being what a re-serialization, rather than a scan,
/// would change — WHICH element carries a one-element array, how many elements that
/// array holds (one collapses, none and several do not), and whether the element is one
/// the policy language types as string-or-array at all.
///
/// Every row is a policy document by the IAM JSON policy grammar, since a document
/// that is not one is refused rather than stored.
fn rows() -> Vec<Row> {
    vec![
        (
            "a document already carrying no insignificant whitespace",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectors","Resource":"*"}]}"#,
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectors","Resource":"*"}]}"#,
        ),
        (
            "a space after every ':' and ','",
            r#"{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "s3vectors:GetVectors", "Resource": "*"}]}"#,
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectors","Resource":"*"}]}"#,
        ),
        (
            "indented over several lines, with a tab and a carriage return",
            "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{\r\n\t\t\t\"Effect\": \"Deny\",\r\n\t\t\t\"Principal\": \"*\",\r\n\t\t\t\"NotAction\": \"s3vectors:GetVectors\"\r\n\t\t}\r\n\t]\r\n}",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Principal":"*","NotAction":"s3vectors:GetVectors"}]}"#,
        ),
        (
            "whitespace wrapped around the whole document",
            "  \n {\"Statement\":{\"Effect\":\"Allow\",\"Action\":\"s3vectors:*\"}} \n ",
            r#"{"Statement":{"Effect":"Allow","Action":"s3vectors:*"}}"#,
        ),
        (
            "spaces, separators and an escape inside a string literal",
            r#"{"Id": "two  spaces: and, a comma", "Statement": [{"Sid": "a\tb", "Effect": "Allow", "Action": "s3vectors:*"}]}"#,
            r#"{"Id":"two  spaces: and, a comma","Statement":[{"Sid":"a\tb","Effect":"Allow","Action":"s3vectors:*"}]}"#,
        ),
        (
            "an escaped quote and an escaped backslash inside a string literal",
            r#"{"Id": "a \" b \\", "Statement": [{"Effect": "Deny", "Action": "s3vectors:PutVectors"}]}"#,
            r#"{"Id":"a \" b \\","Statement":[{"Effect":"Deny","Action":"s3vectors:PutVectors"}]}"#,
        ),
        (
            "members in an order a re-serialization would not keep",
            r#"{"Statement": [{"Resource": "*", "Effect": "Allow", "Action": "s3vectors:GetVectors"}], "Version": "2012-10-17", "Id": "z"}"#,
            r#"{"Statement":[{"Resource":"*","Effect":"Allow","Action":"s3vectors:GetVectors"}],"Version":"2012-10-17","Id":"z"}"#,
        ),
        (
            "number literals keeping their own spelling",
            r#"{"Statement": [{"Effect": "Allow", "Action": "s3vectors:GetVectors", "Condition": {"NumericEquals": {"a": 1e3, "b": 1.50, "c": -0.0}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"s3vectors:GetVectors","Condition":{"NumericEquals":{"a":1e3,"b":1.50,"c":-0.0}}}]}"#,
        ),
        (
            "non-ASCII text inside a string literal",
            "{\"Statement\": [{\"Sid\": \"zugriff f\u{fc}r pr\u{fc}fung \u{30c6}\u{30b9}\u{30c8}\", \"Effect\": \"Allow\", \"Action\": \"s3vectors:GetVectorBucket\"}]}",
            "{\"Statement\":[{\"Sid\":\"zugriff f\u{fc}r pr\u{fc}fung \u{30c6}\u{30b9}\u{30c8}\",\"Effect\":\"Allow\",\"Action\":\"s3vectors:GetVectorBucket\"}]}",
        ),
        (
            "a one-element Action array becomes the string it names",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":["s3vectors:GetVectors"],"Resource":"*"}]}"#,
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectors","Resource":"*"}]}"#,
        ),
        (
            "one-element NotAction, Resource, NotResource and principal arrays, spaced",
            r#"{"Statement": [{"Effect": "Deny", "Principal": {"AWS": ["arn:aws:iam::123456789012:root"]}, "NotAction": ["s3vectors:GetVectors"], "Resource": ["*"]}, {"Effect": "Deny", "NotPrincipal": {"Service": ["s3vectors.amazonaws.com"]}, "Action": "s3vectors:PutVectors", "NotResource": ["arn:aws:s3vectors:us-east-1:123456789012:bucket/b"]}]}"#,
            r#"{"Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"NotAction":"s3vectors:GetVectors","Resource":"*"},{"Effect":"Deny","NotPrincipal":{"Service":"s3vectors.amazonaws.com"},"Action":"s3vectors:PutVectors","NotResource":"arn:aws:s3vectors:us-east-1:123456789012:bucket/b"}]}"#,
        ),
        (
            "arrays of two and of none, and a one-element array under an element outside \
             the string-or-array set, keep their array form",
            r#"{"Statement": [{"Effect": "Allow", "Principal": {"AWS": ["arn:aws:iam::123456789012:root", "*"]}, "Action": ["s3vectors:GetVectors", "s3vectors:ListVectors"], "Resource": [], "Condition": {"StringEquals": {"aws:PrincipalAccount": ["123456789012"]}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","*"]},"Action":["s3vectors:GetVectors","s3vectors:ListVectors"],"Resource":[],"Condition":{"StringEquals":{"aws:PrincipalAccount":["123456789012"]}}}]}"#,
        ),
        (
            "a one-statement Statement array stays an array",
            r#"{ "Statement" : [ { "Effect" : "Allow" , "Principal" : "*" , "Action" : [ "s3vectors:GetVectorBucket" ] } ] }"#,
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectorBucket"}]}"#,
        ),
        (
            "text inside a string literal is never read as an element",
            r#"{"Statement": [{"Sid": "\"Action\":[\"s3vectors:GetVectors\"]", "Effect": "Allow", "Principal": "*", "Action": ["s3vectors:GetVectors"]}]}"#,
            r#"{"Statement":[{"Sid":"\"Action\":[\"s3vectors:GetVectors\"]","Effect":"Allow","Principal":"*","Action":"s3vectors:GetVectors"}]}"#,
        ),
    ]
}

/// Every accepted document is reported as the service renders it: the reported bytes
/// are the row's, the document still means the same thing, and putting the reported
/// bytes back reports them again.
#[tokio::test]
async fn a_stored_policy_is_reported_in_the_services_own_rendering() {
    let router = with_bucket("policy-whitespace").await;

    for (case, submitted, reported) in rows() {
        let actual = put_then_get(&router, "policy-whitespace", submitted).await;
        assert_eq!(
            actual, reported,
            "case {case:?}: expected {reported:?}, got {actual:?}"
        );

        // Nothing but the rendering moved: the document still parses, and its meaning is
        // the submitted one — a one-element array and the string it names say the same
        // thing in the policy language.
        let reported_value: Value =
            serde_json::from_str(&actual).expect("the reported document is JSON");
        assert!(
            reported_value.get("Statement").is_some(),
            "case {case:?}: the reported document must still be a policy, got {actual:?}"
        );

        // The reported form is a fixed point, so a get-modify-put round trip is
        // stable: drift detection that diffs a read against the last write sees a
        // difference only when the document itself differs.
        let again = put_then_get(&router, "policy-whitespace", &actual).await;
        assert_eq!(
            again, actual,
            "case {case:?}: putting the reported document back must report it again, \
             got {again:?}"
        );
    }
}

/// The bytes are the bucket's own: each bucket reports its own document, and a
/// document whose only difference from another is its whitespace is reported
/// identically by both.
#[tokio::test]
async fn each_bucket_reports_its_own_document_and_formatting_is_not_part_of_it() {
    let router = router();
    for bucket in ["policy-bytes-one", "policy-bytes-two"] {
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
    }

    let compact = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3vectors:ListVectors"}]}"#;
    let spaced = r#"{"Version": "2012-10-17", "Statement": [ {"Effect": "Allow", "Principal": "*", "Action": "s3vectors:ListVectors"} ]}"#;

    let one = put_then_get(&router, "policy-bytes-one", compact).await;
    let two = put_then_get(&router, "policy-bytes-two", spaced).await;
    assert_eq!(
        (one.as_str(), two.as_str()),
        (compact, compact),
        "two documents that differ only in whitespace must be reported alike, \
         got {one:?} and {two:?}"
    );

    // A document that is not a policy document at all is refused rather than stored,
    // so there is no reported form for one: the whitespace rule applies to what the
    // grammar already accepted.
    let (status, body) = call(
        &router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-bytes-one", "policy": "  { not a policy  " }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a document outside the policy grammar must be refused, got {status} {body}"
    );
    let body = ok(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": "policy-bytes-one" }),
    )
    .await;
    assert_eq!(
        body["policy"].as_str(),
        Some(compact),
        "a refused put must leave the stored document alone, got {body}"
    );
}
