// SPDX-License-Identifier: CC0-1.0

//! What a term grant's `termToken` is, and what it therefore authorises.
//!
//! ONE rule is pinned here, over a SET of tokens rather than any one of them: **the
//! receipt an operation of this service mints — the value an announced document URL
//! carries, and the only thing that authorises a fetch of a document gated behind it — is
//! drawn from neither a store counter nor anything the request carried, so a caller that
//! was handed no URL can derive none of it, and only the receipt a mint answered with
//! authorises the document that mint announced.** It is the rule the inquiry-export plane
//! already answers under (`export_document_authorisation.rs` pins it there), asked of the
//! term grant `GetTermForReport` mints: the receipt that gates every report whose
//! `acceptanceType` requires the explicit acceptance of its term.
//!
//! Both planes that read that receipt are asserted for every row, because both decide the
//! same question: `GetReport`, which answers a modelled `400 ValidationException` for a
//! token it never issued, and the UNSIGNED document plane behind the announced URL, which
//! answers `403 AccessDenied` — the plane a guessed receipt would otherwise reach with no
//! credential at all.
//!
//! The rows differ along what the rule generalises over: the ordinal a counter-derived
//! guess names (the one before the first mint, the first few, the one the last mint took
//! and the one after it), the store prefix it carries, an id the request itself carried, a
//! receipt mangled three ways, a receipt minted for another report's term, a word, and the
//! absence of a receipt altogether — and, across emulators, every receipt another process
//! minted.
//!
//! Every assertion goes over the router, so each case exercises routing, the front door,
//! the handler, the persisted state and serialization — the same path a client takes.

use std::collections::BTreeSet;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, get_report, get_term_for_report, Answer, Expected, CATALOG};

/// The endpoint this emulator is addressed on, which is the endpoint an announced URL
/// therefore names.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// The fixed text this service answers for a token it did not issue, and the `reason` it
/// reports beside it. Spelled out here rather than imported from the crate so a changed
/// constant fails this suite instead of redefining what it asserts.
const INVALID_TOKEN: &str = "The provided token is invalid.";
/// See [`INVALID_TOKEN`].
const INVALID_TOKEN_REASON: &str = "invalidToken";

/// The wire error code the document plane refuses an unauthorised fetch with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The prefix this service's counter-minted store ids carry for the term-grant store
/// (`crate::state`'s `TERM_GRANT_ID_PREFIX`), spelled out here because a guess is written
/// from OUTSIDE the emulator: this is what a caller that read the emulator's own id
/// convention would write down.
const COUNTER_PREFIX: &str = "term";

/// How many acceptances a process takes before its receipts are offered to another one.
/// More than one, so a counter-minted receipt would collide on every ordinal rather than
/// only on the first.
const RECEIPTS_PER_PROCESS: usize = 8;

/// One acceptance, as the caller that took it holds it.
struct Accepted {
    /// The report the term was accepted for.
    report_id: String,
    /// The version the acceptance was taken through.
    version: i64,
    /// The `termToken` the acceptance answered: the receipt under test.
    receipt: String,
    /// The path of the report document the announced URL names, without its query string.
    document_path: String,
}

impl Accepted {
    /// The document URI a caller that holds this receipt follows.
    fn authorised_uri(&self) -> String {
        format!("{}?termToken={}", self.document_path, self.receipt)
    }

    /// The same document URI, presenting `token` instead of the receipt.
    fn uri_presenting(&self, token: &str) -> String {
        if token.is_empty() {
            return self.document_path.clone();
        }
        format!("{}?termToken={token}", self.document_path)
    }
}

/// `GetReport`'s own `termToken` constraints (`ShortStringAttribute`, `@length(1..256)`,
/// `^[a-zA-Z0-9_\-\s]*$`, as `smithy_query(query="operation",operation="GetReport")`
/// reports them), judged here rather than through the crate so a token this service mints
/// but its own `GetReport` would refuse fails this suite.
fn satisfies_term_token_constraints(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c.is_whitespace())
}

/// The catalog rows whose `acceptanceType` requires the explicit acceptance of their term
/// — the downloads a receipt gates at all. Each row carries the versions it publishes, so
/// which version a case accepts through comes from the harness ([`Expected::latest`])
/// rather than from a second copy of that rule here.
fn gated_reports() -> Vec<&'static Expected> {
    CATALOG
        .iter()
        .filter(|row| row.requires_explicit_acceptance())
        .collect()
}

/// Accept the term governing one version of `report_id` and take the receipt and the
/// document path the announcement carries.
async fn accept(router: &MultiServiceRouter, report_id: &str, version: i64) -> Accepted {
    let term = get_term_for_report(
        router,
        &format!("?reportId={report_id}&reportVersion={version}"),
    )
    .await;
    assert_eq!(
        term.status, 200,
        "case {report_id:?} v{version}: the acceptance must be served, got {} with {:?}",
        term.status, term.body
    );
    let receipt = term.body["termToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the acceptance carries no termToken: {:?}", term.body))
        .to_string();
    assert!(
        satisfies_term_token_constraints(&receipt),
        "case {report_id:?} v{version}: a minted receipt must satisfy GetReport's own \
         termToken constraints, got {receipt:?}"
    );

    // The report document the receipt authorises, read back through GetReport: the URL it
    // announces is the one a caller follows.
    let download = get_report(
        router,
        &format!("?reportId={report_id}&reportVersion={version}&termToken={receipt}"),
    )
    .await;
    assert_eq!(
        download.status, 200,
        "case {report_id:?} v{version}: the receipt its own acceptance minted must download \
         the report, got {} with {:?}",
        download.status, download.body
    );
    let url = download.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("the download carries no URL: {:?}", download.body))
        .to_string();
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"));
    let (host, path_and_query) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("{url:?} carries no path"));
    assert_eq!(
        host, ENDPOINT,
        "case {report_id:?} v{version}: the announced URL must name the endpoint the caller \
         reached, got {url:?}"
    );
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    Accepted {
        report_id: report_id.to_string(),
        version,
        receipt,
        document_path: format!("/{path}"),
    }
}

/// What an unsigned fetch of a document URI answered: the status and the `x-amzn-errortype`
/// a refusal carries.
struct Fetched {
    status: u16,
    error_type: String,
}

/// Follow a document URI exactly as a client follows an announced URL — addressed to the
/// endpoint that URL names, with NO credential of any kind.
async fn fetch(router: &MultiServiceRouter, uri: &str) -> Fetched {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", ENDPOINT)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Fetched { status, error_type }
}

/// `GetReport` of `accepted`'s report version, presenting `token`.
async fn download_presenting(
    router: &MultiServiceRouter,
    accepted: &Accepted,
    token: &str,
) -> Answer {
    get_report(
        router,
        &format!(
            "?reportId={}&reportVersion={}&termToken={token}",
            accepted.report_id, accepted.version
        ),
    )
    .await
}

/// A value a caller that read this emulator's id convention can write down: the `nth` id
/// its store counters mint.
fn counter_derived(nth: usize) -> String {
    format!("{COUNTER_PREFIX}-{nth:016x}")
}

/// Assert that `token` authorises neither plane for `accepted`: `GetReport` refuses it as
/// the modelled `ValidationException` and serves no URL, and the unsigned document plane
/// refuses the fetch.
async fn authorises_nothing(
    router: &MultiServiceRouter,
    accepted: &Accepted,
    case: &str,
    token: &str,
) {
    if !token.is_empty() {
        let refused = download_presenting(router, accepted, token).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token no acceptance minted must be refused 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(INVALID_TOKEN),
            "case {case:?}: expected this service's invalid-token text, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["reason"].as_str(),
            Some(INVALID_TOKEN_REASON),
            "case {case:?}: expected the {INVALID_TOKEN_REASON:?} reason, got {:?}",
            refused.body
        );
        assert!(
            refused.body["documentPresignedUrl"].is_null(),
            "case {case:?}: a refused download must carry no URL, got {:?}",
            refused.body
        );
    }

    let fetched = fetch(router, &accepted.uri_presenting(token)).await;
    assert_eq!(
        fetched.status, 403,
        "case {case:?}: the unsigned document plane must refuse a fetch presenting it, got \
         HTTP {}",
        fetched.status
    );
    assert_eq!(
        fetched.error_type, ACCESS_DENIED,
        "case {case:?}: expected the {ACCESS_DENIED:?} error type, got {:?}",
        fetched.error_type
    );
}

/// Assert that `accepted`'s own receipt still authorises both planes.
async fn authorises_its_document(router: &MultiServiceRouter, accepted: &Accepted, case: &str) {
    let served = download_presenting(router, accepted, &accepted.receipt).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: the receipt its own acceptance minted must download the report, got {} \
         with {:?}",
        served.status, served.body
    );
    let fetched = fetch(router, &accepted.authorised_uri()).await;
    assert_eq!(
        fetched.status, 200,
        "case {case:?}: the announced URL must serve the document, got HTTP {}",
        fetched.status
    );
}

#[tokio::test]
async fn a_derivable_value_is_no_term_grant_receipt() {
    let router = emulator();
    let gated = gated_reports();
    let (first, second) = (
        *gated.first().expect("the catalog gates a report"),
        *gated.get(1).expect("the catalog gates a second report"),
    );
    // Two acceptances of two different reports, taken through two different versions: each
    // report is governed by its own term document, so one's receipt is another's guess. The
    // second is taken first so the counter has already advanced when the report under test
    // accepts.
    let other = accept(&router, second.id, 1).await;
    let accepted = accept(&router, first.id, first.latest()).await;
    let mints = 2;

    let mut guesses: Vec<(String, String)> = vec![
        ("no token at all".to_string(), String::new()),
        ("a word".to_string(), "unaccepted".to_string()),
        (
            "the store prefix alone".to_string(),
            COUNTER_PREFIX.to_string(),
        ),
        (
            "the report id the request carried".to_string(),
            accepted.report_id.clone(),
        ),
        (
            "the receipt in upper case".to_string(),
            accepted.receipt.to_uppercase(),
        ),
        (
            "the receipt truncated".to_string(),
            accepted
                .receipt
                .chars()
                .take(accepted.receipt.chars().count().saturating_sub(1))
                .collect(),
        ),
        (
            "the receipt with a digit appended".to_string(),
            format!("{}0", accepted.receipt),
        ),
        (
            "another report's receipt".to_string(),
            other.receipt.clone(),
        ),
    ];
    // Every ordinal a counter-derived guess can name around the mints this process took:
    // the one before the first, the first few, the one the last acceptance took, and the
    // one after it.
    for nth in [0, 1, 2, 3, mints, mints + 1] {
        guesses.push((
            format!("the counter-derived id {nth}"),
            counter_derived(nth),
        ));
    }

    for (label, token) in &guesses {
        let case = format!("{} v{}: {label}", accepted.report_id, accepted.version);
        authorises_nothing(&router, &accepted, &case, token).await;
    }

    // And what the acceptance DID hand out still authorises its document, after every one
    // of those refusals.
    authorises_its_document(&router, &accepted, "the minted receipt").await;
}

#[tokio::test]
async fn every_gated_report_refuses_the_receipt_of_another_term() {
    let router = emulator();
    let gated = gated_reports();
    assert!(
        gated.len() > 1,
        "the catalog must gate more than one report for this rule to be observable, got {:?}",
        gated.iter().map(|row| row.id).collect::<Vec<_>>()
    );

    // One acceptance per gated report, through the newest version that report publishes.
    let mut accepted = Vec::new();
    for row in &gated {
        accepted.push(accept(&router, row.id, row.latest()).await);
    }

    // Every receipt is its own: no two acceptances share one, whichever report they were
    // taken on.
    let receipts: BTreeSet<&str> = accepted
        .iter()
        .map(|taken| taken.receipt.as_str())
        .collect();
    assert_eq!(
        receipts.len(),
        accepted.len(),
        "every acceptance must mint its own receipt, got {:?}",
        accepted
            .iter()
            .map(|taken| taken.receipt.as_str())
            .collect::<Vec<_>>()
    );

    for taken in &accepted {
        for other in &accepted {
            if other.report_id == taken.report_id {
                continue;
            }
            let case = format!(
                "{} v{} presenting the receipt of {}",
                taken.report_id, taken.version, other.report_id
            );
            authorises_nothing(&router, taken, &case, &other.receipt).await;
        }
        authorises_its_document(
            &router,
            taken,
            &format!("{} v{}: its own receipt", taken.report_id, taken.version),
        )
        .await;
    }
}

#[tokio::test]
async fn a_receipt_minted_in_one_emulator_authorises_nothing_in_another() {
    let gated = *gated_reports().first().expect("the catalog gates a report");
    // Two emulators that took the same calls in the same order: a receipt drawn from a
    // store counter would be the SAME value in both, so every receipt of one authorising
    // nothing in the other is what says it is drawn from no counter.
    let left = emulator();
    let right = emulator();
    let mut left_receipts = Vec::new();
    let mut right_receipts = Vec::new();
    let mut mirror = None;
    for nth in 1..=RECEIPTS_PER_PROCESS {
        left_receipts.push(accept(&left, gated.id, gated.latest()).await);
        let taken = accept(&right, gated.id, gated.latest()).await;
        if nth == RECEIPTS_PER_PROCESS {
            mirror = Some(taken);
        } else {
            right_receipts.push(taken);
        }
    }
    let mirror = mirror.expect("one acceptance of the second emulator is kept");

    let left_set: BTreeSet<&str> = left_receipts
        .iter()
        .map(|taken| taken.receipt.as_str())
        .collect();
    let right_set: BTreeSet<&str> = right_receipts
        .iter()
        .map(|taken| taken.receipt.as_str())
        .chain([mirror.receipt.as_str()])
        .collect();
    assert_eq!(
        left_set.len(),
        left_receipts.len(),
        "the receipts of one emulator must be distinct, got {left_set:?}"
    );
    let shared: Vec<&&str> = left_set.intersection(&right_set).collect();
    assert!(
        shared.is_empty(),
        "case {RECEIPTS_PER_PROCESS} acceptances in each of two emulators: no receipt of one \
         may be derivable in the other, yet {shared:?} appear in both"
    );

    // And each of those receipts authorises nothing in the emulator that did not mint it,
    // on either plane.
    for (nth, taken) in left_receipts.iter().enumerate() {
        let case = format!("acceptance {} of the first emulator, offered to the second", nth + 1);
        authorises_nothing(&right, &mirror, &case, &taken.receipt).await;
    }
    authorises_its_document(&right, &mirror, "the second emulator's own receipt").await;
}
