// SPDX-License-Identifier: CC0-1.0

//! How GuardDuty answers a value that breaks a constraint its own model declares.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//!
//! > A value outside a constraint this service's model declares is refused with HTTP
//! > 400 under `BadRequestException` — the only client error the model binds to any
//! > of its operations — never under a code the model declares for nothing, and with
//! > ONE FIXED SENTENCE for the class: a value the service could not read as the
//! > member's modelled type (anything outside an enumerated value set) is refused as a
//! > payload it could not process, and every other unacceptable value with the generic
//! > invalid-input sentence. Neither names the member at fault, interpolates the bound
//! > that was broken, nor echoes the caller's value. The refused request changes no
//! > stored state, and the same request with a value inside the constraint succeeds.
//!
//! The bound is already published in the model the caller built its request from, so
//! the refusal restates none of it — which is also why a client may branch on, or
//! snapshot, the one sentence.
//!
//! `smithy_query(query="service_info")` reports exactly two common errors for this
//! service — `BadRequestException` (`httpError` 400) and `InternalServerErrorException`
//! (500) — and every operation declares those two and nothing else
//! (`smithy_query(operation="CreateFilter,UpdateFindingsFeedback")`). An SDK builds its
//! exception classes from the error code, so a refusal named by a shape the model does
//! not declare cannot be caught by a GuardDuty-specific `except` block at all: a plainly
//! fixable input arrives as an unknown failure.
//!
//! The rows below span the constraint KINDS the model uses (`@range` on an integer,
//! `@length` on a string and on a map, an enum value set, `@required`) and span
//! operations and members beyond the two the recorded calls used, because the rule is
//! about the refusal CHANNEL and not about one member.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, the modelled-constraint walk
//! and the handlers together.

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state;

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router,
};

/// The only client error GuardDuty's model binds to an operation, and the error shape
/// restJson1 resolves a response to (the `x-amzn-errortype` header).
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// The code the framework names a boundary refusal by when a service does not state
/// its own. No GuardDuty operation declares a shape by this name, so it must never
/// reach a client of this service.
const UNMODELED_CODE: &str = "ValidationException";

/// A string of `len` characters drawn from the filter-name alphabet, so a length row
/// fails ONLY on length.
fn name_of(len: usize) -> String {
    "a".repeat(len)
}

/// A `FindingCriteria` every row can send: `CreateFilter.FindingCriteria` is required.
fn criteria() -> Value {
    json!({"criterion": {"severity": {"greaterThanOrEqual": 1}}})
}

/// A `CreateFilter` body with `name` and the required criteria, plus whatever `extra`
/// members a row overrides.
fn create_filter_body(name: &str, extra: &[(&str, Value)]) -> Value {
    let mut body = json!({"name": name, "findingCriteria": criteria()});
    for (key, value) in extra {
        body[*key] = value.clone();
    }
    body
}

/// Assert one refusal: the status, the error shape on the wire, that the code is one
/// the model declares rather than the framework's default, and the FIXED sentence the
/// class carries — which names no member and interpolates no bound.
fn assert_modeled_refusal(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        MODELED_CLIENT_ERROR,
        "case {case}: expected the modelled error shape in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        MODELED_CLIENT_ERROR,
        "case {case}: expected the modelled error code, got {body:?}"
    );
    assert_ne!(
        error_code(body),
        UNMODELED_CODE,
        "case {case}: refused under a code this service's model declares for nothing, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case}: expected GuardDuty's fixed text for this refusal class, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// A value outside a declared constraint, on an operation under a detector.
// ---------------------------------------------------------------------------

/// Rows refused by a constraint on a member of an operation addressed under a
/// detector. Each is `(why, method, path suffix after the detector id, body, the
/// fixed message the refusal carries)`.
fn detector_scoped_refusals() -> Vec<(String, &'static str, String, Option<Value>, String)> {
    // The published bounds come from the model, not from the recorded calls:
    // `CreateFilter.Rank -> FilterRank` is `@range` 1..100, `Name -> FilterName` is
    // `@length` 3..64, `Description -> FilterDescription` is 0..512, `Action ->
    // FilterAction` is the enum (ARCHIVE | NOOP);
    // `UpdateFindingsFeedback.Feedback -> Feedback` is the enum (USEFUL | NOT_USEFUL);
    // `ListFindings.MaxResults` is `@range` 1..50
    // (`smithy_query(operation="CreateFilter,UpdateFindingsFeedback,ListFindings")`).
    let mut rows: Vec<(String, &'static str, String, Option<Value>, String)> = Vec::new();

    for (rank, why) in [
        (0, "rank 0: one below the published minimum of 1"),
        (-1, "rank -1: below the published minimum and negative"),
        (101, "rank 101: one above the published maximum of 100"),
        (150, "rank 150: well above the published maximum"),
        (i32::MAX, "rank i32::MAX: the top of the member's own type"),
    ] {
        rows.push((
            format!("CreateFilter with {why}"),
            "POST",
            "/filter".to_string(),
            Some(create_filter_body(
                &format!("rank-row-{rank}"),
                &[("rank", json!(rank))],
            )),
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ));
    }

    for (len, why) in [
        (2, "a name of 2 characters: one below the published minimum of 3"),
        (1, "a name of 1 character"),
        (65, "a name of 65 characters: one above the published maximum of 64"),
        (200, "a name far above the published maximum"),
    ] {
        rows.push((
            format!("CreateFilter with {why}"),
            "POST",
            "/filter".to_string(),
            Some(create_filter_body(&name_of(len), &[])),
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ));
    }

    rows.push((
        "CreateFilter with a description of 513 characters: one above the published maximum of 512"
            .to_string(),
        "POST",
        "/filter".to_string(),
        Some(create_filter_body(
            "description-row",
            &[("description", json!(name_of(513)))],
        )),
        state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
    ));

    for action in ["DELETE", "noop", "", "ARCHIVE_AND_NOTIFY"] {
        rows.push((
            format!("CreateFilter with action {action:?}, outside the FilterAction enum"),
            "POST",
            "/filter".to_string(),
            Some(create_filter_body(
                "action-row",
                &[("action", json!(action))],
            )),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE.to_string(),
        ));
    }

    rows.push((
        "CreateFilter without the required findingCriteria".to_string(),
        "POST",
        "/filter".to_string(),
        Some(json!({"name": "required-row"})),
        // An ABSENT member is not a value, so neither fixed text above states it:
        // the service says which member it cannot act without.
        state::missing_parameter_message("findingCriteria"),
    ));

    for feedback in ["MAYBE_USEFUL", "useful", "", "USEFUL "] {
        rows.push((
            format!("UpdateFindingsFeedback with feedback {feedback:?}, outside the Feedback enum"),
            "POST",
            "/findings/feedback".to_string(),
            Some(json!({"findingIds": [], "feedback": feedback})),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE.to_string(),
        ));
    }

    for (max_results, why) in [
        (0, "maxResults 0: one below the published minimum of 1"),
        (51, "maxResults 51: one above the published maximum of 50"),
        (1000, "maxResults 1000: well above the published maximum"),
    ] {
        rows.push((
            format!("ListFindings with {why}"),
            "POST",
            "/findings".to_string(),
            Some(json!({"maxResults": max_results})),
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ));
    }

    rows
}

#[tokio::test]
async fn a_value_outside_a_modeled_constraint_is_refused_under_the_modeled_error() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (why, method, suffix, body, expected) in detector_scoped_refusals() {
        let (status, response) = call(
            &router,
            method,
            &format!("/detector/{detector_id}{suffix}"),
            body,
        )
        .await;
        assert_modeled_refusal(&why, status, &response, &expected);
    }
}

/// The same rule on operations that take no detector id, so the refusal channel is
/// shown to belong to the SERVICE and not to the filter/feedback handlers. Each row
/// runs on a fresh router with no detector stored, so a row that slipped past the
/// boundary would answer 200 (or a handler rejection) rather than pass for the wrong
/// reason.
#[tokio::test]
async fn the_modeled_refusal_channel_covers_every_operation_of_the_service() {
    // `CreateDetector.Tags -> TagMap` is `@length` 1..200, `ClientToken` is 0..64,
    // `FindingPublishingFrequency` is an enum and `Enable` is `@required`;
    // `ListDetectors.MaxResults` is `@range` 1..50
    // (`smithy_query(operation="CreateDetector,ListDetectors")`).
    let rows: Vec<(String, &str, String, Option<Value>, String)> = vec![
        (
            "CreateDetector with an empty tags map: the published minimum is 1 entry".to_string(),
            "POST",
            "/detector".to_string(),
            Some(json!({"enable": true, "tags": {}})),
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ),
        (
            "CreateDetector with a clientToken of 65 characters: one above the published maximum of 64"
                .to_string(),
            "POST",
            "/detector".to_string(),
            Some(json!({"enable": true, "clientToken": name_of(65)})),
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ),
        (
            "CreateDetector with a findingPublishingFrequency outside the enum".to_string(),
            "POST",
            "/detector".to_string(),
            Some(json!({"enable": true, "findingPublishingFrequency": "EVERY_MINUTE"})),
            state::UNPROCESSABLE_PAYLOAD_MESSAGE.to_string(),
        ),
        (
            "ListDetectors with maxResults 0: one below the published minimum of 1".to_string(),
            "GET",
            "/detector?maxResults=0".to_string(),
            None,
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ),
        (
            "ListDetectors with maxResults 51: one above the published maximum of 50".to_string(),
            "GET",
            "/detector?maxResults=51".to_string(),
            None,
            state::INVALID_INPUT_PARAMETERS_MESSAGE.to_string(),
        ),
    ];

    for (why, method, path, body, expected) in rows {
        let router = new_router();
        let (status, response) = call(&router, method, &path, body).await;
        assert_modeled_refusal(&why, status, &response, &expected);
    }
}

// ---------------------------------------------------------------------------
// The complement: a value INSIDE the constraint is accepted.
// ---------------------------------------------------------------------------

/// Each published bound is exercised ON the bound, not merely well inside it: a
/// refusal channel that refused `rank` 100 would satisfy every assertion above and
/// still be wrong.
#[tokio::test]
async fn a_value_inside_a_modeled_constraint_is_accepted() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // `CreateFilter`: both ends of `Rank`'s range, both ends of `Name`'s length, both
    // `FilterAction` members, and `Description` at its maximum.
    let filter_rows: Vec<(String, Value)> = vec![
        (
            "rank 1: the published minimum".to_string(),
            create_filter_body("rank-min", &[("rank", json!(1))]),
        ),
        (
            "rank 100: the published maximum".to_string(),
            create_filter_body("rank-max", &[("rank", json!(100))]),
        ),
        (
            "rank 50: inside the range".to_string(),
            create_filter_body("rank-mid", &[("rank", json!(50))]),
        ),
        (
            "no rank at all: the member is optional".to_string(),
            create_filter_body("rank-absent", &[]),
        ),
        (
            "a name of 3 characters: the published minimum".to_string(),
            create_filter_body(&name_of(3), &[]),
        ),
        (
            "a name of 64 characters: the published maximum".to_string(),
            create_filter_body(&name_of(64), &[]),
        ),
        (
            "action ARCHIVE".to_string(),
            create_filter_body("action-archive", &[("action", json!("ARCHIVE"))]),
        ),
        (
            "action NOOP".to_string(),
            create_filter_body("action-noop", &[("action", json!("NOOP"))]),
        ),
        (
            "a description of 512 characters: the published maximum".to_string(),
            create_filter_body(
                "description-max",
                &[("description", json!(name_of(512)))],
            ),
        ),
        (
            "an empty description: the published minimum is 0".to_string(),
            create_filter_body("description-empty", &[("description", json!(""))]),
        ),
    ];

    for (why, body) in filter_rows {
        let expected_name = body["name"].as_str().unwrap_or_default().to_string();
        let (status, response) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateFilter with {why}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            response["name"].as_str(),
            Some(expected_name.as_str()),
            "case CreateFilter with {why}: expected the saved name back, got {response:?}"
        );
    }

    // `ListFindings.MaxResults`: both ends of the range.
    for max_results in [1, 50, 7] {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"maxResults": max_results})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ListFindings with maxResults {max_results}: expected 200, got {status} {response:?}"
        );
    }

    // `UpdateFindingsFeedback.Feedback`: every member of the enum.
    for feedback in ["USEFUL", "NOT_USEFUL"] {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/feedback"),
            Some(json!({"findingIds": [], "feedback": feedback})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateFindingsFeedback with feedback {feedback}: expected 200, got {status} {response:?}"
        );
    }

    // `CreateDetector`: a one-entry tag map (the published minimum), a 64-character
    // client token (the published maximum) and every `FindingPublishingFrequency`
    // member. Each runs against its own Region, because a detector is one per account
    // per Region.
    for (region, body) in [
        ("us-west-1", json!({"enable": true, "tags": {"team": "security"}})),
        (
            "us-west-2",
            json!({"enable": true, "clientToken": name_of(64)}),
        ),
        (
            "eu-west-1",
            json!({"enable": true, "findingPublishingFrequency": "FIFTEEN_MINUTES"}),
        ),
        (
            "eu-west-2",
            json!({"enable": true, "findingPublishingFrequency": "ONE_HOUR"}),
        ),
        (
            "eu-west-3",
            json!({"enable": true, "findingPublishingFrequency": "SIX_HOURS"}),
        ),
    ] {
        let router = new_router();
        let (status, response) = call_in(&router, region, "POST", "/detector", Some(body.clone())).await;
        assert_eq!(
            status, 200,
            "case CreateDetector in {region} with {body}: expected 200, got {status} {response:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A refused request changes nothing, and the same request inside the constraint
// then succeeds.
// ---------------------------------------------------------------------------

/// Cause 1's invariant, over every out-of-range rank rather than the recorded one: the
/// refusal leaves no filter under the name it used, and the same name with a rank
/// inside the range is then saved and readable.
#[tokio::test]
async fn a_refused_create_filter_leaves_no_filter_behind() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for (index, rank) in [0, -5, 101, 150, 10_000].into_iter().enumerate() {
        let name = format!("retry-row-{index}");
        let case = format!("CreateFilter {name} with rank {rank}");

        let (status, refused) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(create_filter_body(&name, &[("rank", json!(rank))])),
        )
        .await;
        assert_modeled_refusal(
            &case,
            status,
            &refused,
            state::INVALID_INPUT_PARAMETERS_MESSAGE,
        );

        // Nothing was saved under that name.
        let (status, read) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create must leave no filter to read, got {status} {read:?}"
        );

        // The same name, with a rank inside the range, is accepted.
        let (status, created) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(create_filter_body(&name, &[("rank", json!(5))])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the retry with a valid rank must succeed, got {status} {created:?}"
        );
        assert_eq!(
            created["name"].as_str(),
            Some(name.as_str()),
            "case {case}: the retry must answer with the saved name, got {created:?}"
        );

        let (status, read) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the saved filter must be readable, got {status} {read:?}"
        );
        assert_eq!(
            read["rank"].as_i64(),
            Some(5),
            "case {case}: expected the rank the retry stored, got {read:?}"
        );
    }
}

/// Cause 2's invariant, over every out-of-enum feedback value: the refusal leaves the
/// finding's `Service.UserFeedback` unset, and a following valid value marks it.
#[tokio::test]
async fn a_refused_findings_feedback_leaves_the_finding_unmarked() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": ["Recon:EC2/PortProbeUnprotectedPort"]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );

    let (status, listed) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "list-findings: got {status} {listed:?}");
    let finding_id = listed["findingIds"][0]
        .as_str()
        .unwrap_or_else(|| panic!("list-findings: expected a finding id, got {listed:?}"))
        .to_string();

    /// Read one finding back and answer its `service.userFeedback`.
    async fn user_feedback(
        router: &smithy_server_runtime::MultiServiceRouter,
        detector_id: &str,
        finding_id: &str,
    ) -> Option<String> {
        let (status, got) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/findings/get"),
            Some(json!({"findingIds": [finding_id]})),
        )
        .await;
        assert_eq!(status, 200, "get-findings: got {status} {got:?}");
        got["findings"][0]["service"]["userFeedback"]
            .as_str()
            .map(str::to_string)
    }

    assert_eq!(
        user_feedback(&router, &detector_id, &finding_id).await,
        None,
        "a finding nobody has given feedback on must report none"
    );

    // Every out-of-enum value is refused, and none of them marks the finding.
    for feedback in ["MAYBE_USEFUL", "not_useful", "", "USEFUL,NOT_USEFUL"] {
        let case = format!("UpdateFindingsFeedback with feedback {feedback:?}");
        let (status, refused) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/feedback"),
            Some(json!({"findingIds": [finding_id.clone()], "feedback": feedback})),
        )
        .await;
        assert_modeled_refusal(
            &case,
            status,
            &refused,
            state::UNPROCESSABLE_PAYLOAD_MESSAGE,
        );
        assert_eq!(
            user_feedback(&router, &detector_id, &finding_id).await,
            None,
            "case {case}: a refused call must leave the finding unmarked"
        );
    }

    // A valid value, for each member of the enum, is recorded and read back.
    for feedback in ["USEFUL", "NOT_USEFUL", "USEFUL"] {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/feedback"),
            Some(json!({
                "findingIds": [finding_id.clone()],
                "feedback": feedback,
                "comments": "noted by the test",
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateFindingsFeedback {feedback}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            user_feedback(&router, &detector_id, &finding_id).await.as_deref(),
            Some(feedback),
            "case UpdateFindingsFeedback {feedback}: expected the feedback to be reported back"
        );
    }

    // A finding id the detector does not hold is not an error and marks nothing.
    let (status, response) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/feedback"),
        Some(json!({"findingIds": ["ffffffffffffffffffffffffffffffff"], "feedback": "USEFUL"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateFindingsFeedback on an id the detector does not hold: got {status} {response:?}"
    );
}

// ---------------------------------------------------------------------------
// The filter round trip the two causes are observed through.
// ---------------------------------------------------------------------------

/// Everything `CreateFilter` stores is reported by `GetFilter`, and the rules the
/// model cannot express are refused by the handler under the service's own error
/// class.
#[tokio::test]
async fn a_saved_filter_is_reported_as_it_was_stored() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let criteria = criteria();
    let (status, created) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({
            "name": "round.trip_filter-1",
            "description": "severity at least one",
            "action": "ARCHIVE",
            "rank": 42,
            "findingCriteria": criteria.clone(),
            "tags": {"team": "security"},
        })),
    )
    .await;
    assert_eq!(status, 200, "create-filter: got {status} {created:?}");
    assert_eq!(
        created["name"].as_str(),
        Some("round.trip_filter-1"),
        "create-filter: got {created:?}"
    );

    let (status, read) = call(
        &router,
        "GET",
        &format!("/detector/{detector_id}/filter/round.trip_filter-1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "get-filter: got {status} {read:?}");
    for (member, expected) in [
        ("name", json!("round.trip_filter-1")),
        ("description", json!("severity at least one")),
        ("action", json!("ARCHIVE")),
        ("rank", json!(42)),
        (
            "findingCriteria",
            // A read reports every condition under both spellings of its pair
            // (`filter_condition_spellings`).
            crate::filter_condition_spellings::reported_criteria(&criteria),
        ),
        ("tags", json!({"team": "security"})),
        ("version", json!(1)),
    ] {
        assert_eq!(
            read[member], expected,
            "get-filter {member}: expected {expected}, got {read:?}"
        );
    }
    assert!(
        read["createdAt"].is_number() && read["updatedAt"].is_number(),
        "get-filter: expected timestamps for the saved filter, got {read:?}"
    );

    // An action the request did not name takes the documented default.
    let (status, created) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(create_filter_body("default-action", &[])),
    )
    .await;
    assert_eq!(status, 200, "create-filter without action: got {status} {created:?}");
    let (_, read) = call(
        &router,
        "GET",
        &format!("/detector/{detector_id}/filter/default-action"),
        None,
    )
    .await;
    assert_eq!(
        read["action"],
        json!("NOOP"),
        "get-filter on a filter created without an action: expected the documented default, got {read:?}"
    );
    assert!(
        read["rank"].is_null(),
        "get-filter on a filter created without a rank: expected no rank, got {read:?}"
    );

    // A name the model's length bounds admit but the documented alphabet does not is
    // refused by the handler, under the service's own error class.
    for (name, why) in [
        ("has a space", "a whitespace is an invalid character"),
        ("has/slash", "a forward slash is not in the alphabet"),
        ("has:colon", "a colon is not in the alphabet"),
        ("has+plus", "a plus is not in the alphabet"),
    ] {
        let (status, refused) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(create_filter_body(name, &[])),
        )
        .await;
        assert_eq!(
            status, 400,
            "case CreateFilter name {name:?} ({why}): expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_shape(&refused),
            MODELED_CLIENT_ERROR,
            "case CreateFilter name {name:?} ({why}): expected the modelled envelope, got {refused:?}"
        );
        assert_ne!(
            error_code(&refused),
            UNMODELED_CODE,
            "case CreateFilter name {name:?} ({why}): refused under an unmodelled code, got {refused:?}"
        );
    }

    // A second create under a name the detector already holds is refused; the saved
    // filter is untouched.
    let (status, refused) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(create_filter_body("round.trip_filter-1", &[])),
    )
    .await;
    assert_eq!(
        status, 400,
        "create-filter under an existing name: expected 400, got {status} {refused:?}"
    );
    let (_, read) = call(
        &router,
        "GET",
        &format!("/detector/{detector_id}/filter/round.trip_filter-1"),
        None,
    )
    .await;
    assert_eq!(
        read["action"],
        json!("ARCHIVE"),
        "create-filter under an existing name must not overwrite it, got {read:?}"
    );

    // A replayed `ClientToken` returns the filter the first call saved.
    let token = "a-client-token-the-test-chose";
    let (status, first) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(create_filter_body(
            "token-filter",
            &[("clientToken", json!(token))],
        )),
    )
    .await;
    assert_eq!(status, 200, "create-filter with a token: got {status} {first:?}");
    let (status, replayed) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(create_filter_body(
            "token-filter-under-another-name",
            &[("clientToken", json!(token))],
        )),
    )
    .await;
    assert_eq!(status, 200, "replayed create-filter: got {status} {replayed:?}");
    assert_eq!(
        replayed["name"], first["name"],
        "a replayed clientToken must answer with the filter the first call saved, got {replayed:?}"
    );

    // A filter name the detector does not hold is refused, never a hollow 200.
    for name in [
        "never-created".to_string(),
        "n".to_string(),
        "never.created_2".to_string(),
        name_of(200),
    ] {
        let (status, read) = call(
            &router,
            "GET",
            &format!("/detector/{detector_id}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case GetFilter {name:?}: expected 400, got {status} {read:?}"
        );
    }
}

/// "The maximum number of saved filters per Amazon Web Services account per Region is
/// 100" (`smithy_query(query="doc",shape="CreateFilter")`). A detector is itself one
/// per account per Region, so the 101st filter on it is refused.
#[tokio::test]
async fn the_published_filter_quota_is_enforced() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for index in 0..100 {
        let (status, created) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(create_filter_body(&format!("quota-{index}"), &[])),
        )
        .await;
        assert_eq!(
            status, 200,
            "filter {index} of the published 100: expected 200, got {status} {created:?}"
        );
    }

    let (status, refused) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(create_filter_body("quota-101", &[])),
    )
    .await;
    assert_eq!(
        status, 400,
        "the 101st filter: expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_shape(&refused),
        MODELED_CLIENT_ERROR,
        "the 101st filter: expected the modelled envelope, got {refused:?}"
    );
}
