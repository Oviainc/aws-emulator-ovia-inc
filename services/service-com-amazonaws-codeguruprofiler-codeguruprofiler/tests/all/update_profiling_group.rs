// SPDX-License-Identifier: CC0-1.0

//! `UpdateProfilingGroup`, end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rule every case here asserts: `UpdateProfilingGroup` writes the request's
//! `agentOrchestrationConfig` onto the ONE stored profiling-group record, leaves every
//! other member of that record as it stood, and answers with the record as it now
//! stands — so a later `DescribeProfilingGroup` and a later `ConfigureAgent` report the
//! value the update stored, and an update naming a group the store does not hold
//! answers the modelled 404 `ResourceNotFoundException`.
//!
//! The harness, the group table and the per-case expectations are the ones
//! [`crate::profiling_group_lifecycle`] already builds; this suite varies what the rule
//! generalises over: the group it updates (compute platform, orchestration flag at
//! create, tag count, name length) and the sequence of `profilingEnabled` values the
//! update sends (enable, disable, repeat the same value, re-enable).

use crate::profiling_group_lifecycle::{group_cases, GroupCase, Harness};

/// The `profilingEnabled` values one group is driven through, in order. The repeat
/// covers an update that asks for the value already stored (the operation is
/// `@idempotent`), and the first row differs from some rows' create-time value and
/// agrees with others', so neither "changes it" nor "leaves it" is privileged.
const TOGGLES: [bool; 5] = [true, false, false, true, false];

/// `UpdateProfilingGroup`: `PUT /profilingGroups/{profilingGroupName}` with the
/// required `agentOrchestrationConfig` in the payload.
async fn update(harness: &Harness, name: &str, profiling_enabled: bool) -> crate::profiling_group_lifecycle::Answer {
    harness
        .send(
            "PUT",
            &format!("/profilingGroups/{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "agentOrchestrationConfig": { "profilingEnabled": profiling_enabled }
            }))
            .expect("a serializable body"),
        )
        .await
}

/// A modelled timestamp member as epoch seconds. Every `Timestamp` of this service
/// carries `@timestampFormat("date-time")`, so these arrive as ISO-8601 strings and are
/// parsed back to an instant here, which is what the comparison below is about.
fn epoch_seconds(description: &serde_json::Value, member: &str) -> f64 {
    let value = description
        .get(member)
        .unwrap_or_else(|| panic!("no {member} in {description:?}"));
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{member} must be a date-time string, got {value}"));
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("{member} {text:?} must parse as a date-time: {error}"))
        .as_secs_f64()
}

/// Every member of the description an update must leave exactly as it stood, plus the
/// `profilingEnabled` it must now report.
fn assert_updated_description(
    case: &GroupCase,
    label: &str,
    expected_enabled: bool,
    created: &serde_json::Value,
    previous: &serde_json::Value,
    description: &serde_json::Value,
) {
    assert_eq!(
        description
            .pointer("/agentOrchestrationConfig/profilingEnabled")
            .and_then(serde_json::Value::as_bool),
        Some(expected_enabled),
        "case {case:?} ({label}): profilingEnabled must be what the update stored, got {description:?}"
    );
    assert_eq!(
        description.get("name").and_then(serde_json::Value::as_str),
        Some(case.name.as_str()),
        "case {case:?} ({label}): wrong name, got {description:?}"
    );
    assert_eq!(
        description.get("arn").and_then(serde_json::Value::as_str),
        Some(case.expected_arn().as_str()),
        "case {case:?} ({label}): an update must not re-mint the arn, got {description:?}"
    );
    assert_eq!(
        description
            .get("computePlatform")
            .and_then(serde_json::Value::as_str),
        Some(case.expected_compute_platform()),
        "case {case:?} ({label}): an update must not change computePlatform, got {description:?}"
    );
    assert_eq!(
        description.get("tags"),
        created.get("tags"),
        "case {case:?} ({label}): an update must not change the tags, got {description:?}"
    );
    assert_eq!(
        description.get("createdAt"),
        created.get("createdAt"),
        "case {case:?} ({label}): an update must not change createdAt, got {description:?}"
    );
    assert_eq!(
        description.get("profilingStatus"),
        previous.get("profilingStatus"),
        "case {case:?} ({label}): an update must not touch the profiling status, got {description:?}"
    );
    let created_at = epoch_seconds(description, "createdAt");
    let updated_at = epoch_seconds(description, "updatedAt");
    assert!(
        updated_at >= created_at,
        "case {case:?} ({label}): updatedAt {updated_at} must not precede createdAt {created_at}"
    );
}

#[tokio::test]
async fn an_update_stores_the_orchestration_config_and_answers_from_the_stored_group() {
    let harness = Harness::new();

    for (index, case) in group_cases().iter().enumerate() {
        let created = harness.create(case, &format!("tok-{index}")).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: create must succeed, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_description = created.json();
        // What the group reported just before each update: the members the update must
        // leave alone are compared against THIS, since the reads interleaved below
        // (`ConfigureAgent` records the agent ping) move the profiling status. It is a
        // READ of the group, not the create's payload — the two differ by exactly
        // `profilingStatus`, which a create omits while nothing has been reported and a
        // read reports as the (empty) container.
        let mut previous_description = harness.describe(&case.name).await.json();

        for (step, enabled) in TOGGLES.iter().copied().enumerate() {
            let label = format!("update {step} -> {enabled}");
            let updated = update(&harness, &case.name, enabled).await;
            assert_eq!(
                updated.status,
                200,
                "case {case:?} ({label}): UpdateProfilingGroup must answer 200, got {} {}",
                updated.status,
                String::from_utf8_lossy(&updated.body)
            );
            let updated_description = updated.json();
            assert_updated_description(
                case,
                &label,
                enabled,
                &created_description,
                &previous_description,
                &updated_description,
            );

            // The decisive assertion: the value was STORED, so the reader answers with
            // the same record rather than the update's response being an echo.
            let described = harness.describe(&case.name).await;
            assert_eq!(
                described.status,
                200,
                "case {case:?} ({label}): describe must answer 200, got {} {}",
                described.status,
                String::from_utf8_lossy(&described.body)
            );
            let described_description = described.json();
            assert_eq!(
                described_description, updated_description,
                "case {case:?} ({label}): describe must return the record the update stored"
            );

            // `ConfigureAgent`'s `shouldProfile` reads the same stored flag, so the
            // update is visible to the agent too.
            let configured = harness.configure_agent(&case.name).await;
            assert_eq!(
                configured.status,
                200,
                "case {case:?} ({label}): ConfigureAgent must answer 200, got {} {}",
                configured.status,
                String::from_utf8_lossy(&configured.body)
            );
            assert_eq!(
                configured
                    .json()
                    .get("shouldProfile")
                    .and_then(serde_json::Value::as_bool),
                Some(enabled),
                "case {case:?} ({label}): shouldProfile must follow the stored config, got {}",
                String::from_utf8_lossy(&configured.body)
            );

            previous_description = harness.describe(&case.name).await.json();
        }
    }
}

#[tokio::test]
async fn an_update_touches_only_the_group_it_names() {
    let harness = Harness::new();
    let cases = group_cases();

    for (index, case) in cases.iter().enumerate() {
        assert_eq!(
            harness.create(case, &format!("tok-{index}")).await.status,
            201,
            "case {case:?}: create must succeed"
        );
    }

    // Disable profiling on ONE group; every other group keeps the flag it was created
    // with, because the update writes only the record it names.
    let target = &cases[1];
    let updated = update(&harness, &target.name, false).await;
    assert_eq!(
        updated.status,
        200,
        "case {target:?}: update must answer 200, got {} {}",
        updated.status,
        String::from_utf8_lossy(&updated.body)
    );

    for case in &cases {
        let expected = if case.name == target.name {
            false
        } else {
            case.expected_profiling_enabled()
        };
        let described = harness.describe(&case.name).await;
        let description = described.json();
        assert_eq!(
            description
                .pointer("/agentOrchestrationConfig/profilingEnabled")
                .and_then(serde_json::Value::as_bool),
            Some(expected),
            "case {case:?}: only {} was updated, got {description:?}",
            target.name
        );
    }
}

#[tokio::test]
async fn updating_a_group_the_store_does_not_hold_answers_the_modelled_404() {
    let harness = Harness::new();
    let cases = group_cases();
    assert_eq!(
        harness.create(&cases[0], "tok-0").await.status,
        201,
        "create must succeed"
    );

    // Names the store does not hold, varying in length and form, all inside the
    // modelled `ProfilingGroupName` constraints (len 1..255, pattern ^[\w-]+$) so the
    // request reaches the handler rather than being refused at the boundary.
    for name in [
        "no-such-group",
        "z",
        &cases[0].name.to_uppercase(),
        &format!("{}-missing", cases[0].name),
        &"m".repeat(255),
    ] {
        let answer = update(&harness, name, true).await;
        assert_eq!(
            answer.status,
            404,
            "name {name:?}: an unknown group must answer 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "name {name:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The failed updates stored nothing: the one real group still reports the config it
    // was created with.
    let described = harness.describe(&cases[0].name).await;
    let description = described.json();
    assert_eq!(
        description
            .pointer("/agentOrchestrationConfig/profilingEnabled")
            .and_then(serde_json::Value::as_bool),
        Some(cases[0].expected_profiling_enabled()),
        "a refused update must store nothing, got {description:?}"
    );
}

#[tokio::test]
async fn an_update_without_the_required_orchestration_config_is_refused_as_a_bad_request() {
    let harness = Harness::new();
    let cases = group_cases();
    assert_eq!(
        harness.create(&cases[0], "tok-0").await.status,
        201,
        "create must succeed"
    );

    // `agentOrchestrationConfig` is `required` on the input shape, so a payload without
    // it is a client error, not a 501 and not a silent success.
    let answer = harness
        .send(
            "PUT",
            &format!("/profilingGroups/{}", cases[0].name),
            "application/json",
            b"{}".to_vec(),
        )
        .await;
    assert_eq!(
        answer.status,
        400,
        "a missing required member must answer 400, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}
