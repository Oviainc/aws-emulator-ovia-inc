// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every reader of a number this service MINTED reads it by: only
//! the spelling the service hands out is that number.
//!
//! The rule: a `NextToken` and a server-minted numeric id are rendered one way —
//! plain decimal digits — so a value that spells the same number any other way
//! (a leading `+`, a padding zero) is a value this service never handed out. It
//! therefore continues no walk and names no resource: the listing refuses it with
//! its modelled 400 instead of serving the page the number it resembles would
//! have started, and the reader of an id answers its modelled 404.
//!
//! It is the SERVICE's rule and not one listing's, so the tables below span the
//! OPERATION — the three listings that resume at a POSITION (`ListAlerts`,
//! `ListClusterAlerts`, `DescribeSchedule`, driven through the fixture
//! `tests/all/shared_position_cursor_rule.rs` fills their sequences with), the
//! one that resumes at an ALERT ID (`ListMultiplexAlerts`) and the one that
//! resumes at an OFFERING ID (`ListOfferings`, whose ids `DescribeOffering`
//! reads too) — and, within each, the spellings a client can write the same
//! number in.
//!
//! The rule holds whatever page the number an unminted spelling resembles would
//! have named — the first page, a page inside the sequence, or a position past
//! its end, which a handed-out token answers with an empty last page — so the
//! positional table below runs it at each of those pages as well as at each
//! spelling.
//!
//! What each of those readers does with a token it DID hand out is pinned by
//! `tests/all/shared_position_cursor_rule.rs`, `tests/all/multiplex_alerts.rs`
//! and `tests/all/offering_catalog.rs`; this suite pins which spellings of one
//! are the token at all.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::settings;
use crate::multiplex_start::{add_programs, multiplex_with_settings, start_multiplex};
use crate::shared_position_cursor_rule::{
    answer, entry_names, filled_sequence, positional_listings, reported, token, ENTRIES,
};
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive, medialive_with_engine};

/// The ways a client can spell `minted` without writing the rendering the
/// service hands out: the spellings `str::parse` admits for an unsigned number
/// beside its plain decimal form — an explicit sign, one padding zero, several
/// padding zeros, and both at once.
fn other_spellings(minted: &str) -> Vec<String> {
    vec![
        format!("+{minted}"),
        format!("0{minted}"),
        format!("000{minted}"),
        format!("+0{minted}"),
    ]
}

/// The positions whose plain rendering a positional listing answers DIFFERENTLY,
/// over a sequence of `entries` entries: the first page, a page inside the
/// sequence, the position exactly at its end and two past it (a walk of a
/// sequence that has since shrunk, which the cursor rule answers with an empty
/// last page rather than a refusal), and one far past it.
///
/// A spelling the service never handed out is not a position at any of them, so
/// running the table at each is what keeps a reader that refuses only the
/// spellings naming a page it would otherwise have reported from passing.
fn positions(entries: usize) -> Vec<usize> {
    vec![0, 1, entries, entries + 2, entries + 7]
}

/// `ListMultiplexAlerts` on one multiplex, with `query` appended
/// (`smithy_query(query="operation", operation="ListMultiplexAlerts")`: GET
/// `/prod/multiplexes/{MultiplexId}/alerts`, `NextToken` an `@httpQuery`).
async fn list_multiplex_alerts(
    router: &MultiServiceRouter,
    multiplex_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/prod/multiplexes/{multiplex_id}/alerts{query}"),
        None,
    )
    .await
}

/// `ListOfferings` with `query` appended (`smithy_query(query="operation",
/// operation="ListOfferings")`: GET `/prod/offerings`).
async fn list_offerings(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/offerings{query}"), None).await
}

/// `DescribeOffering` for `offering_id` (`smithy_query(query="operation",
/// operation="DescribeOffering")`: GET `/prod/offerings/{OfferingId}`, where
/// `OfferingId` is an `@httpLabel` and so travels percent-encoded).
async fn describe_offering(router: &MultiServiceRouter, offering_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/prod/offerings/{}", percent_encoded(offering_id)),
        None,
    )
    .await
}

/// The entries a listing reported under `member`, whatever their shape: an
/// absent member and an empty list both mean "no entries" to a client.
fn entries(case: &str, member: &str, body: &Value) -> Vec<Value> {
    match body.get(member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a {member} list, got {other}"),
    }
}

/// Assert `body` is the modelled 400 a reader answers a token it never handed
/// out with: the client error, a message naming the value the request carried,
/// and nothing reported.
fn assert_refused(case: &str, member: &str, asked_for: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected a spelling this reader never hands out to be refused, got \
         {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "BadRequestException",
        "case {case:?}: expected the modelled client error, got {body}"
    );
    let message = error_message(body);
    assert!(
        message.contains(asked_for),
        "case {case:?}: expected the refusal to name the value the request carried \
         ({asked_for:?}), got {message:?}"
    );
    assert!(
        entries(case, member, body).is_empty(),
        "case {case:?}: expected a refusal to report no entries, got {body}"
    );
}

/// Rule: every listing of this service that resumes at a POSITION honours only
/// the spelling it mints that position in, so a token spelling the same number
/// another way is refused with the modelled 400 rather than serving the page the
/// handed-out token serves.
///
/// The rows span the OPERATION — all three positional listings, over three
/// unrelated derived sequences — and, within each, the token the service itself
/// handed out (which must be served) against four other spellings of that same
/// number (which must not).
#[tokio::test]
async fn a_positional_token_is_honoured_only_as_the_listing_mints_it() {
    let names = entry_names();
    // Per listing: how many spellings it refused, so a row that quietly stopped
    // running is visible at the end.
    let mut refused: Vec<(&'static str, usize)> = Vec::new();

    for row in positional_listings() {
        // A router of its own per row, so each listing is driven against a store
        // only this row filled.
        let (router, engine) = medialive_with_engine();
        let owner = filled_sequence(&router, &engine, &row).await;

        // The token the listing HANDS OUT for the page after a page of one: the
        // plain decimal rendering of how many entries it has reported.
        let case = format!("{} the handed-out token", row.op);
        let (status, body) = answer(
            &router,
            &row,
            &owner,
            &[("maxResults", "1".to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first page to be served, got {status}: {body}"
        );
        let minted = token(&body)
            .unwrap_or_else(|| panic!("case {case:?}: expected a token beside a page of one"));
        assert_eq!(
            minted, "1",
            "case {case:?}: expected the token to be the plain rendering of one entry reported, \
             got {minted:?}"
        );

        // The control: that spelling resumes exactly where it says, so the rows
        // below cannot pass by refusing every token.
        let (status, body) = answer(
            &router,
            &row,
            &owner,
            &[("maxResults", "1".to_string()), ("nextToken", minted.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the token the listing handed out to be served, got \
             {status}: {body}"
        );
        assert_eq!(
            reported(&case, &row, &body),
            names[1..2],
            "case {case:?}: expected the handed-out token to resume at the second entry, got \
             {body}"
        );

        // Every other spelling of that same number: refused, because the
        // listing never handed one of them out.
        let mut count = 0_usize;
        for asked_for in other_spellings(&minted) {
            let case = format!("{} nextToken={asked_for:?}", row.op);
            let (status, body) = answer(
                &router,
                &row,
                &owner,
                &[("maxResults", "1".to_string()), ("nextToken", asked_for.clone())],
            )
            .await;
            assert_refused(&case, row.member, &asked_for, status, &body);
            count += 1;
        }
        refused.push((row.op, count));
    }

    assert_eq!(
        refused.len(),
        3,
        "case \"every positional listing\": expected all three listings to have run, got \
         {refused:?}"
    );
    for (op, count) in &refused {
        assert_eq!(
            *count,
            other_spellings("1").len(),
            "case {op:?}: expected every other spelling to have been refused, got {count}"
        );
    }
}

/// Rule: the spelling is what makes a `NextToken` one this listing handed out, so
/// a spelling it never mints is refused with the modelled 400 whatever page the
/// number it resembles names — the first page, a page inside the sequence or a
/// position past its end, each of which the plain rendering of that same number
/// is served at.
///
/// The rows span the OPERATION — all three positional listings, over three
/// unrelated derived sequences — and, within each, five positions answered three
/// different ways (the whole sequence, its tail, an empty last page) crossed with
/// the plain rendering the listing mints (which must be served, and reports
/// exactly the tail from that position) and four other spellings of the same
/// number (which must not be served at all).
#[tokio::test]
async fn an_unminted_positional_spelling_is_refused_whatever_page_its_number_names() {
    let names = entry_names();
    // Per listing: the spellings it refused, so a row that quietly stopped
    // running is visible at the end.
    let mut refused: Vec<(&'static str, Vec<String>)> = Vec::new();

    for row in positional_listings() {
        // A router of its own per row, so each listing is driven against a store
        // only this row filled.
        let (router, engine) = medialive_with_engine();
        let owner = filled_sequence(&router, &engine, &row).await;
        let mut answered: Vec<String> = Vec::new();

        for position in positions(ENTRIES) {
            // The control: the plain rendering of this position is the one
            // spelling the listing hands out, so it is served and reports the
            // sequence from there on — which is what makes the refusals below a
            // statement about the SPELLING and not about the number.
            let plain = position.to_string();
            let case = format!("{} nextToken={plain:?}", row.op);
            let (status, body) =
                answer(&router, &row, &owner, &[("nextToken", plain.clone())]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the spelling this listing mints to be served, got \
                 {status}: {body}"
            );
            let served = reported(&case, &row, &body);
            assert_eq!(
                served,
                names[position.min(names.len())..],
                "case {case:?}: expected the sequence from position {position} on, got {served:?}"
            );
            let handed_out = token(&body);
            assert_eq!(
                handed_out, None,
                "case {case:?}: expected no token beside a page reaching the end, got \
                 {handed_out:?}"
            );

            // Every other spelling of that same position: refused, because the
            // listing never handed one of them out — however it would have
            // answered the position itself.
            for asked_for in other_spellings(&plain) {
                let case = format!("{} nextToken={asked_for:?}", row.op);
                let (status, body) =
                    answer(&router, &row, &owner, &[("nextToken", asked_for.clone())]).await;
                assert_refused(&case, row.member, &asked_for, status, &body);
                answered.push(asked_for);
            }
        }
        refused.push((row.op, answered));
    }

    // Every positional listing of this service ran the rows above, which is what
    // makes this a statement about the SERVICE's rule and not one listing's.
    assert_eq!(
        refused.len(),
        3,
        "case \"every positional listing\": expected all three listings to have run, got \
         {refused:?}"
    );
    let expected = positions(ENTRIES).len() * other_spellings("1").len();
    for (op, answered) in &refused {
        assert_eq!(
            answered.len(),
            expected,
            "case {op:?}: expected every other spelling of every position to have been refused, \
             got {answered:?}"
        );
    }
}

/// Rule: `ListMultiplexAlerts` resumes at the alert `Id` it hands a token out
/// as, which is the plain decimal rendering of that id, so a token spelling the
/// same id another way is refused with the modelled 400 rather than serving the
/// page that id starts.
///
/// One fixture — a running multiplex whose two programs no channel feeds, so the
/// service has raised one alert per program — read with the token the listing
/// itself handed out and then with four other spellings of it.
#[tokio::test]
async fn an_alert_id_token_is_honoured_only_as_the_listing_mints_it() {
    let case = "a multiplex with two alerting programs";
    let router = medialive();
    let multiplex_id = multiplex_with_settings(&router, case, "minted-token", settings(2_000_000, 1))
        .await;
    add_programs(&router, case, &multiplex_id, 2).await;
    // The create and the start are each asynchronous, so the clock is what
    // carries the multiplex into RUNNING, where the service observes that
    // nothing feeds its programs and raises the alerts this listing reports.
    wait_for_the_clock();
    let (status, starting) = start_multiplex(&router, &multiplex_id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartMultiplex: expected 202, got {status}: {starting}"
    );
    wait_for_the_clock();

    let (status, body) = list_multiplex_alerts(&router, &multiplex_id, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first page of alerts, got {status}: {body}"
    );
    let first = entries(case, "alerts", &body);
    assert_eq!(
        first.len(),
        1,
        "case {case:?}: expected a page of one alert, got {body}"
    );
    let minted = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a token beside a page of one: {body}"))
        .to_string();

    // The control: the token the listing handed out is the id of the alert the
    // next page starts at, and it is served.
    let (status, body) = list_multiplex_alerts(&router, &multiplex_id, &format!("?nextToken={minted}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the token the listing handed out to be served, got \
         {status}: {body}"
    );
    let resumed = entries(case, "alerts", &body);
    assert_eq!(
        resumed.len(),
        1,
        "case {case:?}: expected the handed-out token to resume at the one alert left over, got \
         {body}"
    );
    assert_eq!(
        resumed[0]["id"].as_str(),
        Some(minted.as_str()),
        "case {case:?}: expected the resumed page to start at the alert the token names, got \
         {body}"
    );

    for asked_for in other_spellings(&minted) {
        let case = format!("ListMultiplexAlerts nextToken={asked_for:?}");
        let (status, body) = list_multiplex_alerts(
            &router,
            &multiplex_id,
            &format!("?nextToken={}", percent_encoded(&asked_for)),
        )
        .await;
        assert_refused(&case, "alerts", &asked_for, status, &body);
    }
}

/// Rule: an offering id is the plain decimal rendering the catalog mints, so a
/// value spelling the same id another way is one the service never handed out:
/// `ListOfferings` refuses it as a token with its modelled 400, and
/// `DescribeOffering` answers its modelled 404 because it names no offering.
///
/// The rows are the id the catalog itself handed out — which both readers must
/// serve — against four other spellings of that same id.
#[tokio::test]
async fn an_offering_id_is_honoured_only_as_the_catalog_mints_it() {
    let case = "the published catalog";
    let router = medialive();

    let (status, body) = list_offerings(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first page of offerings, got {status}: {body}"
    );
    let minted = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a token beside a page of one: {body}"))
        .to_string();

    // The controls: the id the catalog handed out is the token the next page
    // starts at, and it is the offering `DescribeOffering` reports.
    let (status, body) = list_offerings(&router, &format!("?maxResults=1&nextToken={minted}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the token the catalog handed out to be served, got {status}: \
         {body}"
    );
    let resumed = entries(case, "offerings", &body);
    assert_eq!(
        resumed.len(),
        1,
        "case {case:?}: expected the resumed page to carry one offering, got {body}"
    );
    assert_eq!(
        resumed[0]["offeringId"].as_str(),
        Some(minted.as_str()),
        "case {case:?}: expected the resumed page to start at the offering the token names, got \
         {body}"
    );
    let (status, body) = describe_offering(&router, &minted).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the minted offering id to be described, got {status}: {body}"
    );

    for asked_for in other_spellings(&minted) {
        let case = format!("ListOfferings nextToken={asked_for:?}");
        let (status, body) = list_offerings(
            &router,
            &format!("?nextToken={}", percent_encoded(&asked_for)),
        )
        .await;
        assert_refused(&case, "offerings", &asked_for, status, &body);

        // The same value as an id names no offering, so the reader of the id
        // answers the 404 it models rather than describing the offering the
        // number it resembles.
        let case = format!("DescribeOffering {asked_for:?}");
        let (status, body) = describe_offering(&router, &asked_for).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a spelling the catalog never mints to name no offering, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected the modelled not-found error, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&asked_for),
            "case {case:?}: expected the refusal to name the id the request carried, got \
             {message:?}"
        );
    }
}
