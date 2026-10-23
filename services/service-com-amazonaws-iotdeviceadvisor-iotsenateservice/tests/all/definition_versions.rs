// SPDX-License-Identifier: CC0-1.0

//! `UpdateSuiteDefinition` over the wire: a definition's configuration VERSIONS.
//!
//! The rule under test: recording a configuration on an existing suite definition hands out
//! that definition's next version label, advances `latestVersion` to it and REPLACES the
//! definition's one live configuration, leaving the definition's identity (id, ARN)
//! unchanged — so every read of it, naming an earlier version or naming none, answers the
//! configuration last recorded, while both of its timestamps move to the write instant.
//!
//! The table spans the configuration class the rule generalises over rather than one
//! update: the rows of [`definition_rows`] (every `Protocol` the model enumerates, device
//! lists of each permitted size, both booleans, both `suiteDefinitionName` bounds and both
//! `rootGroup` forms) are applied in sequence to ONE definition, and after every update
//! every retained version is read back.

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use crate::suite_lifecycle::{definition_rows, thing_arn, Answer, Emulator, ROLE_ARN};

/// `UpdateSuiteDefinition` is `PATCH /suiteDefinitions/{suiteDefinitionId}`.
async fn update_definition(emulator: &Emulator, id: &str, configuration: &Value) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!("/suiteDefinitions/{id}"),
            "us-east-1",
            Some(json!({ "suiteDefinitionConfiguration": configuration })),
        )
        .await
}

/// Read one definition, optionally naming a version.
async fn get_definition(emulator: &Emulator, id: &str, version: Option<&str>) -> Answer {
    let path = match version {
        Some(version) => format!("/suiteDefinitions/{id}?suiteDefinitionVersion={version}"),
        None => format!("/suiteDefinitions/{id}"),
    };
    emulator.call("GET", &path, "us-east-1", None).await
}

/// Each update hands out the next version label, advances `latestVersion`, and leaves
/// every label already handed out readable — answering the definition's one LIVE
/// configuration.
#[tokio::test]
async fn updating_advances_the_version_and_every_label_serves_the_live_configuration() {
    let emulator = Emulator::new();

    let rows = definition_rows();
    let (first_case, first_configuration) = rows[0].clone();
    let created = emulator
        .create_definition(&first_configuration, "us-east-1")
        .await;
    assert_eq!(
        created.status, 200,
        "case {first_case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
        created.status, created.body
    );
    let id = created.string("suiteDefinitionId");
    let arn = created.string("suiteDefinitionArn");
    // The write stamp observed before each update; an update must move it forward.
    let mut previous_created_at = created
        .member("createdAt")
        .as_f64()
        .unwrap_or_else(|| panic!("case {first_case:?}: expected a createdAt timestamp"));

    // Every version label handed out so far — each must stay readable.
    let mut labels: Vec<String> = vec!["v1".to_string()];

    for (index, (case, configuration)) in rows.iter().enumerate().skip(1) {
        let updated = update_definition(&emulator, &id, configuration).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        let expected_version = format!("v{}", index + 1);
        assert_eq!(
            updated.string("suiteDefinitionVersion"),
            expected_version,
            "case {case:?}: an update must advance to the next version, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("suiteDefinitionId"),
            id,
            "case {case:?}: an update must keep the definition's id, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("suiteDefinitionArn"),
            arn,
            "case {case:?}: an update must keep the definition's ARN, got {}",
            updated.body
        );
        assert_eq!(
            updated.member("suiteDefinitionName"),
            &configuration["suiteDefinitionName"],
            "case {case:?}: suiteDefinitionName must come from the stored configuration, got {}",
            updated.body
        );
        let updated_created_at = updated.member("createdAt").as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a createdAt timestamp, got {}",
                updated.body
            )
        });
        assert!(
            updated_created_at > previous_created_at,
            "case {case:?}: an update must advance createdAt past {previous_created_at}, got \
             {updated_created_at}"
        );
        let last_updated = updated.member("lastUpdatedAt").as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a lastUpdatedAt timestamp, got {}",
                updated.body
            )
        });
        assert!(
            last_updated >= updated_created_at,
            "case {case:?}: lastUpdatedAt must not precede createdAt, got {last_updated}"
        );

        labels.push(expected_version.clone());

        // The unqualified read answers the configuration this update recorded.
        let latest = get_definition(&emulator, &id, None).await;
        assert_eq!(
            latest.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteDefinition, got {} {}",
            latest.status, latest.body
        );
        assert_eq!(
            latest.string("latestVersion"),
            expected_version,
            "case {case:?}: latestVersion must be the version just recorded, got {}",
            latest.body
        );
        assert_eq!(
            latest.member("suiteDefinitionConfiguration"),
            configuration,
            "case {case:?}: an unqualified read must answer the configuration just recorded, \
             got {}",
            latest.body
        );
        let read_created_at = latest.member("createdAt").as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a createdAt timestamp, got {}",
                latest.body
            )
        });
        assert_eq!(
            read_created_at, updated_created_at,
            "case {case:?}: a read must report the write stamp the update minted, got {}",
            latest.body
        );
        previous_created_at = read_created_at;

        // Every label handed out so far still reads 200, echoes the version asked for, and
        // answers the ONE live configuration — there are no per-version snapshots.
        for version in &labels {
            let historic = get_definition(&emulator, &id, Some(version)).await;
            assert_eq!(
                historic.status, 200,
                "case {case:?}: version {version} must stay readable, got {} {}",
                historic.status, historic.body
            );
            assert_eq!(
                historic.member("suiteDefinitionConfiguration"),
                configuration,
                "case {case:?}: version {version} must answer the CURRENT configuration, got {}",
                historic.body
            );
            assert_eq!(
                historic.string("suiteDefinitionVersion"),
                *version,
                "case {case:?}: a read naming {version} must report it, got {}",
                historic.body
            );
            assert_eq!(
                historic.string("latestVersion"),
                expected_version,
                "case {case:?}: latestVersion must be the newest version whichever version a \
                 read names, got {}",
                historic.body
            );
        }

        // The projection `ListSuiteDefinitions` builds follows the latest version.
        let listed = emulator
            .call("GET", "/suiteDefinitions", "us-east-1", None)
            .await;
        let entry = listed.member("suiteDefinitionInformationList")[0].clone();
        assert_eq!(
            entry["suiteDefinitionName"], configuration["suiteDefinitionName"],
            "case {case:?}: the listing must project the latest configuration, got {entry}"
        );
    }
}

/// An update touches only the definition it names, and the ids it did not name keep their
/// own versions.
#[tokio::test]
async fn an_update_touches_only_the_named_definition() {
    let emulator = Emulator::new();
    let configuration = |name: &str| {
        json!({
            "suiteDefinitionName": name,
            "rootGroup": "root-group",
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": [{ "thingArn": thing_arn(3) }],
        })
    };

    let first = emulator
        .create_definition(&configuration("first"), "us-east-1")
        .await
        .string("suiteDefinitionId");
    let second = emulator
        .create_definition(&configuration("second"), "us-east-1")
        .await
        .string("suiteDefinitionId");

    let updated = update_definition(&emulator, &first, &configuration("first-renamed")).await;
    assert_eq!(
        updated.string("suiteDefinitionVersion"),
        "v2",
        "case {:?}: expected the update to advance only the named definition, got {}",
        "two definitions",
        updated.body
    );

    let untouched = get_definition(&emulator, &second, None).await;
    assert_eq!(
        untouched.string("latestVersion"),
        "v1",
        "case {:?}: a definition nobody updated must keep its single version, got {}",
        "two definitions",
        untouched.body
    );
    assert_eq!(
        untouched.member("suiteDefinitionConfiguration"),
        &configuration("second"),
        "case {:?}: a definition nobody updated must keep its configuration, got {}",
        "two definitions",
        untouched.body
    );
}

/// What an update refuses: an id the store does not hold — AWS answers that with
/// `ResourceNotFoundException` (HTTP 404 per the shape's own `httpError`), which this
/// operation's error list does not advertise — and a configuration that breaks the
/// `rootGroup` rule, refused with the modelled `ValidationException`. Never a 501 and
/// never a success.
#[tokio::test]
async fn updates_outside_the_rules_are_refused() {
    let emulator = Emulator::new();
    let legal = json!({
        "suiteDefinitionName": "refusals",
        "rootGroup": "root-group",
        "devicePermissionRoleArn": ROLE_ARN,
    });
    let known = emulator
        .create_definition(&legal, "us-east-1")
        .await
        .string("suiteDefinitionId");

    // (case, suiteDefinitionId, configuration, expected status)
    let rows: Vec<(&str, String, Value, u16)> = vec![
        (
            "unknown but well-formed id",
            "00000000-0000-4000-8000-000000000000".to_string(),
            legal.clone(),
            404,
        ),
        (
            // `UUID` is `len 12..36`: shorter than the minimum is refused by the modelled
            // constraint before the handler runs.
            "id shorter than the modelled minimum",
            "short".to_string(),
            legal.clone(),
            400,
        ),
        (
            // The model's prose rule: an empty `rootGroup` needs intendedForQualification.
            "empty rootGroup, not for qualification",
            known.clone(),
            json!({
                "suiteDefinitionName": "refusals",
                "rootGroup": "",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            400,
        ),
        (
            "empty rootGroup, explicitly not for qualification",
            known.clone(),
            json!({
                "suiteDefinitionName": "refusals",
                "rootGroup": "",
                "intendedForQualification": false,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            400,
        ),
        (
            "empty rootGroup for a qualification suite is accepted",
            known.clone(),
            json!({
                "suiteDefinitionName": "refusals-qualification",
                "rootGroup": "",
                "intendedForQualification": true,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            200,
        ),
    ];

    for (case, id, configuration, expected_status) in rows {
        let answer = update_definition(&emulator, &id, &configuration).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected the modelled ValidationException, got {:?} {}",
                answer.error_code,
                answer.body
            );
        }
        if expected_status == 404 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: an id the store does not hold is a not-found, got {:?} {}",
                answer.error_code,
                answer.body
            );
            assert_eq!(
                answer.member("message").as_str(),
                Some(format!("Unable to find resource with suiteDefinitionId: {id}").as_str()),
                "case {case:?}: expected the AWS not-found template, got {}",
                answer.body
            );
        }
    }

    // The refusals left the stored definition where it was; only the accepted one moved it.
    let stored = get_definition(&emulator, &known, None).await;
    assert_eq!(
        stored.string("latestVersion"),
        "v2",
        "case {:?}: exactly one of the rows above may record a version, got {}",
        "refusals",
        stored.body
    );
}

/// A definition's version LABELS are bounded: past the retention count the lowest-ordinal
/// labels age out, the most recent ones stay addressable (each answering the one live
/// configuration), and the labels keep counting by ORDINAL (so `v10` follows `v9`).
#[tokio::test]
async fn version_history_is_bounded_and_counts_by_ordinal() {
    let emulator = Emulator::new();
    let keep = iotdeviceadvisor::state::MAX_VERSIONS_PER_DEFINITION;
    let configuration = |n: usize| {
        json!({
            "suiteDefinitionName": format!("bounded-{n}"),
            "rootGroup": format!("group-{n}"),
            "devicePermissionRoleArn": ROLE_ARN,
        })
    };

    let id = emulator
        .create_definition(&configuration(1), "us-east-1")
        .await
        .string("suiteDefinitionId");

    // `keep` updates on top of the created v1: one version more than retention allows.
    for n in 2..=keep + 1 {
        let updated = update_definition(&emulator, &id, &configuration(n)).await;
        assert_eq!(
            updated.string("suiteDefinitionVersion"),
            format!("v{n}"),
            "case {:?}: version labels count by ordinal, got {}",
            format!("update {n}"),
            updated.body
        );
    }

    let latest = get_definition(&emulator, &id, None).await;
    assert_eq!(
        latest.string("latestVersion"),
        format!("v{}", keep + 1),
        "case {:?}: latestVersion must be the last version recorded, got {}",
        "bounded history",
        latest.body
    );
    assert_eq!(
        latest.member("suiteDefinitionConfiguration"),
        &configuration(keep + 1),
        "case {:?}: the latest read must answer the last configuration recorded, got {}",
        "bounded history",
        latest.body
    );

    // The oldest label aged out; every retained one still answers the live configuration.
    let aged_out = get_definition(&emulator, &id, Some("v1")).await;
    assert_eq!(
        (aged_out.status, aged_out.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {:?}: a version past retention must read as not found, got {} {}",
        "bounded history",
        aged_out.status,
        aged_out.body
    );
    for n in 2..=keep + 1 {
        let historic = get_definition(&emulator, &id, Some(&format!("v{n}"))).await;
        assert_eq!(
            historic.status,
            200,
            "case {:?}: version v{n} must be retained, got {} {}",
            "bounded history",
            historic.status,
            historic.body
        );
        assert_eq!(
            historic.member("suiteDefinitionConfiguration"),
            &configuration(keep + 1),
            "case {:?}: version v{n} must answer the CURRENT configuration, got {}",
            "bounded history",
            historic.body
        );
    }

    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let held = state
        .lock()
        .definitions
        .get(&id)
        .map(|definition| definition.versions.len())
        .unwrap_or_default();
    assert_eq!(
        held, keep,
        "case {:?}: the store must retain at most {keep} version labels, it holds {held}",
        "bounded history"
    );
}
