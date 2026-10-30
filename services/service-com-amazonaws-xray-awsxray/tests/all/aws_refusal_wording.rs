// SPDX-License-Identifier: CC0-1.0

//! What X-Ray's refusals and declined-report notes SAY, end to end over the wire
//! (the `Xray` harness in [`super::xray_wire`]), for the four message classes
//! whose text a client branches on.
//!
//! One rule, four classes: the message of a refusal (or of a per-item note
//! inside a 2xx) is AWS's own wording for that class — its fixed text verbatim,
//! and only the parts AWS takes from the request or from state interpolated into
//! it. So:
//!
//! * `PutEncryptionConfig` selecting KMS encryption without naming a key states
//!   the FIXED sentence for that condition, into which nothing of the request
//!   reaches — so the same text whatever region the call lands in, whatever the
//!   account already had stored, and however the absent key was expressed;
//! * a value outside a modelled constraint is stated in AWS's validation
//!   envelope, whose `Value '<value>'` clause QUOTES the offending value — a
//!   collection rendered the way a collection renders, `[<element>, …]`, in
//!   request order — beside the lowerCamel member path and the bound from the
//!   model; a collection whose elements have no such rendering (a list of
//!   structures) keeps the valueless clause, because a value stating fewer
//!   elements than were sent would be worse than none;
//! * a tagging call whose `ResourceARN` names no resource states the
//!   not-found template with the ARN echoed verbatim and undecorated, the same
//!   sentence from all three tagging operations; and
//! * a `GetSamplingTargets` statistics document naming a rule the account does
//!   not hold is declined with a FIXED note, which is NOT the sentence a
//!   whole-request refusal for a missing rule carries.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_sampling_targets::UNKNOWN_RULE_ERROR_CODE;
use service_com_amazonaws_xray_awsxray::state::{
    resource_not_found_message, ENCRYPTION_KEY_ID_REQUIRED_MESSAGE,
    SAMPLING_RULE_NOT_FOUND_MESSAGE, UNKNOWN_SAMPLING_RULE_MESSAGE,
};

use super::xray_wire::{trace_id, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

// =============================================================================
// 1. PutEncryptionConfig(Type=KMS) with no key states one fixed sentence
// =============================================================================

#[tokio::test]
async fn kms_encryption_without_a_key_is_refused_with_the_fixed_sentence() {
    // The sentence interpolates nothing, so it cannot vary from call to call.
    assert_eq!(
        ENCRYPTION_KEY_ID_REQUIRED_MESSAGE, "Key ID is required for type KMS",
        "the fixed text must be the service's own, got {ENCRYPTION_KEY_ID_REQUIRED_MESSAGE:?}"
    );

    // Rows differ along everything the request and the account can vary that the
    // message does NOT carry: the region the call is signed for, and whether the
    // account already holds a configuration (and which) that the refusal must
    // leave alone.
    let cases: [(&str, &str, Option<Value>); 5] = [
        ("a fresh account", "us-east-1", None),
        ("another region", "eu-west-1", None),
        ("a region in another hemisphere", "ap-southeast-2", None),
        (
            "an account already encrypting with a key",
            "us-east-1",
            Some(json!({"Type": "KMS", "KeyId": "alias/Seed"})),
        ),
        (
            "an account explicitly on default encryption",
            "eu-west-1",
            Some(json!({"Type": "NONE"})),
        ),
    ];

    for (case, region, seeded) in cases {
        let xray = Xray::new();
        if let Some(seed) = seeded {
            let put = xray
                .call_in_region(region, "/PutEncryptionConfig", &seed)
                .await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: seeding a configuration must succeed, got {put:?}"
            );
        }

        let refused = xray
            .call_in_region(region, "/PutEncryptionConfig", &json!({"Type": "KMS"}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            said(&refused),
            ENCRYPTION_KEY_ID_REQUIRED_MESSAGE,
            "case {case:?}: expected {ENCRYPTION_KEY_ID_REQUIRED_MESSAGE:?}, got {:?}",
            said(&refused)
        );
    }
}

// =============================================================================
// 2. A constraint clause quotes the value the caller submitted
// =============================================================================

/// AWS's one-error validation envelope over a `@length` MAXIMUM clause: the
/// value as submitted (rendered, or absent when the walk has no rendering for
/// it), the lowerCamel member path, and the bound the model declares.
///
/// Spelled here rather than read off the service, so the test pins the text
/// instead of restating whatever the service produced.
fn too_long_clause(value: Option<&str>, member_path: &str, max: usize) -> String {
    let stated = match value {
        Some(value) => format!("Value '{value}' "),
        None => "Value ".to_string(),
    };
    format!(
        "1 validation error detected: {stated}at '{member_path}' failed to satisfy constraint: \
         Member must have length less than or equal to {max}"
    )
}

/// `n` distinct trace ids, in the order they will be sent.
fn trace_ids(n: u32) -> Vec<String> {
    (1..=n).map(trace_id).collect()
}

/// `n` distinct tag keys, in the order they will be sent.
fn tag_keys(n: u32) -> Vec<String> {
    (1..=n).map(|index| format!("Key{index}")).collect()
}

#[tokio::test]
async fn a_collection_over_its_length_maximum_is_refused_quoting_the_collection() {
    let xray = Xray::new();

    // The bounds come from the model, not from this file:
    // `InsightStateList` @length 0..1, `TraceIdListForRetrieval` @length 0..100,
    // `TagKeyList` @length 0..200 and `TagList` @length 0..200
    // (`smithy_query(query="shape",shape="InsightStateList,TraceIdListForRetrieval,
    // TagKeyList,TagList")`), and `InsightState` admits exactly ACTIVE and
    // CLOSED.
    let long_ids = trace_ids(101);
    let long_keys = tag_keys(201);
    let rendered_ids = format!("[{}]", long_ids.join(", "));
    let rendered_keys = format!("[{}]", long_keys.join(", "));
    let structures: Vec<Value> = tag_keys(201)
        .iter()
        .map(|key| json!({"Key": key, "Value": "v"}))
        .collect();

    // Rows differ along what the rule generalises over: which operation and
    // member carries the collection, the TYPE of its elements (enum, string,
    // structure), how long the collection is, how far past the bound it reaches,
    // and the ORDER the elements were sent in.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "two insight states where the model admits one",
            "/InsightSummaries",
            json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "States": ["ACTIVE", "CLOSED"],
            }),
            too_long_clause(Some("[ACTIVE, CLOSED]"), "states", 1),
        ),
        (
            "the same two states in the other order",
            "/InsightSummaries",
            json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "States": ["CLOSED", "ACTIVE"],
            }),
            too_long_clause(Some("[CLOSED, ACTIVE]"), "states", 1),
        ),
        (
            "three states, one of them repeated",
            "/InsightSummaries",
            json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "States": ["ACTIVE", "CLOSED", "ACTIVE"],
            }),
            too_long_clause(Some("[ACTIVE, CLOSED, ACTIVE]"), "states", 1),
        ),
        (
            "one trace id past the retrieval maximum",
            "/StartTraceRetrieval",
            json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "TraceIds": long_ids,
            }),
            too_long_clause(Some(&rendered_ids), "traceIds", 100),
        ),
        (
            "one tag key past the maximum",
            "/UntagResource",
            json!({
                "ResourceARN": "arn:aws:xray:us-east-1:000000000000:group/absent/000000000000",
                "TagKeys": long_keys,
            }),
            too_long_clause(Some(&rendered_keys), "tagKeys", 200),
        ),
        (
            // The fail-safe direction: a list of STRUCTURES has no rendering as
            // a value, so the clause states none rather than one missing a part
            // of itself.
            "one tag past the maximum, as structures",
            "/TagResource",
            json!({
                "ResourceARN": "arn:aws:xray:us-east-1:000000000000:group/absent/000000000000",
                "Tags": structures,
            }),
            too_long_clause(None, "tags", 200),
        ),
    ];

    for (case, path, body, expected) in cases {
        let refused = xray.call(path, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            said(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            said(&refused)
        );
    }
}

#[tokio::test]
async fn a_scalar_outside_its_constraint_still_quotes_the_scalar() {
    let xray = Xray::new();

    // The same clause, on the members whose value the walk could always render:
    // a number outside `GetInsightSummariesMaxResults` @range 1..100 and a
    // string outside `GroupName` @length 1..32
    // (`smithy_query(query="operation",operation="GetInsightSummaries")`). Rows
    // sit on each published bound and one past it, so the accepted edge and the
    // refused one are both covered.
    let long_name = "g".repeat(33);
    let refusals: Vec<(&str, Value, String)> = vec![
        (
            "MaxResults one below the minimum",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "MaxResults": 0}),
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "MaxResults one above the maximum",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "MaxResults": 101}),
            "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "a GroupName one character past its maximum",
            json!({
                "StartTime": now() - 1.0,
                "EndTime": now(),
                "GroupName": long_name.clone(),
            }),
            too_long_clause(Some(&long_name), "groupName", 32),
        ),
    ];

    for (case, body, expected) in refusals {
        let refused = xray.call("/InsightSummaries", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            said(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            said(&refused)
        );
    }

    // And the bounds themselves are accepted, so the rows above are refusals of
    // the value and not of the member: the account holds the reserved `Default`
    // group, which `GetInsightSummaries` resolves.
    for (case, max_results) in [("the minimum", 1), ("the maximum", 100)] {
        let answered = xray
            .call(
                "/InsightSummaries",
                &json!({
                    "StartTime": now() - 3_600.0,
                    "EndTime": now(),
                    "GroupName": "Default",
                    "MaxResults": max_results,
                }),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: MaxResults {max_results} is inside the published range, got {answered:?}"
        );
    }
}

// =============================================================================
// 3. A tagging call on an ARN naming no resource states one template
// =============================================================================

/// Create a group and answer the ARN it persisted. Every group carries a filter
/// expression, because `CreateGroup` refuses one that supplies none.
async fn create_group(xray: &Xray, name: &str) -> String {
    let created = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": name, "FilterExpression": "ok"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating group {name:?} must succeed, got {created:?}"
    );
    created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGroup must answer a GroupARN, got {created:?}"))
        .to_string()
}

#[tokio::test]
async fn an_unresolvable_tag_target_is_reported_with_the_not_found_template() {
    let xray = Xray::new();

    // One ARN that WAS a resource and is not any more, so the row is not just a
    // string nothing ever matched.
    let deleted = create_group(&xray, "doomed").await;
    let gone = xray
        .call("/DeleteGroup", &json!({"GroupARN": &deleted}))
        .await;
    assert_eq!(
        gone.status, 200,
        "deleting the group must succeed, got {gone:?}"
    );

    // Rows differ along the FORM of the unresolvable ARN: a deleted group's, a
    // group name never created, another X-Ray resource type, another vendor's
    // ARN, and the shortest value `AmazonResourceName` @length admits (min 1).
    let arns: Vec<(&str, String)> = vec![
        ("the ARN of a deleted group", deleted),
        (
            "a group name never created",
            "arn:aws:xray:us-east-1:000000000000:group/absent/000000000000".to_string(),
        ),
        (
            "a sampling-rule ARN",
            "arn:aws:xray:us-east-1:000000000000:sampling-rule/absent".to_string(),
        ),
        ("another vendor's ARN", "arn:aws:s3:::some-bucket".to_string()),
        ("the shortest ARN the model allows", "x".to_string()),
    ];

    // Every operation that resolves a `ResourceARN` words the miss the same way.
    for (case, arn) in &arns {
        let calls = [
            (
                "/TagResource",
                json!({"ResourceARN": arn, "Tags": [{"Key": "Stage", "Value": "Production"}]}),
            ),
            (
                "/UntagResource",
                json!({"ResourceARN": arn, "TagKeys": ["Stage"]}),
            ),
            ("/ListTagsForResource", json!({"ResourceARN": arn})),
        ];
        let expected = resource_not_found_message(arn);
        // The template is fixed text around the ARN as submitted, undecorated.
        assert_eq!(
            expected,
            format!("Resource {arn} not found"),
            "case {case:?}: the template must echo the ARN verbatim, got {expected:?}"
        );
        for (path, body) in &calls {
            let refused = xray.call(path, body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} on {path}: expected 404 ResourceNotFoundException, got {refused:?}"
            );
            assert_eq!(
                said(&refused),
                expected,
                "case {case:?} on {path}: expected {expected:?}, got {:?}",
                said(&refused)
            );
            assert_eq!(
                refused.get("ResourceName").as_str(),
                Some(arn.as_str()),
                "case {case:?} on {path}: the refusal must name the ARN it could not resolve, \
                 got {refused:?}"
            );
        }
    }
}

// =============================================================================
// 4. A statistics document naming an unknown rule carries the fixed note
// =============================================================================

/// A sampling rule definition body for `CreateSamplingRule`.
fn rule_body(name: &str, priority: i64) -> Value {
    json!({
        "SamplingRule": {
            "RuleName": name,
            "ResourceARN": "*",
            "Priority": priority,
            "FixedRate": 0.05,
            "ReservoirSize": 1,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            // The model documents the sampling rule format as `1`.
            "Version": 1,
        }
    })
}

/// One `SamplingStatisticsDocument` for `rule_name`. `ClientID` is 24
/// hexadecimal characters, the only length the model accepts.
fn statistics(rule_name: &str, client: u32) -> Value {
    json!({
        "RuleName": rule_name,
        "ClientID": format!("{client:024x}"),
        "Timestamp": now(),
        "RequestCount": 10,
        "SampledCount": 1,
        "BorrowCount": 0,
    })
}

#[tokio::test]
async fn an_unknown_rules_statistics_document_carries_the_fixed_note() {
    // The note is NOT the whole-request refusal's sentence: a declined report
    // inside a 200 and a refused call are two classes.
    assert_eq!(
        UNKNOWN_SAMPLING_RULE_MESSAGE, "Unknown rule",
        "the fixed note must be the service's own, got {UNKNOWN_SAMPLING_RULE_MESSAGE:?}"
    );
    assert_ne!(
        UNKNOWN_SAMPLING_RULE_MESSAGE, SAMPLING_RULE_NOT_FOUND_MESSAGE,
        "a declined report and a refused call are different classes, so their text differs"
    );

    let xray = Xray::new();
    let created = xray.call("/CreateSamplingRule", &rule_body("known", 100)).await;
    assert_eq!(
        created.status, 200,
        "creating the rule that does exist must succeed, got {created:?}"
    );

    // Rows differ along the shape of the name nothing was created under — a
    // plain word, a name that differs from a stored one only in case, one at the
    // longest length `SamplingRule$RuleName` admits (@length 1..32), one at the
    // shortest, and a batch reporting TWO unknown rules at once, so the note is
    // shown to be fixed text rather than anything derived from the name.
    let cases: Vec<(&str, Vec<String>)> = vec![
        (
            "a name nothing was created under",
            vec!["never-created".to_string()],
        ),
        ("the stored name in another case", vec!["KNOWN".to_string()]),
        ("the longest name the model admits", vec!["n".repeat(32)]),
        ("the shortest name the model admits", vec!["n".to_string()]),
        (
            "two unknown rules in one call",
            vec!["first-absent".to_string(), "second-absent".to_string()],
        ),
    ];

    for (case, unknown) in cases {
        let mut documents: Vec<Value> = vec![statistics("known", 7)];
        documents.extend(unknown.iter().map(|name| statistics(name, 7)));
        let answered = xray
            .call(
                "/SamplingTargets",
                &json!({ "SamplingStatisticsDocuments": documents }),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: GetSamplingTargets must still succeed, got {answered:?}"
        );
        let unprocessed = answered
            .get("UnprocessedStatistics")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            unprocessed.len(),
            unknown.len(),
            "case {case:?}: exactly the unknown rules are unprocessed, got {unprocessed:?}"
        );
        for (name, entry) in unknown.iter().zip(&unprocessed) {
            let member = |key: &str| entry.get(key).and_then(Value::as_str);
            assert_eq!(
                member("RuleName"),
                Some(name.as_str()),
                "case {case:?}: the entry must name the rule reported, got {entry:?}"
            );
            assert_eq!(
                member("ErrorCode"),
                Some(UNKNOWN_RULE_ERROR_CODE),
                "case {case:?}: the entry's ErrorCode must be {UNKNOWN_RULE_ERROR_CODE:?}, got \
                 {entry:?}"
            );
            assert_eq!(
                member("Message"),
                Some(UNKNOWN_SAMPLING_RULE_MESSAGE),
                "case {case:?}: the entry's Message must be \
                 {UNKNOWN_SAMPLING_RULE_MESSAGE:?}, got {entry:?}"
            );
        }
    }

    // And the whole-request refusal for a missing rule keeps its own sentence:
    // the two classes did not collapse into one text.
    let refused = xray
        .call("/DeleteSamplingRule", &json!({"RuleName": "never-created"}))
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "deleting a rule that does not exist must be refused, got {refused:?}"
    );
    assert_eq!(
        said(&refused),
        SAMPLING_RULE_NOT_FOUND_MESSAGE,
        "a refused call keeps its own sentence, got {:?}",
        said(&refused)
    );
}
