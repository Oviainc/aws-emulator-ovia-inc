// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the dedicated origination-number listing:
//! `ListOriginationNumbers` reports the origination numbers this account holds.
//!
//! The rule under test: the listing reports ONE entry per origination number this
//! account's persisted inventory holds, every member of an entry read from that
//! number's record (the number, its status, its country, its route and its
//! capabilities, in the order the record carries them), ordered oldest first, in pages
//! of at most the requested size — the published maximum when the request asks for no
//! size — with a `NextToken` returned only while numbers remain; a token the listing
//! did not mint is bad input, and reading changes nothing a later caller sees.
//!
//! The rows below differ along what that rule generalises over: the phone-number FORM
//! (short code, national and international E.164), the ROUTE of the number (every value
//! the modelled enum accepts), the CAPABILITIES it carries (one, two, all three, and
//! their order), its COUNTRY, the STATUS its record holds, the CREATION INSTANT that
//! orders the listing (ties included), the number COUNT across the page bound, the page
//! SIZE (both published bounds and past each) and the token walked from. Every call
//! goes over the wire through the router the emulator serves in production, so nothing
//! but persisted state carries information between calls.
//!
//! A dedicated origination number is acquired OUT OF BAND — it is requested and
//! registered through a separate AWS request, which no SNS API performs — so the suite
//! seeds those records the way that provisioning writes them, the same seam the opt-out
//! and sandbox suites use for a record no SNS API mints.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::ops::types::{
    NUMBER_CAPABILITY_VALUES, ROUTE_TYPE_VALUES,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    rfc3339, OriginationNumberRecord, INVALID_NEXT_TOKEN_REFUSAL, ORIGINATION_NUMBERS_PAGE_SIZE,
};

use super::topics_subscriptions_publish::{text_of, Answer, Sns};

/// The page `MaxResults` bounds: `smithy_query(query="shape",
/// shape="MaxItemsListOriginationNumbers")` reports `range {min: 1, max: 30}`.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 30;

/// One origination number as the listing reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    created_at: String,
    phone_number: String,
    status: String,
    iso2_country_code: String,
    route_type: String,
    number_capabilities: Vec<String>,
}

/// One origination number the suite puts in the account's inventory before any call is
/// made, with the seconds past the suite's base instant it was created at.
#[derive(Debug, Clone)]
struct Seeded {
    case: &'static str,
    phone_number: &'static str,
    status: &'static str,
    iso2_country_code: &'static str,
    route_type: &'static str,
    number_capabilities: &'static [&'static str],
    created_after: u64,
}

/// The inventory the ordering table drives: one row per combination the rule
/// generalises over.
///
/// The routes span every value the modelled `RouteType` enum accepts
/// (`ROUTE_TYPE_VALUES`) and the capability sets every value `NumberCapability` accepts
/// (`NUMBER_CAPABILITY_VALUES`), one of them in the reverse of the enum's order so the
/// listing is pinned to report the record's own order rather than a sorted one. The
/// statuses differ because the model leaves `PhoneNumberInformation$Status` an
/// unconstrained string: whatever the record holds is what the listing must report.
/// Two rows share a creation instant, so their relative order is fixed by the number
/// rather than by chance.
const SEEDED: &[Seeded] = &[
    Seeded {
        case: "a north american long code, sms only",
        phone_number: "+12065550100",
        status: "Active",
        iso2_country_code: "US",
        route_type: "Transactional",
        number_capabilities: &["SMS"],
        created_after: 30,
    },
    Seeded {
        case: "a toll free number carrying every capability",
        phone_number: "+18005550199",
        status: "Active",
        iso2_country_code: "US",
        route_type: "Promotional",
        number_capabilities: &["SMS", "MMS", "VOICE"],
        created_after: 20,
    },
    Seeded {
        case: "a short code, premium route",
        phone_number: "12345",
        status: "Active",
        iso2_country_code: "US",
        route_type: "Premium",
        number_capabilities: &["SMS", "MMS"],
        created_after: 10,
    },
    Seeded {
        case: "an international number still being provisioned",
        phone_number: "+447700900123",
        status: "Pending",
        iso2_country_code: "GB",
        route_type: "Transactional",
        number_capabilities: &["VOICE", "SMS"],
        created_after: 40,
    },
    Seeded {
        case: "created at the same instant as another, sorting first by number",
        phone_number: "+3519100000001",
        status: "Active",
        iso2_country_code: "PT",
        route_type: "Promotional",
        number_capabilities: &["MMS"],
        created_after: 10,
    },
];

/// The body of the response's `<PhoneNumbers>` element, or `None` when the response
/// carries no such member at all — a refused call serves no listing.
fn phone_numbers_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<PhoneNumbers>") {
        let rest = &body[start + "<PhoneNumbers>".len()..];
        let end = rest.find("</PhoneNumbers>")?;
        return Some(&rest[..end]);
    }
    body.contains("<PhoneNumbers/>").then_some("")
}

/// The `<member>…</member>` blocks of `block` at its OUTERMOST level, in document
/// order.
///
/// An entry of this listing carries a list of its own (`NumberCapabilities`), whose
/// members close first, so the walk counts opens against closes instead of stopping at
/// the first `</member>`. A list member's open tag carries the envelope's default
/// namespace, so an opening is matched up to its first `>`.
fn outer_members(block: &str) -> Vec<&str> {
    let close = "</member>";
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut content_from = 0usize;
    let mut at = 0usize;
    while at < block.len() {
        let next_open = block[at..].find("<member").map(|found| at + found);
        let next_close = block[at..].find(close).map(|found| at + found);
        let opens_first = match (next_open, next_close) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(open), Some(close_at)) => open < close_at,
        };
        if opens_first {
            let open = next_open.expect("an opening tag was found");
            let Some(tag_end) = block[open..].find('>').map(|found| open + found + 1) else {
                break;
            };
            if depth == 0 {
                content_from = tag_end;
            }
            depth += 1;
            at = tag_end;
        } else {
            let close_at = next_close.expect("a closing tag was found");
            depth = depth.saturating_sub(1);
            if depth == 0 {
                found.push(&block[content_from..close_at]);
            }
            at = close_at + close.len();
        }
    }
    found
}

/// The `<member>` texts of the `tag` list nested inside one entry, in document order.
fn nested_list(member: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let Some(start) = member.find(&open) else {
        return Vec::new();
    };
    let rest = &member[start + open.len()..];
    let Some(end) = rest.find(&format!("</{tag}>")) else {
        return Vec::new();
    };
    outer_members(&rest[..end])
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Every entry the listing carries, in document order.
fn reported_numbers(answer: &Answer) -> Vec<Reported> {
    let block = phone_numbers_block(&answer.body)
        .unwrap_or_else(|| panic!("no PhoneNumbers member in {}", answer.body));
    outer_members(block)
        .into_iter()
        .map(|member| Reported {
            created_at: text_of(member, "CreatedAt"),
            phone_number: text_of(member, "PhoneNumber"),
            status: text_of(member, "Status"),
            iso2_country_code: text_of(member, "Iso2CountryCode"),
            route_type: text_of(member, "RouteType"),
            number_capabilities: nested_list(member, "NumberCapabilities"),
        })
        .collect()
}

/// One `ListOriginationNumbers` call.
async fn list(sns: &Sns, params: &[(&str, &str)]) -> Answer {
    sns.call("ListOriginationNumbers", params).await
}

/// Put one origination number in the account's inventory, the way the out-of-band
/// provisioning writes it.
fn seed(sns: &Sns, number: &Seeded, created_at: SystemTime) {
    let state = sns.state();
    let mut state = state.lock();
    state.origination_numbers.insert(
        number.phone_number.to_string(),
        OriginationNumberRecord {
            phone_number: number.phone_number.to_string(),
            status: number.status.to_string(),
            iso2_country_code: number.iso2_country_code.to_string(),
            route_type: number.route_type.to_string(),
            number_capabilities: number
                .number_capabilities
                .iter()
                .map(|capability| (*capability).to_string())
                .collect(),
            created_at,
        },
    );
}

/// What the listing must report for one seeded row created at `created_at`.
fn expected(number: &Seeded, created_at: SystemTime) -> Reported {
    Reported {
        created_at: rfc3339(created_at)
            .unwrap_or_else(|| panic!("case {:?}: the seeded instant renders", number.case)),
        phone_number: number.phone_number.to_string(),
        status: number.status.to_string(),
        iso2_country_code: number.iso2_country_code.to_string(),
        route_type: number.route_type.to_string(),
        number_capabilities: number
            .number_capabilities
            .iter()
            .map(|capability| (*capability).to_string())
            .collect(),
    }
}

/// The suite's base instant: a fixed point, so a rendered timestamp carries no
/// fraction and the rows' order is the one the table states.
fn base() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
}

/// The listing reports every number the inventory holds, each entry's members read
/// from that number's record, oldest first — and reading it changes nothing.
#[tokio::test]
async fn the_listing_reports_every_held_number_from_its_own_record_oldest_first() {
    let sns = Sns::new();

    // The table spans the routes and capabilities the model defines, so no value of
    // either enum is left untested by it.
    for route in ROUTE_TYPE_VALUES {
        assert!(
            SEEDED.iter().any(|number| number.route_type == *route),
            "the table must drive the modelled route {route:?}, got {:?}",
            SEEDED
                .iter()
                .map(|number| number.route_type)
                .collect::<Vec<&str>>()
        );
    }
    for capability in NUMBER_CAPABILITY_VALUES {
        assert!(
            SEEDED
                .iter()
                .any(|number| number.number_capabilities.contains(capability)),
            "the table must drive the modelled capability {capability:?}"
        );
    }

    let empty = list(&sns, &[]).await;
    assert_eq!(
        empty.status, 200,
        "an account holding no origination number: expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert!(
        reported_numbers(&empty).is_empty(),
        "an account holding no origination number: expected an empty listing, got {}",
        empty.body
    );
    assert!(
        empty.element("NextToken").is_none(),
        "an account holding no origination number: expected no NextToken, got {}",
        empty.body
    );

    let instant = |number: &Seeded| base() + Duration::from_secs(number.created_after);
    for number in SEEDED {
        seed(&sns, number, instant(number));
    }

    // Oldest first, the number itself breaking a tie.
    let mut order: Vec<&Seeded> = SEEDED.iter().collect();
    order.sort_by(|a, b| {
        a.created_after
            .cmp(&b.created_after)
            .then_with(|| a.phone_number.cmp(b.phone_number))
    });
    let want: Vec<Reported> = order
        .iter()
        .map(|number| expected(number, instant(number)))
        .collect();

    let answer = list(&sns, &[]).await;
    assert_eq!(
        answer.status, 200,
        "the seeded inventory: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let got = reported_numbers(&answer);
    assert_eq!(
        got, want,
        "the seeded inventory: expected every held number once, oldest first, got {got:?}"
    );

    // Each entry's members are the record's, row by row, so a failure names the row.
    for (number, (got, want)) in order.iter().zip(got.iter().zip(want.iter())) {
        assert_eq!(
            got, want,
            "case {:?}: expected the entry its record holds, got {got:?}",
            number.case
        );
    }

    // A read is a report: a second listing answers the same and the store is as it was.
    let again = reported_numbers(&list(&sns, &[]).await);
    assert_eq!(
        again, want,
        "a second listing must report the same numbers, got {again:?}"
    );
    assert_eq!(
        sns.state().lock().origination_numbers.len(),
        SEEDED.len(),
        "the listings must not add or drop a record"
    );
}

/// `MaxResults` is the page size and `NextToken` walks the pages: whatever the size,
/// the pages concatenate to every number held, each once, oldest first, and only the
/// last page carries no token — and a request naming no size is served the published
/// maximum page.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_the_token_walks_every_number_once() {
    // Counts spanning the published maximum page: none, one, one short of a page,
    // exactly a page, one past it, and two pages and a remainder.
    let counts = [
        0usize,
        1,
        ORIGINATION_NUMBERS_PAGE_SIZE - 1,
        ORIGINATION_NUMBERS_PAGE_SIZE,
        ORIGINATION_NUMBERS_PAGE_SIZE + 1,
        ORIGINATION_NUMBERS_PAGE_SIZE * 2 + 1,
    ];
    for count in counts {
        let sns = Sns::new();
        let held: Vec<String> = (0..count).map(|at| format!("+1206555{at:04}")).collect();
        {
            let state = sns.state();
            let mut state = state.lock();
            for (at, number) in held.iter().enumerate() {
                state.origination_numbers.insert(
                    number.clone(),
                    OriginationNumberRecord {
                        phone_number: number.clone(),
                        status: "Active".to_string(),
                        iso2_country_code: "US".to_string(),
                        route_type: ROUTE_TYPE_VALUES[at % ROUTE_TYPE_VALUES.len()].to_string(),
                        number_capabilities: vec![NUMBER_CAPABILITY_VALUES
                            [at % NUMBER_CAPABILITY_VALUES.len()]
                        .to_string()],
                        created_at: base() + Duration::from_secs(at as u64),
                    },
                );
            }
        }

        // Page sizes spanning both published bounds, one that divides the count and one
        // that does not, plus the request that names no size at all.
        let sizes: &[Option<i64>] = &[
            Some(MIN_MAX_RESULTS),
            Some(2),
            Some(7),
            Some(MAX_MAX_RESULTS),
            None,
        ];
        for size in sizes {
            let case = format!("{count} held numbers, MaxResults={size:?}");
            let page_size = match size {
                Some(size) => usize::try_from(*size).expect("the page size is positive"),
                None => ORIGINATION_NUMBERS_PAGE_SIZE,
            };
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let rendered = size.map(|size| size.to_string());
                let mut params: Vec<(&str, &str)> = Vec::new();
                if let Some(rendered) = rendered.as_deref() {
                    params.push(("MaxResults", rendered));
                }
                if let Some(token) = token.as_deref() {
                    params.push(("NextToken", token));
                }
                let answer = list(&sns, &params).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} page {pages}: expected 200, got {} / {}",
                    answer.status, answer.body
                );
                let page = reported_numbers(&answer);
                assert!(
                    page.len() <= page_size,
                    "case {case:?} page {pages}: expected at most {page_size} entries, got {}",
                    page.len()
                );
                pages += 1;
                token = answer.element("NextToken");
                if token.is_some() {
                    assert_eq!(
                        page.len(),
                        page_size,
                        "case {case:?} page {pages}: a page followed by a token must be \
                         full, got {}",
                        page.len()
                    );
                }
                walked.extend(page.into_iter().map(|entry| entry.phone_number));
                if token.is_none() {
                    break;
                }
                assert!(
                    walked.len() < held.len(),
                    "case {case:?}: a NextToken was returned with every number already \
                     reported, got {walked:?}"
                );
            }
            assert_eq!(
                walked, held,
                "case {case:?}: expected every held number once, oldest first, got {walked:?}"
            );
            let want_pages = usize::max(count.div_ceil(page_size), 1);
            assert_eq!(
                pages, want_pages,
                "case {case:?}: expected {want_pages} page(s), got {pages}"
            );
        }
    }
}

/// A `MaxResults` outside the modelled range is refused before the listing is served,
/// at both published bounds plus one past each.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let sns = Sns::new();
    seed(&sns, &SEEDED[0], base());

    let cases: &[(&str, i64, bool)] = &[
        ("one below the published minimum", MIN_MAX_RESULTS - 1, false),
        ("at the published minimum", MIN_MAX_RESULTS, true),
        ("inside the range", 15, true),
        ("at the published maximum", MAX_MAX_RESULTS, true),
        ("one past the published maximum", MAX_MAX_RESULTS + 1, false),
        ("far past the published maximum", 1_000, false),
        ("negative", -1, false),
    ];

    for (case, max_results, accepted) in cases {
        let value = max_results.to_string();
        let answer = list(&sns, &[("MaxResults", value.as_str())]).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_numbers(&answer),
                vec![expected(&SEEDED[0], base())],
                "case {case:?}: expected the one held number, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                phone_numbers_block(&answer.body).is_none(),
                "case {case:?}: a refused request must serve no listing, got {}",
                answer.body
            );
        }
    }

    assert_eq!(
        sns.state().lock().origination_numbers.len(),
        1,
        "the refused calls must leave the inventory alone"
    );
}

/// A `NextToken` this listing did not mint is bad input: the refusal states the
/// member's sentence, serves no page, and leaves the inventory alone.
#[tokio::test]
async fn a_next_token_the_listing_did_not_mint_is_refused() {
    let sns = Sns::new();
    for number in SEEDED {
        seed(&sns, number, base() + Duration::from_secs(number.created_after));
    }

    // One row per way a value can fail to be a token this listing minted.
    let cases: &[(&str, &str)] = &[
        ("not a token at all", "bogus"),
        ("empty", ""),
        ("blank", " "),
        ("zero padded", "007"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("exponential", "1e1"),
        ("past the end of the inventory", "99"),
    ];
    for (case, token) in cases {
        let answer = list(&sns, &[("NextToken", token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
        assert!(
            phone_numbers_block(&answer.body).is_none(),
            "case {case:?}: a refused listing must serve no numbers, got {}",
            answer.body
        );
    }

    // The token the listing hands out is the one it takes back.
    let first = list(&sns, &[("MaxResults", "2")]).await;
    let token = first
        .element("NextToken")
        .unwrap_or_else(|| panic!("expected a NextToken on a partial page, got {}", first.body));
    let second = list(&sns, &[("MaxResults", "2"), ("NextToken", token.as_str())]).await;
    assert_eq!(
        second.status, 200,
        "the listing's own token: expected 200, got {} / {}",
        second.status, second.body
    );
    assert_eq!(
        sns.state().lock().origination_numbers.len(),
        SEEDED.len(),
        "the refused listings must leave the inventory alone"
    );
}
