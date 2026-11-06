// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `DescribeBackups` `Filter` means, driven over the wire.
//!
//! RULE: a `Filter` RESTRICTS the records a describe reports and asserts nothing about what
//! the account holds — the modelled `Filter` is "a filter used to restrict the results of
//! describe calls" — so a filter value naming no stored record selects none of them and the
//! listing is a successful, EMPTY page; the not-found errors are reachable only from the id
//! members the model ties them to (`DescribeBackupsRequest$BackupIds`: "If any IDs aren't
//! found, a `BackupNotFound` error occurs"). A `DeleteBackup` naming a backup the store does
//! not hold is refused with `BackupNotFound` whose `Message` is the requested `BackupId`
//! alone.
//!
//! Each case asserts the rule over a SET of inputs — every supported filter name, value
//! counts of zero, one and several, absent and present values mixed in one filter, and a
//! span of absent identifiers — rather than over one recorded call. The requests go through
//! the same front door as the other suites, whose harness is reused.

use serde_json::{json, Value};

use crate::backup_lifecycle::{
    available_file_system, backup_of, create_file_system_request, error_of, listed_backup_ids,
    lustre_configuration, windows_configuration, ABSENT_FILE_SYSTEM_IDS, VOLUME_IDS,
};
use crate::file_system_lifecycle::{call, emulator, with_client_request_token, PAST_TRANSITION};

/// Absent `BackupId`s spanning the modelled `BackupId` shape: pattern
/// `^(backup-[0-9a-f]{8,})$` with `length 12..128`, so the shortest id the pattern allows,
/// a mid-length one, and the longest the length bound allows.
fn absent_backup_ids() -> Vec<String> {
    vec![
        "backup-00000000".to_string(),
        "backup-0f1e2d3c4b5a69788".to_string(),
        format!("backup-{}", "f".repeat(121)),
    ]
}

/// Require a 200 and return the page, with the two invariants of an answered listing
/// checked: `Backups` is a present list, and a page that is not cut carries no `NextToken`.
async fn page(
    router: &smithy_server_runtime::MultiServiceRouter,
    label: &str,
    request: &Value,
) -> Value {
    let (status, body) = call(router, "DescribeBackups", request).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        body["Backups"].is_array(),
        "case {label:?}: expected a present Backups list, got {body}"
    );
    body
}

/// RULE: a filter value that names nothing the store holds selects no record, so the
/// listing answers 200 with an empty `Backups` list and no `NextToken` — for every supported
/// filter name, and whether the filter carries no values, one, or several.
#[tokio::test]
async fn a_filter_that_selects_nothing_answers_an_empty_page() {
    let router = emulator().await;
    let windows_id = available_file_system(
        &router,
        &create_file_system_request(
            "WINDOWS",
            &[router.subnet_a()],
            Some(windows_configuration(None)),
        ),
    )
    .await;
    let lustre_id = available_file_system(
        &router,
        &create_file_system_request(
            "LUSTRE",
            &[router.subnet_a()],
            Some(lustre_configuration(Some("PERSISTENT_1"), None)),
        ),
    )
    .await;
    let windows_backup = backup_of(&router, &windows_id).await["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let lustre_backup = backup_of(&router, &lustre_id).await["BackupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !windows_backup.is_empty() && !lustre_backup.is_empty(),
        "the setup must have stored two backups, got {windows_backup:?} and {lustre_backup:?}"
    );

    // (label, Filters) — one row per supported filter name
    // (`DescribeBackupsRequest$Filters`: "The supported names are file-system-id,
    // backup-type, file-system-type, and volume-id"), over value counts 0, 1 and 3, with
    // the enum rows naming a member of the modelled enum that no stored backup carries
    // (`BackupType`: AUTOMATIC | USER_INITIATED | AWS_BACKUP; `FileSystemType`: WINDOWS |
    // LUSTRE | ONTAP | OPENZFS).
    let mut empty_selections: Vec<(String, Value)> = vec![
        (
            "backup-type AUTOMATIC".to_string(),
            json!({ "Filters": [{ "Name": "backup-type", "Values": ["AUTOMATIC"] }] }),
        ),
        (
            "backup-type AWS_BACKUP".to_string(),
            json!({ "Filters": [{ "Name": "backup-type", "Values": ["AWS_BACKUP"] }] }),
        ),
        (
            "file-system-type ONTAP".to_string(),
            json!({ "Filters": [{ "Name": "file-system-type", "Values": ["ONTAP"] }] }),
        ),
        (
            "file-system-type OPENZFS".to_string(),
            json!({ "Filters": [{ "Name": "file-system-type", "Values": ["OPENZFS"] }] }),
        ),
        (
            "every absent file system at once".to_string(),
            json!({ "Filters": [{
                "Name": "file-system-id",
                "Values": ABSENT_FILE_SYSTEM_IDS,
            }] }),
        ),
        (
            // `Filter$Values` is modelled `length 0..20`, so an empty value set is a legal
            // filter — and one no record can satisfy.
            "a filter with no values".to_string(),
            json!({ "Filters": [{ "Name": "file-system-id", "Values": [] }] }),
        ),
        (
            "a present file system and an absent type".to_string(),
            json!({ "Filters": [
                { "Name": "file-system-id", "Values": [windows_id.clone()] },
                { "Name": "file-system-type", "Values": ["OPENZFS"] },
            ] }),
        ),
    ];
    for file_system_id in ABSENT_FILE_SYSTEM_IDS {
        empty_selections.push((
            format!("file-system-id {file_system_id}"),
            json!({ "Filters": [{ "Name": "file-system-id", "Values": [file_system_id] }] }),
        ));
    }
    for volume_id in VOLUME_IDS {
        empty_selections.push((
            format!("volume-id {volume_id}"),
            json!({ "Filters": [{ "Name": "volume-id", "Values": [volume_id] }] }),
        ));
    }

    for (label, request) in &empty_selections {
        let body = page(&router, label, request).await;
        assert_eq!(
            listed_backup_ids(&body),
            Vec::<String>::new(),
            "case {label:?}: expected an empty Backups list, got {body}"
        );
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {label:?}: an empty page must carry no NextToken, got {:?}",
            body["NextToken"]
        );
    }

    // The same filter names, with values that DO select: a value naming nothing beside a
    // value that matches selects the match, which is what makes a filter a selector rather
    // than an existence check.
    let selecting: &[(&str, Value, Vec<String>)] = &[
        (
            "an absent file system beside a stored one",
            json!({ "Filters": [{
                "Name": "file-system-id",
                "Values": [ABSENT_FILE_SYSTEM_IDS[0], windows_id.as_str()],
            }] }),
            vec![windows_backup.clone()],
        ),
        (
            "a volume beside a stored file system",
            json!({ "Filters": [
                { "Name": "file-system-id", "Values": [lustre_id.clone()] },
                { "Name": "backup-type", "Values": ["USER_INITIATED"] },
            ] }),
            vec![lustre_backup.clone()],
        ),
    ];
    for (label, request, expected) in selecting {
        let body = page(&router, label, request).await;
        assert_eq!(
            listed_backup_ids(&body),
            *expected,
            "case {label:?}: expected {expected:?}, got {body}"
        );
    }

    // And the teardown check a lifecycle guard makes: once the backups are gone, the
    // listing that selected them is empty rather than a refusal. A backup is deleted once
    // the clock has settled it out of CREATING.
    tokio::time::sleep(PAST_TRANSITION).await;
    for backup_id in [&windows_backup, &lustre_backup] {
        let (status, body) = call(
            &router,
            "DeleteBackup",
            &with_client_request_token(&json!({ "BackupId": backup_id })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {backup_id:?}: the delete must succeed, got {status} with body {body}"
        );
    }
    for (label, file_system_id) in [("windows", &windows_id), ("lustre", &lustre_id)] {
        let body = page(
            &router,
            label,
            &json!({ "Filters": [{ "Name": "file-system-id", "Values": [file_system_id] }] }),
        )
        .await;
        assert_eq!(
            listed_backup_ids(&body),
            Vec::<String>::new(),
            "case {label:?}: after the deletes the listing must be empty, got {body}"
        );
    }
}

/// RULE: not-found is reachable from the id member the model ties it to — `BackupIds` —
/// and an unsupported filter NAME is still the modelled `BadRequest`, so making filter
/// values selectors does not make this listing answer everything with a 200.
#[tokio::test]
async fn backup_ids_and_unsupported_filter_names_are_still_refused() {
    let router = emulator().await;

    for backup_id in absent_backup_ids() {
        let (status, code, message) = error_of(
            &router,
            "DescribeBackups",
            &json!({ "BackupIds": [backup_id.clone()] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BackupNotFound"),
            "case {backup_id:?}: expected HTTP 400 BackupNotFound, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("Backup '{backup_id}' does not exist."),
            "case {backup_id:?}: expected the Message to name the requested id, got {message:?}"
        );
    }

    // The `FilterName` members this listing does NOT select by, from the modelled
    // `FilterName` enum (file-system-id | backup-type | file-system-type | volume-id |
    // data-repository-type | file-cache-id | file-cache-type).
    for name in ["data-repository-type", "file-cache-id", "file-cache-type"] {
        let (status, code, message) = error_of(
            &router,
            "DescribeBackups",
            &json!({ "Filters": [{ "Name": name, "Values": ["anything"] }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {name:?}: expected HTTP 400 BadRequest, got {status} {code}"
        );
        assert!(
            message.contains(name),
            "case {name:?}: expected the Message to name the filter, got {message:?}"
        );
    }
}

/// RULE: `DeleteBackup`'s `BackupNotFound$Message` is the requested `BackupId` alone, while
/// the listing addressed by `BackupIds` keeps its own recorded wording — so the message a
/// caller reads is the one its operation records, built from the id the REQUEST carried.
#[tokio::test]
async fn delete_backup_reports_the_requested_id_as_its_message() {
    let router = emulator().await;

    for backup_id in absent_backup_ids() {
        let (status, code, message) = error_of(
            &router,
            "DeleteBackup",
            &with_client_request_token(&json!({ "BackupId": backup_id.clone() })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BackupNotFound"),
            "case {backup_id:?}: expected HTTP 400 BackupNotFound, got {status} {code}"
        );
        assert_eq!(
            message, backup_id,
            "case {backup_id:?}: expected the Message to be the requested id alone, got \
             {message:?}"
        );
        // The modelled `ErrorMessage` bound (`length 1..256`) holds for every id a request
        // can carry.
        assert!(
            (1..=256).contains(&message.len()),
            "case {backup_id:?}: expected a Message within the modelled length, got {} bytes",
            message.len()
        );

        let (_, _, listed) = error_of(
            &router,
            "DescribeBackups",
            &json!({ "BackupIds": [backup_id.clone()] }),
        )
        .await;
        assert_ne!(
            listed, message,
            "case {backup_id:?}: the listing keeps its own wording, got {listed:?}"
        );
    }
}
