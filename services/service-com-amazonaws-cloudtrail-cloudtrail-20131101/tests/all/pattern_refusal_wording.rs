// SPDX-License-Identifier: CC0-1.0

//! How this service words a request refused at the boundary for a `@pattern` violation.
//!
//! The rule under test: a value outside a member's modelled `@pattern` is refused with
//! HTTP 400 `ValidationException` whose message names the member's dotted path and the
//! expression the value had to match — `1 validation error detected: Value at
//! '<member path>' failed to satisfy constraint: Member must satisfy regular expression
//! pattern: <expression>` — and echoes NO part of the value that was sent. It holds for
//! every pattern-constrained member of every operation, at any depth of the request, so
//! the rows below span several operations, several expressions and a member nested inside
//! a list.
//!
//! The sibling suite `range_refusal_wording.rs` holds the same value-free rule for the
//! `@range` class, and pins that a `@length` violation — a class this service has no
//! recorded value-free wording for — keeps the framework's own sentence. What this suite
//! adds beside it is the `@pattern` class, the expression rendering (anchors included, as
//! the model writes it) and the multi-violation envelope.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness, so what
//! is asserted is what a client sees: the HTTP status, the error code and the message.
//!
//! Each expression below is quoted from this service's Smithy model
//! (`smithy_query(query="shape", shape="<Shape>")`), named per row.

use serde_json::{json, Value};

use super::emulator::Emulator;

/// The envelope AWS states one violation under.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a `@pattern` violation at `path` states, for the expression `pattern`.
fn pattern_clause(path: &str, pattern: &str) -> String {
    format!(
        "Value at '{path}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {pattern}"
    )
}

/// A value outside a member's `@pattern` is refused with the member's path and the
/// expression, and with no echo of what was sent — over every operation and expression
/// below, including a member nested inside a list.
#[tokio::test]
async fn a_pattern_violation_states_the_member_and_the_expression_without_the_value() {
    let emulator = Emulator::new();

    // (case, operation, request, member path, the model's expression for that member)
    //
    // Sources: `ListImportFailuresRequest$ImportId` and `StopImportRequest$ImportId`
    // target `UUID`; `GetResourcePolicyRequest$ResourceArn` targets `ResourceArn`;
    // `DescribeQueryRequest$QueryAlias` targets `QueryAlias` and
    // `$EventDataStoreOwnerAccountId` targets `AccountId`; `Destination$Location`
    // (reached through `CreateChannelRequest$Destinations`) targets `Location`.
    let rows: [(&str, &str, Value, &str, &str); 6] = [
        (
            "an import id in upper-case hex",
            "ListImportFailures",
            json!({"ImportId": "ZZZZZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZZZZZZZZZ"}),
            "importId",
            r"^[a-f0-9\-]+$",
        ),
        (
            "the same expression on another operation",
            "StopImport",
            json!({"ImportId": "0F0F0F0F-1a2b-3c4d-5e6f-7a8b9c0d1e2f"}),
            "importId",
            r"^[a-f0-9\-]+$",
        ),
        (
            "an ARN holding a character the expression excludes",
            "GetResourcePolicy",
            json!({"ResourceArn": "not an arn"}),
            "resourceArn",
            r"^[a-zA-Z0-9._/\-:]+$",
        ),
        (
            "a query alias that does not start with a letter",
            "DescribeQuery",
            json!({"QueryAlias": "1-leading-digit"}),
            "queryAlias",
            r"^[a-zA-Z][a-zA-Z0-9._\-]*$",
        ),
        (
            "an account id that is not all digits",
            "DescribeQuery",
            json!({"EventDataStoreOwnerAccountId": "abcdefghijkl"}),
            "eventDataStoreOwnerAccountId",
            r"^\d+$",
        ),
        (
            "a member nested inside a list",
            "CreateChannel",
            json!({
                "Name": "a-valid-channel-name",
                "Source": "Custom",
                "Destinations": [{"Type": "EVENT_DATA_STORE", "Location": "not a location"}],
            }),
            "destinations.1.member.location",
            r"^[a-zA-Z0-9._/\-:*]+$",
        ),
    ];

    for (case, operation, request, path, pattern) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            one_error(&pattern_clause(path, pattern)),
            "case {case}: expected the clause for {path}, got {:?}",
            reply.message()
        );
    }
}

/// A violation of a class this service has no recorded value-free wording for keeps the
/// framework's own clause: it still states the member and the requirement under the same
/// envelope, so the value-free rewording is the recorded classes and nothing wider.
#[tokio::test]
async fn a_violation_of_another_class_still_states_the_member_and_the_requirement() {
    let emulator = Emulator::new();
    let valid_id = "0f0f0f0f-1a2b-3c4d-5e6f-7a8b9c0d1e2f";

    // (case, request, the clause fragment the refusal has to carry).
    // `ListImportFailuresRequest$ImportId` is `@required` and `$NextToken` is
    // `@length 4..1000`, so each row below breaks exactly one non-pattern rule.
    let rows: [(&str, Value, &str); 2] = [
        (
            "a required member left out",
            json!({}),
            "at 'importId' failed to satisfy constraint: Member must not be null",
        ),
        (
            "a token under the modelled length",
            json!({"ImportId": valid_id, "NextToken": "abc"}),
            "at 'nextToken' failed to satisfy constraint: Member must have length greater \
             than or equal to 4",
        ),
    ];

    for (case, request, fragment) in rows {
        let reply = emulator.call("ListImportFailures", &request).await;
        reply.expect_error(case, 400, "ValidationException");
        let message = reply.message();
        assert!(
            message.starts_with("1 validation error detected: ") && message.contains(fragment),
            "case {case}: expected one error carrying {fragment:?}, got {message:?}"
        );
    }
}

/// Two broken rules in one request are one refusal stating both clauses, each naming its
/// member and its requirement and neither echoing what was sent.
#[tokio::test]
async fn two_violations_are_stated_under_one_envelope() {
    let emulator = Emulator::new();
    let sent = "ZZZZZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZZZZZZZZZ";

    let reply = emulator
        .call(
            "ListImportFailures",
            &json!({"ImportId": sent, "MaxResults": 0}),
        )
        .await;
    reply.expect_error("a bad id beside a bad page size", 400, "ValidationException");
    let message = reply.message();
    let case = "a bad id beside a bad page size";
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case}: expected both violations under one envelope, got {message:?}"
    );
    assert!(
        message.contains(&pattern_clause("importId", r"^[a-f0-9\-]+$")),
        "case {case}: expected the pattern clause for importId, got {message:?}"
    );
    assert!(
        message.contains(
            "at 'maxResults' failed to satisfy constraint: Member must have value greater \
             than or equal to 1"
        ),
        "case {case}: expected the page-size clause too, got {message:?}"
    );
    assert!(
        !message.contains(sent),
        "case {case}: the rejected id must not be echoed, got {message:?}"
    );
}
