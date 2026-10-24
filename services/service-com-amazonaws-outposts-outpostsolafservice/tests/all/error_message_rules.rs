// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two rules this service words its refusals by, driven
//! over the wire through `MultiServiceRouter::dispatch` exactly as a signed
//! `aws outposts` call arrives.
//!
//! Rule A — `NotFoundException` (HTTP 404): the body is the IDENTIFIER ECHO
//! `<member>=<value as supplied>`, where `<member>` is the validation-path name of
//! the input member the identifier arrived in and the value is what the client
//! sent, verbatim. Not an English sentence, and never normalised: a request that
//! named a resource by ARN reads its ARN back, not the bare id the service
//! resolved it to.
//!
//! Rule B — `ValidationException` (HTTP 400): the body is the standard AWS
//! validation report — the violation count, then one
//! `Value at '<member>' failed to satisfy constraint: <requirement>` clause per
//! violated constraint, joined with `; ` — and it NEVER renders the offending
//! value, whichever operation or member it came from.
//!
//! Each rule is asserted over a SET of inputs: every route in the implemented
//! slice that can state it, both identifier forms the modelled `@httpLabel`
//! patterns accept, and every constraint class the model puts on this service's
//! members (pattern, length minimum, length maximum, numeric range, required).

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{
    segment, Outposts, ABSENT_OUTPOST_ID, ABSENT_SITE_ID, OUTPOST_ARN_PREFIX, SITE_ARN_PREFIX,
};

/// The validation-path names rule A echoes, lowerCamel as a report spells a
/// member name (`SiteId` -> `siteId`).
const SITE_ID_MEMBER: &str = "siteId";
/// See [`SITE_ID_MEMBER`].
const OUTPOST_ID_MEMBER: &str = "outpostId";

/// The `@pattern` the model declares on `SiteId`, from
/// `smithy_query(query="operation",operation="GetSite")`. A report reproduces a
/// pattern exactly as the model spells it, anchors included.
const SITE_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/)?(os-[a-f0-9]{17})$";
/// The `@pattern` the model declares on `OutpostId`. See [`SITE_ID_PATTERN`].
const OUTPOST_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";
/// The `@pattern` the model declares on `ConnectionId` (`GetConnection`).
const CONNECTION_ID_PATTERN: &str = r"^[a-zA-Z0-9+/=]{1,1024}$";
/// The `@pattern` the model declares on `CountryCode` (`CreateQuote`).
const COUNTRY_CODE_PATTERN: &str = r"^[A-Z]{2}$";
/// The `@pattern` the model declares on `OutpostIdentifier` (`CreateQuote`),
/// which unlike `OutpostId` does not parenthesise its bare-id alternative.
const OUTPOST_IDENTIFIER_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";

/// A site ARN in another partition, region and account than the one this
/// emulator mints in — still a value the modelled `SiteId` pattern accepts, so
/// the boundary passes it through to the store.
const FOREIGN_SITE_ARN: &str =
    "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111";
/// The same, for `OutpostId`.
const FOREIGN_OUTPOST_ARN: &str =
    "arn:aws-cn:outposts:cn-north-1:123456789012:outpost/op-2222222222222222a";

/// One rule-A case: a call that names a resource the store does not hold, and
/// the input member plus the exact value it named it with.
struct NotFound {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    supplied: String,
}

/// The `$.Message` rule A requires for this case.
impl NotFound {
    fn expected(&self) -> String {
        format!("{}={}", self.member, self.supplied)
    }
}

/// A `GET`/`DELETE` case whose identifier travels in the URI.
fn by_label(
    case: &'static str,
    method: &'static str,
    collection: &str,
    member: &'static str,
    supplied: &str,
) -> NotFound {
    NotFound {
        case,
        method,
        path: format!("/{collection}/{}", segment(supplied)),
        body: None,
        member,
        supplied: supplied.to_string(),
    }
}

/// Rule A over every route in the implemented slice that can state it, and over
/// both identifier forms the modelled patterns accept — including ARNs this
/// emulator would never mint, which is what shows the echo is the value as
/// supplied rather than anything the service reconstructed.
#[tokio::test]
async fn not_found_bodies_echo_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_site_arn = format!("{SITE_ARN_PREFIX}{ABSENT_SITE_ID}");
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let cases: Vec<NotFound> = vec![
        by_label(
            "GetSite by bare id",
            "GET",
            "sites",
            SITE_ID_MEMBER,
            ABSENT_SITE_ID,
        ),
        by_label(
            "GetSite by site ARN",
            "GET",
            "sites",
            SITE_ID_MEMBER,
            &absent_site_arn,
        ),
        by_label(
            "GetSite by a site ARN from another partition",
            "GET",
            "sites",
            SITE_ID_MEMBER,
            FOREIGN_SITE_ARN,
        ),
        by_label(
            "DeleteSite by bare id",
            "DELETE",
            "sites",
            SITE_ID_MEMBER,
            ABSENT_SITE_ID,
        ),
        by_label(
            "DeleteSite by site ARN",
            "DELETE",
            "sites",
            SITE_ID_MEMBER,
            &absent_site_arn,
        ),
        by_label(
            "GetOutpost by bare id",
            "GET",
            "outposts",
            OUTPOST_ID_MEMBER,
            ABSENT_OUTPOST_ID,
        ),
        by_label(
            "GetOutpost by Outpost ARN",
            "GET",
            "outposts",
            OUTPOST_ID_MEMBER,
            &absent_outpost_arn,
        ),
        by_label(
            "GetOutpost by an Outpost ARN from another partition",
            "GET",
            "outposts",
            OUTPOST_ID_MEMBER,
            FOREIGN_OUTPOST_ARN,
        ),
        by_label(
            "DeleteOutpost by bare id",
            "DELETE",
            "outposts",
            OUTPOST_ID_MEMBER,
            ABSENT_OUTPOST_ID,
        ),
        by_label(
            "DeleteOutpost by Outpost ARN",
            "DELETE",
            "outposts",
            OUTPOST_ID_MEMBER,
            &absent_outpost_arn,
        ),
        // The identifier here arrives in the BODY, and names the site the new
        // Outpost would live at — so the echo names `siteId`, the member the
        // absent identifier came in, not the resource being created.
        NotFound {
            case: "CreateOutpost at a site nothing holds, by bare id",
            method: "POST",
            path: "/outposts".to_string(),
            body: Some(json!({ "Name": "orphan", "SiteId": ABSENT_SITE_ID })),
            member: SITE_ID_MEMBER,
            supplied: ABSENT_SITE_ID.to_string(),
        },
        NotFound {
            case: "CreateOutpost at a site nothing holds, by site ARN",
            method: "POST",
            path: "/outposts".to_string(),
            body: Some(json!({ "Name": "orphan", "SiteId": absent_site_arn })),
            member: SITE_ID_MEMBER,
            supplied: absent_site_arn.clone(),
        },
        // The three PATCH updates state rule A on the same identifier members, and
        // a PATCH carries a body, so these rows also show the echo is of the LABEL
        // and never of anything the payload held.
        NotFound {
            case: "UpdateSite on a site nothing holds, by bare id",
            method: "PATCH",
            path: format!("/sites/{ABSENT_SITE_ID}"),
            body: Some(json!({ "Name": "renamed" })),
            member: SITE_ID_MEMBER,
            supplied: ABSENT_SITE_ID.to_string(),
        },
        NotFound {
            case: "UpdateSite on a site nothing holds, by site ARN",
            method: "PATCH",
            path: format!("/sites/{}", segment(&absent_site_arn)),
            body: Some(json!({ "Description": "re-described" })),
            member: SITE_ID_MEMBER,
            supplied: absent_site_arn.clone(),
        },
        NotFound {
            case: "UpdateSite by a site ARN from another partition",
            method: "PATCH",
            path: format!("/sites/{}", segment(FOREIGN_SITE_ARN)),
            body: Some(json!({ "Notes": "noted" })),
            member: SITE_ID_MEMBER,
            supplied: FOREIGN_SITE_ARN.to_string(),
        },
        NotFound {
            case: "UpdateSiteRackPhysicalProperties on a site nothing holds, by bare id",
            method: "PATCH",
            path: format!("/sites/{ABSENT_SITE_ID}/rackPhysicalProperties"),
            body: Some(json!({ "PowerPhase": "THREE_PHASE" })),
            member: SITE_ID_MEMBER,
            supplied: ABSENT_SITE_ID.to_string(),
        },
        NotFound {
            case: "UpdateSiteRackPhysicalProperties on a site nothing holds, by site ARN",
            method: "PATCH",
            path: format!(
                "/sites/{}/rackPhysicalProperties",
                segment(&absent_site_arn)
            ),
            body: Some(json!({ "UplinkGbps": "UPLINK_10G" })),
            member: SITE_ID_MEMBER,
            supplied: absent_site_arn.clone(),
        },
        NotFound {
            case: "UpdateOutpost on an Outpost nothing holds, by bare id",
            method: "PATCH",
            path: format!("/outposts/{ABSENT_OUTPOST_ID}"),
            body: Some(json!({ "Description": "re-described" })),
            member: OUTPOST_ID_MEMBER,
            supplied: ABSENT_OUTPOST_ID.to_string(),
        },
        NotFound {
            case: "UpdateOutpost on an Outpost nothing holds, by Outpost ARN",
            method: "PATCH",
            path: format!("/outposts/{}", segment(&absent_outpost_arn)),
            body: Some(json!({ "Name": "renamed" })),
            member: OUTPOST_ID_MEMBER,
            supplied: absent_outpost_arn.clone(),
        },
        NotFound {
            case: "UpdateOutpost by an Outpost ARN from another partition",
            method: "PATCH",
            path: format!("/outposts/{}", segment(FOREIGN_OUTPOST_ARN)),
            body: Some(json!({ "SupportedHardwareType": "SERVER" })),
            member: OUTPOST_ID_MEMBER,
            supplied: FOREIGN_OUTPOST_ARN.to_string(),
        },
    ];

    for expected in &cases {
        let reply = outposts
            .call(expected.method, &expected.path, expected.body.as_ref())
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException, got {} {} {}",
            expected.case,
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body, "Message");
        assert_eq!(
            actual,
            expected.expected(),
            "case {:?}: expected $.Message {:?}, got {:?}",
            expected.case,
            expected.expected(),
            actual
        );
    }
}

/// Rule A reports what the STORE holds, not a constant: the same identifier is a
/// 200 while the resource exists and the echo once it is gone, in whichever form
/// the later call names it.
#[tokio::test]
async fn a_deleted_resource_is_echoed_back_in_the_form_the_later_call_named_it() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "decommission-site" }))
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost(&json!({ "Name": "decommission-outpost", "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let outpost_arn = text(&outpost, "OutpostArn");

    // While it exists, the ARN form reads it back.
    let found = outposts
        .call("GET", &format!("/outposts/{}", segment(&outpost_arn)), None)
        .await;
    assert_eq!(
        found.status, 200,
        "case \"GetOutpost by ARN while it exists\": expected HTTP 200, got {} {}",
        found.status, found.body
    );

    let deleted = outposts
        .call(
            "DELETE",
            &format!("/outposts/{}", segment(&outpost_arn)),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"DeleteOutpost by ARN\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );

    // Now gone. Each later call echoes the identifier IT supplied.
    let gone: Vec<(&str, &str, String, String)> = vec![
        (
            "DeleteOutpost again, by the same ARN",
            "DELETE",
            format!("/outposts/{}", segment(&outpost_arn)),
            format!("{OUTPOST_ID_MEMBER}={outpost_arn}"),
        ),
        (
            "DeleteOutpost again, by bare id",
            "DELETE",
            format!("/outposts/{}", segment(&outpost_id)),
            format!("{OUTPOST_ID_MEMBER}={outpost_id}"),
        ),
        (
            "GetOutpost after the delete, by ARN",
            "GET",
            format!("/outposts/{}", segment(&outpost_arn)),
            format!("{OUTPOST_ID_MEMBER}={outpost_arn}"),
        ),
        (
            "GetOutpost after the delete, by bare id",
            "GET",
            format!("/outposts/{}", segment(&outpost_id)),
            format!("{OUTPOST_ID_MEMBER}={outpost_id}"),
        ),
    ];
    for (case, method, path, expected) in &gone {
        let reply = outposts.call(method, path, None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body, "Message");
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected $.Message {expected:?}, got {actual:?}"
        );
    }
}

/// One rule-B case: a request that violates a modelled constraint, and the whole
/// report the rule requires for it.
struct Report {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expected: String,
}

/// The report for a single violated constraint.
fn one(case: &'static str, method: &'static str, path: String, body: Option<Value>, clause: &str) -> Report {
    Report {
        case,
        method,
        path,
        body,
        expected: format!("1 validation error detected: {clause}"),
    }
}

/// One clause of the report: the member and the requirement, and no value.
fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement a member outside its `@pattern` states.
fn pattern(regex: &str) -> String {
    format!("Member must satisfy regular expression pattern: {regex}")
}

/// A valid `CreateQuote` body — `CountryCode` and `RequestedCapacities` are the
/// two members the model marks `@required`, and `QuoteCapacityList` is
/// `@length(min: 0)`, so an empty list satisfies it — with `overrides` merged in.
fn quote(overrides: Value) -> Value {
    let mut request = json!({ "CountryCode": "US", "RequestedCapacities": [] });
    if let (Some(target), Some(extra)) = (request.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    request
}

/// Rule B over every constraint class the model puts on this service's members,
/// across a route whose member is a URI label, one whose member is in the body,
/// and one whose member is a query parameter — and across implemented operations
/// and one that is still a 501 stub (`GetConnection`), because the report is
/// produced at the boundary, before any handler runs.
#[tokio::test]
async fn validation_reports_name_the_member_and_constraint_and_omit_the_value() {
    let outposts = Outposts::new();
    let long_description = "x".repeat(1025);

    let cases: Vec<Report> = vec![
        one(
            "GetSite with a label that is no site identifier",
            "GET",
            "/sites/not-a-site-id".to_string(),
            None,
            &clause(SITE_ID_MEMBER, &pattern(SITE_ID_PATTERN)),
        ),
        one(
            "GetOutpost with a label whose suffix is not hex",
            "GET",
            "/outposts/op-zzzzzzzzzzzzzzzzz".to_string(),
            None,
            &clause(OUTPOST_ID_MEMBER, &pattern(OUTPOST_ID_PATTERN)),
        ),
        one(
            "DeleteSite with a label that is an Outpost id",
            "DELETE",
            format!("/sites/{ABSENT_OUTPOST_ID}"),
            None,
            &clause(SITE_ID_MEMBER, &pattern(SITE_ID_PATTERN)),
        ),
        one(
            "UpdateSite with a label that is no site identifier",
            "PATCH",
            "/sites/not-a-site-id".to_string(),
            Some(json!({ "Name": "renamed" })),
            &clause(SITE_ID_MEMBER, &pattern(SITE_ID_PATTERN)),
        ),
        one(
            "UpdateSiteRackPhysicalProperties with a label whose hex is too short",
            "PATCH",
            "/sites/os-0000000000000000/rackPhysicalProperties".to_string(),
            Some(json!({ "PowerPhase": "THREE_PHASE" })),
            &clause(SITE_ID_MEMBER, &pattern(SITE_ID_PATTERN)),
        ),
        one(
            "UpdateOutpost with a label that is a site id",
            "PATCH",
            format!("/outposts/{ABSENT_SITE_ID}"),
            Some(json!({ "Name": "renamed" })),
            &clause(OUTPOST_ID_MEMBER, &pattern(OUTPOST_ID_PATTERN)),
        ),
        one(
            "GetConnection with a connection id outside its pattern",
            "GET",
            "/connections/conn-does-not-exist".to_string(),
            None,
            &clause("connectionId", &pattern(CONNECTION_ID_PATTERN)),
        ),
        one(
            "CreateQuote with a country code of the right length, wrong alphabet",
            "POST",
            "/quotes".to_string(),
            Some(quote(json!({ "CountryCode": "us" }))),
            &clause("countryCode", &pattern(COUNTRY_CODE_PATTERN)),
        ),
        one(
            "CreateQuote with an Outpost identifier outside its pattern",
            "POST",
            "/quotes".to_string(),
            Some(quote(json!({ "OutpostIdentifier": "op-not-an-outpost" }))),
            &clause("outpostIdentifier", &pattern(OUTPOST_IDENTIFIER_PATTERN)),
        ),
        // `QuoteDescription` is @sensitive: the report names the bound it broke
        // and nothing of what the caller sent.
        one(
            "CreateQuote with a description past its length maximum",
            "POST",
            "/quotes".to_string(),
            Some(quote(json!({ "Description": long_description }))),
            &clause(
                "description",
                "Member must have length less than or equal to 1024",
            ),
        ),
        one(
            "ListOutposts with a page size below its range minimum",
            "GET",
            "/outposts?MaxResults=0".to_string(),
            None,
            &clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        one(
            "ListSites with a page size above its range maximum",
            "GET",
            "/sites?MaxResults=1001".to_string(),
            None,
            &clause(
                "maxResults",
                "Member must have value less than or equal to 1000",
            ),
        ),
        // A member that was ABSENT has no value to omit, and the report says so
        // in the framework's own words rather than naming a value that never
        // arrived.
        Report {
            case: "CreateQuote with the required country code absent",
            method: "POST",
            path: "/quotes".to_string(),
            body: Some(json!({ "RequestedCapacities": [] })),
            expected: "1 validation error detected: Value null at 'countryCode' \
                       failed to satisfy constraint: Member must not be null"
                .to_string(),
        },
        // Two constraints on ONE member: the count is the number of clauses, and
        // the value is omitted from each of them.
        Report {
            case: "CreateQuote with a country code too long AND outside its pattern",
            method: "POST",
            path: "/quotes".to_string(),
            body: Some(quote(json!({ "CountryCode": "USA" }))),
            expected: format!(
                "2 validation errors detected: {}; {}",
                clause(
                    "countryCode",
                    "Member must have length less than or equal to 2"
                ),
                clause("countryCode", &pattern(COUNTRY_CODE_PATTERN)),
            ),
        },
        Report {
            case: "CreateQuote with a country code too short AND outside its pattern",
            method: "POST",
            path: "/quotes".to_string(),
            body: Some(quote(json!({ "CountryCode": "U" }))),
            expected: format!(
                "2 validation errors detected: {}; {}",
                clause(
                    "countryCode",
                    "Member must have length greater than or equal to 2"
                ),
                clause("countryCode", &pattern(COUNTRY_CODE_PATTERN)),
            ),
        },
    ];

    for expected in &cases {
        let reply = outposts
            .call(expected.method, &expected.path, expected.body.as_ref())
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {:?}: expected 400 ValidationException, got {} {} {}",
            expected.case,
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body, "message");
        assert_eq!(
            actual, expected.expected,
            "case {:?}: expected $.message {:?}, got {:?}",
            expected.case, expected.expected, actual
        );
    }
}

/// The invariant half of rule B, stated over values a caller would not want
/// echoed: whatever the request held, the report holds none of it, and its
/// declared count always equals the number of clauses it carries.
#[tokio::test]
async fn no_validation_report_renders_any_value_the_request_supplied() {
    let outposts = Outposts::new();
    let secret = "s3cret-value-the-caller-sent";

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetSite with a secret in the label",
            "GET",
            format!("/sites/{}", segment(secret)),
            None,
        ),
        (
            "GetConnection with a secret in the label",
            "GET",
            format!("/connections/{}", segment(secret)),
            None,
        ),
        (
            "CreateQuote with a secret in the @sensitive description",
            "POST",
            "/quotes".to_string(),
            Some(quote(
                json!({ "Description": format!("{secret}{}", "x".repeat(1024)) }),
            )),
        ),
        (
            "CreateQuote with a secret in the Outpost identifier",
            "POST",
            "/quotes".to_string(),
            Some(quote(json!({ "OutpostIdentifier": secret }))),
        ),
        (
            "CreateQuote with secrets in two members at once",
            "POST",
            "/quotes".to_string(),
            Some(quote(
                json!({ "CountryCode": "SECRETS", "OutpostIdentifier": secret }),
            )),
        ),
        (
            "ListSites with a page size far above its maximum",
            "GET",
            "/sites?MaxResults=987654".to_string(),
            None,
        ),
    ];

    for (case, method, path, body) in &cases {
        let reply = outposts.call(method, path, body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body, "message");
        assert!(
            !actual.contains(secret) && !actual.contains("SECRETS") && !actual.contains("987654"),
            "case {case:?}: the report rendered a value the request supplied: {actual:?}"
        );
        let (declared, clauses) = counted(&actual);
        assert_eq!(
            declared,
            Some(clauses),
            "case {case:?}: the report declares {declared:?} violations but carries {clauses}: {actual:?}"
        );
        for clause in body_of(&actual).split("; ") {
            assert!(
                clause.starts_with("Value at '") || clause.starts_with("Value null at '"),
                "case {case:?}: clause {clause:?} is not a report clause: {actual:?}"
            );
        }
    }
}

/// The violation count a report DECLARES, and how many clauses it actually
/// carries — the two the rule keeps equal.
fn counted(report: &str) -> (Option<usize>, usize) {
    let declared = report
        .split_whitespace()
        .next()
        .and_then(|count| count.parse().ok());
    let clauses = body_of(report).split("; ").count();
    (declared, clauses)
}

/// Everything after a report's `<N> validation error(s) detected: ` preamble.
fn body_of(report: &str) -> &str {
    report.split_once(": ").map_or(report, |(_, body)| body)
}

/// The error message a body carries under `key` (`Message` for a modelled error
/// shape, `message` for the framework's synthetic envelope), read as the empty
/// string when the body carries none.
fn message(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A string member of a response, read as the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
