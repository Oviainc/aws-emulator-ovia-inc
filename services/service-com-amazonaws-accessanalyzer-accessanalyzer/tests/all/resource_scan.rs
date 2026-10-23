// SPDX-License-Identifier: CC0-1.0

//! `StartResourceScan`, driven over the wire.
//!
//! The rule under test: a scan asked for against an EXTERNAL access analyzer is recorded
//! on that analyzer and, once the scan's span has elapsed, becomes the resource the
//! analyzer reports as its most recently analyzed one (`AnalyzerSummary$lastResourceAnalyzed`
//! / `lastResourceAnalyzedAt`), while a scan against an analyzer this account does not
//! hold, against an analyzer of any other family, or naming a member outside the form the
//! model documents for it, is refused and records nothing.
//!
//! Every case goes through the real router (the shared wire harness in
//! [`super::harness`]), so restJson1 routing, boundary constraint validation, the handler
//! and the response serialization all run as they do in the emulator binary. Nothing
//! reaches into `crate::state`: a scan is only believed when a later `GetAnalyzer` or
//! `ListAnalyzers` reports it back.
//!
//! The tables span what the rule generalises over — the ARN forms a `resourceArn` takes
//! (every segment layout AWS publishes: a full ARN, one with no account, one with no
//! region or account, a typed resource segment, a resource segment carrying further
//! colons, another partition), every analyzer `Type` the model accepts, whether
//! `resourceOwnerAccount` is carried and the forms it can be malformed in, how many scans
//! one analyzer has been asked for, and how many distinct resources it has scanned. The
//! recorded `arn:aws:s3:::…`-bucket scan of the end-to-end unit is one row.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    ACCOUNT_ID_DIGITS, MAX_SCANNED_RESOURCES_PER_ANALYZER, RESOURCE_SCAN_SPAN,
};

use super::analyzer_lifecycle::ANALYZER_TYPES;
use crate::harness::{
    expected_arn, is_timestamp, timestamp_at_least, wire_reason, Emulator, Reply,
};

/// How long a case waits PAST [`RESOURCE_SCAN_SPAN`] before reading the analyzer back, so
/// a loaded machine cannot read the scan a moment before the clock owes it.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// The `reason` every refusal of this service carries, as the wire spells the model's
/// `fieldValidationFailed` discriminator.
fn field_validation_failed() -> &'static str {
    wire_reason("fieldValidationFailed")
}

/// Create an analyzer of `r#type` and answer the ARN it must carry.
async fn create_analyzer(emulator: &Emulator, name: &str, r#type: &str) -> String {
    let reply = emulator
        .create_analyzer(json!({"analyzerName": name, "type": r#type}))
        .await;
    assert_eq!(
        reply.status, 200,
        "creating the {type} analyzer {name}: expected 200, got {reply:?}"
    );
    expected_arn(name)
}

/// `POST /resource/scan` for one analyzer and resource, optionally naming the owner.
async fn start_scan(
    emulator: &Emulator,
    analyzer_arn: &str,
    resource_arn: &str,
    owner_account: Option<&str>,
) -> Reply {
    let mut body = json!({"analyzerArn": analyzer_arn, "resourceArn": resource_arn});
    if let Some(owner) = owner_account {
        body["resourceOwnerAccount"] = json!(owner);
    }
    emulator.start_resource_scan(body).await
}

/// The `analyzer` member of a `GetAnalyzer` reply.
fn analyzer_of(reply: &Reply) -> &Value {
    reply
        .member("analyzer")
        .unwrap_or_else(|| panic!("expected an analyzer in {:?}", reply.body))
}

/// The analyzer named `name` as a listing reports it.
fn listed_analyzer(reply: &Reply, name: &str) -> Value {
    reply
        .member("analyzers")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
        })
        .cloned()
        .unwrap_or_else(|| panic!("expected {name} in the listing {:?}", reply.body))
}

/// A member of a projected analyzer, treating JSON `null` as absent.
fn member<'a>(analyzer: &'a Value, name: &str) -> Option<&'a Value> {
    analyzer.get(name).filter(|value| !value.is_null())
}

/// The `lastResourceAnalyzed` a projected analyzer reports, `None` when it reports none.
fn last_analyzed(analyzer: &Value) -> Option<&str> {
    member(analyzer, "lastResourceAnalyzed").and_then(Value::as_str)
}

/// RULE: once a scan's span has elapsed, the resource it named is the analyzer's most
/// recently analyzed resource, and the instant it reports is when the scan finished — for
/// every ARN form a resource can be named by, and whether or not the caller named the
/// owning account.
#[tokio::test]
async fn a_finished_scan_becomes_the_analyzers_most_recently_analyzed_resource() {
    let emulator = Emulator::new();

    // (case, resourceArn, resourceOwnerAccount) — the rows differ along the segment
    // layouts an AWS resource ARN takes, and along whether the owner is named.
    let cases: &[(&str, &str, Option<&str>)] = &[
        (
            "an S3 bucket ARN, which carries no region and no account (the recorded case)",
            "arn:aws:s3:::scan-bucket",
            None,
        ),
        (
            "a fully qualified SQS queue ARN",
            "arn:aws:sqs:us-east-1:000000000000:scan-queue",
            None,
        ),
        (
            "an IAM role ARN, which carries no region",
            "arn:aws:iam::000000000000:role/scan-role",
            None,
        ),
        (
            "a KMS key ARN, whose resource segment is typed",
            "arn:aws:kms:us-west-2:000000000000:key/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            None,
        ),
        (
            "a Secrets Manager secret ARN, whose resource segment carries further colons",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:scan-secret-AbCdEf",
            None,
        ),
        (
            "an ARN in another partition",
            "arn:aws-us-gov:sns:us-gov-west-1:000000000000:scan-topic",
            None,
        ),
        (
            "the owner account named explicitly",
            "arn:aws:sqs:us-east-1:000000000000:owned-queue",
            Some("111111111111"),
        ),
        (
            "an owner account that is not the ARN's own account segment",
            "arn:aws:sqs:us-east-1:000000000000:cross-account-queue",
            Some("222222222222"),
        ),
    ];

    // One analyzer per row, so each row's answer is about its own scan alone.
    let mut started: Vec<(&str, String, &str, Value)> = Vec::new();
    for (index, (case, resource_arn, owner)) in cases.iter().enumerate() {
        let name = format!("scan_target_{index}");
        let analyzer_arn = create_analyzer(&emulator, &name, "ACCOUNT").await;
        let created = emulator.get_analyzer(&name).await;
        let created_at = member(analyzer_of(&created), "createdAt").cloned();

        let reply = start_scan(&emulator, &analyzer_arn, resource_arn, *owner).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: starting a scan must answer HTTP 200, got {reply:?}"
        );
        assert!(
            reply.carries_no_content(),
            "case {case:?}: StartResourceScan's output is Unit, so there is nothing to \
             parse, got {reply:?}"
        );
        started.push((
            case,
            name,
            resource_arn,
            created_at.unwrap_or_else(|| panic!("case {case:?}: the analyzer reports a createdAt")),
        ));
    }

    // The clock owes every one of these scans its result.
    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;

    for (case, name, resource_arn, created_at) in &started {
        let reply = emulator.get_analyzer(name).await;
        let analyzer = analyzer_of(&reply);
        assert_eq!(
            last_analyzed(analyzer),
            Some(*resource_arn),
            "case {case:?}: the scanned resource must be the one reported, got {analyzer:?}"
        );
        let analyzed_at = member(analyzer, "lastResourceAnalyzedAt");
        assert!(
            is_timestamp(analyzed_at),
            "case {case:?}: lastResourceAnalyzedAt must be an instant from this run, got \
             {analyzed_at:?}"
        );
        assert!(
            timestamp_at_least(analyzed_at, Some(created_at)),
            "case {case:?}: the scan finished after the analyzer was created, so \
             {analyzed_at:?} must be at or after {created_at:?}"
        );

        // Every reader of the analyzer reports the same derived pair: the listing projects
        // the same record `GetAnalyzer` does.
        let listing = emulator.list_analyzers("").await;
        let listed = listed_analyzer(&listing, name);
        assert_eq!(
            last_analyzed(&listed),
            Some(*resource_arn),
            "case {case:?}: the listing must report the same scanned resource, got {listed:?}"
        );
        assert_eq!(
            member(&listed, "lastResourceAnalyzedAt"),
            analyzed_at,
            "case {case:?}: the listing must report the same instant, got {listed:?}"
        );
    }
}

/// RULE: the scan is not instantaneous — `StartResourceScan` starts it and answers with
/// nothing, so a read taken straight after the call still reports what the analyzer had
/// analyzed BEFORE it, and the new result appears only once the scan's span has elapsed.
#[tokio::test]
async fn a_scan_is_outstanding_until_its_span_has_elapsed() {
    let emulator = Emulator::new();
    let analyzer_arn = create_analyzer(&emulator, "outstanding_scan", "ACCOUNT").await;
    let resource_arn = "arn:aws:s3:::outstanding-bucket";

    let before = emulator.get_analyzer("outstanding_scan").await;
    assert_eq!(
        last_analyzed(analyzer_of(&before)),
        None,
        "an analyzer that has scanned nothing reports no lastResourceAnalyzed, got {before:?}"
    );
    assert_eq!(
        member(analyzer_of(&before), "lastResourceAnalyzedAt"),
        None,
        "an analyzer that has scanned nothing reports no lastResourceAnalyzedAt, got {before:?}"
    );

    let reply = start_scan(&emulator, &analyzer_arn, resource_arn, None).await;
    assert_eq!(
        reply.status, 200,
        "starting the scan must answer HTTP 200, got {reply:?}"
    );

    let outstanding = emulator.get_analyzer("outstanding_scan").await;
    assert_eq!(
        last_analyzed(analyzer_of(&outstanding)),
        None,
        "a scan that is not due yet has analyzed nothing, so the analyzer still reports no \
         lastResourceAnalyzed, got {outstanding:?}"
    );

    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;

    let settled = emulator.get_analyzer("outstanding_scan").await;
    assert_eq!(
        last_analyzed(analyzer_of(&settled)),
        Some(resource_arn),
        "once the scan is due the analyzer reports the resource it scanned, got {settled:?}"
    );
}

/// RULE: the analyzer reports the scan that finished MOST recently — a later scan of
/// another resource replaces the answer, and rescanning a resource it already scanned
/// makes that resource the answer again, at a later instant.
#[tokio::test]
async fn the_most_recently_finished_scan_is_the_one_reported() {
    let emulator = Emulator::new();
    let analyzer_arn = create_analyzer(&emulator, "sequenced_scans", "ACCOUNT").await;
    let first = "arn:aws:s3:::first-bucket";
    let second = "arn:aws:sqs:us-east-1:000000000000:second-queue";

    // (case, the resource scanned, the resource expected afterwards)
    let sequence: &[(&str, &str, &str)] = &[
        ("the first scan of the analyzer", first, first),
        ("a scan of a second resource", second, second),
        ("a rescan of the first resource", first, first),
    ];

    let mut previous_instant: Option<Value> = None;
    for (case, scanned, expected) in sequence {
        let reply = start_scan(&emulator, &analyzer_arn, scanned, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: starting the scan must answer HTTP 200, got {reply:?}"
        );
        tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;

        let settled = emulator.get_analyzer("sequenced_scans").await;
        let analyzer = analyzer_of(&settled);
        assert_eq!(
            last_analyzed(analyzer),
            Some(*expected),
            "case {case:?}: the most recently finished scan is the one reported, got {analyzer:?}"
        );
        let analyzed_at = member(analyzer, "lastResourceAnalyzedAt").cloned();
        if let Some(previous) = &previous_instant {
            assert!(
                timestamp_at_least(analyzed_at.as_ref(), Some(previous)),
                "case {case:?}: a later scan finished later, so {analyzed_at:?} must be at \
                 or after {previous:?}"
            );
        }
        previous_instant = analyzed_at;
    }
}

/// RULE: the action is supported only for EXTERNAL access analyzers, so an analyzer of the
/// unused-access or internal-access family is refused with `ValidationException` (HTTP
/// 400) and records no scan; the refusal is about the analyzer the request names rather
/// than one malformed member, so it carries no `fieldList`.
#[tokio::test]
async fn a_scan_requires_an_external_access_analyzer() {
    let emulator = Emulator::new();
    let resource_arn = "arn:aws:s3:::family-bucket";

    // Every `Type` the model accepts; `ACCOUNT` and `ORGANIZATION` are the external
    // access analyzers and the other four configure unused or internal access.
    let external: &[&str] = &["ACCOUNT", "ORGANIZATION"];
    let mut accepted: Vec<&str> = Vec::new();
    let mut refused: Vec<&str> = Vec::new();
    for analyzer_type in ANALYZER_TYPES {
        let name = format!("family_{}", analyzer_type.to_lowercase());
        let analyzer_arn = create_analyzer(&emulator, &name, analyzer_type).await;
        let reply = start_scan(&emulator, &analyzer_arn, resource_arn, None).await;

        if external.contains(analyzer_type) {
            assert_eq!(
                reply.status, 200,
                "case {analyzer_type:?}: an external access analyzer scans a resource, got \
                 {reply:?}"
            );
            accepted.push(analyzer_type);
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {analyzer_type:?}: only an external access analyzer scans a resource, so \
             this must be refused with HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {analyzer_type:?}: the refusal must name the modelled error, got {reply:?}"
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some(field_validation_failed()),
            "case {analyzer_type:?}: the refusal must carry the modelled reason, got {reply:?}"
        );
        assert!(
            reply.member("fieldList").is_none(),
            "case {analyzer_type:?}: no single member is at fault, so there is no fieldList, \
             got {reply:?}"
        );
        refused.push(analyzer_type);
    }
    assert_eq!(
        accepted.len() + refused.len(),
        ANALYZER_TYPES.len(),
        "every modelled analyzer type is covered: accepted {accepted:?}, refused {refused:?}"
    );

    // Past the span, a refused scan must still have recorded nothing, while an accepted
    // one is reported.
    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;
    for analyzer_type in ANALYZER_TYPES {
        let name = format!("family_{}", analyzer_type.to_lowercase());
        let reply = emulator.get_analyzer(&name).await;
        let expected = external
            .contains(analyzer_type)
            .then_some(resource_arn);
        assert_eq!(
            last_analyzed(analyzer_of(&reply)),
            expected,
            "case {analyzer_type:?}: expected lastResourceAnalyzed {expected:?}, got {reply:?}"
        );
    }
}

/// RULE: the scan is scoped by `analyzerArn`, so an ARN this account and Region holds no
/// analyzer for is `ResourceNotFoundException` (HTTP 404) — a name that never existed, one
/// whose analyzer has been deleted, and an ARN naming another account or Region alike —
/// and the refusal carries the ARN asked for in the modelled `resourceId`.
#[tokio::test]
async fn a_scan_names_an_analyzer_this_account_holds() {
    let emulator = Emulator::new();
    let resource_arn = "arn:aws:s3:::missing-analyzer-bucket";
    let account = engine::emulator_account_id();

    // An analyzer that existed and was deleted is one row, so its ARN is minted the way
    // every other analyzer ARN is.
    let deleted = create_analyzer(&emulator, "deleted_analyzer", "ACCOUNT").await;
    let delete = emulator.delete_analyzer("deleted_analyzer").await;
    assert!(
        delete.status < 300,
        "deleting the analyzer must succeed, got {delete:?}"
    );
    // One live analyzer, so the refusals below are about the ARN and not about an empty
    // store.
    let live = create_analyzer(&emulator, "live_analyzer", "ACCOUNT").await;

    let cases: Vec<(&str, String)> = vec![
        ("an analyzer name that never existed", expected_arn("absent")),
        ("an analyzer that has been deleted", deleted),
        (
            "a live analyzer's name in another Region",
            format!("arn:aws:access-analyzer:eu-west-1:{account}:analyzer/live_analyzer"),
        ),
        (
            "a live analyzer's name in another account",
            "arn:aws:access-analyzer:us-east-1:999999999999:analyzer/live_analyzer".to_string(),
        ),
        (
            "a live analyzer's name in another partition",
            format!("arn:aws-cn:access-analyzer:us-east-1:{account}:analyzer/live_analyzer"),
        ),
    ];

    for (case, analyzer_arn) in &cases {
        let reply = start_scan(&emulator, analyzer_arn, resource_arn, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an analyzer this account does not hold must be refused with HTTP \
             404, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ResourceNotFoundException",
            "case {case:?}: the refusal must name the modelled error, got {reply:?}"
        );
        assert_eq!(
            reply.member("resourceId").and_then(Value::as_str),
            Some(analyzer_arn.as_str()),
            "case {case:?}: the refusal must carry the ARN asked for, got {reply:?}"
        );
        assert_eq!(
            reply.member("resourceType").and_then(Value::as_str),
            Some("analyzer"),
            "case {case:?}: the refusal must name the resource kind, got {reply:?}"
        );
    }

    // No refusal recorded a scan against the analyzer that does exist.
    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;
    let settled = emulator.get_analyzer("live_analyzer").await;
    assert_eq!(
        last_analyzed(analyzer_of(&settled)),
        None,
        "a refused scan records nothing anywhere, got {settled:?}"
    );
    assert_eq!(
        live,
        expected_arn("live_analyzer"),
        "the live analyzer keeps the ARN the create reported"
    );
}

/// RULE: a member is refused when it is not the form the model documents for it — a
/// `resourceArn` whose resource segment names nothing, and a `resourceOwnerAccount` that
/// is not a 12-digit account ID — with `ValidationException` (HTTP 400) naming the member
/// in `fieldList`; a well-formed account ID, and no account ID at all, are both accepted.
#[tokio::test]
async fn a_scan_refuses_a_member_that_is_not_the_form_it_documents() {
    let emulator = Emulator::new();
    let analyzer_arn = create_analyzer(&emulator, "member_forms", "ACCOUNT").await;
    let resource_arn = "arn:aws:sqs:us-east-1:000000000000:forms-queue";
    // The documented account-ID width, read off the one constant the handler bounds the
    // member by, so the rows move with it.
    let digits = ACCOUNT_ID_DIGITS;
    let too_short = "0".repeat(digits - 1);
    let too_long = "0".repeat(digits + 1);
    let exactly = "0".repeat(digits);

    // (case, resourceArn, resourceOwnerAccount, the member expected in fieldList — None
    // when the request is accepted)
    let cases: Vec<(String, &str, Option<String>, Option<&str>)> = vec![
        (
            "no owner account at all".to_string(),
            resource_arn,
            None,
            None,
        ),
        (
            format!("an owner account of exactly {digits} digits"),
            resource_arn,
            Some(exactly),
            None,
        ),
        (
            "an owner account that is another account".to_string(),
            resource_arn,
            Some("123456789012".to_string()),
            None,
        ),
        (
            format!("an owner account one digit short of {digits}"),
            resource_arn,
            Some(too_short),
            Some("resourceOwnerAccount"),
        ),
        (
            format!("an owner account one digit past {digits}"),
            resource_arn,
            Some(too_long),
            Some("resourceOwnerAccount"),
        ),
        (
            "an empty owner account".to_string(),
            resource_arn,
            Some(String::new()),
            Some("resourceOwnerAccount"),
        ),
        (
            "an owner account that is not decimal".to_string(),
            resource_arn,
            Some("abcdefghijkl".to_string()),
            Some("resourceOwnerAccount"),
        ),
        (
            "an owner account of the right width with one non-digit".to_string(),
            resource_arn,
            Some("00000000000a".to_string()),
            Some("resourceOwnerAccount"),
        ),
        (
            "an owner account padded to the right width".to_string(),
            resource_arn,
            Some(" 00000000000".to_string()),
            Some("resourceOwnerAccount"),
        ),
        (
            "a resourceArn whose resource segment is empty".to_string(),
            "arn:aws:s3:::",
            None,
            Some("resourceArn"),
        ),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, resource, owner, expected_member) in &cases {
        let reply = start_scan(&emulator, &analyzer_arn, resource, owner.as_deref()).await;
        let Some(expected_member) = expected_member else {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a well-formed request must be accepted, got {reply:?}"
            );
            accepted.push(case.clone());
            continue;
        };

        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: the refusal must name the modelled error, got {reply:?}"
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some(field_validation_failed()),
            "case {case:?}: the refusal must carry the modelled reason, got {reply:?}"
        );
        let named: Vec<&str> = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            named.contains(expected_member),
            "case {case:?}: fieldList must name {expected_member}, got {named:?} in {reply:?}"
        );
        let message = reply
            .member("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(expected_member),
            "case {case:?}: the message must name the member that failed, got {message:?}"
        );
    }
    assert_eq!(
        accepted.len(),
        3,
        "the three well-formed rows are accepted, got {accepted:?}"
    );

    // Only the accepted rows scanned anything, and they all named the same resource.
    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;
    let settled = emulator.get_analyzer("member_forms").await;
    assert_eq!(
        last_analyzed(analyzer_of(&settled)),
        Some(resource_arn),
        "a refused member records no scan, so the last accepted one still stands, got \
         {settled:?}"
    );
}

/// RULE: the number of distinct resources one analyzer remembers scanning is bounded, and
/// `StartResourceScan` models no quota error, so past the ceiling the call still succeeds
/// and the analyzer still reports the scan that finished most recently.
#[tokio::test]
async fn the_scanned_resource_store_is_bounded() {
    let emulator = Emulator::new();
    let analyzer_arn = create_analyzer(&emulator, "bounded_scans", "ACCOUNT").await;

    // Ten past the ceiling, so the eviction path runs rather than merely being reached.
    let total = MAX_SCANNED_RESOURCES_PER_ANALYZER + 10;
    let mut last = String::new();
    for index in 0..total {
        let resource_arn = format!("arn:aws:sqs:us-east-1:000000000000:bounded-queue-{index}");
        let reply = start_scan(&emulator, &analyzer_arn, &resource_arn, None).await;
        assert_eq!(
            reply.status, 200,
            "case {index}: scanning past the ceiling must still answer HTTP 200, got {reply:?}"
        );
        last = resource_arn;
    }

    tokio::time::sleep(RESOURCE_SCAN_SPAN + SETTLE_MARGIN).await;
    let settled = emulator.get_analyzer("bounded_scans").await;
    assert_eq!(
        last_analyzed(analyzer_of(&settled)),
        Some(last.as_str()),
        "case {total}: the most recently finished scan is still the one reported, got {settled:?}"
    );
}
