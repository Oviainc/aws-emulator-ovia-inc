// SPDX-License-Identifier: CC0-1.0

//! A suite definition's ONE live configuration, and its timestamps as write stamps.
//!
//! Two rules, both about the single live record Device Advisor keeps per
//! `suiteDefinitionId`:
//!
//! 1. A `GetSuiteDefinition` that names a version the definition has handed out succeeds,
//!    echoes that version back and reports the newest one in `latestVersion`, but the
//!    `suiteDefinitionConfiguration` it serves is the definition's CURRENT one: there are no
//!    per-version configuration snapshots, so a version-qualified read and an unqualified
//!    read of the same definition answer the same configuration.
//! 2. `UpdateSuiteDefinition` stamps BOTH `createdAt` and `lastModifiedAt`/`lastUpdatedAt`
//!    of that one live record with the write instant, so a read after an update reports a
//!    `createdAt` strictly greater than the one a read before it reported, and successive
//!    writes are strictly ordered by either stamp.
//!
//! The tables span the configuration class the rules generalise over rather than the one
//! configuration the report cited: the rows of [`definition_rows`] (every `Protocol` the
//! model enumerates, device lists of each permitted size, both booleans, both
//! `suiteDefinitionName` bounds and both `rootGroup` forms) are used in turn as the created
//! configuration AND as the configuration an update replaces it with.

use serde_json::{json, Value};

use crate::suite_lifecycle::{definition_rows, Answer, Emulator, ROLE_ARN};

const REGION: &str = "us-east-1";

/// `UpdateSuiteDefinition` is `PATCH /suiteDefinitions/{suiteDefinitionId}`.
async fn update(emulator: &Emulator, id: &str, configuration: &Value) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!("/suiteDefinitions/{id}"),
            REGION,
            Some(json!({ "suiteDefinitionConfiguration": configuration })),
        )
        .await
}

/// `GetSuiteDefinition` is `GET /suiteDefinitions/{suiteDefinitionId}`, with
/// `suiteDefinitionVersion` bound to the query string.
async fn get(emulator: &Emulator, id: &str, version: Option<&str>) -> Answer {
    let path = match version {
        Some(version) => format!("/suiteDefinitions/{id}?suiteDefinitionVersion={version}"),
        None => format!("/suiteDefinitions/{id}"),
    };
    emulator.call("GET", &path, REGION, None).await
}

/// One timestamp member as epoch seconds, or a failure naming the case.
fn stamp(answer: &Answer, member: &str, case: &str) -> f64 {
    answer.member(member).as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a {member} timestamp, got {}",
            answer.body
        )
    })
}

/// Rule 1, over the whole configuration table: whichever version a read names, it serves
/// the definition's current configuration, echoes the version asked for and reports the
/// newest one as `latestVersion`.
#[tokio::test]
async fn a_version_qualified_read_serves_the_current_configuration() {
    let emulator = Emulator::new();
    let rows = definition_rows();

    for index in 0..rows.len() {
        let (initial_case, initial) = &rows[index];
        // The NEXT row is what the update replaces it with, so every row is exercised in
        // both positions as the loop walks the table.
        let (replacement_case, replacement) = &rows[(index + 1) % rows.len()];
        let case = format!("{initial_case} -> {replacement_case}");

        let created = emulator.create_definition(initial, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup create failed with {} {}",
            created.status, created.body
        );
        let id = created.string("suiteDefinitionId");

        let before = get(&emulator, &id, None).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteDefinition, got {} {}",
            before.status, before.body
        );
        let first_version = before.string("suiteDefinitionVersion");
        assert_eq!(
            before.member("suiteDefinitionConfiguration"),
            initial,
            "case {case:?}: the created configuration must read back, got {}",
            before.body
        );

        let updated = update(&emulator, &id, replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        let latest_version = updated.string("suiteDefinitionVersion");
        assert_ne!(
            latest_version, first_version,
            "case {case:?}: an update must hand out a new version, got {}",
            updated.body
        );

        // The version the create minted is still readable — and answers the CURRENT
        // configuration, not the content it was created with.
        let historical = get(&emulator, &id, Some(&first_version)).await;
        assert_eq!(
            historical.status, 200,
            "case {case:?}: a read naming {first_version} must succeed, got {} {}",
            historical.status, historical.body
        );
        assert_eq!(
            historical.member("suiteDefinitionConfiguration"),
            replacement,
            "case {case:?}: version {first_version} must serve the current configuration, got {}",
            historical.body
        );
        assert_eq!(
            historical.string("suiteDefinitionVersion"),
            first_version,
            "case {case:?}: a read must echo the version it was asked for, got {}",
            historical.body
        );
        assert_eq!(
            historical.string("latestVersion"),
            latest_version,
            "case {case:?}: latestVersion must be the newest version, got {}",
            historical.body
        );

        // Naming the newest version, and naming none, answer exactly the same content.
        let qualified_latest = get(&emulator, &id, Some(&latest_version)).await;
        let unqualified = get(&emulator, &id, None).await;
        assert_eq!(
            qualified_latest.member("suiteDefinitionConfiguration"),
            unqualified.member("suiteDefinitionConfiguration"),
            "case {case:?}: a qualified and an unqualified read must agree, got {} and {}",
            qualified_latest.body,
            unqualified.body
        );
        assert_eq!(
            unqualified.member("suiteDefinitionConfiguration"),
            replacement,
            "case {case:?}: the unqualified read must serve the configuration just stored, got {}",
            unqualified.body
        );
    }
}

/// A version the definition never handed out is still refused as not found — the read
/// VALIDATES the version it echoes.
#[tokio::test]
async fn a_version_the_definition_never_handed_out_is_not_found() {
    let emulator = Emulator::new();
    let configuration = json!({
        "suiteDefinitionName": "version-validation",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    });
    let id = emulator
        .create_definition(&configuration, REGION)
        .await
        .string("suiteDefinitionId");
    update(&emulator, &id, &configuration).await;

    // (case, the version named) — `SuiteDefinitionVersion` is `len 2..255`, so each is a
    // legal member value the definition simply never handed out.
    let unknown = [
        ("one past the newest", "v3".to_string()),
        ("far past the newest", "v4096".to_string()),
        ("not a label this service mints", "LATEST".to_string()),
        ("the shortest legal version string", "v0".to_string()),
        ("the longest legal version string", "v".repeat(255)),
    ];
    for (case, version) in unknown {
        let answer = get(&emulator, &id, Some(&version)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: version {version} must read as not found, got {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // And the versions it DID hand out still read.
    for version in ["v1", "v2"] {
        let answer = get(&emulator, &id, Some(version)).await;
        assert_eq!(
            answer.status, 200,
            "case {version:?}: a version the definition handed out must read, got {} {}",
            answer.status, answer.body
        );
    }
}

/// Rule 2, over the whole configuration table: an update rewrites both write stamps of the
/// one live record, so each successive write reads as strictly later than the last.
#[tokio::test]
async fn an_update_advances_both_write_stamps() {
    let emulator = Emulator::new();
    let rows = definition_rows();

    let (first_case, first) = rows[0].clone();
    let created = emulator.create_definition(&first, REGION).await;
    assert_eq!(
        created.status, 200,
        "case {first_case:?}: setup create failed with {} {}",
        created.status, created.body
    );
    let id = created.string("suiteDefinitionId");

    let before = get(&emulator, &id, None).await;
    let mut previous_created = stamp(&before, "createdAt", &first_case);
    let mut previous_modified = stamp(&before, "lastModifiedAt", &first_case);
    assert!(
        previous_modified >= previous_created,
        "case {first_case:?}: lastModifiedAt must not precede createdAt, got \
         {previous_modified} and {previous_created}"
    );

    for (case, configuration) in rows.iter().skip(1) {
        let updated = update(&emulator, &id, configuration).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        let update_created = stamp(&updated, "createdAt", case);
        let update_modified = stamp(&updated, "lastUpdatedAt", case);
        assert!(
            update_created > previous_created,
            "case {case:?}: the update response must advance createdAt past \
             {previous_created}, got {update_created}"
        );
        assert!(
            update_modified >= update_created,
            "case {case:?}: lastUpdatedAt must not precede createdAt, got {update_modified} \
             and {update_created}"
        );

        let read = get(&emulator, &id, None).await;
        let read_created = stamp(&read, "createdAt", case);
        let read_modified = stamp(&read, "lastModifiedAt", case);
        assert!(
            read_created > previous_created,
            "case {case:?}: a read after the update must report a createdAt past \
             {previous_created}, got {read_created}"
        );
        assert!(
            read_modified > previous_modified,
            "case {case:?}: a read after the update must report a lastModifiedAt past \
             {previous_modified}, got {read_modified}"
        );
        assert_eq!(
            read_created, update_created,
            "case {case:?}: the read must report the write stamp the update minted, got \
             {read_created} and {update_created}"
        );

        // A read never moves a stamp: reading again, and reading by version, report the
        // same instants.
        let again = get(&emulator, &id, Some(&read.string("latestVersion"))).await;
        assert_eq!(
            (
                stamp(&again, "createdAt", case),
                stamp(&again, "lastModifiedAt", case)
            ),
            (read_created, read_modified),
            "case {case:?}: a read must not move the write stamps, got {}",
            again.body
        );

        previous_created = read_created;
        previous_modified = read_modified;
    }
}

/// The write stamps belong to the record the call named: an update of one definition leaves
/// another definition's stamps and configuration exactly where they were.
#[tokio::test]
async fn an_update_moves_only_the_named_definitions_stamps() {
    let emulator = Emulator::new();
    let configuration = |name: &str| {
        json!({
            "suiteDefinitionName": name,
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        })
    };

    let target = emulator
        .create_definition(&configuration("target"), REGION)
        .await
        .string("suiteDefinitionId");
    let bystander = emulator
        .create_definition(&configuration("bystander"), REGION)
        .await
        .string("suiteDefinitionId");

    let before = get(&emulator, &bystander, None).await;
    let before_created = stamp(&before, "createdAt", "bystander");
    let before_modified = stamp(&before, "lastModifiedAt", "bystander");

    let updated = update(&emulator, &target, &configuration("target-v2")).await;
    assert_eq!(
        updated.status, 200,
        "case {:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
        "bystander",
        updated.status,
        updated.body
    );

    let after = get(&emulator, &bystander, None).await;
    assert_eq!(
        (
            stamp(&after, "createdAt", "bystander"),
            stamp(&after, "lastModifiedAt", "bystander")
        ),
        (before_created, before_modified),
        "case {:?}: an update must not move another definition's write stamps, got {}",
        "bystander",
        after.body
    );
    assert_eq!(
        after.member("suiteDefinitionConfiguration"),
        &configuration("bystander"),
        "case {:?}: an update must not touch another definition's configuration, got {}",
        "bystander",
        after.body
    );
    assert_eq!(
        after.string("latestVersion"),
        "v1",
        "case {:?}: an update must not advance another definition's version, got {}",
        "bystander",
        after.body
    );
}
