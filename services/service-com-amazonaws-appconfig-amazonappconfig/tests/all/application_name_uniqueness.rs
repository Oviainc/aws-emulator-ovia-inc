// SPDX-License-Identifier: CC0-1.0

//! An application's name addresses it within the account, over every write that sets one.
//!
//! RULE: the account holds at most one application per name — `CreateApplication` under a
//! name an application already holds, and `UpdateApplication` renaming a record onto a
//! name a DIFFERENT application holds, are both refused with the modelled
//! `BadRequestException` (HTTP 400, never `ConflictException`) whose message names the
//! submitted name, the id of the application already answering to it and the account, and
//! neither call writes anything: the account still holds exactly the one application under
//! that name, with the record it had.
//!
//! The tables span what that rule generalises over: the FORM of the name (plain, a single
//! character and 64 characters at `Name`'s published length bounds, non-ASCII,
//! URI-significant punctuation), WHAT ELSE the colliding request carries (nothing, a
//! different `Description`, the empty `Description`, `Tags`), and HOW CLOSE a free name may
//! be to a taken one (differing in case, in surrounding whitespace, by a suffix, or by one
//! character at the length bound) — because the conflict is on the whole name, exactly, and
//! nothing else about the request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{account, json_call, router, Answer};

/// The published length bounds of `CreateApplication`'s `Name` (length 1..64)
/// (`smithy_query(query="operation",operation="CreateApplication")`).
const MIN_NAME: usize = 1;
/// See [`MIN_NAME`].
const MAX_NAME: usize = 64;

/// `POST /applications`.
async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "POST", "/applications", body).await
}

/// `PATCH /applications/{ApplicationId}`.
async fn update(router: &MultiServiceRouter, application_id: &str, body: &Value) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/applications/{application_id}"),
        body,
    )
    .await
}

/// `GET /applications/{ApplicationId}`.
async fn get(router: &MultiServiceRouter, application_id: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application_id}"),
        &Value::Null,
    )
    .await
}

/// Every application the account holds, read back through `ListApplications`.
async fn listed_applications(router: &MultiServiceRouter) -> Vec<Value> {
    let answer = json_call(router, "GET", "/applications", &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "ListApplications must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of each entry of a listing.
fn ids_of(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed application must carry an Id, got {item}"))
                .to_string()
        })
        .collect()
}

/// The ids of every application the account holds.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    ids_of(&listed_applications(router).await)
}

/// The ids of the applications the account holds under `name`, which the rule bounds at
/// one. Read through the same status-checked listing as [`listed_ids`], so a listing that
/// failed cannot read as "no application holds that name".
async fn ids_named(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let holding: Vec<Value> = listed_applications(router)
        .await
        .into_iter()
        .filter(|item| item.get("Name").and_then(Value::as_str) == Some(name))
        .collect();
    ids_of(&holding)
}

/// Create an application, asserting the create AWS serves is still served.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let answer = create(router, body).await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: a free name must still be created, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateApplication answered no Id"))
}

/// Assert one refused write: the status, the modelled error code and the whole message the
/// rule demands, which names the submitted name, the id already holding it and the account.
fn assert_name_taken(case: &str, answer: &Answer, name: &str, holder_id: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let code = answer
        .error_code()
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        code, "BadRequestException",
        "case {case:?}: the conflict on a taken application name is BadRequestException, got \
         {code:?}"
    );
    let message = answer.string("Message").unwrap_or_default();
    let expected = format!(
        "Application with name {name} already exists with id {holder_id} in account {}",
        account()
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected message {expected:?}, got {message:?}"
    );
}

/// The names the tables below collide on: one row per form of `Name` the rule generalises
/// over.
fn colliding_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "my-mobile-app".to_string()),
        (
            "a single-character name at Name's lower length bound",
            "a".repeat(MIN_NAME),
        ),
        (
            "a name at Name's upper length bound",
            "n".repeat(MAX_NAME),
        ),
        ("a non-ASCII name", "приложение-配置".to_string()),
        (
            "URI-significant punctuation in the name",
            "a/b?c#d=e&f".to_string(),
        ),
        ("a name that is only punctuation", "._-".to_string()),
    ]
}

/// A second application cannot take a name an application already holds, whatever else the
/// request carries, and the refused create writes nothing.
#[tokio::test]
async fn a_create_under_a_taken_application_name_is_refused_and_writes_nothing() {
    for (case, name) in colliding_names() {
        // One account per row: the name's scope is the account, so each row starts from an
        // account that holds nothing.
        let router = router();
        let original = create_ok(
            &router,
            case,
            &json!({ "Name": name, "Description": "the original" }),
        )
        .await;
        let record = get(&router, &original).await.json();

        // Rows differ along what ELSE the colliding request carries: the conflict is on the
        // name alone.
        let collisions: Vec<(String, Value)> = vec![
            (format!("{case}, the name alone"), json!({ "Name": name })),
            (
                format!("{case}, a different description entirely"),
                json!({ "Name": name, "Description": "a different description entirely" }),
            ),
            (
                format!("{case}, the empty description"),
                json!({ "Name": name, "Description": "" }),
            ),
            (
                format!("{case}, with tags"),
                json!({ "Name": name, "Tags": { "team": "platform" } }),
            ),
            (
                format!("{case}, the identical request"),
                json!({ "Name": name, "Description": "the original" }),
            ),
        ];

        for (row, body) in &collisions {
            let answer = create(&router, body).await;
            assert_name_taken(row, &answer, &name, &original);

            // Nothing was written: the account still holds exactly the one application,
            // under that name, with the record it had.
            let ids = listed_ids(&router).await;
            assert_eq!(
                ids,
                vec![original.clone()],
                "case {row:?}: the refused create must leave exactly the original \
                 application, got {ids:?}"
            );
            let after = get(&router, &original).await.json();
            assert_eq!(
                after, record,
                "case {row:?}: the refused create must not touch the stored record \
                 {record}, got {after}"
            );
        }
    }
}

/// Only the duplicate is refused: a name that merely RESEMBLES a taken one is free, and the
/// application created under it is its own record.
#[tokio::test]
async fn a_name_that_only_resembles_a_taken_one_is_still_free() {
    let router = router();
    let taken = "MyMobileApp";
    let original = create_ok(&router, "the original", &json!({ "Name": taken })).await;

    // Rows differ along HOW CLOSE the free name is to the taken one: case, surrounding
    // whitespace, a suffix, a prefix, and one character shorter at the length bound.
    let free: Vec<(&str, String)> = vec![
        ("the same name in another case", taken.to_lowercase()),
        ("the same name upper-cased", taken.to_uppercase()),
        ("the same name with trailing whitespace", format!("{taken} ")),
        ("the same name with leading whitespace", format!(" {taken}")),
        ("the taken name with a suffix", format!("{taken}-2")),
        ("the taken name with a prefix", format!("not-{taken}")),
        (
            "the taken name one character shorter",
            taken[..taken.len() - 1].to_string(),
        ),
    ];

    let mut minted = vec![original.clone()];
    for (case, name) in &free {
        let id = create_ok(&router, case, &json!({ "Name": name })).await;
        assert!(
            !minted.contains(&id),
            "case {case:?}: a free name must mint its own application, got {id:?} again"
        );
        let fetched = get(&router, &id).await;
        assert_eq!(
            fetched.string("Name").as_deref(),
            Some(name.as_str()),
            "case {case:?}: the created application must read back under its own name, got {:?}",
            fetched.string("Name")
        );
        minted.push(id);
    }

    let ids = listed_ids(&router).await;
    assert_eq!(
        ids.len(),
        minted.len(),
        "the account must hold every application created, expected {} entries, got {ids:?}",
        minted.len()
    );
    let still_taken = ids_named(&router, taken).await;
    assert_eq!(
        still_taken,
        vec![original.clone()],
        "the taken name must still address exactly the original application, got \
         {still_taken:?}"
    );
}

/// The same invariant over a rename: `UpdateApplication` onto a name another application
/// holds is refused and patches nothing, while renaming a record to the name it already
/// carries, or to a free one, is served.
#[tokio::test]
async fn a_rename_onto_a_taken_application_name_is_refused_and_patches_nothing() {
    for (case, name) in colliding_names() {
        let router = router();
        let holder = create_ok(&router, case, &json!({ "Name": name })).await;
        let renaming = create_ok(
            &router,
            case,
            &json!({ "Name": "the-one-being-renamed", "Description": "untouched" }),
        )
        .await;
        let record = get(&router, &renaming).await.json();

        // Rows differ along whether the rename also carries a `Description`: the conflict
        // is on the name alone here too.
        for (row, body) in [
            (format!("{case}, renamed onto it"), json!({ "Name": name })),
            (
                format!("{case}, renamed onto it with a new description"),
                json!({ "Name": name, "Description": "rewritten" }),
            ),
        ] {
            let answer = update(&router, &renaming, &body).await;
            assert_name_taken(&row, &answer, &name, &holder);
            let after = get(&router, &renaming).await.json();
            assert_eq!(
                after, record,
                "case {row:?}: the refused rename must patch nothing, expected {record}, got \
                 {after}"
            );
        }

        // Renaming a record to the name it already carries collides with nothing: the
        // holder of that name is itself.
        let itself = update(&router, &holder, &json!({ "Name": name })).await;
        assert_eq!(
            itself.status,
            200,
            "case {case:?}: renaming an application to its own name must be served, got {} / {}",
            itself.status,
            String::from_utf8_lossy(&itself.body)
        );

        // A rename to a free name is served, and the name it gives up becomes free: the
        // invariant is over the names applications CURRENTLY hold, not over names ever used.
        let released = update(&router, &holder, &json!({ "Name": "moved-along" })).await;
        assert_eq!(
            released.status,
            200,
            "case {case:?}: a rename to a free name must be served, got {} / {}",
            released.status,
            String::from_utf8_lossy(&released.body)
        );
        let reused = create(&router, &json!({ "Name": name })).await;
        assert_eq!(
            reused.status,
            201,
            "case {case:?}: the released name must be free again, got {} / {}",
            reused.status,
            String::from_utf8_lossy(&reused.body)
        );
        assert_ne!(
            reused.string("Id"),
            Some(holder.clone()),
            "case {case:?}: the reused name must address a new application, not {holder:?}"
        );
        let holding = ids_named(&router, &name).await;
        assert_eq!(
            holding.len(),
            1,
            "case {case:?}: exactly one application may hold {name:?}, got {holding:?}"
        );
    }
}
