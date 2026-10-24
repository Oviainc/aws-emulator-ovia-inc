// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 traffic policy slice:
//! `CreateTrafficPolicy` and the `GetTrafficPolicy` read that has to report
//! what the create persisted.
//!
//! The rules under test:
//!
//! 1. `CreateTrafficPolicy` persists a traffic policy and answers with it: a
//!    freshly minted `Id`, `Version` 1, the `Name`, `Document` and `Comment`
//!    the request carried, and a `Type` READ OFF the document (its
//!    `RecordType`) — the request carries no type at all. `GetTrafficPolicy`
//!    on `(Id, Version)` reports exactly the same policy afterwards, so the
//!    table below spans the record type, the document's entry-point form
//!    (`StartEndpoint` vs `StartRule`), the number of endpoints, whether a
//!    comment was given and how long the name and comment are.
//! 2. The document is the policy's definition, so a document the published
//!    traffic policy document format does not define is refused with the
//!    modelled `InvalidTrafficPolicyDocument` and persists nothing. The second
//!    table spans one malformation per structural rule of that format.
//! 3. A `Name` the account already holds is refused with the modelled
//!    `TrafficPolicyAlreadyExists`, and the policy already holding the name is
//!    left as it was.
//! 4. An `(Id, Version)` pair the store does not hold is the modelled
//!    `NoSuchTrafficPolicy`, whichever half is missing; a `Version` outside the
//!    modelled range is refused before the handler.
//! 5. A read changes nothing: reading a policy twice answers the same policy,
//!    and the account's traffic policy count follows the policies it holds —
//!    not the reads of them.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES, TRAFFIC_POLICY_FORMAT_VERSION,
};

use crate::hosted_zone_records::{call, escape_xml, router, Reply, ENDPOINT};

/// `TrafficPolicyName` is `len 0..512`
/// (smithy_query(query="shape",shape="TrafficPolicyName")).
const NAME_MAX_LEN: usize = 512;

/// `TrafficPolicyComment` is `len 0..1024`
/// (smithy_query(query="shape",shape="TrafficPolicyComment")).
const COMMENT_MAX_LEN: usize = 1024;

/// `TrafficPolicyVersion` is `range 1..1000`
/// (smithy_query(query="shape",shape="TrafficPolicyVersion")).
const VERSION_MIN: i64 = 1;
/// The other published bound of `TrafficPolicyVersion`.
const VERSION_MAX: i64 = 1000;

/// A traffic policy document whose entry point is a single endpoint.
fn document_with_start_endpoint(record_type: &str, value: &str) -> String {
    format!(
        "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
         \"RecordType\":\"{record_type}\",\"StartEndpoint\":\"only\",\
         \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"{value}\"}}}}}}"
    )
}

/// A traffic policy document whose entry point is a rule over two endpoints.
fn document_with_start_rule(record_type: &str, primary: &str, secondary: &str) -> String {
    format!(
        "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
         \"RecordType\":\"{record_type}\",\"StartRule\":\"failover_to_secondary\",\
         \"Endpoints\":{{\"primary\":{{\"Type\":\"value\",\"Value\":\"{primary}\"}},\
         \"secondary\":{{\"Type\":\"value\",\"Value\":\"{secondary}\"}}}},\
         \"Rules\":{{\"failover_to_secondary\":{{\"RuleType\":\"failover\",\
         \"Primary\":{{\"EndpointReference\":\"primary\"}},\
         \"Secondary\":{{\"EndpointReference\":\"secondary\"}}}}}}}}"
    )
}

/// Create a traffic policy. `comment` of `None` sends no `Comment` element.
async fn create_policy(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    comment: Option<&str>,
) -> Reply {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    call(
        router,
        "POST",
        "/2013-04-01/trafficpolicy",
        &format!(
            "<CreateTrafficPolicyRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Name>{}</Name><Document>{}</Document>{comment}\
             </CreateTrafficPolicyRequest>",
            escape_xml(name),
            escape_xml(document)
        ),
    )
    .await
}

/// Read one traffic policy version back.
async fn get_policy(router: &MultiServiceRouter, id: &str, version: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicy/{id}/{version}"),
        "",
    )
    .await
}

/// How many traffic policies the account holds, as `GetAccountLimit` reports
/// it — the count that has to agree with the policies in the store.
async fn policy_count(router: &MultiServiceRouter) -> String {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("Count")
        .unwrap_or_else(|| panic!("the reply must carry Count; body {}", reply.body))
        .to_string()
}

/// `(Id, Version, Name, Type, Document, Comment)` as a reply carrying a
/// `TrafficPolicy` reports them.
fn policy_of(reply: &Reply, case: &str) -> PolicyView {
    let pick = |path: &str| {
        reply.first(path).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry TrafficPolicy/{path}; body {}",
                reply.body
            )
        })
    };
    PolicyView {
        id: pick("TrafficPolicy/Id").to_string(),
        version: pick("TrafficPolicy/Version").to_string(),
        name: pick("TrafficPolicy/Name").to_string(),
        record_type: pick("TrafficPolicy/Type").to_string(),
        document: pick("TrafficPolicy/Document").to_string(),
        comment: reply.first("TrafficPolicy/Comment").map(str::to_string),
    }
}

/// One `TrafficPolicy` as the wire reports it.
#[derive(Debug, PartialEq, Eq)]
struct PolicyView {
    id: String,
    version: String,
    name: String,
    record_type: String,
    document: String,
    comment: Option<String>,
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form Route 53 mints a traffic
/// policy id in, which is also the 36 characters `TrafficPolicyId` allows.
fn is_traffic_policy_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('-').collect();
    segments.len() == 5
        && segments
            .iter()
            .map(|segment| segment.len())
            .eq([8, 4, 4, 4, 12])
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// One row of the create table: the request sent, and what every read of the
/// resulting policy must report.
struct CreateRow {
    case: &'static str,
    name: String,
    document: String,
    comment: Option<String>,
    /// The `Type` the document's `RecordType` names.
    expect_type: &'static str,
}

#[tokio::test]
async fn a_created_traffic_policy_reads_back_as_it_was_created() {
    let long_name = "n".repeat(NAME_MAX_LEN);
    let long_comment = "c".repeat(COMMENT_MAX_LEN);
    let rows = [
        CreateRow {
            case: "an A policy with one endpoint and a comment",
            name: "single-endpoint".to_string(),
            document: document_with_start_endpoint("A", "192.0.2.1"),
            comment: Some("the simplest policy there is".to_string()),
            expect_type: "A",
        },
        CreateRow {
            case: "an AAAA policy with no comment at all",
            name: "ipv6-endpoint".to_string(),
            document: document_with_start_endpoint("AAAA", "2001:db8::1"),
            comment: None,
            expect_type: "AAAA",
        },
        CreateRow {
            case: "a CNAME policy whose entry point is a rule over two endpoints",
            name: "failover".to_string(),
            document: document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
            comment: Some("primary with a backup".to_string()),
            expect_type: "CNAME",
        },
        CreateRow {
            case: "a TXT policy whose document carries XML special characters",
            name: "quoted & escaped".to_string(),
            document: document_with_start_endpoint("TXT", "v=spf1 <include:example.com> ~all"),
            comment: Some("a&b<c>d\"e'f".to_string()),
            expect_type: "TXT",
        },
        CreateRow {
            case: "an MX policy with a name at the 512-character maximum",
            name: long_name.clone(),
            document: document_with_start_endpoint("MX", "10 mail.example.com"),
            comment: None,
            expect_type: "MX",
        },
        CreateRow {
            case: "an SRV policy with a comment at the 1024-character maximum",
            name: "long-comment".to_string(),
            document: document_with_start_rule("SRV", "1 1 80 a.example.com", "1 1 80 b.example.com"),
            comment: Some(long_comment.clone()),
            expect_type: "SRV",
        },
    ];

    // Every record type the table spans is one the model's own `RRType` enum
    // names, so no row asserts a type a traffic policy could not carry.
    for row in &rows {
        assert!(
            R_R_TYPE_VALUES.contains(&row.expect_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.expect_type
        );
    }

    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        let created = create_policy(&router, &row.name, &row.document, row.comment.as_deref()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateTrafficPolicy, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = policy_of(&created, row.case);
        let expected = PolicyView {
            id: answered.id.clone(),
            version: "1".to_string(),
            name: row.name.clone(),
            record_type: row.expect_type.to_string(),
            document: row.document.clone(),
            comment: row.comment.clone(),
        };
        assert_eq!(
            answered, expected,
            "case {:?}: CreateTrafficPolicy must answer with the policy it persisted, \
             got {answered:?}; body {}",
            row.case, created.body
        );
        assert!(
            is_traffic_policy_id(&answered.id),
            "case {:?}: expected an 8-4-4-4-12 lowercase-hex traffic policy id, got {:?}",
            row.case,
            answered.id
        );
        assert!(
            !minted.contains(&answered.id),
            "case {:?}: every created policy must get its own id, but {:?} was minted twice",
            row.case,
            answered.id
        );
        // The Location names the resource the read is bound to, at the endpoint
        // the request was sent to.
        let expected_location =
            format!("https://{ENDPOINT}/2013-04-01/trafficpolicy/{}/1", answered.id);
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: expected the Location {expected_location:?}, got {:?}",
            row.case,
            created.location
        );
        assert_eq!(
            created.first("TrafficPolicy/Id").map(str::to_string),
            Some(answered.id.clone()),
            "case {:?}: the answered id must be the one the Location names; body {}",
            row.case,
            created.body
        );

        // Round trip: the read reports the very same policy.
        let read = get_policy(&router, &answered.id, "1").await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetTrafficPolicy, got {} with body {}",
            row.case, read.status, read.body
        );
        let got = policy_of(&read, row.case);
        assert_eq!(
            got, answered,
            "case {:?}: GetTrafficPolicy must report the policy the create persisted, \
             got {got:?} after {answered:?}; body {}",
            row.case, read.body
        );

        minted.push(answered.id);
    }

    // The account holds exactly the policies that were created.
    let count = policy_count(&router).await;
    assert_eq!(
        count,
        minted.len().to_string(),
        "expected the account to hold {} traffic policies, got Count {count:?}",
        minted.len()
    );
}

#[tokio::test]
async fn an_input_outside_a_modelled_length_is_refused_and_persists_nothing() {
    // The published bounds of `TrafficPolicyName` (0..512) and
    // `TrafficPolicyComment` (0..1024): at the bound the create is served, one
    // character past it the request is refused and nothing is stored.
    let router = router();
    let document = document_with_start_endpoint("A", "192.0.2.10");
    let rows = [
        (
            "an empty name, which the 0 minimum allows",
            String::new(),
            None,
            201,
        ),
        (
            "a name at the maximum",
            "a".repeat(NAME_MAX_LEN),
            None,
            201,
        ),
        (
            "a name one character past the maximum",
            "b".repeat(NAME_MAX_LEN + 1),
            None,
            400,
        ),
        (
            "a comment at the maximum",
            "at-comment-bound".to_string(),
            Some("c".repeat(COMMENT_MAX_LEN)),
            201,
        ),
        (
            "a comment one character past the maximum",
            "past-comment-bound".to_string(),
            Some("c".repeat(COMMENT_MAX_LEN + 1)),
            400,
        ),
    ];

    let mut served = 0usize;
    for (case, name, comment, status) in &rows {
        let reply = create_policy(&router, name, &document, comment.as_deref()).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status} from CreateTrafficPolicy, got {} with body {}",
            reply.status, reply.body
        );
        if *status == 201 {
            served += 1;
        }
        let count = policy_count(&router).await;
        assert_eq!(
            count,
            served.to_string(),
            "case {case:?}: expected the account to hold {served} traffic policies, \
             got Count {count:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The document is the policy's definition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_document_the_format_does_not_define_is_refused() {
    // One row per structural rule of the published document format: it is a
    // JSON object, it declares the format version, it names the record type it
    // creates, it defines the endpoints it can route to, and it has exactly one
    // entry point — a `StartEndpoint` naming one of those endpoints or a
    // `StartRule` naming one of the `Rules`.
    let rows = [
        ("not JSON at all", "this is not a policy".to_string()),
        (
            "a JSON array rather than an object",
            "[\"not\",\"an\",\"object\"]".to_string(),
        ),
        (
            "no AWSPolicyFormatVersion",
            "{\"RecordType\":\"A\",\"StartEndpoint\":\"only\",\
             \"Endpoints\":{\"only\":{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}"
                .to_string(),
        ),
        (
            "an AWSPolicyFormatVersion no published format carries",
            document_with_start_endpoint("A", "192.0.2.1")
                .replace(TRAFFIC_POLICY_FORMAT_VERSION, "1999-01-01"),
        ),
        (
            "no RecordType",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"StartEndpoint\":\"only\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
            ),
        ),
        (
            "a RecordType outside the modelled RRType enum",
            document_with_start_endpoint("NOTATYPE", "192.0.2.1"),
        ),
        (
            "a RecordType that is not even a string",
            document_with_start_endpoint("A", "192.0.2.1").replace("\"A\"", "7"),
        ),
        (
            "no Endpoints",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartEndpoint\":\"only\"}}"
            ),
        ),
        (
            "an empty Endpoints block",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartEndpoint\":\"only\",\"Endpoints\":{{}}}}"
            ),
        ),
        (
            "an endpoint that is not a JSON object",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartEndpoint\":\"only\",\
                 \"Endpoints\":{{\"only\":\"192.0.2.1\"}}}}"
            ),
        ),
        (
            "a StartEndpoint naming no endpoint in Endpoints",
            document_with_start_endpoint("A", "192.0.2.1").replace("\"only\",", "\"missing\","),
        ),
        (
            "neither a StartEndpoint nor a StartRule",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
            ),
        ),
        (
            "both a StartEndpoint and a StartRule",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartEndpoint\":\"only\",\"StartRule\":\"only\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}},\
                 \"Rules\":{{\"only\":{{\"RuleType\":\"failover\"}}}}}}"
            ),
        ),
        (
            "a StartRule with no Rules block",
            format!(
                "{{\"AWSPolicyFormatVersion\":\"{TRAFFIC_POLICY_FORMAT_VERSION}\",\
                 \"RecordType\":\"A\",\"StartRule\":\"switch\",\
                 \"Endpoints\":{{\"only\":{{\"Type\":\"value\",\"Value\":\"192.0.2.1\"}}}}}}"
            ),
        ),
        (
            "a StartRule naming no rule in Rules",
            document_with_start_rule("A", "192.0.2.1", "192.0.2.2")
                .replace("\"StartRule\":\"failover_to_secondary\"", "\"StartRule\":\"missing\""),
        ),
    ];

    let router = router();
    for (case, document) in &rows {
        let reply = create_policy(&router, case, document, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a document the format does not define, \
             got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidTrafficPolicyDocument",
            "case {case:?}: expected InvalidTrafficPolicyDocument, got body {}",
            reply.body
        );
        let count = policy_count(&router).await;
        assert_eq!(
            count, "0",
            "case {case:?}: a refused create must persist nothing, got Count {count:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// One name, one policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_the_account_already_holds_is_refused() {
    let router = router();
    let first = create_policy(
        &router,
        "taken",
        &document_with_start_endpoint("A", "192.0.2.1"),
        Some("the policy that holds the name"),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "expected 201 from CreateTrafficPolicy, got {} with body {}",
        first.status, first.body
    );
    let held = policy_of(&first, "the policy that holds the name");

    // The refusal is about the NAME, so it stands whatever else differs: the
    // document, its record type and the comment all differ here.
    let again = create_policy(
        &router,
        "taken",
        &document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
        None,
    )
    .await;
    assert_eq!(
        again.status, 409,
        "expected HTTP 409 for a name the account already holds, got {} with body {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "TrafficPolicyAlreadyExists",
        "expected TrafficPolicyAlreadyExists, got body {}",
        again.body
    );

    // The policy already holding the name is untouched, and no second policy
    // came into being.
    let read = get_policy(&router, &held.id, "1").await;
    let got = policy_of(&read, "after the refusal");
    assert_eq!(
        got, held,
        "a refused create must leave the policy holding the name exactly as it was, \
         got {got:?} after {held:?}; body {}",
        read.body
    );
    let count = policy_count(&router).await;
    assert_eq!(
        count, "1",
        "a refused create must persist nothing, got Count {count:?}"
    );

    // A different name is a different policy, with an id of its own.
    let other = create_policy(
        &router,
        "not-taken",
        &document_with_start_endpoint("A", "192.0.2.2"),
        None,
    )
    .await;
    assert_eq!(
        other.status, 201,
        "expected 201 for a name the account does not hold, got {} with body {}",
        other.status, other.body
    );
    let other = policy_of(&other, "a second name");
    assert_ne!(
        other.id, held.id,
        "two policies must not share an id, got {:?} twice",
        other.id
    );
}

// ---------------------------------------------------------------------------
// Reading a policy the store does not hold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_or_version_the_store_does_not_hold_is_refused() {
    let router = router();
    let created = create_policy(
        &router,
        "only-one-version",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicy, got {} with body {}",
        created.status, created.body
    );
    let held = policy_of(&created, "the only policy");

    // Both halves of `(Id, Version)` address the policy, so either half naming
    // something the store does not hold is the same refusal; a version outside
    // the modelled range never reaches the handler at all.
    let rows = [
        (
            "an id no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
            VERSION_MIN.to_string(),
            404,
            "NoSuchTrafficPolicy",
        ),
        (
            "a version this policy does not hold",
            held.id.clone(),
            "2".to_string(),
            404,
            "NoSuchTrafficPolicy",
        ),
        (
            "the highest version the model allows, which this policy does not hold",
            held.id.clone(),
            VERSION_MAX.to_string(),
            404,
            "NoSuchTrafficPolicy",
        ),
        (
            "a version below the modelled minimum",
            held.id.clone(),
            (VERSION_MIN - 1).to_string(),
            400,
            "",
        ),
        (
            "a version above the modelled maximum",
            held.id.clone(),
            (VERSION_MAX + 1).to_string(),
            400,
            "",
        ),
    ];

    for (case, id, version, status, code) in &rows {
        let reply = get_policy(&router, id, version).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status} from GetTrafficPolicy, got {} with body {}",
            reply.status, reply.body
        );
        if !code.is_empty() {
            assert_eq!(
                reply.error_code(),
                *code,
                "case {case:?}: expected {code}, got body {}",
                reply.body
            );
        }
    }

    // The version the policy does hold still reads.
    let read = get_policy(&router, &held.id, &VERSION_MIN.to_string()).await;
    let got = policy_of(&read, "the version the policy holds");
    assert_eq!(
        got, held,
        "the stored version must still read back, got {got:?}; body {}",
        read.body
    );
}

#[tokio::test]
async fn reading_a_traffic_policy_leaves_the_account_exactly_as_it_was() {
    // A read reports; it does not mint, move or consume anything. So the policy
    // a repeated read answers with, and the count of policies the account
    // holds, are the same after any number of reads.
    let router = router();
    let created = create_policy(
        &router,
        "read-repeatedly",
        &document_with_start_endpoint("A", "192.0.2.1"),
        Some("read me twice"),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicy, got {} with body {}",
        created.status, created.body
    );
    let held = policy_of(&created, "the created policy");

    for attempt in 1..=3 {
        let read = get_policy(&router, &held.id, "1").await;
        let got = policy_of(&read, "a repeated read");
        assert_eq!(
            got, held,
            "read {attempt}: every read must report the same policy, got {got:?}; body {}",
            read.body
        );
        let count = policy_count(&router).await;
        assert_eq!(
            count, "1",
            "read {attempt}: reading a policy must not move the account's count, \
             got Count {count:?}"
        );
    }
}
