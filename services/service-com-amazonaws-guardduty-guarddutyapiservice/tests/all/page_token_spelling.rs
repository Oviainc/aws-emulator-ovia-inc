// SPDX-License-Identifier: CC0-1.0

//! A page token resolves in the ONE spelling the service minted it in, and in no
//! other.
//!
//! Every request goes over the wire through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! [`crate::detector_lifecycle`] suite), so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rule under test, asserted over a SET of listings and a SET of spellings rather
//! than one recorded call: a `nextToken` is opaque, so the only string a paginated
//! report resumes at a position for is the exact one it handed out for that position;
//! any other rendering of the same position — padded to another width, carrying an
//! upper-case hexadecimal digit, carrying a sign — is a token this service never
//! issued and is refused with HTTP 400 under the `InvalidInputException` class naming
//! `nextToken`, exactly like a token belonging to another listing.
//!
//! The rows span:
//!
//! 1. the LISTING paged — four reports over four resource families, three of them
//!    paged through query parameters and one through a request body;
//! 2. the way the spelling differs from the minted one — a padding digit removed, a
//!    padding digit added, the case of a hexadecimal digit, a sign in front of the
//!    position, a sign inside it, and the whole token in upper case.
//!
//! Every listing holds more entries than the position its token names, so the
//! position a re-spelling resolves to would be in range: each refusal is about the
//! SPELLING and not about the position being past the end. The positive row beside it
//! pins what the minted token still does — it resumes the listing at the entry after
//! the page it ended — so the refusals cannot be passed by refusing every token.

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use crate::member_association::create_members;

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// How many entries each listing under test is given, beyond the page size below: so
/// a first page leaves entries behind, a token is owed, and the position it names is
/// one the listing can answer.
const ENTRIES: usize = 12;

/// The page size every listing is first asked for. The token that comes back names
/// this position, whose hexadecimal rendering carries a LETTER, which is what lets a
/// row differ from the minted spelling in the CASE of a digit alone.
const PAGE: usize = 10;

/// One ordered list a caller can page, with the resource family behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// `ListFilters`, paged through query parameters.
    Filters,
    /// `ListIPSets`, paged through query parameters.
    IpSets,
    /// `ListMembers`, paged through query parameters.
    Members,
    /// `ListFindings`, paged through the request body.
    Findings,
}

/// Every listing the rows below are asserted over.
const LISTED: [Listed; 4] = [
    Listed::Filters,
    Listed::IpSets,
    Listed::Members,
    Listed::Findings,
];

impl Listed {
    /// The URI collection segment a listing reported through a GET lives under, and
    /// `None` for one reported through a POST body.
    fn collection(self) -> Option<&'static str> {
        match self {
            Self::Filters => Some("filter"),
            Self::IpSets => Some("ipset"),
            Self::Members => Some("member"),
            Self::Findings => None,
        }
    }

    /// Ask this listing for one page: `max_results` entries (the report's own page
    /// size when `None`), resuming at `token` when one is given.
    async fn page(
        self,
        router: &Router,
        detector_id: &str,
        max_results: Option<usize>,
        token: Option<&str>,
    ) -> (u16, Value) {
        match self.collection() {
            Some(collection) => {
                let mut parameters: Vec<String> = Vec::new();
                if let Some(size) = max_results {
                    parameters.push(format!("maxResults={size}"));
                }
                if let Some(token) = token {
                    parameters.push(format!("nextToken={}", in_query(token)));
                }
                let query = if parameters.is_empty() {
                    String::new()
                } else {
                    format!("?{}", parameters.join("&"))
                };
                call(
                    router,
                    "GET",
                    &format!("/detector/{detector_id}/{collection}{query}"),
                    None,
                )
                .await
            }
            None => {
                let mut body = json!({});
                if let Some(size) = max_results {
                    body["maxResults"] = json!(size);
                }
                if let Some(token) = token {
                    body["nextToken"] = json!(token);
                }
                call(
                    router,
                    "POST",
                    &format!("/detector/{detector_id}/findings"),
                    Some(body),
                )
                .await
            }
        }
    }

    /// The keys one page of this listing reported, in the order it reported them.
    fn entries(self, body: &Value, case: &str) -> Vec<String> {
        match self {
            Self::Filters => strings(&body["filterNames"], case),
            Self::IpSets => strings(&body["ipSetIds"], case),
            Self::Findings => strings(&body["findingIds"], case),
            Self::Members => body["members"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|member| {
                    member["accountId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected an accountId on every member, got {member:?}")
                        })
                        .to_string()
                })
                .collect(),
        }
    }

    /// One page of this listing, failing the test unless the call succeeded: its
    /// entries and the token it owes for the page after it.
    async fn page_ok(
        self,
        router: &Router,
        detector_id: &str,
        max_results: Option<usize>,
        token: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let (status, body) = self.page(router, detector_id, max_results, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {self:?}, got {status} {body:?}"
        );
        let next = body["nextToken"]
            .as_str()
            .filter(|next| !next.is_empty())
            .map(str::to_string);
        (self.entries(&body, case), next)
    }
}

/// A token as a QUERY parameter value carries its one non-hexadecimal character
/// percent-encoded, so a report paged through the URI is handed the same bytes as one
/// paged through a JSON body.
fn in_query(token: &str) -> String {
    token.replace('+', "%2B")
}

/// The strings a reported list carries, failing the test on an entry that is not one.
fn strings(value: &Value, case: &str) -> Vec<String> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string entry, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// A router holding one enabled detector with [`ENTRIES`] filters, [`ENTRIES`] IPSets,
/// [`ENTRIES`] members and the whole sample-finding catalogue behind it — so every
/// listing of [`LISTED`] holds more than [`PAGE`] entries.
async fn router_with_listings() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Names created out of sorted order, so a page is a window of the LIST's order
    // and not of the creation order.
    for index in (0..ENTRIES).rev() {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(json!({
                "name": format!("filter-{index:02}"),
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 1}}}
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "create-filter {index}: expected 200, got {status} {body:?}"
        );

        // None of the lists asks for activation: the account's allowance for the
        // lists GuardDuty is USING is smaller than the number a detector may hold.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/ipset"),
            Some(json!({
                "name": format!("ipset-{index:02}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/lists/{index:02}.txt"),
                "activate": false
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "create-ip-set {index}: expected 200, got {status} {body:?}"
        );
    }

    let account_ids: Vec<String> = (0..ENTRIES)
        .map(|index| format!("{:012}", index + 1))
        .collect();
    let accounts: Vec<(&str, &str)> = account_ids
        .iter()
        .map(|account_id| (account_id.as_str(), "member@example.com"))
        .collect();
    let (status, body) = create_members(&router, &detector_id, &accounts).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );

    // No `findingTypes`, which generates a sample of every supported finding type —
    // far more than one page.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );

    (router, detector_id)
}

/// The token a listing hands out after its first page of [`PAGE`] entries, beside the
/// whole list in the order the report gives it — walked page by page with the tokens
/// the report itself issues, which is the only thing a caller may do with one.
async fn minted_token(router: &Router, detector_id: &str, listed: Listed) -> (String, Vec<String>) {
    let mut all: Vec<String> = Vec::new();
    let mut minted: Option<String> = None;
    let mut token: Option<String> = None;
    for index in 0.. {
        let case = format!("{listed:?} page {index} of {PAGE}");
        let (page, next) = listed
            .page_ok(router, detector_id, Some(PAGE), token.as_deref(), &case)
            .await;
        assert!(
            page.len() <= PAGE,
            "case {case:?}: a page must carry at most the {PAGE} entries asked for, got {}",
            page.len()
        );
        all.extend(page);
        if index == 0 {
            minted = next.clone();
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert!(
        all.len() > PAGE,
        "case {listed:?}: the rule needs a listing of more than {PAGE} entries, got {}",
        all.len()
    );
    let token = minted.unwrap_or_else(|| {
        panic!("case {listed:?}: entries remain after the first page, so a nextToken is owed")
    });
    (token, all)
}

/// Every way to re-spell a minted token so the re-spelling names the same position in
/// the same listing while differing, as a string, from what the minter wrote.
///
/// The transformations are stated on the token's TAIL — the position's low digit and
/// the run of padding zeros in front of it — so none of them needs to know where the
/// listing's own identity ends and the position begins. The padding rows and the
/// digit-case row are the ones a padding- or case-tolerant reader answers with a page;
/// the two sign rows are the spellings an integer parse is itself tolerant of, pinned
/// here so neither tolerance can come back.
fn respellings(token: &str) -> Vec<(String, String)> {
    let digits: Vec<char> = token.chars().collect();
    let low = digits[digits.len() - 1];
    let padding = digits[digits.len() - 2];
    assert!(
        low.is_ascii_alphabetic() && low.is_ascii_hexdigit(),
        "the token must end in a hexadecimal LETTER for the case row to differ from the minted spelling, got {token:?}"
    );
    assert_eq!(
        padding, '0',
        "the position must be zero-padded for the padding rows to name the same position, got {token:?}"
    );
    let head = &token[..token.len() - 1];
    let shortened = format!("{}{low}", &token[..token.len() - 2]);
    // Where the padding — and so the position — begins: the start of the run of zeros
    // the low digit is at the end of.
    let mut position_at = token.len() - 1;
    while position_at > 0 && digits[position_at - 1] == '0' {
        position_at -= 1;
    }

    vec![
        (
            "one padding digit fewer in front of the position".to_string(),
            shortened,
        ),
        (
            "one padding digit more in front of the position".to_string(),
            format!("{head}0{low}"),
        ),
        (
            "the position's low digit in upper case".to_string(),
            format!("{head}{}", low.to_ascii_uppercase()),
        ),
        (
            "a sign in front of the position".to_string(),
            format!("{}+{}", &token[..position_at], &token[position_at..]),
        ),
        (
            "a sign between the position's padding and its low digit".to_string(),
            format!("{}+{low}", &token[..token.len() - 2]),
        ),
        (
            "the whole token in upper case".to_string(),
            token.to_uppercase(),
        ),
    ]
}

/// Assert one refusal: HTTP 400 under this service's rejection class, with the ONE
/// page-token sentence the whole pagination surface answers — which names no member
/// — and quoting the token back nowhere.
fn assert_refused(case: &str, status: u16, body: &Value, token: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the token to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(page_token_refusal().as_str()),
        "case {case:?}: expected the page-token refusal, got {body:?}"
    );
    assert!(
        !body["message"].as_str().unwrap_or_default().contains(token),
        "case {case:?}: the refusal must not quote the token back, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The minted spelling resumes the listing; every other spelling is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_token_a_listing_minted_resumes_it_at_the_entry_after_the_page() {
    let (router, detector_id) = router_with_listings().await;

    for listed in LISTED {
        let (token, all) = minted_token(&router, &detector_id, listed).await;
        let case = format!("{listed:?} resumed at the minted token");
        let (resumed, next) = listed
            .page_ok(&router, &detector_id, Some(PAGE), Some(&token), &case)
            .await;
        let expected = all[PAGE..(2 * PAGE).min(all.len())].to_vec();
        assert_eq!(
            resumed, expected,
            "case {case:?}: the page must resume at the entry after the first page"
        );
        assert_eq!(
            next.is_some(),
            all.len() > 2 * PAGE,
            "case {case:?}: a nextToken is owed exactly while entries remain past {} of {}, got {next:?}",
            2 * PAGE,
            all.len()
        );
    }
}

#[tokio::test]
async fn a_re_spelling_of_a_token_this_service_minted_is_refused() {
    let (router, detector_id) = router_with_listings().await;

    for listed in LISTED {
        let (token, all) = minted_token(&router, &detector_id, listed).await;
        for (difference, respelled) in respellings(&token) {
            assert_ne!(
                respelled, token,
                "case {difference:?} on {listed:?}: the row must differ from the minted token"
            );
            let case = format!("{listed:?} handed {difference}");
            let (status, body) = listed
                .page(&router, &detector_id, None, Some(&respelled))
                .await;
            assert_refused(&case, status, &body, &respelled);
            // The position the re-spelling names is one this listing holds, so the
            // refusal is about the spelling alone.
            assert!(
                all.len() > PAGE,
                "case {case:?}: the position must be in range for the refusal to be about the spelling, got {} entries",
                all.len()
            );
        }
    }
}
