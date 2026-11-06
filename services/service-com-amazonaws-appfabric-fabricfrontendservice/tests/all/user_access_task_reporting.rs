// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what AppFabric's user-access-task operations REPORT for
//! something they could not do, driven over the wire (restJson1) through the
//! same router the emulator boots.
//!
//! Two rules, each asserted over a table of inputs rather than one recorded
//! call:
//!
//! * `StartUserAccessTasks` reports ONE item per application tenant of the
//!   named app bundle, and a tenant it holds no authentication token for carries
//!   the service's FIXED `error` (code and whole sentence) with no `taskId`, so
//!   the sentence is the same whichever application, tenant or address the item
//!   is for;
//! * `BatchGetUserAccessTasks` is a BATCH: an id the named bundle holds no
//!   record for is ONE ENTRY's outcome, answered in place with that id and the
//!   service's `taskError`, while every other entry still reports its task and
//!   the call still answers HTTP 200.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` builds, and the set-up helpers and the
//! expected wording are `user_access_tasks`'.

use crate::app_bundle_ingestion_lifecycle::{is_v4_uuid, Emulator};
use crate::user_access_tasks::{
    app_bundle, authorization, member, no_task_record_message, poll_tasks, result_items,
    start_tasks, task_items, NO_AUTHENTICATION_TOKEN_CODE, NO_AUTHENTICATION_TOKEN_MESSAGE,
    NO_TASK_RECORD_CODE, Setup,
};

/// A `UUID`-shaped task id no start call ever minted, built from `seed` so one
/// case can name several distinct ones and name the same one twice. The form is
/// the model's `UUID` pattern
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), so the
/// request is accepted and the answer is about the id, not its shape.
fn unminted_task_id(seed: u32) -> String {
    format!("{seed:08x}-dead-4bee-8fff-ffffffffffff")
}

/// The modelled bound on one poll's `taskIdList` (`smithy_query(shape=TaskIdList)`:
/// `@length(min: 1, max: 50)`).
const MAX_TASK_IDS_PER_POLL: usize = 50;

/// One row of the no-token table: the case name, the bundle's application
/// tenants as (app, tenant), and the address the start call searches for.
type NoTokenCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

/// RULE: `StartUserAccessTasks` reports one item per application tenant of the
/// named app bundle, and a tenant for which it holds no authentication token for
/// the user is reported with the service's FIXED error — one code and one whole
/// sentence, carrying no value of the request or of the store — and no `taskId`,
/// so a caller is told the user still has to authorize instead of being handed a
/// shorter list.
///
/// The table spans the application, the tenant identifier, the searched address
/// and the number of such tenants in one bundle, over applications from the
/// service's own supported set (`state::SUPPORTED_APPS`), including ones the
/// issue did not cite.
#[tokio::test]
async fn a_tenant_with_no_authentication_token_reports_the_services_fixed_error() {
    // (case, the bundle's not-yet-authorized tenants as (app, tenant), the
    // searched address)
    let cases: Vec<NoTokenCase<'_>> = vec![
        (
            "one tenant, a plain address",
            &[("SLACK", "tenant-slack")],
            "ada.lovelace@example.com",
        ),
        (
            "another application, a long tenant identifier",
            &[("SMARTSHEET", "0123456789-tenant-identifier-0123456789")],
            "grace.hopper@example.net",
        ),
        (
            "two applications sharing one tenant identifier",
            &[("ASANA", "shared-tenant"), ("ZENDESK", "shared-tenant")],
            "alan.turing@example.org",
        ),
        (
            "one application, two tenants, an address carrying the punctuation the model allows",
            &[("JIRA", "tenant-a"), ("JIRA", "tenant-b")],
            "a.b+c_d-e!#$%&*@example.com",
        ),
        (
            "three tenants",
            &[
                ("OKTA", "tenant-okta"),
                ("CISCO", "tenant-cisco"),
                ("M365", "tenant-m365"),
            ],
            "ada@example.com",
        ),
    ];

    for (case, tenants, email) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        for (app, tenant) in tenants {
            authorization(&emulator, &bundle, app, tenant, false).await;
        }

        let started = start_tasks(&emulator, &bundle, email).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: StartUserAccessTasks must answer HTTP 201, got {} with body {}",
            started.status, started.body
        );
        let items = task_items(&started);
        assert_eq!(
            items.len(),
            tenants.len(),
            "case {case:?}: expected one item per application tenant ({}), got {items:?}",
            tenants.len()
        );
        for (index, (app, tenant)) in tenants.iter().enumerate() {
            let item = &items[index];
            assert_eq!(
                (member(item, "app"), member(item, "tenantId")),
                (app.to_string(), tenant.to_string()),
                "case {case:?}: item {index} must report the tenant it is about, got {item}"
            );
            assert!(
                item.get("taskId").is_none(),
                "case {case:?}: item {index} started no task, so it must carry no taskId, got {item}"
            );
            let error = item
                .get("error")
                .unwrap_or_else(|| panic!("case {case:?}: expected an error in item {index}: {item}"));
            assert_eq!(
                (member(error, "errorCode"), member(error, "errorMessage")),
                (
                    NO_AUTHENTICATION_TOKEN_CODE.to_string(),
                    NO_AUTHENTICATION_TOKEN_MESSAGE.to_string()
                ),
                "case {case:?}: item {index} must carry the service's fixed no-token error, got {item}"
            );
            let message = member(error, "errorMessage");
            for value in [*app, *tenant, email] {
                assert!(
                    !message.contains(value),
                    "case {case:?}: the sentence is fixed, so it must not carry {value:?}, got {message:?}"
                );
            }
        }
    }
}

/// RULE: a tenant whose authorization HAS completed the connect handshake is
/// reported with a task instead of that error, and the two outcomes are reported
/// side by side in one answer — the fan-out reports every application tenant of
/// the bundle, whichever outcome each has. ROUND TRIP: each reported task id is
/// pollable back under the bundle.
#[tokio::test]
async fn one_answer_reports_both_outcomes_for_the_bundles_tenants() {
    // (case, the bundle's tenants as (app, tenant, authorized))
    let cases: Vec<(&str, Setup<'_>)> = vec![
        (
            "authorized first, not-authorized second",
            &[("SLACK", "tenant-slack", true), ("OKTA", "tenant-okta", false)],
        ),
        (
            "not-authorized first, authorized second",
            &[("ZOOM", "tenant-zoom", false), ("GOOGLE", "tenant-google", true)],
        ),
        (
            "not-authorized between two authorized ones",
            &[
                ("DROPBOX", "tenant-dropbox", true),
                ("SMARTSHEET", "tenant-smartsheet", false),
                ("M365AUDITLOGS", "tenant-m365", true),
            ],
        ),
    ];

    for (case, tenants) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        for (app, tenant, authorized) in tenants {
            authorization(&emulator, &bundle, app, tenant, *authorized).await;
        }

        let started = start_tasks(&emulator, &bundle, "ada.lovelace@example.com").await;
        let items = task_items(&started);
        assert_eq!(
            items.len(),
            tenants.len(),
            "case {case:?}: expected one item per application tenant, got {items:?}"
        );
        let mut started_ids: Vec<String> = Vec::new();
        for (index, (app, _, authorized)) in tenants.iter().enumerate() {
            let item = &items[index];
            if *authorized {
                let task_id = member(item, "taskId");
                assert!(
                    is_v4_uuid(&task_id) && item.get("error").is_none(),
                    "case {case:?}: item {index} ({app}) must carry a task id and no error, got {item}"
                );
                started_ids.push(task_id);
            } else {
                assert_eq!(
                    item.get("error").map(|error| member(error, "errorCode")),
                    Some(NO_AUTHENTICATION_TOKEN_CODE.to_string()),
                    "case {case:?}: item {index} ({app}) must carry the no-token error, got {item}"
                );
            }
        }

        let polled = poll_tasks(&emulator, &bundle, &started_ids).await;
        let reported: Vec<String> = result_items(&polled)
            .iter()
            .map(|result| member(result, "taskId"))
            .collect();
        assert_eq!(
            reported, started_ids,
            "case {case:?}: every started task must be pollable back, expected {started_ids:?}, got {reported:?}"
        );
    }
}

/// RULE: `BatchGetUserAccessTasks` answers a batch PER ENTRY: an id the named
/// app bundle holds no record for is that entry's own outcome — the entry
/// carries the requested id and the service's `taskError` (its code, and its
/// fixed text followed by that very id) and nothing else — while every other
/// entry still reports its task, and the call answers HTTP 200 whatever mix of
/// ids it was given.
///
/// The table spans how many ids are unresolvable, where they sit among the
/// resolvable ones, whether either kind repeats, the order they are named in,
/// and the number of entries up to the modelled maximum.
#[tokio::test]
async fn an_id_with_no_record_is_reported_as_its_own_entry() {
    let emulator = Emulator::new();
    let bundle = app_bundle(&emulator).await;
    // Each tenant authorized, so each gets a task to poll beside the ids that
    // name nothing.
    let tenants = [
        ("SLACK", "tenant-mix-1"),
        ("ZENDESK", "tenant-mix-2"),
        ("GOOGLE", "tenant-mix-3"),
    ];
    for (app, tenant) in tenants {
        authorization(&emulator, &bundle, app, tenant, true).await;
    }
    let started = start_tasks(&emulator, &bundle, "ada.lovelace@example.com").await;
    let task_ids: Vec<String> = task_items(&started)
        .iter()
        .map(|item| member(item, "taskId"))
        .collect();
    assert_eq!(
        task_ids.len(),
        tenants.len(),
        "the set-up bundle must hold one task per tenant, got {}",
        started.body
    );

    // Each entry is either one of the bundle's own task ids (`Held(position)`)
    // or an id nothing minted (`Unminted(seed)`).
    #[derive(Clone, Copy, Debug)]
    enum Entry {
        Held(usize),
        Unminted(u32),
    }
    use Entry::{Held, Unminted};

    let cases: Vec<(&str, Vec<Entry>)> = vec![
        ("one unresolvable id alone — the modelled minimum", vec![Unminted(1)]),
        ("the same unresolvable id twice", vec![Unminted(2), Unminted(2)]),
        ("two different unresolvable ids", vec![Unminted(3), Unminted(4)]),
        ("an unresolvable id after a held one", vec![Held(0), Unminted(5)]),
        ("an unresolvable id before a held one", vec![Unminted(6), Held(1)]),
        (
            "an unresolvable id between held ones",
            vec![Held(0), Unminted(7), Held(2)],
        ),
        (
            "unresolvable and held ids interleaved, each repeated",
            vec![
                Unminted(8),
                Held(1),
                Unminted(8),
                Held(1),
                Held(2),
                Unminted(9),
            ],
        ),
        (
            "every held id once, then every unresolvable one",
            vec![
                Held(0),
                Held(1),
                Held(2),
                Unminted(10),
                Unminted(11),
                Unminted(12),
            ],
        ),
        (
            "the modelled maximum, alternating",
            (0..MAX_TASK_IDS_PER_POLL)
                .map(|index| {
                    if index % 2 == 0 {
                        Held(index % 3)
                    } else {
                        Unminted(100 + index as u32)
                    }
                })
                .collect(),
        ),
        (
            "the modelled maximum, every entry unresolvable",
            (0..MAX_TASK_IDS_PER_POLL)
                .map(|index| Unminted(200 + index as u32))
                .collect(),
        ),
    ];

    for (case, entries) in cases {
        let ids: Vec<String> = entries
            .iter()
            .map(|entry| match entry {
                Held(position) => task_ids[*position].clone(),
                Unminted(seed) => unminted_task_id(*seed),
            })
            .collect();
        let answer = poll_tasks(&emulator, &bundle, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch poll must answer HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let results = result_items(&answer);
        assert_eq!(
            results.len(),
            ids.len(),
            "case {case:?}: expected one result per requested entry ({}), got {} results",
            ids.len(),
            results.len()
        );
        for (index, result) in results.iter().enumerate() {
            assert_eq!(
                member(result, "taskId"),
                ids[index],
                "case {case:?}: result {index} must report the id requested at that position, got {result}"
            );
            match entries[index] {
                Held(position) => {
                    let (app, tenant) = tenants[position];
                    assert_eq!(
                        (member(result, "app"), member(result, "tenantId")),
                        (app.to_string(), tenant.to_string()),
                        "case {case:?}: result {index} must report the task's own tenant, got {result}"
                    );
                }
                Unminted(_) => {
                    let task_error = result.get("taskError").unwrap_or_else(|| {
                        panic!("case {case:?}: expected a taskError in result {index}: {result}")
                    });
                    assert_eq!(
                        (
                            member(task_error, "errorCode"),
                            member(task_error, "errorMessage")
                        ),
                        (
                            NO_TASK_RECORD_CODE.to_string(),
                            no_task_record_message(&ids[index])
                        ),
                        "case {case:?}: result {index} must report the no-record taskError for the id asked for, got {result}"
                    );
                    // Nothing is invented for a task that does not exist.
                    for absent in [
                        "app",
                        "tenantId",
                        "tenantDisplayName",
                        "resultStatus",
                        "email",
                        "userId",
                        "userFullName",
                        "userFirstName",
                        "userLastName",
                        "userStatus",
                    ] {
                        assert!(
                            result.get(absent).is_none(),
                            "case {case:?}: result {index} names no task, so it must report no {absent}, got {result}"
                        );
                    }
                }
            }
        }
    }

    // A poll is a REPORT: an unresolvable id is not a record, so asking for it
    // never makes one, and the bundle's own tasks are untouched by every call
    // above.
    let after = poll_tasks(&emulator, &bundle, &task_ids).await;
    let reported: Vec<String> = result_items(&after)
        .iter()
        .map(|result| member(result, "taskId"))
        .collect();
    assert_eq!(
        reported, task_ids,
        "polling must leave exactly the started tasks pollable, expected {task_ids:?}, got {reported:?}"
    );
    let repeat = poll_tasks(&emulator, &bundle, &[unminted_task_id(1)]).await;
    assert_eq!(
        repeat.status, 200,
        "an unresolvable id must still be answered per entry on a later poll, got {} with body {}",
        repeat.status, repeat.body
    );
    assert_eq!(
        result_items(&repeat)
            .first()
            .and_then(|result| result.get("taskError").map(|error| member(error, "errorCode"))),
        Some(NO_TASK_RECORD_CODE.to_string()),
        "a later poll of the same unresolvable id must report the same taskError, got {}",
        repeat.body
    );
}

/// RULE: a bundle the store does not hold is the one thing a poll refuses, with
/// the modelled `ResourceNotFoundException` (HTTP 404) — the per-entry
/// `taskError` above is for an id INSIDE a bundle that exists, so a poll naming
/// an unknown bundle answers no result list at all, whatever its ids are.
#[tokio::test]
async fn an_unknown_bundle_is_still_refused_whatever_the_ids_are() {
    let emulator = Emulator::new();
    let held = app_bundle(&emulator).await;
    authorization(&emulator, &held, "SLACK", "tenant-held", true).await;
    let started = start_tasks(&emulator, &held, "ada.lovelace@example.com").await;
    let task_id = task_items(&started)
        .first()
        .map(|item| member(item, "taskId"))
        .unwrap_or_else(|| {
            panic!(
                "the set-up start call must report one started task, got {}",
                started.body
            )
        });
    assert!(
        is_v4_uuid(&task_id),
        "the set-up start call must report a UUID-shaped taskId, got {task_id:?}"
    );

    // (case, the ids the poll names under a bundle the store does not hold)
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("an id nothing minted", vec![unminted_task_id(1)]),
        (
            "an id another bundle does hold",
            vec![task_id.clone()],
        ),
        (
            "both kinds at once",
            vec![task_id.clone(), unminted_task_id(2)],
        ),
    ];
    let unknown_bundle = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    for (case, ids) in cases {
        let answer = poll_tasks(&emulator, unknown_bundle, &ids).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown bundle must be refused HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must be the modelled ResourceNotFoundException, got {:?} with body {}",
            answer.error_code(),
            answer.body
        );
        assert!(
            answer.body.get("userAccessResultsList").is_none(),
            "case {case:?}: a refused poll must answer no partial result list, got {}",
            answer.body
        );
    }

    // The bundle that does exist still answers its own task.
    let polled = poll_tasks(&emulator, &held, std::slice::from_ref(&task_id)).await;
    assert_eq!(
        result_items(&polled)
            .first()
            .map(|result| member(result, "taskId")),
        Some(task_id),
        "the held bundle's task must still be pollable, got {}",
        polled.body
    );
}
