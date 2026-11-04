// SPDX-License-Identifier: CC0-1.0

//! The refusals GuardDuty words WITHOUT naming the member at fault.
//!
//! A refusal's message is part of the contract a client codes against — it is
//! snapshotted, branched on and scraped — so each class of refusal carries ONE text,
//! reproduced as the service words it. Three surfaces answer a sentence that names
//! neither the member nor the value, and each rule below is asserted over a SET of
//! inputs rather than over one call:
//!
//! 1. A `ResourceArn` this endpoint resolves no taggable resource for is refused with
//!    ONE fixed sentence, whatever made it unresolvable — a detector nothing minted,
//!    another account's, another partition's or another Region's resource, a
//!    sub-resource id the detector does not hold, a sub-resource type GuardDuty does
//!    not tag — on the tag READ, the tag WRITE and the tag REMOVAL alike. The
//!    sentence makes no claim about the account, so a caller with a typo in the ARN
//!    is not pointed at its IAM configuration.
//! 2. A `findingIds` entry that could not be a finding id at all is refused with the
//!    service's generic input-parameter sentence, which names no member — in BOTH
//!    directions of the archive write. An entry that IS of the finding-id form but
//!    names no finding of this detector is a different class, and keeps the
//!    finding-identifier sentence, so the generic one is not a blanket answer.
//! 3. A page token this service did not issue is refused with that same generic
//!    sentence on EVERY paginated operation of this service, whichever way the token
//!    travels (a query parameter or a body member).
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, the modelled-constraint walk
//! and the handlers together.

use serde_json::{json, Value};

use crate::client_rejections::{page_token_refusal, UNUSABLE_INPUT_VALUE};
use crate::detector_lifecycle::{
    all_finding_ids, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{arn_of, tag_map, untag_in, uri_label};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::INVALID_FINDING_IDS_MESSAGE;

/// The Region every request in this suite is signed for, unless a case names another.
const REGION: &str = "us-east-1";

/// A Region whose endpoint therefore serves no detector of this suite's.
const OTHER_REGION: &str = "eu-west-1";

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator and in its `type` member.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE those rejections are enveloped in — what restJson1
/// reports in `x-amzn-errortype`.
const ENVELOPE: &str = "BadRequestException";

/// A well-formed identifier of the kind this service mints (32 hexadecimal
/// characters) that no create here ever minted.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// Assert one refusal: HTTP 400, the error class in the body and the error shape in
/// the header, the exact message, and nothing of the caller's echoed back.
fn assert_refused(case: &str, status: u16, body: &Value, expected: &str, sent: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case:?}: expected the modelled error shape {ENVELOPE}, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    for echoed in sent {
        assert!(
            !message.contains(echoed),
            "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1: one sentence for every unresolvable tag ARN, on all three operations.
// ---------------------------------------------------------------------------

/// One `ListTagsForResource` call — `GET /tags/{ResourceArn}`, a NON-GREEDY URI
/// label (`smithy_query(operation="ListTagsForResource")`).
async fn list_tags_in(router: &Router, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", uri_label(arn)),
        None,
    )
    .await
}

/// The claim rule 1 forbids every tag-surface refusal from making.
const OWNERSHIP_CLAIM: &str = "is not owned by the current account";

#[tokio::test]
async fn an_unresolvable_tag_arn_is_refused_with_one_fixed_sentence() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // Rows span WHY the ARN resolves to nothing, which is what the one sentence has
    // to cover: the scope the ARN claims (account, partition, Region, endpoint), the
    // resource it names (a detector nothing minted, a value no detector id could be,
    // a sub-resource id the detector does not hold) and the KIND of resource (a type
    // this service does not tag). Every row satisfies `GuardDutyArn`'s modelled
    // pattern (`smithy_query(operation="ListTagsForResource")`), so each reaches the
    // handler instead of being refused at the request boundary.
    let rows: Vec<(String, &str, String)> = vec![
        (
            "a detector id nothing minted".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{ABSENT_DETECTOR_ID}")),
        ),
        (
            "a value no detector id could be".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{}", "z".repeat(32))),
        ),
        (
            "another account's detector".to_string(),
            REGION,
            format!("arn:aws:guardduty:{REGION}:111122223333:detector/{detector}"),
        ),
        (
            "another partition's detector".to_string(),
            REGION,
            format!("arn:aws-cn:guardduty:{REGION}:000000000000:detector/{detector}"),
        ),
        (
            "an ARN claiming another Region".to_string(),
            REGION,
            arn_of(OTHER_REGION, &format!("detector/{detector}")),
        ),
        (
            "this Region's ARN at another Region's endpoint".to_string(),
            OTHER_REGION,
            arn_of(REGION, &format!("detector/{detector}")),
        ),
        (
            "a filter the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/filter/{ABSENT_ID}")),
        ),
        (
            "an ipSetId the detector does not hold".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/ipset/{ABSENT_ID}")),
        ),
        (
            "a sub-resource type GuardDuty does not tag".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/widget/{ABSENT_ID}")),
        ),
    ];

    let written = tag_map(&[("Owner", "payments")]);
    for (why, region, arn) in &rows {
        // The READ, the WRITE and the REMOVAL answer the same refusal: the ARN is the
        // same input to all three, so the sentence cannot differ between them.
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags_in(&router, region, arn).await),
            ("TagResource", tag_in(&router, region, arn, &written).await),
            ("UntagResource", untag_in(&router, region, arn, &["Owner"]).await),
        ] {
            let case = format!("{operation} with {why}");
            assert_refused(
                &case,
                status,
                &body,
                INVALID_RESOURCE_ARN_MESSAGE,
                &[arn.as_str(), detector.as_str()],
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(OWNERSHIP_CLAIM),
                "case {case:?}: the refusal must make no account claim, got {message:?}"
            );
        }
    }

    // Not a blanket refusal: the ARN this endpoint DOES resolve still tags and reads.
    let held = arn_of(REGION, &format!("detector/{detector}"));
    let (status, body) = tag_in(&router, REGION, &held, &written).await;
    assert_eq!(
        status, 204,
        "case \"the detector's own ARN\": expected 204, got {status} {body:?}"
    );
    let (status, body) = list_tags_in(&router, REGION, &held).await;
    assert_eq!(
        status, 200,
        "case \"the detector's own ARN\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["tags"], written,
        "case \"the detector's own ARN\": expected the tags the write stored, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a findingIds entry that could not be a finding id.
// ---------------------------------------------------------------------------

/// One archive write in `direction` — `POST
/// /detector/{DetectorId}/findings/{archive|unarchive}`
/// (`smithy_query(operation="ArchiveFindings,UnarchiveFindings")`).
async fn set_archived(
    router: &Router,
    detector_id: &str,
    ids: &[String],
    direction: &str,
) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "POST",
        &format!("/detector/{detector_id}/findings/{direction}"),
        Some(json!({"findingIds": ids})),
    )
    .await
}

/// The finding types the detector under test is populated with. Source: the sample
/// catalogue `CreateSampleFindings` publishes; the rule is about the id, not the
/// type, so the rows only need findings to exist.
const SAMPLE_TYPES: [&str; 2] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Discovery:S3/MaliciousIPCaller",
];

#[tokio::test]
async fn a_finding_id_that_could_not_be_one_is_refused_without_naming_the_member() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let (status, body) = call_in(
        &router,
        REGION,
        "POST",
        &format!("/detector/{detector}/findings/create"),
        Some(json!({"findingTypes": SAMPLE_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );
    let held = all_finding_ids(&router, &detector, 50).await;
    assert_eq!(
        held.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {held:?}"
    );

    // Rows span the FORM of the value: the alphabet, both sides of the id length, and
    // another id convention entirely. A finding id is the 32 hexadecimal characters
    // this service mints, and `FindingIds` is modelled as a plain list of strings
    // (`smithy_query(operation="ArchiveFindings")`), so every row reaches the handler.
    let malformed: [(&str, &str); 5] = [
        ("not-a-finding-id", "not hexadecimal at all"),
        ("abc123", "far shorter than an id"),
        (
            "0123456789abcdef0123456789abcde",
            "31 characters: one short of the id length",
        ),
        (
            "0123456789abcdef0123456789abcdef0",
            "33 characters: one past the id length",
        ),
        (
            "12345678-1234-1234-1234-123456789012",
            "a dashed UUID, which no finding id is",
        ),
    ];

    for (id, why) in malformed {
        // The batch SHAPES: the value alone, and beside an id the detector holds.
        for (shape, batch) in [
            ("alone", vec![id.to_string()]),
            ("after a held id", vec![held[0].clone(), id.to_string()]),
        ] {
            for direction in ["archive", "unarchive"] {
                let case = format!("{direction} {why}, {shape}");
                let (status, body) = set_archived(&router, &detector, &batch, direction).await;
                assert_refused(&case, status, &body, UNUSABLE_INPUT_VALUE, &[id, "findingIds"]);
            }
        }
    }

    // A value that IS of the finding-id form but names no finding of this detector is
    // a different class, so the generic sentence is not a blanket answer.
    for direction in ["archive", "unarchive"] {
        let case = format!("{direction} a well-formed finding id nobody minted");
        let (status, body) =
            set_archived(&router, &detector, &[ABSENT_ID.to_string()], direction).await;
        assert_refused(
            &case,
            status,
            &body,
            INVALID_FINDING_IDS_MESSAGE,
            &[ABSENT_ID],
        );
    }

    // And the ids the detector DOES hold are still written in both directions.
    for direction in ["archive", "unarchive"] {
        let (status, body) = set_archived(&router, &detector, &held, direction).await;
        assert_eq!(
            status, 200,
            "case {direction:?} with every held id: expected 200, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one sentence for an unissued page token, on every paginated report.
// ---------------------------------------------------------------------------

/// How one paginated report carries its `NextToken`: in the query string, or in the
/// request body (`smithy_query(operation="ListDetectors,ListFindings")` — the
/// service binds both ways).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenIn {
    Query,
    Body,
}

/// One paginated report: its operation name, the method and path it is bound to,
/// where its token travels, and the body members it cannot answer without.
struct Report {
    operation: &'static str,
    method: &'static str,
    path: String,
    token_in: TokenIn,
    body: Value,
}

/// Send one page request to `report`, carrying `token` the way that report takes it.
async fn page(router: &Router, report: &Report, token: Option<&str>) -> (u16, Value) {
    let (path, body) = match (report.token_in, token) {
        (TokenIn::Query, Some(token)) => {
            let separator = if report.path.contains('?') { '&' } else { '?' };
            (
                format!("{}{separator}nextToken={token}", report.path),
                report.body.clone(),
            )
        }
        (TokenIn::Body, Some(token)) => {
            let mut body = report.body.clone();
            body["nextToken"] = json!(token);
            (report.path.clone(), body)
        }
        (_, None) => (report.path.clone(), report.body.clone()),
    };
    let payload = if report.method == "GET" {
        None
    } else {
        Some(body)
    };
    call_in(router, REGION, report.method, &path, payload).await
}

/// Every paginated report of this service that a detector in one Region can be asked
/// for, spanning both token bindings and both URI shapes (a collection of its own and
/// one rooted in a detector).
fn reports(detector: &str) -> Vec<Report> {
    let query = |operation: &'static str, method: &'static str, path: String| Report {
        operation,
        method,
        path,
        token_in: TokenIn::Query,
        body: json!({}),
    };
    let body = |operation: &'static str, path: String, body: Value| Report {
        operation,
        method: "POST",
        path,
        token_in: TokenIn::Body,
        body,
    };
    vec![
        query("ListDetectors", "GET", "/detector".to_string()),
        query("ListFilters", "GET", format!("/detector/{detector}/filter")),
        query("ListIPSets", "GET", format!("/detector/{detector}/ipset")),
        query(
            "ListThreatIntelSets",
            "GET",
            format!("/detector/{detector}/threatintelset"),
        ),
        query(
            "ListThreatEntitySets",
            "GET",
            format!("/detector/{detector}/threatentityset"),
        ),
        query(
            "ListTrustedEntitySets",
            "GET",
            format!("/detector/{detector}/trustedentityset"),
        ),
        query(
            "ListPublishingDestinations",
            "GET",
            format!("/detector/{detector}/publishingDestination"),
        ),
        query("ListMembers", "GET", format!("/detector/{detector}/member")),
        query("ListInvitations", "GET", "/invitation".to_string()),
        query("ListOrganizationAdminAccounts", "GET", "/admin".to_string()),
        query(
            "ListMalwareProtectionPlans",
            "GET",
            "/malware-protection-plan".to_string(),
        ),
        query("ListMalwareScans", "POST", "/malware-scan".to_string()),
        body(
            "ListFindings",
            format!("/detector/{detector}/findings"),
            json!({}),
        ),
        body(
            "ListCoverage",
            format!("/detector/{detector}/coverage"),
            json!({}),
        ),
        body(
            "DescribeMalwareScans",
            format!("/detector/{detector}/malware-scans"),
            json!({}),
        ),
        body(
            "ListInvestigations",
            format!("/detector/{detector}/investigation/list"),
            json!({}),
        ),
        body(
            "GetUsageStatistics",
            format!("/detector/{detector}/usage/statistics"),
            json!({"usageStatisticsType": "SUM_BY_FEATURES", "usageCriteria": {}}),
        ),
        body(
            "ListCustomDetectionRules",
            "/custom-detection-rule/rule".to_string(),
            json!({}),
        ),
    ]
}

#[tokio::test]
async fn an_unissued_page_token_is_refused_the_same_way_by_every_paginated_report() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // Rows span the ways a token fails to resolve: not a cursor at all, a negative
    // one, and positions past the end of the report being paged. Each matches the
    // modelled `NextToken` pattern the investigation report declares
    // (`smithy_query(operation="ListInvestigations")`), so every row reaches the
    // handler on every report rather than being refused at the boundary on some.
    let tokens = [
        ("not a cursor at all", "not-a-token"),
        ("a negative position", "-1"),
        ("a position far past the end", "9999"),
        (
            "a position past every list this service can hold",
            "99999999999999999999999999",
        ),
    ];

    for report in reports(&detector) {
        // Not a blanket refusal: the report answers when no token is handed to it.
        let (status, body) = page(&router, &report, None).await;
        assert_eq!(
            status,
            200,
            "case {:?} with no token: expected 200, got {status} {body:?}",
            report.operation
        );

        for (why, token) in tokens {
            let case = format!("{} with {why}", report.operation);
            let (status, body) = page(&router, &report, Some(token)).await;
            assert_refused(&case, status, &body, &page_token_refusal(), &[token, "nextToken"]);
        }
    }
}
