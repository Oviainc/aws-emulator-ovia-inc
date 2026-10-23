// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VM import slice: `ImportImage` and
//! `DescribeImportImageTasks`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `ImportImage` STARTS A TASK FROM THE REQUEST and `DescribeImportImageTasks` REPORTS
//!    THE STORE: every member the request sets round-trips through the describe, and the
//!    members the service owns (the task id, the state, the progress, the snapshot ids and
//!    the AMI) are derived rather than taken;
//! 2. a task is `active` and the CLOCK, not a caller, completes it — with the AMI the
//!    import created, which is then an image of the account like any other;
//! 3. a value outside what the model documents for a disk container, an architecture, a
//!    platform, a license type or a boot mode is refused with EC2's own error code, and
//!    nothing is stored;
//! 4. one `ClientToken` names one import task;
//! 5. the report is narrowed by `ImportTaskIds` and by the documented `task-state` filter
//!    over its published value catalog, and cut into `MaxResults`-sized pages.
//!
//! Each table row is a distinct input along what the rule generalises over — which disk
//! image format, which source (an S3 URL or a bucket and key), how many disks, which
//! platform, architecture and boot mode, which task state, which page size — so no row is
//! the specification.

use std::time::{Duration, Instant};

use ec2::ops::types::{ImportImageTask, SnapshotDetail, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FilterPattern, IMPORT_DISK_IMAGE_FORMATS,
    IMPORT_IMAGE_LICENSE_TYPE_AWS, IMPORT_IMAGE_LICENSE_TYPE_BYOL, IMPORT_IMAGE_TASK_ID_PREFIX,
    IMPORT_TASK_STATE_ACTIVE, IMPORT_TASK_STATE_COMPLETED, IMPORT_TASK_STATE_DELETED,
    IMPORT_TASK_STATE_DELETING, IMPORT_TASK_STATES, IMPORT_TASK_STATUS_MESSAGE_CONVERTING,
    ImportImageTaskRecord, MAX_ACTIVE_IMPORT_PROGRESS, MAX_FILTER_PATTERN_TOKENS,
    MAX_IMPORT_DISK_CONTAINERS, MAX_IMPORT_LICENSE_SPECIFICATIONS, MAX_PAGE_SIZE, MatchBudget,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, indexed};
use super::wire::{child, child_text, document, is_long_id};

/// The Region every call here is signed for. An import task carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn import(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ImportImage", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeImportImageTasks", params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The sets nested inside an import image task: a scalar member of the task itself is
/// never read from inside one of these.
const NESTED_SETS: [&str; 3] = ["snapshotDetailSet", "tagSet", "licenseSpecifications"];

/// The `<item>` elements of the `importImageTaskSet` a response carries, as whole XML
/// fragments are useless for assertions: each item is read through [`task_member`].
fn task_count(case: &str, body: &str) -> usize {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "importTaskId").is_some())
        .count()
}

/// Every `importTaskId` a response reports, in the order it reported them.
fn task_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "importTaskId")
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// A scalar member of the FIRST import image task a response carries (`ImportImage`'s own
/// response is one such task), or `None` when the task does not carry it.
fn task_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let task = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no import task (no <importTaskId>) in {body}"));
    task.children()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == name
                && !NESTED_SETS.contains(&node.tag_name().name())
        })
        .map(|node| node.text().unwrap_or_default().to_string())
}

/// A scalar member of the FIRST import task's `index`th snapshot detail, read through the
/// `snapshotDetailSet` so a detail's `status` is never confused with the task's.
fn snapshot_member(case: &str, body: &str, index: usize, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let task = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no import task (no <importTaskId>) in {body}"));
    let set = child(task, "snapshotDetailSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <snapshotDetailSet> in {body}"));
    let item = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .nth(index)
        .unwrap_or_else(|| panic!("case {case:?}: no snapshot detail {index} in {body}"))
        ;
    child_text(item, name).or_else(|| {
        child(item, "userBucket").and_then(|bucket| child_text(bucket, name))
    })
}

/// How many disks the first import task of a response reports.
fn snapshot_count(case: &str, body: &str) -> usize {
    let parsed = document(case, body);
    let Some(task) = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
    else {
        return 0;
    };
    child(task, "snapshotDetailSet").map_or(0, |set| {
        set.children()
            .filter(|node| node.is_element() && node.tag_name().name() == "item")
            .count()
    })
}

/// The tags of the first import task of a response.
fn task_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let Some(task) = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
    else {
        return Vec::new();
    };
    let Some(set) = child(task, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// One disk container that names its image by an S3 URL in `format`.
fn url_container(index: usize, format: &str, url: &str) -> Params {
    vec![
        p(&format!("DiskContainer.{index}.Format"), format),
        p(&format!("DiskContainer.{index}.Url"), url),
    ]
}

/// The parameters of the simplest import this service accepts: one `VMDK` at an S3 URL.
fn one_disk() -> Params {
    url_container(1, "VMDK", "s3://imports/disk-0.vmdk")
}

/// The form fields of one `Filters` entry.
///
/// `DescribeImportImageTasksRequest$Filters` carries `xmlName: "Filters"` (unlike
/// `DescribeVolumes`, whose list flattens to `Filter.N`), so this operation's filters reach
/// the wire as `Filters.1.Name` / `Filters.1.Value.N` — which is why the suite builds them
/// here rather than through the shared harness's `filters` helper.
fn task_filter(name: &str, values: &[&str]) -> Params {
    let mut fields = vec![p("Filters.1.Name", name)];
    fields.extend(indexed("Filters.1.Value", values));
    fields
}

/// Start one import and answer the task id it minted, failing the case if it was refused.
async fn started_task(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = import(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ImportImage, got {status} with body {body}"
    );
    task_member(case, &body, "importTaskId")
        .unwrap_or_else(|| panic!("case {case:?}: no importTaskId in {body}"))
}

/// The task ids `DescribeImportImageTasks` reports for `params`, in the order reported.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImportImageTasks, \
         got {status} with body {body}"
    );
    task_ids(case, &body)
}

/// The response of a describe narrowed to one task id.
async fn describe_one(router: &MultiServiceRouter, case: &str, task_id: &str) -> String {
    let (status, body) = describe(router, &indexed("ImportTaskId", &[task_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {task_id}, got {status} with body {body}"
    );
    assert_eq!(
        task_count(case, &body),
        1,
        "case {case:?}: expected exactly one task for {task_id}, got body {body}"
    );
    body
}

/// How many import tasks the store holds.
fn stored_tasks(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().import_image_tasks.len()
}

// ---------------------------------------------------------------------------
// RULE 1: the request round-trips, the service's own members are derived
// ---------------------------------------------------------------------------

/// One row: the parameters under test, and the members the describe must report for them.
type MemberRow = (&'static str, Params, Vec<(&'static str, &'static str)>);

/// RULE 1 over the disk image format catalog, the two sources a container may name, both
/// documented platforms and architectures, the boot modes and the encryption members: what
/// the request set is what a later `DescribeImportImageTasks` reports, and what the service
/// owns is derived.
#[tokio::test]
async fn every_documented_import_parameter_round_trips_through_the_describe() {
    let mut rows: Vec<MemberRow> = Vec::new();
    // The format catalog, from `ImageDiskContainer$Format`: every published value is a row,
    // including the ones the issue never cited.
    for format in IMPORT_DISK_IMAGE_FORMATS {
        rows.push((
            format,
            url_container(1, format, "s3://imports/disk-0.img"),
            vec![("disk.format", format), ("disk.url", "s3://imports/disk-0.img")],
        ));
    }
    rows.push((
        "a format spelled in lower case is reported in the catalog's spelling",
        url_container(1, "vmdk", "s3://imports/disk-0.vmdk"),
        vec![("disk.format", "VMDK")],
    ));
    rows.push((
        "an i386 import reports that architecture",
        [one_disk(), vec![p("Architecture", "i386")]].concat(),
        vec![("architecture", "i386")],
    ));
    rows.push((
        "an x86_64 import reports that architecture",
        [one_disk(), vec![p("Architecture", "x86_64")]].concat(),
        vec![("architecture", "x86_64")],
    ));
    rows.push((
        "a Linux import retains the source-system license by default",
        [one_disk(), vec![p("Platform", "Linux")]].concat(),
        vec![("platform", "Linux"), ("licenseType", IMPORT_IMAGE_LICENSE_TYPE_BYOL)],
    ));
    rows.push((
        "a Windows import replaces the source-system license by default",
        [one_disk(), vec![p("Platform", "Windows")]].concat(),
        vec![("platform", "Windows"), ("licenseType", IMPORT_IMAGE_LICENSE_TYPE_AWS)],
    ));
    rows.push((
        "a license type the request names wins over the platform default",
        [
            one_disk(),
            vec![
                p("Platform", "Windows"),
                p("LicenseType", IMPORT_IMAGE_LICENSE_TYPE_BYOL),
            ],
        ]
        .concat(),
        vec![("licenseType", IMPORT_IMAGE_LICENSE_TYPE_BYOL)],
    ));
    rows.push((
        "a legacy-bios import reports that boot mode",
        [one_disk(), vec![p("BootMode", "legacy-bios")]].concat(),
        vec![("bootMode", "legacy-bios")],
    ));
    rows.push((
        "a uefi import reports that boot mode",
        [one_disk(), vec![p("BootMode", "uefi")]].concat(),
        vec![("bootMode", "uefi")],
    ));
    rows.push((
        "an encrypted import reports its KMS key",
        [
            one_disk(),
            vec![
                p("Encrypted", "true"),
                p("KmsKeyId", "alias/aws/ebs"),
            ],
        ]
        .concat(),
        vec![("encrypted", "true"), ("kmsKeyId", "alias/aws/ebs")],
    ));
    rows.push((
        "the description and the usage operation are the request's",
        [
            one_disk(),
            vec![
                p("Description", "a staging VM"),
                p("UsageOperation", "RunInstances:0002"),
            ],
        ]
        .concat(),
        vec![
            ("description", "a staging VM"),
            ("usageOperation", "RunInstances:0002"),
        ],
    ));
    rows.push((
        "a request that names no hypervisor imports onto the one valid value",
        one_disk(),
        vec![("hypervisor", "xen")],
    ));
    rows.push((
        "a request that names no encryption is not encrypted",
        one_disk(),
        vec![("encrypted", "false")],
    ));

    for (case, params, expected) in rows {
        let (router, engine) = emulator();
        let task_id = started_task(&router, case, &params).await;
        assert!(
            is_long_id(IMPORT_IMAGE_TASK_ID_PREFIX, &task_id),
            "case {case:?}: expected an {IMPORT_IMAGE_TASK_ID_PREFIX}- long id, got {task_id:?}"
        );
        let body = describe_one(&router, case, &task_id).await;
        for (member, value) in expected {
            // A member named `disk.<name>` is one of the task's first snapshot detail; every
            // other is the task's own.
            let actual = match member.strip_prefix("disk.") {
                Some(name) => snapshot_member(case, &body, 0, name),
                None => task_member(case, &body, member),
            };
            assert_eq!(
                actual.as_deref(),
                Some(value),
                "case {case:?}: expected <{member}> {value:?}, got {actual:?} in {body}"
            );
        }
        // What the service owns: the task is running, it has made some progress and it has
        // no AMI and no snapshot yet.
        let status = task_member(case, &body, "status");
        assert_eq!(
            status.as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {case:?}: expected status {IMPORT_TASK_STATE_ACTIVE:?}, got {status:?}"
        );
        let message = task_member(case, &body, "statusMessage");
        assert_eq!(
            message.as_deref(),
            Some(IMPORT_TASK_STATUS_MESSAGE_CONVERTING),
            "case {case:?}: expected statusMessage \
             {IMPORT_TASK_STATUS_MESSAGE_CONVERTING:?}, got {message:?}"
        );
        let progress = task_member(case, &body, "progress")
            .unwrap_or_else(|| panic!("case {case:?}: no <progress> in {body}"));
        let percent: u64 = progress
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: progress {progress:?} is not a number"));
        assert!(
            percent <= MAX_ACTIVE_IMPORT_PROGRESS,
            "case {case:?}: expected progress at most {MAX_ACTIVE_IMPORT_PROGRESS}, \
             got {percent}"
        );
        assert_eq!(
            task_member(case, &body, "imageId"),
            None,
            "case {case:?}: an active import has created no AMI yet, body {body}"
        );
        assert_eq!(
            snapshot_member(case, &body, 0, "snapshotId"),
            None,
            "case {case:?}: an active import has created no snapshot yet, body {body}"
        );
        let detail_status = snapshot_member(case, &body, 0, "status");
        assert_eq!(
            detail_status.as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {case:?}: expected the disk to be {IMPORT_TASK_STATE_ACTIVE:?}, \
             got {detail_status:?}"
        );
        assert_eq!(
            stored_tasks(&engine),
            1,
            "case {case:?}: expected exactly one stored task"
        );
    }
}

/// A container may name its disk image by a bucket and key instead of a URL, and an import
/// of several disks reports one snapshot detail per disk, in the order the request listed
/// them.
#[tokio::test]
async fn a_disk_is_named_by_a_url_or_by_a_bucket_and_key_and_every_disk_is_reported() {
    let (router, _engine) = emulator();

    let case = "an S3 bucket and key name the disk image";
    let params = vec![
        p("DiskContainer.1.Format", "VHDX"),
        p("DiskContainer.1.UserBucket.S3Bucket", "imports"),
        p("DiskContainer.1.UserBucket.S3Key", "disk-0.vhdx"),
        p("DiskContainer.1.DeviceName", "/dev/sda1"),
        p("DiskContainer.1.Description", "the root disk"),
    ];
    let task_id = started_task(&router, case, &params).await;
    let body = describe_one(&router, case, &task_id).await;
    for (member, expected) in [
        ("s3Bucket", "imports"),
        ("s3Key", "disk-0.vhdx"),
        ("deviceName", "/dev/sda1"),
        ("description", "the root disk"),
        ("format", "VHDX"),
    ] {
        let actual = snapshot_member(case, &body, 0, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected the disk's <{member}> {expected:?}, \
             got {actual:?} in {body}"
        );
    }

    for disks in [1_usize, 2, 3] {
        let (router, _engine) = emulator();
        let case = "one snapshot detail per imported disk";
        let params: Params = (1..=disks)
            .flat_map(|index| {
                url_container(index, "RAW", &format!("s3://imports/disk-{index}.raw"))
            })
            .collect();
        let task_id = started_task(&router, case, &params).await;
        let body = describe_one(&router, case, &task_id).await;
        assert_eq!(
            snapshot_count(case, &body),
            disks,
            "case {case:?}: expected {disks} snapshot details, got body {body}"
        );
        let last = snapshot_member(case, &body, disks - 1, "url");
        assert_eq!(
            last.as_deref(),
            Some(format!("s3://imports/disk-{disks}.raw").as_str()),
            "case {case:?}: expected the last disk to be the last one listed, got {last:?}"
        );
    }
}

/// The tags a request applies to the task it creates round-trip through the describe, and
/// a specification naming any other resource type is refused.
#[tokio::test]
async fn the_tags_of_the_task_round_trip_and_another_resource_type_is_refused() {
    let (router, engine) = emulator();

    let case = "the task carries the tags the request applied to it";
    let params = [
        one_disk(),
        vec![
            p("TagSpecification.1.ResourceType", "import-image-task"),
            p("TagSpecification.1.Tag.1.Key", "project"),
            p("TagSpecification.1.Tag.1.Value", "migration"),
            p("TagSpecification.1.Tag.2.Key", "owner"),
            p("TagSpecification.1.Tag.2.Value", "platform"),
        ],
    ]
    .concat();
    let task_id = started_task(&router, case, &params).await;
    let body = describe_one(&router, case, &task_id).await;
    let tags = task_tags(case, &body);
    assert_eq!(
        tags,
        vec![
            ("owner".to_string(), "platform".to_string()),
            ("project".to_string(), "migration".to_string()),
        ],
        "case {case:?}: expected both tags, got {tags:?} in {body}"
    );

    let case = "a tag specification for another resource type is refused";
    let refused = [
        one_disk(),
        vec![
            p("TagSpecification.1.ResourceType", "image"),
            p("TagSpecification.1.Tag.1.Key", "project"),
            p("TagSpecification.1.Tag.1.Value", "migration"),
        ],
    ]
    .concat();
    let (status, body) = import(&router, &refused).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {body}"
    );
    assert_eq!(
        stored_tasks(&engine),
        1,
        "case {case:?}: the refusal stored nothing, so only the task the first import \
         started is held"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the clock completes the import, and the AMI it created is an image
// ---------------------------------------------------------------------------

/// A task is `active` and the clock — not a caller — moves it to `completed`, minting the
/// AMI of the imported VM and a snapshot id per imported disk. The AMI is then an image of
/// the account, and both are stable across reads.
#[tokio::test]
async fn the_clock_completes_the_import_and_mints_the_ami_it_created() {
    for (case, disks, platform, architecture) in [
        ("one Linux disk", 1_usize, "Linux", "x86_64"),
        ("two Windows disks", 2, "Windows", "x86_64"),
        ("three i386 disks", 3, "Linux", "i386"),
    ] {
        let (router, engine) = emulator();
        let params: Params = (1..=disks)
            .flat_map(|index| {
                url_container(index, "VMDK", &format!("s3://imports/disk-{index}.vmdk"))
            })
            .chain([p("Platform", platform), p("Architecture", architecture)])
            .collect();
        let task_id = started_task(&router, case, &params).await;

        // Before the span is up the caller still sees the transient state.
        let body = describe_one(&router, case, &task_id).await;
        let status = task_member(case, &body, "status");
        assert_eq!(
            status.as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {case:?}: expected the import to still be \
             {IMPORT_TASK_STATE_ACTIVE:?}, got {status:?}"
        );

        past_the_transition().await;

        let body = describe_one(&router, case, &task_id).await;
        let status = task_member(case, &body, "status");
        assert_eq!(
            status.as_deref(),
            Some(IMPORT_TASK_STATE_COMPLETED),
            "case {case:?}: expected the clock to have completed the import, got {status:?}"
        );
        assert_eq!(
            task_member(case, &body, "progress"),
            None,
            "case {case:?}: a completed import has no progress left to report, body {body}"
        );
        let image_id = task_member(case, &body, "imageId")
            .unwrap_or_else(|| panic!("case {case:?}: no <imageId> in {body}"));
        assert!(
            is_long_id("ami", &image_id),
            "case {case:?}: expected an ami- long id, got {image_id:?}"
        );
        for index in 0..disks {
            let snapshot_id = snapshot_member(case, &body, index, "snapshotId")
                .unwrap_or_else(|| panic!("case {case:?}: no <snapshotId> for disk {index}"));
            assert!(
                is_long_id("snap", &snapshot_id),
                "case {case:?}: expected a snap- long id for disk {index}, \
                 got {snapshot_id:?}"
            );
            let detail_status = snapshot_member(case, &body, index, "status");
            assert_eq!(
                detail_status.as_deref(),
                Some(IMPORT_TASK_STATE_COMPLETED),
                "case {case:?}: expected disk {index} to be completed, got {detail_status:?}"
            );
        }

        // Reading again reports the same AMI: the completion was applied once.
        let again = describe_one(&router, case, &task_id).await;
        assert_eq!(
            task_member(case, &again, "imageId").as_deref(),
            Some(image_id.as_str()),
            "case {case:?}: expected the same AMI on a second read, body {again}"
        );

        // The AMI the import created is an image of the account, named after the import.
        let (status, images) = call(
            &router,
            REGION,
            "DescribeImages",
            &indexed("ImageId", &[image_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with {images}"
        );
        let parsed = document(case, &images);
        let image = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "imageId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no image in {images}"));
        assert_eq!(
            child_text(image, "name").as_deref(),
            Some(task_id.as_str()),
            "case {case:?}: expected the AMI to be named after the import task, in {images}"
        );
        assert_eq!(
            child_text(image, "architecture").as_deref(),
            Some(architecture),
            "case {case:?}: expected the AMI's architecture to be the import's, in {images}"
        );
        assert_eq!(
            stored_tasks(&engine),
            1,
            "case {case:?}: completing an import creates no second task"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a request outside what the model documents is refused
// ---------------------------------------------------------------------------

/// One refusal row: `(case, the parameters, the HTTP status, the error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// RULE 3 over every documented catalog and combination rule: the refusal carries EC2's
/// own code, and nothing is stored.
#[tokio::test]
async fn a_request_outside_what_the_model_documents_is_refused_and_stores_nothing() {
    let rows: Vec<RefusalRow> = vec![
        (
            // `arm64` is a value of the modelled `ArchitectureValues` enum, but NOT one
            // `ImportImageRequest$Architecture` publishes.
            "an architecture outside i386 | x86_64",
            [one_disk(), vec![p("Architecture", "arm64")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a hypervisor outside xen",
            [one_disk(), vec![p("Hypervisor", "kvm")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a platform outside Windows | Linux",
            [one_disk(), vec![p("Platform", "macOS")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a license type outside AWS | BYOL",
            [one_disk(), vec![p("LicenseType", "FREE")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "the uefi-preferred boot mode, which importing does not support",
            [one_disk(), vec![p("BootMode", "uefi-preferred")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a KMS key without the encrypted flag",
            [one_disk(), vec![p("KmsKeyId", "alias/aws/ebs")]].concat(),
            400,
            "InvalidParameterCombination",
        ),
        (
            "no disk container at all",
            Vec::new(),
            400,
            "MissingParameter",
        ),
        (
            "a disk container naming two sources",
            vec![
                p("DiskContainer.1.Format", "VMDK"),
                p("DiskContainer.1.Url", "s3://imports/disk-0.vmdk"),
                p("DiskContainer.1.UserBucket.S3Bucket", "imports"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a disk container naming no source",
            vec![p("DiskContainer.1.Format", "VMDK")],
            400,
            "MissingParameter",
        ),
        (
            "a disk container with no format",
            vec![p("DiskContainer.1.Url", "s3://imports/disk-0.vmdk")],
            400,
            "MissingParameter",
        ),
        (
            "a format outside the published catalog",
            url_container(1, "QCOW2", "s3://imports/disk-0.qcow2"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a snapshot id no snapshot of the account has",
            vec![
                p("DiskContainer.1.Format", "VMDK"),
                p("DiskContainer.1.SnapshotId", "snap-0123456789abcdef0"),
            ],
            400,
            "InvalidSnapshot.NotFound",
        ),
        (
            "something that is not a snapshot id at all",
            vec![
                p("DiskContainer.1.Format", "VMDK"),
                p("DiskContainer.1.SnapshotId", "not-a-snapshot"),
            ],
            400,
            "InvalidSnapshotID.Malformed",
        ),
        (
            "a DryRun that would have succeeded",
            [one_disk(), vec![p("DryRun", "true")]].concat(),
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let (status, body) = import(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} in {body}"
        );
        assert_eq!(
            stored_tasks(&engine),
            0,
            "case {case:?}: a refused import must store nothing"
        );
    }
}

/// The per-task element counts the model leaves unbounded are accepted at their ceiling
/// and refused one past it, so one request cannot store an unbounded number of records.
#[tokio::test]
async fn the_unbounded_lists_of_a_task_are_capped_at_their_ceilings() {
    // `(case, how many elements, the parameters for that many, the expected status)`
    let disks = |count: usize| -> Params {
        (1..=count)
            .flat_map(|index| url_container(index, "RAW", &format!("s3://imports/disk-{index}.raw")))
            .collect()
    };
    let licenses = |count: usize| -> Params {
        let mut fields = one_disk();
        for index in 1..=count {
            fields.push(p(
                &format!("LicenseSpecifications.{index}.LicenseConfigurationArn"),
                format!("arn:aws:license-manager:us-west-2:123456789012:license-configuration:lic-{index}"),
            ));
        }
        fields
    };
    let rows: Vec<(&str, Params, u16)> = vec![
        (
            "disk containers at the ceiling",
            disks(MAX_IMPORT_DISK_CONTAINERS),
            200,
        ),
        (
            "one disk container past the ceiling",
            disks(MAX_IMPORT_DISK_CONTAINERS + 1),
            400,
        ),
        (
            "license specifications at the ceiling",
            licenses(MAX_IMPORT_LICENSE_SPECIFICATIONS),
            200,
        ),
        (
            "one license specification past the ceiling",
            licenses(MAX_IMPORT_LICENSE_SPECIFICATIONS + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in rows {
        let (router, engine) = emulator();
        let (status, body) = import(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                stored_tasks(&engine),
                1,
                "case {case:?}: expected the task to be stored"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got body {body}"
            );
            assert_eq!(
                stored_tasks(&engine),
                0,
                "case {case:?}: a refused import must store nothing"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The shared filter matcher this describe's `task-state` filter runs on
// ---------------------------------------------------------------------------

/// Every path through the shared matcher charges the budget, and a filter value above the
/// token ceiling matches nothing — the invariant that makes the budget a bound on the
/// PRODUCT of candidates, filters, values and pattern length, which is what keeps one
/// `DescribeImportImageTasks` call's filtering work bounded.
///
/// Driven through [`ec2::state::FilterPattern`] directly because the shapes that matter —
/// an empty candidate value, a pattern of nothing but wildcards, a value longer than any
/// handler accepts — are properties of the matcher rather than of one request.
#[test]
fn every_comparison_the_shared_matcher_makes_is_charged_to_the_budget() {
    // `(case, the filter value, the candidate value, the budget, matched, exhausted)`
    let wildcards = "*".repeat(64);
    let at_ceiling = "a".repeat(MAX_FILTER_PATTERN_TOKENS);
    let past_ceiling = "a".repeat(MAX_FILTER_PATTERN_TOKENS + 1);
    let rows: Vec<(&str, &str, &str, u64, bool, bool)> = vec![
        (
            "an empty candidate value against an all-wildcard pattern is charged per token",
            wildcards.as_str(),
            "",
            8,
            false,
            true,
        ),
        (
            "the same comparison inside a budget that covers it matches",
            wildcards.as_str(),
            "",
            1_000,
            true,
            false,
        ),
        ("a comparison costs at least one step", "", "", 1, true, true),
        (
            "a filter value at the token ceiling still matches",
            at_ceiling.as_str(),
            at_ceiling.as_str(),
            1_000_000,
            true,
            false,
        ),
        (
            "a filter value past the token ceiling matches nothing",
            past_ceiling.as_str(),
            past_ceiling.as_str(),
            1_000_000,
            false,
            false,
        ),
        (
            "a wildcard still matches what it did",
            "*-ami-*",
            "import-ami-0000000000000001c",
            1_000_000,
            true,
            false,
        ),
        (
            "a single-character wildcard still matches exactly one",
            "activ?",
            IMPORT_TASK_STATE_ACTIVE,
            1_000_000,
            true,
            false,
        ),
        (
            "a value that does not match still does not",
            IMPORT_TASK_STATE_COMPLETED,
            IMPORT_TASK_STATE_ACTIVE,
            1_000_000,
            false,
            false,
        ),
    ];

    for (case, pattern, value, steps, expected_match, expected_exhausted) in rows {
        let compiled = FilterPattern::compile(pattern);
        let mut budget = MatchBudget::new(steps);
        let matched = compiled.matches_within(value, &mut budget);
        assert_eq!(
            matched, expected_match,
            "case {case:?}: expected matched={expected_match}, got {matched}"
        );
        assert_eq!(
            budget.exhausted(),
            expected_exhausted,
            "case {case:?}: expected exhausted={expected_exhausted}, got {}",
            budget.exhausted()
        );
    }
}


/// A repeat of the same request under one token reports the task that token started and
/// imports nothing again; a different token starts a second import.
#[tokio::test]
async fn one_client_token_names_one_import_task() {
    let (router, engine) = emulator();
    let case = "the same token twice";
    let params = [one_disk(), vec![p("ClientToken", "import-once")]].concat();

    let first = started_task(&router, case, &params).await;
    let second = started_task(&router, case, &params).await;
    assert_eq!(
        first, second,
        "case {case:?}: expected one task for one token, got {first} and {second}"
    );
    assert_eq!(
        stored_tasks(&engine),
        1,
        "case {case:?}: expected exactly one stored task"
    );

    let case = "a different token";
    let other = started_task(
        &router,
        case,
        &[one_disk(), vec![p("ClientToken", "import-again")]].concat(),
    )
    .await;
    assert_ne!(
        other, first,
        "case {case:?}: expected a second task, got {other} again"
    );
    assert_eq!(
        stored_tasks(&engine),
        2,
        "case {case:?}: expected two stored tasks"
    );

    // A replay arriving after the clock completed the import reports the SAME thing the
    // describe does: the completion is applied once, by whichever call is first to see it
    // due.
    past_the_transition().await;
    let case = "a replay after the import completed";
    let (status, body) = import(
        &router,
        &[one_disk(), vec![p("ClientToken", "import-once")]].concat(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let replayed_status = task_member(case, &body, "status");
    assert_eq!(
        replayed_status.as_deref(),
        Some(IMPORT_TASK_STATE_COMPLETED),
        "case {case:?}: expected the replay to report the completed task, \
         got {replayed_status:?} in {body}"
    );
    let replayed_image = task_member(case, &body, "imageId");
    let described = describe_one(&router, case, &first).await;
    assert_eq!(
        replayed_image,
        task_member(case, &described, "imageId"),
        "case {case:?}: expected the replay and the describe to name one AMI, \
         got {replayed_image:?} and body {described}"
    );
    assert_eq!(
        stored_tasks(&engine),
        2,
        "case {case:?}: a replay starts no further import"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: narrowing, filtering and paging
// ---------------------------------------------------------------------------

/// Seed one task in `status`, as a cancel would leave it, and answer its id.
///
/// `deleting` and `deleted` are states of the published `task-state` catalog that no
/// operation of this emulator produces yet, so every state of the catalog is written into
/// the store directly here — the filter is a rule about the catalog, not about the two
/// states an import walks, and seeding takes the clock out of the comparison. An `active`
/// task is seeded with a window long enough that it is still active when the filter runs;
/// what the clock does to a real import is [`the_clock_completes_the_import_and_mints_the_ami_it_created`].
fn seed_task(engine: &engine::InMemoryEngine, status: &str) -> String {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let task_id = store.next_id(IMPORT_IMAGE_TASK_ID_PREFIX);
    let now = Instant::now();
    let completed_at = if status == IMPORT_TASK_STATE_ACTIVE {
        now + Duration::from_secs(3_600)
    } else {
        now
    };
    store.import_image_tasks.insert(
        task_id.clone(),
        ImportImageTaskRecord {
            view: ImportImageTask {
                import_task_id: Some(task_id.clone()),
                status: Some(status.to_string()),
                snapshot_details: Some(vec![SnapshotDetail {
                    format: Some("VMDK".to_string()),
                    status: Some(status.to_string()),
                    ..SnapshotDetail::default()
                }]),
                tags: Some(Vec::<Tag>::new()),
                ..ImportImageTask::default()
            },
            started_at: now,
            completed_at,
        },
    );
    task_id
}

/// The `task-state` filter selects the tasks in the state named, over every value of its
/// published catalog, and several values are joined with an OR.
#[tokio::test]
async fn the_task_state_filter_selects_over_its_published_catalog() {
    let (router, engine) = emulator();

    // One task per published state, so every row of the catalog is a case.
    let seeded: Vec<(&str, String)> = IMPORT_TASK_STATES
        .iter()
        .map(|state| (*state, seed_task(&engine, state)))
        .collect();

    for (state, task_id) in &seeded {
        let case = *state;
        let selected = described_ids(&router, case, &task_filter("task-state", &[state])).await;
        assert_eq!(
            selected,
            vec![task_id.clone()],
            "case {case:?}: expected the one task in that state, got {selected:?}"
        );
    }

    let case = "two values are joined with an OR";
    let mut selected = described_ids(
        &router,
        case,
        &task_filter(
            "task-state",
            &[IMPORT_TASK_STATE_DELETING, IMPORT_TASK_STATE_DELETED],
        ),
    )
    .await;
    selected.sort();
    let mut wanted: Vec<String> = seeded
        .iter()
        .filter(|(state, _)| {
            *state == IMPORT_TASK_STATE_DELETING || *state == IMPORT_TASK_STATE_DELETED
        })
        .map(|(_, task_id)| task_id.clone())
        .collect();
    wanted.sort();
    assert_eq!(
        selected, wanted,
        "case {case:?}: expected both cancelled tasks, got {selected:?}"
    );

    let case = "a task an import completed is selected by the completed state";
    let completed = started_task(&router, case, &one_disk()).await;
    past_the_transition().await;
    let selected = described_ids(
        &router,
        case,
        &task_filter("task-state", &[IMPORT_TASK_STATE_COMPLETED]),
    )
    .await;
    assert!(
        selected.contains(&completed),
        "case {case:?}: expected {completed} among {selected:?}"
    );

    let case = "a value no task is in selects nothing";
    let selected = described_ids(
        &router,
        case,
        &task_filter("task-state", &["no-such-state"]),
    )
    .await;
    assert!(
        selected.is_empty(),
        "case {case:?}: expected no task, got {selected:?}"
    );

    let case = "a filter this operation does not document selects nothing";
    let selected = described_ids(
        &router,
        case,
        &task_filter("not-a-filter-of-this-operation", &["active"]),
    )
    .await;
    assert!(
        selected.is_empty(),
        "case {case:?}: an import task has no such attribute to match, got {selected:?}"
    );
    // ... while the documented filter still selects, so the empty answer above is the
    // narrowing and not an empty store.
    let case = "the documented filter still selects";
    let selected = described_ids(
        &router,
        case,
        &task_filter("task-state", &[IMPORT_TASK_STATE_DELETING]),
    )
    .await;
    assert_eq!(
        selected.len(),
        1,
        "case {case:?}: expected the one deleting task, got {selected:?}"
    );
}

/// `ImportTaskIds` narrows the report to the ids named, an id no task of the account has
/// selects no row, and something that is not an import task id at all is refused.
#[tokio::test]
async fn import_task_ids_narrow_the_report_and_a_malformed_id_is_refused() {
    let (router, _engine) = emulator();
    let first = started_task(&router, "first", &one_disk()).await;
    let second = started_task(&router, "second", &one_disk()).await;
    let third = started_task(&router, "third", &one_disk()).await;

    for (case, ids) in [
        ("one id", vec![second.clone()]),
        ("two ids", vec![first.clone(), third.clone()]),
        (
            "every id",
            vec![first.clone(), second.clone(), third.clone()],
        ),
    ] {
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut selected = described_ids(&router, case, &indexed("ImportTaskId", &named)).await;
        selected.sort();
        let mut wanted = ids.clone();
        wanted.sort();
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected the tasks named, got {selected:?}"
        );
    }

    // A well-formed id no task of the account has selects no row: the answer is "a list of
    // zero or more import image tasks" (`DescribeImportImageTasksResult$ImportImageTasks`),
    // so an absent task is an absent row, not a refusal.
    for (case, ids, wanted) in [
        (
            "an id no task of the account has",
            vec!["import-ami-0123456789abcdef0".to_string()],
            Vec::new(),
        ),
        (
            "a known id beside an unknown one",
            vec![first.clone(), "import-ami-0123456789abcdef0".to_string()],
            vec![first.clone()],
        ),
    ] {
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let selected = described_ids(&router, case, &indexed("ImportTaskId", &named)).await;
        assert_eq!(
            selected, wanted,
            "case {case:?}: expected {wanted:?}, got {selected:?}"
        );
    }

    // Something that is not an import task id at all is refused before any lookup.
    for (case, id) in [
        ("an id of another resource kind", "vpc-0123456789abcdef0"),
        ("something that is not an id", "not-an-import-task"),
        ("the prefix with no suffix", "import-ami-"),
    ] {
        let (status, body) = describe(&router, &indexed("ImportTaskId", &[id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidConversionTaskId",
            "case {case:?}: expected InvalidConversionTaskId, got body {body}"
        );
    }
}

/// Paging covers every task exactly once at every page size, the token is opaque and one
/// this service did not mint is refused, and `MaxResults` is accepted at its bounds and
/// refused past them.
#[tokio::test]
async fn paging_covers_every_task_exactly_once_for_each_page_size() {
    let (router, _engine) = emulator();
    let mut all: Vec<String> = Vec::new();
    for index in 1..=5 {
        all.push(
            started_task(
                &router,
                "seeding the store",
                &url_container(1, "VMDK", &format!("s3://imports/disk-{index}.vmdk")),
            )
            .await,
        );
    }
    all.sort();

    for page_size in 1..=5_usize {
        let case = "paging at every page size";
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=all.len() {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(next) = token.clone() {
                params.push(p("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({page_size} per page): expected HTTP 200, \
                 got {status} with body {body}"
            );
            let page = task_ids(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} tasks in a page, got {page:?}"
            );
            seen.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen, all,
            "case {case:?} ({page_size} per page): expected every task exactly once, \
             got {seen:?}"
        );
    }

    for (case, max_results, expected_status) in [
        ("the smallest page size", "1".to_string(), 200_u16),
        ("the largest page size", MAX_PAGE_SIZE.to_string(), 200),
        ("one past the largest", (MAX_PAGE_SIZE + 1).to_string(), 400),
        ("a page of none", "0".to_string(), 400),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results.clone())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={max_results}, \
             got {status} with body {body}"
        );
    }

    let case = "a token this service did not mint";
    let (status, body) = describe(&router, &[p("NextToken", "deadbeef")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "case {case:?}: expected InvalidPaginationToken, got body {body}"
    );
}

/// Describing is a report: a `DryRun` describe says so without reporting anything, and two
/// reads of the same store answer the same thing.
#[tokio::test]
async fn describing_reports_the_store_and_changes_nothing() {
    let (router, engine) = emulator();
    let case = "an empty store";
    let selected = described_ids(&router, case, &[]).await;
    assert!(
        selected.is_empty(),
        "case {case:?}: an account that has imported nothing has no task, got {selected:?}"
    );

    let task_id = started_task(&router, "one import", &one_disk()).await;
    let case = "a DryRun describe";
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got body {body}"
    );

    let case = "two reads of the same store";
    let before = described_ids(&router, case, &[]).await;
    let after = described_ids(&router, case, &[]).await;
    assert_eq!(
        before, after,
        "case {case:?}: expected the same report twice, got {before:?} then {after:?}"
    );
    assert_eq!(
        before,
        vec![task_id],
        "case {case:?}: expected the one task that was started, got {before:?}"
    );
    assert_eq!(
        stored_tasks(&engine),
        1,
        "case {case:?}: describing must not write to the store"
    );
}
