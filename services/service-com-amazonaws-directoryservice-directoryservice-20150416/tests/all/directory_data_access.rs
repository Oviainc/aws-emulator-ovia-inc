// SPDX-License-Identifier: CC0-1.0

//! Directory Service Data API access — `EnableDirectoryDataAccess` and
//! `DescribeDirectoryDataAccess` — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached
//! as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the
//! state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a directory's data access is reported from the store: `Disabled` until an enable
//!   records otherwise, the transient `Enabling` while the transition that enable owes
//!   is outstanding, and `Enabled` once the clock has settled it one transition span
//!   later — over a table of directories differing in type (Simple AD / Managed
//!   Microsoft AD), size, edition and network type, with an untouched directory in the
//!   same store as the control, so a status that is not per-directory cannot pass, and
//!   every reported value checked against the catalog the crate generates from the
//!   model;
//! * an enable is refused when the directory is already in the state it asks for
//!   (`DirectoryInDesiredStateException`), both while `Enabling` and once `Enabled`,
//!   while the control directory in the same store still accepts one;
//! * an enable needs a directory that is in service: one still being built by the
//!   transition its create owes is refused with `DirectoryUnavailableException`, and the
//!   same call succeeds once that stage transition has fallen due — while the read
//!   answers throughout, because a read needs no particular stage;
//! * both operations resolve the `DirectoryId` in the store and refuse an id no
//!   directory is stored under with `DirectoryDoesNotExistException` — over a table of
//!   well-formed ids differing in digits and form — while an id the MODEL refuses is
//!   refused before that resolution happens at all, and neither operation ever answers
//!   the scaffold's `NotImplemented`.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::ops::types::DATA_ACCESS_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, string, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The status a directory nothing has enabled carries: the `Disabled` value of the
/// modelled `DataAccessStatus` enum, whose whole catalog the crate generates from the
/// model as [`DATA_ACCESS_STATUS_VALUES`].
const DISABLED: &str = "Disabled";

/// The status a directory carries between the enable and the transition it owes.
const ENABLING: &str = "Enabling";

/// The status a directory carries once that transition has fallen due.
const ENABLED: &str = "Enabled";

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="EnableDirectoryDataAccess")`), so every
/// row here is a value the boundary validator admits and the handler must resolve in the
/// store. The rows are that shape with different digits, including both extremes of the
/// alphabet.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-9f8e7d6c5b", "digits and letters interleaved"),
];

/// One row of the directory table: how the directory is created, and whether this case
/// enables data access on it or leaves it as the untouched control.
struct AccessCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    enabled: bool,
}

/// The `DataAccessStatus` `DescribeDirectoryDataAccess` reports for `directory_id`,
/// checked against the modelled catalog.
async fn data_access_status(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "DescribeDirectoryDataAccess",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    let status = string(&answer, "DataAccessStatus", case);
    assert!(
        DATA_ACCESS_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: expected a modelled DataAccessStatus one of \
         {DATA_ACCESS_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &AccessCase) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

#[tokio::test]
async fn data_access_status_is_reported_from_the_store_for_each_directory() {
    let router = service();

    // The rows differ along everything the two creates persist — directory type, size,
    // edition, network type — plus whether this case enables data access at all, so the
    // control row proves the status is per-directory rather than per-store.
    let rows = vec![
        AccessCase {
            case: "a minimal Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("simple.example.com"),
            enabled: true,
        },
        AccessCase {
            case: "a large Simple AD directory with a dual-stack network",
            operation: "CreateDirectory",
            body: json!({
                "Name": "large.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack"
            }),
            enabled: true,
        },
        AccessCase {
            case: "a minimal Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("corp.example.com"),
            enabled: true,
        },
        AccessCase {
            case: "a Standard-edition Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "standard.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            enabled: true,
        },
        AccessCase {
            case: "the control directory, never enabled",
            operation: "CreateDirectory",
            body: minimal_create("control.example.com"),
            enabled: false,
        },
    ];

    let mut ids: Vec<(&AccessCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        ids.push((row, id));
    }

    // Every directory is readable while it is still being built, and reports the status
    // the store holds for a directory nothing has enabled.
    for (row, id) in &ids {
        let status = data_access_status(&router, id, row.case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: a freshly created directory reports {DISABLED}, got {status:?}",
            row.case
        );
    }

    // The stage transition each create owes falls due, so the directories are in
    // service and an enable is admitted.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, id) in &ids {
        if !row.enabled {
            continue;
        }
        let answer = ok(
            &router,
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {:?}: EnableDirectoryDataAccess has no modelled output members, got \
             {answer}",
            row.case
        );
    }

    // The instant after the enable the transition it owes is still outstanding, so the
    // transient status is observable — and the control directory is untouched.
    for (row, id) in &ids {
        let status = data_access_status(&router, id, row.case).await;
        let expected = if row.enabled { ENABLING } else { DISABLED };
        assert_eq!(
            status, expected,
            "case {:?}: straight after the enable the directory reports {expected}, got \
             {status:?}",
            row.case
        );
    }

    // A second enable while the first transition is still outstanding asks for the
    // state the directory is already moving to.
    for (row, id) in &ids {
        if !row.enabled {
            continue;
        }
        refused(
            &router,
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": id }),
            "DirectoryInDesiredStateException",
            row.case,
        )
        .await;
    }

    // The clock settles the transition the enable owed; nothing else did.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, id) in &ids {
        let status = data_access_status(&router, id, row.case).await;
        let expected = if row.enabled { ENABLED } else { DISABLED };
        assert_eq!(
            status, expected,
            "case {:?}: one transition span after the enable the directory reports \
             {expected}, got {status:?}",
            row.case
        );
    }

    // Repeating the read does not move the status on: a report is not a mutation.
    for (row, id) in &ids {
        let status = data_access_status(&router, id, row.case).await;
        let expected = if row.enabled { ENABLED } else { DISABLED };
        assert_eq!(
            status, expected,
            "case {:?}: a repeated read reports {expected}, got {status:?}",
            row.case
        );
    }

    // An enable on an already-enabled directory is refused for the same reason, while
    // the control directory in the same store still accepts one: the refusal is about
    // the directory's state, not about the operation having been called before.
    for (row, id) in &ids {
        if row.enabled {
            refused(
                &router,
                "EnableDirectoryDataAccess",
                json!({ "DirectoryId": id }),
                "DirectoryInDesiredStateException",
                row.case,
            )
            .await;
        } else {
            ok(
                &router,
                "EnableDirectoryDataAccess",
                json!({ "DirectoryId": id }),
                row.case,
            )
            .await;
            let status = data_access_status(&router, id, row.case).await;
            assert_eq!(
                status, ENABLING,
                "case {:?}: the control directory enables like any other, got \
                 {status:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn an_enable_needs_a_directory_that_is_in_service() {
    let router = service();

    // Both creates leave the directory in the transient stage its create owes a
    // transition out of, so both rows exercise the window before it falls due.
    let rows = vec![
        AccessCase {
            case: "a Simple AD directory still being built",
            operation: "CreateDirectory",
            body: minimal_create("pending-simple.example.com"),
            enabled: true,
        },
        AccessCase {
            case: "a Managed Microsoft AD directory still being built",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("pending-corp.example.com"),
            enabled: true,
        },
    ];

    let mut ids: Vec<(&AccessCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        // The read needs no particular stage: it reports what the store holds.
        let status = data_access_status(&router, &id, row.case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: a directory still being built reports {DISABLED}, got {status:?}",
            row.case
        );
        refused(
            &router,
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": &id }),
            "DirectoryUnavailableException",
            row.case,
        )
        .await;
        ids.push((row, id));
    }

    // The refusal was about the stage, so the very same call is admitted once the
    // transition the create owed has fallen due.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in &ids {
        ok(
            &router,
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        let status = data_access_status(&router, id, row.case).await;
        assert_eq!(
            status, ENABLING,
            "case {:?}: the enable is admitted once the directory is in service, got \
             {status:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn both_operations_resolve_the_directory_in_the_store() {
    let router = service();

    // One stored directory, so the store is not empty and a refusal cannot come from
    // there being nothing at all.
    let stored = create_directory(
        &router,
        minimal_create("resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        for operation in ["EnableDirectoryDataAccess", "DescribeDirectoryDataAccess"] {
            refused(
                &router,
                operation,
                json!({ "DirectoryId": absent }),
                "DirectoryDoesNotExistException",
                class,
            )
            .await;
        }
    }

    // An id the MODEL refuses never reaches that resolution: the boundary validator
    // refuses it first, so the answer names neither the directory nor the scaffold.
    let malformed = [
        ("d-ABCDEF0123", "upper case, outside the pattern's alphabet"),
        ("d-0a1b2c3d", "too few digits"),
        ("d-0a1b2c3d4e5", "too many digits"),
        ("", "empty"),
    ];
    for (id, class) in malformed {
        let (status, parsed) = call(
            &router,
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {class:?}: the model refuses {id:?}, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
        assert!(
            !wire_type.contains("DirectoryDoesNotExistException"),
            "case {class:?}: the input is refused before the directory is resolved, got \
             {parsed}"
        );
        assert!(
            !wire_type.contains("NotImplemented"),
            "case {class:?}: the operation must not answer the scaffold signal, got \
             {parsed}"
        );
    }

    // And the stored directory still round-trips, so none of the refusals above touched
    // it.
    ok(
        &router,
        "EnableDirectoryDataAccess",
        json!({ "DirectoryId": &stored }),
        "the stored directory",
    )
    .await;
    let status = data_access_status(&router, &stored, "the stored directory").await;
    assert_eq!(
        status, ENABLING,
        "case {:?}: the stored directory enables and reports back, got {status:?}",
        "the stored directory"
    );
}
