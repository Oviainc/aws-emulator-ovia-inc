// SPDX-License-Identifier: CC0-1.0

//! RULE — a `nextToken` this service issues is an OPAQUE continuation value: the only
//! thing a client may do with one is hand it back.
//!
//! Three things follow, and this suite asserts all three over every paginated listing the
//! service serves out of seeded state:
//!
//! 1. The token carries NO value the same response published — not as itself, not inside
//!    itself, and not in the bytes it decodes to. Encoding a record's id is not making a
//!    token opaque; it only hides the id from a grep, and a client that decodes the token
//!    still reads the collection's keying out of it. So the cursor carries the POSITION in
//!    the operation's own reported order, and a SCOPED cursor carries the digest of the
//!    resource it was cut for rather than that resource's id.
//! 2. The token is accepted back only by the operation — and, for a scoped listing, the
//!    resource — that cut it. Anything else is refused with the modelled
//!    `ValidationException`, including a cursor of this service's own whose characters a
//!    client changed or truncated: the payload is sealed with a digest over it, so a value
//!    a client assembled decodes to no cursor at all rather than to a resume point the
//!    service never cut.
//! 3. The token resumes EXACTLY after the page it was cut from, so walking a listing a
//!    page at a time reports every record once, in the order one unpaged call reports them.
//!
//! The rows span what the rule generalises over: a whole service-owned collection
//! (`ListReports`), the sub-collection of ONE resource (`ListReportVersions`, the scoped
//! case) and the calling account's own set (`ListCustomerAgreements`), each walked at page
//! sizes that cut it in different places, including both published bounds of
//! `MaxResultsAttribute`'s `@range(min: 1, max: 300)`.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, get, Answer, CATALOG};

/// One paginated listing this service serves out of seeded, service-owned state.
struct Listing {
    /// The operation, as a case label.
    case: &'static str,
    /// Its modelled `@http` URI.
    path: &'static str,
    /// The query this operation needs before `maxResults` and `nextToken`: the resource a
    /// SCOPED listing pages the sub-collection of, or nothing for a whole collection.
    scope: &'static str,
    /// The output member whose list the page cuts.
    items: &'static str,
}

/// The report whose versions the scoped row pages. Checked against [`CATALOG`] by the
/// tests below, so it cannot silently name a report the catalog does not publish several
/// versions of.
const SCOPED_REPORT: &str = "report-5eevElBv6MyRmQsD";

/// The paginated listings, with the `@http` URIs and output members from
/// `smithy_query(query="operation", operation="ListReports,ListReportVersions,ListCustomerAgreements")`.
const LISTINGS: &[Listing] = &[
    Listing {
        case: "ListReports",
        path: "/v1/report/list",
        scope: "",
        items: "reports",
    },
    Listing {
        case: "ListReportVersions",
        path: "/v1/report/listVersions",
        scope: "reportId=report-5eevElBv6MyRmQsD",
        items: "reports",
    },
    Listing {
        case: "ListCustomerAgreements",
        path: "/v1/customer-agreement/list",
        scope: "",
        items: "customerAgreements",
    },
];

/// The page sizes every listing is walked at: the floor and the ceiling of
/// `MaxResultsAttribute`'s `@range(min: 1, max: 300)`, and sizes between them that cut the
/// collections at different places.
const PAGE_SIZES: &[usize] = &[1, 2, 3, 5, 7, 300];

/// The shortest published value a match inside a token is evidence rather than
/// coincidence: below it a value is a one- or two-character number or a status word any
/// digest can carry by chance.
const MEANINGFUL: usize = 8;

/// One page of `listing`, with `query` appended after whatever scope the listing needs.
async fn page(router: &MultiServiceRouter, listing: &Listing, query: &str) -> Answer {
    let mut uri = listing.path.to_string();
    let mut separator = '?';
    for part in [listing.scope, query] {
        if part.is_empty() {
            continue;
        }
        uri.push(separator);
        uri.push_str(part);
        separator = '&';
    }
    get(router, &uri).await
}

/// The ARN of every record on the page — the one member every summary this service pages
/// carries that names exactly one record, so two pages can be compared without caring
/// which collection they came from.
fn arns(answer: &Answer, items: &str) -> Vec<String> {
    answer.body[items]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["arn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every string this answer published, at any depth.
fn published_strings(body: &Value, into: &mut Vec<String>) {
    match body {
        Value::String(text) => into.push(text.clone()),
        Value::Array(rows) => {
            for row in rows {
                published_strings(row, into);
            }
        }
        Value::Object(members) => {
            for value in members.values() {
                published_strings(value, into);
            }
        }
        _ => {}
    }
}

/// The forms a client can trivially read a token in: the token as it arrived and, when it
/// is hex, the bytes it decodes to.
fn readable_forms(token: &str) -> Vec<String> {
    let mut forms = vec![token.to_string()];
    if let Ok(bytes) = ::hex::decode(token) {
        forms.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    forms
}

/// Assert `token` is opaque: inside `NextTokenAttribute`'s length, carrying only
/// characters an `@httpQuery` value needs no escaping for, and carrying no value `answer`
/// published in any form a client can read it in.
fn assert_opaque(case: &str, token: &str, answer: &Answer) {
    assert!(
        !token.is_empty() && token.len() <= 2048,
        "case {case:?}: NextTokenAttribute is @length(1..2048), got {} bytes",
        token.len()
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte)),
        "case {case:?}: a token travels in an @httpQuery value, so it must carry only \
         characters unreserved in a URI, got {token:?}"
    );
    let mut published = Vec::new();
    published_strings(&answer.body, &mut published);
    for form in readable_forms(token) {
        for value in &published {
            if value.len() < MEANINGFUL || value == token {
                continue;
            }
            assert!(
                !form.contains(value.as_str()),
                "case {case:?}: a pagination token must carry no value the same response \
                 published, got {token:?}, which reads as {form:?} and carries {value:?}"
            );
        }
    }
}

/// `token` with one character changed, still a well-formed hex string of the same length,
/// so what refuses it is the cursor's seal rather than a decoding failure.
fn with_one_character_changed(token: &str) -> String {
    let mut bytes = token.as_bytes().to_vec();
    let first = bytes.first_mut().expect("an issued token is not empty");
    *first = if *first == b'a' { b'b' } else { b'a' };
    String::from_utf8(bytes).expect("a hex token is ASCII")
}

#[tokio::test]
async fn the_scoped_row_names_a_report_the_catalog_publishes_several_versions_of() {
    let row = CATALOG
        .iter()
        .find(|row| row.id == SCOPED_REPORT)
        .unwrap_or_else(|| panic!("case {SCOPED_REPORT:?}: the catalog must publish it"));
    assert!(
        row.versions.len() > 1,
        "case {SCOPED_REPORT:?}: the scoped row must page, so the report needs several \
         versions, got {:?}",
        row.versions
    );
}

#[tokio::test]
async fn a_next_token_is_opaque_and_resumes_exactly_after_its_page() {
    let router = emulator();

    for listing in LISTINGS {
        // What one unpaged call reports is the order and the content every walk below has
        // to reproduce — the rule, rather than one recorded page.
        let whole = page(&router, listing, "").await;
        assert_eq!(
            whole.status, 200,
            "case {:?}: an unpaged listing must be served, got {} with {:?}",
            listing.case, whole.status, whole.body
        );
        let expected = arns(&whole, listing.items);
        assert!(
            expected.len() > 1,
            "case {:?}: the rule needs several records to page over, got {expected:?}",
            listing.case
        );
        assert!(
            whole.next_token().is_none(),
            "case {:?}: a page holding every record promises no further one, so no \
             nextToken is owed, got {:?}",
            listing.case,
            whole.next_token()
        );

        for page_size in PAGE_SIZES {
            let case = format!("{} maxResults={page_size}", listing.case);
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut calls = 0usize;
            loop {
                calls += 1;
                assert!(
                    calls <= expected.len() + 1,
                    "case {case:?}: paging must terminate, got {calls} calls for {} records",
                    expected.len()
                );
                let query = match token.as_deref() {
                    None => format!("maxResults={page_size}"),
                    Some(value) => format!("maxResults={page_size}&nextToken={value}"),
                };
                let answer = page(&router, listing, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a cursor this operation cut must be served, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                let rows = arns(&answer, listing.items);
                assert!(
                    rows.len() <= *page_size,
                    "case {case:?}: a page is cut at maxResults, got {} records",
                    rows.len()
                );
                walked.extend(rows.iter().cloned());
                match answer.next_token() {
                    None => break,
                    Some(issued) => {
                        assert_opaque(&case, &issued, &answer);
                        assert_eq!(
                            rows.len(),
                            *page_size,
                            "case {case:?}: a nextToken promises a further record, so it is \
                             only cut on a full page, got {} records",
                            rows.len()
                        );
                        token = Some(issued);
                    }
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: walking the pages must report every record once, in the \
                 order one unpaged call reports them, got {walked:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_this_operation_did_not_cut_is_refused_as_a_next_token() {
    let router = emulator();

    // The cursor each listing really cuts, so the rows below can hand every one of them to
    // the operations that did not cut it.
    let mut issued: Vec<String> = Vec::new();
    for listing in LISTINGS {
        let token = page(&router, listing, "maxResults=1")
            .await
            .next_token()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a collection of several records must page",
                    listing.case
                )
            });
        issued.push(token);
    }

    for (index, listing) in LISTINGS.iter().enumerate() {
        let own = issued[index].as_str();
        let mut rows: Vec<(String, String)> = vec![
            ("a value that is not a cursor".to_string(), "not-a-token".to_string()),
            ("a bare position".to_string(), "0".to_string()),
            ("a bare record id".to_string(), CATALOG[0].id.to_string()),
            (
                "a bare record ARN".to_string(),
                format!("arn:aws:artifact:us-east-1::report/{}:1", CATALOG[0].id),
            ),
            ("hex that is not a cursor".to_string(), ::hex::encode("artifact")),
            (
                "its own cursor with one character changed".to_string(),
                with_one_character_changed(own),
            ),
            (
                "its own cursor truncated".to_string(),
                own[..own.len() - 2].to_string(),
            ),
        ];
        for (other, token) in issued.iter().enumerate() {
            if other != index {
                rows.push((
                    format!("the cursor {} cut", LISTINGS[other].case),
                    token.clone(),
                ));
            }
        }

        for (what, token) in rows {
            let case = format!("{} given {what}", listing.case);
            let refused = page(&router, listing, &format!("nextToken={token}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a value this operation did not cut must be refused, got {} \
                 with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                refused.body
            );
        }
    }

    // A SCOPED cursor binds to the ONE resource it was cut for: every other report in the
    // catalog refuses it, rather than resuming its own versions at that position.
    let scoped = LISTINGS
        .iter()
        .find(|listing| !listing.scope.is_empty())
        .expect("one listing pages the sub-collection of a resource");
    let scoped_token = page(&router, scoped, "maxResults=1")
        .await
        .next_token()
        .expect("a report with several versions must page");
    for row in CATALOG {
        if row.id == SCOPED_REPORT {
            continue;
        }
        let case = format!("ListReportVersions {} given another report's cursor", row.id);
        let refused = get(
            &router,
            &format!(
                "/v1/report/listVersions?reportId={}&nextToken={scoped_token}",
                row.id
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a cursor cut for another report must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }
}
