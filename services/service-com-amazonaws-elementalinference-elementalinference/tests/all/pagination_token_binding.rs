// SPDX-License-Identifier: CC0-1.0

//! What a `nextToken` of this service BINDS, end to end over the wire.
//!
//! Every case here drives the real request path through the shared harness
//! (`tests/all/harness.rs`) — the router picks the service from the SigV4 credential
//! scope, the framework deserializes and validates the request, the handler runs, and
//! the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees.
//!
//! THE RULE under test, for every paginated operation of this service (`SearchFixtures`,
//! `ListFeeds`, `ListDictionaries`): a continuation token continues the ONE request that
//! produced it. A request that submits a token the service issued for a different
//! request — another operation, another endpoint, or other selection criteria — or a
//! token the service never issued at all, is refused with the modelled
//! `ValidationException` (HTTP 400); it is never served a page computed from criteria
//! the token does not belong to. Only the criteria that decide WHICH records a request
//! selects bind the token: `maxResults` is the `@paginated` `pageSize`
//! (`smithy_query(query="operation",operation="SearchFixtures")`), so the next batch may
//! be asked for in a different size, and two requests the model documents as the same
//! search continue each other.
//!
//! The rows below differ along what the rule generalises over: which criterion moves
//! (the sport, either end of the date window, the filters, the Region, the operation),
//! how a token is forged (not hex, hex of a bare id, a token-shaped payload with
//! another request's digest, a truncated token), and which operation issued it.

use crate::harness::{
    call, call_in_region, message_of, minimal_feed, past_the_transition_span, router_with_engine,
    REGION,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::types::Competitor;
use service_com_amazonaws_elementalinference_elementalinference::state::{
    fixture_start_secs, parse_fixture_date, ComAmazonawsElementalinferenceElementalinferenceState,
    DataSourceSport, Fixture, FIXTURE_STATUS_SCHEDULED, PAGE_TOKEN_MALFORMED_MESSAGE,
    PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The first day the catalog below holds events on.
const DAY_ONE: &str = "2026-03-14";

/// The second day it holds events on — the window's other end.
const DAY_TWO: &str = "2026-03-15";

/// A day it holds no event on, for a window that reaches one day further.
const DAY_THREE: &str = "2026-03-16";

/// Another Region's endpoint, for the cases about which endpoint issued a token.
const OTHER_REGION: &str = "eu-west-1";

/// One held fixture: the id, sport, day and competitors are everything a case here
/// selects on.
fn fixture(
    id: &str,
    sport: DataSourceSport,
    day: &str,
    hour: i64,
    competitors: &[&str],
) -> Fixture {
    let epoch_day = parse_fixture_date(day).unwrap_or_else(|| panic!("{day:?} is a real UTC day"));
    Fixture {
        id: id.to_string(),
        sport,
        name: competitors.join(" at "),
        group: None,
        scheduled_start_secs: fixture_start_secs(epoch_day, hour, 0)
            .unwrap_or_else(|| panic!("{day:?} {hour}:00 is a representable instant")),
        status: FIXTURE_STATUS_SCHEDULED.to_string(),
        competitors: competitors
            .iter()
            .enumerate()
            .map(|(index, name)| Competitor {
                name: Some((*name).to_string()),
                is_home: Some(index == 0),
            })
            .collect(),
    }
}

/// The catalog these suites install: four basketball fixtures over two days and two
/// american-football ones, with competitors chosen so a filter selects a strict subset.
/// The ids sort in the order the cases expect pages in.
fn seed_rows() -> Vec<Fixture> {
    vec![
        fixture(
            "bk-1",
            DataSourceSport::Basketball,
            DAY_ONE,
            18,
            &["Alpha Antelopes", "Omega Owls"],
        ),
        fixture(
            "bk-2",
            DataSourceSport::Basketball,
            DAY_ONE,
            20,
            &["Beacon Bears", "Cobalt Comets"],
        ),
        fixture(
            "bk-3",
            DataSourceSport::Basketball,
            DAY_ONE,
            22,
            &["Alpha Antelopes", "Delta Dolphins"],
        ),
        fixture(
            "bk-4",
            DataSourceSport::Basketball,
            DAY_TWO,
            19,
            &["Echo Eagles", "Alpha Antelopes"],
        ),
        fixture(
            "af-1",
            DataSourceSport::AmericanFootball,
            DAY_ONE,
            17,
            &["Gridiron Giants", "Harbor Hawks"],
        ),
        fixture(
            "af-2",
            DataSourceSport::AmericanFootball,
            DAY_TWO,
            21,
            &["Alpha Antelopes", "Iron Ibis"],
        ),
    ]
}

/// A router over a catalog holding exactly [`seed_rows`], and nothing else.
fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    let state = engine.state::<ComAmazonawsElementalinferenceElementalinferenceState>();
    {
        let mut state = state.lock();
        state.clear_fixtures();
        state.seed_fixtures(seed_rows());
    }
    router
}

/// `SearchFixtures`: `POST /v1/fixtures` (`OPERATIONS.md`).
async fn search(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v1/fixtures", Some(body)).await
}

/// The `fixtureId`s of a 200 search answer, in the order it carries them.
fn ids_of(answer: &Value) -> Vec<String> {
    answer["fixtures"]
        .as_array()
        .unwrap_or_else(|| panic!("fixtures is a list on {answer}"))
        .iter()
        .map(|fixture| {
            fixture["fixtureId"]
                .as_str()
                .unwrap_or_else(|| panic!("fixtureId is a string on {fixture}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` of a 200 answer, which the case needs in order to submit it again.
fn token_of(label: &str, answer: &Value) -> String {
    answer["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {label:?}: records remain, so a nextToken comes back, got {answer}")
        })
        .to_string()
}

/// Run `body` and return the page it answered with, after asserting it is a 200.
async fn page(router: &MultiServiceRouter, label: &str, body: Value) -> Value {
    let (status, answer) = search(router, body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the search answers 200, got {status} with {answer}"
    );
    answer
}

/// Assert that `answered` is the modelled refusal for a token that does not belong to
/// the request that submitted it.
fn assert_refused(label: &str, answered: (u16, Value), expected_message: &str) {
    let (status, error) = answered;
    assert_eq!(
        status, 400,
        "case {label:?}: a nextToken that does not belong to this request is a 400, got \
         {status} with {error}"
    );
    assert_eq!(
        error["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is a ValidationException, got {error}"
    );
    assert_eq!(
        message_of(&error),
        expected_message,
        "case {label:?}: expected the sentence for this rejection, got {error}"
    );
}

/// The search whose page-one token the criteria table submits: one sport over both held
/// days, a page at a time.
fn base_search() -> Value {
    json!({"sport": "basketball", "startDate": DAY_ONE, "endDate": DAY_TWO, "maxResults": 1})
}

/// One row of the criteria table: the criteria page two is asked for with, and the page
/// that must come back — or `None` when the token does not belong to those criteria.
struct CriteriaCase {
    label: &'static str,
    body: Value,
    expected_page: Option<&'static [&'static str]>,
}

/// The criteria rows, each a change to [`base_search`] with the page-one token attached.
fn criteria_cases() -> Vec<CriteriaCase> {
    let with = |mutate: fn(&mut Value)| {
        let mut body = base_search();
        mutate(&mut body);
        body
    };
    vec![
        CriteriaCase {
            label: "the same criteria",
            body: base_search(),
            expected_page: Some(&["bk-2"]),
        },
        CriteriaCase {
            label: "the same criteria, asked for in a larger page",
            body: with(|body| body["maxResults"] = json!(3)),
            expected_page: Some(&["bk-2", "bk-3", "bk-4"]),
        },
        CriteriaCase {
            label: "the same criteria with no maxResults at all",
            body: with(|body| {
                body.as_object_mut().expect("the body is an object").remove("maxResults");
            }),
            expected_page: Some(&["bk-2", "bk-3", "bk-4"]),
        },
        CriteriaCase {
            label: "the other sport the model declares",
            body: with(|body| body["sport"] = json!("american-football")),
            expected_page: None,
        },
        CriteriaCase {
            label: "a window starting a day later",
            body: with(|body| body["startDate"] = json!(DAY_TWO)),
            expected_page: None,
        },
        CriteriaCase {
            label: "a window ending a day later",
            body: with(|body| body["endDate"] = json!(DAY_THREE)),
            expected_page: None,
        },
        CriteriaCase {
            label: "no endDate, so a window of the first day only",
            body: with(|body| {
                body.as_object_mut().expect("the body is an object").remove("endDate");
            }),
            expected_page: None,
        },
        CriteriaCase {
            label: "a filter the first search did not carry",
            body: with(|body| {
                body["filters"] = json!([{"name": "COMPETITOR", "values": ["Alpha Antelopes"]}]);
            }),
            expected_page: None,
        },
    ]
}

/// A `SearchFixtures` token continues the search that produced it and no other: a
/// request whose selection criteria differ is refused instead of being served a page
/// positioned in another search's selection, while a different page SIZE — the
/// `@paginated` pageSize — is the same search.
#[tokio::test]
async fn a_search_token_is_honored_only_for_the_search_that_produced_it() {
    let router = seeded_router();

    let first = page(&router, "page one", base_search()).await;
    assert_eq!(
        ids_of(&first),
        vec!["bk-1".to_string()],
        "page one of the base search is the first fixture in id order, got {first}"
    );
    let token = token_of("page one", &first);

    for case in criteria_cases() {
        let mut body = case.body.clone();
        body["nextToken"] = json!(token);
        match case.expected_page {
            Some(expected) => {
                let answered = page(&router, case.label, body.clone()).await;
                assert_eq!(
                    ids_of(&answered),
                    expected,
                    "case {:?}: {body} continues the same search, got {answered}",
                    case.label
                );
            }
            None => assert_refused(
                case.label,
                search(&router, body).await,
                PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
            ),
        }
    }
}

/// A window stated with an explicit `endDate` and the same window stated by omitting it
/// are the SAME search — "If you omit this parameter, Elemental Inference searches only
/// the day that you specified in startDate" (`SearchFixturesRequest$endDate`) — so a
/// token of one continues the other.
#[tokio::test]
async fn the_documented_default_window_continues_the_window_it_names() {
    let router = seeded_router();
    let explicit = json!({
        "sport": "basketball", "startDate": DAY_ONE, "endDate": DAY_ONE, "maxResults": 1
    });
    let omitted = json!({"sport": "basketball", "startDate": DAY_ONE, "maxResults": 1});

    for (label, first_body, second_body) in [
        ("an explicit endDate, continued with none", &explicit, &omitted),
        ("no endDate, continued with the explicit one", &omitted, &explicit),
    ] {
        let first = page(&router, label, first_body.clone()).await;
        assert_eq!(
            ids_of(&first),
            vec!["bk-1".to_string()],
            "case {label:?}: page one is the first fixture of the day, got {first}"
        );
        let mut body = second_body.clone();
        body["nextToken"] = json!(token_of(label, &first));
        let answered = page(&router, label, body).await;
        assert_eq!(
            ids_of(&answered),
            vec!["bk-2".to_string()],
            "case {label:?}: the same window continues after the first fixture, got {answered}"
        );
    }
}

/// One filter row: the filters page two is asked for with, and the page that must come
/// back — or `None` when they are not the filters the token was issued for.
struct FilterCase {
    label: &'static str,
    filters: Value,
    expected_page: Option<&'static [&'static str]>,
}

/// The filters the base filtered search carries: one on a competitor several fixtures
/// share, one on two competitors a different pair of fixtures has. Together they select
/// `bk-1` and `bk-3`.
fn base_filters() -> Value {
    json!([
        {"name": "COMPETITOR", "values": ["Alpha"]},
        {"name": "COMPETITOR", "values": ["Owls", "Dolphins"]},
    ])
}

/// Filter rows: orderings and repetitions that select exactly the same fixtures are the
/// same search; any change to WHICH fixtures the filters select is another search.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "the same filters, in the same order",
            filters: base_filters(),
            expected_page: Some(&["bk-3"]),
        },
        FilterCase {
            label: "the two filters in the other order",
            filters: json!([
                {"name": "COMPETITOR", "values": ["Owls", "Dolphins"]},
                {"name": "COMPETITOR", "values": ["Alpha"]},
            ]),
            expected_page: Some(&["bk-3"]),
        },
        FilterCase {
            label: "one filter's values in the other order",
            filters: json!([
                {"name": "COMPETITOR", "values": ["Alpha"]},
                {"name": "COMPETITOR", "values": ["Dolphins", "Owls"]},
            ]),
            expected_page: Some(&["bk-3"]),
        },
        FilterCase {
            label: "a value repeated within one filter",
            filters: json!([
                {"name": "COMPETITOR", "values": ["Alpha", "Alpha"]},
                {"name": "COMPETITOR", "values": ["Owls", "Dolphins", "Owls"]},
            ]),
            expected_page: Some(&["bk-3"]),
        },
        FilterCase {
            label: "one filter's values narrowed",
            filters: json!([
                {"name": "COMPETITOR", "values": ["Alpha"]},
                {"name": "COMPETITOR", "values": ["Dolphins"]},
            ]),
            expected_page: None,
        },
        FilterCase {
            label: "one of the two filters dropped",
            filters: json!([{"name": "COMPETITOR", "values": ["Alpha"]}]),
            expected_page: None,
        },
        FilterCase {
            label: "a third filter appended",
            filters: json!([
                {"name": "COMPETITOR", "values": ["Alpha"]},
                {"name": "COMPETITOR", "values": ["Owls", "Dolphins"]},
                {"name": "COMPETITOR", "values": ["Antelopes"]},
            ]),
            expected_page: None,
        },
        FilterCase {
            label: "no filters at all",
            filters: json!([]),
            expected_page: None,
        },
    ]
}

/// `filters` bind the token by the selection they make: a reordering or a repetition
/// selects the same fixtures and continues the same search ("A fixture must satisfy
/// every filter in the array … Within one filter, a fixture must match at least one of
/// the values", `SearchFixturesRequest$filters`), while filters that select differently
/// are a different search and cannot use its token.
#[tokio::test]
async fn filters_bind_a_token_by_the_selection_they_make() {
    let router = seeded_router();
    let mut base = base_search();
    base["filters"] = base_filters();

    let first = page(&router, "filtered page one", base.clone()).await;
    assert_eq!(
        ids_of(&first),
        vec!["bk-1".to_string()],
        "the filtered selection starts at bk-1, got {first}"
    );
    let token = token_of("filtered page one", &first);

    for case in filter_cases() {
        let mut body = base.clone();
        body["filters"] = case.filters.clone();
        body["nextToken"] = json!(token);
        match case.expected_page {
            Some(expected) => {
                let answered = page(&router, case.label, body.clone()).await;
                assert_eq!(
                    ids_of(&answered),
                    expected,
                    "case {:?}: {body} continues the same selection, got {answered}",
                    case.label
                );
            }
            None => assert_refused(
                case.label,
                search(&router, body).await,
                PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
            ),
        }
    }
}

/// A token this service never issued is refused rather than read as a cursor: one that
/// is not a token of this service at all, and one shaped like a token but carrying
/// another request's digest.
#[tokio::test]
async fn a_search_token_the_service_never_issued_is_refused() {
    let router = seeded_router();
    let real = token_of(
        "page one",
        &page(&router, "page one", base_search()).await,
    );

    // Not a token of this service in any request: not hex, hex that does not pair up,
    // the hex of a bare fixture id or of a single digit (a cursor a caller would forge
    // to resume "after id 0"), and a real token cut short.
    let hex_of_an_id = hex::encode("bk-1");
    let hex_of_a_digit = hex::encode("0");
    let truncated = real[..38].to_string();
    for token in [
        "not-hex",
        "zzzz",
        "",
        "30",
        hex_of_an_id.as_str(),
        hex_of_a_digit.as_str(),
        truncated.as_str(),
    ] {
        let label = format!("nextToken={token:?}");
        let mut body = base_search();
        body["nextToken"] = json!(token);
        assert_refused(
            &label,
            search(&router, body).await,
            PAGE_TOKEN_MALFORMED_MESSAGE,
        );
    }

    // Token-shaped, with a digest that is no request's: every digit zero, every digit
    // f, and a digest the token's own id was hashed into.
    for digest in ["0".repeat(32), "f".repeat(32), hex::encode("sixteen bytes!!!")] {
        let forged = hex::encode(format!("1:{digest}:bk-1"));
        let label = format!("a token-shaped payload with digest {digest:?}");
        let mut body = base_search();
        body["nextToken"] = json!(forged);
        assert_refused(
            &label,
            search(&router, body).await,
            PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
        );
    }
}

/// `GET /v1/dictionaries` or `/v1/feeds` with one `nextToken`.
async fn list(router: &MultiServiceRouter, region: &str, path: &str, token: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{path}?nextToken={token}"),
        None,
    )
    .await
}

/// `GET /v1/dictionaries` or `/v1/feeds`, one record per page, and the token that page
/// carries.
async fn first_list_token(router: &MultiServiceRouter, region: &str, path: &str) -> String {
    let (status, answered) = call_in_region(
        router,
        region,
        "GET",
        &format!("{path}?maxResults=1"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "page one of {path} in {region} answers 200, got {status} with {answered}"
    );
    token_of(path, &answered)
}

/// Create one dictionary through the wire path.
async fn create_dictionary(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": "eng"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDictionary({name}) answers 202, got {status} with {created}"
    );
}

/// Create one feed through `region`'s endpoint.
async fn create_feed_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, created) =
        call_in_region(router, region, "POST", "/v1/feed", Some(minimal_feed(name))).await;
    assert_eq!(
        status, 202,
        "CreateFeed({name}) in {region} answers 202, got {status} with {created}"
    );
}

/// Each paginated operation honors only its OWN tokens: the token of another operation
/// of the same service names no page of this one, however well-formed it is.
#[tokio::test]
async fn a_token_of_one_paginated_operation_is_not_honored_by_another() {
    let router = seeded_router();
    for index in 0..2 {
        create_feed_in(&router, REGION, &format!("token-feed-{index}")).await;
        create_dictionary(&router, &format!("tokendict{index}")).await;
    }
    past_the_transition_span();

    let feed_token = first_list_token(&router, REGION, "/v1/feeds").await;
    let dictionary_token = first_list_token(&router, REGION, "/v1/dictionaries").await;
    let search_token = token_of(
        "page one",
        &page(&router, "page one", base_search()).await,
    );

    // Each row: whose token it is, and the operation it is submitted to.
    for (owner, token) in [
        ("ListFeeds", &feed_token),
        ("ListDictionaries", &dictionary_token),
        ("SearchFixtures", &search_token),
    ] {
        for path in ["/v1/feeds", "/v1/dictionaries"] {
            if (owner == "ListFeeds") == (path == "/v1/feeds") {
                continue;
            }
            let label = format!("a {owner} token submitted to {path}");
            assert_refused(
                &label,
                list(&router, REGION, path, token).await,
                PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
            );
        }
        if owner != "SearchFixtures" {
            let label = format!("a {owner} token submitted to SearchFixtures");
            let mut body = base_search();
            body["nextToken"] = json!(token);
            assert_refused(
                &label,
                search(&router, body).await,
                PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
            );
        }
    }

    // And each token still continues the listing that issued it.
    for (path, token, member) in [
        ("/v1/feeds", &feed_token, "feeds"),
        ("/v1/dictionaries", &dictionary_token, "dictionaries"),
    ] {
        let (status, answered) = list(&router, REGION, path, token).await;
        assert_eq!(
            status, 200,
            "case {path:?}: its own token is served, got {status} with {answered}"
        );
        assert_eq!(
            answered[member].as_array().map(Vec::len),
            Some(1),
            "case {path:?}: the second page reports the one remaining record, got {answered}"
        );
    }
}

/// A listing's token belongs to the endpoint that issued it: the service is regional, so
/// another Region's endpoint enumerates another set of records and cannot be paged with
/// this one's token.
#[tokio::test]
async fn a_list_token_is_honored_only_by_the_endpoint_that_issued_it() {
    let router = seeded_router();
    for index in 0..2 {
        create_feed_in(&router, REGION, &format!("home-feed-{index}")).await;
        create_feed_in(&router, OTHER_REGION, &format!("away-feed-{index}")).await;
    }
    past_the_transition_span();

    let home_token = first_list_token(&router, REGION, "/v1/feeds").await;
    let away_token = first_list_token(&router, OTHER_REGION, "/v1/feeds").await;

    // Each row: the endpoint a token was issued by, the endpoint it is submitted to,
    // and whether that endpoint may serve it.
    for (issued_by, submitted_to, token) in [
        (REGION, OTHER_REGION, &home_token),
        (OTHER_REGION, REGION, &away_token),
    ] {
        let label = format!("a token of {issued_by} submitted to {submitted_to}");
        assert_refused(
            &label,
            list(&router, submitted_to, "/v1/feeds", token).await,
            PAGE_TOKEN_OTHER_REQUEST_MESSAGE,
        );
    }

    for (region, token) in [(REGION, &home_token), (OTHER_REGION, &away_token)] {
        let (status, answered) = list(&router, region, "/v1/feeds", token).await;
        assert_eq!(
            status, 200,
            "case {region:?}: its own token is served, got {status} with {answered}"
        );
        assert_eq!(
            answered["feeds"].as_array().map(Vec::len),
            Some(1),
            "case {region:?}: the second page reports the one remaining feed of this \
             endpoint, got {answered}"
        );
    }
}
