// SPDX-License-Identifier: CC0-1.0

//! What `DescribeConversionTasks` reports out of the conversion-task store, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. THE REPORT IS OF THE STORE: every member a writer of a conversion task set is reported
//!    back, for an import of either kind — "if the task is for importing an instance, this
//!    contains information about the import instance task" / "… a volume …"
//!    (`ConversionTask$ImportInstance`, `ConversionTask$ImportVolume`) — including each disk
//!    of a multi-volume instance import and the task's tags;
//! 2. A REQUEST NAMING NO ID REPORTS ALL THE ACCOUNT'S TASKS and one naming ids reports
//!    exactly those: "describes the specified conversion tasks or all your conversion tasks"
//!    (`DescribeConversionTasks`), in `ConversionTaskId` order however the request listed
//!    them, an id named twice naming it once;
//! 3. AN ID OUTSIDE THE MINTED FORM NAMES NOTHING TO LOOK UP, so its FORM is judged before
//!    the store is reached: it is refused 400 `InvalidConversionTaskId` naming the parameter
//!    and the value it was sent, and no task is reported at all. An id EC2 COULD have issued
//!    that the account holds no task for is NOT an error — the operation reports a LIST of
//!    the account's tasks, which an account holding none of them reports empty — so such an
//!    id selects no row;
//! 4. DESCRIBING CHANGES NOTHING A LATER CALLER SEES: two identical calls answer identically,
//!    an `active` task stays `active` however often it is polled, and the only move a report
//!    shows is one the CLOCK already owed — the end of a cancellation `CancelConversionTask`
//!    started;
//! 5. `DryRun` ANSWERS WHAT THE REQUEST WOULD HAVE DONE: `DryRunOperation` for one that
//!    would have worked, and the refusal it would have been refused with otherwise.
//!
//! Each row is a distinct input along what the rules generalise over — which resource the
//! conversion imports, how many disks the import carries, how many tags the task holds, how
//! many tasks the account has, how many ids the request names and in which order, and which
//! way an id departs from the minted form — so no row is the specification. The id rows are
//! GENERATED from the crate's own prefixes, so a kind added there is covered here without a
//! new literal.
//!
//! A conversion task is seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_conversion_task`], the writer
//! `ImportInstance` and `ImportVolume` will use once they are implemented (both still
//! scaffolds in this crate), and what the report says is read back over the WIRE.

use std::time::Duration;

use ec2::ops::types::{
    DiskImageDescription, DiskImageVolumeDescription, ImportInstanceTaskDetails,
    ImportInstanceVolumeDetailItem, ImportVolumeTaskDetails, Tag,
};
use ec2::state::{
    CONVERSION_TASK_ID_PARAMETER, CONVERSION_TASK_ID_PREFIXES, CONVERSION_TASK_STATE_ACTIVE,
    CONVERSION_TASK_STATE_CANCELLED, CONVERSION_TASK_STATE_CANCELLING,
    ComAmazonawsEc2Amazonec2State as Ec2State, ConversionTaskDetail, ConversionTaskSpec,
    DRY_RUN_OPERATION, INVALID_CONVERSION_TASK_ID, INVALID_PARAMETER_VALUE, MAX_RESOURCE_IDS,
    TRANSITION_DELAY, conversion_task_id_is_well_formed, invalid_parameter_format_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, indexed, member};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. A conversion task carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The two kinds of import one conversion task can be: "the task can be the import of an
/// instance or volume" (`CancelConversionTask`). Every rule here holds for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A whole machine, as `ImportInstance` imports one.
    Instance,
    /// A bare disk, as `ImportVolume` imports one.
    Volume,
}

/// One seeded conversion task: which kind of import it is, how many disks that import
/// carries and how many tags the task holds.
///
/// The members themselves are DERIVED from the case and the disk's position, so the
/// expectations below are generated from the same derivation rather than copied from the
/// seed by hand.
#[derive(Debug, Clone, Copy)]
struct Seed {
    case: &'static str,
    kind: Kind,
    disks: usize,
    tags: usize,
}

/// The disk image formats a seeded disk cycles through: members of the `DiskImageFormat`
/// enum (`smithy_query(query="shape",shape="DiskImageFormat")`).
const DISK_IMAGE_FORMATS: [&str; 3] = ["VMDK", "RAW", "VHD"];

/// The zones a seeded import places its disks in, cycling so one task's disks do not all
/// carry the same value.
const ZONES: [&str; 2] = ["us-east-1a", "us-east-1b"];

/// The disk image of disk `index` of the import `case`, filled in so the round-trip has
/// every member of the shape to compare.
fn disk_image(case: &str, index: usize) -> DiskImageDescription {
    DiskImageDescription {
        checksum: Some(format!("d41d8cd98f00b204e9800998ecf8427{index:x}")),
        format: Some(DISK_IMAGE_FORMATS[index % DISK_IMAGE_FORMATS.len()].to_string()),
        import_manifest_url: Some(format!(
            "https://disk-images.s3.amazonaws.com/{case}-{index}.manifest"
        )),
        size: Some(8 + index as i64),
    }
}

/// The EBS volume disk `index` of the import `case` is converted into.
fn disk_volume(index: usize) -> DiskImageVolumeDescription {
    DiskImageVolumeDescription {
        id: Some(format!("vol-0123456789abcde{index:x}0")),
        size: Some(16 + index as i64),
    }
}

/// Disk `index` of the instance import `case`, as the task reports it.
fn volume_item(case: &str, index: usize) -> ImportInstanceVolumeDetailItem {
    ImportInstanceVolumeDetailItem {
        availability_zone: Some(ZONES[index % ZONES.len()].to_string()),
        availability_zone_id: Some(format!("use1-az{}", index % ZONES.len() + 1)),
        bytes_converted: Some(1024 * (index as i64 + 1)),
        description: Some(format!("disk {index} of {case}")),
        image: Some(disk_image(case, index)),
        status: Some(CONVERSION_TASK_STATE_ACTIVE.to_string()),
        status_message: Some(format!("converting disk {index}")),
        volume: Some(disk_volume(index)),
    }
}

/// The tags the task `case` holds: `tags` of them, keyed so two tasks never share a value.
fn task_tags(case: &str, tags: usize) -> Vec<Tag> {
    (0..tags)
        .map(|index| Tag {
            key: Some(format!("key-{index}")),
            value: Some(format!("{case}-{index}")),
        })
        .collect()
}

/// When the seeded task `case` expires: "the time when the task expires"
/// (`ConversionTask$ExpirationTime`), a STRING on the modelled shape, so the store keeps
/// what the writer rendered.
fn expiration_time(case: &str) -> String {
    format!("2026-11-30T00:00:0{}.000Z", case.len() % 10)
}

/// Store the conversion task `seed` describes and answer the `ConversionTaskId` it was
/// minted under.
fn seed_task(engine: &engine::InMemoryEngine, seed: Seed) -> String {
    let case = seed.case;
    let detail = match seed.kind {
        Kind::Instance => ConversionTaskDetail::Instance(ImportInstanceTaskDetails {
            description: Some(format!("the import of {case}")),
            instance_id: Some("i-0123456789abcdef0".to_string()),
            platform: Some("Windows".to_string()),
            volumes: Some((0..seed.disks).map(|index| volume_item(case, index)).collect()),
        }),
        // A volume import converts ONE disk of its own, so its detail carries the image and
        // the volume directly rather than a set of them.
        Kind::Volume => ConversionTaskDetail::Volume(ImportVolumeTaskDetails {
            availability_zone: Some(ZONES[seed.disks % ZONES.len()].to_string()),
            availability_zone_id: Some(format!("use1-az{}", seed.disks % ZONES.len() + 1)),
            bytes_converted: Some(2048 * (seed.disks as i64 + 1)),
            description: Some(format!("the import of {case}")),
            image: Some(disk_image(case, seed.disks)),
            volume: Some(disk_volume(seed.disks)),
        }),
    };
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_conversion_task(ConversionTaskSpec {
            detail,
            expiration_time: Some(expiration_time(case)),
            tags: task_tags(case, seed.tags),
        })
        .unwrap_or_else(|| panic!("case {case:?}: the store must store the conversion task"))
}

/// Every member the task `seed` was stored with, as `(path, value)` pairs into the reported
/// `<item>`: a slash-separated path of element names, a numeric step selecting the nth entry
/// of a set (counting from 1).
///
/// The disk image and the EBS volume of disk `index` of the import `case`, as `(path, value)`
/// pairs under `prefix` — the two sub-structures an import of either kind reports a disk
/// through.
fn expected_disk_members(case: &str, prefix: &str, index: usize) -> Vec<(String, String)> {
    let image = disk_image(case, index);
    let volume = disk_volume(index);
    vec![
        (
            format!("{prefix}/image/checksum"),
            image.checksum.unwrap_or_default(),
        ),
        (
            format!("{prefix}/image/format"),
            image.format.unwrap_or_default(),
        ),
        (
            format!("{prefix}/image/importManifestUrl"),
            image.import_manifest_url.unwrap_or_default(),
        ),
        (
            format!("{prefix}/image/size"),
            image.size.unwrap_or_default().to_string(),
        ),
        (
            format!("{prefix}/volume/id"),
            volume.id.unwrap_or_default(),
        ),
        (
            format!("{prefix}/volume/size"),
            volume.size.unwrap_or_default().to_string(),
        ),
    ]
}

/// Generated from the SAME derivation the seed uses, so the expectation cannot drift from
/// what was stored.
fn expected_members(seed: Seed, conversion_task_id: &str) -> Vec<(String, String)> {
    let case = seed.case;
    let mut expected = vec![
        ("conversionTaskId".to_string(), conversion_task_id.to_string()),
        ("expirationTime".to_string(), expiration_time(case)),
        (
            "state".to_string(),
            CONVERSION_TASK_STATE_ACTIVE.to_string(),
        ),
    ];
    match seed.kind {
        Kind::Instance => {
            expected.push((
                "importInstance/description".to_string(),
                format!("the import of {case}"),
            ));
            expected.push((
                "importInstance/instanceId".to_string(),
                "i-0123456789abcdef0".to_string(),
            ));
            expected.push(("importInstance/platform".to_string(), "Windows".to_string()));
            for index in 0..seed.disks {
                let item = volume_item(case, index);
                let prefix = format!("importInstance/volumes/{}", index + 1);
                expected.push((
                    format!("{prefix}/availabilityZone"),
                    item.availability_zone.clone().unwrap_or_default(),
                ));
                expected.push((
                    format!("{prefix}/availabilityZoneId"),
                    item.availability_zone_id.clone().unwrap_or_default(),
                ));
                expected.push((
                    format!("{prefix}/bytesConverted"),
                    item.bytes_converted.unwrap_or_default().to_string(),
                ));
                expected.push((
                    format!("{prefix}/description"),
                    item.description.clone().unwrap_or_default(),
                ));
                expected.push((
                    format!("{prefix}/status"),
                    item.status.clone().unwrap_or_default(),
                ));
                expected.push((
                    format!("{prefix}/statusMessage"),
                    item.status_message.clone().unwrap_or_default(),
                ));
                expected.extend(expected_disk_members(case, &prefix, index));
            }
        }
        Kind::Volume => {
            expected.push((
                "importVolume/availabilityZone".to_string(),
                ZONES[seed.disks % ZONES.len()].to_string(),
            ));
            expected.push((
                "importVolume/availabilityZoneId".to_string(),
                format!("use1-az{}", seed.disks % ZONES.len() + 1),
            ));
            expected.push((
                "importVolume/bytesConverted".to_string(),
                (2048 * (seed.disks as i64 + 1)).to_string(),
            ));
            expected.push((
                "importVolume/description".to_string(),
                format!("the import of {case}"),
            ));
            expected.extend(expected_disk_members(case, "importVolume", seed.disks));
        }
    }
    for (index, tag) in task_tags(case, seed.tags).into_iter().enumerate() {
        let prefix = format!("tagSet/{}", index + 1);
        expected.push((format!("{prefix}/key"), tag.key.unwrap_or_default()));
        expected.push((format!("{prefix}/value"), tag.value.unwrap_or_default()));
    }
    expected
}

/// One `DescribeConversionTasks` call carrying `params`.
async fn describe(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, "DescribeConversionTasks", params).await
}

/// One `DescribeConversionTasks` call naming `ids` as `ConversionTaskId.N`, the indexed form
/// ec2Query flattens a list into (the shared harness's [`super::ec2_query::indexed`]).
async fn describe_ids(router: &MultiServiceRouter, ids: &[&str]) -> (u16, String) {
    describe(router, &indexed("ConversionTaskId", ids)).await
}

/// The value at `path` inside the reported task `item`, or `None` when no such element is
/// there: element names separated by `/`, a numeric step selecting the nth entry of a set.
fn at(item: roxmltree::Node<'_, '_>, path: &str) -> Option<String> {
    let mut node = item;
    let steps: Vec<&str> = path.split('/').collect();
    for (position, step) in steps.iter().enumerate() {
        let next = match step.parse::<usize>() {
            // A numeric step indexes the entries of the set the previous step named.
            Ok(index) => node
                .children()
                .filter(roxmltree::Node::is_element)
                .nth(index - 1)?,
            Err(_) => child(node, step)?,
        };
        if position + 1 == steps.len() {
            return Some(next.text().unwrap_or_default().to_string());
        }
        node = next;
    }
    None
}

/// Every task a response reported, as its `<item>` element, in the order it reported them.
///
/// The `<conversionTasks>` member must be PRESENT on every success: an account with no task
/// has an empty list of them, which is what the operation answers.
fn reported_items<'a, 'i>(
    case: &str,
    document: &'a roxmltree::Document<'i>,
    body: &str,
) -> Vec<roxmltree::Node<'a, 'i>> {
    let root = document.root_element();
    assert!(
        child(root, "conversionTasks").is_some(),
        "case {case:?}: the response must carry a <conversionTasks> member, got {body}"
    );
    set_items(root, "conversionTasks")
}

/// The ids a response reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    reported_items(case, &parsed, body)
        .into_iter()
        .map(|item| child_text(item, "conversionTaskId").unwrap_or_default())
        .collect()
}

/// The ids `DescribeConversionTasks` reports for `params`, having pinned the call succeeded.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    reported_ids(case, &body)
}

// ---------------------------------------------------------------------------
// RULE 1: the report is of the store
// ---------------------------------------------------------------------------

/// The seeded tasks the round-trip is asserted over: both kinds of import, an instance
/// import of one disk and of several, a task with no tag and one with several.
fn round_trip_seeds() -> Vec<Seed> {
    vec![
        Seed {
            case: "one-disk-instance-import",
            kind: Kind::Instance,
            disks: 1,
            tags: 1,
        },
        Seed {
            case: "three-disk-instance-import",
            kind: Kind::Instance,
            disks: 3,
            tags: 2,
        },
        Seed {
            case: "untagged-instance-import",
            kind: Kind::Instance,
            disks: 2,
            tags: 0,
        },
        Seed {
            case: "volume-import",
            kind: Kind::Volume,
            disks: 0,
            tags: 1,
        },
        Seed {
            case: "untagged-volume-import",
            kind: Kind::Volume,
            disks: 1,
            tags: 0,
        },
    ]
}

/// Rule 1: every member a writer of a conversion task set is reported back, for an import of
/// either kind, each disk of a multi-disk instance import included.
#[tokio::test]
async fn every_member_of_a_stored_conversion_task_is_reported() {
    let (router, engine) = emulator();
    for seed in round_trip_seeds() {
        let case = seed.case;
        let conversion_task_id = seed_task(&engine, seed);
        let (status, body) = describe_ids(&router, &[&conversion_task_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status} with body {body}"
        );
        let parsed = document(case, &body);
        let items = reported_items(case, &parsed, &body);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: naming one task must report exactly that task, got {} in {body}",
            items.len()
        );
        let item = items[0];
        for (path, expected) in expected_members(seed, &conversion_task_id) {
            let actual = at(item, &path);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {path} to report {expected:?}, got {actual:?} \
                 in {body}"
            );
        }
        // The detail of the OTHER kind of import is not reported at all: a task is the
        // import of an instance or of a volume, never of both.
        let absent = match seed.kind {
            Kind::Instance => "importVolume",
            Kind::Volume => "importInstance",
        };
        assert!(
            child(item, absent).is_none(),
            "case {case:?}: a {:?} import must report no <{absent}>, got {body}",
            seed.kind
        );
        // Nothing has gone wrong with an import that has just started, so there is no
        // status message to report.
        assert_eq!(
            at(item, "statusMessage"),
            None,
            "case {case:?}: a task nothing has gone wrong with reports no statusMessage, \
             got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: what a request narrows the report to
// ---------------------------------------------------------------------------

/// Rule 2: a request naming no id reports ALL the account's tasks, and one naming ids reports
/// exactly those — in `ConversionTaskId` order however the request listed them, an id named
/// twice naming it once.
#[tokio::test]
async fn the_report_is_narrowed_to_the_ids_the_request_names() {
    let (router, engine) = emulator();

    // An account holding no conversion task at all has an EMPTY list of them.
    assert_eq!(
        described_ids(&router, "an account with no conversion task", &[]).await,
        Vec::<String>::new(),
        "an account holding no conversion task must report an empty list"
    );

    let mut seeded: Vec<String> = Vec::new();
    for seed in round_trip_seeds() {
        seeded.push(seed_task(&engine, seed));
    }
    let mut in_id_order = seeded.clone();
    in_id_order.sort();

    // `(case, the ids the request names, the ids the report must carry)`: no id at all, one
    // id, a subset, every id, the ids in reverse, and one id named twice.
    let first = seeded[0].clone();
    let last = seeded[seeded.len() - 1].clone();
    let mut reversed = in_id_order.clone();
    reversed.reverse();
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("no id at all", Vec::new(), in_id_order.clone()),
        (
            "one id",
            vec![first.clone()],
            vec![first.clone()],
        ),
        (
            "a subset of the account's tasks",
            vec![last.clone(), first.clone()],
            {
                let mut both = vec![first.clone(), last.clone()];
                both.sort();
                both
            },
        ),
        (
            "every id the account holds",
            seeded.clone(),
            in_id_order.clone(),
        ),
        (
            "every id in reverse order",
            reversed.clone(),
            in_id_order.clone(),
        ),
        (
            "one id named twice",
            vec![first.clone(), first.clone()],
            vec![first.clone()],
        ),
    ];

    for (case, named, expected) in rows {
        let ids: Vec<&str> = named.iter().map(String::as_str).collect();
        let (status, body) = describe_ids(&router, &ids).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status} with body {body}"
        );
        let actual = reported_ids(case, &body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: an id outside the minted form is refused; one no task answers to
// selects no row
// ---------------------------------------------------------------------------

/// Values that are NOT conversion task ids: each row departs from the minted form in one
/// way — a suffix a digit short of a minted length or a digit past it, a digit outside
/// lowercase hexadecimal, an empty suffix, a missing or unknown kind segment, another
/// resource's id, and a value of no resource-id shape at all. Generated over BOTH minting
/// prefixes, so neither kind's form is asserted only through the other's.
fn malformed_id_rows() -> Vec<(String, String)> {
    let departures = [
        ("one digit short of the short form", "0123abc"),
        ("one digit past the short form", "0123abcde"),
        ("one digit short of the long form", "0123456789abcdef"),
        ("one digit past the long form", "0123456789abcdef01"),
        ("an uppercase suffix", "0123ABCD"),
        ("a suffix that is not hexadecimal", "0123abcg"),
        ("an empty suffix", ""),
    ];
    let mut rows = Vec::new();
    for prefix in CONVERSION_TASK_ID_PREFIXES {
        for (case, suffix) in departures {
            rows.push((
                format!("{case} under the {prefix} prefix"),
                format!("{prefix}-{suffix}"),
            ));
        }
    }
    for (case, id) in [
        ("no kind segment at all", "import-0123abcd"),
        ("an unknown kind segment", "import-snap-0123abcd"),
        ("another resource's id", "i-0123456789abcdef0"),
        ("a value of no resource-id shape", "the-first-task"),
    ] {
        rows.push((case.to_string(), id.to_string()));
    }
    rows
}

/// Every id form a conversion task EC2 issued can carry, used here for an id EC2 COULD have
/// issued that no task of the account answers to: each kind's minting prefix crossed with
/// the long (17 hex digit) form EC2 mints today and the short (8 hex digit) form it minted
/// before long ids.
fn unheld_id_rows() -> Vec<(String, String)> {
    let suffixes = [
        ("the long (17 hex digit) form", "0123456789abcdef0"),
        ("the short (8 hex digit) form", "0123abcd"),
    ];
    let mut rows = Vec::new();
    for prefix in CONVERSION_TASK_ID_PREFIXES {
        for (length, suffix) in suffixes {
            rows.push((
                format!("an unheld {prefix} id in {length}"),
                format!("{prefix}-{suffix}"),
            ));
        }
    }
    rows
}

/// Rule 3: a value outside the minted form is refused for its FORMAT, naming the parameter
/// and the value the request sent, and nothing is reported — even when the request also names
/// an id the account does hold.
#[tokio::test]
async fn an_id_ec2_could_not_have_issued_is_refused_before_any_lookup() {
    let (router, engine) = emulator();
    let held = seed_task(
        &engine,
        Seed {
            case: "a-held-import",
            kind: Kind::Instance,
            disks: 1,
            tags: 1,
        },
    );
    for (case, conversion_task_id) in malformed_id_rows() {
        assert!(
            !conversion_task_id_is_well_formed(&conversion_task_id),
            "case {case:?}: {conversion_task_id} must not read as a minted conversion task id"
        );
        for (shape, ids) in [
            ("alone", vec![conversion_task_id.as_str()]),
            ("beside a held id", vec![held.as_str(), conversion_task_id.as_str()]),
        ] {
            let case = format!("{case} ({shape})");
            let (status, body) = describe_ids(&router, &ids).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside the minted form is refused, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_CONVERSION_TASK_ID,
                "case {case:?}: expected {INVALID_CONVERSION_TASK_ID}, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                invalid_parameter_format_message(
                    CONVERSION_TASK_ID_PARAMETER,
                    &conversion_task_id
                ),
                "case {case:?}: the refusal must name the parameter and the value the \
                 request sent, got {message:?}"
            );
            assert!(
                member(&body, "conversionTasks").is_none(),
                "case {case:?}: a refused request reports no task, got {body}"
            );
        }
    }
}

/// Rule 3: an id EC2 could have issued that the account holds no task for selects NO ROW:
/// the operation reports a list of the account's conversion tasks, which an account holding
/// none of them reports empty, so such an id is not an error — the ids of the account's own
/// tasks beside it are still reported.
#[tokio::test]
async fn an_id_the_account_holds_no_task_for_selects_no_row() {
    let (router, engine) = emulator();
    let held = seed_task(
        &engine,
        Seed {
            case: "a-held-import",
            kind: Kind::Volume,
            disks: 0,
            tags: 1,
        },
    );
    for (case, conversion_task_id) in unheld_id_rows() {
        assert!(
            conversion_task_id_is_well_formed(&conversion_task_id),
            "case {case:?}: {conversion_task_id} must read as a minted conversion task id"
        );
        assert_ne!(
            conversion_task_id, held,
            "case {case:?}: the unheld id must not be the one the account holds"
        );
        // `(how the request names it, the ids it names, the ids the report must carry)`.
        let rows: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
            ("alone", vec![conversion_task_id.as_str()], Vec::new()),
            (
                "beside a held id",
                vec![held.as_str(), conversion_task_id.as_str()],
                vec![held.clone()],
            ),
        ];
        for (shape, ids, expected) in rows {
            let case = format!("{case} ({shape})");
            let (status, body) = describe_ids(&router, &ids).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an id no task answers to selects no row rather than failing, \
                 got {status} with body {body}"
            );
            let actual = reported_ids(&case, &body);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
            );
        }
    }
    // The account's own task is still reported after every unresolved id: such a describe
    // wrote nothing.
    assert_eq!(
        described_ids(&router, "after the unresolved ids", &[]).await,
        vec![held],
        "a describe naming an unheld id must leave the account's tasks reportable"
    );
}

/// Rule 3: a request naming more ids than one EC2 request may carry is refused
/// `InvalidParameterValue` before any of them is looked at, while a request AT that width is
/// served — the bound, not one count, is what is asserted. The width is the crate's own
/// documented one (`MAX_RESOURCE_IDS`, "Up to 1000 resource IDs"), so the two cannot drift.
#[tokio::test]
async fn a_request_naming_more_ids_than_ec2_carries_is_refused() {
    let (router, engine) = emulator();
    let held = seed_task(
        &engine,
        Seed {
            case: "a-held-import",
            kind: Kind::Instance,
            disks: 1,
            tags: 1,
        },
    );
    // Ids EC2 could have issued, each distinct and none of them one this store mints (its
    // own counter starts low, so a suffix in the top of the 17-digit space is not a minted
    // id), so the only thing under test is HOW MANY the request names.
    let minted: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("import-i-f{index:016x}"))
        .collect();
    // `(case, how many ids the request names, whether it is served)`.
    let rows = [
        ("one id past the width", MAX_RESOURCE_IDS + 1, false),
        ("at the width", MAX_RESOURCE_IDS, true),
        ("one id short of the width", MAX_RESOURCE_IDS - 1, true),
    ];
    for (case, count, served) in rows {
        let ids: Vec<&str> = minted
            .iter()
            .take(count)
            .map(String::as_str)
            .collect();
        let (status, body) = describe_ids(&router, &ids).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: a request naming {count} ids is served, got {status}"
            );
            // Every id named is one EC2 could have issued and none names a task of this
            // account, so the report is empty.
            assert_eq!(
                reported_ids(case, &body),
                Vec::<String>::new(),
                "case {case:?}: none of the {count} ids names a task of the account"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a request naming {count} ids is refused, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&MAX_RESOURCE_IDS.to_string()),
                "case {case:?}: the refusal must name the width, got {message:?}"
            );
        }
    }
    // The account's own task is reported throughout: none of these requests wrote anything.
    assert_eq!(
        described_ids(&router, "after the width rows", &[]).await,
        vec![held],
        "a refused describe must leave the account's tasks reportable"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: describing changes nothing a later caller sees
// ---------------------------------------------------------------------------

/// Rule 4: two identical describes answer identically and polling a task never moves it; the
/// only move a report shows is the one the CLOCK already owed — the end of a cancellation.
#[tokio::test]
async fn describing_reports_the_clocks_moves_and_causes_none() {
    let (router, engine) = emulator();
    let polled = seed_task(
        &engine,
        Seed {
            case: "the-polled-import",
            kind: Kind::Instance,
            disks: 2,
            tags: 1,
        },
    );
    let cancelled = seed_task(
        &engine,
        Seed {
            case: "the-cancelled-import",
            kind: Kind::Volume,
            disks: 0,
            tags: 1,
        },
    );

    // Two identical calls answer identically, and an `active` task stays `active` however
    // often it is read.
    let (first_status, first) = describe_ids(&router, &[&polled]).await;
    assert_eq!(
        first_status, 200,
        "the first describe must succeed, got {first_status} with body {first}"
    );
    for round in 1..=3 {
        let case = format!("poll {round}");
        let (status, body) = describe_ids(&router, &[&polled]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status} with body {body}"
        );
        let parsed = document(&case, &body);
        let items = reported_items(&case, &parsed, &body);
        assert_eq!(
            at(items[0], "state").as_deref(),
            Some(CONVERSION_TASK_STATE_ACTIVE),
            "case {case:?}: polling must not move the task, got {body}"
        );
        // The requestId is per call, so the bodies are compared with it removed.
        assert_eq!(
            without_request_id(&body),
            without_request_id(&first),
            "case {case:?}: two identical describes must answer identically"
        );
    }

    // A cancel is what moves a task, and the clock finishes it: the report shows
    // `cancelling` straight after the call and `cancelled` once the span has passed —
    // neither caused by the read.
    let (cancel_status, cancel_body) = call(
        &router,
        REGION,
        "CancelConversionTask",
        &[p("ConversionTaskId", cancelled.clone())],
    )
    .await;
    assert_eq!(
        cancel_status, 200,
        "the cancel must be admitted, got {cancel_status} with body {cancel_body}"
    );
    let case = "straight after the cancel";
    let (status, body) = describe_ids(&router, &[&cancelled]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    assert_eq!(
        at(reported_items(case, &parsed, &body)[0], "state").as_deref(),
        Some(CONVERSION_TASK_STATE_CANCELLING),
        "case {case:?}: a cancel leaves the task in the non-terminal cancelling state, \
         got {body}"
    );

    past_the_transition().await;
    let case = "once the clock owes the end of the cancellation";
    let (status, body) = describe_ids(&router, &[&cancelled]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    assert_eq!(
        at(reported_items(case, &parsed, &body)[0], "state").as_deref(),
        Some(CONVERSION_TASK_STATE_CANCELLED),
        "case {case:?}: the clock finishes the cancellation, got {body}"
    );

    // The task the cancel did not name is untouched, and both records are still reportable:
    // a state is only reportable while the task it belongs to is there.
    let case = "the task the cancel did not name";
    let (status, body) = describe_ids(&router, &[&polled]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    assert_eq!(
        at(reported_items(case, &parsed, &body)[0], "state").as_deref(),
        Some(CONVERSION_TASK_STATE_ACTIVE),
        "case {case:?}: a cancel of another task must not move it, got {body}"
    );
    let mut both = vec![polled, cancelled];
    both.sort();
    assert_eq!(
        described_ids(&router, "every task the account holds", &[]).await,
        both,
        "every stored task stays reportable"
    );
}

/// The body with its per-call `requestId` value removed, so two answers can be compared for
/// everything the OPERATION decided. The id is read with the harness's shared element reader
/// rather than sliced out by hand.
fn without_request_id(body: &str) -> String {
    match super::ec2_query::element(body, "requestId") {
        Some(request_id) => body.replace(&request_id, ""),
        None => body.to_string(),
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a dry run answers what the request would have done
// ---------------------------------------------------------------------------

/// What a dry run of one request must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DryRun {
    /// The request would have worked: `DryRunOperation`, HTTP 412.
    WouldHaveWorked,
    /// The request would have been refused, with this code, HTTP 400.
    WouldHaveBeenRefused(&'static str),
}

/// Rule 5: a dry run answers what the request WOULD have done — `DryRunOperation` for one
/// that would have worked, and the refusal it would have been refused with otherwise.
#[tokio::test]
async fn a_dry_run_answers_what_the_request_would_have_done() {
    let (router, engine) = emulator();
    let held = seed_task(
        &engine,
        Seed {
            case: "a-held-import",
            kind: Kind::Instance,
            disks: 1,
            tags: 1,
        },
    );
    let rows: Vec<(&str, Vec<String>, DryRun)> = vec![
        ("a dry run naming no id", Vec::new(), DryRun::WouldHaveWorked),
        (
            "a dry run naming a held task",
            vec![held.clone()],
            DryRun::WouldHaveWorked,
        ),
        (
            "a dry run naming a value outside the minted form",
            vec!["import-i-0123abc".to_string()],
            DryRun::WouldHaveBeenRefused(INVALID_CONVERSION_TASK_ID),
        ),
        (
            "a dry run naming an id no task answers to",
            vec!["import-vol-0123456789abcdef0".to_string()],
            DryRun::WouldHaveWorked,
        ),
    ];
    for (case, named, expectation) in rows {
        let ids: Vec<&str> = named.iter().map(String::as_str).collect();
        let mut params = indexed("ConversionTaskId", &ids);
        params.push(p("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        let (expected_status, expected_code) = match expectation {
            DryRun::WouldHaveWorked => (412, DRY_RUN_OPERATION),
            DryRun::WouldHaveBeenRefused(code) => (400, code),
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            member(&body, "conversionTasks").is_none(),
            "case {case:?}: a dry run reports no task, got {body}"
        );
    }
    // The dry runs changed nothing: the account still holds exactly the one seeded task.
    assert_eq!(
        described_ids(&router, "after the dry runs", &[]).await,
        vec![held],
        "a dry run must leave the account's tasks as they were"
    );
}
