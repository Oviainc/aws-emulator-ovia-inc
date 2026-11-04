// SPDX-License-Identifier: CC0-1.0

//! The WORDS GuardDuty refuses a request with.
//!
//! A refusal's message is part of the contract a client codes against — it is
//! snapshotted, branched on and scraped — so each class of refusal has ONE text, and
//! each text is reproduced as the service words it. The rules under test, each
//! asserted over a SET of inputs rather than over one recorded call:
//!
//! 1. A value outside a constraint the model declares carries one FIXED sentence for
//!    its class: a value the service could not read as the member's modelled type
//!    (anything outside an enumerated value set) is refused as a payload it could not
//!    process; every other unacceptable value gets the generic invalid-input
//!    sentence. Neither names the member, interpolates the bound, or echoes the value.
//! 2. A criteria structure the service will not accept is refused as an invalid value
//!    of the MEMBER the request carried it in (`findingCriteria`, `sortCriteria`),
//!    with nothing from inside the structure echoed.
//! 3. A value that cannot be an account id at all is a FORMAT refusal worded in terms
//!    of the parameter that carried it: the LIST, where the request names one, and the
//!    `accountId` member where the id sits inside a structure.
//! 4. A resource a refusal is ABOUT, named by an identifier the request supplied and
//!    by nothing else, is identified in the text: the rule an association conflicts on
//!    or the rule a lookup does not find.
//! 5. A conflict over a resource the service protects is worded in terms of that
//!    resource, in one fixed sentence that quotes no name back.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, the modelled-constraint walk
//! and the handlers together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    as_delegated_admin, call, call_in, enabled_detector, error_code, error_shape, new_router,
    published_rule_ids,
};

/// The only client error GuardDuty's model binds to an operation, and the shape
/// restJson1 resolves every 400 of this service to (the `x-amzn-errortype` header).
const ENVELOPE: &str = "BadRequestException";

/// The error class a refusal a HANDLER made carries in the body.
const INVALID_INPUT: &str = "InvalidInputException";

/// The two fixed texts a refusal the request BOUNDARY made carries, written out here
/// rather than read from the service so the test pins the words themselves.
const INVALID_INPUT_PARAMETERS: &str =
    "The request is rejected because the one or more input parameters have invalid values.";
/// See [`INVALID_INPUT_PARAMETERS`].
const UNPROCESSABLE_PAYLOAD: &str =
    "The request is rejected because the JSON could not be processed.";

/// `AccountDetail.AccountId` is a member of its own, so its format refusal names it
/// (`smithy_query(shape="AccountDetail")`).
const ACCOUNT_ID_FORMAT: &str = "The request failed because the 'accountId' was formatted incorrectly.";

/// `CreateCustomDetectionRuleAssociation` and `ListCustomDetectionRuleAssociations`
/// are bound at `/custom-detection-rule/association`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The read of an association is
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// `CreateMalwareProtectionPlan` is `POST /malware-protection-plan`
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`).
const PLANS_PATH: &str = "/malware-protection-plan";

/// An IAM role ARN of the form `CreateMalwareProtectionPlan.Role` takes.
const PLAN_ROLE: &str = "arn:aws:iam::000000000000:role/GuardDutyMalwareProtectionRole";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The message a LIST of account ids carrying a value that cannot be one is refused
/// with, naming the list `member` the request sent.
fn account_ids_format(member: &str) -> String {
    format!(
        "The request failed because parameter {member} has accountId values that are formatted incorrectly."
    )
}

/// The message a criteria structure the service will not accept is refused with,
/// naming the `member` the request carried it in.
fn invalid_parameter_value(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has an invalid value.")
}

/// One request the modelled-constraint boundary refuses, and what its answer must
/// say.
struct BoundaryRow {
    case: String,
    method: &'static str,
    path: String,
    body: Value,
    /// The value the request sent, which the refusal must not echo.
    sent: String,
    /// The member it sat on, which the refusal must not name either.
    member: &'static str,
    /// The fixed sentence the refusal's class carries.
    expected: &'static str,
}

/// A `FindingCriteria` keyed by `field`.
///
/// The value is one the severity scale admits, because the rows below key the
/// criteria by `severity` as their BASELINE — a criterion asserting a field equals a
/// value that field cannot take is itself refused, which is the
/// `criterion_value_domain` suite's rule and not this suite's.
fn criteria(field: &str) -> Value {
    json!({"criterion": {field: {"equals": ["5"]}}})
}

/// Assert the status, the error shape and the exact message of one refusal.
fn assert_refusal(case: &str, status: u16, body: &Value, expected_status: u16, expected: &str) {
    assert_eq!(
        status, expected_status,
        "case {case}: expected HTTP {expected_status}, got {status} {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected),
        "case {case}: expected the refusal text {expected:?}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: one fixed text per class of modelled-constraint refusal.
// ---------------------------------------------------------------------------

/// Rows span the constraint KINDS the model uses and the operations and members they
/// sit on: `@range` on an integer, `@length` on a string and on a map, and an
/// enumerated value set — each with the value that broke it, so the assertions below
/// can check the text echoes none of them.
///
/// `CreateFilter.Rank` is `@range` 1..100, `Name -> FilterName` `@length` 3..64 and
/// `Action -> FilterAction` the enum ARCHIVE | NOOP;
/// `UpdateFindingsFeedback.Feedback -> Feedback` is the enum USEFUL | NOT_USEFUL;
/// `ListFindings.MaxResults` is `@range` 1..50; `CreateDetector.Tags -> TagMap` is
/// `@length` 1..200
/// (`smithy_query(operation="CreateFilter,UpdateFindingsFeedback,ListFindings,CreateDetector")`).
#[tokio::test]
async fn a_modelled_constraint_refusal_carries_one_fixed_text_for_its_class() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let rows: Vec<BoundaryRow> = vec![
        BoundaryRow {
            case: "CreateFilter with rank 0, one below the published minimum".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({"name": "rank-low", "findingCriteria": criteria("severity"), "rank": 0}),
            sent: "0".to_string(),
            member: "rank",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateFilter with rank 101, one above the published maximum".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({"name": "rank-high", "findingCriteria": criteria("severity"), "rank": 101}),
            sent: "101".to_string(),
            member: "rank",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateFilter with rank 4242, far above the published maximum".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({"name": "rank-far", "findingCriteria": criteria("severity"), "rank": 4242}),
            sent: "4242".to_string(),
            member: "rank",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateFilter with a name of 2 characters, one below the published minimum"
                .to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({"name": "ab", "findingCriteria": criteria("severity")}),
            sent: "ab".to_string(),
            member: "name",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateFilter with a name of 65 characters, one above the published maximum"
                .to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({"name": "z".repeat(65), "findingCriteria": criteria("severity")}),
            sent: "z".repeat(65),
            member: "name",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "ListFindings with maxResults 51, one above the published maximum".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/findings"),
            body: json!({"maxResults": 51}),
            sent: "51".to_string(),
            member: "maxResults",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateDetector with an empty tags map, below the published minimum of 1"
                .to_string(),
            method: "POST",
            path: "/detector".to_string(),
            body: json!({"enable": true, "tags": {}}),
            // Nothing of an empty map can be echoed, so the member name stands in.
            sent: "tags".to_string(),
            member: "tags",
            expected: INVALID_INPUT_PARAMETERS,
        },
        BoundaryRow {
            case: "CreateFilter with an action outside FilterAction".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/filter"),
            body: json!({
                "name": "action-row",
                "findingCriteria": criteria("severity"),
                "action": "ARCHIVE_AND_PAGE",
            }),
            sent: "ARCHIVE_AND_PAGE".to_string(),
            member: "action",
            expected: UNPROCESSABLE_PAYLOAD,
        },
        BoundaryRow {
            case: "UpdateFindingsFeedback with a feedback outside the Feedback enum".to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/findings/feedback"),
            body: json!({"findingIds": [], "feedback": "SOMEWHAT_USEFUL"}),
            sent: "SOMEWHAT_USEFUL".to_string(),
            member: "feedback",
            expected: UNPROCESSABLE_PAYLOAD,
        },
        BoundaryRow {
            case: "UpdateFindingsFeedback with a feedback member of the enum in the wrong case"
                .to_string(),
            method: "POST",
            path: format!("/detector/{detector_id}/findings/feedback"),
            body: json!({"findingIds": [], "feedback": "useful"}),
            sent: "useful".to_string(),
            member: "feedback",
            expected: UNPROCESSABLE_PAYLOAD,
        },
    ];

    for row in rows {
        let BoundaryRow {
            case,
            method,
            path,
            body,
            sent,
            member,
            expected,
        } = row;
        let (status, response) = call(&router, method, &path, Some(body)).await;
        assert_refusal(&case, status, &response, 400, expected);
        assert_eq!(
            error_shape(&response),
            ENVELOPE,
            "case {case}: expected the modelled envelope shape, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        // The class is stated and nothing else: no member, no bound, no value. The
        // bound is published in the model the caller built the request from.
        assert!(
            !message.contains(member),
            "case {case}: a boundary refusal must not name the member, got {message:?}"
        );
        assert!(
            !message.contains(sent.as_str()),
            "case {case}: a boundary refusal must not echo the caller's value, got {message:?}"
        );
        assert!(
            !message.chars().any(|c| c.is_ascii_digit()),
            "case {case}: a boundary refusal must interpolate no bound, got {message:?}"
        );
    }

    // The complement: on the published bound, and on each member of each enum, the
    // same requests are served.
    for (case, path, body) in [
        (
            "rank 1, the published minimum",
            format!("/detector/{detector_id}/filter"),
            json!({"name": "rank-min", "findingCriteria": criteria("severity"), "rank": 1}),
        ),
        (
            "rank 100, the published maximum",
            format!("/detector/{detector_id}/filter"),
            json!({"name": "rank-max", "findingCriteria": criteria("severity"), "rank": 100}),
        ),
        (
            "action ARCHIVE, a member of the enum",
            format!("/detector/{detector_id}/filter"),
            json!({"name": "action-ok", "findingCriteria": criteria("severity"), "action": "ARCHIVE"}),
        ),
        (
            "feedback NOT_USEFUL, a member of the enum",
            format!("/detector/{detector_id}/findings/feedback"),
            json!({"findingIds": [], "feedback": "NOT_USEFUL"}),
        ),
    ] {
        let (status, response) = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case}: a value inside the constraint must be served, got {status} {response:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a criteria structure is refused under the member that carried it.
// ---------------------------------------------------------------------------

/// Every operation that takes a `FindingCriteria` or a `SortCriteria` refuses a name
/// that is no field of a finding with the invalid-value text for the MEMBER the
/// request carried, and echoes nothing of the name.
///
/// The rows span how the name fails to be a field — an unknown top-level name, an
/// unknown member of a sub-document, a document rather than a field in it, a casing
/// variant, a path extending a scalar and a dotted name of several segments — and span
/// the operations and the two members, because the rule is about the member a refusal
/// names and not about one call.
#[tokio::test]
async fn a_criteria_member_the_service_will_not_accept_names_that_member() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // A filter to update, so `UpdateFilter` is exercised on one that exists.
    let (status, created) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({"name": "criteria.baseline", "findingCriteria": criteria("severity")})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the baseline filter: expected 200, got {status} {created:?}"
    );

    // Names that are no field of a finding. None is the name the recorded call used:
    // the rule is about the CLASS of name, not about one of them.
    let non_fields: [(&str, &str); 6] = [
        ("notAFindingField", "an unknown top-level name"),
        ("service.actionType", "no member of the service document"),
        (
            "resource.instanceDetails",
            "a detail document rather than a field in it",
        ),
        ("Severity", "a top-level field in the wrong case"),
        (
            "service.archived.value",
            "a path extending a scalar that has nothing beneath it",
        ),
        (
            "a.deeply.nested.name.of.many.segments",
            "a dotted name no document carries",
        ),
    ];

    for (index, (name, why)) in non_fields.into_iter().enumerate() {
        let criterion_attempts = [
            (
                "CreateFilter",
                "POST",
                format!("/detector/{detector_id}/filter"),
                json!({"name": format!("refused-{index}"), "findingCriteria": criteria(name)}),
            ),
            (
                "UpdateFilter",
                "POST",
                format!("/detector/{detector_id}/filter/criteria.baseline"),
                json!({"findingCriteria": criteria(name)}),
            ),
            (
                "ListFindings",
                "POST",
                format!("/detector/{detector_id}/findings"),
                json!({"findingCriteria": criteria(name)}),
            ),
            (
                "GetFindingsStatistics",
                "POST",
                format!("/detector/{detector_id}/findings/statistics"),
                json!({
                    "findingStatisticTypes": ["COUNT_BY_SEVERITY"],
                    "findingCriteria": criteria(name),
                }),
            ),
        ];
        for (operation, method, path, body) in criterion_attempts {
            let case = format!("{operation} with the criterion key {name:?} ({why})");
            let (status, response) = call(&router, method, &path, Some(body)).await;
            assert_refusal(
                &case,
                status,
                &response,
                400,
                &invalid_parameter_value("findingCriteria"),
            );
            assert_eq!(
                error_code(&response),
                INVALID_INPUT,
                "case {case}: expected the {INVALID_INPUT} class, got {response:?}"
            );
            assert!(
                !response.to_string().contains(name),
                "case {case}: the refusal must echo nothing of the caller's key, got {response:?}"
            );
        }

        // The same rule under the OTHER member: a sort attribute is refused as an
        // invalid `sortCriteria`, never as an invalid `findingCriteria`.
        let sort_attempts = [
            (
                "ListFindings",
                format!("/detector/{detector_id}/findings"),
                json!({"sortCriteria": {"attributeName": name}}),
            ),
            (
                "GetFindings",
                format!("/detector/{detector_id}/findings/get"),
                json!({"findingIds": [], "sortCriteria": {"attributeName": name}}),
            ),
        ];
        for (operation, path, body) in sort_attempts {
            // An absent-or-empty sort attribute asks for no ordering at all, so the
            // empty name is not a refusal and is covered by the catalogue suite.
            if name.is_empty() {
                continue;
            }
            let case = format!("{operation} with the sort attribute {name:?} ({why})");
            let (status, response) = call(&router, "POST", &path, Some(body)).await;
            assert_refusal(
                &case,
                status,
                &response,
                400,
                &invalid_parameter_value("sortCriteria"),
            );
        }
    }

    // The complement: a criterion on a real field is accepted, and the filter reads
    // back carrying exactly the criteria the create stored.
    let accepted = criteria("service.archived");
    let (status, response) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({"name": "criteria.accepted", "findingCriteria": accepted.clone()})),
    )
    .await;
    assert_eq!(
        status, 200,
        "a criterion on a real finding field must be accepted, got {status} {response:?}"
    );
    let (status, read) = call(
        &router,
        "GET",
        &format!("/detector/{detector_id}/filter/criteria.accepted"),
        None,
    )
    .await;
    assert_eq!(status, 200, "get-filter: got {status} {read:?}");
    assert_eq!(
        read["findingCriteria"],
        crate::filter_condition_spellings::reported_criteria(&accepted),
        "the saved filter must report the criteria the create stored, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the account-id format refusal names the parameter that carried it.
// ---------------------------------------------------------------------------

/// A malformed account id is refused in terms of the parameter it arrived in: the
/// `accountIds` list for the batch operations, each organization scope list under its
/// own name, and the `accountId` member itself for the create that carries account
/// DETAILS.
///
/// The rows span the malformed value's shape (too short, too long, not digits, mixed,
/// empty-ish) and the operation, so the rule is shown to hold for the class and not
/// for one recorded value.
#[tokio::test]
async fn a_malformed_account_id_is_refused_in_terms_of_its_parameter() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let rule_id = published_rule_ids(&router).await[0].clone();
    // The organization-configuration row below is available only to the
    // organization's delegated administrator, so the standing is established first:
    // what is under test here is the account-id refusal, not the authorization gate.
    as_delegated_admin(&router, "us-east-1").await;

    // `AccountId` is `len 12..12` and constrains no characters
    // (`smithy_query(shape="AccountId")`), so a 12-character non-numeric value
    // reaches the handler; the shorter and longer rows are refused by the boundary
    // first and are covered by rule 1.
    let malformed: [(&str, &str); 4] = [
        ("12345abcdefg", "12 characters, not all digits"),
        ("balloon-face", "12 characters of prose"),
        ("            ", "12 spaces"),
        ("1111-2222-33", "12 characters with separators"),
    ];

    for (account_id, why) in malformed {
        let batch_rows = [
            (
                "DisassociateMembers",
                format!("/detector/{detector_id}/member/disassociate"),
                json!({"accountIds": [account_id]}),
                account_ids_format("accountIds"),
            ),
            (
                "GetMembers",
                format!("/detector/{detector_id}/member/get"),
                json!({"accountIds": [account_id]}),
                account_ids_format("accountIds"),
            ),
            (
                "InviteMembers",
                format!("/detector/{detector_id}/member/invite"),
                json!({"accountIds": [account_id]}),
                account_ids_format("accountIds"),
            ),
            (
                "DeleteMembers",
                format!("/detector/{detector_id}/member/delete"),
                json!({"accountIds": [account_id]}),
                account_ids_format("accountIds"),
            ),
            (
                "GetUsageStatistics",
                format!("/detector/{detector_id}/usage/statistics"),
                json!({
                    "usageStatisticsType": "SUM_BY_ACCOUNT",
                    "usageCriteria": {"accountIds": [account_id]},
                }),
                account_ids_format("accountIds"),
            ),
            (
                "CreateCustomDetectionRuleOrgConfiguration",
                "/custom-detection-rule/org-configuration".to_string(),
                json!({"ruleId": rule_id, "mode": "LIVE", "includeAccountIds": [account_id]}),
                account_ids_format("includeAccountIds"),
            ),
            (
                "CreateMembers",
                format!("/detector/{detector_id}/member"),
                json!({"accountDetails": [{"accountId": account_id, "email": "a@example.com"}]}),
                ACCOUNT_ID_FORMAT.to_string(),
            ),
        ];

        for (operation, path, body, expected) in batch_rows {
            let case = format!("{operation} with an account id that is {why}");
            let (status, response) = call(&router, "POST", &path, Some(body)).await;
            assert_refusal(&case, status, &response, 400, &expected);
            assert_eq!(
                error_code(&response),
                INVALID_INPUT,
                "case {case}: expected the {INVALID_INPUT} class, got {response:?}"
            );
            assert!(
                !response.to_string().contains(account_id),
                "case {case}: the refusal must echo nothing of the caller's value, got {response:?}"
            );
        }
    }

    // The complement: a well-formed account id the administrator holds no member
    // record for is not a format refusal at all but a per-account result.
    let (status, response) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/member/disassociate"),
        Some(json!({"accountIds": ["210987654321"]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed account id must be answered per account, got {status} {response:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a refusal about ONE rule identifies that rule.
// ---------------------------------------------------------------------------

/// A second association for a rule the account already associated is the modelled
/// conflict, and the text carries the `RuleId` the request named — the only thing that
/// says WHICH rule conflicted. A rule the catalogue does not publish is the modelled
/// 404, carrying the same request value.
///
/// The rows span every published rule and every `AssociationMode` member for the
/// conflict, and the forms `RuleId`'s pattern admits for the absence.
#[tokio::test]
async fn an_association_refusal_identifies_the_rule_it_is_about() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    assert!(
        !published.is_empty(),
        "GuardDuty publishes custom detection rules, so the catalogue must not be empty"
    );

    for rule_id in &published {
        for mode in MODES {
            let router = new_router();
            let case = format!("rule {rule_id:?} in mode {mode}");

            let (status, created) = call(
                &router,
                "POST",
                ASSOCIATIONS_PATH,
                Some(json!({"ruleId": rule_id, "mode": mode})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the first association must be made, got {status} {created:?}"
            );
            let association_id = created["ruleAssociation"]["associationId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected an associationId, got {created:?}")
                })
                .to_string();

            let (status, refused) = call(
                &router,
                "POST",
                ASSOCIATIONS_PATH,
                Some(json!({"ruleId": rule_id, "mode": mode})),
            )
            .await;
            assert_refusal(
                &case,
                status,
                &refused,
                409,
                &format!("Association already exists for rule {rule_id}"),
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {case}: expected the modelled conflict class, got {refused:?}"
            );

            // The conflict refused the second create rather than replacing the
            // first: the association the first call made still reads back.
            let (status, read) = call(
                &router,
                "GET",
                &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the first association must survive the conflict, got {status} {read:?}"
            );
            assert_eq!(
                read["ruleAssociation"]["mode"].as_str(),
                Some(mode),
                "case {case}: expected the mode the first call stored, got {read:?}"
            );
        }
    }

    // A `RuleId` the catalogue does not publish is the modelled 404, naming the rule
    // the request asked for. Every row satisfies `RuleId`'s own pattern
    // (`^[a-z0-9]+(-[a-z0-9]+)*$` — `smithy_query(operation="CreateCustomDetectionRuleAssociation")`),
    // so each reaches the handler rather than the boundary.
    let absent = [
        ("1d6468139ca547399612b508fff653cd", "32 hexadecimal characters"),
        ("no-such-rule", "a plausible but unpublished slug"),
        ("q", "a single character: the low end of RuleId"),
        ("0-1-2-3", "digits in several segments"),
        ("r", "another single character"),
    ];
    for (rule_id, why) in absent {
        for mode in MODES {
            let case = format!("the unpublished rule {rule_id:?} ({why}) in mode {mode}");
            let (status, refused) = call(
                &router,
                "POST",
                ASSOCIATIONS_PATH,
                Some(json!({"ruleId": rule_id, "mode": mode})),
            )
            .await;
            assert_refusal(
                &case,
                status,
                &refused,
                404,
                &format!("Rule not found: {rule_id}"),
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case}: expected the modelled not-found class, got {refused:?}"
            );
        }

        // The same absence through the READ of the rule itself, which answers about
        // the same catalogue.
        let case = format!("get the unpublished rule {rule_id:?} ({why})");
        let (status, refused) = call(&router, "GET", &format!("{RULES_PATH}/{rule_id}"), None).await;
        assert_refusal(
            &case,
            status,
            &refused,
            404,
            &format!("Rule not found: {rule_id}"),
        );
    }

    // The ASSOCIATION of a published rule is a resource of the account's own, and an
    // id nothing minted is the same shape of refusal under its own prefix: an
    // `AssociationId` the request supplied is what says which association is missing.
    // The rows span the forms `AssociationId`'s pattern admits (`^[a-zA-Z0-9_-]{1,64}$`
    // — `smithy_query(shape="AssociationDetail")`), and each is read under a rule the
    // catalogue DOES publish, so the rule is not what is missing.
    let published_rule = &published[0];
    let association_ids: Vec<(String, &str)> = vec![
        (
            "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            "32 hexadecimal characters",
        ),
        ("A".to_string(), "a single character: the low end of AssociationId"),
        (
            "an_id-with-separators".to_string(),
            "underscores and dashes",
        ),
        ("9".repeat(64), "64 characters: the high end"),
    ];
    for (association_id, why) in association_ids {
        let case = format!("GET the unminted association {association_id:?} ({why})");
        let (status, refused) = call(
            &router,
            "GET",
            &format!("{RULES_PATH}/{published_rule}/association/{association_id}"),
            None,
        )
        .await;
        assert_refusal(
            &case,
            status,
            &refused,
            404,
            &format!("Association not found: {association_id}"),
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case}: expected the modelled not-found class, got {refused:?}"
        );

        // The UPDATE of the same absent association is the same refusal, worded the
        // same way: both name the association the request asked about.
        let case = format!("PUT the unminted association {association_id:?} ({why})");
        let (status, refused) = call(
            &router,
            "PUT",
            &format!("{RULES_PATH}/{published_rule}/association/{association_id}"),
            Some(json!({"mode": MODES[0]})),
        )
        .await;
        assert_refusal(
            &case,
            status,
            &refused,
            404,
            &format!("Association not found: {association_id}"),
        );

        // The DELETE is not a refusal at all: it is `@idempotent`
        // (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`), so an
        // association that is already absent is the state it asks for and it answers
        // the empty success with no message to word.
        let case = format!("DELETE the unminted association {association_id:?} ({why})");
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("{RULES_PATH}/{published_rule}/association/{association_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: an idempotent delete of an absent association expected 200, got {status} {deleted:?}"
        );
        assert_eq!(
            deleted["message"],
            Value::Null,
            "case {case}: a success words no refusal, got {deleted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the already-protected conflict is worded in terms of the resource.
// ---------------------------------------------------------------------------

/// GuardDuty protects one S3 bucket with one Malware Protection plan, so a second
/// create for a bucket a plan already protects is the modelled conflict — in one fixed
/// sentence about the bucket, with no name quoted back.
///
/// The rows span the bucket name, the Region the plan is created through and what else
/// the second create carries (another role, prefixes, tags, a tagging action), because
/// the conflict is about the PROTECTED RESOURCE and about nothing else in the request.
#[tokio::test]
async fn a_second_plan_for_a_protected_bucket_is_refused_in_terms_of_the_bucket() {
    const ALREADY_PROTECTED: &str =
        "The request was rejected because the provided S3 bucket is already protected.";

    for (bucket, region) in [
        ("findings-archive", "us-east-1"),
        ("a.bucket.with.dots", "eu-west-1"),
        ("bucket-1234567890", "ap-southeast-2"),
    ] {
        let router = new_router();
        let (status, created) = call_in(
            &router,
            region,
            "POST",
            PLANS_PATH,
            Some(json!({
                "role": PLAN_ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": bucket}},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "the first plan for {bucket} in {region}: expected 200, got {status} {created:?}"
        );
        let plan_id = created["malwareProtectionPlanId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a malwareProtectionPlanId, got {created:?}"))
            .to_string();

        // Whatever else the second create asks for, the bucket is the conflict.
        let seconds = [
            (
                "the same request again",
                json!({
                    "role": PLAN_ROLE,
                    "protectedResource": {"s3Bucket": {"bucketName": bucket}},
                }),
            ),
            (
                "another role",
                json!({
                    "role": "arn:aws:iam::000000000000:role/AnotherRole",
                    "protectedResource": {"s3Bucket": {"bucketName": bucket}},
                }),
            ),
            (
                "object prefixes and tags",
                json!({
                    "role": PLAN_ROLE,
                    "protectedResource": {
                        "s3Bucket": {"bucketName": bucket, "objectPrefixes": ["incoming/"]},
                    },
                    "tags": {"team": "security"},
                }),
            ),
            (
                "a tagging action",
                json!({
                    "role": PLAN_ROLE,
                    "protectedResource": {"s3Bucket": {"bucketName": bucket}},
                    "actions": {"tagging": {"status": "ENABLED"}},
                }),
            ),
        ];

        for (why, body) in seconds {
            let case = format!("a second plan for {bucket} in {region} with {why}");
            let (status, refused) = call_in(&router, region, "POST", PLANS_PATH, Some(body)).await;
            assert_refusal(&case, status, &refused, 409, ALREADY_PROTECTED);
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {case}: expected the modelled conflict class, got {refused:?}"
            );
            assert!(
                !refused.to_string().contains(bucket),
                "case {case}: the refusal must not quote the bucket back, got {refused:?}"
            );
        }

        // The refused creates changed nothing: the plan the first call minted still
        // reads back with the bucket it protects.
        let (status, read) = call_in(
            &router,
            region,
            "GET",
            &format!("{PLANS_PATH}/{plan_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "the first plan for {bucket} must survive the conflicts, got {status} {read:?}"
        );
        assert_eq!(
            read["protectedResource"]["s3Bucket"]["bucketName"].as_str(),
            Some(bucket),
            "expected the bucket the first create protected, got {read:?}"
        );

        // Another bucket in the same Region is not protected yet, so it is accepted:
        // the conflict is per protected resource.
        let (status, other) = call_in(
            &router,
            region,
            "POST",
            PLANS_PATH,
            Some(json!({
                "role": PLAN_ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": format!("{bucket}-two")}},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "another bucket in {region} must be accepted, got {status} {other:?}"
        );
    }
}
