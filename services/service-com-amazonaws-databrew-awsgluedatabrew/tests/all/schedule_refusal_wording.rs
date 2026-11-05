// SPDX-License-Identifier: CC0-1.0

//! The sentences DataBrew words a SCHEDULE's refusals with.
//!
//! The rule pinned here: the message states the kind of resource the request addressed,
//! the identifier it carried for it, and the predicate DataBrew words that kind's
//! refusals with — and a schedule's predicate is the short, period-less one: `Schedule
//! <Name> not found` for a miss and `Schedule <Name> already exists` for a name a
//! schedule already holds. The wording belongs to the RESOURCE rather than to the
//! operation, so every operation that addresses a schedule states the same sentence,
//! while the kinds whose refusals DataBrew words in its sentence form keep theirs
//! (`tests/all/resource_not_found_message.rs`).
//!
//! The sentences the model documents on the error SHAPES ("One or more resources can't
//! be found.", "Updating or deleting a resource can cause an inconsistent state.",
//! `smithy_query(query="shape",shape="ResourceNotFoundException,ConflictException")`)
//! describe the error CLASSES; they are not the message a particular request answers
//! with.
//!
//! The tables are two-dimensional: every operation that answers the refusal crossed with
//! the forms a schedule name takes — one character and the modelled maximum
//! (`ScheduleName` is `len 1..255`, `smithy_query(query="shape",shape="ScheduleName")`),
//! a name carrying a space, a name differing from a stored one only by case, and a name
//! whose characters a create would refuse but a lookup still simply misses. The status
//! and the error code are asserted on every row too, so a message fix cannot quietly
//! change either.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator};

/// One request: its method, its URI and the body it carries.
type Request = (&'static str, String, Option<Value>);

/// One operation that answers a schedule miss: its name, and the request that addresses
/// a schedule by the name it is given.
struct MissCase {
    operation: &'static str,
    request: fn(&str) -> Request,
}

/// Every operation that answers a schedule miss, as a request built from the absent
/// schedule's name (`smithy_query(query="digest")` for the method and URI each is bound
/// to).
fn miss_cases() -> Vec<MissCase> {
    vec![
        MissCase {
            operation: "DescribeSchedule",
            request: |name| ("GET", format!("/schedules/{}", label(name)), None),
        },
        MissCase {
            operation: "UpdateSchedule",
            request: |name| {
                (
                    "PUT",
                    format!("/schedules/{}", label(name)),
                    Some(json!({"CronExpression": "cron(0 6 * * ? *)"})),
                )
            },
        },
    ]
}

/// The forms of schedule name the rule must carry through to the message unchanged.
fn name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "absent-schedule".to_string()),
        ("digits and hyphens", "brew-2026-07-x9".to_string()),
        ("one character, the modelled minimum", "x".to_string()),
        (
            "255 characters, the modelled maximum",
            "n".repeat(255),
        ),
        ("a name carrying a space", "an absent schedule".to_string()),
        (
            "a name differing from the stored one only by case",
            "NIGHTLY".to_string(),
        ),
        (
            "characters a create would refuse, so nothing can hold it",
            "bad_name".to_string(),
        ),
    ]
}

/// `CreateSchedule`: `POST /schedules`.
async fn create_schedule(emulator: &Emulator, name: &str) -> crate::databrew_harness::Reply {
    emulator
        .call(
            "POST",
            "/schedules",
            Some(json!({"Name": name, "CronExpression": "cron(0 12 * * ? *)"})),
        )
        .await
}

/// A schedule a request can miss BESIDE: the store holds one schedule, so a miss is a
/// miss of that name rather than of an empty store.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        create_schedule(&emulator, "nightly").await.status,
        200,
        "the schedule the misses are measured against must be created"
    );
    emulator
}

#[tokio::test]
async fn a_schedule_miss_states_the_name_it_could_not_find() {
    let emulator = seeded().await;

    for miss in miss_cases() {
        for (form, name) in name_forms() {
            let case = format!("{} / {form}", miss.operation);
            let (method, uri, body) = (miss.request)(&name);
            let refused = emulator.call(method, &uri, body).await;
            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected 404, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            let expected = format!("Schedule {name} not found");
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                refused.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_second_create_of_one_name_states_that_the_schedule_already_exists() {
    // The names a schedule can be created under, which the conflict then names back: the
    // modelled length bounds and the character forms a create admits.
    let names = [
        ("a plain name", "nightly".to_string()),
        ("digits and hyphens", "brew-2026-07-x9".to_string()),
        ("one character, the modelled minimum", "x".to_string()),
        ("255 characters, the modelled maximum", "n".repeat(255)),
        ("a name carrying a space", "every other hour".to_string()),
    ];

    for (form, name) in names {
        let emulator = Emulator::new();
        assert_eq!(
            create_schedule(&emulator, &name).await.status,
            200,
            "case {form:?}: the first create must be served"
        );

        let duplicate = create_schedule(&emulator, &name).await;
        assert_eq!(
            duplicate.status, 409,
            "case {form:?}: expected 409, got {} {:?}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_type.as_deref(),
            Some("ConflictException"),
            "case {form:?}: expected ConflictException, got {:?}",
            duplicate.error_type
        );
        let expected = format!("Schedule {name} already exists");
        assert_eq!(
            duplicate.error_message(),
            expected,
            "case {form:?}: expected {expected:?}, got {:?}",
            duplicate.error_message()
        );
    }
}

/// The predicate is the resource KIND's: a dataset's conflict and miss keep DataBrew's
/// sentence form, so the schedule wording above is not a wording the whole service moved
/// to.
#[tokio::test]
async fn another_resource_kind_keeps_its_own_sentences() {
    let emulator = Emulator::new();
    let dataset = json!({
        "Name": "orders",
        "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "orders.csv"}},
        "Format": "CSV",
    });
    assert_eq!(
        emulator
            .call("POST", "/datasets", Some(dataset.clone()))
            .await
            .status,
        200,
        "the dataset the conflict is measured against must be created"
    );

    let duplicate = emulator.call("POST", "/datasets", Some(dataset)).await;
    assert_eq!(
        (duplicate.status, duplicate.error_message()),
        (409, "Dataset orders already exists.".to_string()),
        "a dataset conflict keeps DataBrew's sentence form, got {} {:?}",
        duplicate.status,
        duplicate.body
    );

    let missed = emulator.call("GET", "/datasets/ghost", None).await;
    assert_eq!(
        (missed.status, missed.error_message()),
        (404, "Dataset ghost wasn't found.".to_string()),
        "a dataset miss keeps DataBrew's sentence form, got {} {:?}",
        missed.status,
        missed.body
    );
}
