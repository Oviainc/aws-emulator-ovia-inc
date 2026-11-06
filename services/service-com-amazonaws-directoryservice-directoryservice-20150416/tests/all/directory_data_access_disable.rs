// SPDX-License-Identifier: CC0-1.0

//! Withdrawing Directory Service Data API access — `DisableDirectoryDataAccess`,
//! read back through `DescribeDirectoryDataAccess` — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the lifecycle suite's, reached
//! as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the
//! state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a data-access toggle records the transition it starts and the clock settles it: a
//!   disable moves the directory out of `Enabled` into the transient `Disabling`, and
//!   `Disabled` falls due one transition span later — over a table of directories
//!   differing in type (Simple AD / Managed Microsoft AD), size, edition and network
//!   type, with an enabled directory in the same store left alone as the control, so a
//!   status that is not per-directory cannot pass;
//! * the toggle round-trips: a directory whose access has been withdrawn can be enabled
//!   again, and enabling and disabling in turn keeps reporting the status the store
//!   holds;
//! * a disable is refused when the directory is already in the state it asks for
//!   (`DirectoryInDesiredStateException`) — while `Disabling`, once `Disabled`, and on a
//!   directory nothing ever enabled — while an enabled directory in the same store still
//!   accepts one;
//! * a disable needs a directory that is in service: one still being built by the
//!   transition its create owes is refused with `DirectoryUnavailableException`, and the
//!   same directory accepts the call once that stage transition has fallen due;
//! * the disable resolves the `DirectoryId` in the store and refuses an id no directory
//!   is stored under with `DirectoryDoesNotExistException` — over a table of well-formed
//!   ids differing in digits and form — while an id the MODEL refuses is refused before
//!   that resolution happens at all, and the operation never answers the scaffold's
//!   `NotImplemented`.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::ops::types::DATA_ACCESS_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, string, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The status a directory nothing has enabled carries, and the one a withdrawal settles
/// back on: the `Disabled` value of the modelled `DataAccessStatus` enum, whose whole
/// catalog the crate generates from the model as [`DATA_ACCESS_STATUS_VALUES`].
const DISABLED: &str = "Disabled";

/// The status a directory carries between the disable and the transition it owes.
const DISABLING: &str = "Disabling";

/// The status a directory carries once an enable's transition has fallen due.
const ENABLED: &str = "Enabled";

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="DisableDirectoryDataAccess")`), so every
/// row here is a value the boundary validator admits and the handler must resolve in the
/// store. The rows are that shape with different digits, including both extremes of the
/// alphabet.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-5c4b3a2190", "digits and letters interleaved"),
];

/// One row of the directory table: how the directory is created, and whether this case
/// withdraws its data access or leaves it enabled as the control.
struct DisableCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    disabled: bool,
}

/// The `DataAccessStatus` `DescribeDirectoryDataAccess` reports for `directory_id`,
/// checked against the modelled catalog.
async fn data_access_status(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
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
async fn create(router: &MultiServiceRouter, row: &DisableCase) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

/// A toggle call that must be accepted, asserting the operation's modelled output has no
/// members.
async fn toggle(router: &MultiServiceRouter, operation: &str, directory_id: &str, case: &str) {
    let answer = ok(router, operation, json!({ "DirectoryId": directory_id }), case).await;
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: {operation} has no modelled output members, got {answer}"
    );
}

/// The table of directories both toggle suites below drive, differing along everything
/// the two creates persist — directory type, size, edition and network type — plus
/// whether this row is withdrawn at all, so the control row proves the status is
/// per-directory rather than per-store.
fn rows() -> Vec<DisableCase> {
    vec![
        DisableCase {
            case: "a minimal Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("disable-simple.example.com"),
            disabled: true,
        },
        DisableCase {
            case: "a large Simple AD directory with a dual-stack network",
            operation: "CreateDirectory",
            body: json!({
                "Name": "disable-large.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack"
            }),
            disabled: true,
        },
        DisableCase {
            case: "a minimal Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("disable-corp.example.com"),
            disabled: true,
        },
        DisableCase {
            case: "a Standard-edition Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "disable-standard.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            disabled: true,
        },
        DisableCase {
            case: "the control directory, enabled and left alone",
            operation: "CreateDirectory",
            body: minimal_create("disable-control.example.com"),
            disabled: false,
        },
    ]
}

/// Create every row, let the stage transition its create owes fall due, enable data
/// access on all of them and let that transition fall due too, so every directory in the
/// store starts the case below `Enabled`.
async fn store_with_enabled_directories(
    router: &MultiServiceRouter,
    rows: &[DisableCase],
) -> Vec<(usize, String)> {
    let mut ids: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        ids.push((index, create(router, row).await));
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (index, id) in &ids {
        toggle(router, "EnableDirectoryDataAccess", id, rows[*index].case).await;
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (index, id) in &ids {
        let case = rows[*index].case;
        let status = data_access_status(router, id, case).await;
        assert_eq!(
            status, ENABLED,
            "case {case:?}: the enable settles on {ENABLED} before the disable is \
             exercised, got {status:?}"
        );
    }
    ids
}

#[tokio::test]
async fn a_disable_records_the_transition_the_clock_then_settles() {
    let router = service();
    let rows = rows();
    let ids = store_with_enabled_directories(&router, &rows).await;

    for (index, id) in &ids {
        if rows[*index].disabled {
            toggle(&router, "DisableDirectoryDataAccess", id, rows[*index].case).await;
        }
    }

    // The instant after the disable the transition it owes is still outstanding, so the
    // transient status is observable — and the control directory is untouched.
    for (index, id) in &ids {
        let case = rows[*index].case;
        let expected = if rows[*index].disabled {
            DISABLING
        } else {
            ENABLED
        };
        let status = data_access_status(&router, id, case).await;
        assert_eq!(
            status, expected,
            "case {case:?}: straight after the disable the directory reports {expected}, \
             got {status:?}"
        );
    }

    // The clock settles the transition the disable owed; nothing else did.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Two reads in a row, because a report is not a mutation: the second must answer
    // exactly what the first did.
    for pass in ["the first read", "the repeated read"] {
        for (index, id) in &ids {
            let case = rows[*index].case;
            let expected = if rows[*index].disabled {
                DISABLED
            } else {
                ENABLED
            };
            let status = data_access_status(&router, id, case).await;
            assert_eq!(
                status, expected,
                "case {case:?}: one transition span after the disable, on {pass}, the \
                 directory reports {expected}, got {status:?}"
            );
        }
    }

    // And the toggle round-trips: a withdrawn directory is enabled again, which is only
    // possible because the disable left it in the settled `Disabled` state.
    for (index, id) in &ids {
        if !rows[*index].disabled {
            continue;
        }
        let case = rows[*index].case;
        toggle(&router, "EnableDirectoryDataAccess", id, case).await;
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let status = data_access_status(&router, id, case).await;
        assert_eq!(
            status, ENABLED,
            "case {case:?}: a withdrawn directory enables again, got {status:?}"
        );
    }
}

#[tokio::test]
async fn a_disable_is_refused_when_the_directory_is_already_in_that_state() {
    let router = service();
    let rows = rows();
    let ids = store_with_enabled_directories(&router, &rows).await;

    for (index, id) in &ids {
        if rows[*index].disabled {
            toggle(&router, "DisableDirectoryDataAccess", id, rows[*index].case).await;
        }
    }

    // A second disable while the first transition is still outstanding asks for the state
    // the directory is already moving to, while the control directory — still `Enabled` —
    // accepts one.
    for (index, id) in &ids {
        let case = rows[*index].case;
        if rows[*index].disabled {
            refused(
                &router,
                "DisableDirectoryDataAccess",
                json!({ "DirectoryId": id }),
                "DirectoryInDesiredStateException",
                case,
            )
            .await;
        } else {
            toggle(&router, "DisableDirectoryDataAccess", id, case).await;
            let status = data_access_status(&router, id, case).await;
            assert_eq!(
                status, DISABLING,
                "case {case:?}: the control directory withdraws like any other, got \
                 {status:?}"
            );
        }
    }

    // Once the transition has fallen due the directory IS in the state the disable asks
    // for, so the refusal holds for the settled status too.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (index, id) in &ids {
        let status = data_access_status(&router, id, rows[*index].case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: the withdrawal settles on {DISABLED}, got {status:?}",
            rows[*index].case
        );
        refused(
            &router,
            "DisableDirectoryDataAccess",
            json!({ "DirectoryId": id }),
            "DirectoryInDesiredStateException",
            rows[*index].case,
        )
        .await;
    }

    // A directory nothing ever enabled is already in that state as well, over both
    // creates.
    for row in [
        DisableCase {
            case: "a Simple AD directory nothing ever enabled",
            operation: "CreateDirectory",
            body: minimal_create("never-enabled-simple.example.com"),
            disabled: true,
        },
        DisableCase {
            case: "a Managed Microsoft AD directory nothing ever enabled",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("never-enabled-corp.example.com"),
            disabled: true,
        },
    ] {
        let id = create(&router, &row).await;
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let status = data_access_status(&router, &id, row.case).await;
        assert_eq!(
            status, DISABLED,
            "case {:?}: a directory nothing enabled reports {DISABLED}, got {status:?}",
            row.case
        );
        refused(
            &router,
            "DisableDirectoryDataAccess",
            json!({ "DirectoryId": &id }),
            "DirectoryInDesiredStateException",
            row.case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_disable_needs_a_directory_that_is_in_service() {
    let router = service();

    // Both creates leave the directory in the transient stage its create owes a
    // transition out of, so both rows exercise the window before it falls due.
    let rows = vec![
        DisableCase {
            case: "a Simple AD directory still being built",
            operation: "CreateDirectory",
            body: minimal_create("pending-disable-simple.example.com"),
            disabled: true,
        },
        DisableCase {
            case: "a Managed Microsoft AD directory still being built",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("pending-disable-corp.example.com"),
            disabled: true,
        },
    ];

    let mut ids: Vec<(&DisableCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        // The stage is checked before the status is, so a directory still being built is
        // refused for being out of service rather than for being already `Disabled`.
        refused(
            &router,
            "DisableDirectoryDataAccess",
            json!({ "DirectoryId": &id }),
            "DirectoryUnavailableException",
            row.case,
        )
        .await;
        ids.push((row, id));
    }

    // The refusal was about the stage, so the very same directory takes the call once the
    // transition the create owed has fallen due and its access has been enabled.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in &ids {
        toggle(&router, "EnableDirectoryDataAccess", id, row.case).await;
        tokio::time::sleep(PAST_ONE_SPAN).await;
        toggle(&router, "DisableDirectoryDataAccess", id, row.case).await;
        let status = data_access_status(&router, id, row.case).await;
        assert_eq!(
            status, DISABLING,
            "case {:?}: the disable is admitted once the directory is in service, got \
             {status:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_disable_resolves_the_directory_in_the_store() {
    let router = service();

    // One stored directory, so the store is not empty and a refusal cannot come from
    // there being nothing at all.
    let stored = create_directory(
        &router,
        minimal_create("disable-resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    toggle(
        &router,
        "EnableDirectoryDataAccess",
        &stored,
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        refused(
            &router,
            "DisableDirectoryDataAccess",
            json!({ "DirectoryId": absent }),
            "DirectoryDoesNotExistException",
            class,
        )
        .await;
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
            "DisableDirectoryDataAccess",
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
    toggle(
        &router,
        "DisableDirectoryDataAccess",
        &stored,
        "the stored directory",
    )
    .await;
    let status = data_access_status(&router, &stored, "the stored directory").await;
    assert_eq!(
        status, DISABLING,
        "case {:?}: the stored directory withdraws and reports back, got {status:?}",
        "the stored directory"
    );
}
