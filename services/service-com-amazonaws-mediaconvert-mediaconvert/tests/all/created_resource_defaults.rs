// SPDX-License-Identifier: CC0-1.0

//! Where the two rules meet: a REPEATED create and the members the service applies.
//!
//! `tests/all/resource_projection_defaults.rs` pins the members MediaConvert fills in
//! itself, and `tests/all/idempotent_repeats.rs` pins that a repeated create returns the
//! stored resource. This suite pins their INTERACTION: a repeat that omits a member the
//! first create set must not re-apply the service default over the stored value, and a
//! repeat that sets a member the first create omitted must not overwrite the default the
//! stored resource carries. Either way the repeat stores nothing and the reader keeps
//! answering what the FIRST create persisted.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::Emulator;
// One `JobTemplateSettings` fixture serves every job-template suite, so a model change to
// the shape is chased through one file.
use super::job_template_crud::settings as template_settings_with;

/// A structurally real `JobTemplateSettings`: one input template and one output group.
fn template_settings() -> Value {
    template_settings_with(1, 1)
}

/// A repeated create neither re-defaults a member the first create SET nor overwrites a
/// default the stored resource carries.
///
/// The table spans the class by resource kind (queue, job template), by which side sets
/// the member (the first create, the repeat, or neither), and by the member being a
/// number, an enum or a structure.
#[tokio::test]
async fn a_repeat_create_leaves_the_service_applied_members_as_stored() {
    // (case, collection, output member, first create, the repeat)
    let cases: Vec<(&str, &str, &str, Value, Value)> = vec![
        (
            "queue: the first create set the concurrency, the repeat omits it",
            "queues",
            "/queue",
            json!({ "name": "explicit-first", "concurrentJobs": 7, "maximumConcurrentFeeds": 3 }),
            json!({ "name": "explicit-first" }),
        ),
        (
            "queue: the first create omitted the concurrency, the repeat sets it",
            "queues",
            "/queue",
            json!({ "name": "defaulted-first" }),
            json!({ "name": "defaulted-first", "concurrentJobs": 9 }),
        ),
        (
            "job template: the first create set priority and interval, the repeat omits them",
            "jobTemplates",
            "/jobTemplate",
            json!({
                "name": "explicit-template",
                "priority": 25,
                "statusUpdateInterval": "SECONDS_300",
                "accelerationSettings": { "mode": "ENABLED" },
                "settings": template_settings()
            }),
            json!({ "name": "explicit-template", "settings": template_settings() }),
        ),
        (
            "job template: the first create omitted them, the repeat sets them",
            "jobTemplates",
            "/jobTemplate",
            json!({ "name": "defaulted-template", "settings": template_settings() }),
            json!({
                "name": "defaulted-template",
                "priority": -50,
                "statusUpdateInterval": "SECONDS_10",
                "accelerationSettings": { "mode": "PREFERRED" },
                "settings": template_settings()
            }),
        ),
    ];

    for (case, collection, member, first_request, repeat_request) in cases {
        let emulator = Emulator::new();
        let name = first_request["name"].as_str().unwrap_or_default().to_string();

        let first = emulator
            .send(
                "POST",
                &format!("/2017-08-29/{collection}"),
                Some(first_request),
            )
            .await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: the first create must succeed, got {} with body {}",
            first.status, first.body
        );
        let stored = first.at(member).clone();

        let again = emulator
            .send(
                "POST",
                &format!("/2017-08-29/{collection}"),
                Some(repeat_request),
            )
            .await;
        assert_eq!(
            again.status, 201,
            "case {case:?}: the repeat must succeed, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.at(member),
            &stored,
            "case {case:?}: the repeat must answer the stored resource {stored}, got {}",
            again.body
        );

        // And the reader agrees: nothing about the stored resource moved.
        let fetched = emulator
            .send("GET", &format!("/2017-08-29/{collection}/{name}"), None)
            .await;
        assert_eq!(
            fetched.at(member),
            &stored,
            "case {case:?}: the reader must answer {stored}, got {}",
            fetched.body
        );
    }
}
