// SPDX-License-Identifier: CC0-1.0

//! What a row of the offering catalog reports, and how the offerings search
//! pages it.
//!
//! Three rules, each asserted over the WHOLE seeded catalog or over a table of
//! searches rather than over the one row a recording happened to carry:
//!
//! * an offering's `usageType` is the billing usage type of its own line item,
//!   so it names the term and payment option that offering is billed under (and
//!   the region and instance family where the plan type is scoped to them), and
//!   no two offerings share one;
//! * an offering's `description` names the term, the payment option and the plan
//!   type, in that order;
//! * the catalog is walked in a stable order in which, within one term and
//!   scope, offerings run in lexicographic payment-option order; and the search
//!   answers with a resumable cursor whenever it returned anything.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, so
//! what is pinned is the HTTP behaviour. The harness (`Emulator`, `text`) is the
//! one the sibling lifecycle suite defines.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator};

/// The terms the catalog sells, as `(durationSeconds, description spelling,
/// usage-type spelling)`.
const TERMS: [(i64, &str, &str); 2] = [(31_536_000, "1 year", "1yr"), (94_608_000, "3 year", "3yr")];

/// `SavingsPlanType` (model enum: Compute | EC2Instance | SageMaker | Database)
/// and how each plan type is named in an offering's description.
const PLAN_LABELS: [(&str, &str); 4] = [
    ("Compute", "Compute"),
    ("EC2Instance", "EC2 Instance"),
    ("SageMaker", "SageMaker"),
    ("Database", "Database"),
];

/// Every `SavingsPlanPaymentOption` the model declares, in lexicographic order.
const PAYMENT_OPTIONS_SORTED: [&str; 3] = ["All Upfront", "No Upfront", "Partial Upfront"];

/// The value of an offering property (`SavingsPlanOfferingPropertyKey`:
/// `region` or `instanceFamily`), or `""` when the offering does not carry it.
fn property(offering: &Value, name: &str) -> String {
    offering
        .get("properties")
        .and_then(Value::as_array)
        .and_then(|properties| {
            properties
                .iter()
                .find(|property| text(property, "name") == name)
        })
        .map(|property| text(property, "value"))
        .unwrap_or_default()
}

/// Whether `token` is a `PaginationToken` the model admits (length 0..1024,
/// pattern `^[A-Za-z0-9/=\+]+$`).
fn is_pagination_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 1024
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'=' | b'+'))
}

/// Walk a search to the end, returning every page's `(offeringIds, nextToken)`.
///
/// `request` is sent as supplied for the first page and with `nextToken` added
/// for each page after it.
async fn walk(emulator: &Emulator, case: &str, request: &Value) -> Vec<(Vec<String>, Option<String>)> {
    let mut pages: Vec<(Vec<String>, Option<String>)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next {
            page_request["nextToken"] = json!(token);
        }
        let answer = emulator
            .call("DescribeSavingsPlansOfferings", page_request.clone())
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {page_request}, got {} {:?}",
            answer.status, answer.body
        );
        let ids: Vec<String> = answer
            .list("searchResults")
            .iter()
            .map(|offering| text(offering, "offeringId"))
            .collect();
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        pages.push((ids, next.clone()));
        assert!(
            pages.len() <= 2_000,
            "case {case:?}: paging did not terminate after {} pages",
            pages.len()
        );
        if next.is_none() {
            return pages;
        }
    }
}

/// Every catalog row reports the usage type and description of its own term,
/// payment option and plan type, and a `usageTypes` search addresses exactly the
/// row that reports it.
#[tokio::test]
async fn every_offering_reports_the_usage_type_and_description_of_its_own_row() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;
    assert!(
        catalog.len() > 20,
        "the seeded catalog must be a catalog: got {} offerings",
        catalog.len()
    );

    let mut by_usage_type: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for offering in &catalog {
        let id = text(offering, "offeringId");
        let duration = offering
            .get("durationSeconds")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let Some(&(_, term_label, term_token)) =
            TERMS.iter().find(|(seconds, _, _)| *seconds == duration)
        else {
            panic!("case {id:?}: unexpected durationSeconds {duration}");
        };
        let payment_option = text(offering, "paymentOption");
        let payment_token: String = payment_option
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let plan_type = text(offering, "planType");
        let Some(&(_, plan_label)) = PLAN_LABELS.iter().find(|(value, _)| *value == plan_type)
        else {
            panic!("case {id:?}: unexpected planType {plan_type:?}");
        };

        let usage_type = text(offering, "usageType");
        let suffix = format!(":{term_token}{payment_token}");
        assert!(
            usage_type.ends_with(&suffix),
            "case {id:?} ({plan_type}/{duration}/{payment_option}): usageType must be billed under its own term and payment option, so it must end with {suffix:?}, got {usage_type:?}"
        );
        // SavingsPlanUsageType: length 0..255, pattern ^[a-zA-Z0-9_ \/.:-]+$.
        assert!(
            !usage_type.is_empty()
                && usage_type.len() <= 255
                && usage_type
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric()
                        || matches!(b, b'_' | b' ' | b'/' | b'.' | b':' | b'-')),
            "case {id:?}: usageType {usage_type:?} must satisfy the model's constraint"
        );
        // A region-scoped plan type bills under a region-prefixed usage type.
        let region = property(offering, "region");
        if !region.is_empty() {
            assert!(
                usage_type.starts_with(&format!("{region}-")),
                "case {id:?}: a usage type of an offering scoped to {region:?} must name that region, got {usage_type:?}"
            );
        }
        let family = property(offering, "instanceFamily");
        if !family.is_empty() {
            assert!(
                usage_type.contains(&format!(":{family}:")),
                "case {id:?}: a usage type of an offering scoped to instance family {family:?} must name it, got {usage_type:?}"
            );
        }

        assert_eq!(
            text(offering, "description"),
            format!("{term_label} {payment_option} {plan_label} Savings Plan"),
            "case {id:?} ({plan_type}/{duration}/{payment_option}): description must name the term, the payment option and the plan type"
        );

        by_usage_type.entry(usage_type).or_default().push(id);
    }

    for (usage_type, ids) in &by_usage_type {
        assert_eq!(
            ids.len(),
            1,
            "case {usage_type:?}: a usage type must tell one offering from another, got {ids:?}"
        );
    }

    // And the `usageTypes` filter finds that offering by the usage type it
    // reports — sampled across the catalog, so the rows include plan types,
    // regions, instance families and terms no recording cited.
    for offering in catalog.iter().step_by(catalog.len() / 12 + 1) {
        let id = text(offering, "offeringId");
        let usage_type = text(offering, "usageType");
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "usageTypes": [usage_type.clone()], "maxResults": 1000 }),
            )
            .await;
        let found: Vec<String> = answer
            .list("searchResults")
            .iter()
            .map(|result| text(result, "offeringId"))
            .collect();
        assert_eq!(
            found,
            vec![id.clone()],
            "case usageTypes {usage_type:?}: expected exactly the offering reporting it ({id:?}), got {:?}",
            answer.body
        );
    }
}

/// The catalog is enumerated in a stable order in which the offerings of one
/// term and scope run in lexicographic payment-option order.
#[tokio::test]
async fn catalog_rows_run_in_lexicographic_payment_option_order() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;

    // Group by everything that distinguishes an offering EXCEPT its payment
    // option: within a group the rows must run in payment-option order.
    let mut groups: BTreeMap<(String, i64, String, String), Vec<String>> = BTreeMap::new();
    for offering in &catalog {
        let key = (
            text(offering, "planType"),
            offering
                .get("durationSeconds")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            property(offering, "region"),
            property(offering, "instanceFamily"),
        );
        groups
            .entry(key)
            .or_default()
            .push(text(offering, "paymentOption"));
    }
    assert!(
        groups.len() >= 8,
        "the catalog must cover several plan types and terms, got {} groups",
        groups.len()
    );
    for (key, options) in &groups {
        assert_eq!(
            options.as_slice(),
            PAYMENT_OPTIONS_SORTED.as_slice(),
            "case {key:?}: a term and scope sells every payment option, in lexicographic order, got {options:?}"
        );
    }

    // The order is the same for a search that only narrows the catalog, and the
    // same again on a repeat of the identical search.
    for (case, request) in [
        ("planTypes Compute", json!({ "planTypes": ["Compute"], "maxResults": 1000 })),
        (
            "planTypes Database in one region",
            json!({
                "planTypes": ["Database"],
                "filters": [{ "name": "region", "values": ["eu-west-1"] }],
                "maxResults": 1000,
            }),
        ),
        (
            "every one-year offering",
            json!({ "durations": [31_536_000], "maxResults": 1000 }),
        ),
    ] {
        let answer = emulator
            .call("DescribeSavingsPlansOfferings", request.clone())
            .await;
        let narrowed: Vec<String> = answer
            .list("searchResults")
            .iter()
            .map(|offering| text(offering, "offeringId"))
            .collect();
        let expected: Vec<String> = catalog
            .iter()
            .filter(|offering| narrowed.contains(&text(offering, "offeringId")))
            .map(|offering| text(offering, "offeringId"))
            .collect();
        assert!(
            !narrowed.is_empty(),
            "case {case:?}: expected offerings, got {:?}",
            answer.body
        );
        assert_eq!(
            narrowed, expected,
            "case {case:?}: a narrowed search must keep catalog order"
        );
        let repeat = emulator
            .call("DescribeSavingsPlansOfferings", request.clone())
            .await;
        let repeated: Vec<String> = repeat
            .list("searchResults")
            .iter()
            .map(|offering| text(offering, "offeringId"))
            .collect();
        assert_eq!(
            repeated, narrowed,
            "case {case:?}: the same search must answer with the same rows in the same order"
        );
    }

    // The recorded row class: the Compute offerings of one term, in order.
    let compute = emulator
        .call(
            "DescribeSavingsPlansOfferings",
            json!({ "planTypes": ["Compute"], "durations": [94_608_000], "maxResults": 1000 }),
        )
        .await;
    let options: Vec<String> = compute
        .list("searchResults")
        .iter()
        .map(|offering| text(offering, "paymentOption"))
        .collect();
    assert_eq!(
        options.as_slice(),
        PAYMENT_OPTIONS_SORTED.as_slice(),
        "case three-year Compute offerings: expected {PAYMENT_OPTIONS_SORTED:?}, got {options:?}"
    );
}

/// The offerings search answers with a resumable cursor: its first answer
/// always carries one, resuming it loses and repeats nothing, and the walk ends
/// on the resumed page that has no more results to return.
#[tokio::test]
async fn the_offerings_search_answers_with_a_resumable_cursor() {
    let emulator = Emulator::new();
    let catalog: Vec<String> = emulator
        .all_offerings()
        .await
        .iter()
        .map(|offering| text(offering, "offeringId"))
        .collect();

    // The rows differ along what the rule generalises over: the page size asked
    // for (none, one, a size that divides the result set, a size that is exactly
    // the result set, a size past it) and how narrow the search is (the whole
    // catalog, one plan type, nothing at all).
    let compute = json!({ "planTypes": ["Compute"] });
    let nothing = json!({
        "planTypes": ["Compute"],
        "filters": [{ "name": "instanceFamily", "values": ["m5"] }],
    });
    let cases: [(&str, Value, i64); 8] = [
        ("the whole catalog, at the service's own page size", json!({}), 0),
        ("the whole catalog, 101 at a time", json!({}), 101),
        ("the whole catalog, at the model's largest page", json!({}), 1000),
        ("one plan type, one at a time", compute.clone(), 1),
        ("one plan type, at the service's own page size", compute.clone(), 0),
        ("one plan type, three at a time", compute.clone(), 3),
        ("one plan type, exactly its size", compute.clone(), 6),
        ("a search nothing satisfies", nothing.clone(), 5),
    ];

    for (case, filters, max_results) in cases {
        let mut request = filters.clone();
        request["maxResults"] = json!(max_results);
        let pages = walk(&emulator, case, &request).await;

        let expected: Vec<String> = {
            let mut unpaged = filters.clone();
            unpaged["maxResults"] = json!(1000);
            emulator
                .search_offerings(&unpaged)
                .await
                .iter()
                .map(|offering| text(offering, "offeringId"))
                .collect()
        };

        for (index, (ids, next)) in pages.iter().enumerate() {
            let is_last = index + 1 == pages.len();
            assert_eq!(
                next.is_none(),
                is_last,
                "case {case:?} page {index}: only the page the walk ends on carries no token, got {next:?}"
            );
            if let Some(token) = next {
                assert!(
                    is_pagination_token(token),
                    "case {case:?} page {index}: {token:?} must satisfy PaginationToken"
                );
            }
            if max_results > 0 {
                assert!(
                    i64::try_from(ids.len()).unwrap_or(i64::MAX) <= max_results,
                    "case {case:?} page {index}: a page must not exceed maxResults {max_results}, got {}",
                    ids.len()
                );
            } else {
                assert!(
                    ids.len() <= state::DEFAULT_OFFERINGS_PAGE_SIZE,
                    "case {case:?} page {index}: a page the caller did not bound must not exceed the service's own page size {}, got {}",
                    state::DEFAULT_OFFERINGS_PAGE_SIZE,
                    ids.len()
                );
            }
        }

        let walked: Vec<String> = pages.iter().flat_map(|(ids, _)| ids.clone()).collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must yield the result set once, in order"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            walked.len(),
            "case {case:?}: paging must not repeat an offering"
        );
        assert!(
            expected.is_empty() || expected.len() <= catalog.len(),
            "case {case:?}: a search cannot return more than the catalog, got {} of {}",
            expected.len(),
            catalog.len()
        );
        // The FIRST answer always carries a cursor, so a walk is at least two
        // calls long and ends on the resumed page that has nothing more to
        // return.
        let first = pages.first().expect("a walk answers at least one page");
        assert!(
            first.1.is_some(),
            "case {case:?}: the first answer of a search must carry the cursor to resume at, got {:?}",
            first.1
        );
        assert!(
            pages.len() >= 2,
            "case {case:?}: a walk that always hands back its first cursor takes at least two calls, got {}",
            pages.len()
        );
    }
}

/// A search whose filters admit nothing still answers with a cursor: the first
/// answer of a search carries one whatever its page held, and the walk
/// terminates on the next call.
#[tokio::test]
async fn a_search_whose_filters_admit_nothing_still_answers_with_a_cursor() {
    let emulator = Emulator::new();
    // Rows differ along WHY nothing matched: a plan type crossed with a
    // property it is not scoped to, a property value the catalog carries for no
    // offering, a usage type no offering reports, and a currency crossed with a
    // plan type that never bills in it.
    let cases: [(&str, Value); 4] = [
        (
            "a plan type that carries no region property",
            json!({
                "planTypes": ["Compute"],
                "filters": [{ "name": "region", "values": ["us-east-1"] }],
                "maxResults": 5,
            }),
        ),
        (
            "a region the catalog has no offering in",
            json!({
                "filters": [{ "name": "region", "values": ["me-central-1"] }],
                "maxResults": 5,
            }),
        ),
        (
            "a usage type no offering reports",
            json!({ "usageTypes": ["ComputeSP:99yrAllUpfront"], "maxResults": 1 }),
        ),
        (
            "a product type crossed with the wrong service code",
            json!({
                "productType": "Lambda",
                "serviceCodes": ["AmazonSageMaker"],
                "maxResults": 1000,
            }),
        ),
    ];

    for (case, request) in cases {
        let pages = walk(&emulator, case, &request).await;
        assert!(
            pages.iter().all(|(ids, _)| ids.is_empty()),
            "case {case:?}: expected no offering to match, got {:?}",
            pages
                .iter()
                .map(|(ids, _)| ids.len())
                .collect::<Vec<usize>>()
        );
        assert_eq!(
            pages.len(),
            2,
            "case {case:?}: the first answer carries a cursor and resuming it ends the walk: expected two pages, got {pages:?}"
        );
        let (_, first_token) = &pages[0];
        let token = first_token
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: a page that admitted nothing must still carry the cursor to resume at, got {pages:?}"));
        assert!(
            is_pagination_token(token),
            "case {case:?}: {token:?} must satisfy PaginationToken"
        );
        assert!(
            pages[1].1.is_none(),
            "case {case:?}: a resumed answer with no more results to return must answer no token, got {:?}",
            pages[1].1
        );
    }
}

/// A search that names no page size gets the service's own page: a truncated,
/// resumable page rather than the whole catalog.
#[tokio::test]
async fn a_search_that_names_no_page_size_gets_the_services_own_page() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;
    assert!(
        catalog.len() > state::DEFAULT_OFFERINGS_PAGE_SIZE,
        "this rule needs a catalog bigger than one default page: got {} offerings against a page of {}",
        catalog.len(),
        state::DEFAULT_OFFERINGS_PAGE_SIZE
    );

    // Both spellings of "no size asked for": the member left out, and the
    // member carrying the model's own default of 0.
    for (case, request) in [
        ("maxResults left out", json!({})),
        ("maxResults 0, the model's default", json!({ "maxResults": 0 })),
    ] {
        let answer = emulator
            .call("DescribeSavingsPlansOfferings", request.clone())
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {request}, got {} {:?}",
            answer.status, answer.body
        );
        let ids: Vec<String> = answer
            .list("searchResults")
            .iter()
            .map(|offering| text(offering, "offeringId"))
            .collect();
        assert_eq!(
            ids.len(),
            state::DEFAULT_OFFERINGS_PAGE_SIZE,
            "case {case:?}: expected the service's own page of {} offerings, got {}",
            state::DEFAULT_OFFERINGS_PAGE_SIZE,
            ids.len()
        );
        assert_eq!(
            ids.as_slice(),
            &catalog[..state::DEFAULT_OFFERINGS_PAGE_SIZE]
                .iter()
                .map(|offering| text(offering, "offeringId"))
                .collect::<Vec<String>>()[..],
            "case {case:?}: the page must be the first page of the catalog"
        );
        let token = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| {
                panic!("case {case:?}: a truncated page must carry a cursor, got {:?}", answer.body)
            });
        assert!(
            is_pagination_token(&token),
            "case {case:?}: {token:?} must satisfy PaginationToken"
        );

        // And resuming it walks the rest of the catalog, in order, once.
        let walked: Vec<String> = walk(&emulator, case, &request)
            .await
            .into_iter()
            .flat_map(|(ids, _)| ids)
            .collect();
        assert_eq!(
            walked,
            catalog
                .iter()
                .map(|offering| text(offering, "offeringId"))
                .collect::<Vec<String>>(),
            "case {case:?}: resuming the service's own pages must yield the whole catalog in order"
        );
    }
}

/// A token one search issued cannot resume another operation's walk, and a
/// token this paginator never issued is refused with the modelled 400.
#[tokio::test]
async fn a_token_this_search_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let plans_token = emulator
        .call("DescribeSavingsPlans", json!({ "maxResults": 1 }))
        .await
        .get("nextToken")
        .as_str()
        .map(std::string::ToString::to_string);
    let mut cases: Vec<String> = vec![
        "zzzz".to_string(),
        "AAAA".to_string(),
        "0102030405".to_string(),
        // Hex, and decodable, but minted for another paginator.
        hex::encode("plans:1"),
    ];
    cases.extend(plans_token);
    for case in cases {
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "maxResults": 1, "nextToken": case.clone() }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case nextToken {case:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
