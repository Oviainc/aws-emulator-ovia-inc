// SPDX-License-Identifier: CC0-1.0

//! The three service-wide answer rules of PartnerCentral Benefits, over the wire.
//!
//! Each is a rule about EVERY operation, not about one of them, so each test below is a
//! TABLE spanning the class it covers; the calls the issue happened to name are single
//! rows among them.
//!
//! 1. **A refused input** is answered 400 `ValidationException` whose message is the AWS
//!    validation envelope (`N validation error(s) detected: `) over one clause per
//!    violation, and each clause names the MEMBER PATH and the VIOLATED CONSTRAINT only
//!    — never a rendering of the value that was rejected — with a `@pattern` requirement
//!    quoted WITHOUT the `^` / `$` anchors the model spells it with.
//! 2. **The partner-entitlement gate**: a request from an account that holds no active
//!    AWS partner benefit is refused `AccessDeniedException` with the status the service
//!    serves that shape with ([`DENIED_STATUS`]), on reads as well as writes, after the
//!    modelled constraints are checked and before any resource is looked up — on every
//!    operation the gate fronts, which per the recorded answers is every one except the
//!    two tag-MUTATING operations (see [`UNGATED_OPERATIONS`]).
//! 3. **A lookup that found nothing** is answered 404 `ResourceNotFoundException` whose
//!    message names the resource and the identifier the request named it with,
//!    backtick-quoted: ``<Resource> `<identifier>` not found``.
//!
//! Every constraint and pattern quoted here comes from the model, via
//! `smithy_query(query="operation",operation="...",
//! model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")`.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::service::constraint_envelope;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, APPLICATION_KIND, BENEFIT_KIND, CATALOG_KIND,
};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::benefit_application_lifecycle::{error_code, Harness};

/// The production catalog, one of the two the service serves.
const CATALOG: &str = "AWS";

/// The sentence the entitlement gate refuses with, written out here rather than read
/// from the service's own constant so the text itself is under test.
const ENTITLEMENT_MESSAGE: &str = "This action requires an active AWS partner benefit. \
Register as an AWS partner if you haven't already, or contact AWS partner central \
support.";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, which the ARNs below must carry.
const REGION: &str = "us-west-2";

/// `BenefitApplicationIdentifier`'s `@pattern`, with the anchors the model spells it
/// with stripped — which is exactly what a clause reporting it must state.
pub(crate) const APPLICATION_IDENTIFIER_PATTERN: &str = r"(arn:.+|benappl-[0-9a-z]{14})";

/// `BenefitAllocationIdentifier`'s `@pattern`, unanchored.
pub(crate) const ALLOCATION_IDENTIFIER_PATTERN: &str = r"(arn:.+|benalloc-[0-9a-z]{14})";

/// The `@pattern` on `GetBenefitInput$Identifier` / `BenefitId`, unanchored.
pub(crate) const BENEFIT_IDENTIFIER_PATTERN: &str = r"(arn:.+|ben-[0-9a-z]{14})";

/// `Arn`'s `@pattern`, unanchored — the shape `ResourceArn` targets.
pub(crate) const ARN_PATTERN: &str =
    r"arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)";

/// `TaggableResourceArn`'s `@pattern`, unanchored — the shape every tagging operation's
/// `resourceArn` targets.
pub(crate) const TAGGABLE_ARN_PATTERN: &str =
    r"arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*";

/// `CatalogName`'s `@pattern`, unanchored.
pub(crate) const CATALOG_NAME_PATTERN: &str = "[A-Za-z0-9_-]+";

/// An application id the modelled `BenefitApplicationIdentifier` admits and no create
/// ever minted, so every lookup of it finds nothing.
pub(crate) const UNKNOWN_APPLICATION: &str = "benappl-00000000000001";

/// A benefit id the modelled `BenefitId` admits and no catalog holds.
pub(crate) const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// An allocation id the modelled `BenefitAllocationIdentifier` admits.
pub(crate) const UNKNOWN_ALLOCATION: &str = "benalloc-00000000000001";

/// A `ClientToken` every `@pattern` on one admits (`^[!-~]{1,64}$` on create/update,
/// `^[a-zA-Z0-9-]{1,64}$` on cancel/amend).
pub(crate) const CLIENT_TOKEN: &str = "gate-probe-token";

/// A `TaggableResourceArn` the modelled pattern admits.
pub(crate) fn taggable_arn() -> String {
    format!(
        "arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{CATALOG}/benefit-application/{UNKNOWN_APPLICATION}"
    )
}

/// The status this service serves its `AccessDeniedException` with, written out here
/// rather than read from the service's own constant so the value itself is under test.
///
/// The model declares the shape `@httpError(403)`; the recorded answers (corpus
/// `simparity-wire-aws-v1`, fixture
/// `partnercentral-benefits-list-and-tag-validation-error-paths`, steps `task#1`
/// `ListBenefits` and `task#4` `ListTagsForResource`) carry 400, and the status a
/// service serves is settled by what it serves.
const DENIED_STATUS: u16 = 400;

/// The operations the entitlement gate does NOT front.
///
/// Recorded behaviour (corpus `simparity-behavioral-aws-v1`, units
/// `partnercentral-benefits.tag-resource-malformed-arn-rejected` and
/// `partnercentral-benefits.untag-resource-malformed-arn-rejected`): a well-formed
/// tag-MUTATING call naming a resource that does not exist is answered
/// ``("ResourceNotFoundException", "Catalog `null` not found")`` — the resource lookup's
/// own answer — from the same account whose `ListBenefits` and `CreateBenefitApplication`
/// calls record the gate's refusal; the wire corpus records the same asymmetry on one
/// fixture, whose `TagResource` step is answered `ResourceNotFoundException` while its
/// `ListTagsForResource` step is answered `AccessDeniedException`. So the two tag-writing
/// operations answer an un-entitled caller with their own logic, and the gate's table
/// below spans every other operation — the tag-READING one included.
const UNGATED_OPERATIONS: &[&str] = &["TagResource", "UntagResource"];

/// The envelope a single violation renders into.
pub(crate) fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One clause of the envelope: the member path, then the constraint it failed.
pub(crate) fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause a `@pattern` violation states.
pub(crate) fn pattern_clause(path: &str, pattern: &str) -> String {
    clause(
        path,
        &format!("Member must satisfy regular expression pattern: {pattern}"),
    )
}

/// The message an error body carries, read ONLY under the member name the MODEL declares
/// — `Message`, on this service's `ValidationException` as on every other error shape it
/// declares. The framework's own generic refusal envelope spells it lowercase `message`
/// and carries nothing else, so a refusal that travelled through that envelope rather
/// than through the modelled shape has no message as far as these tests are concerned,
/// which is exactly the defect they must see. The body itself is pinned in
/// `tests/all/validation_error_body.rs`.
pub(crate) fn message_of(response: &Value) -> Option<String> {
    response
        .get("Message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The invariants of rule 1, checked on every refusal in every table: the AWS envelope,
/// no rendering of the rejected value, and no pattern anchors.
pub(crate) fn assert_validation_invariants(case: &str, message: &str) {
    assert!(
        message.starts_with("1 validation error detected: ")
            || message
                .split_once(" validation errors detected: ")
                .is_some_and(|(count, _)| count.parse::<u32>().is_ok_and(|n| n > 1)),
        "case {case:?}: expected the AWS validation envelope, got {message:?}"
    );
    for clause in message
        .split_once(": ")
        .map(|(_, clauses)| clauses)
        .unwrap_or_default()
        .split("; ")
    {
        assert!(
            clause.starts_with("Value at '") || clause.starts_with("Value null at '"),
            "case {case:?}: a clause must name the member and nothing of the value, \
             got {clause:?}"
        );
        if let Some((_, pattern)) = clause.split_once("regular expression pattern: ") {
            assert_unanchored(case, pattern);
        }
    }
    assert!(
        !message.contains("Value '"),
        "case {case:?}: the message renders a rejected value, got {message:?}"
    );
}

/// A reported `@pattern` carries neither anchor: no leading `^`, and no trailing `$`
/// that is not escaped into a literal dollar sign.
fn assert_unanchored(case: &str, pattern: &str) {
    assert!(
        !pattern.starts_with('^'),
        "case {case:?}: a reported pattern must carry no start anchor, got {pattern:?}"
    );
    if let Some(body) = pattern.strip_suffix('$') {
        let backslashes = body.len() - body.trim_end_matches('\\').len();
        assert!(
            backslashes % 2 == 1,
            "case {case:?}: a reported pattern must carry no end anchor, got {pattern:?}"
        );
    }
}

/// Call `operation`, require the `expected_status` and `expected_code`, and return the
/// message the answer carried.
async fn refusal(
    harness: &Harness,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) -> String {
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        expected_code,
        "case {case:?}: expected {expected_code}, got {response:?}"
    );
    message_of(&response)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no message: {response:?}"))
}

/// One well-formed request per operation the service declares — nothing in any of them
/// violates a modelled constraint, so the only thing that can refuse them is the
/// entitlement gate, except on the tag-MUTATING path ([`UNGATED_OPERATIONS`]), which the
/// gate does not front and which answers its own resource lookup instead.
pub(crate) fn well_formed_requests() -> Vec<(&'static str, Value)> {
    vec![
        ("ListBenefits", json!({ "Catalog": CATALOG })),
        ("ListBenefitApplications", json!({ "Catalog": CATALOG })),
        ("ListBenefitAllocations", json!({ "Catalog": CATALOG })),
        (
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": UNKNOWN_BENEFIT,
                "Name": "a gate probe",
            }),
        ),
        (
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
        ),
        (
            "GetBenefit",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_BENEFIT }),
        ),
        (
            "GetBenefitAllocation",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_ALLOCATION }),
        ),
        (
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
        ),
        (
            "UpdateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": UNKNOWN_APPLICATION,
                "Revision": "1",
                "Name": "a gate probe",
            }),
        ),
        (
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": UNKNOWN_APPLICATION,
            }),
        ),
        (
            "RecallBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "Identifier": UNKNOWN_APPLICATION,
                "Reason": "a gate probe",
            }),
        ),
        (
            "AmendBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": UNKNOWN_APPLICATION,
                "Revision": "1",
                "AmendmentReason": "a gate probe",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "a gate probe" }],
            }),
        ),
        (
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": "arn:aws:s3:::gate-probe-bucket",
            }),
        ),
        (
            "DisassociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": "arn:aws:s3:::gate-probe-bucket",
            }),
        ),
        ("ListTagsForResource", json!({ "resourceArn": taggable_arn() })),
        (
            "TagResource",
            json!({
                "resourceArn": taggable_arn(),
                "tags": [{ "Key": "probe", "Value": "gate" }],
            }),
        ),
        (
            "UntagResource",
            json!({ "resourceArn": taggable_arn(), "tagKeys": ["probe"] }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: how a refused input is worded
// ---------------------------------------------------------------------------

/// Every `@pattern` and `@range` the service's inputs declare, on every operation that
/// declares one: the clause names the member and the constraint, the value is absent,
/// and the pattern is reported unanchored.
#[tokio::test]
async fn constraint_violations_name_the_member_and_the_constraint_only() {
    let harness = Harness::new();
    // (case, operation, body, the message the refusal must carry)
    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            // The recorded instance of the rule.
            "a malformed GetBenefitApplication identifier".to_string(),
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "not-a-valid-identifier!!" }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a GetBenefitApplication identifier of the wrong resource kind".to_string(),
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_BENEFIT }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a malformed GetBenefitAllocation identifier".to_string(),
            "GetBenefitAllocation",
            json!({ "Catalog": CATALOG, "Identifier": "benalloc-TOOSHORT" }),
            one(&pattern_clause("identifier", ALLOCATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a malformed GetBenefit identifier".to_string(),
            "GetBenefit",
            json!({ "Catalog": CATALOG, "Identifier": "ben-00000000000000x" }),
            one(&pattern_clause("identifier", BENEFIT_IDENTIFIER_PATTERN)),
        ),
        (
            "a malformed SubmitBenefitApplication identifier".to_string(),
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "benappl-" }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a malformed UpdateBenefitApplication identifier".to_string(),
            "UpdateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": "BENAPPL-00000000000001",
                "Revision": "1",
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a malformed CancelBenefitApplication identifier".to_string(),
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": "benappl 00000000000001",
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "an AssociateBenefitApplicationResource resource that is not an ARN".to_string(),
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": "not-an-arn",
            }),
            one(&pattern_clause("resourceArn", ARN_PATTERN)),
        ),
        (
            "a DisassociateBenefitApplicationResource resource in another partition".to_string(),
            "DisassociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": "arn:aws-cn:s3:::gate-probe-bucket",
            }),
            one(&pattern_clause("resourceArn", ARN_PATTERN)),
        ),
        (
            "a catalog outside the modelled CatalogName pattern".to_string(),
            "ListBenefits",
            json!({ "Catalog": "not a catalog!" }),
            one(&pattern_clause("catalog", CATALOG_NAME_PATTERN)),
        ),
        (
            // The recorded instance of the @range half of the rule.
            "maxResults one past ListBenefits' maximum".to_string(),
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 1000 }),
            one(&clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )),
        ),
        (
            "maxResults under ListBenefits' minimum".to_string(),
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 0 }),
            one(&clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        ),
        (
            "maxResults one past ListBenefitApplications' maximum".to_string(),
            "ListBenefitApplications",
            json!({ "Catalog": CATALOG, "MaxResults": 101 }),
            one(&clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )),
        ),
        (
            "maxResults one past ListBenefitAllocations' maximum".to_string(),
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "MaxResults": 101 }),
            one(&clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )),
        ),
        (
            "an empty nextToken, under the modelled 1-character minimum".to_string(),
            "ListBenefits",
            json!({ "Catalog": CATALOG, "NextToken": "" }),
            one(&clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            "a nextToken one character past the 2048 maximum".to_string(),
            "ListBenefitApplications",
            json!({ "Catalog": CATALOG, "NextToken": "a".repeat(2049) }),
            one(&clause(
                "nextToken",
                "Member must have length less than or equal to 2048",
            )),
        ),
        (
            "a required member absent is reported as the null it was".to_string(),
            "CreateBenefitApplication",
            json!({ "Catalog": CATALOG, "BenefitIdentifier": UNKNOWN_BENEFIT }),
            one(
                "Value null at 'clientToken' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a tag key past the modelled 128-character maximum".to_string(),
            "TagResource",
            json!({
                "resourceArn": taggable_arn(),
                "tags": [{ "Key": "k".repeat(129), "Value": "gate" }],
            }),
            one(&clause(
                "tags.1.member.key",
                "Member must have length less than or equal to 128",
            )),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let message = refusal(
            &harness,
            &case,
            operation,
            &body,
            400,
            "ValidationException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_validation_invariants(&case, &message);
    }
}

/// Two violations in one request render the plural envelope, with one clause each and
/// still nothing of either value.
#[tokio::test]
async fn several_violations_render_the_plural_envelope() {
    let harness = Harness::new();
    let case = "maxResults out of range and an empty nextToken";
    let message = refusal(
        &harness,
        case,
        "ListBenefits",
        &json!({ "Catalog": CATALOG, "MaxResults": 424_242, "NextToken": "" }),
        400,
        "ValidationException",
    )
    .await;
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural envelope, got {message:?}"
    );
    for expected in [
        clause(
            "maxResults",
            "Member must have value less than or equal to 100",
        ),
        clause(
            "nextToken",
            "Member must have length greater than or equal to 1",
        ),
    ] {
        assert!(
            message.contains(&expected),
            "case {case:?}: expected a clause {expected:?} in {message:?}"
        );
    }
    assert_validation_invariants(case, &message);
}

/// The point of the rule: whatever a caller sends, the refusal never hands it back.
/// Asserted over values that would be unmistakable if they were echoed.
#[tokio::test]
async fn a_refusal_never_echoes_the_rejected_value() {
    let harness = Harness::new();
    // (operation, body, the distinctive value that body carries)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "benappl-sensitivevalue!!" }),
            "sensitivevalue".to_string(),
        ),
        (
            "GetBenefitApplication",
            // A quote and a backslash: an echoed value would also have to be escaped
            // into the JSON envelope, which is a second way for it to show up.
            json!({ "Catalog": CATALOG, "Identifier": r#"not"a\identifier""# }),
            r#"not"a\identifier""#.to_string(),
        ),
        (
            "GetBenefitApplication",
            // Longer than any echo cap a value rendering could have used, so even a
            // truncated echo would show up.
            json!({ "Catalog": CATALOG, "Identifier": format!("benappl-{}", "+".repeat(400)) }),
            "+".repeat(64),
        ),
        (
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": "arn:private:customer-secret",
            }),
            "customer-secret".to_string(),
        ),
        (
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 424_242 }),
            "424242".to_string(),
        ),
        (
            "ListBenefits",
            json!({ "Catalog": CATALOG, "NextToken": "n".repeat(2049) }),
            "n".repeat(64),
        ),
        (
            "TagResource",
            json!({
                "resourceArn": taggable_arn(),
                "tags": [{ "Key": "k".repeat(129), "Value": "gate" }],
            }),
            "k".repeat(64),
        ),
    ];

    for (operation, body, secret) in rows {
        let case = format!("{operation} {body}");
        let message = refusal(
            &harness,
            &case,
            operation,
            &body,
            400,
            "ValidationException",
        )
        .await;
        assert!(
            !message.contains(&secret),
            "case {case:?}: the refusal echoes {secret:?}: {message:?}"
        );
        assert_validation_invariants(&case, &message);
    }
}

/// The renderer itself, over every constraint class the framework can report and every
/// `@pattern` this service's model declares: the clause for each class, and a pattern
/// reported with its anchors stripped and nothing else changed.
///
/// Reaching it directly is what lets the escaped-`$` case be covered — a model may spell
/// a pattern whose last character is a literal dollar sign, which is NOT an anchor.
#[test]
fn the_renderer_states_each_constraint_class_and_strips_only_the_anchors() {
    // (case, the violation, the clause it must render into)
    let rows: Vec<(&str, Violation, String)> = vec![
        (
            "a required member absent",
            violation("clientToken", ViolationKind::MissingRequiredMember),
            "Value null at 'clientToken' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            "under a @length minimum",
            violation("nextToken", ViolationKind::MinLength(1)),
            clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            ),
        ),
        (
            "past a @length maximum",
            violation("nextToken", ViolationKind::MaxLength(2048)),
            clause(
                "nextToken",
                "Member must have length less than or equal to 2048",
            ),
        ),
        (
            "under a @range minimum",
            violation("maxResults", ViolationKind::MinValue("1".to_string())),
            clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "past a @range maximum",
            violation("maxResults", ViolationKind::MaxValue("100".to_string())),
            clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            ),
        ),
        (
            "outside an enum's value set",
            violation(
                "status",
                ViolationKind::EnumValue(vec!["ACTIVE".to_string(), "INACTIVE".to_string()]),
            ),
            clause(
                "status",
                "Member must satisfy enum value set: [ACTIVE, INACTIVE]",
            ),
        ),
        (
            "a repeated element where @uniqueItems applies",
            violation("programs", ViolationKind::UniqueItems),
            clause("programs", "Member must have unique values"),
        ),
        (
            "BenefitApplicationIdentifier's modelled @pattern",
            violation(
                "identifier",
                ViolationKind::Pattern(r"^(arn:.+|benappl-[0-9a-z]{14})$".to_string()),
            ),
            pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN),
        ),
        (
            "BenefitId's modelled @pattern",
            violation(
                "benefitIdentifier",
                ViolationKind::Pattern(r"^(arn:.+|ben-[0-9a-z]{14})$".to_string()),
            ),
            pattern_clause("benefitIdentifier", BENEFIT_IDENTIFIER_PATTERN),
        ),
        (
            "BenefitAllocationIdentifier's modelled @pattern",
            violation(
                "identifier",
                ViolationKind::Pattern(r"^(arn:.+|benalloc-[0-9a-z]{14})$".to_string()),
            ),
            pattern_clause("identifier", ALLOCATION_IDENTIFIER_PATTERN),
        ),
        (
            "Arn's modelled @pattern",
            violation(
                "resourceArn",
                ViolationKind::Pattern(
                    r"^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$"
                        .to_string(),
                ),
            ),
            pattern_clause("resourceArn", ARN_PATTERN),
        ),
        (
            "CatalogName's modelled @pattern",
            violation(
                "catalog",
                ViolationKind::Pattern("^[A-Za-z0-9_-]+$".to_string()),
            ),
            pattern_clause("catalog", CATALOG_NAME_PATTERN),
        ),
        (
            "TaggableResourceArn's modelled @pattern",
            violation(
                "resourceArn",
                ViolationKind::Pattern(
                    r"^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$"
                        .to_string(),
                ),
            ),
            pattern_clause("resourceArn", TAGGABLE_ARN_PATTERN),
        ),
        (
            // A pattern carrying no anchors at all is quoted as it stands.
            "an unanchored pattern",
            violation(
                "program",
                ViolationKind::Pattern("[A-Za-z0-9_-]+".to_string()),
            ),
            pattern_clause("program", "[A-Za-z0-9_-]+"),
        ),
        (
            // An escaped `$` is a literal dollar sign, not the end anchor, so it stays.
            "a pattern ending in an escaped dollar sign",
            violation("amount", ViolationKind::Pattern(r"^[0-9]+\$$".to_string())),
            pattern_clause("amount", r"[0-9]+\$"),
        ),
        (
            "a pattern whose last character is an escaped backslash",
            violation("path", ViolationKind::Pattern(r"^[a-z]+\\$".to_string())),
            pattern_clause("path", r"[a-z]+\\"),
        ),
    ];

    for (case, violated, expected) in rows {
        let message = constraint_envelope(std::slice::from_ref(&violated))
            .unwrap_or_else(|| panic!("case {case:?}: the renderer declined {violated:?}"));
        assert_eq!(
            message,
            one(&expected),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_validation_invariants(case, &message);
    }
}

/// A violation as the framework hands it over, carrying the value the caller sent — the
/// rendering is what must drop it.
fn violation(path: &str, kind: ViolationKind) -> Violation {
    let value = match kind {
        ViolationKind::MissingRequiredMember => None,
        _ => Some("the-rejected-value".to_string()),
    };
    Violation {
        path: path.to_string(),
        kind,
        value,
        sensitive: false,
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the partner-entitlement gate
// ---------------------------------------------------------------------------

/// Every operation the entitlement gate fronts refuses a well-formed request from an
/// account with no active AWS partner benefit — reads exactly as writes — with
/// `AccessDeniedException`, [`DENIED_STATUS`] and the entitlement message; the
/// tag-MUTATING path ([`UNGATED_OPERATIONS`]) answers such a caller with its own logic
/// instead.
#[tokio::test]
async fn every_operation_refuses_a_caller_with_no_active_partner_benefit() {
    let harness = Harness::unentitled();
    for (operation, body) in well_formed_requests() {
        if UNGATED_OPERATIONS.contains(&operation) {
            let (status, response) = harness.call(operation, &body).await;
            assert_ne!(
                error_code(&response),
                "AccessDeniedException",
                "case {operation:?}: the tag-mutating path is not behind the gate, got \
                 {status} with {response:?}"
            );
            assert!(
                [404, 501].contains(&status),
                "case {operation:?}: expected the resource lookup's own answer (404), or the \
                 501 of a tagging operation not implemented yet, got {status} with {response:?}"
            );
            continue;
        }
        let message = refusal(
            &harness,
            operation,
            operation,
            &body,
            DENIED_STATUS,
            "AccessDeniedException",
        )
        .await;
        assert_eq!(
            message, ENTITLEMENT_MESSAGE,
            "case {operation:?}: expected the entitlement message, got {message:?}"
        );
    }
}

/// The other side of the gate: the SAME requests from an account that does hold an
/// active partner benefit are not refused as denied — they are served, or answered by the
/// operation's own logic (a lookup that found nothing, or the 501 of an operation this
/// emulator has not implemented yet).
#[tokio::test]
async fn an_entitled_account_is_not_refused_by_the_gate() {
    let harness = Harness::new();
    for (operation, body) in well_formed_requests() {
        let (status, response) = harness.call(operation, &body).await;
        assert_ne!(
            error_code(&response),
            "AccessDeniedException",
            "case {operation:?}: an entitled account must not be refused by the gate, \
             got {status} with {response:?}"
        );
        assert!(
            [200, 404, 409, 501].contains(&status),
            "case {operation:?}: expected the operation's own answer, got {status} with \
             {response:?}"
        );
    }
}

/// The gate's place in the order: BEHIND the modelled-constraint walk, so a malformed
/// request is a `ValidationException` whether the caller is entitled or not, and AHEAD
/// of every resource lookup, so an un-entitled caller is never told whether a resource
/// exists.
#[tokio::test]
async fn shape_validation_precedes_the_gate_and_the_gate_precedes_every_lookup() {
    let harness = Harness::unentitled();
    // Malformed requests: refused as validation failures even though the caller is not
    // entitled.
    let malformed: Vec<(&str, &str, Value)> = vec![
        (
            "a malformed identifier",
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "not-an-identifier!!" }),
        ),
        (
            "maxResults past the modelled maximum",
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 1000 }),
        ),
        (
            "an absent required member",
            "CreateBenefitApplication",
            json!({ "Catalog": CATALOG, "BenefitIdentifier": UNKNOWN_BENEFIT }),
        ),
        (
            "a catalog outside the modelled pattern",
            "ListBenefitApplications",
            json!({ "Catalog": "not a catalog!" }),
        ),
    ];
    for (case, operation, body) in malformed {
        let message = refusal(&harness, case, operation, &body, 400, "ValidationException").await;
        assert_validation_invariants(case, &message);
    }

    // Requests naming resources the service does not hold: the gate answers first, so
    // the 404 never reaches the caller.
    let absent: Vec<(&str, &str, Value)> = vec![
        (
            "a catalog the service does not serve",
            "ListBenefits",
            json!({ "Catalog": "NotACatalog" }),
        ),
        (
            "an application no create minted",
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
        ),
        (
            "a benefit no catalog holds",
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": UNKNOWN_BENEFIT,
            }),
        ),
    ];
    for (case, operation, body) in absent {
        let message =
            refusal(&harness, case, operation, &body, DENIED_STATUS, "AccessDeniedException").await;
        assert_eq!(
            message, ENTITLEMENT_MESSAGE,
            "case {case:?}: expected the entitlement message, got {message:?}"
        );
    }
}

/// The gate is a read of state, not a write: a refused request changes nothing, so the
/// same call refused twice is refused identically, and an entitled caller afterwards is
/// still served.
#[tokio::test]
async fn a_refused_request_leaves_the_store_untouched() {
    let harness = Harness::unentitled();
    let body = json!({
        "Catalog": CATALOG,
        "ClientToken": CLIENT_TOKEN,
        "BenefitIdentifier": UNKNOWN_BENEFIT,
    });
    for attempt in 1..=3 {
        let case = format!("CreateBenefitApplication attempt {attempt}");
        let message = refusal(
            &harness,
            &case,
            "CreateBenefitApplication",
            &body,
            DENIED_STATUS,
            "AccessDeniedException",
        )
        .await;
        assert_eq!(
            message, ENTITLEMENT_MESSAGE,
            "case {case:?}: expected the entitlement message, got {message:?}"
        );
    }
    // And nothing was created: an entitled emulator's catalog still lists no application
    // for that client token.
    let entitled = Harness::new();
    let (status, response) = entitled
        .call("ListBenefitApplications", &json!({ "Catalog": CATALOG }))
        .await;
    assert_eq!(
        status, 200,
        "an entitled ListBenefitApplications must be served, got {status} with {response:?}"
    );
    let summaries = response["BenefitApplicationSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        summaries, 0,
        "a refused create must store nothing, got {response:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: how a lookup that found nothing is worded
// ---------------------------------------------------------------------------

/// Every lookup this service does: the 404 names the resource and the identifier the
/// request named it with, backtick-quoted, whichever spelling the modelled identifier
/// pattern admits.
#[tokio::test]
async fn a_lookup_that_found_nothing_names_the_resource_and_the_identifier() {
    let harness = Harness::new();
    let unknown_catalog = "not-a-real-catalog-34556872";
    let application_arn = format!(
        "arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{CATALOG}/benefit-application/{UNKNOWN_APPLICATION}"
    );
    let benefit_arn =
        format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{CATALOG}/benefit/{UNKNOWN_BENEFIT}");
    // (case, operation, body, the resource the message must name, the identifier it
    // must quote)
    let rows: Vec<(String, &str, Value, &str, String)> = vec![
        (
            // The recorded instance of the rule.
            "a catalog that names none, on a create".to_string(),
            "CreateBenefitApplication",
            json!({
                "Catalog": unknown_catalog,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": UNKNOWN_BENEFIT,
                "Name": "x",
            }),
            CATALOG_KIND,
            unknown_catalog.to_string(),
        ),
        (
            "a catalog that names none, on ListBenefits".to_string(),
            "ListBenefits",
            json!({ "Catalog": unknown_catalog }),
            CATALOG_KIND,
            unknown_catalog.to_string(),
        ),
        (
            "a catalog that names none, on ListBenefitApplications".to_string(),
            "ListBenefitApplications",
            json!({ "Catalog": unknown_catalog }),
            CATALOG_KIND,
            unknown_catalog.to_string(),
        ),
        (
            "a benefit no catalog holds, by id".to_string(),
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": UNKNOWN_BENEFIT,
            }),
            BENEFIT_KIND,
            UNKNOWN_BENEFIT.to_string(),
        ),
        (
            "a benefit no catalog holds, by ARN".to_string(),
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": "gate-probe-token-2",
                "BenefitIdentifier": benefit_arn.clone(),
            }),
            BENEFIT_KIND,
            benefit_arn,
        ),
        (
            "an application no create minted, on a read".to_string(),
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
            APPLICATION_KIND,
            UNKNOWN_APPLICATION.to_string(),
        ),
        (
            "an application named by an ARN no create minted".to_string(),
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": application_arn.clone() }),
            APPLICATION_KIND,
            application_arn,
        ),
        (
            "an application no create minted, on a submit".to_string(),
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
            APPLICATION_KIND,
            UNKNOWN_APPLICATION.to_string(),
        ),
        (
            "an application no create minted, on an update".to_string(),
            "UpdateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": UNKNOWN_APPLICATION,
                "Revision": "1",
            }),
            APPLICATION_KIND,
            UNKNOWN_APPLICATION.to_string(),
        ),
        (
            "an application no create minted, on a cancel".to_string(),
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": UNKNOWN_APPLICATION,
            }),
            APPLICATION_KIND,
            UNKNOWN_APPLICATION.to_string(),
        ),
    ];

    for (case, operation, body, resource, identifier) in rows {
        let message = refusal(
            &harness,
            &case,
            operation,
            &body,
            404,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_message(resource, &identifier);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("`{identifier}`")),
            "case {case:?}: the message must quote the identifier it was given, \
             got {message:?}"
        );
    }
}
