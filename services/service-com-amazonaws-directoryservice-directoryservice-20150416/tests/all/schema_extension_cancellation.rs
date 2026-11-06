// SPDX-License-Identifier: CC0-1.0

//! `CancelSchemaExtension`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test is that a cancel winds back the stored extension the request names
//! whenever that extension's own status is one the operation documents as cancellable,
//! leaving it in the transient `CancelInProgress` the clock settles on `Cancelled`, and
//! refuses every other request from the same stored record: an id naming no stored
//! directory or no extension of it is the modelled "entity does not exist", and an
//! extension past the cancellable states is the modelled client error. The table rows
//! differ along everything a start varies in (description form and length, LDIF size,
//! whether a snapshot was taken first) and, for the refusals, along the status the stored
//! extension is in.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The values of the modelled `SchemaExtensionStatus` enum this suite names
/// (`smithy_query(query="shape",shape="SchemaExtensionStatus")` lists `Initializing`,
/// `CreatingSnapshot`, `UpdatingSchema`, `Replicating`, `CancelInProgress`,
/// `RollbackInProgress`, `Cancelled`, `Failed`, `Completed`).
const STATUS_INITIALIZING: &str = "Initializing";
const STATUS_CANCEL_IN_PROGRESS: &str = "CancelInProgress";
const STATUS_CANCELLED: &str = "Cancelled";
const STATUS_COMPLETED: &str = "Completed";

/// The maximum length of the modelled `Description`, from its `@length` trait
/// (`min: 0, max: 128`).
const DESCRIPTION_MAX: usize = 128;

/// A well-formed `DirectoryId` (`^d-[0-9a-f]{10}$`) that is never created.
const ABSENT_DIRECTORY_ID: &str = "d-9f0c3a7b12";

/// A well-formed `SchemaExtensionId` (`^e-[0-9a-f]{10}$`) that is never minted.
const ABSENT_SCHEMA_EXTENSION_ID: &str = "e-1a2b3c4d5e";

/// A Managed Microsoft AD directory past the stage transition its create owes, so it is
/// in service for the extensions the cases apply.
async fn directory_in_service(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// Start a schema extension of `directory_id`, returning its minted id.
async fn start(
    router: &MultiServiceRouter,
    directory_id: &str,
    description: &str,
    ldif: &str,
    snapshot_first: bool,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "StartSchemaExtension",
        json!({
            "DirectoryId": directory_id,
            "Description": description,
            "LdifContent": ldif,
            "CreateSnapshotBeforeSchemaExtension": snapshot_first,
        }),
        case,
    )
    .await;
    string(&answer, "SchemaExtensionId", case)
}

/// `CancelSchemaExtension`, asserting it was accepted and carried no members back: the
/// modelled `CancelSchemaExtensionResult` declares none.
async fn cancel(
    router: &MultiServiceRouter,
    directory_id: &str,
    schema_extension_id: &str,
    case: &str,
) {
    let answer = ok(
        router,
        "CancelSchemaExtension",
        json!({
            "DirectoryId": directory_id,
            "SchemaExtensionId": schema_extension_id,
        }),
        case,
    )
    .await;
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: expected an empty CancelSchemaExtension result, got {answer}"
    );
}

/// The reported extension `schema_extension_id` of `directory_id`, read back through
/// `ListSchemaExtensions`.
async fn reported(
    router: &MultiServiceRouter,
    directory_id: &str,
    schema_extension_id: &str,
    case: &str,
) -> Value {
    let answer = ok(
        router,
        "ListSchemaExtensions",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    let found = answer["SchemaExtensionsInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected SchemaExtensionsInfo, got {answer}"))
        .clone();
    found
        .iter()
        .find(|entry| entry["SchemaExtensionId"] == json!(schema_extension_id))
        .unwrap_or_else(|| panic!("case {case:?}: expected {schema_extension_id} among {found:?}"))
        .clone()
}

/// The `SchemaExtensionStatus` of one reported extension.
async fn status(
    router: &MultiServiceRouter,
    directory_id: &str,
    schema_extension_id: &str,
    case: &str,
) -> String {
    let entry = reported(router, directory_id, schema_extension_id, case).await;
    string(&entry, "SchemaExtensionStatus", case)
}

/// A numeric member of a reported extension, in epoch seconds — the unit an `awsJson1_1`
/// timestamp is rendered in.
fn epoch_member(entry: &Value, member: &str, case: &str) -> f64 {
    entry[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {entry}"))
}

/// One row of the cancel table: how the extension it cancels was started.
struct CancelCase {
    case: &'static str,
    description: String,
    ldif: String,
    snapshot_first: bool,
}

#[tokio::test]
async fn a_cancel_winds_back_an_extension_still_in_a_cancellable_status() {
    let router = service();
    let directory = directory_in_service(&router, "corp.example.com", "the cancelled directory").await;

    // The rows differ along everything a start varies in: the form and length of the
    // `Description` (its `@length` trait admits up to 128 characters and its pattern
    // admits punctuation and whitespace after the first), the size and shape of the
    // `LdifContent`, and whether the directory was snapshotted first — none of which the
    // cancel rule depends on.
    let cases = vec![
        CancelCase {
            case: "a plain description, no snapshot",
            description: "Add_user_attributes".to_string(),
            ldif: "dn: CN=Shoe-Size,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n\
                   changetype: add\nobjectClass: attributeSchema\n"
                .to_string(),
            snapshot_first: false,
        },
        CancelCase {
            case: "a punctuated description with whitespace, snapshot first",
            description: "pre-flight schema change #2 @ 03:00".to_string(),
            ldif: "dn: CN=Badge-Id,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n"
                .to_string(),
            snapshot_first: true,
        },
        CancelCase {
            case: "a description at the published length bound, a large LDIF file",
            description: "d".repeat(DESCRIPTION_MAX),
            ldif: "dn: CN=Bulk,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n".repeat(40),
            snapshot_first: false,
        },
        CancelCase {
            case: "a description and an LDIF file of one character each",
            description: "a".to_string(),
            ldif: "#".to_string(),
            snapshot_first: false,
        },
    ];

    let mut cancelled: Vec<(&CancelCase, String)> = Vec::new();
    // One extension nothing cancels, so the cancels can be shown to touch only the
    // extension each request names.
    let bystander = start(
        &router,
        &directory,
        "left_alone",
        "dn: CN=Bystander,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n",
        false,
        "the bystander",
    )
    .await;

    for row in &cases {
        let case = row.case;
        let id = start(
            &router,
            &directory,
            &row.description,
            &row.ldif,
            row.snapshot_first,
            case,
        )
        .await;
        let before = status(&router, &directory, &id, case).await;
        assert_eq!(
            before, STATUS_INITIALIZING,
            "case {case:?}: expected a fresh extension in {STATUS_INITIALIZING}, got {before:?}"
        );

        cancel(&router, &directory, &id, case).await;

        // The cancel answers before the winding back is done, so a caller listing
        // straight afterwards sees the transient status and no end instant yet.
        let entry = reported(&router, &directory, &id, case).await;
        assert_eq!(
            entry["SchemaExtensionStatus"],
            json!(STATUS_CANCEL_IN_PROGRESS),
            "case {case:?}: expected {STATUS_CANCEL_IN_PROGRESS} straight after the cancel, got \
             {entry}"
        );
        assert!(
            entry["EndDateTime"].is_null(),
            "case {case:?}: expected no EndDateTime while the cancel is still under way, got \
             {entry}"
        );
        assert_eq!(
            entry["Description"],
            json!(row.description),
            "case {case:?}: expected Description {:?} to survive the cancel, got {entry}",
            row.description
        );
        cancelled.push((row, id));
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Once the transition the cancel owes has fallen due every cancelled extension has
    // settled on the terminal status, with an end instant at or after its start.
    for (row, id) in &cancelled {
        let case = row.case;
        let entry = reported(&router, &directory, id, case).await;
        assert_eq!(
            entry["SchemaExtensionStatus"],
            json!(STATUS_CANCELLED),
            "case {case:?}: expected {STATUS_CANCELLED} once the owed transition fell due, got \
             {entry}"
        );
        let started_at = epoch_member(&entry, "StartDateTime", case);
        let ended_at = epoch_member(&entry, "EndDateTime", case);
        assert!(
            ended_at >= started_at,
            "case {case:?}: expected EndDateTime {ended_at} at or after StartDateTime \
             {started_at}"
        );

        // A listing is a read: the settled status does not move on being read again.
        let again = status(&router, &directory, id, case).await;
        assert_eq!(
            again, STATUS_CANCELLED,
            "case {case:?}: expected {STATUS_CANCELLED} to hold across reads, got {again:?}"
        );
    }

    // A cancel winds back only the extension its request named: the one nothing cancelled
    // completed on the transition its own start owed.
    let bystander_status = status(&router, &directory, &bystander, "the bystander").await;
    assert_eq!(
        bystander_status, STATUS_COMPLETED,
        "expected the extension nothing cancelled to reach {STATUS_COMPLETED}, got \
         {bystander_status:?}"
    );
}

#[tokio::test]
async fn a_cancel_is_refused_for_an_absent_or_uncancellable_extension() {
    let router = service();
    let directory = directory_in_service(&router, "corp.example.com", "refusals").await;
    let other = directory_in_service(&router, "sales.example.org", "refusals").await;
    let ldif = "dn: CN=Refused,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n";

    // An extension of ANOTHER directory is as absent as one nothing started.
    let other_directorys = start(
        &router,
        &other,
        "other_directory",
        ldif,
        false,
        "an extension of another directory",
    )
    .await;

    for (case, directory_id, schema_extension_id) in [
        (
            "an absent DirectoryId",
            ABSENT_DIRECTORY_ID.to_string(),
            ABSENT_SCHEMA_EXTENSION_ID.to_string(),
        ),
        (
            "an absent SchemaExtensionId",
            directory.clone(),
            ABSENT_SCHEMA_EXTENSION_ID.to_string(),
        ),
        (
            "an extension of another directory",
            directory.clone(),
            other_directorys.clone(),
        ),
    ] {
        refused(
            &router,
            "CancelSchemaExtension",
            json!({
                "DirectoryId": directory_id,
                "SchemaExtensionId": schema_extension_id,
            }),
            "EntityDoesNotExistException",
            case,
        )
        .await;
    }

    // The refused cancels wound nothing back: the other directory's extension is still
    // the service's to complete.
    let case = "an extension of another directory";
    let untouched = status(&router, &other, &other_directorys, case).await;
    assert!(
        untouched == STATUS_INITIALIZING || untouched == STATUS_COMPLETED,
        "case {case:?}: expected a refused cancel to leave the extension on the path its own \
         start set, got {untouched:?}"
    );

    // An extension past the cancellable states is refused with the modelled client
    // error: one the clock already completed, one this emulator is still winding back,
    // and one already cancelled.
    let completed = start(&router, &directory, "already_done", ldif, false, "completed").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = status(&router, &directory, &completed, "completed").await;
    assert_eq!(
        settled, STATUS_COMPLETED,
        "expected the extension to have completed before the cancel, got {settled:?}"
    );

    let winding_back = start(
        &router,
        &directory,
        "being_cancelled",
        ldif,
        false,
        "cancel in progress",
    )
    .await;
    cancel(&router, &directory, &winding_back, "cancel in progress").await;

    for (case, schema_extension_id, expected_status) in [
        ("an extension already completed", &completed, STATUS_COMPLETED),
        (
            "an extension still being cancelled",
            &winding_back,
            STATUS_CANCEL_IN_PROGRESS,
        ),
    ] {
        let (status_code, parsed) = call(
            &router,
            "CancelSchemaExtension",
            json!({
                "DirectoryId": &directory,
                "SchemaExtensionId": schema_extension_id,
            }),
        )
        .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected 400, got {status_code} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("ClientException"),
            "case {case:?}: expected ClientException, got {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected_status) && message.contains(STATUS_INITIALIZING),
            "case {case:?}: expected the message to name the status {expected_status} that \
             refused the cancel and the states that admit one, got {message:?}"
        );

        // The refusal left the stored status where it was.
        let after = status(&router, &directory, schema_extension_id, case).await;
        assert_eq!(
            after, expected_status,
            "case {case:?}: expected the refused cancel to leave the status at \
             {expected_status}, got {after:?}"
        );
    }

    // Once the cancel it is winding back settles, a further cancel of the terminal record
    // is refused the same way.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = status(&router, &directory, &winding_back, "already cancelled").await;
    assert_eq!(
        settled, STATUS_CANCELLED,
        "expected the cancelled extension to settle on {STATUS_CANCELLED}, got {settled:?}"
    );
    refused(
        &router,
        "CancelSchemaExtension",
        json!({
            "DirectoryId": &directory,
            "SchemaExtensionId": &winding_back,
        }),
        "ClientException",
        "an extension already cancelled",
    )
    .await;
    let after = status(&router, &directory, &winding_back, "already cancelled").await;
    assert_eq!(
        after, STATUS_CANCELLED,
        "expected the refused cancel to leave the status at {STATUS_CANCELLED}, got {after:?}"
    );
}
