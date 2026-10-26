// SPDX-License-Identifier: CC0-1.0

//! `ListInputs`: which inputs the listing reports, in which order, how a page is
//! cut, and that a listed input is the input `DescribeInput` reports — end to end
//! over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: one `ListInputs` call over one stored input is ONE row
//! of them. The wire harness (`medialive`, `call`, `error_code`), the input setup
//! (`resolved_input_body`) and the clock wait are the crate's shared ones —
//! reused, not copied.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{MAX_INPUTS, MAX_LIST_PAGE_SIZE};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::resolved_input_body;
use crate::wire::{call, error_code, medialive};

/// The collection URI `ListInputs` and `CreateInput` are bound to
/// (`smithy_query(query="operation", operation="ListInputs,CreateInput")`: GET
/// and POST /prod/inputs).
const INPUTS: &str = "/prod/inputs";

/// Every member `Input` and `DescribeInputResponse` share, so a listed input can
/// be compared with the describe of the same input member by member
/// (`smithy_query(query="shape", shape="Input,DescribeInputResponse")`: the two
/// shapes carry the same twenty-two members, by jsonName).
const MEMBERS: [&str; 22] = [
    "arn",
    "attachedChannels",
    "destinations",
    "id",
    "inputClass",
    "inputDevices",
    "inputPartnerIds",
    "inputSourceType",
    "mediaConnectFlows",
    "name",
    "roleArn",
    "securityGroups",
    "sources",
    "state",
    "tags",
    "type",
    "srtSettings",
    "inputNetworkLocation",
    "multicastSettings",
    "smpte2110ReceiverGroupSettings",
    "sdiSources",
    "routerSettings",
];

/// `ListInputs` with `query` appended verbatim (both input members are
/// `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{INPUTS}{query}"), None).await
}

/// The `inputs` member as a list, whatever its length
/// (`ListInputsResponse$Inputs`, jsonName `inputs`). An empty list and an absent
/// member both mean "no entries" to a client, so both read here as the empty
/// list.
fn listed(case: &str, body: &Value) -> Vec<Value> {
    match body.get("inputs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected an inputs list, got {other}"),
    }
}

/// `CreateInput` with `body`, pointed at resources this account really holds,
/// answering the created input.
async fn create_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", INPUTS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateInput to answer 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// The id of one created input.
async fn create_input_id(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    create_input(router, case, body).await["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DescribeInput` on one input, asserting the 200 a stored input answers with.
async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("{INPUTS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeInput({id}) to answer 200, got {status}: {described}"
    );
    described
}

/// `DeleteInput` on one input, asserting the 200 the delete of a stored input
/// answers with.
async fn delete_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, deleted) = call(router, "DELETE", &format!("{INPUTS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteInput({id}) to answer 200, got {status}: {deleted}"
    );
    deleted
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// A `CreateInput` body for the `index`th input of a collection: a pull input
/// with the redundant pair of source URLs such an input ingests through.
fn pull_body(index: usize) -> Value {
    json!({
        "name": format!("listed-{index}"),
        "type": "URL_PULL",
        "sources": [
            source(&format!("https://example.com/{index}/a.m3u8")),
            source(&format!("https://example.com/{index}/b.m3u8")),
        ],
    })
}

/// Walk every page of the listing, answering the ids reported in order and the
/// size of each page.
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
            "case {case:?}: expected ListInputs to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} inputs in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for input in &page {
            ids.push(input["id"].as_str().unwrap_or_default().to_string());
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
            pages.len() <= 64,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// A collection larger than a page size a client would ask for by hand, and
/// within the store's own bound (`MAX_INPUTS`): what a row uses to assert that
/// the service never cuts a page below the `MaxResults` the request asked for.
const A_LARGE_COLLECTION: usize = 37;

/// One row of the pagination table: how many inputs the store holds and the page
/// size the request asks for.
struct Paging {
    case: &'static str,
    inputs: usize,
    page_size: Option<i32>,
}

/// Rule: a paginated list operation reports every resource of its type this
/// service's store holds, each exactly once, in the order their ids were minted,
/// cut into pages of at most the requested page size bounded by the service's own
/// ceiling, whose token resumes at the first resource not yet reported and is
/// absent on the last page.
///
/// The rows differ along the COUNT of stored inputs and the page size asked for —
/// none stored, one, a size that divides the store unevenly, no size at all, a
/// size past the service's ceiling, and more inputs than a hand-written page size
/// would hold — because that is what the rule generalises over.
#[tokio::test]
async fn list_inputs_pages_through_every_stored_input_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            inputs: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            inputs: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one input, page of one",
            inputs: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three inputs, page of one",
            inputs: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five inputs, page of two",
            inputs: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four inputs, default page size",
            inputs: 4,
            page_size: None,
        },
        Paging {
            case: "three inputs, page size past the ceiling",
            inputs: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // A collection far larger than any page a caller would ask for by
            // hand: the service's ceiling is the published `MaxResults` bound,
            // well above what the store can hold, so one page carries them all.
            case: "a large collection, no page size",
            inputs: A_LARGE_COLLECTION,
            page_size: None,
        },
    ];

    for Paging {
        case,
        inputs,
        page_size,
    } in table
    {
        assert!(
            inputs <= MAX_INPUTS,
            "case {case:?}: the row asks for {inputs} inputs, past the store bound {MAX_INPUTS}"
        );
        let router = medialive();

        let mut created: Vec<String> = Vec::new();
        for index in 0..inputs {
            created.push(create_input_id(&router, case, pull_body(index)).await);
        }

        let (reported, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            reported, created,
            "case {case:?}: expected the listing to report every stored input once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if inputs == 0 { 1 } else { inputs.div_ceil(size) };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {inputs} inputs of {size}, \
             got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                inputs - size * index
            } else {
                size
            };
            assert_eq!(
                *page, expected,
                "case {case:?}: expected page {index} to carry {expected} inputs, got {pages:?}"
            );
        }
    }
}

/// One row of the projection table: what the create supplies, and whether a
/// channel attaches the created input.
struct Stored {
    case: &'static str,
    body: fn() -> Value,
    attach_channel: bool,
}

/// Rule: a listed resource is the stored resource read through the list shape, so
/// every member it shares with the resource's own describe reports the same value
/// that describe reports for it.
///
/// `Input` and `DescribeInputResponse` carry the same twenty-two members (see
/// [`MEMBERS`]), so the assertion walks all of them. The rows differ along WHICH
/// per-pipeline list the input's `InputType` family ingests through, along the
/// optional members the create supplies (tags, a role) and along whether a
/// channel attaches the input, because the derived `AttachedChannels`, `State`,
/// `InputClass` and `InputSourceType` are what a listing could most easily get
/// wrong.
#[tokio::test]
async fn a_listed_input_is_the_input_describe_reports() {
    let table = [
        Stored {
            case: "url pull with two sources",
            body: || {
                json!({
                    "name": "two-source-pull",
                    "type": "URL_PULL",
                    "sources": [
                        source("https://example.com/a.m3u8"),
                        source("https://example.com/b.m3u8"),
                    ],
                })
            },
            attach_channel: false,
        },
        Stored {
            case: "a dynamic ts file, attached to a channel",
            body: || {
                json!({
                    "name": "dynamic-ts",
                    "type": "TS_FILE",
                    "sources": [source("s3://bucket/a/$urlPath$")],
                })
            },
            attach_channel: true,
        },
        Stored {
            case: "rtmp push with two destinations and tags",
            body: || {
                json!({
                    "name": "two-destination-push",
                    "type": "RTMP_PUSH",
                    "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
                    "tags": { "env": "test", "owner": "fixer" },
                })
            },
            attach_channel: false,
        },
        Stored {
            case: "mediaconnect with one flow and a role",
            body: || {
                json!({
                    "name": "one-flow",
                    "type": "MEDIACONNECT",
                    "mediaConnectFlows": [
                        { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                    ],
                    "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
                })
            },
            attach_channel: false,
        },
        Stored {
            case: "an sdi input, attached to a channel",
            body: || {
                json!({
                    "name": "sdi",
                    "type": "SDI",
                    "sdiSources": ["sdi-listing"],
                })
            },
            attach_channel: true,
        },
    ];

    for Stored {
        case,
        body,
        attach_channel,
    } in table
    {
        let router = medialive();
        let id = create_input_id(&router, case, body()).await;

        if attach_channel {
            let (status, channel) = call(
                &router,
                "POST",
                "/prod/channels",
                Some(json!({
                    "name": "attaching",
                    "inputAttachments": [
                        { "inputId": id.clone(), "inputAttachmentName": "slot-0" },
                    ],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected CreateChannel to answer 201, got {status}: {channel}"
            );
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
            "case {case:?}: expected the one stored input, got {body}"
        );
        let input = &page[0];
        let described = describe_input(&router, case, &id).await;

        for member in MEMBERS {
            assert_eq!(
                input.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one DescribeInput \
                 reports, got {:?} vs {:?}",
                input.get(member),
                described.get(member)
            );
        }
        assert_eq!(
            input["id"],
            json!(id),
            "case {case:?}: expected the created input's id, got {:?}",
            input["id"]
        );
        // The derived member the rows vary over: an input some channel attaches
        // is ATTACHED, one no channel attaches is DETACHED.
        let expected_state = if attach_channel { "ATTACHED" } else { "DETACHED" };
        assert_eq!(
            input["state"],
            json!(expected_state),
            "case {case:?}: expected the listed input to be {expected_state}, got {:?}",
            input["state"]
        );
        assert_eq!(
            input["attachedChannels"].as_array().map(Vec::len),
            Some(usize::from(attach_channel)),
            "case {case:?}: expected {} attached channel(s) in the listed input, got {:?}",
            usize::from(attach_channel),
            input["attachedChannels"]
        );
    }
}

/// Rule: a deleted resource keeps its record in the terminal state its own state
/// enum models, so every reader — the listing included — goes on reporting it
/// there, which is what an SDK waiter polling for that state reads.
///
/// The rows differ along WHICH of the stored inputs are deleted — the first, one
/// in the middle, the last, several, all of them, none — because the delete's
/// position in mint order is what a paged listing could get wrong, and each row
/// is walked unpaged, one input at a time and two at a time.
#[tokio::test]
async fn a_deleted_input_stays_listed_in_its_terminal_state() {
    // (case, the indices of the five stored inputs the row deletes)
    let table: [(&str, &[usize]); 6] = [
        ("nothing deleted", &[]),
        ("the first input deleted", &[0]),
        ("a middle input deleted", &[2]),
        ("the last input deleted", &[4]),
        ("two inputs deleted", &[1, 3]),
        ("every input deleted", &[0, 1, 2, 3, 4]),
    ];

    for (case, deleted_indices) in table {
        let router = medialive();
        let mut created: Vec<String> = Vec::new();
        for index in 0..5 {
            created.push(create_input_id(&router, case, pull_body(index)).await);
        }
        for index in deleted_indices {
            delete_input(&router, case, &created[*index]).await;
        }
        // The delete opens a `DELETING` window and the clock owes the terminal
        // `DELETED`, so the assertion below is made once that is due.
        if !deleted_indices.is_empty() {
            wait_for_the_clock();
        }

        for page_size in [None, Some(1), Some(2)] {
            let (reported, pages) = every_page(&router, case, page_size).await;
            assert_eq!(
                reported, created,
                "case {case:?}: with page size {page_size:?}, expected the listing to report \
                 every input the account holds, in mint order; pages were {pages:?}"
            );
        }

        // Each listed input carries the state its own describe reports for it:
        // the terminal `DELETED` for a deleted one, `DETACHED` for the rest.
        let (status, body) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        for (index, input) in listed(case, &body).iter().enumerate() {
            let expected = if deleted_indices.contains(&index) {
                "DELETED"
            } else {
                "DETACHED"
            };
            assert_eq!(
                input["state"],
                json!(expected),
                "case {case:?}: expected input {index} to be listed as {expected}, got {:?}",
                input["state"]
            );
            let described = describe_input(&router, case, &created[index]).await;
            assert_eq!(
                input["state"], described["state"],
                "case {case:?}: expected the listed state to equal the described one, got {:?} \
                 vs {:?}",
                input["state"], described["state"]
            );
        }
    }
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// repeating it over the same store answers the same inputs, in the same states.
///
/// The rows differ along how many inputs the store holds and how the listing is
/// paged, because a token-resumed walk is where a reporting call would most
/// easily consume state.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    for (case, inputs, page_size) in [
        ("one input, unpaged", 1usize, None),
        ("four inputs, page of one", 4, Some(1)),
        ("four inputs, page of three", 4, Some(3)),
    ] {
        let router = medialive();
        let mut created: Vec<String> = Vec::new();
        for index in 0..inputs {
            created.push(create_input_id(&router, case, pull_body(index)).await);
        }

        let (first, _) = every_page(&router, case, page_size).await;
        let (second, _) = every_page(&router, case, page_size).await;
        assert_eq!(
            first, created,
            "case {case:?}: expected the first walk to report every stored input"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to report the same inputs, \
             got {second:?} after {first:?}"
        );

        // And the inputs themselves are still readable, in the same state.
        for id in &created {
            let described = describe_input(&router, case, id).await;
            assert_eq!(
                described["state"],
                json!("DETACHED"),
                "case {case:?}: expected the input to still be DETACHED after two listings, \
                 got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: a page size that asks for no results at all is not a page size, so the
/// request is refused with this operation's modelled `BadRequestException`
/// (400) — one of the six errors `smithy_query(query="operation",
/// operation="ListInputs")` declares for it.
///
/// The rows differ along the non-positive size asked for, and the smallest
/// admissible size (1) is listed beside them as the published bound that must
/// still succeed.
#[tokio::test]
async fn a_page_size_that_asks_for_nothing_is_refused() {
    let router = medialive();
    create_input(&router, "a stored input", pull_body(0)).await;

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
