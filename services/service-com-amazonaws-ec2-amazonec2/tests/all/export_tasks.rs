// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeExportTasks`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `DescribeExportTasks` REPORTS THE STORE: every member a stored export task carries
//!    reaches the wire as stored, nested structures and tags included, and a call that
//!    narrows nothing reports every task the account holds;
//! 2. `ExportTaskIds` NARROWS the report, and an id the account holds no task for selects
//!    no row rather than failing the call — the recorded behaviour of the real service
//!    (`ec2.op-contract-describeExportTasks` names an export task id no account holds and
//!    AWS answers a result);
//! 3. the state the CLOCK has reached is what is reported: a cancelled task reads
//!    `cancelling` straight after `CancelExportTask` and `cancelled` once that move falls
//!    due, with no second call needed to make it happen;
//! 4. `Filters` select with EC2's AND/OR rule — "If you specify multiple filters, the
//!    filters are joined with an AND" and, within one filter, the values with an OR
//!    (`Filter`, `Filter$Values`) — over the names this operation answers, with EC2's
//!    `*`/`?` wildcards, and a name it does not answer is REFUSED rather than silently
//!    selecting nothing;
//! 5. the request-controlled factors of the match are BOUNDED: a request at a published
//!    bound is answered and one past it is refused with `InvalidParameterValue`;
//! 6. the report is a READ: two identical calls answer the same set and the store is
//!    unchanged by either.
//!
//! Each table row is a distinct input along what the rule generalises over — which state,
//! disk image format, container format and target environment a task carries, how many tags,
//! which ids narrow the report, which filter name and how many values, which bound and which
//! side of it — so no row is the specification.
//!
//! Export tasks are SEEDED into the store here, the way the sibling cancel suite
//! ([`super::fpga_image_tasks_and_volume_io`]) seeds them: `CreateInstanceExportTask`, the
//! operation that starts one, is still a scaffold (it carries the crate's
//! `SCAFFOLDED, NOT IMPLEMENTED` marker), so the store is the only writer available — and
//! seeding is what lets one row per published catalog value be a case. The one mutation that
//! IS implemented, `CancelExportTask`, is driven over the wire in rule 3, so the describe is
//! proven to report a change made through the API and not just a value written beside it.

use std::time::{Duration, Instant};

use ec2::ops::types::{
    CONTAINER_FORMAT_VALUES, DISK_IMAGE_FORMAT_VALUES, EXPORT_ENVIRONMENT_VALUES,
    EXPORT_TASK_STATE_VALUES, ExportTask, ExportToS3Task, InstanceExportDetails, Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXPORT_TASK_STATE_ACTIVE,
    EXPORT_TASK_STATE_CANCELLED, EXPORT_TASK_STATE_CANCELLING, EXPORT_TASK_STATE_COMPLETED,
    ExportTaskRecord, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, filters, indexed};
use super::volume_lifecycle::{REGION, p, past_the_transition};
use super::wire::{child, child_text, document, set_items};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The prefix of an INSTANCE export task id: "the ID returned by the
/// CreateInstanceExportTask and ExportImage operations"
/// (`CancelExportTaskRequest$ExportTaskId`), which for an instance export is `export-i-…`.
const EXPORT_TASK_ID_PREFIX: &str = "export-i";

/// The code EC2 answers for a request member whose value it will not accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeExportTasks", params).await
}

async fn cancel(router: &MultiServiceRouter, export_task_id: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "CancelExportTask",
        &[p("ExportTaskId", export_task_id)],
    )
    .await
}

/// The `ExportTaskId.N` form fields for a narrowing request.
fn ids(values: &[&str]) -> Params {
    indexed("ExportTaskId", values)
}

/// One export task a row names: what varies between the rows of a table.
///
/// `tags` are `(key, value)` pairs in the order the store holds them,
/// `container_format` is absent on a row that exports the disk image alone ("If absent,
/// only the disk image is exported" — `ExportToS3Task$ContainerFormat`), and
/// `status_message` is absent on a task that has nothing to say about its state ("The status
/// message related to the export task" — `ExportTask$StatusMessage`).
struct TaskSpec<'a> {
    description: &'a str,
    state: &'a str,
    status_message: Option<&'a str>,
    s3_bucket: &'a str,
    s3_key: &'a str,
    container_format: Option<&'a str>,
    disk_image_format: &'a str,
    instance_id: &'a str,
    target_environment: &'a str,
    tags: &'a [(&'a str, &'a str)],
}

/// `spec` as the modelled shape the describe must report.
fn export_task(spec: TaskSpec<'_>) -> ExportTask {
    let TaskSpec {
        description,
        state,
        status_message,
        s3_bucket,
        s3_key,
        container_format,
        disk_image_format,
        instance_id,
        target_environment,
        tags,
    } = spec;
    ExportTask {
        export_task_id: None,
        description: Some(description.to_string()),
        state: Some(state.to_string()),
        status_message: status_message.map(str::to_string),
        export_to_s3_task: Some(ExportToS3Task {
            s3_bucket: Some(s3_bucket.to_string()),
            s3_key: Some(s3_key.to_string()),
            container_format: container_format.map(str::to_string),
            disk_image_format: Some(disk_image_format.to_string()),
        }),
        instance_export_details: Some(InstanceExportDetails {
            instance_id: Some(instance_id.to_string()),
            target_environment: Some(target_environment.to_string()),
        }),
        tags: Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
    }
}

/// Store `view` under a freshly minted export task id, and answer that id.
fn seed(engine: &engine::InMemoryEngine, view: &ExportTask) -> String {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let export_task_id = store.next_id(EXPORT_TASK_ID_PREFIX);
    let mut view = view.clone();
    view.export_task_id = Some(export_task_id.clone());
    store.export_tasks.insert(
        export_task_id.clone(),
        ExportTaskRecord {
            view,
            cancelled_at: None,
        },
    );
    export_task_id
}

/// How many export tasks the store holds.
fn stored_tasks(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().export_tasks.len()
}

/// One export task as a response reported it: every member flattened to the text on the
/// wire, so what the describe wrote can be compared against what the store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    export_task_id: String,
    description: String,
    state: String,
    status_message: String,
    s3_bucket: String,
    s3_key: String,
    container_format: String,
    disk_image_format: String,
    instance_id: String,
    target_environment: String,
    tags: Vec<(String, String)>,
}

/// A stored export task projected the way [`reported`] reads one off the wire, so the two
/// can be compared member by member. An absent member is the empty string on both sides.
fn expected(task: &ExportTask) -> Reported {
    let s3 = task.export_to_s3_task.clone().unwrap_or_default();
    let instance = task.instance_export_details.clone().unwrap_or_default();
    let text = |value: Option<String>| value.unwrap_or_default();
    Reported {
        export_task_id: text(task.export_task_id.clone()),
        description: text(task.description.clone()),
        state: text(task.state.clone()),
        status_message: text(task.status_message.clone()),
        s3_bucket: text(s3.s3_bucket),
        s3_key: text(s3.s3_key),
        container_format: text(s3.container_format),
        disk_image_format: text(s3.disk_image_format),
        instance_id: text(instance.instance_id),
        target_environment: text(instance.target_environment),
        tags: task
            .tags
            .iter()
            .flatten()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// Every export task a response carries, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "exportTaskId").is_some())
        .map(|item| {
            let nested = |set: &str, name: &str| {
                child(item, set)
                    .and_then(|inner| child_text(inner, name))
                    .unwrap_or_default()
            };
            Reported {
                export_task_id: child_text(item, "exportTaskId").unwrap_or_default(),
                description: child_text(item, "description").unwrap_or_default(),
                state: child_text(item, "state").unwrap_or_default(),
                status_message: child_text(item, "statusMessage").unwrap_or_default(),
                s3_bucket: nested("exportToS3", "s3Bucket"),
                s3_key: nested("exportToS3", "s3Key"),
                container_format: nested("exportToS3", "containerFormat"),
                disk_image_format: nested("exportToS3", "diskImageFormat"),
                instance_id: nested("instanceExport", "instanceId"),
                target_environment: nested("instanceExport", "targetEnvironment"),
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Every `exportTaskId` a response reported, sorted, so a selection can be compared
/// whatever order the report took.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let mut found: Vec<String> = reported(case, body)
        .into_iter()
        .map(|task| task.export_task_id)
        .collect();
    found.sort();
    found
}

/// The ids `params` selects, out of a call that must succeed.
async fn selected(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_ids(case, &body)
}

/// The table rule 1, 2, 4 and 6 work from: one export task per row, each row a distinct
/// combination of what an export task can carry.
///
/// Between them the rows cover every value of the published `ExportTaskState`,
/// `DiskImageFormat`, `ContainerFormat` and `ExportEnvironment` catalogs
/// (`ec2::ops::types::EXPORT_TASK_STATE_VALUES` and the three beside it, generated from the
/// model), which [`the_rows_cover_every_published_catalog_value`] pins.
fn rows() -> Vec<(&'static str, ExportTask)> {
    vec![
        (
            "an active export of a VMDK into an OVA for VMware, with two tags",
            export_task(TaskSpec {
                description: "nightly VM export",
                state: EXPORT_TASK_STATE_ACTIVE,
                status_message: None,
                s3_bucket: "my-vm-exports",
                s3_key: "vms/instance-a.ova",
                container_format: Some("ova"),
                disk_image_format: "VMDK",
                instance_id: "i-0123456789abcdef0",
                target_environment: "vmware",
                tags: &[("Name", "instance-a"), ("team", "platform")],
            }),
        ),
        (
            "a completed raw export for Citrix, with no container format and one tag",
            export_task(TaskSpec {
                description: "disaster recovery copy",
                state: EXPORT_TASK_STATE_COMPLETED,
                status_message: Some("the export finished successfully"),
                s3_bucket: "dr-exports",
                s3_key: "raw/instance-b.raw",
                container_format: None,
                disk_image_format: "RAW",
                instance_id: "i-00000000000000001",
                target_environment: "citrix",
                tags: &[("Name", "instance-b")],
            }),
        ),
        (
            "a cancelling VHD export for Microsoft, untagged",
            export_task(TaskSpec {
                description: "move to Hyper-V",
                state: EXPORT_TASK_STATE_CANCELLING,
                status_message: Some("cancellation requested by the owner"),
                s3_bucket: "hyperv-exports",
                s3_key: "vhd/instance-c.vhd",
                container_format: None,
                disk_image_format: "VHD",
                instance_id: "i-00000000000000002",
                target_environment: "microsoft",
                tags: &[],
            }),
        ),
        (
            "a cancelled export whose description carries spaces and punctuation",
            export_task(TaskSpec {
                description: "export of the 'legacy' app server (2026)",
                state: EXPORT_TASK_STATE_CANCELLED,
                status_message: None,
                s3_bucket: "my-vm-exports",
                s3_key: "vms/instance-d.ova",
                container_format: Some("ova"),
                disk_image_format: "VMDK",
                instance_id: "i-00000000000000003",
                target_environment: "vmware",
                tags: &[("owner", "ops")],
            }),
        ),
    ]
}

/// Seed [`rows`] and answer `(case, the stored task, its id)` per row.
fn seed_rows(engine: &engine::InMemoryEngine) -> Vec<(&'static str, ExportTask, String)> {
    rows()
        .into_iter()
        .map(|(case, view)| {
            let export_task_id = seed(engine, &view);
            let mut stored = view;
            stored.export_task_id = Some(export_task_id.clone());
            (case, stored, export_task_id)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The table is tied to the model's own catalogs
// ---------------------------------------------------------------------------

/// Every value of the four published catalogs an export task carries appears in [`rows`],
/// so a value the model adds cannot leave the table narrower than the catalog unnoticed.
#[test]
fn the_rows_cover_every_published_catalog_value() {
    let seeded = rows();
    let covered = |pick: &dyn Fn(&ExportTask) -> Option<String>| -> Vec<String> {
        seeded
            .iter()
            .filter_map(|(_, task)| pick(task))
            .collect::<Vec<String>>()
    };
    let catalogs: [(&str, &[&str], Vec<String>); 4] = [
        (
            "ExportTaskState",
            EXPORT_TASK_STATE_VALUES,
            covered(&|task| task.state.clone()),
        ),
        (
            "DiskImageFormat",
            DISK_IMAGE_FORMAT_VALUES,
            covered(&|task| {
                task.export_to_s3_task
                    .as_ref()
                    .and_then(|s3| s3.disk_image_format.clone())
            }),
        ),
        (
            "ContainerFormat",
            CONTAINER_FORMAT_VALUES,
            covered(&|task| {
                task.export_to_s3_task
                    .as_ref()
                    .and_then(|s3| s3.container_format.clone())
            }),
        ),
        (
            "ExportEnvironment",
            EXPORT_ENVIRONMENT_VALUES,
            covered(&|task| {
                task.instance_export_details
                    .as_ref()
                    .and_then(|details| details.target_environment.clone())
            }),
        ),
    ];
    for (case, catalog, values) in catalogs {
        for published in catalog {
            assert!(
                values.iter().any(|value| value == published),
                "case {case:?}: the table covers no task with the published value \
                 {published:?}, only {values:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 1: the report is the store
// ---------------------------------------------------------------------------

/// A call that narrows nothing reports every stored export task, each member as stored.
#[tokio::test]
async fn an_unnarrowed_call_reports_every_stored_task_as_stored() {
    let (router, engine) = emulator();

    // An account that holds no export task is reported as holding none, rather than
    // refused: "Describes the specified export instance tasks or all of your export
    // instance tasks".
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an empty store: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        reported("an empty store", &body).is_empty(),
        "an empty store: expected no export task, got {body}"
    );

    let seeded = seed_rows(&engine);
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the seeded store: expected HTTP 200, got {status} with body {body}"
    );
    let found = reported("the seeded store", &body);
    assert_eq!(
        found.len(),
        seeded.len(),
        "the seeded store: expected {} tasks, got {found:?}",
        seeded.len()
    );
    for (case, stored, export_task_id) in &seeded {
        let wanted = expected(stored);
        let got = found
            .iter()
            .find(|task| &task.export_task_id == export_task_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {export_task_id} is not in the report {found:?}")
            });
        assert_eq!(
            got, &wanted,
            "case {case:?}: expected the task as stored {wanted:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: ExportTaskIds narrows, and an unknown id selects nothing
// ---------------------------------------------------------------------------

/// `ExportTaskIds` narrows the report to the ids named, and an id the account holds no task
/// for selects no row instead of failing the call — whatever shape that id has.
#[tokio::test]
async fn export_task_ids_narrow_the_report_and_an_unheld_id_selects_nothing() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let held: Vec<String> = seeded
        .iter()
        .map(|(_, _, export_task_id)| export_task_id.clone())
        .collect();

    // `(case, the ids the request names, the ids it must select)`.
    let mut id_rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "one id the account holds".to_string(),
            ids(&[held[0].as_str()]),
            vec![held[0].clone()],
        ),
        (
            "two ids the account holds".to_string(),
            ids(&[held[1].as_str(), held[3].as_str()]),
            {
                let mut wanted = vec![held[1].clone(), held[3].clone()];
                wanted.sort();
                wanted
            },
        ),
        (
            "the same id twice".to_string(),
            ids(&[held[2].as_str(), held[2].as_str()]),
            vec![held[2].clone()],
        ),
        (
            "every id the account holds".to_string(),
            ids(&held.iter().map(String::as_str).collect::<Vec<&str>>()),
            {
                let mut wanted = held.clone();
                wanted.sort();
                wanted
            },
        ),
        (
            "no id at all, which narrows nothing".to_string(),
            Vec::new(),
            {
                let mut wanted = held.clone();
                wanted.sort();
                wanted
            },
        ),
    ];
    // An id of each shape the account holds nothing for: the long instance-export form, an
    // image-export id, the short form, a value of no particular shape, and another
    // resource's id. None of them is an error, and none of them selects a row.
    for unheld in [
        "export-i-0123456789abcdef0",
        "export-ami-0123456789abcdef0",
        "export-i-01234567",
        "not-an-export-task",
        "vol-0123456789abcdef0",
    ] {
        id_rows.push((
            format!("the unheld id {unheld:?} on its own"),
            ids(&[unheld]),
            Vec::new(),
        ));
        id_rows.push((
            format!("the unheld id {unheld:?} beside one the account holds"),
            ids(&[unheld, held[0].as_str()]),
            vec![held[0].clone()],
        ));
    }

    for (case, params, wanted) in id_rows {
        let got = selected(&router, &case, &params).await;
        assert_eq!(
            got, wanted,
            "case {case:?}: expected the tasks {wanted:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the report follows the clock
// ---------------------------------------------------------------------------

/// A cancel started over the wire is reported as `cancelling` straight away and as
/// `cancelled` once the clock owes that move — with no call in between to make it happen.
#[tokio::test]
async fn the_report_follows_a_cancel_through_the_clock() {
    let (router, engine) = emulator();
    let view = export_task(TaskSpec {
        description: "an instance export to cancel",
        state: EXPORT_TASK_STATE_ACTIVE,
        status_message: None,
        s3_bucket: "my-vm-exports",
        s3_key: "vms/instance-e.ova",
        container_format: Some("ova"),
        disk_image_format: "VMDK",
        instance_id: "i-00000000000000004",
        target_environment: "vmware",
        tags: &[],
    });
    let export_task_id = seed(&engine, &view);

    let state_now = |case: &'static str, body: &str| -> String {
        let found = reported(case, body);
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly one task, got {found:?}"
        );
        found[0].state.clone()
    };

    let (status, body) = describe(&router, &ids(&[export_task_id.as_str()])).await;
    assert_eq!(
        status, 200,
        "before the cancel: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        state_now("before the cancel", &body),
        EXPORT_TASK_STATE_ACTIVE,
        "before the cancel the task is {EXPORT_TASK_STATE_ACTIVE}, body {body}"
    );

    let (status, body) = cancel(&router, &export_task_id).await;
    assert_eq!(
        status, 200,
        "cancelling the task: expected HTTP 200, got {status} with body {body}"
    );

    let (_, body) = describe(&router, &ids(&[export_task_id.as_str()])).await;
    assert_eq!(
        state_now("straight after the cancel", &body),
        EXPORT_TASK_STATE_CANCELLING,
        "straight after the cancel the task is {EXPORT_TASK_STATE_CANCELLING}, body {body}"
    );
    // The transition is owed by the clock, so the `cancelling` task is selected by the
    // `state` filter the caller would use to find one.
    let found = selected(
        &router,
        "the cancelling task by state",
        &filters(&[("state", &[EXPORT_TASK_STATE_CANCELLING])]),
    )
    .await;
    assert_eq!(
        found,
        vec![export_task_id.clone()],
        "the state filter selects the cancelling task, got {found:?}"
    );

    past_the_transition().await;

    let (_, body) = describe(&router, &ids(&[export_task_id.as_str()])).await;
    assert_eq!(
        state_now("once the cancellation falls due", &body),
        EXPORT_TASK_STATE_CANCELLED,
        "once the cancellation falls due the task is {EXPORT_TASK_STATE_CANCELLED}, body {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: filtering
// ---------------------------------------------------------------------------

/// Each filter name this operation answers selects on the member it names, several values of
/// one filter are joined with an OR, several filters with an AND, and EC2's `*`/`?`
/// wildcards match within a value.
#[tokio::test]
async fn filters_select_with_ec2s_and_or_rule_over_the_names_this_operation_answers() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let id_of = |index: usize| seeded[index].2.clone();
    let sorted = |mut values: Vec<String>| {
        values.sort();
        values
    };

    // `(case, the filter parameters, the ids they must select)`.
    let filter_rows: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "export-task-id names one task",
            filters(&[("export-task-id", &[seeded[1].2.as_str()])]),
            vec![id_of(1)],
        ),
        (
            "export-task-id with two values is an OR",
            filters(&[(
                "export-task-id",
                &[seeded[0].2.as_str(), seeded[2].2.as_str()],
            )]),
            sorted(vec![id_of(0), id_of(2)]),
        ),
        (
            "export-task-id naming no stored task selects nothing",
            filters(&[("export-task-id", &["export-i-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "state selects the task in that state",
            filters(&[("state", &[EXPORT_TASK_STATE_COMPLETED])]),
            vec![id_of(1)],
        ),
        (
            "state with two values is an OR",
            filters(&[(
                "state",
                &[EXPORT_TASK_STATE_ACTIVE, EXPORT_TASK_STATE_CANCELLED],
            )]),
            sorted(vec![id_of(0), id_of(3)]),
        ),
        (
            "description matches the whole value, case-sensitively",
            filters(&[("description", &["nightly VM export"])]),
            vec![id_of(0)],
        ),
        (
            "description in the wrong case selects nothing",
            filters(&[("description", &["NIGHTLY VM EXPORT"])]),
            Vec::new(),
        ),
        (
            "a * wildcard matches within a description",
            filters(&[("description", &["*export*"])]),
            sorted(vec![id_of(0), id_of(3)]),
        ),
        (
            "a ? wildcard matches one character of a description",
            filters(&[("description", &["?ightly VM export"])]),
            vec![id_of(0)],
        ),
        (
            "status-message selects the task carrying exactly that message",
            filters(&[("status-message", &["the export finished successfully"])]),
            vec![id_of(1)],
        ),
        (
            "a * wildcard on status-message selects every task that carries one, and no \
             task that carries none",
            filters(&[("status-message", &["*"])]),
            sorted(vec![id_of(1), id_of(2)]),
        ),
        (
            "status-message selects nothing when no task carries that message",
            filters(&[("status-message", &["*disk full*"])]),
            Vec::new(),
        ),
        (
            "tag-key selects every task carrying that key",
            filters(&[("tag-key", &["Name"])]),
            sorted(vec![id_of(0), id_of(1)]),
        ),
        (
            "tag-key with two values is an OR",
            filters(&[("tag-key", &["team", "owner"])]),
            sorted(vec![id_of(0), id_of(3)]),
        ),
        (
            "tag:<key> selects on that key's value",
            filters(&[("tag:Name", &["instance-b"])]),
            vec![id_of(1)],
        ),
        (
            "tag:<key> with a wildcard value",
            filters(&[("tag:Name", &["instance-*"])]),
            sorted(vec![id_of(0), id_of(1)]),
        ),
        (
            "tag:<key> for a key the task does not carry selects nothing",
            filters(&[("tag:Name", &["instance-c"])]),
            Vec::new(),
        ),
        (
            "two filters are joined with an AND",
            filters(&[
                ("state", &[EXPORT_TASK_STATE_ACTIVE]),
                ("tag-key", &["team"]),
            ]),
            vec![id_of(0)],
        ),
        (
            "an AND no task satisfies selects nothing",
            filters(&[
                ("state", &[EXPORT_TASK_STATE_COMPLETED]),
                ("tag-key", &["team"]),
            ]),
            Vec::new(),
        ),
        (
            "a filter beside ExportTaskIds narrows further",
            {
                let mut params = ids(&[seeded[0].2.as_str(), seeded[1].2.as_str()]);
                params.extend(filters(&[("state", &[EXPORT_TASK_STATE_COMPLETED])]));
                params
            },
            vec![id_of(1)],
        ),
    ];

    for (case, params, wanted) in filter_rows {
        let got = selected(&router, case, &params).await;
        assert_eq!(
            got, wanted,
            "case {case:?}: expected the tasks {wanted:?}, got {got:?}"
        );
    }
}

/// A filter name this operation does not answer is REFUSED, so a misspelling can never read
/// as "no export tasks".
#[tokio::test]
async fn a_filter_name_this_operation_does_not_answer_is_refused() {
    let (router, engine) = emulator();
    seed_rows(&engine);

    // `(case, the filter name)`: a name of no resemblance, a documented name in the wrong
    // case (EC2 filter names are case-sensitive), a plural misspelling, a `tag:` prefix with
    // no key to select on, and the empty name.
    let name_rows: [(&str, &str); 6] = [
        ("a name of no resemblance", "sim2real-not-a-real-filter"),
        ("a documented name in the wrong case", "State"),
        ("a documented name misspelled", "export-task-ids"),
        ("the tag prefix with no key", "tag:"),
        ("the tag prefix misspelled", "tags:Name"),
        ("the empty name", ""),
    ];
    for (case, name) in name_rows {
        let (status, body) = describe(&router, &filters(&[(name, &["any"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: every request-controlled factor is bounded
// ---------------------------------------------------------------------------

/// A request AT each published bound is answered, and one PAST it is refused with
/// `InvalidParameterValue` — the ids it names, the filters it carries, the values of one
/// filter, and the size of one value.
#[tokio::test]
async fn each_published_bound_is_answered_and_one_past_it_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);
    let held = seeded[0].2.clone();

    let id_list = |count: usize| -> Params {
        (0..count)
            .map(|index| {
                (
                    format!("ExportTaskId.{}", index + 1),
                    format!("export-i-{index:017x}"),
                )
            })
            .collect()
    };
    // Each filter carries a DISTINCT name this operation answers, so the row is about the
    // number of filters rather than about any one name: `tag:<key>` is the one family with
    // as many names as there are keys.
    let filter_list = |count: usize| -> Params {
        (0..count)
            .flat_map(|index| {
                vec![
                    (format!("Filter.{}.Name", index + 1), format!("tag:k{index}")),
                    (format!("Filter.{}.Value.1", index + 1), "*".to_string()),
                ]
            })
            .collect()
    };
    let value_list = |count: usize| -> Params {
        let mut params = vec![("Filter.1.Name".to_string(), "state".to_string())];
        params.extend((0..count).map(|index| {
            (
                format!("Filter.1.Value.{}", index + 1),
                format!("state-{index}"),
            )
        }));
        params
    };
    let value_of_bytes = |bytes: usize| -> Params {
        filters(&[("description", &["x".repeat(bytes).as_str()])])
    };

    // `(case, the parameters, whether the request is answered)`.
    let bound_rows: Vec<(String, Params, bool)> = vec![
        (
            format!("{MAX_RESOURCE_IDS} ids, the bound itself"),
            id_list(MAX_RESOURCE_IDS),
            true,
        ),
        (
            format!("{} ids, one past the bound", MAX_RESOURCE_IDS + 1),
            id_list(MAX_RESOURCE_IDS + 1),
            false,
        ),
        (
            format!("{MAX_FILTERS} filters, the bound itself"),
            filter_list(MAX_FILTERS),
            true,
        ),
        (
            format!("{} filters, one past the bound", MAX_FILTERS + 1),
            filter_list(MAX_FILTERS + 1),
            false,
        ),
        (
            format!("{MAX_FILTER_VALUES} values of one filter, the bound itself"),
            value_list(MAX_FILTER_VALUES),
            true,
        ),
        (
            format!(
                "{} values of one filter, one past the bound",
                MAX_FILTER_VALUES + 1
            ),
            value_list(MAX_FILTER_VALUES + 1),
            false,
        ),
        (
            format!("a {MAX_FILTER_VALUE_BYTES}-byte filter value, the bound itself"),
            value_of_bytes(MAX_FILTER_VALUE_BYTES),
            true,
        ),
        (
            format!(
                "a {}-byte filter value, one past the bound",
                MAX_FILTER_VALUE_BYTES + 1
            ),
            value_of_bytes(MAX_FILTER_VALUE_BYTES + 1),
            false,
        ),
    ];

    for (case, params, answered) in bound_rows {
        let (status, body) = describe(&router, &params).await;
        if answered {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
        }
    }

    // Nothing any of those requests did removed a task or added one.
    assert_eq!(
        stored_tasks(&engine),
        seeded.len(),
        "the bound rows left the store holding {} tasks",
        stored_tasks(&engine)
    );
    let got = selected(&router, "after the bound rows", &ids(&[held.as_str()])).await;
    assert_eq!(
        got,
        vec![held.clone()],
        "after the bound rows the held task is still reported, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 6: the report is a read
// ---------------------------------------------------------------------------

/// Describing export tasks changes nothing a later caller sees: two identical calls report
/// the same set, the store holds the same tasks, and a pending cancellation is not advanced
/// by having been read.
#[tokio::test]
async fn describing_export_tasks_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let seeded = seed_rows(&engine);

    // A task with a cancellation that is NOT yet due, so a read that advanced the clock
    // would be visible as a state change.
    let pending = seed(
        &engine,
        &export_task(TaskSpec {
            description: "a cancellation that is not due yet",
            state: EXPORT_TASK_STATE_ACTIVE,
            status_message: None,
            s3_bucket: "my-vm-exports",
            s3_key: "vms/instance-f.ova",
            container_format: Some("ova"),
            disk_image_format: "VMDK",
            instance_id: "i-00000000000000005",
            target_environment: "vmware",
            tags: &[],
        }),
    );
    {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        store
            .export_tasks
            .get_mut(&pending)
            .expect("the task was just seeded")
            .cancelled_at = Some(Instant::now() + Duration::from_secs(3_600));
    }

    let (_, first) = describe(&router, &[]).await;
    let before = reported("the first call", &first);
    for _ in 0..3 {
        let (status, body) = describe(&router, &[]).await;
        assert_eq!(
            status, 200,
            "a repeated call: expected HTTP 200, got {status} with body {body}"
        );
        let again = reported("a repeated call", &body);
        assert_eq!(
            again, before,
            "a repeated call must report the same set: expected {before:?}, got {again:?}"
        );
    }
    assert_eq!(
        stored_tasks(&engine),
        seeded.len() + 1,
        "the reads left the store holding {} tasks",
        stored_tasks(&engine)
    );
    let still_pending = engine
        .state::<Ec2State>()
        .lock()
        .export_tasks
        .get(&pending)
        .expect("the task is still stored")
        .state_at(Instant::now());
    assert_eq!(
        still_pending, EXPORT_TASK_STATE_CANCELLING,
        "the pending cancellation is still {EXPORT_TASK_STATE_CANCELLING} after the reads, \
         got {still_pending:?}"
    );
}
