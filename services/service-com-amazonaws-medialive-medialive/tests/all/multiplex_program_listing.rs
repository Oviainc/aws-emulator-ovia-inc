// SPDX-License-Identifier: CC0-1.0

//! `ListMultiplexPrograms`: which programs the listing reports, in which order,
//! how a page is cut, which multiplex a program belongs to, and that a listed
//! program is the program `DescribeMultiplexProgram` reports — end to end over
//! the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: the recorded case (one `ListMultiplexPrograms` call
//! over the programs a name race created in one multiplex) is ONE row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_LIST_PAGE_SIZE, MAX_MULTIPLEX_PROGRAMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::MULTIPLEXES;
use crate::multiplex_programs::{create_program, describe_program, multiplex, program_settings};
use crate::wire::{call, error_code, error_message, medialive};

/// `ListMultiplexPrograms` on one multiplex, with `query` appended verbatim
/// (`smithy_query(query="operation", operation="ListMultiplexPrograms")`: GET
/// `/prod/multiplexes/{MultiplexId}/programs`, where `MaxResults` and
/// `NextToken` are both `@httpQuery`).
async fn list(router: &MultiServiceRouter, multiplex_id: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{MULTIPLEXES}/{multiplex_id}/programs{query}"),
        None,
    )
    .await
}

/// The `multiplexPrograms` member as a list, whatever its length
/// (`ListMultiplexProgramsResponse$MultiplexPrograms`, jsonName
/// `multiplexPrograms`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["multiplexPrograms"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a multiplexPrograms list, got {body}"))
}

/// A program created in `multiplex_id`, asserted to have succeeded.
async fn stored_program(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    program_name: &str,
    program_number: i64,
) {
    let (status, program) = create_program(
        router,
        multiplex_id,
        program_name,
        &format!("token-{multiplex_id}-{program_name}"),
        program_settings(program_number),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplexProgram({program_name}): expected 201, \
         got {status}: {program}"
    );
}

/// Walk every page of the listing, answering the program names reported in
/// order and the size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    page_size: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut names: Vec<String> = Vec::new();
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

        let (status, body) = list(router, multiplex_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListMultiplexPrograms to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} programs in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for program in &page {
            names.push(
                program["programName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
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
    (names, pages)
}

/// One row of the pagination table: how many programs the multiplex holds and
/// the page size the request asks for.
struct Paging {
    case: &'static str,
    programs: usize,
    page_size: Option<i32>,
}

/// Rule: a paginated list operation reports every member of the collection the
/// request names, each exactly once, in the order the collection keys them,
/// cut into pages of at most the requested page size bounded by the service's
/// own ceiling, whose token resumes at the first member not yet reported and is
/// absent on the last page.
///
/// A multiplex keys its programs by `ProgramName` ("The name of the multiplex
/// program", the identifier `DescribeMultiplexProgram`'s
/// `/programs/{ProgramName}` label names one by), so the order is name order —
/// which is why every row here CREATES its programs in the reverse of that
/// order: a listing that reported creation order would pass nothing below.
///
/// The rows differ along the COUNT of stored programs and the page size asked
/// for — none, one, a size that divides the collection unevenly, no size at all,
/// a size past the service's ceiling, and more programs than one default page
/// holds — because that is what the rule generalises over.
#[tokio::test]
async fn list_multiplex_programs_pages_through_every_program_in_name_order() {
    let table = [
        Paging {
            case: "no programs at all",
            programs: 0,
            page_size: None,
        },
        Paging {
            case: "no programs, page of one",
            programs: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one program, page of one",
            programs: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three programs, page of one",
            programs: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five programs, page of two",
            programs: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four programs, default page size",
            programs: 4,
            page_size: None,
        },
        Paging {
            case: "three programs, page size past the ceiling",
            programs: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // More programs than a single default page holds, so the default
            // page size is itself a cut and not just a ceiling nobody reaches.
            case: "more programs than one default page holds",
            programs: MAX_LIST_PAGE_SIZE + 7,
            page_size: None,
        },
    ];

    for Paging {
        case,
        programs,
        page_size,
    } in table
    {
        assert!(
            programs <= MAX_MULTIPLEX_PROGRAMS,
            "case {case:?}: the row asks for {programs} programs, past the per-multiplex \
             bound {MAX_MULTIPLEX_PROGRAMS}"
        );
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "paged").await;

        // Zero-padded, so name order is also the index order the expectation
        // below is written in — and created from the last name to the first.
        let names: Vec<String> = (0..programs)
            .map(|index| format!("program-{index:03}"))
            .collect();
        for (index, name) in names.iter().enumerate().rev() {
            stored_program(&router, case, &multiplex_id, name, index as i64 + 1).await;
        }

        let (reported, pages) = every_page(&router, case, &multiplex_id, page_size).await;
        assert_eq!(
            reported, names,
            "case {case:?}: expected the listing to report every stored program once, \
             in ProgramName order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if programs == 0 {
            1
        } else {
            programs.div_ceil(size)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {programs} programs of {size}, \
             got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                programs - size * index
            } else {
                size
            };
            assert_eq!(
                *page, expected,
                "case {case:?}: expected page {index} to carry {expected} programs, \
                 got {pages:?}"
            );
        }
    }
}

/// One row of the projection table: the program's name, the settings the create
/// supplies, and whether a channel's output destination names the program.
struct Stored {
    case: &'static str,
    program_name: &'static str,
    settings: fn() -> Value,
    attach_channel: bool,
}

/// Rule: a listed resource is the stored resource read through the list shape,
/// so every member it shares with the resource's own describe reports the same
/// value that describe reports for it.
///
/// `MultiplexProgramSummary` carries `ChannelId` and `ProgramName`
/// (`smithy_query(query="shape", shape="MultiplexProgramSummary")`), both of
/// which `MultiplexProgram` carries too, so the assertion walks both. The rows
/// differ along WHICH optional settings the stored program carries — none of
/// which the summary reports — and along whether a channel's destination names
/// the program, because the derived `ChannelId` is what a listing could most
/// easily get wrong.
#[tokio::test]
async fn a_listed_program_is_the_program_describe_reports() {
    // Every member the two shapes share, from the `smithy_query` above.
    const MEMBERS: [&str; 2] = ["channelId", "programName"];

    let table = [
        Stored {
            case: "the required member only",
            program_name: "news",
            settings: || program_settings(1),
            attach_channel: false,
        },
        Stored {
            case: "a service descriptor",
            program_name: "sports",
            settings: || {
                json!({
                    "programNumber": 2,
                    "serviceDescriptor": {
                        "providerName": "a-provider",
                        "serviceName": "a-service",
                    },
                })
            },
            attach_channel: false,
        },
        Stored {
            case: "statmux video settings",
            program_name: "statmux",
            settings: || {
                json!({
                    "programNumber": 3,
                    "videoSettings": {
                        "statmuxSettings": {
                            "maximumBitrate": 10_000_000,
                            "minimumBitrate": 1_000_000,
                            "priority": 2,
                        }
                    },
                })
            },
            attach_channel: false,
        },
        Stored {
            case: "fed by a channel",
            program_name: "fed",
            settings: || json!({ "programNumber": 4, "preferredChannelPipeline": "PIPELINE_1" }),
            attach_channel: true,
        },
        Stored {
            case: "fed by a channel, constant bitrate",
            program_name: "fed-cbr",
            settings: || {
                json!({
                    "programNumber": 5,
                    "videoSettings": { "constantBitrate": 5_000_000 },
                })
            },
            attach_channel: true,
        },
    ];

    for Stored {
        case,
        program_name,
        settings,
        attach_channel,
    } in table
    {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "projected").await;
        let (status, created) = create_program(
            &router,
            &multiplex_id,
            program_name,
            &format!("token-{program_name}"),
            settings(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {created}"
        );

        let mut channel_id = Value::Null;
        if attach_channel {
            let (status, response) = call(
                &router,
                "POST",
                "/prod/channels",
                Some(json!({
                    "name": "feeds-the-program",
                    "destinations": [{
                        "id": "multiplex-out",
                        "multiplexSettings": {
                            "multiplexId": multiplex_id.clone(),
                            "programName": program_name,
                        },
                    }],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateChannel: expected 201, got {status}: {response}"
            );
            channel_id = response["channel"]["id"].clone();
        }

        let (status, body) = list(&router, &multiplex_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored program, got {body}"
        );
        let summary = &page[0];
        let (status, described) = describe_program(&router, &multiplex_id, program_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {described}"
        );

        for member in MEMBERS {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 DescribeMultiplexProgram reports, got {:?} vs {:?}",
                summary.get(member),
                described.get(member)
            );
        }
        assert_eq!(
            summary["programName"],
            json!(program_name),
            "case {case:?}: expected the created program's name, got {:?}",
            summary["programName"]
        );
        // The derived member the rows vary over: the channel whose destination
        // names this program, absent while no channel does.
        assert_eq!(
            summary["channelId"], channel_id,
            "case {case:?}: expected the listed channelId to be {channel_id:?}, got {:?}",
            summary["channelId"]
        );
        // The summary is the SUMMARY shape: it carries those two members and
        // not the settings the full description reports.
        assert_eq!(
            summary["multiplexProgramSettings"],
            Value::Null,
            "case {case:?}: expected no settings on a summary, got {:?}",
            summary["multiplexProgramSettings"]
        );
    }
}

/// Rule: a listing scoped to a parent resource reports the members of THAT
/// parent only, so a collection the parent holds nothing in is an empty page
/// while an identifier no parent has is the operation's modelled 404.
///
/// The rows differ along how the programs are spread over two multiplexes —
/// all in the first, all in the second, split between them, none at all —
/// because mixing one parent's members into another's is exactly what a listing
/// keyed on the wrong thing would do.
#[tokio::test]
async fn programs_are_listed_only_under_the_multiplex_that_holds_them() {
    // (case, the names created in the first multiplex, in the second)
    let table: [(&str, &[&str], &[&str]); 5] = [
        ("neither multiplex holds a program", &[], &[]),
        ("only the first holds programs", &["alpha", "beta"], &[]),
        ("only the second holds programs", &[], &["gamma"]),
        (
            "both hold programs, one name shared",
            &["alpha", "shared"],
            &["shared", "zulu"],
        ),
        (
            "the second holds more than the first",
            &["only"],
            &["one", "three", "two"],
        ),
    ];

    for (case, first_names, second_names) in table {
        let router = medialive();
        let first = multiplex(&router, case, "first").await;
        let second = multiplex(&router, case, "second").await;
        assert_ne!(
            first, second,
            "case {case:?}: expected two distinct multiplexes, got {first} twice"
        );

        for (multiplex_id, names) in [(&first, first_names), (&second, second_names)] {
            for (index, name) in names.iter().enumerate() {
                stored_program(&router, case, multiplex_id, name, index as i64 + 1).await;
            }
        }

        for (multiplex_id, names) in [(&first, first_names), (&second, second_names)] {
            let mut expected: Vec<String> = names.iter().map(ToString::to_string).collect();
            expected.sort();
            let (reported, pages) = every_page(&router, case, multiplex_id, Some(1)).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected multiplex {multiplex_id} to list only its own \
                 programs; pages were {pages:?}"
            );
        }
    }

    // An identifier no multiplex has is the modelled `NotFoundException` (404),
    // which is what distinguishes it from a multiplex holding no program.
    let router = medialive();
    let held = multiplex(&router, "a stored multiplex", "stored").await;
    for unknown in ["6999999", "0", "not-an-id"] {
        let (status, body) = list(&router, unknown, "").await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected an unknown multiplex id to be refused 404, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(unknown),
            "case {unknown:?}: expected the refusal to name the identifier the request \
             carried, got {:?}",
            error_message(&body)
        );
    }
    // And the multiplex that does exist answers an empty page, not a refusal.
    let (status, body) = list(&router, &held, "").await;
    assert_eq!(
        status, 200,
        "expected a multiplex holding no program to answer 200, got {status}: {body}"
    );
    assert!(
        listed("no programs", &body).is_empty(),
        "expected an empty page for a multiplex holding no program, got {body}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "expected no nextToken beside an empty page, got {body}"
    );
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// repeating it over the same multiplex answers the same programs, and the
/// multiplex's own derived `ProgramCount` is unmoved.
///
/// The rows differ along how many programs the multiplex holds and how the
/// listing is paged, because a token-resumed walk is where a reporting call
/// would most easily consume state.
#[tokio::test]
async fn listing_programs_changes_nothing_a_later_caller_sees() {
    for (case, programs, page_size) in [
        ("one program, unpaged", 1usize, None),
        ("four programs, page of one", 4, Some(1)),
        ("four programs, page of three", 4, Some(3)),
    ] {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "reported").await;
        let names: Vec<String> = (0..programs)
            .map(|index| format!("program-{index:03}"))
            .collect();
        for (index, name) in names.iter().enumerate() {
            stored_program(&router, case, &multiplex_id, name, index as i64 + 1).await;
        }

        let (first, _) = every_page(&router, case, &multiplex_id, page_size).await;
        let (second, _) = every_page(&router, case, &multiplex_id, page_size).await;
        assert_eq!(
            first, names,
            "case {case:?}: expected the first walk to report every stored program"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to report the same programs, \
             got {second:?} after {first:?}"
        );

        // And each program is still readable, and still counted.
        for name in &names {
            let (status, described) = describe_program(&router, &multiplex_id, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name} to still be readable after two listings, \
                 got {status}: {described}"
            );
        }
        let (status, multiplex) = call(
            &router,
            "GET",
            &format!("{MULTIPLEXES}/{multiplex_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {multiplex}"
        );
        assert_eq!(
            multiplex["programCount"],
            json!(programs),
            "case {case:?}: expected the multiplex to still count {programs} programs, got {:?}",
            multiplex["programCount"]
        );
    }
}

/// Rule: a page size that asks for no results at all is not a page size, so the
/// request is refused with this operation's modelled `BadRequestException`
/// (400) — one of the seven errors
/// `smithy_query(query="operation", operation="ListMultiplexPrograms")`
/// declares for it.
///
/// The rows differ along the non-positive size asked for, and the smallest
/// admissible size (1) is listed beside them as the published bound that must
/// still succeed.
#[tokio::test]
async fn a_program_page_size_that_asks_for_nothing_is_refused() {
    let router = medialive();
    let multiplex_id = multiplex(&router, "a stored multiplex", "refusing").await;
    stored_program(&router, "a stored program", &multiplex_id, "news", 1).await;

    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &multiplex_id, &format!("?maxResults={max_results}")).await;
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
    let (status, body) = list(&router, &multiplex_id, "?maxResults=1").await;
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
