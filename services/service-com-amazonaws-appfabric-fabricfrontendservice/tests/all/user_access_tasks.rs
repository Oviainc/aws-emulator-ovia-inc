// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppFabric's user access tasks (`StartUserAccessTasks`
//! and the `BatchGetUserAccessTasks` poll that reads them back), driven over the
//! wire (restJson1) through the same router the emulator boots.
//!
//! Each test states a RULE over a table of inputs rather than one recorded call:
//!
//! * a start call fans out over the app bundle's app AUTHORIZATIONS — one item
//!   per authorization, in creation order, carrying a task id when the
//!   authorization has completed the connect handshake and the modelled `error`
//!   when it has not;
//! * every started task is pollable under its bundle by its task id, reporting
//!   the authorization's (`app`, tenant) pair and the start call's email;
//! * a task is `IN_PROGRESS` until the span the clock owes has passed and
//!   terminal afterwards, and polling it never changes what a later poll sees;
//! * a poll answers POSITIONALLY — one result per ENTRY of `taskIdList`, in
//!   request order, repeats included, because the model declares that list
//!   without `@uniqueItems`;
//! * a task id the named bundle holds no record for is ONE ENTRY's outcome —
//!   the entry carries that id and the service's `taskError` and the poll still
//!   answers 200 (see `user_access_task_reporting` for the whole class);
//! * an app bundle the store does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404), in either `Identifier` form.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, is_v4_uuid, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The email address every start call below searches for. One constant, so a
/// poll reporting `email` can only have read it back out of the started task.
const EMAIL: &str = "ada.lovelace@example.com";

/// The `errorCode` AppFabric puts on a start call's item for an application
/// tenant it holds no authentication token for.
pub(crate) const NO_AUTHENTICATION_TOKEN_CODE: &str = "notfound";

/// The whole `errorMessage` of that item: FIXED text, the same whichever
/// application, tenant or email the item is for.
pub(crate) const NO_AUTHENTICATION_TOKEN_MESSAGE: &str =
    "No authentication token was found for this user. User needs to authorize.";

/// The `errorCode` of the per-entry `taskError` a poll reports for a task id the
/// named app bundle holds no record for.
pub(crate) const NO_TASK_RECORD_CODE: &str = "404";

/// The fixed text of that `taskError`'s message, which the requested id follows.
pub(crate) const NO_TASK_RECORD_MESSAGE_PREFIX: &str =
    "Couldn't find a record within current appBundle with the given taskId: ";

/// The whole message a poll reports for the unresolvable id `task_id`.
pub(crate) fn no_task_record_message(task_id: &str) -> String {
    format!("{NO_TASK_RECORD_MESSAGE_PREFIX}{task_id}")
}

/// The values the model's `ResultStatus` enum carries
/// (`RESULT_STATUS_VALUES` in the generated types), so a reported status can be
/// checked against the modelled set rather than against one expected value.
const RESULT_STATUS_VALUES: &[&str] = &["COMPLETED", "EXPIRED", "FAILED", "IN_PROGRESS"];

/// Long enough for the span the emulator makes a task's non-terminal status
/// settle after (`state::USER_ACCESS_TASK_SETTLES_AFTER`, 500 ms) to have
/// passed.
const PAST_THE_SETTLE_SPAN: std::time::Duration = std::time::Duration::from_millis(900);

/// An app bundle to hang authorizations off, as its ARN.
pub(crate) async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// The display name this suite gives the tenant of `app`, distinct from the
/// tenant identifier so a poll reporting `tenantDisplayName` cannot be reporting
/// the identifier instead.
pub(crate) fn display_name(tenant: &str) -> String {
    format!("{tenant} (display)")
}

/// Create one app authorization under `bundle`, and connect it when `connected`.
/// Its ARN is returned so a caller can assert against the authorization it set
/// up.
pub(crate) async fn authorization(emulator: &Emulator, bundle: &str, app: &str, tenant: &str, connected: bool) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": "apiKey",
                "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": display_name(tenant)},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the set-up app authorization for {app}/{tenant} must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let arn = created
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected appAuthorization.appAuthorizationArn in {}", created.body))
        .to_string();
    if connected {
        let connect = emulator
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/appauthorizations/{}/connect",
                    encode(bundle),
                    encode(&arn)
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            connect.status, 200,
            "the set-up app authorization for {app}/{tenant} must connect, got HTTP {} with body {}",
            connect.status, connect.body
        );
    }
    arn
}

/// `POST /useraccess/start`.
pub(crate) async fn start_tasks(emulator: &Emulator, bundle: &str, email: &str) -> Answer {
    emulator
        .call(
            "POST",
            "/useraccess/start",
            Some(json!({"appBundleIdentifier": bundle, "email": email})),
        )
        .await
}

/// `POST /useraccess/batchget`.
pub(crate) async fn poll_tasks(emulator: &Emulator, bundle: &str, task_ids: &[String]) -> Answer {
    emulator
        .call(
            "POST",
            "/useraccess/batchget",
            Some(json!({"appBundleIdentifier": bundle, "taskIdList": task_ids})),
        )
        .await
}

/// The `userAccessTasksList` of a start response.
pub(crate) fn task_items(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("userAccessTasksList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected userAccessTasksList in {}", answer.body))
        .clone()
}

/// The `userAccessResultsList` of a poll response.
pub(crate) fn result_items(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("userAccessResultsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected userAccessResultsList in {}", answer.body))
        .clone()
}

/// A string member of a JSON object, or `""`.
pub(crate) fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The applications and tenants one bundle is set up with, and whether each
/// authorization is connected.
pub(crate) type Setup<'a> = &'a [(&'a str, &'a str, bool)];

/// RULE: `StartUserAccessTasks` reports exactly one item per app authorization
/// the named app bundle holds, in the order the authorizations were created,
/// carrying that authorization's `app` and tenant id. An authorization that has
/// completed the connect handshake gets a task started for it (a distinct
/// `UUID`-shaped `taskId`, no `error`); one that has not holds no authentication
/// token for the user, so it is reported with the service's fixed `error` and NO
/// `taskId`. A bundle holding no authorizations starts no tasks.
///
/// The table spans the number of authorizations (0, 1, 2, 3), which applications
/// they are for, whether two share one application or one tenant, and every
/// connected/not-connected mix — including applications the issue did not cite.
#[tokio::test]
async fn a_start_call_reports_one_item_per_app_authorization_in_creation_order() {
    // (case, the bundle's authorizations as (app, tenant, connected))
    let cases: Vec<(&str, Setup<'_>)> = vec![
        ("no authorizations", &[]),
        ("one connected authorization", &[("SLACK", "tenant-slack", true)]),
        ("one authorization still PendingConnect", &[("OKTA", "tenant-okta", false)]),
        (
            "two connected authorizations for two applications",
            &[("ZOOM", "tenant-zoom", true), ("GOOGLE", "tenant-google", true)],
        ),
        (
            "one application, two tenants",
            &[("JIRA", "tenant-a", true), ("JIRA", "tenant-b", true)],
        ),
        (
            "one tenant id, two applications",
            &[("ASANA", "shared-tenant", true), ("ZENDESK", "shared-tenant", true)],
        ),
        (
            "connected and pending mixed",
            &[
                ("DROPBOX", "tenant-dropbox", true),
                ("CISCO", "tenant-cisco", false),
                ("M365", "tenant-m365", true),
            ],
        ),
    ];

    for (case, setup) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        for (app, tenant, connected) in setup {
            authorization(&emulator, &bundle, app, tenant, *connected).await;
        }

        let started = start_tasks(&emulator, &bundle, EMAIL).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: StartUserAccessTasks must answer HTTP 201, got {} with body {}",
            started.status, started.body
        );
        let items = task_items(&started);
        assert_eq!(
            items.len(),
            setup.len(),
            "case {case:?}: expected one item per app authorization ({}), got {items:?}",
            setup.len()
        );

        let mut task_ids: Vec<String> = Vec::new();
        for (index, (app, tenant, connected)) in setup.iter().enumerate() {
            let item = &items[index];
            assert_eq!(
                member(item, "app"),
                *app,
                "case {case:?}: item {index} must report app {app:?}, got {item}"
            );
            assert_eq!(
                member(item, "tenantId"),
                *tenant,
                "case {case:?}: item {index} must report tenantId {tenant:?}, got {item}"
            );
            if *connected {
                let task_id = member(item, "taskId");
                assert!(
                    is_v4_uuid(&task_id),
                    "case {case:?}: item {index} must carry a UUID-shaped taskId, got {item}"
                );
                assert!(
                    item.get("error").is_none(),
                    "case {case:?}: item {index} started a task, so it must carry no error, got {item}"
                );
                task_ids.push(task_id);
            } else {
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
                    "case {case:?}: item {index} started no task, so it must carry the service's fixed no-token error, got {item}"
                );
            }
        }

        let distinct: BTreeSet<&String> = task_ids.iter().collect();
        assert_eq!(
            distinct.len(),
            task_ids.len(),
            "case {case:?}: every started task must have its own id, got {task_ids:?}"
        );

        // ROUND TRIP: every started task is pollable under its bundle, and the
        // poll reports the authorization's pair and this call's email.
        if task_ids.is_empty() {
            continue;
        }
        let polled = poll_tasks(&emulator, &bundle, &task_ids).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: BatchGetUserAccessTasks must answer HTTP 200, got {} with body {}",
            polled.status, polled.body
        );
        let results = result_items(&polled);
        assert_eq!(
            results.len(),
            task_ids.len(),
            "case {case:?}: expected one result per polled task, got {results:?}"
        );
        let connected_pairs: Vec<(&str, &str)> = setup
            .iter()
            .filter(|(_, _, connected)| *connected)
            .map(|(app, tenant, _)| (*app, *tenant))
            .collect();
        for (index, result) in results.iter().enumerate() {
            let (app, tenant) = connected_pairs[index];
            assert_eq!(
                member(result, "taskId"),
                task_ids[index],
                "case {case:?}: result {index} must be the polled task, got {result}"
            );
            assert_eq!(
                member(result, "app"),
                app,
                "case {case:?}: result {index} must report app {app:?}, got {result}"
            );
            assert_eq!(
                member(result, "tenantId"),
                tenant,
                "case {case:?}: result {index} must report tenantId {tenant:?}, got {result}"
            );
            assert_eq!(
                member(result, "tenantDisplayName"),
                display_name(tenant),
                "case {case:?}: result {index} must report the authorization's tenant display name, got {result}"
            );
            assert_eq!(
                member(result, "email"),
                EMAIL,
                "case {case:?}: result {index} must report the searched email, got {result}"
            );
            assert!(
                RESULT_STATUS_VALUES.contains(&member(result, "resultStatus").as_str()),
                "case {case:?}: result {index} must report a modelled ResultStatus, got {result}"
            );
        }
    }
}

/// RULE: a user access task's status is the one the CLOCK owes it: `IN_PROGRESS`
/// until the span past the start call has elapsed, and terminal from then on —
/// and because the emulator makes no outbound connection to the application, the
/// terminal status is the modelled `FAILED` carrying a `taskError` that says so,
/// never a user record nothing looked up. Polling is a REPORT: two polls of the
/// same task report the same thing, and no poll moves the task.
///
/// The table spans the number of tasks started in one call and the application
/// they are for, so the rule is not keyed to a single task.
#[tokio::test]
async fn a_task_is_in_progress_until_the_clock_settles_it_and_then_reports_its_failure() {
    let cases: Vec<(&str, Setup<'_>)> = vec![
        ("one task", &[("SMARTSHEET", "tenant-smartsheet", true)]),
        (
            "three tasks",
            &[
                ("SLACK", "tenant-1", true),
                ("ZOOM", "tenant-2", true),
                ("M365AUDITLOGS", "tenant-3", true),
            ],
        ),
    ];

    for (case, setup) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        for (app, tenant, connected) in setup {
            authorization(&emulator, &bundle, app, tenant, *connected).await;
        }

        let started = start_tasks(&emulator, &bundle, EMAIL).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: StartUserAccessTasks must answer HTTP 201, got {} with body {}",
            started.status, started.body
        );
        let task_ids: Vec<String> = task_items(&started)
            .iter()
            .map(|item| member(item, "taskId"))
            .collect();

        // The settle span is non-zero, so a caller polling straight after the
        // start call still sees the non-terminal status.
        let immediately = poll_tasks(&emulator, &bundle, &task_ids).await;
        for (index, result) in result_items(&immediately).iter().enumerate() {
            assert_eq!(
                member(result, "resultStatus"),
                "IN_PROGRESS",
                "case {case:?}: task {index} must still be IN_PROGRESS right after the start call, got {result}"
            );
            assert!(
                result.get("taskError").is_none(),
                "case {case:?}: task {index} has not terminated, so it must carry no taskError, got {result}"
            );
        }

        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

        let settled = poll_tasks(&emulator, &bundle, &task_ids).await;
        let results = result_items(&settled);
        assert_eq!(
            results.len(),
            task_ids.len(),
            "case {case:?}: expected one result per polled task, got {results:?}"
        );
        for (index, result) in results.iter().enumerate() {
            assert_eq!(
                member(result, "resultStatus"),
                "FAILED",
                "case {case:?}: task {index} must have terminated FAILED, got {result}"
            );
            let task_error = result
                .get("taskError")
                .unwrap_or_else(|| panic!("case {case:?}: expected a taskError in {result}"));
            assert!(
                !member(task_error, "errorCode").is_empty()
                    && !member(task_error, "errorMessage").is_empty(),
                "case {case:?}: task {index} must say why it failed, got {result}"
            );
            // No user member is reported: nothing searched the application for
            // one.
            for absent in [
                "userId",
                "userFullName",
                "userFirstName",
                "userLastName",
                "userStatus",
            ] {
                assert!(
                    result.get(absent).is_none(),
                    "case {case:?}: task {index} must report no {absent}, got {result}"
                );
            }
        }

        // A poll is a report, not a step: the next one says the same thing.
        let again = poll_tasks(&emulator, &bundle, &task_ids).await;
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: polling must not change what a later poll sees, got {} then {}",
            settled.body, again.body
        );
    }
}

/// RULE: both operations resolve their `appBundleIdentifier` in either form the
/// model's `Identifier` accepts (an ARN or the bare UUID), and both are the same
/// bundle: a task started under one form is pollable under the other.
#[tokio::test]
async fn both_identifier_forms_name_the_same_bundle() {
    for (case, start_form, poll_form) in [
        ("start by ARN, poll by UUID", "arn", "uuid"),
        ("start by UUID, poll by ARN", "uuid", "arn"),
    ] {
        let emulator = Emulator::new();
        let arn = app_bundle(&emulator).await;
        let uuid = uuid_of(&arn);
        let form = |which: &str| -> String {
            match which {
                "arn" => arn.clone(),
                _ => uuid.clone(),
            }
        };
        authorization(&emulator, &arn, "SLACK", "tenant-forms", true).await;

        let started = start_tasks(&emulator, &form(start_form), EMAIL).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: StartUserAccessTasks must answer HTTP 201, got {} with body {}",
            started.status, started.body
        );
        let task_ids: Vec<String> = task_items(&started)
            .iter()
            .map(|item| member(item, "taskId"))
            .collect();
        assert_eq!(
            task_ids.len(),
            1,
            "case {case:?}: expected one started task, got {task_ids:?}"
        );

        let polled = poll_tasks(&emulator, &form(poll_form), &task_ids).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: the task must be pollable under the other form, got HTTP {} with body {}",
            polled.status, polled.body
        );
        assert_eq!(
            result_items(&polled)
                .first()
                .map(|result| member(result, "taskId")),
            Some(task_ids[0].clone()),
            "case {case:?}: expected the started task back, got {}",
            polled.body
        );
    }
}

/// RULE: an app bundle the store does not hold is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404) reporting the identifier the caller
/// supplied, by both operations and in either `Identifier` form — and the
/// refused start call persists nothing.
#[tokio::test]
async fn an_unknown_app_bundle_is_refused_by_both_operations() {
    let missing_uuid = unknown_uuid();
    let identifiers = [
        ("the bare UUID form", missing_uuid.clone()),
        ("the ARN form", unknown_bundle_arn(&missing_uuid)),
    ];

    for (case, identifier) in identifiers {
        let emulator = Emulator::new();
        // A bundle that DOES exist, so the refusal is about the one named.
        let existing = app_bundle(&emulator).await;
        authorization(&emulator, &existing, "SLACK", "tenant-existing", true).await;

        for (operation, answer) in [
            (
                "StartUserAccessTasks",
                start_tasks(&emulator, &identifier, EMAIL).await,
            ),
            (
                "BatchGetUserAccessTasks",
                poll_tasks(&emulator, &identifier, &[unknown_uuid()]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {operation} must answer HTTP 404 for an unknown bundle, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: {operation} must answer a ResourceNotFoundException, got {:?} with body {}",
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                member(&answer.body, "resourceId"),
                identifier,
                "case {case:?}: {operation} must report the identifier it refused, got {}",
                answer.body
            );
            assert_eq!(
                member(&answer.body, "resourceType"),
                "AppBundle",
                "case {case:?}: {operation} must report the resource kind it refused, got {}",
                answer.body
            );
        }

        // The refused start call wrote nothing: the bundle that does exist has
        // no task, so polling a fresh start under it is what starts the first.
        let started = start_tasks(&emulator, &existing, EMAIL).await;
        assert_eq!(
            task_items(&started).len(),
            1,
            "case {case:?}: the existing bundle must still hold exactly its one authorization, got {}",
            started.body
        );
    }
}

/// The bound the model puts on one poll's `taskIdList`
/// (`smithy_query(shape=TaskIdList)`: a `list` of `UUID` with
/// `@length(min: 1, max: 50)` and NO `@uniqueItems`).
const MAX_TASK_IDS_PER_POLL: usize = 50;
const MIN_TASK_IDS_PER_POLL: usize = 1;

/// RULE: `BatchGetUserAccessTasks` answers POSITIONALLY — exactly one
/// `UserAccessResultItem` per ENTRY of the requested id list, in request order,
/// repeats included — because the model declares that list as a plain sequence
/// (no `@uniqueItems`), so the caller lines the answer up against the ids it
/// submitted. Outside the modelled length bound the request is refused with
/// HTTP 400 and no partial answer.
///
/// The table spans the number of entries (1 up to the published maximum), how
/// many distinct ids they name, how often each repeats, where the repeats sit,
/// and the order the ids are named in — plus each published bound and one past
/// it.
#[tokio::test]
async fn a_poll_answers_one_result_per_requested_entry_in_request_order() {
    let emulator = Emulator::new();
    let bundle = app_bundle(&emulator).await;
    // Applications from the model's own supported set, including ones the issue
    // did not cite, each with its own tenant so each gets its own task.
    let setup: Setup<'_> = &[
        ("SLACK", "tenant-pos-1", true),
        ("ZENDESK", "tenant-pos-2", true),
        ("SMARTSHEET", "tenant-pos-3", true),
        ("GOOGLE", "tenant-pos-4", true),
    ];
    for (app, tenant, connected) in setup {
        authorization(&emulator, &bundle, app, tenant, *connected).await;
    }
    let started = start_tasks(&emulator, &bundle, EMAIL).await;
    let task_ids: Vec<String> = task_items(&started)
        .iter()
        .map(|item| member(item, "taskId"))
        .collect();
    assert_eq!(
        task_ids.len(),
        setup.len(),
        "the set-up bundle must hold one task per authorization, got {}",
        started.body
    );

    // (case, the positions of `task_ids` the poll names, in request order)
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("one id once — the modelled minimum", vec![0; MIN_TASK_IDS_PER_POLL]),
        ("one id twice", vec![0, 0]),
        ("another id three times", vec![1, 1, 1]),
        ("every id once, in start order", vec![0, 1, 2, 3]),
        ("every id once, reversed", vec![3, 2, 1, 0]),
        ("a repeat at the end", vec![0, 1, 1]),
        ("a repeat at the front", vec![2, 2, 3]),
        ("repeats interleaved with other ids", vec![2, 0, 2, 1, 0]),
        ("every id twice, pairwise", vec![0, 0, 1, 1, 2, 2, 3, 3]),
        (
            "one id filling the modelled maximum",
            vec![3; MAX_TASK_IDS_PER_POLL],
        ),
        (
            "every id cycled up to the modelled maximum",
            (0..MAX_TASK_IDS_PER_POLL).map(|index| index % 4).collect(),
        ),
    ];

    for (case, positions) in cases {
        let ids: Vec<String> = positions
            .iter()
            .map(|position| task_ids[*position].clone())
            .collect();
        let answer = poll_tasks(&emulator, &bundle, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
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
            let position = positions[index];
            assert_eq!(
                member(result, "taskId"),
                task_ids[position],
                "case {case:?}: result {index} must report the id requested at that position, got {result}"
            );
            // Each entry is answered out of the task that id names, so a
            // repeated id reports that task's own authorization every time.
            let (app, tenant, _) = setup[position];
            assert_eq!(
                member(result, "app"),
                app,
                "case {case:?}: result {index} must report app {app:?}, got {result}"
            );
            assert_eq!(
                member(result, "tenantId"),
                tenant,
                "case {case:?}: result {index} must report tenantId {tenant:?}, got {result}"
            );
            assert!(
                RESULT_STATUS_VALUES.contains(&member(result, "resultStatus").as_str()),
                "case {case:?}: result {index} must report a modelled ResultStatus, got {result}"
            );
        }
    }

    // One past each published LENGTH bound: the list itself is outside what the
    // model accepts, so nothing of it is answered. (An id the bundle holds no
    // record for is NOT a refusal — see `user_access_task_reporting`.)
    let refused: Vec<(&str, Vec<String>, u16)> = vec![
        ("no entries at all — below the modelled minimum", Vec::new(), 400),
        (
            "one entry past the modelled maximum",
            vec![task_ids[0].clone(); MAX_TASK_IDS_PER_POLL + 1],
            400,
        ),
    ];
    for (case, ids, expected) in refused {
        let answer = poll_tasks(&emulator, &bundle, &ids).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.body.get("userAccessResultsList").is_none(),
            "case {case:?}: a refused poll must answer no partial result list, got {}",
            answer.body
        );
    }

    // A poll is a REPORT: after every call above, the tasks are still pollable
    // and still report what they did.
    let after = poll_tasks(&emulator, &bundle, &task_ids).await;
    let reported: Vec<String> = result_items(&after)
        .iter()
        .map(|result| member(result, "taskId"))
        .collect();
    assert_eq!(
        reported, task_ids,
        "polling must leave every task pollable, expected {task_ids:?}, got {reported:?}"
    );
}

/// RULE: a poll resolves a task id only within the bundle that holds it, so an
/// id no start call minted — and an id minted under ANOTHER bundle — is reported
/// as an entry carrying that id and the service's "no record in this appBundle"
/// `taskError`, with no task of the other bundle leaking into the answer. A poll
/// of ids the bundle does hold reports one result per ENTRY of the requested
/// list, in request order.
#[tokio::test]
async fn a_task_id_resolves_only_within_its_own_bundle() {
    let emulator = Emulator::new();
    // Two bundles at once, so each is created in its own Region: an account
    // holds one app bundle per Region.
    let first = emulator.fixture_bundle(0).await;
    let second = emulator.fixture_bundle(1).await;
    authorization(&emulator, &first, "SLACK", "tenant-first", true).await;
    authorization(&emulator, &first, "ZOOM", "tenant-first-2", true).await;
    authorization(&emulator, &second, "JIRA", "tenant-second", true).await;

    let first_ids: Vec<String> = task_items(&start_tasks(&emulator, &first, EMAIL).await)
        .iter()
        .map(|item| member(item, "taskId"))
        .collect();
    let second_ids: Vec<String> = task_items(&start_tasks(&emulator, &second, EMAIL).await)
        .iter()
        .map(|item| member(item, "taskId"))
        .collect();
    assert_eq!(
        (first_ids.len(), second_ids.len()),
        (2, 1),
        "the set-up bundles must hold 2 and 1 tasks, got {first_ids:?} and {second_ids:?}"
    );

    // (case, the ids polled under the FIRST bundle, the ids that bundle holds no
    // record for)
    let unresolvable: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "an id no start call minted",
            vec![unknown_uuid()],
            Vec::new(),
        ),
        (
            "an id minted under another bundle",
            vec![second_ids[0].clone()],
            vec![second_ids[0].clone()],
        ),
        (
            "one of this bundle's ids together with another bundle's",
            vec![first_ids[0].clone(), second_ids[0].clone()],
            vec![second_ids[0].clone()],
        ),
    ];
    for (case, ids, from_other_bundle) in unresolvable {
        let answer = poll_tasks(&emulator, &first, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let results = result_items(&answer);
        assert_eq!(
            results.len(),
            ids.len(),
            "case {case:?}: expected one result per requested entry ({}), got {results:?}",
            ids.len()
        );
        for (index, result) in results.iter().enumerate() {
            assert_eq!(
                member(result, "taskId"),
                ids[index],
                "case {case:?}: result {index} must report the id requested at that position, got {result}"
            );
            let held_here = first_ids.contains(&ids[index]);
            let task_error = result.get("taskError").map(|error| {
                (member(error, "errorCode"), member(error, "errorMessage"))
            });
            if held_here {
                assert_eq!(
                    member(result, "tenantId"),
                    "tenant-first",
                    "case {case:?}: result {index} must report this bundle's own task, got {result}"
                );
            } else {
                assert_eq!(
                    task_error,
                    Some((
                        NO_TASK_RECORD_CODE.to_string(),
                        no_task_record_message(&ids[index])
                    )),
                    "case {case:?}: result {index} must report the no-record taskError for {:?}, got {result}",
                    ids[index]
                );
                // Nothing of the OTHER bundle's task leaks into this answer.
                for absent in ["app", "tenantId", "tenantDisplayName", "resultStatus", "email"] {
                    assert!(
                        result.get(absent).is_none(),
                        "case {case:?}: result {index} names no task here, so it must report no {absent}, got {result}"
                    );
                }
            }
        }
        // The other bundle's own poll is unaffected: its task is still its own.
        if !from_other_bundle.is_empty() {
            let elsewhere = poll_tasks(&emulator, &second, &from_other_bundle).await;
            assert_eq!(
                result_items(&elsewhere)
                    .first()
                    .map(|result| member(result, "tenantId")),
                Some("tenant-second".to_string()),
                "case {case:?}: the id must still resolve under the bundle that holds it, got {}",
                elsewhere.body
            );
        }
    }

    // (case, the ids polled, the ids the poll must report in order)
    let accepted: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one id",
            vec![first_ids[0].clone()],
            vec![first_ids[0].clone()],
        ),
        ("both ids", first_ids.clone(), first_ids.clone()),
        (
            "both ids in the other order",
            vec![first_ids[1].clone(), first_ids[0].clone()],
            vec![first_ids[1].clone(), first_ids[0].clone()],
        ),
        (
            "one id named twice",
            vec![first_ids[0].clone(), first_ids[0].clone()],
            vec![first_ids[0].clone(), first_ids[0].clone()],
        ),
        (
            "both ids, the first named twice",
            vec![
                first_ids[0].clone(),
                first_ids[1].clone(),
                first_ids[0].clone(),
            ],
            vec![
                first_ids[0].clone(),
                first_ids[1].clone(),
                first_ids[0].clone(),
            ],
        ),
    ];
    for (case, ids, expected) in accepted {
        let answer = poll_tasks(&emulator, &first, &ids).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let reported: Vec<String> = result_items(&answer)
            .iter()
            .map(|result| member(result, "taskId"))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the poll to report {expected:?}, got {reported:?}"
        );
    }
}
