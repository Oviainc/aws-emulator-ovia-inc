// SPDX-License-Identifier: CC0-1.0

//! The member path a refusal of this service names, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`.
//!
//! The rule under test, stated over the class of refusals it covers:
//!
//! **A refusal of an input member names that member by the path the centralized constraint
//! walk reports for it — the member in lowerCamel, followed by the steps a handler
//! addressed below it — in the clause of the AWS validation envelope and in
//! `ValidationExceptionField$Name` alike.** So one input member has ONE path whichever
//! bound it broke: a constraint the model declares, which the centralized walk enforces, or
//! a rule a handler enforces itself.
//!
//! Two tables span what the rule generalises over:
//!
//! 1. One member, two bounds: the same member refused once by the walk and once by a
//!    handler must be named the same way. Rows: the cursor member of all three paginated
//!    listings (its modelled `@length` on both sides of the bound, against the cursor rule
//!    the handler enforces) and the benefit member of `CreateBenefitApplication` (its
//!    modelled `@pattern`, against the status rule the handler enforces).
//! 2. Every refusal a HANDLER of this service decides about a member: the envelope over the
//!    clause for that path, and the one-member `FieldList` naming it. The rows cross the
//!    case the WIRE spells the member in (`NextToken`, `Tags` and
//!    `BenefitApplicationDetails` in PascalCase; `tags` and `tagKeys` already in
//!    lowerCamel), the operation deciding the refusal, the kind of rule it enforces (a
//!    cursor this service never issued, the reserved tag namespace, a document guard, a
//!    benefit the catalog no longer offers) and the shape of the path (the member alone, and
//!    a member reached below it inside a collection element).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    encode_page_token, ALLOCATION_ID_PREFIX, APPLICATION_ID_PREFIX, BENEFIT_ID_PREFIX, ID_BODY_LEN,
    MAX_DETAILS_DEPTH, RESERVED_TAG_KEY_PREFIX, VALIDATION_CODE_INVALID_VALUE,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::member_refusal_one_construction::withdrawn_benefit;
use crate::reserved_tag_keys::{element_requirement, tag_key_path};
use crate::resource_tagging::tag_payload;
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The modelled maximum of the cursor member (`ListBenefitsInput$NextToken`
/// `@length(min: 1, max: 2048)`, `smithy_query(query="shape",shape="ListBenefitsInput")`),
/// so a row can break the bound from above as well as from below.
const NEXT_TOKEN_MAX: usize = 2048;

/// A tag key in the AWS-reserved namespace, which no caller may supply.
const RESERVED_KEY: &str = "aws:createdBy";

/// `wire_member` as `smithy_schema_server::constraints::path` spells an input member in a
/// validation path: the member with its first character lowercased.
///
/// Written out here rather than read from the service, so a table row cannot adopt a second
/// spelling of a member and still pass.
fn walk_spelling(wire_member: &str) -> String {
    let mut characters = wire_member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The member path the one clause of `message` names: what it states between `Value at '`
/// and the closing quote.
fn stated_path(case: &str, message: &str) -> String {
    message
        .split_once("Value at '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(path, _)| path.to_string())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the refusal states no member clause: {message:?}")
        })
}

/// Call `operation` with `body`, require the modelled `ValidationException` (400) and
/// return the message it carried.
async fn refusal_message(harness: &Harness, case: &str, operation: &str, body: &Value) -> String {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse the request with 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {case:?}: expected ValidationException from {operation}, got {out:?}"
    );
    let message = message_of(&out).unwrap_or_else(|| {
        panic!("case {case:?}: the refusal carries no modelled Message member: {out:?}")
    });
    assert_validation_invariants(case, &message);
    message
}

/// A cursor well formed under this service's own encoding and naming a record no catalog
/// holds, so it is a token this service never issued.
fn unissued_cursor(catalog: &str, prefix: &str) -> String {
    encode_page_token(catalog, &format!("{prefix}{}", "0".repeat(ID_BODY_LEN)))
}

// ---------------------------------------------------------------------------
// Table 1: one member, two bounds, one path
// ---------------------------------------------------------------------------

/// One member a request can break two ways: the call the centralized walk refuses, and the
/// call a handler refuses, both of which must name `path`.
struct Paired {
    case: String,
    operation: &'static str,
    /// The member as the WIRE spells it.
    wire_member: &'static str,
    /// The path both refusals must state.
    path: &'static str,
    /// A request refused by a constraint the MODEL declares on that member.
    by_the_walk: Value,
    /// A request refused by a rule the HANDLER enforces on the same member.
    by_the_handler: Value,
}

/// The same input member refused by the centralized walk and by a handler is named by the
/// same path in both answers.
#[tokio::test]
async fn one_member_is_named_one_path_whichever_bound_it_broke() {
    let harness = Harness::new();
    let (withdrawn_catalog, withdrawn_id, _) = withdrawn_benefit(&harness).await;
    let active_benefit = text("subject", &harness.active_benefit().await, "Id");

    let mut rows: Vec<Paired> = Vec::new();
    // The cursor member of every paginated listing, on both sides of its modelled length:
    // too short (an empty token) and too long (one character past the maximum).
    for (operation, prefix) in [
        ("ListBenefits", BENEFIT_ID_PREFIX),
        ("ListBenefitApplications", APPLICATION_ID_PREFIX),
        ("ListBenefitAllocations", ALLOCATION_ID_PREFIX),
    ] {
        for (bound, token) in [
            ("under the modelled minimum", String::new()),
            ("past the modelled maximum", "t".repeat(NEXT_TOKEN_MAX + 1)),
        ] {
            rows.push(Paired {
                case: format!("{operation}: a cursor {bound}, and one never issued"),
                operation,
                wire_member: "NextToken",
                path: "nextToken",
                by_the_walk: json!({ "Catalog": CATALOG, "NextToken": token }),
                by_the_handler: json!({
                    "Catalog": CATALOG,
                    "NextToken": unissued_cursor(CATALOG, prefix),
                }),
            });
        }
    }
    // The benefit member of a create: a value outside its modelled pattern, and a value
    // inside it naming a benefit the catalog no longer offers.
    rows.push(Paired {
        case: "CreateBenefitApplication: a malformed benefit, and a withdrawn one".to_string(),
        operation: "CreateBenefitApplication",
        wire_member: "BenefitIdentifier",
        path: "benefitIdentifier",
        by_the_walk: json!({
            "Catalog": CATALOG,
            "ClientToken": "tok-path-walk-benefit",
            "BenefitIdentifier": "not-a-benefit-identifier",
            "Name": "member-path",
        }),
        by_the_handler: json!({
            "Catalog": withdrawn_catalog,
            "ClientToken": "tok-path-handler-benefit",
            "BenefitIdentifier": withdrawn_id,
            "Name": "member-path",
        }),
    });

    for row in &rows {
        let case = &row.case;
        assert_eq!(
            row.path,
            walk_spelling(row.wire_member),
            "case {case:?}: the row's own path must be the wire member in lowerCamel, got {:?}",
            row.path
        );
        for (decided_by, body) in [
            ("the centralized walk", &row.by_the_walk),
            ("the handler", &row.by_the_handler),
        ] {
            let case = format!("{case} ({decided_by})");
            let message = refusal_message(&harness, &case, row.operation, body).await;
            assert_eq!(
                stated_path(&case, &message),
                row.path,
                "case {case:?}: expected the clause to name {:?}, got {message:?}",
                row.path
            );
        }
    }

    // The benefit the other rows apply for is still served, so the refusals above are
    // refusals of the member each row names and not of every request.
    harness
        .create(
            "a create the rules admit",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-path-admitted",
                "BenefitIdentifier": active_benefit,
                "Name": "member-path",
            }),
        )
        .await;
}

// ---------------------------------------------------------------------------
// Table 2: every refusal a handler decides
// ---------------------------------------------------------------------------

/// One refusal a handler of this service decides about an input member.
struct Row {
    case: String,
    operation: &'static str,
    /// The member as the WIRE spells it.
    wire_member: &'static str,
    /// The path the refusal must state, in the clause and in the `FieldList`: the member in
    /// lowerCamel, plus any steps the handler addressed below it.
    path: String,
    /// The requirement that member failed, as the refusal must word it.
    requirement: String,
    body: Value,
}

/// A details document nesting one level past the depth this service stores, which the
/// handler's document guard refuses.
fn too_deep_details() -> Value {
    (0..=MAX_DETAILS_DEPTH).fold(json!("leaf"), |inner, _| json!({ "n": inner }))
}

/// Every refusal a handler decides about a member states the AWS validation envelope over
/// the clause for the path the walk reports for that member, and names the same path in its
/// one-member `FieldList`.
#[tokio::test]
async fn an_in_handler_refusal_names_the_walks_path_under_the_envelope() {
    let harness = Harness::new();
    let (withdrawn_catalog, withdrawn_id, withdrawn_status) = withdrawn_benefit(&harness).await;
    let active_benefit = text("subject", &harness.active_benefit().await, "Id");
    let subject = harness
        .create(
            "subject",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-member-path-subject",
                "BenefitIdentifier": active_benefit,
                "Name": "member-path",
            }),
        )
        .await;
    let subject_arn = text("subject", &subject, "Arn");
    let reserved_requirement =
        format!("Member must not use the reserved tag key prefix: {RESERVED_TAG_KEY_PREFIX}");

    let mut rows: Vec<Row> = Vec::new();
    // A cursor this service never issued, on every paginated listing: the listing that
    // issues the cursors is the one the requirement names.
    for (operation, prefix) in [
        ("ListBenefits", BENEFIT_ID_PREFIX),
        ("ListBenefitApplications", APPLICATION_ID_PREFIX),
        ("ListBenefitAllocations", ALLOCATION_ID_PREFIX),
    ] {
        rows.push(Row {
            case: format!("{operation}: a cursor it never issued"),
            operation,
            wire_member: "NextToken",
            path: "nextToken".to_string(),
            requirement: format!("Member must be a pagination token issued by {operation}"),
            body: json!({
                "Catalog": CATALOG,
                "NextToken": unissued_cursor(CATALOG, prefix),
            }),
        });
    }
    rows.push(Row {
        case: "CreateBenefitApplication: a reserved tag key".to_string(),
        operation: "CreateBenefitApplication",
        wire_member: "Tags",
        // The key sits in the `Key` member of one element of the list, which keeps its own
        // full path below the member.
        path: tag_key_path("tags", 1),
        requirement: reserved_requirement.clone(),
        body: json!({
            "Catalog": CATALOG,
            "ClientToken": "tok-member-path-create-tags",
            "BenefitIdentifier": active_benefit,
            "Name": "member-path",
            "Tags": tag_payload(&[(RESERVED_KEY, "mine")]),
        }),
    });
    rows.push(Row {
        case: "TagResource: a reserved tag key".to_string(),
        operation: "TagResource",
        wire_member: "tags",
        path: tag_key_path("tags", 1),
        requirement: reserved_requirement.clone(),
        body: json!({
            "resourceArn": subject_arn,
            "tags": tag_payload(&[(RESERVED_KEY, "mine")]),
        }),
    });
    rows.push(Row {
        case: "UntagResource: a reserved tag key".to_string(),
        operation: "UntagResource",
        wire_member: "tagKeys",
        // Here the refused key IS an element of the member, so the path collapses onto the
        // member and the requirement is the element's requirement set.
        path: "tagKeys".to_string(),
        requirement: element_requirement(&reserved_requirement),
        body: json!({ "resourceArn": subject_arn, "tagKeys": [RESERVED_KEY] }),
    });
    rows.push(Row {
        case: "CreateBenefitApplication: a document past the depth bound".to_string(),
        operation: "CreateBenefitApplication",
        wire_member: "BenefitApplicationDetails",
        path: "benefitApplicationDetails".to_string(),
        requirement: format!("Member must nest no deeper than {MAX_DETAILS_DEPTH} levels"),
        body: json!({
            "Catalog": CATALOG,
            "ClientToken": "tok-member-path-create-details",
            "BenefitIdentifier": active_benefit,
            "Name": "member-path",
            "BenefitApplicationDetails": too_deep_details(),
        }),
    });
    rows.push(Row {
        case: format!("CreateBenefitApplication: a {withdrawn_status} benefit"),
        operation: "CreateBenefitApplication",
        wire_member: "BenefitIdentifier",
        path: "benefitIdentifier".to_string(),
        requirement: format!(
            "Member must name a benefit the catalog offers, which a {withdrawn_status} benefit \
             is not"
        ),
        body: json!({
            "Catalog": withdrawn_catalog,
            "ClientToken": "tok-member-path-withdrawn",
            "BenefitIdentifier": withdrawn_id,
            "Name": "member-path",
        }),
    });

    // The table covers a member the wire spells in PascalCase and one it already spells in
    // lowerCamel, so it cannot shrink to the casing of one of them.
    for spelled_as_the_path in [false, true] {
        assert!(
            rows.iter()
                .any(|row| (row.wire_member == row.path) == spelled_as_the_path),
            "case \"table\": expected a row whose wire member is spelled as its path: \
             {spelled_as_the_path}, got {:?}",
            rows.iter().map(|row| row.wire_member).collect::<Vec<_>>()
        );
    }
    // And a path that carries steps below the input member, so it cannot shrink to the
    // top-level members either.
    assert!(
        rows.iter().any(|row| row.path.contains('.')),
        "case \"table\": expected a row whose path carries steps below the member, got {:?}",
        rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>()
    );

    for row in &rows {
        let case = &row.case;
        let named_member = row.path.split('.').next().unwrap_or_default();
        assert_eq!(
            named_member,
            walk_spelling(row.wire_member),
            "case {case:?}: the row's own path must open with the wire member in lowerCamel, \
             got {:?}",
            row.path
        );
        let (status, out) = harness.call(row.operation, &row.body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {} to refuse the request with 400, got {status} {out:?}",
            row.operation
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        let message = message_of(&out).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal carries no modelled Message member: {out:?}")
        });
        assert_eq!(
            message,
            one(&clause(&row.path, &row.requirement)),
            "case {case:?}: expected the envelope over the clause for {:?}, got {message:?}",
            row.path
        );
        assert_validation_invariants(case, &message);
        let field = out["FieldList"]
            .as_array()
            .and_then(|fields| fields.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            field["Name"],
            json!(row.path),
            "case {case:?}: the refusal's FieldList must name {:?}, got {out:?}",
            row.path
        );
        assert_eq!(
            field["Message"],
            json!(row.requirement),
            "case {case:?}: the FieldList must state the requirement, got {out:?}"
        );
        assert_eq!(
            field["Code"],
            json!(VALIDATION_CODE_INVALID_VALUE),
            "case {case:?}: the FieldList must carry the invalid-value code, got {out:?}"
        );
    }
}
