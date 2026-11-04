// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListBackendEnvironments`.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses, not an internal function's return value.
//!
//! The rules under test:
//!
//! 1. `ListBackendEnvironments` REPORTS the backend environments persisted under the
//!    app its URI names and nothing else — each element being the record
//!    `GetBackendEnvironment` answers for that environment, in ascending
//!    `environmentName` order — and it changes nothing a later caller sees.
//! 2. The page bound is `maxResults`: a page holds `min(bound, remaining)`
//!    environments, a caller that set no bound (member absent, or its modelled default
//!    of 0) gets the modelled maximum, and `nextToken` is the name of the first
//!    environment the page did NOT return, so a paged walk visits the collection once
//!    in order.
//! 3. A `nextToken` this service could not have issued is refused with the modelled
//!    `BadRequestException`; one it could have issued resumes the scan at the first
//!    name at or after it, even when no environment carries that name.
//! 4. `environmentName` is an optional exact-match FILTER over that collection, so a
//!    name no environment of the app carries narrows the page to nothing rather than
//!    refusing the call.
//! 5. An app this store does not hold is the same modelled 404 every other app-scoped
//!    operation answers.
//!
//! Each table spans the class the rule generalises over — how many environments exist,
//! which bound the caller set, where a token falls in the collection, which name the
//! filter asks for — rather than one recorded request.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};

/// The modelled `range` maximum of `ListBackendEnvironments$maxResults`, which is also
/// the page a caller that set no bound gets
/// (`smithy_query(query="operation",operation="ListBackendEnvironments")`).
const MODELLED_MAX_RESULTS: usize = 50;

/// `EnvironmentName`'s modelled `length` maximum (`length 1..255`, from
/// `smithy_query(query="operation",operation="ListBackendEnvironments")`), which is the
/// longest value a `nextToken` this operation issued can have.
const ENVIRONMENT_NAME_MAX_LENGTH: usize = 255;

/// `ListBackendEnvironments` over the wire; `query` is the raw query string (`""` for
/// none), so a case drives `maxResults`, `nextToken` and `environmentName` exactly as a
/// client would.
async fn list_environments(emulator: &Emulator, app_id: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/backendenvironments{query}"),
            None,
        )
        .await
}

/// `GetBackendEnvironment` over the wire.
async fn get_environment(emulator: &Emulator, app_id: &str, environment_name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/backendenvironments/{environment_name}"),
            None,
        )
        .await
}

/// The `backendEnvironments` member of a reply, which is `@required`: the member is
/// present even on a page that holds nothing.
fn listed(label: &str, reply: &Reply) -> Vec<Value> {
    let listed = reply.body.get("backendEnvironments").unwrap_or_else(|| {
        panic!("case {label:?}: ListBackendEnvironments must carry `backendEnvironments`: {reply:?}")
    });
    listed
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {label:?}: `backendEnvironments` must be a list, got {listed:?}")
        })
        .clone()
}

/// The names of the environments one page returned, in the order the page returned them.
fn listed_names(label: &str, reply: &Reply) -> Vec<String> {
    listed(label, reply)
        .iter()
        .map(|record| string_member(record, "environmentName"))
        .collect()
}

/// The `nextToken` a listing reply carries, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create `count` backend environments on `app_id` and answer their names IN THE ORDER
/// `ListBackendEnvironments` must report them (ascending `environmentName`, the key the
/// app's environment map is held under).
///
/// The optional members vary across the created environments, so the records a page
/// carries are distinguishable from one another rather than all alike.
async fn environments_of(emulator: &Emulator, app_id: &str, count: usize) -> Vec<String> {
    let mut names = BTreeSet::new();
    for index in 0..count {
        let name = format!("env-{index:02}");
        let request = if index % 2 == 0 {
            json!({ "environmentName": name })
        } else {
            json!({
                "environmentName": name,
                "stackName": format!("amplify-stack-{index}"),
                "deploymentArtifacts": format!("amplify-artifacts-{index}"),
            })
        };
        let created = emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/backendenvironments"),
                Some(&request),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "seeding environment {name:?} on {app_id}: expected HTTP 200, got {created:?}"
        );
        assert!(
            names.insert(name.clone()),
            "seeding environment {name:?} on {app_id}: expected a fresh name, got it twice"
        );
    }
    names.into_iter().collect()
}

#[tokio::test]
async fn list_backend_environments_reports_exactly_the_apps_persisted_environments() {
    let emulator = Emulator::new();

    // A second app holds environments throughout: a page is scoped to ONE app, so no
    // record of this one may ever appear in the other's listing.
    let bystander_app = emulator.app_id_of_new_app("backend-env-list-bystander").await;
    let bystander_names = environments_of(&emulator, &bystander_app, 3).await;

    // The counts span none, one, a few and more than one page bound's worth.
    for count in [0_usize, 1, 2, 5, 13] {
        let label = format!("{count} environments");
        let app_id = emulator
            .app_id_of_new_app(&format!("backend-env-list-{count}"))
            .await;
        let expected = environments_of(&emulator, &app_id, count).await;

        let reply = list_environments(&emulator, &app_id, "").await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 from ListBackendEnvironments, got {reply:?}"
        );
        assert_eq!(
            listed_names(&label, &reply),
            expected,
            "case {label:?}: ListBackendEnvironments must report every persisted environment of \
             the app, in ascending environmentName order, got {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {label:?}: a page that reached the end carries no nextToken, got {reply:?}"
        );

        // Each listed element is the record the implemented reader answers for that
        // environment, so a listing and a read of the same environment cannot drift.
        for record in listed(&label, &reply) {
            let name = string_member(&record, "environmentName");
            let fetched = get_environment(&emulator, &app_id, &name).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?}: a listed environment must be readable by its name, got \
                 {fetched:?}"
            );
            assert_eq!(
                fetched.member("backendEnvironment"),
                record,
                "case {label:?}: the listed environment {name:?} must equal the record \
                 GetBackendEnvironment answers, got {fetched:?}"
            );
        }

        // Listing is a report: the next caller sees exactly what this one saw, and the
        // other app's listing is untouched.
        let again = list_environments(&emulator, &app_id, "").await;
        assert_eq!(
            again.body, reply.body,
            "case {label:?}: listing environments must not change what a later caller sees, got \
             {again:?}"
        );
        let bystander = list_environments(&emulator, &bystander_app, "").await;
        assert_eq!(
            listed_names("bystander app", &bystander),
            bystander_names,
            "case {label:?}: one app's listing must report no other app's environments, got \
             {bystander:?}"
        );
    }
}

#[tokio::test]
async fn list_backend_environments_pages_the_collection_once_under_every_bound() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-list-paging").await;
    let expected = environments_of(&emulator, &app_id, 13).await;
    let expected_set: BTreeSet<String> = expected.iter().cloned().collect();

    // The bounds span 1, a middle value, exactly the number of records, more than the
    // number of records, the modelled `range` maximum, and the two "caller set no
    // bound" cases: the member absent, and its modelled default of 0.
    let bounds: Vec<(&str, String, Option<usize>)> = vec![
        ("maxResults=1", "?maxResults=1".to_string(), Some(1)),
        ("maxResults=4", "?maxResults=4".to_string(), Some(4)),
        ("maxResults=13", "?maxResults=13".to_string(), Some(13)),
        ("maxResults=14", "?maxResults=14".to_string(), Some(14)),
        ("maxResults=50", "?maxResults=50".to_string(), Some(50)),
        ("maxResults absent", String::new(), None),
        ("maxResults=0", "?maxResults=0".to_string(), None),
    ];

    for (label, query, bound) in bounds {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let paged = match &token {
                Some(value) => format!("{query}{separator}nextToken={value}"),
                None => query.clone(),
            };
            let reply = list_environments(&emulator, &app_id, &paged).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from ListBackendEnvironments, got {reply:?}"
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages"
            );

            let page = listed_names(label, &reply);
            let remaining = expected.len() - seen.len();
            let want = bound.unwrap_or(MODELLED_MAX_RESULTS).min(remaining);
            assert_eq!(
                page.len(),
                want,
                "case {label:?}: page {pages} must hold min(bound, remaining) = {want} \
                 environments, got {page:?}"
            );
            seen.extend(page);

            token = next_token(&reply);
            match &token {
                // The token is the name of the first environment NOT returned.
                Some(value) => assert_eq!(
                    Some(value),
                    expected.get(seen.len()),
                    "case {label:?}: the nextToken must name the first environment not returned, \
                     got {value:?}"
                ),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: a paged walk must visit the whole collection once, in order, got \
             {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected_set,
            "case {label:?}: a paged walk must visit every environment of the app"
        );
    }

    // One past the modelled maximum is refused by the boundary validator, not served.
    let refused = list_environments(&emulator, &app_id, "?maxResults=51").await;
    assert_eq!(
        refused.status, 400,
        "case \"maxResults=51\": a value past the modelled range maximum must be refused, got \
         {refused:?}"
    );
}

#[tokio::test]
async fn list_backend_environments_refuses_a_token_it_never_issued_and_resumes_at_one_it_did() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-list-tokens").await;
    let expected = environments_of(&emulator, &app_id, 5).await;

    // Every token this operation issues is an `environmentName`, modelled
    // `length 1..255`, while `NextToken` admits up to 2000 characters: a longer value
    // could not be a name this service issued, so it is refused rather than served as
    // an arbitrary suffix of the collection.
    for (label, token) in [
        (
            "token one character past the environmentName maximum",
            "t".repeat(ENVIRONMENT_NAME_MAX_LENGTH + 1),
        ),
        ("token at the nextToken maximum", "t".repeat(2000)),
    ] {
        let reply = list_environments(&emulator, &app_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: an unissued nextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            reply.error_code
        );
    }

    // A token the service could have issued resumes the scan at the first name AT OR
    // AFTER it — including a name the app does not hold, which is how a page boundary
    // survives a collection that changed between two calls. The seeded names all start
    // with "env-", so "a" sorts below every one of them and "z" above all of them.
    let resumptions: Vec<(String, String, usize)> = vec![
        (
            "token naming the first environment".to_string(),
            expected[0].clone(),
            0,
        ),
        (
            "token naming a middle environment".to_string(),
            expected[2].clone(),
            2,
        ),
        (
            "token naming the last environment".to_string(),
            expected[4].clone(),
            4,
        ),
        (
            "token naming no environment, below them all".to_string(),
            "a".to_string(),
            0,
        ),
        (
            "token naming no environment, above them all".to_string(),
            "z".to_string(),
            5,
        ),
        (
            "token at the environmentName maximum, above them all".to_string(),
            "z".repeat(ENVIRONMENT_NAME_MAX_LENGTH),
            5,
        ),
    ];
    for (label, token, skipped) in resumptions {
        let reply = list_environments(&emulator, &app_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 from ListBackendEnvironments, got {reply:?}"
        );
        assert_eq!(
            listed_names(&label, &reply),
            expected[skipped..].to_vec(),
            "case {label:?}: the page must resume at the first name at or after the token, got \
             {reply:?}"
        );
    }
}

#[tokio::test]
async fn the_environment_name_member_filters_the_page_by_exact_name() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-list-filter").await;
    let expected = environments_of(&emulator, &app_id, 5).await;

    // A second app carries one of the same names: the filter selects within the app the
    // URI names, never across apps.
    let other_app = emulator.app_id_of_new_app("backend-env-list-filter-other").await;
    let mut other_names = environments_of(&emulator, &other_app, 2).await;
    // A name ONLY the other app holds, so a filter for it on this app matches nothing.
    let only_other = "only-other".to_string();
    let created = emulator
        .call(
            "POST",
            &format!("/apps/{other_app}/backendenvironments"),
            Some(&json!({ "environmentName": only_other })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "seeding {only_other:?} on the other app: expected HTTP 200, got {created:?}"
    );
    other_names.push(only_other.clone());

    // A name the app DOES carry narrows the page to exactly that one environment, which
    // is the record the reader answers for it.
    for name in &expected {
        let reply = list_environments(&emulator, &app_id, &format!("?environmentName={name}")).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected HTTP 200 from a filtered ListBackendEnvironments, got \
             {reply:?}"
        );
        assert_eq!(
            listed_names(name, &reply),
            vec![name.clone()],
            "case {name:?}: the filter must select exactly the environment of that name, got \
             {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {name:?}: a filtered page that reached the end carries no nextToken, got \
             {reply:?}"
        );
        assert_eq!(
            listed(name, &reply)[0],
            get_environment(&emulator, &app_id, name)
                .await
                .member("backendEnvironment"),
            "case {name:?}: the filtered element must equal the record GetBackendEnvironment \
             answers"
        );
    }

    // A name no environment of the app carries narrows the page to NOTHING: the member
    // is a filter, not an address, and this operation models no 404. The cases span a
    // name no app holds, a name only the other app holds, a strict prefix of a stored
    // name, a stored name with a suffix, a differently-cased stored name, and the
    // modelled `length` bounds of `environmentName`.
    let empty_cases: Vec<(String, String)> = vec![
        ("a name no app holds".to_string(), "staging".to_string()),
        ("a name only another app holds".to_string(), only_other),
        (
            "a strict prefix of a stored name".to_string(),
            "env-".to_string(),
        ),
        (
            "a stored name with a suffix".to_string(),
            format!("{}x", expected[0]),
        ),
        (
            "a stored name in different case".to_string(),
            expected[0].to_uppercase(),
        ),
        (
            "one character, the modelled length minimum".to_string(),
            "x".to_string(),
        ),
        (
            "the modelled length maximum".to_string(),
            "z".repeat(ENVIRONMENT_NAME_MAX_LENGTH),
        ),
    ];
    for (label, filter) in empty_cases {
        let reply =
            list_environments(&emulator, &app_id, &format!("?environmentName={filter}")).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: a filter matching nothing must still be served, got {reply:?}"
        );
        assert!(
            listed(&label, &reply).is_empty(),
            "case {label:?}: the filter {filter:?} matches no environment, so the page is empty, \
             got {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {label:?}: an empty page carries no nextToken, got {reply:?}"
        );
    }

    // The filter changed nothing: the unfiltered listings of both apps are what they
    // were.
    assert_eq!(
        listed_names(
            "unfiltered after filtering",
            &list_environments(&emulator, &app_id, "").await
        ),
        expected,
        "filtering must not change what a later unfiltered caller sees"
    );
    assert_eq!(
        listed_names(
            "other app after filtering",
            &list_environments(&emulator, &other_app, "").await
        ),
        other_names,
        "filtering one app's environments must not touch another app's"
    );
}

#[tokio::test]
async fn list_backend_environments_refuses_an_app_the_store_does_not_hold() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("backend-env-list-refusal").await;
    let expected = environments_of(&emulator, &app_id, 2).await;

    // `AppId` is modelled `length 1..20, pattern ^d[a-z0-9]+$`, so each of these is a
    // well-formed id for an app that does not exist — the request passes the boundary
    // validator and the refusal is the handler's.
    for absent_app in ["dnosuchapp1", "d0", &format!("d{}", "9".repeat(18))] {
        for (label, query) in [
            ("unfiltered", String::new()),
            ("with a filter", "?environmentName=staging".to_string()),
            ("with a bound", "?maxResults=1".to_string()),
        ] {
            let refused = list_environments(&emulator, absent_app, &query).await;
            assert_eq!(
                refused.status, 404,
                "case {absent_app:?} {label:?}: expected HTTP 404 from ListBackendEnvironments, \
                 got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("NotFoundException"),
                "case {absent_app:?} {label:?}: expected NotFoundException, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                Some(format!("App {absent_app} not found.")),
                "case {absent_app:?} {label:?}: the 404 names the entity the request addressed, \
                 got {:?}",
                refused.message()
            );
        }
    }

    // The refusals changed nothing, and an app whose environments went with it is
    // itself gone: deleting the app makes its listing the same modelled 404.
    assert_eq!(
        listed_names(
            "surviving app",
            &list_environments(&emulator, &app_id, "").await
        ),
        expected,
        "a refused listing must leave the real app's environments exactly as they were"
    );
    let deleted = emulator.delete_app(&app_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteApp, got {deleted:?}"
    );
    let gone = list_environments(&emulator, &app_id, "").await;
    assert_eq!(
        gone.status, 404,
        "the environments of a deleted app are gone with it, got {gone:?}"
    );
}
