// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is honored only in the SPELLING this service issued it in.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the request, the handler runs, and
//! the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees.
//!
//! THE RULE under test, for every paginated operation of this service (`ListFeeds`,
//! `ListDictionaries`, `SearchFixtures`): a continuation token is OPAQUE, so the only
//! tokens there are, are the byte strings the service handed out. Any other byte
//! string — including a re-spelling of a real token that encodes the same bytes in
//! another letter case — is a token this service never issued in any request, names no
//! page, and is refused with the modelled `ValidationException` (HTTP 400) carrying the
//! malformed-token sentence, rather than served as a resume point.
//!
//! The rows below differ along what the rule generalises over: which characters of a
//! real token are re-spelled (all of them, every other one, the first letter only, the
//! last letter only, one single letter at a time over the WHOLE token, and the digest
//! layer inside an otherwise canonical envelope) and which operation issued the token.

use crate::harness::{call_in_region, message_of, minimal_feed, router_with_engine, REGION};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::types::Competitor;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    fixture_start_secs, parse_fixture_date, ComAmazonawsElementalinferenceElementalinferenceState,
    DataSourceSport, Fixture, FIXTURE_STATUS_SCHEDULED, PAGE_TOKEN_MALFORMED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The UTC day the fixtures this suite seeds are scheduled on, and the day its search
/// selects them with.
const FIXTURE_DAY: &str = "2026-07-09";

/// How many records of each kind the store holds: enough that page one of a one-record
/// page leaves a record behind and so issues a token.
const RECORD_COUNT: usize = 3;

/// One paginated operation of this service, named by the label its wire form is
/// resolved from below.
const PAGINATED_OPERATIONS: [&str; 3] = ["ListFeeds", "ListDictionaries", "SearchFixtures"];

/// One seeded fixture: the id and the slot are all this suite selects on.
fn seeded_fixture(index: usize) -> Fixture {
    let epoch_day = parse_fixture_date(FIXTURE_DAY)
        .unwrap_or_else(|| panic!("{FIXTURE_DAY:?} is a real UTC day"));
    Fixture {
        id: format!("spell-{index:02}"),
        sport: DataSourceSport::Basketball,
        name: format!("Spelling Spartans at Case Crows {index}"),
        group: None,
        scheduled_start_secs: fixture_start_secs(epoch_day, index as i64, 0)
            .unwrap_or_else(|| panic!("slot {index} of {FIXTURE_DAY:?} is a representable instant")),
        status: FIXTURE_STATUS_SCHEDULED.to_string(),
        competitors: vec![
            Competitor {
                name: Some("Spelling Spartans".to_string()),
                is_home: Some(true),
            },
            Competitor {
                name: Some("Case Crows".to_string()),
                is_home: Some(false),
            },
        ],
    }
}

/// A router whose store holds [`RECORD_COUNT`] feeds, dictionaries and fixtures, so
/// every paginated operation below has a page one that issues a token.
async fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    for index in 0..RECORD_COUNT {
        let (status, created) = call_in_region(
            &router,
            REGION,
            "POST",
            "/v1/feed",
            Some(minimal_feed(&format!("spell-feed-{index}"))),
        )
        .await;
        assert_eq!(
            status, 202,
            "CreateFeed answers 202, got {status} with {created}"
        );
        let (status, created) = call_in_region(
            &router,
            REGION,
            "POST",
            "/v1/dictionary",
            Some(json!({"name": format!("spelldict{index}"), "language": "eng"})),
        )
        .await;
        assert_eq!(
            status, 202,
            "CreateDictionary answers 202, got {status} with {created}"
        );
    }
    {
        let state = engine.state::<ComAmazonawsElementalinferenceElementalinferenceState>();
        let mut state = state.lock();
        state.clear_fixtures();
        state.seed_fixtures((0..RECORD_COUNT).map(seeded_fixture));
    }
    router
}

/// Ask `operation` for one page of one record, with the `nextToken` the case carries.
async fn one_page(
    router: &MultiServiceRouter,
    operation: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    if operation == "SearchFixtures" {
        let mut body = json!({"sport": "basketball", "startDate": FIXTURE_DAY, "maxResults": 1});
        if let Some(token) = next_token {
            body["nextToken"] = json!(token);
        }
        return call_in_region(router, REGION, "POST", "/v1/fixtures", Some(body)).await;
    }
    let path = match operation {
        "ListFeeds" => "/v1/feeds",
        "ListDictionaries" => "/v1/dictionaries",
        other => panic!("no such paginated operation: {other}"),
    };
    let uri = match next_token {
        Some(token) => format!("{path}?maxResults=1&nextToken={token}"),
        None => format!("{path}?maxResults=1"),
    };
    call_in_region(router, REGION, "GET", &uri, None).await
}

/// The member one page of `operation` reports its records under, and the member a
/// record carries its id under (`OPERATIONS.md`).
fn record_members(operation: &str) -> (&'static str, &'static str) {
    match operation {
        "ListFeeds" => ("feeds", "id"),
        "ListDictionaries" => ("dictionaries", "id"),
        "SearchFixtures" => ("fixtures", "fixtureId"),
        other => panic!("no such paginated operation: {other}"),
    }
}

/// The ids one 200 page reports, in the order it reports them.
fn ids_of(operation: &str, page: &Value) -> Vec<String> {
    let (member, id_member) = record_members(operation);
    page[member]
        .as_array()
        .unwrap_or_else(|| panic!("the response carries a {member} list, got {page}"))
        .iter()
        .map(|record| {
            record[id_member]
                .as_str()
                .unwrap_or_else(|| panic!("a record carries a {id_member}, got {record}"))
                .to_string()
        })
        .collect()
}

/// Page one of `operation`, and the token it issued.
async fn first_page_token(router: &MultiServiceRouter, operation: &str) -> String {
    let (status, page) = one_page(router, operation, None).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: page one answers 200, got {status} with {page}"
    );
    page["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {operation:?}: records remain, so a nextToken comes back, got {page}")
        })
        .to_string()
}

/// Assert `answered` is the modelled refusal for a token this service never issued.
fn assert_malformed(label: &str, answered: (u16, Value)) {
    let (status, error) = answered;
    assert_eq!(
        status, 400,
        "case {label:?}: a nextToken this service never issued is refused with a 400, got \
         {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is a ValidationException, got {error}"
    );
    assert_eq!(
        message_of(&error),
        PAGE_TOKEN_MALFORMED_MESSAGE,
        "case {label:?}: expected the sentence for a token that is none of this service's, \
         got {error}"
    );
}

/// `token` with the character at `index` upper-cased, which re-spells the hex it is
/// written in without changing the bytes it encodes.
fn upper_at(token: &str, index: usize) -> String {
    token
        .char_indices()
        .map(|(at, character)| {
            if at == index {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}

/// Every position of `token` holding a hex letter, which is where a re-spelling can
/// differ at all (a decimal digit has one spelling).
fn letter_positions(token: &str) -> Vec<usize> {
    token
        .char_indices()
        .filter(|(_, character)| character.is_ascii_alphabetic())
        .map(|(at, _)| at)
        .collect()
}

/// The re-spellings of a real token this suite submits: a label and the token as the
/// case spells it. None of them is a byte string the service ever handed out, while all
/// of them encode the bytes of a real token.
fn respellings(real: &str) -> Vec<(String, String)> {
    let letters = letter_positions(real);
    let first = *letters.first().unwrap_or_else(|| {
        panic!("a token is hex of a payload holding ':' (0x3a), so it holds a hex letter: {real:?}")
    });
    let last = *letters
        .last()
        .unwrap_or_else(|| panic!("a token holds a hex letter: {real:?}"));
    let mut cases = vec![
        ("every character upper case".to_string(), real.to_uppercase()),
        (
            "every other hex letter upper case".to_string(),
            // Mixed case: every second position of the token that holds a letter at
            // all. (The high nibble of any ASCII byte of a token's payload is 2..=7, so
            // a token's letters all sit at odd positions, and upper-casing by position
            // parity would change nothing.)
            letters
                .iter()
                .step_by(2)
                .fold(real.to_string(), |spelled, at| upper_at(&spelled, *at)),
        ),
        (
            "the first hex letter upper case".to_string(),
            upper_at(real, first),
        ),
        (
            "the last hex letter upper case".to_string(),
            upper_at(real, last),
        ),
    ];
    // One letter at a time, over the whole token: no single re-spelled character is a
    // token of this service either.
    cases.extend(
        letters
            .iter()
            .map(|at| (format!("the hex letter at {at} upper case"), upper_at(real, *at))),
    );
    // And the re-spelling one layer in: the criteria digest INSIDE an envelope that is
    // itself spelled the way the encoder writes it.
    if let Some(inner) = digest_respelled(real) {
        cases.push(("the criteria digest upper case".to_string(), inner));
    }
    cases
}

/// `real` with the hex of the criteria digest it carries upper-cased, re-encoded in the
/// lower-case hex the envelope is written in — or `None` when the digest holds no
/// letter to re-spell.
///
/// The payload layout is this service's own (`crate::state::encode_page_token`): a
/// layout marker, the criteria digest and the id the page ended on, colon-separated.
fn digest_respelled(real: &str) -> Option<String> {
    let payload = String::from_utf8(hex::decode(real).expect("a real token is hex"))
        .expect("a real token's payload is text");
    let mut fields = payload.splitn(3, ':');
    let (layout, digest, last_id) = (fields.next()?, fields.next()?, fields.next()?);
    let upper = digest.to_uppercase();
    (upper != digest).then(|| hex::encode(format!("{layout}:{upper}:{last_id}")))
}

/// A `nextToken` re-spelled in another letter case is a token this service never
/// issued: it is refused with the modelled `ValidationException`, the same answer every
/// other string that is none of this service's tokens gets — and the token as issued
/// still continues the listing afterwards, because a refused read changes nothing.
#[tokio::test]
async fn a_next_token_respelled_in_another_case_is_refused() {
    let router = seeded_router().await;

    for operation in PAGINATED_OPERATIONS {
        let real = first_page_token(&router, operation).await;
        let (status, page_two) = one_page(&router, operation, Some(&real)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the token as issued is served, got {status} with {page_two}"
        );
        let expected = ids_of(operation, &page_two);
        assert_eq!(
            expected.len(),
            1,
            "case {operation:?}: page two carries the one next record, got {page_two}"
        );

        for (spelling, token) in respellings(&real) {
            let label = format!("{operation}: {spelling}");
            assert_ne!(
                token, real,
                "case {label:?}: the re-spelling differs from the token as issued"
            );
            assert_malformed(&label, one_page(&router, operation, Some(&token)).await);
        }

        // The token as issued is still the one resume point, unchanged by the refusals.
        let (status, again) = one_page(&router, operation, Some(&real)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the token as issued is still served, got {status} with {again}"
        );
        assert_eq!(
            ids_of(operation, &again),
            expected,
            "case {operation:?}: it names the same page it did before, got {again}"
        );
    }
}
