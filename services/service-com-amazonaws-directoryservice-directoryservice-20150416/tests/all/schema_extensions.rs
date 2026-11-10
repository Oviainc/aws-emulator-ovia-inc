// SPDX-License-Identifier: CC0-1.0

//! The Directory Service schema extension slice, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test is that `StartSchemaExtension` persists the extension the request
//! describes and `ListSchemaExtensions` reports it back from that record: the table rows
//! differ along everything a start varies in — the `Description` form and length, the
//! size and shape of the `LdifContent`, and whether the directory was to be snapshotted
//! first — and each row asserts the stored extension reports ITS OWN description, a
//! freshly minted `SchemaExtensionId`, the transient status the clock owes a transition
//! from, and the snapshot the request asked for (or no snapshot when it did not). The
//! remaining suites assert the invariants: the owed transition settles exactly once,
//! pagination walks every extension once, an extension needs a Managed Microsoft AD
//! directory that is in service, a snapshot-first request is bound by the same manual
//! snapshot quota `CreateSnapshot` is, and an extension does not outlive its directory.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{create_directory, minimal_create, PAST_ONE_SPAN};
use crate::harness::{ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The `SchemaExtensionStatus` an extension carries until the transition the start owes
/// falls due, and the one it reaches after: two values of the modelled
/// `SchemaExtensionStatus` enum (`smithy_query(query="shape",shape="SchemaExtensionStatus")`
/// lists `Initializing`, `CreatingSnapshot`, `UpdatingSchema`, `Replicating`,
/// `CancelInProgress`, `RollbackInProgress`, `Cancelled`, `Failed`, `Completed`).
const STATUS_INITIALIZING: &str = "Initializing";
const STATUS_COMPLETED: &str = "Completed";

/// The maximum length of the modelled `Description`, from its `@length` trait
/// (`min: 0, max: 128`).
const DESCRIPTION_MAX: usize = 128;

/// Directory Service's default quota "Manual snapshots: 5" per directory, which a
/// snapshot-first schema extension counts against like any other manual snapshot.
const MANUAL_SNAPSHOT_QUOTA: usize = 5;

/// A well-formed `DirectoryId` (`^d-[0-9a-f]{10}$`) that is never created.
const ABSENT_DIRECTORY_ID: &str = "d-4b7c1e9a05";

/// The modelled `SchemaExtensionId` pattern, `^e-[0-9a-f]{10}$`, as an assertion.
fn assert_schema_extension_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("e-").unwrap_or_default();
    assert!(
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected a SchemaExtensionId matching ^e-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// A Managed Microsoft AD directory past the stage transition its create owes, so it is
/// in service for the extensions the cases apply.
async fn directory_in_service(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// A `StartSchemaExtension` request body. Every member the model marks required is
/// carried, `CreateSnapshotBeforeSchemaExtension` included: the request walk refuses a
/// null one before this handler runs.
fn start_body(directory_id: &str, description: &str, ldif: &str, snapshot_first: bool) -> Value {
    json!({
        "DirectoryId": directory_id,
        "Description": description,
        "LdifContent": ldif,
        "CreateSnapshotBeforeSchemaExtension": snapshot_first,
    })
}

/// `StartSchemaExtension` with `body`, asserting it was accepted and returning the id.
async fn start(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let answer = ok(router, "StartSchemaExtension", body, case).await;
    let id = string(&answer, "SchemaExtensionId", case);
    assert_schema_extension_id(&id, case);
    id
}

/// The `SchemaExtensionsInfo` list of a `ListSchemaExtensions` answer.
fn extensions(body: &Value, case: &str) -> Vec<Value> {
    body["SchemaExtensionsInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected SchemaExtensionsInfo, got {body}"))
        .clone()
}

/// `ListSchemaExtensions` of `directory_id`, returning its `SchemaExtensionsInfo` list.
async fn list(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Vec<Value> {
    let answer = ok(
        router,
        "ListSchemaExtensions",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    extensions(&answer, case)
}

/// The one reported extension carrying `schema_extension_id`.
fn reported<'a>(found: &'a [Value], schema_extension_id: &str, case: &str) -> &'a Value {
    found
        .iter()
        .find(|entry| entry["SchemaExtensionId"] == json!(schema_extension_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {schema_extension_id} among {found:?}")
        })
}

/// The `SchemaExtensionId`s of a reported list, in the order reported.
fn reported_ids(found: &[Value], case: &str) -> Vec<String> {
    found
        .iter()
        .map(|entry| string(entry, "SchemaExtensionId", case))
        .collect()
}

/// How many snapshots the store holds of `directory_id`, read back through
/// `DescribeSnapshots`.
async fn snapshot_count(router: &MultiServiceRouter, directory_id: &str, case: &str) -> usize {
    let answer = ok(
        router,
        "DescribeSnapshots",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["Snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Snapshots, got {answer}"))
        .len()
}

/// A numeric member of a reported extension, in epoch seconds — the unit an `awsJson1_1`
/// timestamp is rendered in.
fn epoch_member(entry: &Value, member: &str, case: &str) -> f64 {
    entry[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {entry}"))
}

/// One row of the start table: what the request carries and what the stored extension
/// must then report.
struct StartCase {
    case: &'static str,
    description: String,
    ldif: String,
    snapshot_first: bool,
}

#[tokio::test]
async fn a_started_schema_extension_is_reported_from_persisted_state() {
    let router = service();
    let directory = directory_in_service(&router, "corp.example.com", "the extended directory").await;

    // The rows differ along everything a start varies in: the form and length of the
    // `Description` — its pattern `^([a-zA-Z0-9_])[\\a-zA-Z0-9_@#%*+=:?./!\s-]*$` admits
    // punctuation and whitespace after the first character and its `@length` trait admits
    // up to 128 of them — the size and shape of the `LdifContent` (one modest change, a
    // multi-entry file, and a single character, the smallest its `@length` trait admits),
    // and whether the directory was to be snapshotted first.
    let cases = vec![
        StartCase {
            case: "a plain description, snapshot first",
            description: "Add_user_attributes".to_string(),
            ldif: "dn: CN=Shoe-Size,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n\
                   changetype: add\nobjectClass: attributeSchema\n"
                .to_string(),
            snapshot_first: true,
        },
        StartCase {
            case: "a punctuated description with whitespace, no snapshot",
            description: "pre-flight schema change #2 @ 03:00".to_string(),
            ldif: "dn: CN=Badge-Id,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n\
                   changetype: add\nobjectClass: attributeSchema\n"
                .to_string(),
            snapshot_first: false,
        },
        StartCase {
            case: "a description at the published length bound, snapshot first",
            description: "d".repeat(DESCRIPTION_MAX),
            ldif: "dn: CN=Bulk,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n"
                .repeat(40),
            snapshot_first: true,
        },
        StartCase {
            case: "a description and an LDIF file of one character each",
            description: "a".to_string(),
            ldif: "#".to_string(),
            snapshot_first: false,
        },
    ];

    let mut started: Vec<(&StartCase, String)> = Vec::new();
    for row in &cases {
        let case = row.case;
        let snapshots_before = snapshot_count(&router, &directory, case).await;
        let id = start(
            &router,
            start_body(&directory, &row.description, &row.ldif, row.snapshot_first),
            case,
        )
        .await;

        // "If true, creates a snapshot of the directory before applying the schema
        // extension" — and nothing else does.
        let snapshots_after = snapshot_count(&router, &directory, case).await;
        let expected_snapshots = snapshots_before + usize::from(row.snapshot_first);
        assert_eq!(
            snapshots_after, expected_snapshots,
            "case {case:?}: expected {expected_snapshots} snapshot(s) of the directory, got \
             {snapshots_after}"
        );

        // The start answers while the extension is still being applied, so a caller
        // acting straight afterwards observes the transient status and no end instant.
        let found = list(&router, &directory, case).await;
        let entry = reported(&found, &id, case);
        assert_eq!(
            entry["DirectoryId"],
            json!(directory),
            "case {case:?}: expected DirectoryId {directory}, got {entry}"
        );
        assert_eq!(
            entry["Description"],
            json!(row.description),
            "case {case:?}: expected Description {:?}, got {entry}",
            row.description
        );
        assert_eq!(
            entry["SchemaExtensionStatus"],
            json!(STATUS_INITIALIZING),
            "case {case:?}: expected {STATUS_INITIALIZING} straight after the start, got {entry}"
        );
        assert!(
            entry["EndDateTime"].is_null(),
            "case {case:?}: expected no EndDateTime on an extension still being applied, got \
             {entry}"
        );
        let started_at = epoch_member(entry, "StartDateTime", case);
        assert!(
            started_at > 1_600_000_000.0,
            "case {case:?}: expected StartDateTime to be a real epoch instant, got {started_at}"
        );
        started.push((row, id));
    }

    // Distinct starts mint distinct ids.
    let distinct: BTreeSet<&String> = started.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        started.len(),
        "expected {} distinct schema extension ids, got {:?}",
        started.len(),
        started.iter().map(|(_, id)| id).collect::<Vec<&String>>()
    );

    // Every extension is reported, in the order they were started.
    let found = list(&router, &directory, "creation order").await;
    assert_eq!(
        reported_ids(&found, "creation order"),
        started
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<String>>(),
        "expected every started extension once in start order, got {found:?}"
    );

    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Once the owed transition has fallen due every extension has settled, and its
    // description is still its own.
    let found = list(&router, &directory, "after the transition").await;
    for (row, id) in &started {
        let case = row.case;
        let entry = reported(&found, id, case);
        assert_eq!(
            entry["SchemaExtensionStatus"],
            json!(STATUS_COMPLETED),
            "case {case:?}: expected {STATUS_COMPLETED} once the owed transition fell due, got \
             {entry}"
        );
        assert_eq!(
            entry["Description"],
            json!(row.description),
            "case {case:?}: expected Description {:?} to survive the transition, got {entry}",
            row.description
        );
        let started_at = epoch_member(entry, "StartDateTime", case);
        let ended_at = epoch_member(entry, "EndDateTime", case);
        assert!(
            ended_at >= started_at,
            "case {case:?}: expected EndDateTime {ended_at} at or after StartDateTime \
             {started_at}"
        );
    }

    // A read does not change what a later caller sees: the same listing twice.
    let first = list(&router, &directory, "read twice").await;
    let second = list(&router, &directory, "read twice").await;
    assert_eq!(
        first, second,
        "expected ListSchemaExtensions to be a read: got {first:?} then {second:?}"
    );
}

#[tokio::test]
async fn pagination_walks_every_schema_extension_exactly_once() {
    let router = service();
    let directory = directory_in_service(&router, "corp.example.com", "pagination").await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..4 {
        created.push(
            start(
                &router,
                start_body(
                    &directory,
                    &format!("extension_{index}"),
                    "dn: CN=Page,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n",
                    false,
                ),
                "pagination",
            )
            .await,
        );
    }

    // Page sizes spanning 1, several, exactly the record count and past it, plus the
    // documented zero: `Limit`'s documentation says "The maximum number of items to
    // return", and a zero asks for the operation's own maximum.
    for limit in [0usize, 1, 2, created.len(), created.len() + 3] {
        let case = format!("Limit {limit}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "DirectoryId": &directory, "Limit": limit });
            if let Some(token) = next_token.as_ref() {
                body["NextToken"] = json!(token);
            }
            let answer = ok(&router, "ListSchemaExtensions", body, &case).await;
            let page = extensions(&answer, &case);
            if limit > 0 {
                assert!(
                    page.len() <= limit,
                    "case {case:?}: expected at most {limit} extensions per page, got {}",
                    page.len()
                );
            }
            walked.extend(reported_ids(&page, &case));
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match answer["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: expected every extension once in start order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_extension_needs_a_managed_microsoft_ad_directory_in_service() {
    let router = service();
    let in_service = directory_in_service(&router, "corp.example.com", "refusals").await;
    let ldif = "dn: CN=Refused,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n";

    // A well-formed id naming nothing stored is "the specified entity could not be
    // found", whether it is read or written through.
    for (case, operation, body) in [
        (
            "StartSchemaExtension with an absent DirectoryId",
            "StartSchemaExtension",
            start_body(ABSENT_DIRECTORY_ID, "absent", ldif, false),
        ),
        (
            "ListSchemaExtensions with an absent DirectoryId",
            "ListSchemaExtensions",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
        ),
    ] {
        refused(
            &router,
            operation,
            body,
            "EntityDoesNotExistException",
            case,
        )
        .await;
    }

    // "Applies a schema extension to a Microsoft AD directory": a Simple AD directory is
    // not one, so the id the request named is the invalid parameter.
    let simple_ad = create_directory(&router, minimal_create("simple.example.net"), "simple").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "StartSchemaExtension",
        start_body(&simple_ad, "wrong_type", ldif, false),
        "InvalidParameterException",
        "a Simple AD directory",
    )
    .await;

    // A directory still being built by the transition its create owes is unavailable.
    let still_building =
        create_microsoft_ad(&router, minimal_microsoft_ad("new.example.org"), "building").await;
    refused(
        &router,
        "StartSchemaExtension",
        start_body(&still_building, "too_early", ldif, false),
        "DirectoryUnavailableException",
        "a directory still being built",
    )
    .await;

    // Neither refusal persisted anything.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, directory) in [
        ("a Simple AD directory", &simple_ad),
        ("a directory still being built", &still_building),
    ] {
        let found = list(&router, directory, case).await;
        assert!(
            found.is_empty(),
            "case {case:?}: expected a refused start to persist nothing, got {found:?}"
        );
    }

    // A token this service did not issue for THIS listing is not a page of it: a token
    // nobody issued, an empty one, one issued by another paginated operation of the same
    // service, and one this listing issued for another directory.
    let other = directory_in_service(&router, "sales.example.org", "tokens").await;
    for directory in [&in_service, &other] {
        for index in 0..2 {
            start(
                &router,
                start_body(directory, &format!("token_row_{index}"), ldif, false),
                "tokens",
            )
            .await;
        }
    }
    let foreign_token = {
        let answer = ok(
            &router,
            "DescribeDirectories",
            json!({ "Limit": 1 }),
            "a token from another listing",
        )
        .await;
        string(&answer, "NextToken", "a token from another listing")
    };
    let other_directorys_token = {
        let answer = ok(
            &router,
            "ListSchemaExtensions",
            json!({ "DirectoryId": &other, "Limit": 1 }),
            "a token issued for another directory",
        )
        .await;
        string(&answer, "NextToken", "a token issued for another directory")
    };
    for (case, token) in [
        ("a token this service never issued", "not-a-token".to_string()),
        ("an empty token", String::new()),
        ("a token issued by another listing", foreign_token),
        (
            "a token issued for another directory",
            other_directorys_token,
        ),
    ] {
        refused(
            &router,
            "ListSchemaExtensions",
            json!({ "DirectoryId": &in_service, "NextToken": token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_snapshot_first_extension_is_bound_by_the_manual_snapshot_quota() {
    let router = service();
    let directory = directory_in_service(&router, "corp.example.com", "quota").await;
    let ldif = "dn: CN=Quota,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n";

    // Every snapshot up to the published bound is accepted, however it was asked for:
    // the first few through `CreateSnapshot`, the rest through a snapshot-first start.
    for index in 0..MANUAL_SNAPSHOT_QUOTA - 2 {
        ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": &directory, "Name": format!("snap_{index}") }),
            "up to the quota",
        )
        .await;
    }
    for index in 0..2 {
        start(
            &router,
            start_body(
                &directory,
                &format!("snapshot_first_{index}"),
                ldif,
                true,
            ),
            "up to the quota",
        )
        .await;
    }
    assert_eq!(
        snapshot_count(&router, &directory, "at the quota").await,
        MANUAL_SNAPSHOT_QUOTA,
        "expected the directory to hold exactly its manual snapshot quota"
    );

    // One past it the start is refused with the modelled quota error, and persists
    // neither a snapshot nor an extension.
    refused(
        &router,
        "StartSchemaExtension",
        start_body(&directory, "one_too_many", ldif, true),
        "SnapshotLimitExceededException",
        "one past the quota",
    )
    .await;
    assert_eq!(
        snapshot_count(&router, &directory, "one past the quota").await,
        MANUAL_SNAPSHOT_QUOTA,
        "expected the refused start to persist no snapshot"
    );
    let found = list(&router, &directory, "one past the quota").await;
    assert_eq!(
        found.len(),
        2,
        "expected only the two accepted extensions, got {found:?}"
    );

    // The quota binds only the snapshot, so a start that asks for none is unaffected.
    let id = start(
        &router,
        start_body(&directory, "no_snapshot_wanted", ldif, false),
        "no snapshot wanted",
    )
    .await;
    let found = list(&router, &directory, "no snapshot wanted").await;
    reported(&found, &id, "no snapshot wanted");
    assert_eq!(
        snapshot_count(&router, &directory, "no snapshot wanted").await,
        MANUAL_SNAPSHOT_QUOTA,
        "expected a start that asked for no snapshot to take none"
    );
}

#[tokio::test]
async fn a_schema_extension_does_not_outlive_its_directory() {
    let router = service();
    let first = directory_in_service(&router, "corp.example.com", "delete").await;
    let second = directory_in_service(&router, "sales.example.org", "delete").await;
    let ldif = "dn: CN=Doomed,CN=Schema,CN=Configuration,DC=corp,DC=example,DC=com\n";

    start(
        &router,
        start_body(&first, "doomed", ldif, false),
        "delete",
    )
    .await;
    let kept = start(
        &router,
        start_body(&second, "kept", ldif, false),
        "delete",
    )
    .await;

    crate::harness::delete_directory_and_wait(&router, &first, "delete").await;

    // An extension is an extension OF a directory, so it does not outlive it, and the
    // other directory's extension is untouched.
    refused(
        &router,
        "ListSchemaExtensions",
        json!({ "DirectoryId": &first }),
        "EntityDoesNotExistException",
        "the deleted directory",
    )
    .await;
    let found = list(&router, &second, "the surviving directory").await;
    assert_eq!(
        reported_ids(&found, "the surviving directory"),
        vec![kept],
        "expected only the surviving directory's extension, got {found:?}"
    );

    // A directory that has been through none reports an empty list rather than a
    // refusal.
    let fresh = directory_in_service(&router, "fresh.example.net", "fresh").await;
    let found = list(&router, &fresh, "fresh").await;
    assert!(
        found.is_empty(),
        "expected a fresh directory to report no extensions, got {found:?}"
    );
}
