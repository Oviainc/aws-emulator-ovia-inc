// SPDX-License-Identifier: CC0-1.0

//! What `DescribeDataset` answers, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule this suite pins: `DescribeDataset` REPORTS the stored dataset its
//! request names — every member it answers is the one the last accepted write
//! left, and the call writes nothing, so no number of reads settles the
//! transition the clock owes early, moves an instant, or takes the dataset to
//! another version; a request whose members fall outside the bounds the model
//! publishes is refused as client input, and one that is well formed but names a
//! dataset, workspace or version the store does not hold is the modelled
//! not-found refusal, each of them leaving the stored dataset exactly as it was.
//!
//! The rows differ along what that rule generalises over: which form of dataset
//! is read (each `datasetType` the model declares plus one naming none, inside a
//! workspace and outside every workspace), how many accepted writes the dataset
//! has behind it, which member of the request is out of bounds and at which end
//! of its published range, and whether the read is taken while the owed
//! transition is pending or after it fell due.
//!
//! The sibling `dataset_lifecycle` suite covers what the WRITES persist and
//! `dataset_read_workspace_resolution` which resource a doubly-wrong request
//! reports absent; this suite is about the read itself, so it reuses their HTTP
//! harness (`call`, `error_code`, `router`) and the shared `create_dataset` /
//! `ensure_workspace` helpers rather than growing a second copy.
//!
//! Every expected refusal string is written out BY HAND here, as an independent
//! oracle: this suite calls none of the crate's message helpers, so a handler
//! that drifts to a template of its own cannot agree with itself.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, error_code, router};
use crate::dataset_data_segments::create_dataset;
use crate::support::ensure_workspace;

/// Where every dataset request goes, as `OPERATIONS.md` reports the routes:
/// `POST /datasets`, `GET|PUT /datasets/{datasetId}`.
const DATASETS: &str = "/datasets";

/// The workspace the datasets of this suite that live in one are created in.
const WORKSPACE: &str = "reading-desk";

/// The `datasetVersion` a dataset carries when nothing has rewritten it since the
/// create.
const CREATED_VERSION: &str = "1";

/// An `ID` the modelled form admits (len 36..36, lower-case hex groups) that no
/// create of this suite minted.
///
/// Source: `smithy_query(query="operation", operation="DescribeDataset")` —
/// `datasetId` is `len 36..36`.
const UNMINTED_DATASET: &str = "bc1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";

/// How long after an accepted write the transition it owes has certainly fallen
/// due.
fn settle() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// `GET /datasets/{dataset}` with the optional query members a row names.
fn describe_uri(dataset: &str, workspace: Option<&str>, version: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(workspace) = workspace {
        query.push(format!("workspaceName={workspace}"));
    }
    if let Some(version) = version {
        query.push(format!("datasetVersion={version}"));
    }
    if query.is_empty() {
        format!("{DATASETS}/{dataset}")
    } else {
        format!("{DATASETS}/{dataset}?{}", query.join("&"))
    }
}

/// `DescribeDataset`'s body for `dataset`, which must answer 200.
async fn describe(router: &MultiServiceRouter, dataset: &str) -> Value {
    let (status, body) = call(router, "GET", &describe_uri(dataset, None, None), None).await;
    assert_eq!(status, 200, "DescribeDataset {dataset}: got {status} {body}");
    body
}

/// The `datasetStatus.state` a read reports.
fn state(body: &Value) -> &str {
    body["datasetStatus"]["state"].as_str().unwrap_or("")
}

/// `body` without its `datasetStatus`: the members a read reports that the clock
/// cannot move, so two reads taken at different instants are comparable.
fn without_status(body: &Value) -> Value {
    let mut stripped = body.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("datasetStatus");
    }
    stripped
}

/// One form of dataset every read rule below is run over: the `datasetType` the
/// create names, if any, and the workspace that type then requires or forbids.
///
/// The types are the whole `DatasetTypeEnum` value list (`smithy_query(query=
/// "shape", shape="DatasetTypeEnum")` — SESSION | CURATED | EXTERNAL), plus a
/// create naming no type at all, which the member's optionality admits.
/// `workspaceName` is present exactly where the model says it must be:
/// "Required for session and curated datasets. Omit this field for datasets that
/// connect to an external datasource."
#[derive(Debug, Clone, Copy)]
struct Form {
    label: &'static str,
    name: &'static str,
    dataset_type: Option<&'static str>,
    workspace: Option<&'static str>,
}

/// Every form of dataset this suite reads.
const FORMS: [Form; 4] = [
    Form {
        label: "a session dataset of a workspace",
        name: "Session read",
        dataset_type: Some("SESSION"),
        workspace: Some(WORKSPACE),
    },
    Form {
        label: "a curated dataset of a workspace",
        name: "Curated read",
        dataset_type: Some("CURATED"),
        workspace: Some(WORKSPACE),
    },
    Form {
        label: "an external dataset, in no workspace",
        name: "External read",
        dataset_type: Some("EXTERNAL"),
        workspace: None,
    },
    Form {
        label: "a dataset naming no type",
        name: "Untyped read",
        dataset_type: None,
        workspace: None,
    },
];

/// How many times a row polls the dataset inside one owed span. Kept small so the
/// reads land in the window the write opened rather than in the runner's
/// scheduling.
const POLLS: usize = 4;

#[tokio::test]
async fn polling_a_dataset_reports_the_same_dataset_and_settles_nothing_early() {
    let router = router();

    for form in FORMS {
        let id = create_dataset(&router, form.name, form.dataset_type, form.workspace).await;

        // Taken straight after the create, the read reports the transient state
        // the transition the create owes has not settled yet: "State is CREATING
        // after a successfull call to this API."
        let first = describe(&router, &id).await;
        assert_eq!(
            state(&first),
            "CREATING",
            "case {:?}: expected CREATING while the create settles, got {first}",
            form.label
        );

        for poll in 0..POLLS {
            let read = describe(&router, &id).await;
            assert_eq!(
                state(&read),
                "CREATING",
                "case {:?}: poll {poll} must not settle the owed transition, got {read}",
                form.label
            );
            assert_eq!(
                read, first,
                "case {:?}: poll {poll} must report the dataset the read before it did, got {read}",
                form.label
            );
        }

        tokio::time::sleep(settle()).await;
        let settled = describe(&router, &id).await;
        assert_eq!(
            state(&settled),
            "ACTIVE",
            "case {:?}: the owed transition must fall due by itself, got {settled}",
            form.label
        );
        assert_eq!(
            without_status(&settled),
            without_status(&first),
            "case {:?}: the state is the only member the clock may move, got {settled}",
            form.label
        );
        assert_eq!(
            settled["datasetVersion"].as_str().unwrap_or(""),
            CREATED_VERSION,
            "case {:?}: reading a dataset must not take it to another version, got {settled}",
            form.label
        );
        assert_eq!(
            settled["datasetCreationDate"], first["datasetCreationDate"],
            "case {:?}: datasetCreationDate must not move because the dataset was read, got \
             {settled}",
            form.label
        );
        assert_eq!(
            settled["datasetLastUpdateDate"], first["datasetLastUpdateDate"],
            "case {:?}: datasetLastUpdateDate must not move because the dataset was read, got \
             {settled}",
            form.label
        );

        // And a settled dataset stays settled, however often it is read.
        for poll in 0..POLLS {
            let read = describe(&router, &id).await;
            assert_eq!(
                read, settled,
                "case {:?}: settled poll {poll} must report the same dataset, got {read}",
                form.label
            );
        }
    }
}

/// The `UpdateDataset` body a row sends: the members a caller owns, with the
/// required `datasetName` and `datasetSource` always carried.
fn update_body(name: &str, description: &str) -> Value {
    json!({
        "datasetName": name,
        "datasetDescription": description,
        "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
    })
}

/// How many accepted writes this suite puts behind one dataset, so the version a
/// read resolves is checked over a COUNT of writes rather than after one.
const UPDATES: u32 = 3;

#[tokio::test]
async fn a_read_resolves_the_version_the_last_accepted_write_produced() {
    let router = router();
    ensure_workspace(&router, WORKSPACE).await;
    let id = create_dataset(&router, "Versioned read", Some("SESSION"), Some(WORKSPACE)).await;
    tokio::time::sleep(settle()).await;

    let created = describe(&router, &id).await;
    let creation_date = created["datasetCreationDate"].clone();

    for write in 1..=UPDATES {
        // Each write carries its own members, so what a read answers after it
        // can only have come from that write.
        let name = format!("Versioned read {write}");
        let description = format!("after write {write}");
        let (status, accepted) = call(
            &router,
            "PUT",
            &format!("{DATASETS}/{id}"),
            Some(update_body(&name, &description)),
        )
        .await;
        assert_eq!(
            status, 202,
            "write {write}: expected the update to be accepted, got {status} {accepted}"
        );
        tokio::time::sleep(settle()).await;

        // A create counts as the first accepted write, so the version after
        // `write` updates is `write + 1`.
        let expected_version = (write + 1).to_string();
        let read = describe(&router, &id).await;
        assert_eq!(
            read["datasetVersion"].as_str().unwrap_or(""),
            expected_version,
            "write {write}: expected datasetVersion {expected_version}, got {read}"
        );
        assert_eq!(
            read["datasetName"].as_str().unwrap_or(""),
            name,
            "write {write}: a read must report the name the last write left, got {read}"
        );
        assert_eq!(
            read["datasetDescription"].as_str().unwrap_or(""),
            description,
            "write {write}: a read must report the description the last write left, got {read}"
        );
        assert_eq!(
            read["datasetCreationDate"], creation_date,
            "write {write}: a write must not move datasetCreationDate, got {read}"
        );

        // Only the version the last write produced resolves; every version
        // before it names something this store no longer holds.
        let (status, current) =
            call(&router, "GET", &describe_uri(&id, None, Some(&expected_version)), None).await;
        assert_eq!(
            status, 200,
            "write {write}: the version the last write produced must resolve, got {status} \
             {current}"
        );
        for earlier in 1..=write {
            let stale = earlier.to_string();
            let (status, body) =
                call(&router, "GET", &describe_uri(&id, None, Some(&stale)), None).await;
            assert_eq!(
                status, 404,
                "write {write}: datasetVersion {stale} is no longer held, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "write {write}: datasetVersion {stale} must be the modelled refusal, got {body}"
            );
        }
    }
}

/// One refusal row: the request it sends and the answer it must get.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    uri: String,
    status: u16,
    code: &'static str,
    /// The whole message the refusal must carry, written out by hand.
    message: String,
}

/// A value of exactly `length` characters inside the hex alphabet every `ID`
/// member is spelled with, for a row that is wrong only in its LENGTH.
fn hex_run(length: usize) -> String {
    "a".repeat(length)
}

/// How the boundary validator words one broken length bound, as AWS words it.
fn length_refusal(value: &str, member: &str, relation: &str, bound: usize) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must have length {relation} {bound}"
    )
}

/// The same for one broken pattern.
fn pattern_refusal(value: &str, member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// The pattern `Version` publishes — what `datasetVersion` targets.
///
/// Source: `smithy_query(query="shape", shape="Version")` — len 1..10, pattern
/// `^(0|([1-9]{1}\d*))$`.
const VERSION_PATTERN: &str = r"^(0|([1-9]{1}\d*))$";

/// The refusal rows, over the request members `DescribeDataset` carries.
///
/// The bounds come from `smithy_query(query="operation",
/// operation="DescribeDataset")`: `datasetId` (`ID`, len 36..36),
/// `datasetVersion` (`Version`, len 1..10, pattern `^(0|([1-9]{1}\d*))$`) and
/// `workspaceName` (`WorkspaceName`, len 1..64). Each length bound is walked one
/// character PAST each published end, and the pattern both one value it refuses
/// and one carrying a character outside its alphabet.
fn refusals(held: &str) -> Vec<Refusal> {
    let short_id = hex_run(35);
    let long_id = hex_run(37);
    let long_workspace = "w".repeat(65);
    let long_version = "1".repeat(11);
    vec![
        Refusal {
            label: "a datasetId one character under its published length",
            uri: describe_uri(&short_id, None, None),
            status: 400,
            code: "InvalidRequestException",
            message: length_refusal(&short_id, "datasetId", "greater than or equal to", 36),
        },
        Refusal {
            label: "a datasetId one character over its published length",
            uri: describe_uri(&long_id, None, None),
            status: 400,
            code: "InvalidRequestException",
            message: length_refusal(&long_id, "datasetId", "less than or equal to", 36),
        },
        Refusal {
            label: "a datasetVersion one character over its published length",
            uri: describe_uri(held, None, Some(&long_version)),
            status: 400,
            code: "InvalidRequestException",
            message: length_refusal(&long_version, "datasetVersion", "less than or equal to", 10),
        },
        Refusal {
            label: "a datasetVersion carrying a leading zero, which its pattern refuses",
            uri: describe_uri(held, None, Some("01")),
            status: 400,
            code: "InvalidRequestException",
            message: pattern_refusal("01", "datasetVersion", VERSION_PATTERN),
        },
        Refusal {
            label: "a datasetVersion outside the digits its pattern admits",
            uri: describe_uri(held, None, Some("x")),
            status: 400,
            code: "InvalidRequestException",
            message: pattern_refusal("x", "datasetVersion", VERSION_PATTERN),
        },
        Refusal {
            label: "a workspaceName one character over its published length",
            uri: describe_uri(held, Some(&long_workspace), None),
            status: 400,
            code: "InvalidRequestException",
            message: length_refusal(
                &long_workspace,
                "workspaceName",
                "less than or equal to",
                64,
            ),
        },
        Refusal {
            label: "a well-formed datasetId no create minted",
            uri: describe_uri(UNMINTED_DATASET, None, None),
            status: 404,
            code: "ResourceNotFoundException",
            message: absent("Dataset", UNMINTED_DATASET),
        },
        Refusal {
            label: "the lowest datasetVersion the pattern admits, which no write produced",
            uri: describe_uri(held, None, Some("0")),
            status: 404,
            code: "ResourceNotFoundException",
            message: absent("Dataset", held),
        },
        Refusal {
            label: "a datasetVersion later than the one the last write produced",
            uri: describe_uri(held, None, Some("2")),
            status: 404,
            code: "ResourceNotFoundException",
            message: absent("Dataset", held),
        },
        Refusal {
            label: "a workspaceName inside its bounds that this service does not hold",
            uri: describe_uri(held, Some("no-such-desk"), None),
            status: 404,
            code: "ResourceNotFoundException",
            message: absent("Workspace", "no-such-desk"),
        },
    ]
}

#[tokio::test]
async fn a_request_outside_the_published_bounds_is_refused_and_writes_nothing() {
    let router = router();
    ensure_workspace(&router, WORKSPACE).await;
    let held = create_dataset(&router, "Refused read", Some("SESSION"), Some(WORKSPACE)).await;
    tokio::time::sleep(settle()).await;
    let before = describe(&router, &held).await;

    for row in refusals(&held) {
        let (status, body) = call(&router, "GET", &row.uri, None).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected {}, got {status} {body}",
            row.label, row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {:?}: expected {}, got {body}",
            row.label,
            row.code
        );
        assert_eq!(
            body["message"],
            json!(row.message),
            "case {:?}: expected the refusal {:?}, got {body}",
            row.label,
            row.message
        );
    }

    // Every row above is a READ that refused: none of them rewrote, moved or
    // removed the dataset it named.
    let after = describe(&router, &held).await;
    assert_eq!(
        after, before,
        "the refusals must leave the stored dataset exactly as it was: before {before}, after \
         {after}"
    );
    assert_eq!(
        after["datasetVersion"].as_str().unwrap_or(""),
        CREATED_VERSION,
        "a refused read must not take the dataset to another version, got {after}"
    );

    // A request naming the dataset's own workspace and the version its last
    // write produced is served, so the refusals above are about the values the
    // rows carry and not about the query members themselves.
    let (status, served) = call(
        &router,
        "GET",
        &describe_uri(&held, Some(WORKSPACE), Some(CREATED_VERSION)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the dataset's own workspace and version must resolve, got {status} {served}"
    );
    assert_eq!(
        served["datasetId"].as_str().unwrap_or(""),
        held,
        "the served read must report the dataset it was asked for, got {served}"
    );
}
