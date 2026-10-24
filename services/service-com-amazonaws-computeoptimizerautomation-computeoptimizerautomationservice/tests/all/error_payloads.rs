// SPDX-License-Identifier: CC0-1.0

//! The BODY of every client error this service raises.
//!
//! The sibling suite [`crate::automation_rule_lifecycle`] asserts the status and
//! the error code of a refusal; this one asserts what the body says, over a
//! TABLE of inputs per rule rather than one recorded request. Its client helpers
//! are that suite's, so both suites drive the service through the same
//! awsJson1_0 router a real caller reaches.
//!
//! The rules under test:
//!
//! * a rejected `ruleRevision` reports the rule's CURRENT revision and no
//!   resource identifier;
//! * `ResourceNotFoundException` carries one fixed, resource-independent
//!   message;
//! * a refusal on a member's constraint is AWS's validation report — how many
//!   members failed, then the member and the requirement it failed, with no
//!   rejected value echoed and a `@pattern` reported unanchored;
//! * `IdempotentParameterMismatchException` carries one fixed message;
//! * an enrollment status a caller may not set is refused with a message that
//!   names the rejected value and enumerates the settable set.

use serde_json::{json, Value};

use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, member, minimal_rule, string_member,
};

/// The fixed message every `ResourceNotFoundException` of this service carries.
const NOT_FOUND_MESSAGE: &str = "The specified resource could not be found";

/// The fixed message every `IdempotentParameterMismatchException` carries.
const IDEMPOTENT_MISMATCH_MESSAGE: &str = "The request tied to this token is different!";

/// The `message` member of an error body, under either casing the awsJson family
/// renders it with.
///
/// `pub(crate)` so the sibling suites in this binary read a refusal's body the
/// same way rather than each growing a copy of this.
pub(crate) fn error_message(body: &Value) -> String {
    ["message", "Message"]
        .iter()
        .find_map(|key| body.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// Assert a refusal's HTTP status, error code and exact message.
fn assert_error(
    response: &(u16, Value),
    expected_status: u16,
    expected_code: &str,
    expected_message: &str,
    case: &str,
) {
    let (status, body) = response;
    assert_eq!(
        *status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
    assert_eq!(
        error_message(body),
        expected_message,
        "case {case:?}: expected message {expected_message:?}, got {:?}",
        error_message(body)
    );
}

/// The id segment of a rule ARN, i.e. everything after `automation-rule/`.
fn rule_id_of(rule_arn: &str) -> String {
    rule_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Optimistic-concurrency rejection
// ---------------------------------------------------------------------------

/// A `ruleRevision` that is not the rule's current one is refused with a message
/// stating the CURRENT revision — on both operations that take the revision as
/// an optimistic-concurrency token, whatever the rejected value is and however
/// far the rule has moved on. The rejection discloses neither the rule's ARN nor
/// its id, and leaves the rule as it stood.
#[tokio::test]
async fn a_stale_revision_rejection_reports_the_current_revision() {
    // (case, operation, successful updates before the stale call, revision sent)
    let cases: [(&str, &str, i64, i64); 8] = [
        ("update-one-behind", "UpdateAutomationRule", 1, 1),
        ("update-three-behind", "UpdateAutomationRule", 3, 1),
        ("update-revision-zero", "UpdateAutomationRule", 0, 0),
        ("update-revision-past-the-current", "UpdateAutomationRule", 0, 7),
        ("delete-one-behind", "DeleteAutomationRule", 1, 1),
        ("delete-two-behind", "DeleteAutomationRule", 2, 2),
        ("delete-revision-zero", "DeleteAutomationRule", 0, 0),
        ("delete-revision-past-the-current", "DeleteAutomationRule", 2, 99),
    ];

    for (case, operation, bumps, sent) in cases {
        let router = emulator();
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule("revision-case"),
            case,
        )
        .await;
        let rule_arn = string_member(&created, "ruleArn");
        let mut current = member(&created, "ruleRevision").as_i64().unwrap_or_default();
        for bump in 0..bumps {
            let bumped = call_ok(
                &router,
                "UpdateAutomationRule",
                &json!({
                    "ruleArn": rule_arn,
                    "ruleRevision": current,
                    "description": format!("bump {bump}"),
                }),
                case,
            )
            .await;
            current = member(&bumped, "ruleRevision").as_i64().unwrap_or_default();
        }

        // `DeleteAutomationRuleRequest` carries no `description`, so each
        // operation is sent exactly the members it models.
        let request = if operation == "DeleteAutomationRule" {
            json!({"ruleArn": rule_arn, "ruleRevision": sent})
        } else {
            json!({
                "ruleArn": rule_arn,
                "ruleRevision": sent,
                "description": "must not be applied",
            })
        };
        let response = call(&router, operation, &request).await;
        assert_error(
            &response,
            400,
            "InvalidParameterValueException",
            &format!("Rule revision does not match the current revision number {current}"),
            case,
        );

        let message = error_message(&response.1);
        assert!(
            !message.contains(&rule_arn) && !message.contains(&rule_id_of(&rule_arn)),
            "case {case:?}: the rejection must disclose no resource identifier, got {message:?}"
        );

        // The refused call changed nothing: the rule is still there at the
        // revision the message named.
        let read_back = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&read_back, "ruleRevision").as_i64(),
            Some(current),
            "case {case:?}: a refused call leaves the revision alone, got {read_back}"
        );
    }
}

/// Two updates sharing one revision: exactly one wins, and the loser is told the
/// revision the winner left behind.
#[tokio::test]
async fn the_loser_of_a_revision_race_is_told_the_winning_revision() {
    let router = emulator();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("race-case"),
        "race",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();

    let mut winners = 0;
    let mut losers = Vec::new();
    for contender in ["first", "second", "third"] {
        let response = call(
            &router,
            "UpdateAutomationRule",
            &json!({
                "ruleArn": rule_arn,
                "ruleRevision": revision,
                "description": format!("written by {contender}"),
            }),
        )
        .await;
        if response.0 == 200 {
            winners += 1;
        } else {
            losers.push((contender, response));
        }
    }

    assert_eq!(
        winners, 1,
        "case \"race\": exactly one call may take the revision, {winners} did"
    );
    for (contender, response) in &losers {
        assert_error(
            response,
            400,
            "InvalidParameterValueException",
            &format!(
                "Rule revision does not match the current revision number {}",
                revision + 1
            ),
            contender,
        );
    }
}

// ---------------------------------------------------------------------------
// Not found
// ---------------------------------------------------------------------------

/// Every `ResourceNotFoundException` carries the same fixed message, whichever
/// operation raised it and whichever ARN the request named — and names neither
/// the ARN nor the rule id.
#[tokio::test]
async fn not_found_is_one_fixed_message_naming_no_resource() {
    let router = emulator();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("rule-to-delete"),
        "setup",
    )
    .await;
    let deleted_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();
    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({"ruleArn": deleted_arn, "ruleRevision": revision}),
        "setup-delete",
    )
    .await;

    // Well-formed ARNs in the caller's account that name no rule. The form is
    // the model's `RuleArn` pattern
    // (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`).
    let account = engine::emulator_account_id();
    let absent_hex = format!("arn:aws:compute-optimizer::{account}:automation-rule/0123456789abcdef");
    let absent_named = format!("arn:aws:compute-optimizer::{account}:automation-rule/never-created");

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "get-a-deleted-rule",
            "GetAutomationRule",
            json!({"ruleArn": deleted_arn}),
        ),
        (
            "get-an-id-that-never-existed",
            "GetAutomationRule",
            json!({"ruleArn": absent_hex}),
        ),
        (
            "get-a-name-that-never-existed",
            "GetAutomationRule",
            json!({"ruleArn": absent_named}),
        ),
        (
            "update-a-deleted-rule",
            "UpdateAutomationRule",
            json!({"ruleArn": deleted_arn, "ruleRevision": revision, "description": "gone"}),
        ),
        (
            "update-a-rule-that-never-existed",
            "UpdateAutomationRule",
            json!({"ruleArn": absent_hex, "ruleRevision": 1}),
        ),
        (
            "delete-a-deleted-rule",
            "DeleteAutomationRule",
            json!({"ruleArn": deleted_arn, "ruleRevision": revision}),
        ),
        (
            "delete-a-rule-that-never-existed",
            "DeleteAutomationRule",
            json!({"ruleArn": absent_named, "ruleRevision": 1}),
        ),
        (
            "list-the-tags-of-a-deleted-rule",
            "ListTagsForResource",
            json!({"resourceArn": deleted_arn}),
        ),
        (
            "list-the-tags-of-a-rule-that-never-existed",
            "ListTagsForResource",
            json!({"resourceArn": absent_hex}),
        ),
    ];

    for (case, operation, request) in &cases {
        let response = call(&router, operation, request).await;
        assert_error(&response, 404, "ResourceNotFoundException", NOT_FOUND_MESSAGE, case);
        let message = error_message(&response.1);
        // The ARN member is `ruleArn` on the rule operations and `resourceArn`
        // on the tagging ones.
        let arn = ["ruleArn", "resourceArn"]
            .iter()
            .map(|name| string_member(request, name))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        assert!(
            !message.contains(&arn) && !message.contains(&rule_id_of(&arn)),
            "case {case:?}: a not-found body must echo no resource identifier, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Validation reports
// ---------------------------------------------------------------------------

/// A refusal on a member's constraint is AWS's validation report: the number of
/// failing members, then one clause per member naming the member's path and the
/// requirement it failed. No rejected value is echoed, and a `@pattern` is
/// reported without the `^`/`$` the model spells it with.
///
/// The requirements below are the model's own constraints:
/// `RuleName` @length 0..128 @pattern `^[a-zA-Z0-9_-]*$`, `RuleDescription`
/// @length 0..1024 @pattern `^[a-zA-Z0-9_\-\s@\.]*$`, `NextToken` @pattern
/// `^[A-Za-z0-9+/=]+$`, and `ListAutomationRulesRequest$maxResults` @range
/// 1..100
/// (`smithy_query(query="shape",shape="RuleName,RuleDescription,NextToken,ListAutomationRulesRequest")`).
///
/// A `RuleArn` outside its pattern is NOT a row here: a resource string that
/// names no automation rule of this service is refused by authorization
/// (`AccessDeniedException`, 403) before the request is parsed, so it never
/// reaches this report — see `automation_event_and_tagging`'s
/// `a_string_naming_no_rule_of_this_service_is_unauthorized`.
#[tokio::test]
async fn a_constraint_refusal_is_a_validation_report() {
    let router = emulator();

    let long_name = "a".repeat(129);
    let long_description = "d".repeat(1025);
    let spaced_name = "not a legal name";
    let marked_description = "description with a % in it";

    let name_outside_its_pattern = minimal_rule(spaced_name);
    let name_past_its_length = minimal_rule(&long_name);
    let description_outside_its_pattern = {
        let mut request = minimal_rule("described-rule");
        request["description"] = json!(marked_description);
        request
    };
    let description_past_its_length = {
        let mut request = minimal_rule("long-described-rule");
        request["description"] = json!(long_description);
        request
    };
    let two_members_at_once = {
        let mut request = minimal_rule(spaced_name);
        request["description"] = json!(marked_description);
        request
    };
    let empty_name = minimal_rule("");
    let bad_priority = {
        let mut request = minimal_rule("priority-rule");
        request["priority"] = json!("high");
        request
    };
    let organization_rule_without_configuration = {
        let mut request = minimal_rule("organization-rule");
        request["ruleType"] = json!("OrganizationRule");
        request
    };

    // (case, operation, request, expected message, strings the message must not contain)
    let cases: Vec<(&str, &str, Value, String, Vec<String>)> = vec![
        (
            "name-outside-its-pattern",
            "CreateAutomationRule",
            name_outside_its_pattern,
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: [a-zA-Z0-9_-]*"
                .to_string(),
            vec![spaced_name.to_string()],
        ),
        (
            "name-past-its-length",
            "CreateAutomationRule",
            name_past_its_length,
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must have length less than or equal to 128"
                .to_string(),
            vec![long_name.clone()],
        ),
        (
            "description-outside-its-pattern",
            "CreateAutomationRule",
            description_outside_its_pattern,
            "1 validation error detected: Value at 'description' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [a-zA-Z0-9_\\-\\s@\\.]*"
                .to_string(),
            vec![marked_description.to_string()],
        ),
        (
            "description-past-its-length",
            "CreateAutomationRule",
            description_past_its_length,
            "1 validation error detected: Value at 'description' failed to satisfy constraint: \
             Member must have length less than or equal to 1024"
                .to_string(),
            vec![long_description.clone()],
        ),
        (
            "next-token-outside-its-pattern",
            "ListAutomationRules",
            json!({"nextToken": "not a token!"}),
            "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [A-Za-z0-9+/=]+"
                .to_string(),
            vec!["not a token!".to_string()],
        ),
        (
            "max-results-below-its-range",
            "ListAutomationRules",
            json!({"maxResults": 0}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
            Vec::new(),
        ),
        (
            "max-results-past-its-range",
            "ListAutomationRules",
            json!({"maxResults": 101}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
            vec!["101".to_string()],
        ),
        (
            "an-empty-name",
            "CreateAutomationRule",
            empty_name,
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must have length greater than or equal to 1"
                .to_string(),
            Vec::new(),
        ),
        (
            "a-priority-that-is-not-a-decimal",
            "CreateAutomationRule",
            bad_priority,
            "1 validation error detected: Value at 'priority' failed to satisfy constraint: \
             Member must be a decimal number between 0 and 1 with up to 30 digits after the \
             decimal point"
                .to_string(),
            vec!["high".to_string()],
        ),
        (
            "an-organization-rule-without-its-configuration",
            "CreateAutomationRule",
            organization_rule_without_configuration,
            "1 validation error detected: Value at 'organizationConfiguration' failed to satisfy \
             constraint: Member is required for the OrganizationRule rule type"
                .to_string(),
            Vec::new(),
        ),
        (
            "a-filter-name-outside-the-value-set",
            "ListAutomationRules",
            json!({"filters": [{"name": "Nickname", "values": ["x"]}]}),
            // A list element names its 1-based position, then the collection
            // step, so the first filter's name is `filters.1.member.name`.
            "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy \
             constraint: Member must satisfy enum value set: [Name, RecommendedActionType, \
             Status, RuleType, OrganizationConfigurationRuleApplyOrder, AccountId]"
                .to_string(),
            vec!["Nickname".to_string()],
        ),
    ];

    for (case, operation, request, expected, forbidden) in &cases {
        let response = call(&router, operation, request).await;
        assert_error(
            &response,
            400,
            "InvalidParameterValueException",
            expected,
            case,
        );
        let message = error_message(&response.1);
        for value in forbidden {
            assert!(
                !message.contains(value),
                "case {case:?}: the report must not echo the rejected value, got {message:?}"
            );
        }
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {case:?}: a pattern is reported unanchored, got {message:?}"
        );
    }

    // Two members failing one request are counted together, and each is
    // attributed to its own member.
    let response = call(&router, "CreateAutomationRule", &two_members_at_once).await;
    let message = error_message(&response.1);
    assert_eq!(
        response.0, 400,
        "case \"two-members-at-once\": expected HTTP 400, got {} and {}",
        response.0, response.1
    );
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case \"two-members-at-once\": the report counts both members, got {message:?}"
    );
    for member_path in ["name", "description"] {
        assert!(
            message.contains(&format!(
                "Value at '{member_path}' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: "
            )),
            "case \"two-members-at-once\": {member_path} must have its own clause, got {message:?}"
        );
    }

    // A rejected value that itself contains the clause's own marker text is
    // still not echoed: the report is built from the violation, never by
    // editing a sentence the value was already interpolated into.
    let smuggled = format!("{} failed to satisfy constraint: smuggled", "a".repeat(120));
    let response = call(&router, "CreateAutomationRule", &minimal_rule(&smuggled)).await;
    let message = error_message(&response.1);
    assert_eq!(
        response.0, 400,
        "case \"a-value-quoting-the-marker\": expected HTTP 400, got {} and {}",
        response.0, response.1
    );
    assert!(
        !message.contains("smuggled") && !message.contains(&smuggled),
        "case \"a-value-quoting-the-marker\": no part of the rejected value may appear, got \
         {message:?}"
    );

    // The values at the published bounds are accepted, so the refusals above are
    // about the constraint and not about the request.
    let at_the_length_bound = {
        let mut request = minimal_rule(&"a".repeat(128));
        request["description"] = json!("d".repeat(1024));
        request
    };
    call_ok(
        &router,
        "CreateAutomationRule",
        &at_the_length_bound,
        "at-the-length-bound",
    )
    .await;
    for bound in [1, 100] {
        call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": bound}),
            "max-results-at-its-bound",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Idempotency-token mismatch
// ---------------------------------------------------------------------------

/// A client token replayed with a different request is refused with one fixed
/// message, on every operation that takes a token.
#[tokio::test]
async fn an_idempotency_mismatch_is_one_fixed_message() {
    let router = emulator();

    // CreateAutomationRule: the same token with a different name.
    let mut first_create = minimal_rule("token-rule-one");
    first_create["clientToken"] = json!("create-token");
    call_ok(
        &router,
        "CreateAutomationRule",
        &first_create,
        "create-first",
    )
    .await;
    let mut second_create = minimal_rule("token-rule-two");
    second_create["clientToken"] = json!("create-token");
    assert_error(
        &call(&router, "CreateAutomationRule", &second_create).await,
        409,
        "IdempotentParameterMismatchException",
        IDEMPOTENT_MISMATCH_MESSAGE,
        "create-with-a-different-name",
    );

    // UpdateAutomationRule: the same token with a different description.
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("token-update-rule"),
        "update-setup",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();
    call_ok(
        &router,
        "UpdateAutomationRule",
        &json!({
            "ruleArn": rule_arn,
            "ruleRevision": revision,
            "description": "first description",
            "clientToken": "update-token",
        }),
        "update-first",
    )
    .await;
    assert_error(
        &call(
            &router,
            "UpdateAutomationRule",
            &json!({
                "ruleArn": rule_arn,
                "ruleRevision": revision + 1,
                "description": "another description",
                "clientToken": "update-token",
            }),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        IDEMPOTENT_MISMATCH_MESSAGE,
        "update-with-a-different-description",
    );

    // DeleteAutomationRule: the same token against a different rule.
    let doomed = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("token-delete-rule"),
        "delete-setup",
    )
    .await;
    let doomed_arn = string_member(&doomed, "ruleArn");
    let doomed_revision = member(&doomed, "ruleRevision").as_i64().unwrap_or_default();
    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({
            "ruleArn": doomed_arn,
            "ruleRevision": doomed_revision,
            "clientToken": "delete-token",
        }),
        "delete-first",
    )
    .await;
    assert_error(
        &call(
            &router,
            "DeleteAutomationRule",
            &json!({
                "ruleArn": rule_arn,
                "ruleRevision": revision + 1,
                "clientToken": "delete-token",
            }),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        IDEMPOTENT_MISMATCH_MESSAGE,
        "delete-with-a-different-rule",
    );

    // UpdateEnrollmentConfiguration: the same token with a different status.
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active", "clientToken": "enrollment-token"}),
        "enrollment-first",
    )
    .await;
    assert_error(
        &call(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": "Inactive", "clientToken": "enrollment-token"}),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        IDEMPOTENT_MISMATCH_MESSAGE,
        "enrollment-with-a-different-status",
    );
}

// ---------------------------------------------------------------------------
// Enrollment status
// ---------------------------------------------------------------------------

/// A status only the service reports is refused with a message that names the
/// rejected value AND the statuses a caller may send, and changes nothing.
///
/// `EnrollmentStatus` is `Active | Inactive | Pending | Failed`
/// (`smithy_query(query="shape",shape="EnrollmentStatus")`); the request member
/// documents that "The Pending and Failed options cannot be used to update the
/// enrollment status of an account. They are returned in the response".
#[tokio::test]
async fn an_unsettable_enrollment_status_names_the_settable_set() {
    for server_only in ["Pending", "Failed"] {
        let router = emulator();
        call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": "Active"}),
            server_only,
        )
        .await;

        assert_error(
            &call(
                &router,
                "UpdateEnrollmentConfiguration",
                &json!({"status": server_only}),
            )
            .await,
            400,
            "InvalidParameterValueException",
            &format!("Invalid status: {server_only}. Only ACTIVE and INACTIVE are allowed."),
            server_only,
        );

        let read_back = call_ok(
            &router,
            "GetEnrollmentConfiguration",
            &json!({}),
            server_only,
        )
        .await;
        assert_eq!(
            string_member(&read_back, "status"),
            "Active",
            "case {server_only:?}: a refused update leaves the enrollment alone, got {read_back}"
        );
    }

    // Positive control: each settable status the message names is accepted.
    for (named, status) in [("ACTIVE", "Active"), ("INACTIVE", "Inactive")] {
        let router = emulator();
        let updated = call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": status}),
            named,
        )
        .await;
        assert_eq!(
            string_member(&updated, "status"),
            status,
            "case {named:?}: a settable status is stored as sent, got {updated}"
        );
    }
}
