// SPDX-License-Identifier: CC0-1.0

//! The modelled bound of the NOT-FOUND refusal, over the awsJson1_1 wire.
//!
//! RULE: every member of a refusal this service answers carries a value inside the
//! `@length` bound the model gives THAT member, including the identifier the refusal
//! reports back — `ResourceNotFoundException`'s three members are all the modelled
//! `GenericString`, `@length(0, 1024)`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException")`), while the
//! identifier they report is a caller-supplied value whose own shape admits more (an
//! `Arn` runs to 2048 characters — `smithy_query(query="shape",shape="Arn")`) and which
//! the boundary validator hands straight to the handler. So an identifier the member has
//! room for is reported WHOLE — a client reads it to know which of its own requests
//! failed — and a longer one gives way to the member's bound, keeping its leading
//! characters, exactly as an echoed resource gives way to the room an
//! `AccessDeniedException` sentence leaves it (`tests/all/error_member_bounds.rs`).
//!
//! The rows differ along how long the reported identifier is — the ARN shape's published
//! ceiling, one character past the member's bound, exactly at it, one inside it and an
//! ordinary one — crossed with every operation of this service that reports an identifier
//! it could not find, which is what the rule generalises over. A second case reports an
//! `ExecutionId` rather than an ARN, at exactly the member's bound, where nothing has to
//! give way.

use serde_json::{json, Value};

use crate::export_lifecycle::{
    cases, error_code, padded_arn, Client, MAX_ARN_CHARS, NOT_FOUND_STATUS,
};

/// The `@length` ceiling of the modelled `GenericString` every member of
/// `ResourceNotFoundException` targets (smithy_query shape `ResourceNotFoundException`:
/// `Message`, `ResourceId` and `ResourceType` all `@length(min: 0, max: 1024)`). Stated
/// from the MODEL, so the expectation is not read from the code under test.
const MEMBER_MAX_CHARS: usize = 1024;

/// The `@length` ceiling of `GetExecution`'s `ExecutionId` (smithy_query operation
/// `GetExecution`, member `ExecutionId`: `GenericString`, `@length(min: 0, max: 1024)`).
const MAX_EXECUTION_ID_CHARS: usize = 1024;

/// How many characters of the member's room a clipped report may spend on the marker that
/// stands for what was dropped, rather than on the identifier itself. A few characters: a
/// client has to be left with enough of the identifier to match its own request, so the
/// marker must not take the room the identity needs. This pins "the report uses the room
/// the member has" without fixing one arithmetic answer for how the marker is spelled.
const MARKER_SLACK: usize = 8;

/// The resource segment of the ARN this suite asks about (see
/// [`crate::export_lifecycle::padded_arn`]).
const BOUNDED_NOT_FOUND_RESOURCE: &str = "export/bounded_not_found";

/// A well-formed ARN of THIS account, `chars` characters long, naming no export.
///
/// Built by the shared [`crate::export_lifecycle::padded_arn`] builder, which pads inside
/// the RESOURCE segment, so it satisfies the modelled `Arn` pattern and the boundary
/// validator hands it to the handler. The account is the emulator's own because the
/// tagging operations judge the ARN's account BEFORE any lookup
/// (`tests/all/tagging_arn_validation.rs`).
fn bounded_arn(chars: usize) -> String {
    padded_arn(BOUNDED_NOT_FOUND_RESOURCE, chars)
}

/// Every request that hands an ARN the service cannot resolve to a not-found refusal.
fn requests(arn: &str, export: &Value) -> Vec<(&'static str, Value)> {
    vec![
        ("GetExport", json!({ "ExportArn": arn })),
        ("DeleteExport", json!({ "ExportArn": arn })),
        ("UpdateExport", json!({ "ExportArn": arn, "Export": export })),
        (
            "GetExecution",
            json!({ "ExportArn": arn, "ExecutionId": "00000000-0000-0000-0000-000000000000" }),
        ),
        ("ListTagsForResource", json!({ "ResourceArn": arn })),
        (
            "TagResource",
            json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": arn, "ResourceTagKeys": ["env"] }),
        ),
    ]
}

/// Assert the refusal `body` is a not-found whose every member is inside the modelled
/// bound, and that it reports `identifier` — whole when the member had room for it, by its
/// leading characters otherwise.
fn check_bounded_not_found(case: &str, identifier: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case:?}: expected HTTP {NOT_FOUND_STATUS} ResourceNotFoundException, got {status} with {body}"
    );
    for member in ["Message", "ResourceId", "ResourceType"] {
        let value = body[member].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must carry a {member}, got {body}")
        });
        assert!(
            value.chars().count() <= MEMBER_MAX_CHARS,
            "case {case:?}: {member} must satisfy the modelled @length(0, {MEMBER_MAX_CHARS}), got {} characters: {value:?}",
            value.chars().count()
        );
    }
    let reported = body["ResourceId"]
        .as_str()
        .expect("the refusal carries a ResourceId");
    if identifier.chars().count() <= MEMBER_MAX_CHARS {
        // Nothing had to give way, so the bound costs the client nothing.
        assert_eq!(
            reported, identifier,
            "case {case:?}: an identifier the member had room for must be reported whole, got {reported:?}"
        );
    } else {
        // What is reported is the identifier's own leading characters — what a client can
        // still match its request against — and not some other value.
        let kept = reported.trim_end_matches('.');
        assert!(
            !kept.is_empty() && identifier.starts_with(kept),
            "case {case:?}: ResourceId must report the identifier's leading characters, got {reported:?}"
        );
        assert!(
            kept.chars().count() + MARKER_SLACK >= MEMBER_MAX_CHARS,
            "case {case:?}: ResourceId must use the room the member has, got {} of {MEMBER_MAX_CHARS} characters",
            kept.chars().count()
        );
    }
}

/// Over every ARN-addressed not-found path, an outsized ARN is reported inside the
/// member's modelled bound, and one that fits is reported whole.
#[tokio::test]
async fn a_not_found_refusal_reporting_an_outsized_arn_stays_within_the_bound() {
    let client = Client::new();
    let export = cases().remove(0).create_request()["Export"].clone();

    for chars in [
        MAX_ARN_CHARS,
        MEMBER_MAX_CHARS + 1,
        MEMBER_MAX_CHARS,
        MEMBER_MAX_CHARS - 1,
        90,
    ] {
        let arn = bounded_arn(chars);
        for (operation, request) in requests(&arn, &export) {
            let case = format!("an ARN of {chars} characters / {operation}");
            let (status, body) = client.call(operation, &request).await;
            check_bounded_not_found(&case, &arn, status, &body);
        }
    }
}

/// The identifier a not-found reports is not always an ARN: `GetExecution` reports the
/// `ExecutionId` that named no execution of an export that DOES exist, and that member's
/// own ceiling is the refusal member's, so even the longest one is reported whole.
#[tokio::test]
async fn an_execution_id_at_its_ceiling_is_reported_whole() {
    let client = Client::new();
    let created = client
        .ok("CreateExport", &cases().remove(0).create_request())
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    for chars in [MAX_EXECUTION_ID_CHARS, MAX_EXECUTION_ID_CHARS - 1, 36] {
        let execution_id = "e".repeat(chars);
        let case = format!("an ExecutionId of {chars} characters");
        let (status, body) = client
            .call(
                "GetExecution",
                &json!({ "ExportArn": arn, "ExecutionId": execution_id }),
            )
            .await;
        check_bounded_not_found(&case, &execution_id, status, &body);
    }
}
