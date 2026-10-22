// SPDX-License-Identifier: CC0-1.0

//! `BatchGetApplications`, driven over the wire.
//!
//! RULE under test: `BatchGetApplications` answers one stored `ApplicationInfo`
//! per application name the request carries, in the order the request named them
//! and identical to what `GetApplication` answers for that name; it refuses a
//! batch that names nothing with `ApplicationNameRequiredException`, one past the
//! documented maximum of 100 names with `BatchLimitExceededException`, and one
//! naming an application that is not stored with
//! `ApplicationDoesNotExistException`.
//!
//! The harness (`Emulator`, `error_code`) is the lifecycle suite's, so every case
//! here goes through the same front door a client uses.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The maximum batch size CodeDeploy documents for this operation: "The maximum
/// number of application names you can specify is 100"
/// (`smithy_query(query="operation",operation="BatchGetApplications")` models
/// `BatchLimitExceededException` as the refusal past it).
///
/// Deliberately a SECOND copy of the handler's constant: this suite asserts the
/// bound AWS publishes, so importing the implementation's literal would make the
/// test agree with whatever the handler happens to say.
const MAX_BATCH_NAMES: usize = 100;

/// The `applicationsInfo` list of a successful answer.
fn applications_info(body: &Value, case: &str) -> Vec<Value> {
    body["applicationsInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an applicationsInfo list, got {body:?}"))
        .clone()
}

/// Create `name` with `platform`, answering the `applicationId` that was minted.
async fn create(emulator: &Emulator, name: &str, platform: &str) -> String {
    let created = emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": name, "computePlatform": platform}),
        )
        .await;
    created["applicationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected a minted applicationId, got {created:?}"))
        .to_string()
}

/// RULE: the batch answers one stored `ApplicationInfo` per requested name, in
/// request order, carrying the record `CreateApplication` stored — whatever the
/// batch's size, the order it asks in, and whether it repeats a name.
#[tokio::test]
async fn a_batch_answers_the_stored_record_of_every_name_it_asks_for() {
    let emulator = Emulator::new();

    // The applications the rows below draw on: one per compute platform the
    // model's `ComputePlatform` enum carries, so no row depends on one platform.
    let seeded: &[(&str, &str)] = &[
        ("batch-server", "Server"),
        ("batch-lambda", "Lambda"),
        ("batch-ecs", "ECS"),
        ("batch+punctuation=,.@_-", "Server"),
    ];
    let mut ids: Vec<(String, String)> = Vec::new();
    for (name, platform) in seeded {
        ids.push((name.to_string(), create(&emulator, name, platform).await));
    }

    // Rows differ along batch SIZE, ORDER (creation order, reversed, interleaved)
    // and whether a name REPEATS.
    let cases: &[&[&str]] = &[
        &["batch-server"],
        &["batch-server", "batch-lambda"],
        &["batch-ecs", "batch-lambda", "batch-server"],
        &[
            "batch+punctuation=,.@_-",
            "batch-ecs",
            "batch-server",
            "batch-lambda",
        ],
        &["batch-lambda", "batch-lambda"],
    ];

    for requested in cases {
        let case = format!("{requested:?}");
        let body = emulator
            .ok(
                "BatchGetApplications",
                json!({"applicationNames": requested}),
            )
            .await;
        let answered = applications_info(&body, &case);
        assert_eq!(
            answered.len(),
            requested.len(),
            "case {case}: expected one applicationsInfo entry per requested name, got {answered:?}"
        );

        for (position, name) in requested.iter().enumerate() {
            let info = &answered[position];
            assert_eq!(
                info["applicationName"].as_str(),
                Some(*name),
                "case {case}: expected position {position} to answer {name:?}, got {info:?}"
            );
            let expected_platform = seeded
                .iter()
                .find(|(seeded_name, _)| seeded_name == name)
                .map(|(_, platform)| *platform)
                .unwrap_or_else(|| panic!("case {case}: row names the unseeded {name:?}"));
            assert_eq!(
                info["computePlatform"].as_str(),
                Some(expected_platform),
                "case {case}: expected {name:?} on {expected_platform}, got {info:?}"
            );
            let expected_id = ids
                .iter()
                .find(|(seeded_name, _)| seeded_name == name)
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| panic!("case {case}: row names the unseeded {name:?}"));
            assert_eq!(
                info["applicationId"].as_str(),
                Some(expected_id.as_str()),
                "case {case}: expected the minted id {expected_id} for {name:?}, got {info:?}"
            );
            assert!(
                info["createTime"].is_number(),
                "case {case}: expected a createTime for {name:?}, got {info:?}"
            );

            // The same record a single read answers: the batch is a read of the
            // store, not a second copy of it.
            let single = emulator
                .ok("GetApplication", json!({"applicationName": name}))
                .await;
            assert_eq!(
                &single["application"], info,
                "case {case}: expected the batch entry for {name:?} to equal GetApplication's, \
                 got {info:?}"
            );
        }
    }
}

/// RULE: the batch reports what is stored NOW — an application created after an
/// earlier batch is answered by a later one, and one deleted since is refused.
#[tokio::test]
async fn a_batch_follows_the_store_as_it_changes() {
    let emulator = Emulator::new();
    create(&emulator, "present", "Server").await;

    let (status, body) = emulator
        .call(
            "BatchGetApplications",
            json!({"applicationNames": ["present", "later"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case before-create: expected HTTP 400 for an unstored name, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ApplicationDoesNotExistException",
        "case before-create: expected ApplicationDoesNotExistException, got {body:?}"
    );

    create(&emulator, "later", "Lambda").await;
    let body = emulator
        .ok(
            "BatchGetApplications",
            json!({"applicationNames": ["present", "later"]}),
        )
        .await;
    let answered = applications_info(&body, "after-create");
    let names: Vec<&str> = answered
        .iter()
        .map(|info| info["applicationName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["present", "later"],
        "case after-create: expected both names in request order, got {answered:?}"
    );

    emulator
        .ok("DeleteApplication", json!({"applicationName": "later"}))
        .await;
    let (status, body) = emulator
        .call(
            "BatchGetApplications",
            json!({"applicationNames": ["present", "later"]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case after-delete: expected HTTP 400 for a deleted name, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ApplicationDoesNotExistException",
        "case after-delete: expected ApplicationDoesNotExistException, got {body:?}"
    );

    // Reading the batch changed nothing: the application still there reads back.
    let still_there = emulator
        .ok(
            "BatchGetApplications",
            json!({"applicationNames": ["present"]}),
        )
        .await;
    assert_eq!(
        applications_info(&still_there, "after-delete-survivor").len(),
        1,
        "case after-delete-survivor: expected the surviving application, got {still_there:?}"
    );
}

/// RULE: the documented batch maximum is 100 names — a batch of exactly 100 is
/// answered, and one past it is refused with `BatchLimitExceededException` even
/// when every name it carries is stored.
#[tokio::test]
async fn the_documented_batch_maximum_is_enforced() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..=MAX_BATCH_NAMES)
        .map(|index| format!("bounded-{index:03}"))
        .collect();
    for name in &names {
        create(&emulator, name, "Server").await;
    }

    // Each published bound, and one past it.
    for size in [1usize, MAX_BATCH_NAMES - 1, MAX_BATCH_NAMES] {
        let requested = &names[..size];
        let body = emulator
            .ok(
                "BatchGetApplications",
                json!({"applicationNames": requested}),
            )
            .await;
        let answered = applications_info(&body, &format!("size {size}"));
        assert_eq!(
            answered.len(),
            size,
            "case size {size}: expected {size} entries, got {}",
            answered.len()
        );
    }

    let (status, body) = emulator
        .call(
            "BatchGetApplications",
            json!({"applicationNames": names.clone()}),
        )
        .await;
    assert_eq!(
        status,
        400,
        "case size {}: expected HTTP 400 past the maximum, got {status} {body:?}",
        names.len()
    );
    assert_eq!(
        error_code(&body),
        "BatchLimitExceededException",
        "case size {}: expected BatchLimitExceededException, got {body:?}",
        names.len()
    );
}

/// RULE: every way a batch can be unacceptable is answered with CodeDeploy's own
/// code for it — the name-required code for a batch that names nothing or
/// carries an empty name, the invalid-name code for a name outside the modelled
/// form, and the does-not-exist code for one that is simply not stored.
#[tokio::test]
async fn an_unacceptable_batch_is_answered_with_codedeploys_code_for_it() {
    let emulator = Emulator::new();
    create(&emulator, "stored", "Server").await;

    let too_long = "n".repeat(101);
    let cases: &[(&str, Value, &str)] = &[
        (
            "absent member",
            json!({}),
            "ApplicationNameRequiredException",
        ),
        (
            "empty batch",
            json!({"applicationNames": []}),
            "ApplicationNameRequiredException",
        ),
        (
            "empty name",
            json!({"applicationNames": [""]}),
            "ApplicationNameRequiredException",
        ),
        (
            "empty name beside a stored one",
            json!({"applicationNames": ["stored", ""]}),
            "ApplicationNameRequiredException",
        ),
        (
            "name outside the pattern",
            json!({"applicationNames": ["has spaces"]}),
            "InvalidApplicationNameException",
        ),
        (
            "name past the length maximum",
            json!({"applicationNames": [too_long.clone()]}),
            "InvalidApplicationNameException",
        ),
        (
            "unstored name",
            json!({"applicationNames": ["not-stored"]}),
            "ApplicationDoesNotExistException",
        ),
        (
            "unstored name beside a stored one",
            json!({"applicationNames": ["stored", "not-stored"]}),
            "ApplicationDoesNotExistException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("BatchGetApplications", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
    }
}
