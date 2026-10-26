// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of a refusal this service answers for a value the model's own constraints
//! reject.
//!
//! `validation_message_rendering.rs` pins how such a refusal is WORDED. This suite pins
//! the rest of the modelled error: `ValidationException` declares `message` AND `reason`
//! required, plus `fieldList` ("The field that caused the error, if applicable"), so a
//! body carrying only `__type` and `message` is missing a member every SDK deserializes
//! and a client branches on. One rule, over every operation and every member:
//!
//! * the status is 400 and the code is `ValidationException` (what the model declares);
//! * `reason` is `ValidationExceptionReason`'s `fieldValidationFailed`, because what was
//!   refused is a field of the request;
//! * `fieldList` carries one entry per refused member, naming it and stating the clause
//!   its value failed — the SAME clause the message states, so the two cannot disagree;
//! * no clause echoes the rejected value back.
//!
//! The rows differ along what the rule generalises over: the constraint kind (numeric
//! range floor and ceiling, string length floor and ceiling, a pattern, a list bound),
//! where the request carried the value (`@httpQuery` and `@httpLabel`), the member, the
//! operation, the HTTP method (a read, a removal and a write that also carries a body),
//! and how MANY members one request breaks. Every assertion is derived from the answer
//! itself rather than from a recorded body: the clause the `fieldList` entry states is
//! matched against the member it names, and the message against the clauses.

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap, the signing scope, the senders and the answer type come from the
// suite harness, so this suite cannot disagree with its siblings about how a request
// reaches the service. Only what this rule needs is added here: the two error-shape
// accessors the harness's `Answer` has no reason to carry.
use super::harness::{call, emulator, get, Answer};

/// `ValidationExceptionReason`'s value for a member of the request whose value failed
/// validation. Spelled out here rather than imported from the crate, so a changed constant
/// fails this suite instead of redefining what it asserts.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `message` an error body carries, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The `reason` an error body carries, or the empty string when it carries none.
fn reason(answer: &Answer) -> &str {
    answer.body["reason"].as_str().unwrap_or_default()
}

/// The `fieldList` entries, as (name, message) pairs, or an empty vector when the body
/// carries no list.
fn field_list(answer: &Answer) -> Vec<(String, String)> {
    answer.body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The first report id the catalog publishes, read back through `ListReports` rather than
/// hard-coded, so a row naming a report names one that really exists and only the member
/// under test is refused.
async fn a_published_report_id(router: &MultiServiceRouter) -> String {
    let listed = get(router, "/v1/report/list").await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["reports"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the catalog must publish a report, got {:?}", listed.body))
        .to_string()
}

/// The whole rule, asserted over one answer: the status, the code, the required `reason`,
/// and a `fieldList` whose entries agree with the message.
///
/// `members` is what each entry must name, in no particular order — the request's own
/// members, which is what a caller fixes the request by. An entry may name a position
/// INSIDE one of them (a list element has its own constraints), so a name is accepted when
/// it begins with the member it belongs to.
fn assert_whole_refusal(case: &str, answer: &Answer, members: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a value outside its modelled constraint must be refused with HTTP \
         400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        reason(answer),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: the modelled shape requires `reason`, and a refused field is \
         {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );

    let fields = field_list(answer);
    assert_eq!(
        fields.len(),
        members.len(),
        "case {case:?}: expected one fieldList entry per refused member ({}), got {:?}",
        members.len(),
        answer.body
    );
    for (name, clause) in &fields {
        assert!(
            members.iter().any(|member| name.starts_with(member)),
            "case {case:?}: the fieldList entry must name one of {members:?}, got {name:?}"
        );
        // The entry states the clause for the member it names, so a client reading the
        // list learns which member failed and what it had to satisfy.
        assert!(
            clause.contains(&format!("at '{name}'")),
            "case {case:?}: the entry for {name:?} must state the clause for that member, \
             got {clause:?}"
        );
        assert!(
            clause.contains("failed to satisfy constraint: Member must"),
            "case {case:?}: the entry for {name:?} must state the requirement its value \
             failed, got {clause:?}"
        );
        // And the message states every clause the list carries: one refusal, not two
        // accounts of it.
        assert!(
            message(answer).contains(clause.as_str()),
            "case {case:?}: the message must state the clause {clause:?}, got {:?}",
            message(answer)
        );
    }
    // The generic half of the wording rule, which the shape must not reintroduce: neither
    // the message nor an entry quotes the value it refused.
    assert!(
        !message(answer).contains("Value '"),
        "case {case:?}: the message must not quote the rejected value, got {:?}",
        message(answer)
    );
    for (name, clause) in &fields {
        assert!(
            !clause.contains("Value '"),
            "case {case:?}: the entry for {name:?} must not quote the rejected value, got \
             {clause:?}"
        );
    }
}

/// One refused request, and the member its refusal must name.
struct Row {
    /// What this row is a case of.
    case: String,
    method: &'static str,
    uri: String,
    /// The request body, for a row whose operation carries one.
    body: Bytes,
    member: &'static str,
}

#[tokio::test]
async fn every_modelled_constraint_refusal_carries_the_whole_validation_exception() {
    let router = emulator();
    let report = a_published_report_id(&router).await;

    // Every member and constraint below is the model's, as `smithy_query` reports it for
    // this service:
    //   ListReports / ListReportVersions / ListCustomerAgreements /
    //     ListComplianceInquiries.maxResults -> MaxResultsAttribute @httpQuery,
    //     @range(min: 1, max: 300)
    //   ListReports.nextToken -> NextTokenAttribute @httpQuery, @length(1..2048)
    //   GetReportMetadata.reportVersion -> VersionAttribute @httpQuery, @range(min: 1)
    //   GetReportMetadata.reportId -> ReportId @httpQuery, @required
    //   GetReport.termToken -> ShortStringAttribute @httpQuery, @length(1..256),
    //     @pattern ^[a-zA-Z0-9_\-\s]*$
    //   UntagResource.resourceArn / TagResource.resourceArn -> LongStringAttribute
    //     @httpLabel, @length(1..2048), @pattern ^[^<>]*$
    //   UntagResource.tagKeys -> TagKeys @httpQuery, @required, @length(1..50)
    let rows = vec![
        Row {
            case: "a page size past its ceiling".to_string(),
            method: "GET",
            uri: "/v1/report/list?maxResults=301".to_string(),
            body: Bytes::new(),
            member: "maxResults",
        },
        Row {
            case: "a page size below its floor".to_string(),
            method: "GET",
            uri: "/v1/report/list?maxResults=0".to_string(),
            body: Bytes::new(),
            member: "maxResults",
        },
        Row {
            case: "the same range on another paginated operation".to_string(),
            method: "GET",
            uri: "/v1/customer-agreement/list?maxResults=301".to_string(),
            body: Bytes::new(),
            member: "maxResults",
        },
        Row {
            case: "the same range on a third paginated operation".to_string(),
            method: "GET",
            uri: "/v1/compliance-inquiry/list?maxResults=0".to_string(),
            body: Bytes::new(),
            member: "maxResults",
        },
        Row {
            case: "the same range on an operation that also names a resource".to_string(),
            method: "GET",
            uri: format!("/v1/report/listVersions?reportId={report}&maxResults=301"),
            body: Bytes::new(),
            member: "maxResults",
        },
        Row {
            case: "a pagination token past its length".to_string(),
            method: "GET",
            uri: format!("/v1/report/list?nextToken={}", "t".repeat(2049)),
            body: Bytes::new(),
            member: "nextToken",
        },
        Row {
            case: "a pagination token below its length".to_string(),
            method: "GET",
            uri: "/v1/report/list?nextToken=".to_string(),
            body: Bytes::new(),
            member: "nextToken",
        },
        Row {
            case: "a version below its range".to_string(),
            method: "GET",
            uri: format!("/v1/report/getMetadata?reportId={report}&reportVersion=0"),
            body: Bytes::new(),
            member: "reportVersion",
        },
        Row {
            case: "a term token past its length".to_string(),
            method: "GET",
            uri: format!(
                "/v1/report/get?reportId={report}&termToken={}",
                "a".repeat(257)
            ),
            body: Bytes::new(),
            member: "termToken",
        },
        Row {
            case: "a term token outside its pattern".to_string(),
            method: "GET",
            uri: format!("/v1/report/get?reportId={report}&termToken=token.with.dots"),
            body: Bytes::new(),
            member: "termToken",
        },
        // A value the URI carried in its PATH, not its query string, and on a method that
        // is neither a read nor a creation: the rule is about where the value came from,
        // not about which verb asked.
        Row {
            case: "a path-bound resource name outside its pattern".to_string(),
            method: "DELETE",
            uri: "/tags/tagged%3Cthing%3E?tagKeys=owner".to_string(),
            body: Bytes::new(),
            member: "resourceArn",
        },
        Row {
            case: "a path-bound resource name past its length".to_string(),
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=owner", "a".repeat(2049)),
            body: Bytes::new(),
            member: "resourceArn",
        },
        Row {
            case: "a list of keys with an element below its length".to_string(),
            method: "DELETE",
            uri: "/tags/tagged-thing?tagKeys=".to_string(),
            body: Bytes::new(),
            member: "tagKeys",
        },
        // And the same refusal on an operation whose request ALSO carries a body the
        // gateway binds: the body here is well formed and the only thing wrong with the
        // request is its path-bound member, so the refusal is still this service's and
        // still carries the whole shape.
        Row {
            case: "a path-bound resource name outside its pattern, with a body".to_string(),
            method: "POST",
            uri: "/tags/tagged%3Cthing%3E".to_string(),
            body: Bytes::from(json!({ "tags": { "owner": "compliance" } }).to_string()),
            member: "resourceArn",
        },
        // A member the shape requires, absent, is NOT a row: the router names no
        // operation for a request that does not carry one (it answers its own "not
        // implemented" for the method and path), so no refusal of this service's is
        // reached for it.
    ];

    for row in rows {
        let answer = call(&router, row.method, &row.uri, row.body.clone()).await;
        assert_whole_refusal(&row.case, &answer, &[row.member]);
    }
}

#[tokio::test]
async fn a_request_breaking_two_constraints_names_both_fields_in_one_refusal() {
    let router = emulator();

    // `fieldList` is a LIST, and the message counts what it carries: a request breaking
    // two members is one refusal naming both, not one that reports whichever was walked
    // first.
    let answer = get(
        &router,
        &format!("/v1/report/list?maxResults=301&nextToken={}", "t".repeat(2049)),
    )
    .await;
    let case = "a page size and a pagination token both outside their constraints";
    assert_whole_refusal(case, &answer, &["maxResults", "nextToken"]);

    let mut named: Vec<String> = field_list(&answer)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec!["maxResults".to_string(), "nextToken".to_string()],
        "case {case:?}: both refused members must be named, got {:?}",
        answer.body
    );
    assert!(
        message(&answer).starts_with("2 validation errors detected: "),
        "case {case:?}: the message must count the refusals it states, got {:?}",
        message(&answer)
    );
}

#[tokio::test]
async fn each_published_bound_is_served_and_every_value_outside_it_is_refused_in_full() {
    let router = emulator();

    // `MaxResultsAttribute` is `@range(min: 1, max: 300)`: both published bounds are
    // admissible, so a refusal is about the bound and not about the member. The refused
    // values are each bound crossed by one and the extremes of the modelled integer, so
    // the rule is asserted at the edges and far outside them.
    for served in ["1", "300"] {
        let answer = get(&router, &format!("/v1/report/list?maxResults={served}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={served}: a published bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
    }
    for refused in ["0", "-1", "301", "100000", "2147483647", "-2147483648"] {
        let answer = get(&router, &format!("/v1/report/list?maxResults={refused}")).await;
        assert_whole_refusal(&format!("maxResults={refused}"), &answer, &["maxResults"]);
    }

    // `VersionAttribute` is `@range(min: 1)` with no ceiling, so a version far above the
    // published set is NOT a validation refusal: the rule follows the modelled bound.
    let report = a_published_report_id(&router).await;
    for refused in ["0", "-1", "-9007199254740991"] {
        let answer = get(
            &router,
            &format!("/v1/report/getMetadata?reportId={report}&reportVersion={refused}"),
        )
        .await;
        assert_whole_refusal(
            &format!("reportVersion={refused}"),
            &answer,
            &["reportVersion"],
        );
    }
    let above = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report}&reportVersion=9007199254740991"),
    )
    .await;
    assert_eq!(
        (above.status, above.error_code()),
        (404, "ResourceNotFoundException"),
        "case reportVersion=9007199254740991: a version the model admits is not a \
         validation refusal — the report publishes no such version, got {} with {:?}",
        above.status,
        above.body
    );
}
