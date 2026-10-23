// SPDX-License-Identifier: CC0-1.0

//! `ListInputSecurityGroups`: which groups the listing reports, in which order,
//! how a page is cut, and that a listed group is the group
//! `DescribeInputSecurityGroup` reports — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: the issue's recorded case (one
//! `ListInputSecurityGroups` call over the groups the lifecycle created) is ONE
//! row of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_INPUT_SECURITY_GROUPS, MAX_LIST_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::input_security_group::{
    create_group as create_input_security_group, delete_group, describe_group as read_group,
};
use crate::wire::{call, error_code, medialive};

/// The collection URI `ListInputSecurityGroups` and `CreateInputSecurityGroup`
/// are bound to (`smithy_query(query="operation",
/// operation="ListInputSecurityGroups,CreateInputSecurityGroup")`: GET and POST
/// /prod/inputSecurityGroups).
const GROUPS: &str = "/prod/inputSecurityGroups";

/// `ListInputSecurityGroups` with `query` appended verbatim (both input members
/// are `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{GROUPS}{query}"), None).await
}

/// The `inputSecurityGroups` member as a list, whatever its length
/// (`ListInputSecurityGroupsResponse$InputSecurityGroups`, jsonName
/// `inputSecurityGroups`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["inputSecurityGroups"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an inputSecurityGroups list, got {body}"))
}

/// `CreateInputSecurityGroup` with `body`, answering the created group's id.
/// The request itself is the `input_security_group` suite's helper, so the two
/// suites cannot disagree about how a group is created.
async fn create_group(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    create_input_security_group(router, case, body).await["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DescribeInputSecurityGroup` on one group, asserting the 200 a stored group
/// answers with.
async fn describe_group(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = read_group(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeInputSecurityGroup({id}) to answer 200, \
         got {status}: {described}"
    );
    described
}

/// `DeleteInputSecurityGroup` on one group, asserting the 200 the delete of a
/// stored group answers with.
async fn delete_stored_group(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, deleted) = delete_group(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteInputSecurityGroup({id}) to answer 200, \
         got {status}: {deleted}"
    );
    deleted
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
            "case {case:?}: expected ListInputSecurityGroups to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} groups in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for group in &page {
            ids.push(group["id"].as_str().unwrap_or_default().to_string());
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

/// One row of the pagination table: how many groups the store holds and the
/// page size the request asks for.
struct Paging {
    case: &'static str,
    groups: usize,
    page_size: Option<i32>,
}

/// Rule: a paginated list operation reports every resource of its type this
/// service's store holds, each exactly once, in the order their ids were
/// minted, cut into pages of at most the requested page size bounded by the
/// service's own ceiling, whose token resumes at the first resource not yet
/// reported and is absent on the last page.
///
/// The rows differ along the COUNT of stored groups and the page size asked
/// for — none stored, one, a size that divides the store unevenly, no size at
/// all, a size past the service's ceiling, and more groups than one default
/// page holds — because that is what the rule generalises over.
#[tokio::test]
async fn list_input_security_groups_pages_through_every_stored_group_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            groups: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            groups: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one group, page of one",
            groups: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three groups, page of one",
            groups: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five groups, page of two",
            groups: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four groups, default page size",
            groups: 4,
            page_size: None,
        },
        Paging {
            case: "three groups, page size past the ceiling",
            groups: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // More groups than a single default page holds, so the default page
            // size is itself a cut and not just a ceiling nobody reaches.
            case: "more groups than one default page holds",
            groups: MAX_LIST_PAGE_SIZE + 7,
            page_size: None,
        },
    ];

    for Paging {
        case,
        groups,
        page_size,
    } in table
    {
        assert!(
            groups <= MAX_INPUT_SECURITY_GROUPS,
            "case {case:?}: the row asks for {groups} groups, past the store bound \
             {MAX_INPUT_SECURITY_GROUPS}"
        );
        let router = medialive();

        let mut created: Vec<String> = Vec::new();
        for index in 0..groups {
            let octet = index % 256;
            created.push(
                create_group(
                    &router,
                    case,
                    json!({ "whitelistRules": [{ "cidr": format!("10.0.{octet}.0/24") }] }),
                )
                .await,
            );
        }

        let (reported, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            reported, created,
            "case {case:?}: expected the listing to report every stored group once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if groups == 0 {
            1
        } else {
            groups.div_ceil(size)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {groups} groups of {size}, \
             got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                groups - size * index
            } else {
                size
            };
            assert_eq!(
                *page, expected,
                "case {case:?}: expected page {index} to carry {expected} groups, got {pages:?}"
            );
        }
    }
}

/// One row of the projection table: what the create supplies, and whether some
/// resource references the group.
struct Stored {
    case: &'static str,
    body: fn() -> Value,
    attach_channel: bool,
    attach_input: bool,
}

/// Rule: a listed resource is the stored resource read through the list shape,
/// so every member it shares with the resource's own describe reports the same
/// value that describe reports for it.
///
/// `InputSecurityGroup` and `DescribeInputSecurityGroupResponse` carry the same
/// seven members (`smithy_query(query="shape",
/// shape="InputSecurityGroup,DescribeInputSecurityGroupResponse")`), so the
/// assertion walks all of them. The rows differ along WHICH optional members the
/// stored group carries and along whether a channel or an input references it,
/// because `Inputs`, `Channels` and the derived `State` are what a listing could
/// most easily get wrong.
#[tokio::test]
async fn a_listed_group_is_the_group_describe_reports() {
    // Every member the two shapes share, from the `smithy_query` above.
    const MEMBERS: [&str; 7] = [
        "arn",
        "id",
        "inputs",
        "state",
        "tags",
        "whitelistRules",
        "channels",
    ];

    let table = [
        Stored {
            case: "no rules at all",
            body: || json!({}),
            attach_channel: false,
            attach_input: false,
        },
        Stored {
            case: "one host route",
            body: || json!({ "whitelistRules": [{ "cidr": "192.0.2.1/32" }] }),
            attach_channel: false,
            attach_input: false,
        },
        Stored {
            case: "several rules and tags",
            body: || {
                json!({
                    "whitelistRules": [
                        { "cidr": "203.0.113.0/24" },
                        { "cidr": "0.0.0.0/0" },
                    ],
                    "tags": { "env": "test", "owner": "fixer" },
                })
            },
            attach_channel: false,
            attach_input: false,
        },
        Stored {
            case: "referenced by a channel",
            body: || json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
            attach_channel: true,
            attach_input: false,
        },
        Stored {
            case: "referenced by an input",
            body: || json!({ "whitelistRules": [{ "cidr": "198.51.100.0/24" }] }),
            attach_channel: false,
            attach_input: true,
        },
        Stored {
            case: "referenced by both",
            body: || json!({ "tags": { "tier": "gold" } }),
            attach_channel: true,
            attach_input: true,
        },
    ];

    for Stored {
        case,
        body,
        attach_channel,
        attach_input,
    } in table
    {
        let router = medialive();
        let id = create_group(&router, case, body()).await;

        if attach_channel {
            let (status, channel) = call(
                &router,
                "POST",
                "/prod/channels",
                Some(json!({
                    "name": "uses-the-group",
                    "channelSecurityGroups": [id.clone()],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected CreateChannel to answer 201, got {status}: {channel}"
            );
        }
        if attach_input {
            let (status, created) = call(
                &router,
                "POST",
                "/prod/inputs",
                Some(json!({
                    "name": "uses-the-group",
                    "type": "URL_PULL",
                    "inputSecurityGroups": [id.clone()],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected CreateInput to answer 201, got {status}: {created}"
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
            "case {case:?}: expected the one stored group, got {body}"
        );
        let group = &page[0];
        let described = describe_group(&router, case, &id).await;

        for member in MEMBERS {
            assert_eq!(
                group.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 DescribeInputSecurityGroup reports, got {:?} vs {:?}",
                group.get(member),
                described.get(member)
            );
        }
        assert_eq!(
            group["id"],
            json!(id),
            "case {case:?}: expected the created group's id, got {:?}",
            group["id"]
        );
        // The derived members the rows vary over: a group nothing references is
        // IDLE, one some resource references is IN_USE.
        let referenced = attach_channel || attach_input;
        let expected_state = if referenced { "IN_USE" } else { "IDLE" };
        assert_eq!(
            group["state"],
            json!(expected_state),
            "case {case:?}: expected the listed group to be {expected_state}, got {:?}",
            group["state"]
        );
        assert_eq!(
            group["channels"].as_array().map(Vec::len),
            Some(usize::from(attach_channel)),
            "case {case:?}: expected {} channel(s) in the listed group, got {:?}",
            usize::from(attach_channel),
            group["channels"]
        );
        assert_eq!(
            group["inputs"].as_array().map(Vec::len),
            Some(usize::from(attach_input)),
            "case {case:?}: expected {} input(s) in the listed group, got {:?}",
            usize::from(attach_input),
            group["inputs"]
        );
    }
}

/// Rule: a listing reports the resources the account currently holds, so a
/// resource a delete has removed leaves it — while the deleted resource's own
/// read goes on reporting it in the terminal state its enum models.
///
/// This is the recorded behaviour, not a choice of the implementation: the
/// `medialive.input-security-group-lifecycle` fixture's check after its delete
/// is `assert group_id not in {g["Id"] for g in groups_after}` (it failed with
/// "deleted group … (terminal='DELETED') still present in
/// list_input_security_groups" while the listing reported deleted groups), and
/// the same fixture reads that group back as `DELETED` through
/// `DescribeInputSecurityGroup` — so the two readers are recorded to differ
/// exactly here. Both halves are asserted below.
///
/// The rows differ along WHICH of the stored groups are deleted — the first, one
/// in the middle, the last, several, all of them, none — because the delete's
/// position in mint order is what a paged listing could get wrong, and each row
/// is walked unpaged and one group at a time.
#[tokio::test]
async fn a_deleted_group_leaves_the_listing_but_is_still_described() {
    // (case, the indices of the five stored groups the row deletes)
    let table: [(&str, &[usize]); 6] = [
        ("nothing deleted", &[]),
        ("the first group deleted", &[0]),
        ("a middle group deleted", &[2]),
        ("the last group deleted", &[4]),
        ("two groups deleted", &[1, 3]),
        ("every group deleted", &[0, 1, 2, 3, 4]),
    ];

    for (case, deleted_indices) in table {
        let router = medialive();
        let mut created: Vec<String> = Vec::new();
        for index in 0..5 {
            created.push(
                create_group(
                    &router,
                    case,
                    json!({ "whitelistRules": [{ "cidr": format!("10.1.{index}.0/24") }] }),
                )
                .await,
            );
        }
        for index in deleted_indices {
            delete_stored_group(&router, case, &created[*index]).await;
        }

        let survivors: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| !deleted_indices.contains(index))
            .map(|(_, id)| id.clone())
            .collect();

        for page_size in [None, Some(1), Some(2)] {
            let (reported, pages) = every_page(&router, case, page_size).await;
            assert_eq!(
                reported, survivors,
                "case {case:?}: with page size {page_size:?}, expected the listing to report \
                 only the groups still in the account, in mint order; pages were {pages:?}"
            );
        }

        // The deleted groups are still READABLE, in the terminal state — the
        // listing dropped them, the store did not.
        for index in deleted_indices {
            let described = describe_group(&router, case, &created[*index]).await;
            assert_eq!(
                described["state"],
                json!("DELETED"),
                "case {case:?}: expected the deleted group to still describe as DELETED, got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// repeating it over the same store answers the same groups.
///
/// The rows differ along how many groups the store holds and how the listing is
/// paged, because a token-resumed walk is where a reporting call would most
/// easily consume state.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    for (case, groups, page_size) in [
        ("one group, unpaged", 1usize, None),
        ("four groups, page of one", 4, Some(1)),
        ("four groups, page of three", 4, Some(3)),
    ] {
        let router = medialive();
        let mut created: Vec<String> = Vec::new();
        for index in 0..groups {
            created.push(
                create_group(
                    &router,
                    case,
                    json!({ "whitelistRules": [{ "cidr": format!("172.16.{index}.0/24") }] }),
                )
                .await,
            );
        }

        let (first, _) = every_page(&router, case, page_size).await;
        let (second, _) = every_page(&router, case, page_size).await;
        assert_eq!(
            first, created,
            "case {case:?}: expected the first walk to report every stored group"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to report the same groups, \
             got {second:?} after {first:?}"
        );

        // And the groups themselves are still readable, in the same state.
        for id in &created {
            let described = describe_group(&router, case, id).await;
            assert_eq!(
                described["state"],
                json!("IDLE"),
                "case {case:?}: expected the group to still be IDLE after two listings, got {:?}",
                described["state"]
            );
        }
    }
}

/// Rule: a page size that asks for no results at all is not a page size, so the
/// request is refused with this operation's modelled `BadRequestException`
/// (400) — one of the six errors
/// `smithy_query(query="operation", operation="ListInputSecurityGroups")`
/// declares for it.
///
/// The rows differ along the non-positive size asked for, and the smallest
/// admissible size (1) is listed beside them as the published bound that must
/// still succeed.
#[tokio::test]
async fn a_page_size_that_asks_for_nothing_is_refused() {
    let router = medialive();
    create_group(&router, "a stored group", json!({})).await;

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
