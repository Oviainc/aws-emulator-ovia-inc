// SPDX-License-Identifier: CC0-1.0

//! `ListApplications` as a paginated report over the account's applications.
//!
//! RULE: the listing reports exactly the applications this account holds — every one of
//! them, each exactly once, each as the record a `GetApplication` of it answers — in an
//! order that is stable across calls, cut into pages of at most `MaxResults` entries,
//! with a `NextToken` handed back only when entries are left over and a token that
//! resumes the traversal where the previous page ended.
//!
//! The tables span what the rule generalises over: HOW MANY applications the account
//! holds (none, one, several, and one past the service's page size), WHICH page size the
//! request asks for (absent, both published bounds of `MaxResults` and values in
//! between), WHAT each application carries (`Name` and `Description` present or absent
//! and at their published length bounds), and WHICH mutation last changed the collection
//! (a create, an update of a record in it, a delete — including one taken out midway
//! through a paginated traversal). Every entry is checked against the record a
//! `GetApplication` of it answers, so the listing reports stored state rather than
//! anything derived from the request that asked for it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::MAX_LIST_PAGE_SIZE;

use crate::appconfig_workflow::{create_application, create_profile, json_call, router, Answer};

/// The published bounds of `Name` (length 1..64) and `Description` (length 0..1024) on
/// `CreateApplication`
/// (`smithy_query(query="operation",operation="CreateApplication")`).
const MAX_NAME: usize = 64;
/// See [`MAX_NAME`].
const MAX_DESCRIPTION: usize = 1024;

/// `ListApplications` with `query` as the raw query string (empty for a request that
/// names neither `MaxResults` nor `NextToken`). Both are `httpQuery` members, named
/// `max_results` and `next_token`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/applications".to_string()
    } else {
        format!("/applications?{query}")
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a listing (an answer with no `Items` at all reads as empty).
fn items(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of every entry of a listing, in the order it was served.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed application must carry an Id, got {item}"))
                .to_string()
        })
        .collect()
}

/// Walk the whole listing one page at a time and return the ids in the order they were
/// served, asserting the per-page half of the rule as it goes: a page never exceeds the
/// size the request asked for, and a page that carries a `NextToken` is exactly that
/// size, because a traversal is only cut where a page filled up.
async fn walk(router: &MultiServiceRouter, case: &str, max_results: Option<i32>) -> Vec<String> {
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| size as usize);
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a listing that never ends its traversal fails instead of hanging.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let answer = list(router, &query.join("&")).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let page = listed_ids(&answer);
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
        );
        token = answer.string("NextToken");
        if token.is_some() {
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?}: a page carrying a NextToken is a full page of {page_size}, got \
                 {page:?}"
            );
        }
        collected.extend(page);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the paginated walk did not terminate");
}

/// The listing reports exactly the applications the account holds, in an order that does
/// not move between calls — asserted while the collection grows from empty.
#[tokio::test]
async fn a_listing_reports_exactly_the_accounts_applications() {
    let router = router();

    let empty = list(&router, "").await;
    assert_eq!(
        empty.status,
        200,
        "case \"empty account\": expected 200, got {} / {}",
        empty.status,
        String::from_utf8_lossy(&empty.body)
    );
    assert!(
        empty
            .json()
            .get("Items")
            .is_some_and(|value| value.as_array().is_some_and(|items| items.is_empty())),
        "case \"empty account\": an account holding no application reports an empty Items, \
         got {}",
        empty.json()
    );
    assert!(
        empty.string("NextToken").is_none(),
        "case \"empty account\": nothing is left over, so there is no NextToken, got {:?}",
        empty.string("NextToken")
    );

    // Rows differ along the number of applications held and along the names used.
    let mut created: Vec<String> = Vec::new();
    for (case, name) in [
        ("one application", "alpha"),
        ("two applications", "beta"),
        ("three applications", "gamma"),
        ("four applications", "delta"),
    ] {
        created.push(create_application(&router, name).await);

        let answer = list(&router, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let mut reported = listed_ids(&answer);
        let mut expected = created.clone();
        reported.sort();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the applications created {expected:?}, got \
             {reported:?}"
        );

        // Stable order: a second identical call serves the same sequence.
        let again = list(&router, "").await;
        assert_eq!(
            listed_ids(&again),
            listed_ids(&answer),
            "case {case:?}: the order must not move between calls, got {:?} then {:?}",
            listed_ids(&answer),
            listed_ids(&again)
        );
    }
}

/// Every entry of the listing is the stored record, member for member what a
/// `GetApplication` of the same application answers.
#[tokio::test]
async fn each_listed_application_is_the_record_a_get_of_it_answers() {
    let router = router();

    // Rows differ along which optional member the application carries and along the
    // published length bounds of `Name` (1..64) and `Description` (0..1024).
    let cases: Vec<(&str, Value)> = vec![
        ("name only", json!({ "Name": "plain" })),
        (
            "name and description",
            json!({ "Name": "described", "Description": "a described application" }),
        ),
        (
            "the empty description at the lower length bound",
            json!({ "Name": "empty-description", "Description": "" }),
        ),
        (
            "a single-character name at the lower length bound",
            json!({ "Name": "a" }),
        ),
        (
            "both members at their upper length bounds",
            json!({ "Name": "n".repeat(MAX_NAME), "Description": "d".repeat(MAX_DESCRIPTION) }),
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, body) in &cases {
        let answer = json_call(&router, "POST", "/applications", body).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: CreateApplication must answer 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        created.push(
            answer
                .string("Id")
                .unwrap_or_else(|| panic!("case {case:?}: CreateApplication answered no Id")),
        );
    }

    let listed = items(&list(&router, "").await);
    assert_eq!(
        listed.len(),
        created.len(),
        "every created application must be listed, expected {} entries, got {listed:?}",
        created.len()
    );

    for (index, id) in created.iter().enumerate() {
        let case = cases[index].0;
        let entry = listed
            .iter()
            .find(|item| item.get("Id").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry application {id:?}"));
        let fetched = json_call(&router, "GET", &format!("/applications/{id}"), &Value::Null).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetApplication must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            *entry,
            fetched.json(),
            "case {case:?}: the listed entry must be the record GetApplication answers {}, got \
             {entry}",
            fetched.json()
        );
    }
}

/// Whatever page size a request asks for, the pages of a traversal concatenate to the
/// single-call listing: every application exactly once, in the same order.
#[tokio::test]
async fn pages_of_every_size_concatenate_to_the_whole_collection() {
    // Rows differ along the collection size and the requested page size: `MaxResults` is
    // modelled `@range(min: 1, max: 50)`, so 1 and 50 are its published bounds, and
    // `None` is a request that names no page size at all.
    for size in [0_usize, 1, 2, 5, 7] {
        // A fresh account per row: the listing is account-scoped, so the collection size
        // IS the number of applications the engine holds.
        let router = router();
        for index in 0..size {
            create_application(&router, &format!("app-{index}")).await;
        }
        let whole = listed_ids(&list(&router, "").await);
        assert_eq!(
            whole.len(),
            size,
            "case \"{size} applications, one call\": expected {size} entries, got {whole:?}"
        );

        for page_size in [None, Some(1), Some(2), Some(3), Some(50)] {
            let case = format!("{size} applications, MaxResults={page_size:?}");
            let walked = walk(&router, &case, page_size).await;
            assert_eq!(
                walked, whole,
                "case {case:?}: the pages must concatenate to the whole listing {whole:?}, got \
                 {walked:?}"
            );
            let mut unique = walked.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case:?}: no application may be served twice, got {walked:?}"
            );
        }
    }
}

/// A request naming no page size is cut at the service's own page size, not at the
/// collection: one entry past it is served on a second page.
#[tokio::test]
async fn a_request_naming_no_page_size_is_cut_at_the_services_page_size() {
    let router = router();
    for index in 0..=MAX_LIST_PAGE_SIZE {
        create_application(&router, &format!("app-{index}")).await;
    }

    let first = list(&router, "").await;
    let first_ids = listed_ids(&first);
    assert_eq!(
        first_ids.len(),
        MAX_LIST_PAGE_SIZE,
        "case \"one past the page size\": expected a full page of {MAX_LIST_PAGE_SIZE}, got {} \
         entries",
        first_ids.len()
    );
    let token = first.string("NextToken").unwrap_or_else(|| {
        panic!(
            "case \"one past the page size\": a truncated listing carries a NextToken, got {}",
            String::from_utf8_lossy(&first.body)
        )
    });

    let second = list(&router, &format!("next_token={token}")).await;
    let second_ids = listed_ids(&second);
    assert_eq!(
        second_ids.len(),
        1,
        "case \"one past the page size\": the second page holds the one left-over entry, got \
         {second_ids:?}"
    );
    assert!(
        second.string("NextToken").is_none(),
        "case \"one past the page size\": the last page ends the traversal, got {:?}",
        second.string("NextToken")
    );
    assert!(
        !first_ids.contains(&second_ids[0]),
        "case \"one past the page size\": the second page repeats nothing from the first, got \
         {second_ids:?} after {first_ids:?}"
    );
}

/// A page size outside the modelled range is refused with 400 and reports nothing; the
/// published bounds themselves are accepted.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = router();
    for index in 0..3 {
        create_application(&router, &format!("bounded-{index}")).await;
    }

    // `MaxResults` is `@range(min: 1, max: 50)`: each published bound, and one past each.
    for (case, max_results, expected_status) in [
        ("the lower bound, 1", 1, 200),
        ("the upper bound, 50", 50, 200),
        ("one below the lower bound, 0", 0, 400),
        ("one past the upper bound, 51", 51, 400),
    ] {
        let answer = list(&router, &format!("max_results={max_results}")).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 400 {
            assert!(
                items(&answer).is_empty(),
                "case {case:?}: a refused request reports nothing, got {:?}",
                items(&answer)
            );
        }
    }
}

/// A `NextToken` this listing did not mint is refused with the modelled
/// `BadRequestException`, rather than silently restarting the traversal.
#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    let mut application = String::new();
    for index in 0..3 {
        application = create_application(&router, &format!("tokens-{index}")).await;
    }
    // Three profiles under ONE application, so a ListConfigurationProfiles page of one is
    // truncated and mints a cursor of the same encoding for a different listing.
    for index in 0..3 {
        create_profile(&router, &application, &format!("profile-{index}")).await;
    }

    // A token minted by this very listing, as the control: the rule refuses tokens this
    // listing did not issue, not every token.
    let mine = list(&router, "max_results=1")
        .await
        .string("NextToken")
        .expect("a truncated listing carries a NextToken");
    // A token of the same encoding minted by a DIFFERENT paginated listing of this same
    // service: a cursor is stamped with the listing it belongs to, so this one resumes
    // nothing here.
    let other_listing = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/configurationprofiles?max_results=1"),
        &Value::Null,
    )
    .await
    .string("NextToken")
    .expect("a truncated ListConfigurationProfiles of a three-profile application has a token");

    let resumed = list(&router, &format!("max_results=1&next_token={mine}")).await;
    assert_eq!(
        resumed.status,
        200,
        "case \"the token this listing minted\": expected 200, got {} / {}",
        resumed.status,
        String::from_utf8_lossy(&resumed.body)
    );

    // Rows differ along HOW the token fails to be one of this listing's.
    for (case, token) in [
        ("a cursor minted by another listing", other_listing),
        ("not this service's encoding", "not-a-token".to_string()),
        (
            "a mangled copy of a real token",
            mine[..mine.len() - 1].to_string(),
        ),
        ("an application id passed as a token", "abcdefg".to_string()),
        (
            "a token of the right encoding but no cursor fields",
            "6162".to_string(),
        ),
    ] {
        let answer = list(&router, &format!("next_token={token}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.error_code()
        );
        assert!(
            items(&answer).is_empty(),
            "case {case:?}: a refused request reports nothing, got {:?}",
            items(&answer)
        );
    }
}

/// One mutation of the account's applications, applied by
/// [`the_listing_tracks_every_mutation_of_the_account`]. `Describe` and `Delete` address
/// an application by its position in the live collection, so a row names a resource that
/// earlier rows created rather than an id written into the test.
#[derive(Debug)]
enum Mutation {
    /// `CreateApplication` with this name.
    Create(&'static str),
    /// `UpdateApplication` writing this `Description` onto the live application at this
    /// position.
    Describe(usize, &'static str),
    /// `DeleteApplication` of the live application at this position.
    Delete(usize),
}

/// The listing is a report over the applications the account holds AT THE TIME OF THE
/// CALL, so every mutation of that collection — a create, an update of a record in it, a
/// delete — shows in the next listing, by one call and by a paginated walk alike.
#[tokio::test]
async fn the_listing_tracks_every_mutation_of_the_account() {
    let router = router();

    // Rows differ along WHICH mutation is applied and along what it does to the
    // collection: grow it, change a record inside it, shrink it (including back to
    // empty), and grow it again after a delete.
    let mutations = [
        ("a first create", Mutation::Create("alpha")),
        ("a second create", Mutation::Create("beta")),
        ("a third create", Mutation::Create("gamma")),
        (
            "a description written onto a listed application",
            Mutation::Describe(0, "described after it was listed"),
        ),
        ("a delete from the middle", Mutation::Delete(1)),
        ("a create after a delete", Mutation::Create("delta")),
        (
            "a description written onto the application created after the delete",
            Mutation::Describe(2, "described after the delete"),
        ),
        ("a delete of the first", Mutation::Delete(0)),
        ("a delete of the last", Mutation::Delete(1)),
        ("a delete back to an empty account", Mutation::Delete(0)),
    ];

    let mut live: Vec<String> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    for (case, mutation) in &mutations {
        match mutation {
            Mutation::Create(name) => live.push(create_application(&router, name).await),
            Mutation::Describe(position, description) => {
                let id = &live[*position];
                let answer = json_call(
                    &router,
                    "PATCH",
                    &format!("/applications/{id}"),
                    &json!({ "Description": description }),
                )
                .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: UpdateApplication must answer 200, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }
            Mutation::Delete(position) => {
                let id = live.remove(*position);
                let answer =
                    json_call(&router, "DELETE", &format!("/applications/{id}"), &Value::Null).await;
                assert_eq!(
                    answer.status,
                    204,
                    "case {case:?}: DeleteApplication must answer 204, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                deleted.push(id);
            }
        }

        // The single-call listing holds exactly the live applications.
        let answer = list(&router, "").await;
        let mut reported = listed_ids(&answer);
        let mut expected = live.clone();
        reported.sort();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the listing must hold exactly the live applications {expected:?}, \
             got {reported:?}"
        );
        // And nothing that was deleted, however small the pages are.
        let walked = walk(&router, case, Some(1)).await;
        let mut walked_sorted = walked.clone();
        walked_sorted.sort();
        assert_eq!(
            walked_sorted, expected,
            "case {case:?}: a paginated walk must hold exactly the live applications \
             {expected:?}, got {walked:?}"
        );
        for id in &deleted {
            assert!(
                !walked.contains(id),
                "case {case:?}: the deleted application {id:?} must be gone from the listing, \
                 got {walked:?}"
            );
        }
        // Every surviving entry still reports the stored record, so an update to one of
        // them shows in the listing rather than only in a Get of it.
        for entry in items(&answer) {
            let id = entry
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a listed application must carry an Id"));
            let fetched =
                json_call(&router, "GET", &format!("/applications/{id}"), &Value::Null).await;
            assert_eq!(
                entry,
                fetched.json(),
                "case {case:?}: the listed entry must be the record GetApplication answers {}, \
                 got {entry}",
                fetched.json()
            );
        }
    }
}

/// An application deleted midway through a paginated traversal is simply absent from the
/// pages still to come: the cursor the earlier page handed back still resumes, and no
/// surviving application is skipped or served twice.
#[tokio::test]
async fn a_delete_midway_through_a_walk_drops_only_the_deleted_application() {
    // Rows differ along WHICH entry of the remaining traversal is deleted after the first
    // page of one: the very next one served, one in the middle, and the last one of the
    // collection.
    for target in [1_usize, 2, 3] {
        // A fresh account per row, so the row's deletes are the only ones in play.
        let router = router();
        for index in 0..4 {
            create_application(&router, &format!("midway-{index}")).await;
        }
        // The traversal order is the listing's own, so take it from the listing rather
        // than from the order the applications were created in.
        let order_now = listed_ids(&list(&router, "").await);
        assert_eq!(
            order_now.len(),
            4,
            "case \"delete entry {target}\": expected 4 entries, got {order_now:?}"
        );

        let first = list(&router, "max_results=1").await;
        let served = listed_ids(&first);
        let token = first.string("NextToken").unwrap_or_else(|| {
            panic!("case \"delete entry {target}\": a truncated listing carries a NextToken")
        });

        let doomed = order_now[target].clone();
        let answer = json_call(
            &router,
            "DELETE",
            &format!("/applications/{doomed}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            204,
            "case \"delete entry {target}\": DeleteApplication must answer 204, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // Resume the walk from the token minted before the delete.
        let mut collected = served.clone();
        let mut token = Some(token);
        while let Some(cursor) = token {
            let page = list(&router, &format!("max_results=1&next_token={cursor}")).await;
            assert_eq!(
                page.status,
                200,
                "case \"delete entry {target}\": a cursor minted before the delete must still \
                 resume, got {} / {}",
                page.status,
                String::from_utf8_lossy(&page.body)
            );
            collected.extend(listed_ids(&page));
            token = page.string("NextToken");
        }

        let expected: Vec<String> = order_now
            .iter()
            .filter(|id| **id != doomed)
            .cloned()
            .collect();
        assert_eq!(
            collected, expected,
            "case \"delete entry {target}\": the resumed walk must serve every surviving \
             application exactly once {expected:?}, got {collected:?}"
        );
    }
}

/// Listing is a report: walking it changes nothing a later caller sees. The collection and
/// every record in it are identical before and after a full paginated walk.
#[tokio::test]
async fn walking_the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    for index in 0..4 {
        create_application(&router, &format!("reporting-{index}")).await;
    }

    let before = items(&list(&router, "").await);
    for page_size in [Some(1), Some(2), Some(50), None] {
        let case = format!("a walk with MaxResults={page_size:?}");
        walk(&router, &case, page_size).await;
        let after = items(&list(&router, "").await);
        assert_eq!(
            after, before,
            "case {case:?}: a read must not change what a later caller sees, expected \
             {before:?}, got {after:?}"
        );
    }
}
