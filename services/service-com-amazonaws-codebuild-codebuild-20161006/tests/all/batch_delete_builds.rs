// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchDeleteBuilds`, driven over the wire the router serves
//! (`POST /` with `X-Amz-Target: CodeBuild_20161006.BatchDeleteBuilds` on awsJson1_1)
//! through the harness in `project_build_lifecycle`.
//!
//! RULE under test: `BatchDeleteBuilds` removes every build the request names from
//! this account's build records, whatever status that build carries, and reports each
//! named build exactly once — in `buildsDeleted` when a record was held and removed,
//! and in `buildsNotDeleted` with its id and a `statusCode` saying why when no record
//! was held. `buildsNotDeleted` carries no `@length`, so it is always reported, `[]`
//! when nothing was left behind. A removed build is gone from every later read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project, emulator, error_type, start_build, PAST_THE_SETTLE_SPAN,
};

/// How a row leaves a build before the delete call: the three states a stored build
/// can be in here — still inside its settle span, finished by the clock, or stopped by
/// the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    InProgress,
    Settled,
    Stopped,
}

/// The `buildStatus` a build left in this state reports.
fn expected_status(left: Left) -> &'static str {
    match left {
        Left::InProgress => "IN_PROGRESS",
        // Nothing executes a buildspec here, so a build the clock finished is FAULT.
        Left::Settled => "FAULT",
        Left::Stopped => "STOPPED",
    }
}

/// One row of the table: the builds to set up, which of them the request names (by
/// index, in request order), how many ids naming no build to mix in, and whether the
/// first id is repeated.
#[derive(Debug)]
struct Case {
    name: &'static str,
    project: &'static str,
    builds: &'static [Left],
    named: &'static [usize],
    unknown: usize,
    repeat_first: bool,
}

/// A syntactically valid build id (`<projectName>:<uuid>`) that names no build: the
/// uuid is one this service never mints, since every build's uuid is derived from its
/// project, build number and start instant.
fn unknown_id(project: &str, nth: usize) -> String {
    format!("{project}:00000000-0000-8000-8000-{nth:012}")
}

/// Set up `case`'s builds and hand back their ids by index, plus the order they were
/// started in (which is `buildNumber` order, and so list order).
async fn set_up(router: &MultiServiceRouter, case: &Case) -> (Vec<String>, Vec<usize>) {
    create_project(router, case.project, json!({})).await;
    let mut ids = vec![String::new(); case.builds.len()];
    let mut start_order = Vec::new();

    // The builds the clock must have finished are started first and the settle span
    // waited out once; the rest are started after it, so a single clock can leave
    // different rows of one request in different states.
    for (index, left) in case.builds.iter().enumerate() {
        if *left == Left::Settled {
            let build = start_build(router, json!({"projectName": case.project})).await;
            ids[index] = build["id"].as_str().unwrap_or_default().to_string();
            start_order.push(index);
        }
    }
    if case.builds.contains(&Left::Settled) {
        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
    }
    for (index, left) in case.builds.iter().enumerate() {
        if *left == Left::Settled {
            continue;
        }
        let build = start_build(router, json!({"projectName": case.project})).await;
        let id = build["id"].as_str().unwrap_or_default().to_string();
        if *left == Left::Stopped {
            let (status, body) = call(router, "StopBuild", json!({"id": id.clone()})).await;
            assert_eq!(
                status, 200,
                "case {:?}: StopBuild {id}: expected 200, got {status} with {body}",
                case.name
            );
        }
        ids[index] = id;
        start_order.push(index);
    }

    // Each row really is in the state it claims, so a delete assertion below is about
    // deletion and not about a mis-set-up build.
    let (status, before) = call(router, "BatchGetBuilds", json!({"ids": ids.clone()})).await;
    assert_eq!(
        status, 200,
        "case {:?}: BatchGetBuilds before the delete: expected 200, got {status} with {before}",
        case.name
    );
    for (index, left) in case.builds.iter().enumerate() {
        let reported = before["builds"]
            .as_array()
            .and_then(|builds| {
                builds
                    .iter()
                    .find(|build| build["id"].as_str() == Some(ids[index].as_str()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported["buildStatus"],
            expected_status(*left),
            "case {:?}: build {index} must be {} before the delete, got {reported}",
            case.name,
            expected_status(*left)
        );
    }

    (ids, start_order)
}

/// The ids `case` names, in request order.
fn requested_ids(case: &Case, started: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (position, index) in case.named.iter().enumerate() {
        ids.push(started[*index].clone());
        if position < case.unknown {
            ids.push(unknown_id(case.project, position));
        }
    }
    for position in case.named.len()..case.unknown {
        ids.push(unknown_id(case.project, position));
    }
    if case.repeat_first {
        if let Some(first) = ids.first().cloned() {
            ids.push(first);
        }
    }
    ids
}

/// `items` with every repeat after the first dropped, order kept.
fn deduped(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

/// RULE: every named build that is held is removed and reported in `buildsDeleted`,
/// each named build is reported exactly once, and an id naming no build is reported in
/// `buildsNotDeleted` with that id and a `statusCode` saying so. The table varies the
/// build's status, the number of ids, the mix of held and unheld ids, the request order
/// and a repeated id.
#[tokio::test]
async fn batch_delete_builds_removes_every_named_build_whatever_its_status() {
    let cases: &[Case] = &[
        Case {
            name: "one build still inside its settle span",
            project: "del-in-progress",
            builds: &[Left::InProgress],
            named: &[0],
            unknown: 0,
            repeat_first: false,
        },
        Case {
            name: "one build the clock already finished",
            project: "del-settled",
            builds: &[Left::Settled],
            named: &[0],
            unknown: 0,
            repeat_first: false,
        },
        Case {
            name: "one build the caller stopped",
            project: "del-stopped",
            builds: &[Left::Stopped],
            named: &[0],
            unknown: 0,
            repeat_first: false,
        },
        Case {
            name: "every build of a project, named out of start order",
            project: "del-all",
            builds: &[Left::InProgress, Left::InProgress, Left::InProgress],
            named: &[2, 0, 1],
            unknown: 0,
            repeat_first: false,
        },
        Case {
            name: "some builds, leaving the rest in place",
            project: "del-some",
            builds: &[
                Left::Settled,
                Left::InProgress,
                Left::Stopped,
                Left::InProgress,
            ],
            named: &[1, 3],
            unknown: 0,
            repeat_first: false,
        },
        Case {
            name: "only ids naming no build",
            project: "del-unknown",
            builds: &[Left::InProgress],
            named: &[],
            unknown: 2,
            repeat_first: false,
        },
        Case {
            name: "held and unheld ids mixed in one request",
            project: "del-mixed",
            builds: &[Left::Settled, Left::InProgress],
            named: &[0, 1],
            unknown: 2,
            repeat_first: false,
        },
        Case {
            name: "the same build named twice",
            project: "del-repeat",
            builds: &[Left::InProgress],
            named: &[0],
            unknown: 0,
            repeat_first: true,
        },
    ];

    for case in cases {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        let (started, start_order) = set_up(&router, case).await;
        let requested = requested_ids(case, &started);
        let expected_deleted: Vec<String> = deduped(
            &requested
                .iter()
                .filter(|id| started.contains(id))
                .cloned()
                .collect::<Vec<String>>(),
        );
        let expected_not_deleted: Vec<String> = deduped(
            &requested
                .iter()
                .filter(|id| !started.contains(id))
                .cloned()
                .collect::<Vec<String>>(),
        );

        let (status, body) = call(
            &router,
            "BatchDeleteBuilds",
            json!({"ids": requested.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchDeleteBuilds {requested:?}: expected 200, got {status} with {body}",
            case.name
        );

        let deleted: Vec<String> = body["buildsDeleted"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            deleted, expected_deleted,
            "case {:?}: buildsDeleted must report each held id once, in request order; got {body}",
            case.name
        );
        // `buildsDeleted` targets `BuildIds` (@length min 1), so nothing deleted means
        // the member is absent rather than an empty list.
        assert_eq!(
            body["buildsDeleted"].is_null(),
            expected_deleted.is_empty(),
            "case {:?}: an empty buildsDeleted must be absent, got {body}",
            case.name
        );

        let not_deleted: Vec<String> = body["buildsNotDeleted"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            not_deleted, expected_not_deleted,
            "case {:?}: buildsNotDeleted must report each unheld id once; got {body}",
            case.name
        );
        // `BuildsNotDeleted` carries no `@length`, so the member is always there —
        // `[]` when every named build was deleted.
        assert!(
            body["buildsNotDeleted"].is_array(),
            "case {:?}: buildsNotDeleted is always reported, got {body}",
            case.name
        );
        for (position, id) in expected_not_deleted.iter().enumerate() {
            // CodeBuild builds this text from the request: it names the id as the
            // caller gave it and says it does not exist.
            assert_eq!(
                body["buildsNotDeleted"][position]["statusCode"],
                Value::String(format!("Build {id} does not exist")),
                "case {:?}: the statusCode for {id} must name that id and say why; got {body}",
                case.name
            );
        }

        // Every later read is computed from the store the delete wrote: the deleted
        // builds are gone and the survivors are untouched.
        let (status, after) = call(&router, "BatchGetBuilds", json!({"ids": started.clone()})).await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchGetBuilds after the delete: expected 200, got {status} with {after}",
            case.name
        );
        let reported: Vec<String> = after["builds"]
            .as_array()
            .map(|builds| {
                builds
                    .iter()
                    .map(|build| build["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let survivors: Vec<String> = started
            .iter()
            .filter(|id| !expected_deleted.contains(id))
            .cloned()
            .collect();
        for id in &expected_deleted {
            assert!(
                !reported.contains(id),
                "case {:?}: {id} was deleted, so BatchGetBuilds must not report it; got {after}",
                case.name
            );
        }
        for id in &survivors {
            assert!(
                reported.contains(id),
                "case {:?}: {id} was not named, so it must still be readable; got {after}",
                case.name
            );
        }
        let not_found: Vec<String> = after["buildsNotFound"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        for id in &expected_deleted {
            assert!(
                not_found.contains(id),
                "case {:?}: a deleted build reads as not found; got {after}",
                case.name
            );
        }

        // And the project's own index: the survivors only, still in build-number
        // order (descending, which is the documented default).
        let (status, listed) = call(
            &router,
            "ListBuildsForProject",
            json!({"projectName": case.project}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListBuildsForProject: expected 200, got {status} with {listed}",
            case.name
        );
        let expected_listed: Vec<String> = start_order
            .iter()
            .map(|index| started[*index].clone())
            .filter(|id| !expected_deleted.contains(id))
            .rev()
            .collect();
        let actual_listed: Vec<String> = listed["ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .map(|id| id.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            actual_listed, expected_listed,
            "case {:?}: the project must list only the builds that survived, newest first; got {listed}",
            case.name
        );
    }
}

/// RULE: a deleted build frees nothing a later build reuses — `buildNumber` keeps
/// counting up for a project whose builds were all deleted — and the deletion is
/// durable: deleting the same id again reports it as not deleted, and the operations
/// that answer for an absent build name it by the id the request gave.
#[tokio::test]
async fn deleting_a_build_neither_rewinds_the_build_number_nor_can_be_repeated() {
    let router = emulator();
    create_project(&router, "del-numbers", json!({})).await;

    let mut ids = Vec::new();
    for expected in 1..=3_i64 {
        let build = start_build(&router, json!({"projectName": "del-numbers"})).await;
        assert_eq!(
            build["buildNumber"].as_i64(),
            Some(expected),
            "build {expected}: expected buildNumber {expected}, got {build}"
        );
        ids.push(build["id"].as_str().unwrap_or_default().to_string());
    }

    let (status, body) = call(
        &router,
        "BatchDeleteBuilds",
        json!({"ids": ids.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchDeleteBuilds: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["buildsDeleted"].as_array().map(Vec::len),
        Some(ids.len()),
        "every build of the project was named, so every one is deleted; got {body}"
    );
    assert_eq!(
        body["buildsNotDeleted"],
        json!([]),
        "nothing was left behind, so buildsNotDeleted is an empty list; got {body}"
    );

    // The project still exists, so its counter does: the next build is the fourth.
    let next = start_build(&router, json!({"projectName": "del-numbers"})).await;
    assert_eq!(
        next["buildNumber"].as_i64(),
        Some(4),
        "a deleted build must not free its buildNumber, got {next}"
    );

    // Deleting an already-deleted id is reported, not re-deleted.
    let (status, again) = call(
        &router,
        "BatchDeleteBuilds",
        json!({"ids": [ids[0].clone()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the repeated delete: expected 200, got {status} with {again}"
    );
    assert!(
        again["buildsDeleted"].is_null(),
        "nothing was held, so buildsDeleted is absent; got {again}"
    );
    assert_eq!(
        again["buildsNotDeleted"][0]["id"],
        Value::String(ids[0].clone()),
        "the already-deleted id reads as not deleted; got {again}"
    );
    assert_eq!(
        again["buildsNotDeleted"][0]["statusCode"],
        Value::String(format!("Build {} does not exist", ids[0])),
        "the already-deleted id's statusCode names it; got {again}"
    );

    // The same rule, in the other operation that answers for a build it holds no
    // record of: an absent build is named by the id the request gave.
    let mut absent: Vec<String> = ids.clone();
    absent.push(unknown_id("del-numbers", 7));
    absent.push(unknown_id("del-never-created", 7));
    for id in &absent {
        let (status, body) = call(&router, "StopBuild", json!({"id": id.clone()})).await;
        assert_eq!(
            status, 400,
            "StopBuild {id}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "StopBuild {id}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            Value::String(format!("Build {id} does not exist")),
            "StopBuild {id}: the refusal names that id, got {body}"
        );
    }
}

/// RULE: `ids` is held to the bounds the model declares for it — `BuildIds`
/// `@length(min: 1, max: 100)`, with `ids` itself `@required` — so a request at either
/// published bound is served and one past it, or one leaving `ids` out, is refused with
/// the client error the operation models.
#[tokio::test]
async fn the_ids_list_is_held_to_its_declared_length_bounds() {
    let router = emulator();
    create_project(&router, "del-bounds", json!({})).await;
    let build = start_build(&router, json!({"projectName": "del-bounds"})).await;
    let held = build["id"].as_str().unwrap_or_default().to_string();

    // At the minimum: one id.
    let (status, body) = call(&router, "BatchDeleteBuilds", json!({"ids": [held.clone()]})).await;
    assert_eq!(
        status, 200,
        "one id, the declared minimum: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["buildsDeleted"],
        json!([held.clone()]),
        "one id, the declared minimum: expected it deleted, got {body}"
    );

    // At the maximum: 100 ids, one of them held.
    let second = start_build(&router, json!({"projectName": "del-bounds"})).await;
    let second_id = second["id"].as_str().unwrap_or_default().to_string();
    let mut at_max: Vec<String> = vec![second_id.clone()];
    while at_max.len() < 100 {
        at_max.push(unknown_id("del-bounds", at_max.len()));
    }
    let (status, body) = call(&router, "BatchDeleteBuilds", json!({"ids": at_max.clone()})).await;
    assert_eq!(
        status, 200,
        "100 ids, the declared maximum: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["buildsDeleted"],
        json!([second_id]),
        "100 ids, the declared maximum: only the held one is deleted, got {body}"
    );
    assert_eq!(
        body["buildsNotDeleted"].as_array().map(Vec::len),
        Some(99),
        "100 ids, the declared maximum: the other 99 are reported, got {body}"
    );

    let refused: Vec<(&str, Value)> = vec![
        ("an empty ids list, below the minimum", json!({"ids": []})),
        (
            "101 ids, past the maximum",
            json!({"ids": (0..101).map(|nth| unknown_id("del-bounds", nth)).collect::<Vec<String>>()}),
        ),
        ("ids left out entirely", json!({})),
    ];
    for (case, request) in refused {
        let (status, body) = call(&router, "BatchDeleteBuilds", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }
}
