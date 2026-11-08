// SPDX-License-Identifier: CC0-1.0

//! `ListMultiplexes`: which multiplexes the listing reports, in which order, how
//! a page is cut, and that a listed multiplex is the multiplex
//! `DescribeMultiplex` reports — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: one `ListMultiplexes` call over an account holding a
//! handful of multiplexes is ONE row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{MAX_LIST_PAGE_SIZE, MAX_MULTIPLEXES};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{
    create_body, created, describe_multiplex, settings, MULTIPLEXES, TRANSPORT_STREAM_BITRATE,
    TRANSPORT_STREAM_ID, TRANSPORT_STREAM_RESERVED_BITRATE, VIDEO_BUFFER_DELAY,
};
use crate::multiplex_start::{add_programs, start_multiplex, ZONES};
use crate::wire::{call, error_code, medialive};

/// `ListMultiplexes`, with `query` appended verbatim
/// (`smithy_query(query="operation", operation="ListMultiplexes")`: GET
/// `/prod/multiplexes`, where `MaxResults` and `NextToken` are both
/// `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{MULTIPLEXES}{query}"), None).await
}

/// The `multiplexes` member as a list, whatever its length
/// (`ListMultiplexesResponse$Multiplexes`, jsonName `multiplexes`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["multiplexes"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a multiplexes list, got {body}"))
}

/// A multiplex created under `name` with the two required settings members, as
/// its minted id, asserting the create succeeded.
async fn stored_multiplex(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    multiplex_settings: Value,
) -> String {
    let multiplex = created(
        router,
        case,
        create_body(name, &format!("token-{name}"), multiplex_settings),
    )
    .await;
    multiplex["id"].as_str().unwrap_or_default().to_string()
}

/// Walk every page of the listing, answering the multiplex ids reported in
/// order and the size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = String::new();
        if let Some(max_results) = page_size {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("maxResults={max_results}"));
        }
        if let Some(next) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={next}"));
        }

        let (status, body) = list(router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListMultiplexes to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} multiplexes in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for multiplex in &page {
            ids.push(multiplex["id"].as_str().unwrap_or_default().to_string());
        }

        token = body["nextToken"].as_str().map(ToString::to_string);
        // A token is handed out only when something is left over, so an empty
        // page can never carry one.
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        if token.is_none() {
            break;
        }
        assert!(
            pages.len() <= 128,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// A collection larger than the page size a client would ask for by hand, and
/// within the store's own bound (`MAX_MULTIPLEXES`): what a row uses to assert
/// that the service never cuts a page below the `MaxResults` the request asked
/// for.
const A_LARGE_COLLECTION: usize = 57;

/// One row of the pagination table: how many multiplexes the account holds and
/// the page size the request asks for.
struct Paging {
    case: &'static str,
    multiplexes: usize,
    page_size: Option<i32>,
}

/// Rule: a paginated list operation reports every member of the collection it is
/// scoped to, each exactly once, in the order the collection keys them, cut into
/// pages of at most the requested page size bounded by the service's own
/// ceiling, whose token resumes at the first member not yet reported and is
/// absent on the last page.
///
/// The store keys multiplexes by the `Id` it mints ("The unique id of the
/// multiplex", the label `DescribeMultiplex`'s `/multiplexes/{MultiplexId}`
/// names one by), and mints them in ascending order, so the reported order is
/// also the creation order asserted below.
///
/// The rows differ along the COUNT of stored multiplexes and the page size asked
/// for — none, one, a size that divides the collection unevenly, no size at all,
/// a size past the service's ceiling, and more multiplexes than a hand-written
/// page size would hold — because that is what the rule generalises over.
#[tokio::test]
async fn list_multiplexes_pages_through_every_multiplex_in_id_order() {
    let table = [
        Paging {
            case: "no multiplexes at all",
            multiplexes: 0,
            page_size: None,
        },
        Paging {
            case: "no multiplexes, page of one",
            multiplexes: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one multiplex, page of one",
            multiplexes: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three multiplexes, page of one",
            multiplexes: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five multiplexes, page of two",
            multiplexes: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four multiplexes, default page size",
            multiplexes: 4,
            page_size: None,
        },
        Paging {
            case: "three multiplexes, page size past the ceiling",
            multiplexes: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // A collection far larger than any page a caller would ask for by
            // hand: the service's ceiling is the published `MaxResults` bound,
            // well above what the store can hold, so one page carries them all.
            case: "a large collection, no page size",
            multiplexes: A_LARGE_COLLECTION,
            page_size: None,
        },
    ];

    for Paging {
        case,
        multiplexes,
        page_size,
    } in table
    {
        assert!(
            multiplexes <= MAX_MULTIPLEXES,
            "case {case:?}: the row asks for {multiplexes} multiplexes, past the store's \
             bound {MAX_MULTIPLEXES}"
        );
        let router = medialive();
        let mut minted: Vec<String> = Vec::new();
        for index in 0..multiplexes {
            minted.push(
                stored_multiplex(
                    &router,
                    case,
                    &format!("multiplex-{index:03}"),
                    settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
                )
                .await,
            );
        }

        let (reported, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            reported, minted,
            "case {case:?}: expected the listing to report every stored multiplex once, \
             in minted-id order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if multiplexes == 0 {
            1
        } else {
            multiplexes.div_ceil(size)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {multiplexes} multiplexes \
             of {size}, got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                multiplexes - size * index
            } else {
                size
            };
            assert_eq!(
                *page, expected,
                "case {case:?}: expected page {index} to carry {expected} multiplexes, \
                 got {pages:?}"
            );
        }
    }
}

/// Where in its lifecycle the row's multiplex is driven before it is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    /// Left as created, then settled: the stopped `IDLE` multiplex.
    Idle,
    /// Started and settled: `RUNNING`, with one pipeline healthy per zone.
    Running,
}

/// One row of the projection table: the multiplex's name, the settings the
/// create supplies, the tags it carries, how many programs it holds and where in
/// its lifecycle it is listed.
struct Stored {
    case: &'static str,
    name: &'static str,
    multiplex_settings: fn() -> Value,
    tags: Option<fn() -> Value>,
    programs: i64,
    lifecycle: Lifecycle,
}

/// Rule: a listed resource is the stored resource read through the list shape,
/// so every member it shares with the resource's own describe reports the same
/// value that describe reports for it, and a member the list shape narrows is
/// the narrowed projection of the same stored value.
///
/// `MultiplexSummary` shares eight members with `Multiplex` and narrows
/// `MultiplexSettings` to a `MultiplexSettingsSummary` carrying
/// `TransportStreamBitrate` alone (`smithy_query(query="shape",
/// shape="MultiplexSummary,MultiplexSettingsSummary")`), so the assertion walks
/// the eight and then that one projection. The rows differ along which optional
/// settings members the multiplex carries and their values at the published
/// bounds, along whether it is tagged, along how many programs it holds (the
/// derived `ProgramCount`) and along its lifecycle state (which the derived
/// `PipelinesRunningCount` follows) — the members a listing could most easily
/// get wrong.
#[tokio::test]
async fn a_listed_multiplex_is_the_multiplex_describe_reports() {
    // Every member the two shapes share, from the `smithy_query` above.
    const MEMBERS: [&str; 8] = [
        "arn",
        "availabilityZones",
        "id",
        "name",
        "pipelinesRunningCount",
        "programCount",
        "state",
        "tags",
    ];

    let table = [
        Stored {
            case: "the required settings only",
            name: "required-only",
            multiplex_settings: || settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
            tags: None,
            programs: 0,
            lifecycle: Lifecycle::Idle,
        },
        Stored {
            case: "every settings member at its upper bound",
            name: "upper-bounds",
            multiplex_settings: || {
                json!({
                    "transportStreamBitrate": TRANSPORT_STREAM_BITRATE.1,
                    "transportStreamId": TRANSPORT_STREAM_ID.1,
                    "maximumVideoBufferDelayMilliseconds": VIDEO_BUFFER_DELAY.1,
                    "transportStreamReservedBitrate": TRANSPORT_STREAM_RESERVED_BITRATE.1,
                })
            },
            tags: None,
            programs: 0,
            lifecycle: Lifecycle::Idle,
        },
        Stored {
            case: "tagged, holding programs",
            name: "tagged-with-programs",
            multiplex_settings: || settings(3_000_000, 42),
            tags: Some(|| json!({ "env": "test", "team": "playout" })),
            programs: 3,
            lifecycle: Lifecycle::Idle,
        },
        Stored {
            case: "running, holding one program",
            name: "running",
            multiplex_settings: || settings(20_000_000, 7),
            tags: None,
            programs: 1,
            lifecycle: Lifecycle::Running,
        },
    ];

    for Stored {
        case,
        name,
        multiplex_settings,
        tags,
        programs,
        lifecycle,
    } in table
    {
        let router = medialive();
        let mut request = create_body(
            name,
            &format!("token-{name}"),
            multiplex_settings(),
        );
        if let Some(tags) = tags {
            request["tags"] = tags();
        }
        let multiplex = created(&router, case, request).await;
        let id = multiplex["id"].as_str().unwrap_or_default().to_string();
        add_programs(&router, case, &id, programs).await;

        // The create answers `CREATING`, so every row waits for the clock the
        // creation owes before it drives the multiplex any further.
        wait_for_the_clock();
        match lifecycle {
            Lifecycle::Idle => {}
            Lifecycle::Running => {
                let (status, started) = start_multiplex(&router, &id).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
                );
                wait_for_the_clock();
            }
        }

        let (status, body) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored multiplex, got {body}"
        );
        let summary = &page[0];
        let (status, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
        );

        for member in MEMBERS {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 DescribeMultiplex reports, got {:?} vs {:?}",
                summary.get(member),
                described.get(member)
            );
        }
        // The lifecycle the row drove the multiplex to, and the pipeline count
        // that follows it: one healthy pipeline per availability zone exactly
        // while the multiplex is RUNNING.
        let expected_state = match lifecycle {
            Lifecycle::Idle => "IDLE",
            Lifecycle::Running => "RUNNING",
        };
        assert_eq!(
            summary["state"],
            json!(expected_state),
            "case {case:?}: expected the listed state to be {expected_state}, got {:?}",
            summary["state"]
        );
        assert_eq!(
            summary["pipelinesRunningCount"],
            json!(if lifecycle == Lifecycle::Running { ZONES } else { 0 }),
            "case {case:?}: expected the pipeline count a {expected_state} multiplex runs, \
             got {:?}",
            summary["pipelinesRunningCount"]
        );
        assert_eq!(
            summary["programCount"],
            json!(programs),
            "case {case:?}: expected the listing to count the {programs} stored program(s), \
             got {:?}",
            summary["programCount"]
        );
        // The narrowed member: the summary's settings carry the stored
        // `TransportStreamBitrate` and nothing else of the full settings.
        assert_eq!(
            summary["multiplexSettings"],
            json!({
                "transportStreamBitrate": described["multiplexSettings"]["transportStreamBitrate"],
            }),
            "case {case:?}: expected the summary settings to be the TransportStreamBitrate \
             projection of {:?}, got {:?}",
            described["multiplexSettings"],
            summary["multiplexSettings"]
        );
        // And the summary shape carries no `Destinations`, which only the full
        // description models.
        assert_eq!(
            summary["destinations"],
            Value::Null,
            "case {case:?}: expected no destinations on a summary, got {:?}",
            summary["destinations"]
        );
    }
}

/// Rule: a listing reports the multiplexes the account currently holds, so one
/// whose delete has been ACCEPTED leaves it from that call on — while it is
/// being torn down (`DELETING`) and in the terminal `DELETED` state alike —
/// while that multiplex's own `DescribeMultiplex` goes on reporting the state
/// the teardown is in, which is what the SDK's `MultiplexDeleted` waiter polls
/// for.
///
/// That the listing drops it before the teardown finishes is recorded: the
/// `medialive.multiplex-start-without-programs-rejected` fixture finds the
/// multiplex already absent from `ListMultiplexes` while a `StartMultiplex`
/// sent straight afterwards is still refused on the `DELETING` state. The same
/// split is what the input security group's listing answers with on its own
/// recorded evidence (`crate::input_security_group_listing`).
///
/// The rows differ along WHICH of the five stored multiplexes are deleted — the
/// first, one in the middle, the last, several, all of them, none — because the
/// delete's position in mint order is what a paged listing could get wrong, and
/// each row is walked unpaged, one multiplex at a time and two at a time, in
/// both phases of the teardown.
#[tokio::test]
async fn a_deleted_multiplex_leaves_the_listing_but_is_still_described() {
    // (case, the indices of the five stored multiplexes the row deletes)
    let table: [(&str, &[usize]); 6] = [
        ("nothing deleted", &[]),
        ("the first multiplex deleted", &[0]),
        ("a middle multiplex deleted", &[2]),
        ("the last multiplex deleted", &[4]),
        ("two multiplexes deleted", &[1, 3]),
        ("every multiplex deleted", &[0, 1, 2, 3, 4]),
    ];

    for (case, deleted_indices) in table {
        let router = medialive();
        let mut created: Vec<String> = Vec::new();
        for index in 0..5 {
            created.push(
                stored_multiplex(
                    &router,
                    case,
                    &format!("stored-{index}"),
                    settings(TRANSPORT_STREAM_BITRATE.0, index),
                )
                .await,
            );
        }
        // A multiplex is deleted from the stopped state the create settles into.
        wait_for_the_clock();
        for index in deleted_indices {
            let (status, deleting) = call(
                &router,
                "DELETE",
                &format!("{MULTIPLEXES}/{}", created[*index]),
                None,
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
            );
        }

        let survivors: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| !deleted_indices.contains(index))
            .map(|(_, id)| id.clone())
            .collect();

        // Both phases of the teardown: the transient one the accepted delete
        // opens, and the terminal one the clock then makes due.
        for phase in ["DELETING", "DELETED"] {
            if phase == "DELETED" {
                wait_for_the_clock();
            }
            for page_size in [None, Some(1), Some(2)] {
                let (reported, pages) = every_page(&router, case, page_size).await;
                assert_eq!(
                    reported, survivors,
                    "case {case:?}: with page size {page_size:?} while the deleted ones are \
                     {phase}, expected the listing to report only the multiplexes still in the \
                     account, in mint order; pages were {pages:?}"
                );
            }

            // The deleted multiplexes are still READABLE, in the state the
            // teardown is in — the listing dropped them, the store did not.
            for index in deleted_indices {
                let (status, described) = describe_multiplex(&router, &created[*index]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
                );
                assert_eq!(
                    described["state"],
                    json!(phase),
                    "case {case:?}: expected the deleted multiplex to still describe as {phase}, \
                     got {:?}",
                    described["state"]
                );
            }
        }
    }
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// repeating it answers the same multiplexes, each still readable through its
/// own describe, with the lifecycle state the clock left it in and nothing
/// consumed.
///
/// The rows differ along how many multiplexes the account holds and how the
/// listing is paged, because a token-resumed walk is where a reporting call
/// would most easily consume state.
#[tokio::test]
async fn listing_multiplexes_changes_nothing_a_later_caller_sees() {
    for (case, multiplexes, page_size) in [
        ("one multiplex, unpaged", 1usize, None),
        ("four multiplexes, page of one", 4, Some(1)),
        ("four multiplexes, page of three", 4, Some(3)),
    ] {
        let router = medialive();
        let mut minted: Vec<String> = Vec::new();
        for index in 0..multiplexes {
            minted.push(
                stored_multiplex(
                    &router,
                    case,
                    &format!("reported-{index:03}"),
                    settings(TRANSPORT_STREAM_BITRATE.0 + index as i64, index as i64),
                )
                .await,
            );
        }
        // Past the creation the clock owes, so the state the two walks report
        // cannot differ just because a transition came due between them.
        wait_for_the_clock();

        let (first, _) = every_page(&router, case, page_size).await;
        let (second, _) = every_page(&router, case, page_size).await;
        assert_eq!(
            first, minted,
            "case {case:?}: expected the first walk to report every stored multiplex"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to report the same multiplexes, \
             got {second:?} after {first:?}"
        );

        // And each multiplex is still readable, still IDLE and still its own.
        for (index, id) in minted.iter().enumerate() {
            let (status, described) = describe_multiplex(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {id} to still be readable after two listings, \
                 got {status}: {described}"
            );
            assert_eq!(
                described["state"],
                json!("IDLE"),
                "case {case:?}: expected {id} to still be IDLE, got {:?}",
                described["state"]
            );
            assert_eq!(
                described["name"],
                json!(format!("reported-{index:03}")),
                "case {case:?}: expected {id} to carry its own name, got {:?}",
                described["name"]
            );
        }
    }
}

/// Rule: a page size that asks for no results at all is not a page size, so the
/// request is refused with this operation's modelled `BadRequestException`
/// (400) — one of the six errors
/// `smithy_query(query="operation", operation="ListMultiplexes")` declares for
/// it.
///
/// The rows differ along the non-positive size asked for, and the smallest
/// admissible size (1) is listed beside them as the published bound that must
/// still succeed.
#[tokio::test]
async fn a_multiplex_page_size_that_asks_for_nothing_is_refused() {
    let router = medialive();
    for name in ["first", "second"] {
        stored_multiplex(
            &router,
            "a stored multiplex",
            name,
            settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
        )
        .await;
    }

    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {max_results}: expected a non-positive maxResults to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {max_results}: expected BadRequestException, got {body}"
        );
    }

    // The bound itself: one result per page is a page size, and succeeds.
    let (status, body) = list(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case 1: expected the smallest admissible page size to succeed, got {status}: {body}"
    );
    assert_eq!(
        listed("case 1", &body).len(),
        1,
        "case 1: expected a page of one, got {body}"
    );
}

/// Rule: a listing reports the resources of the collection it is bound to and
/// nothing else, so another resource type's records never appear in it and a
/// token naming a record outside the collection resumes the scan rather than
/// inventing one.
///
/// The rows differ along the token handed back in: none, an id below every
/// minted multiplex id, one between two of them, one past all of them, and a
/// string that is not an id at all.
#[tokio::test]
async fn a_multiplex_listing_reports_only_multiplexes() {
    let router = medialive();
    // A channel exists too, so a listing keyed on the wrong store would report
    // it.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "not-a-multiplex",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {channel}"
    );

    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        minted.push(
            stored_multiplex(
                &router,
                "only multiplexes",
                &format!("listed-{index}"),
                settings(TRANSPORT_STREAM_BITRATE.0, index),
            )
            .await,
        );
    }

    // (case, the nextToken the request carries, the ids expected back)
    //
    // The two tokens that are not ids are DERIVED from the minted ones rather
    // than written out, so the rows keep meaning what they say if the id format
    // changes: a suffix on an id sorts immediately after it (and before the next
    // id, whose differing digit decides the comparison first), and a suffix on
    // the LAST id sorts after every id the store holds.
    let last = minted.last().expect("a minted multiplex id").clone();
    let rows: [(&str, Option<String>, Vec<String>); 5] = [
        ("no token", None, minted.clone()),
        (
            "a token below every minted id",
            Some("0".to_string()),
            minted.clone(),
        ),
        (
            "a token between two minted ids",
            Some(format!("{}x", minted[0])),
            minted[1..].to_vec(),
        ),
        (
            "a token past every minted id",
            Some(format!("{last}x")),
            Vec::new(),
        ),
        (
            "a token that is not an id at all",
            Some("not-an-id".to_string()),
            Vec::new(),
        ),
    ];

    for (case, token, expected) in rows {
        let query = token.map_or_else(String::new, |token| format!("?nextToken={token}"));
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let reported: Vec<String> = listed(case, &body)
            .iter()
            .map(|multiplex| multiplex["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the multiplexes at or after the token, got {reported:?}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {case:?}: expected no nextToken when nothing is left over, got {body}"
        );
    }
}
