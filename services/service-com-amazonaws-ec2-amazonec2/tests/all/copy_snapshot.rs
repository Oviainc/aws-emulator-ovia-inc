// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CopySnapshot`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A COPY IS A NEW SNAPSHOT COMPUTED FROM THE STORED SOURCE: its own id and start time,
//!    the SOURCE's volume size ("Creates an exact copy of an Amazon EBS snapshot"), the
//!    encryption state the source and the request add up to, the REQUEST's own description
//!    and tags, a `TransferType` saying which kind of copy operation made it, and an
//!    arbitrary source volume id that is not the source's — read back through
//!    `DescribeSnapshots`, which reports it exactly as it reports a created snapshot;
//! 2. `SourceRegion` HAS TO NAME A REGION of the account's partition, and is VALIDATED ONLY:
//!    a stored snapshot carries no Region of its own, so every accepted Region copies the
//!    same source, and a value that names no Region is refused;
//! 3. A SOURCE SNAPSHOT THE COPY CANNOT RESOLVE IS REFUSED WITH ONE ANSWER, whether the
//!    value is not a snapshot id at all or is well formed and names no snapshot;
//! 4. A REQUEST THAT CANNOT BE SERVED IS REFUSED WITH EC2'S OWN CODE AND NOTHING IS STORED:
//!    an encryption state the member cannot carry, a KMS key for a copy that is not
//!    encrypted, a destination Outpost or Local Zone this emulator has none of, a completion
//!    duration that is not a whole increment or is outside the modelled range (refused by
//!    the range itself, one step before the handler), a tag specification naming another
//!    resource type, and a `DryRun` that reports whether the call would have worked;
//! 5. THE CLOCK — not a caller — MOVES THE COPY FROM `pending` TO `completed`.
//!
//! Each table row is a distinct input along what the rule generalises over — the source's
//! size and encryption state, which encryption parameter the request carries, how many tags,
//! which completion duration, which Region form, which refusal — so no row is the
//! specification. The source snapshots the rows copy are made through `CreateSnapshots`, the
//! other writer of this crate's snapshot store, and every copy is read back over the wire
//! through `DescribeSnapshots`.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    MAX_KMS_KEY_ID_CHARS, SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_PROGRESS_PENDING,
    SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STATE_PENDING, SNAPSHOT_STORAGE_TIER_STANDARD,
    SNAPSHOT_TRANSFER_TYPE_STANDARD, SNAPSHOT_TRANSFER_TYPE_TIME_BASED, regions_in_partition_of,
};

use super::ec2_query::{element, elements, error_code, error_message};
use super::fpga_image_tasks_and_volume_io::attach_in_store;
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the refusal table: `(case, the parameters beside the source, the error code,
/// the whole refusal sentence)`.
type RefusalRow = (&'static str, Params, &'static str, String);

/// The device a seeded source volume is attached at: the snapshot set of an instance covers
/// the volumes it holds an attachment of, and one data volume is all a seed needs.
const DATA_DEVICE: &str = "/dev/sdf";

/// The AMI a launch here names. A launch resolves no image, and this suite is about the
/// SNAPSHOTS rather than the AMI, so an id the account sees no image for is enough: the
/// launch still gives the instance the one root volume every launch creates, which the
/// seeding excludes from the snapshot set.
const ANY_AMI: &str = "ami-0123456789abcdef0";

/// A snapshot id of the long form this service mints that no snapshot of the account
/// carries.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// The sentence EC2 answers a `CopySnapshot` whose source snapshot it cannot copy from. It
/// names the member and NOT the value, which is how AWS words this refusal — the recorded
/// call behind `issues/.../20261023T102725Z-ec2-copysnapshot-stub.md` carries exactly this
/// text.
const UNRESOLVABLE_SOURCE_MESSAGE: &str = "The specified snapshotId is invalid";

/// A KMS key identifier of the `Key ID` form the member documents
/// (`CopySnapshotRequest$KmsKeyId`: "Key ID. For example,
/// 1234abcd-12ab-34cd-56ef-1234567890ab").
const A_KMS_KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Launch one instance and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            p("ImageId", ANY_AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: launching an instance: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reports the instance it minted"))
}

/// Create one volume of `size` GiB in the Region's first zone, encrypted or not, and answer
/// its id.
async fn create_volume(
    router: &MultiServiceRouter,
    case: &str,
    size: i32,
    encrypted: bool,
) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("Size", size.to_string()),
    ];
    if encrypted {
        params.push(p("Encrypted", "true"));
    }
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {size} GiB volume: expected HTTP 200, got {status} with \
         body {body}"
    );
    element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports the volume it minted"))
}

/// The snapshot a copy is made of: the stored snapshot's id and the id of the volume it was
/// taken of, so a row can assert the copy's own source volume id is NOT that one.
#[derive(Debug, Clone)]
struct Source {
    snapshot_id: String,
    volume_id: String,
}

/// Store one source snapshot of a volume of `size` GiB, encrypted or not, through
/// `CreateSnapshots` — the crate's other writer of the snapshot store.
///
/// The seed gets its OWN instance so the set is exactly one snapshot: an instance holds the
/// root volume its launch created, and excluding that boot volume leaves only the volume
/// this seed attached.
async fn seed_source(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    size: i32,
    encrypted: bool,
) -> Source {
    let instance_id = launch(router, case).await;
    let volume_id = create_volume(router, case, size, encrypted).await;
    attach_in_store(engine, &volume_id, &instance_id, DATA_DEVICE);

    let (status, body) = op(
        router,
        "CreateSnapshots",
        &[
            p("InstanceSpecification.InstanceId", instance_id),
            p("InstanceSpecification.ExcludeBootVolume", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding a source snapshot: expected HTTP 200, got {status} with body \
         {body}"
    );
    let minted = elements(&body, "snapshotId");
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: seeding makes exactly one snapshot, got {minted:?}"
    );
    Source {
        snapshot_id: minted[0].clone(),
        volume_id,
    }
}

/// One snapshot as `DescribeSnapshots` reported it: its direct members as the text the
/// response carried for each, and its tags as the pairs they were reported in.
#[derive(Debug, Clone, Default)]
struct ReportedSnapshot {
    members: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl ReportedSnapshot {
    /// The text of the member `name`, or the empty string when the response reported none —
    /// an element present and empty reads as the empty string, which is what
    /// [`SNAPSHOT_PROGRESS_PENDING`] is.
    fn member(&self, name: &str) -> &str {
        self.members.get(name).map_or("", String::as_str)
    }

    /// This snapshot's tags as `(key, value)` pairs in key order, so a row compares against
    /// a set rather than against one reporting order.
    fn sorted_tags(&self) -> Vec<(String, String)> {
        let mut tags = self.tags.clone();
        tags.sort();
        tags
    }
}

/// The ONE snapshot `DescribeSnapshots` reports for `snapshot_id`, read back over the wire.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
) -> ReportedSnapshot {
    let (status, body) = op(
        router,
        "DescribeSnapshots",
        &[p("SnapshotId.1", snapshot_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSnapshots for {snapshot_id} must succeed, got {status} with \
         body {body}"
    );
    let document = super::wire::document(case, &body);
    let items = super::wire::set_items(document.root_element(), "snapshotSet");
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: exactly one snapshot is reported for {snapshot_id}, got {body}"
    );
    let mut reported = ReportedSnapshot::default();
    for member in items[0].children().filter(roxmltree::Node::is_element) {
        if member.tag_name().name() == "tagSet" {
            for tag in member.children().filter(roxmltree::Node::is_element) {
                reported.tags.push((
                    super::wire::child_text(tag, "key").unwrap_or_default(),
                    super::wire::child_text(tag, "value").unwrap_or_default(),
                ));
            }
            continue;
        }
        reported.members.insert(
            member.tag_name().name().to_string(),
            member.text().unwrap_or_default().to_string(),
        );
    }
    reported
}

/// Every snapshot id the store holds, in store order.
fn stored_snapshot_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<ec2::ComAmazonawsEc2Amazonec2State>();
    let guard = store.lock();
    guard.snapshots.keys().cloned().collect()
}

/// The `TagSpecification` form fields for one `snapshot` specification carrying `tags`.
fn tag_specification(tags: &[(&str, &str)]) -> Params {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![p("TagSpecification.1.ResourceType", "snapshot")];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

// ---------------------------------------------------------------------------
// RULE 1: a copy is a new snapshot computed from the stored source
// ---------------------------------------------------------------------------

/// One row of the copy table.
struct CopyRow {
    case: &'static str,
    /// The source volume's size in GiB, which the copy has to report as its own.
    source_size: i32,
    /// Whether the source volume — and so the source snapshot — is encrypted.
    source_encrypted: bool,
    /// The copy's own parameters beside `SourceSnapshotId` and `SourceRegion`.
    extra: fn() -> Params,
    /// The encryption state the copy must end up in.
    encrypted: bool,
    /// The `transferType` the copy must report.
    transfer_type: &'static str,
    /// The `completionDurationMinutes` the copy must report, empty for none.
    completion_duration: &'static str,
    /// The `description` the copy must report, empty for none.
    description: &'static str,
    /// The tags the copy must carry, in key order.
    tags: &'static [(&'static str, &'static str)],
}

/// RULE 1: the copy is a snapshot in its own right, every member of it read off the stored
/// SOURCE (its volume size, its encryption state) or off the REQUEST (the description, the
/// tags, the completion duration) — never off the source's own description or tags, which a
/// copy does not inherit.
///
/// The rows differ along what the rule generalises over: the source's size, whether the
/// source is encrypted, which encryption parameter the request carries, how many tags it
/// asks for, and which completion duration it names — including both published bounds of the
/// modelled range (1..2880), each a whole 15 minute increment.
#[tokio::test]
async fn a_copy_is_a_new_snapshot_computed_from_the_stored_source() {
    let rows: [CopyRow; 7] = [
        CopyRow {
            case: "an unencrypted source copied with nothing else asked for",
            source_size: 8,
            source_encrypted: false,
            extra: Vec::new,
            encrypted: false,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_STANDARD,
            completion_duration: "",
            description: "",
            tags: &[],
        },
        CopyRow {
            case: "a bigger source copied with a description",
            source_size: 100,
            source_encrypted: false,
            extra: || vec![p("Description", "the quarterly backup")],
            encrypted: false,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_STANDARD,
            completion_duration: "",
            description: "the quarterly backup",
            tags: &[],
        },
        CopyRow {
            case: "an encrypted source is copied encrypted without asking",
            source_size: 16,
            source_encrypted: true,
            extra: Vec::new,
            encrypted: true,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_STANDARD,
            completion_duration: "",
            description: "",
            tags: &[],
        },
        CopyRow {
            case: "Encrypted raises an unencrypted source's copy",
            source_size: 32,
            source_encrypted: false,
            extra: || vec![p("Encrypted", "true")],
            encrypted: true,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_STANDARD,
            completion_duration: "",
            description: "",
            tags: &[],
        },
        CopyRow {
            case: "one tag the request asks for",
            source_size: 1,
            source_encrypted: false,
            extra: || tag_specification(&[("Name", "copy")]),
            encrypted: false,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_STANDARD,
            completion_duration: "",
            description: "",
            tags: &[("Name", "copy")],
        },
        CopyRow {
            case: "a time-based copy at the smallest completion duration",
            source_size: 4,
            source_encrypted: false,
            extra: || {
                [
                    vec![p("CompletionDurationMinutes", "15")],
                    tag_specification(&[("owner", "backups"), ("tier", "cold")]),
                ]
                .concat()
            },
            encrypted: false,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_TIME_BASED,
            completion_duration: "15",
            description: "",
            tags: &[("owner", "backups"), ("tier", "cold")],
        },
        CopyRow {
            case: "a time-based copy at the largest completion duration",
            source_size: 64,
            source_encrypted: true,
            extra: || {
                vec![
                    p("CompletionDurationMinutes", "2880"),
                    p("KmsKeyId", A_KMS_KEY_ID),
                ]
            },
            encrypted: true,
            transfer_type: SNAPSHOT_TRANSFER_TYPE_TIME_BASED,
            completion_duration: "2880",
            description: "",
            tags: &[],
        },
    ];

    for row in rows {
        let (router, engine) = emulator();
        let source = seed_source(
            &router,
            &engine,
            row.case,
            row.source_size,
            row.source_encrypted,
        )
        .await;
        let before = described(&router, row.case, &source.snapshot_id).await;

        let params = [
            vec![
                p("SourceSnapshotId", &source.snapshot_id),
                p("SourceRegion", REGION),
            ],
            (row.extra)(),
        ]
        .concat();
        let (status, body) = op(&router, "CopySnapshot", &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            row.case
        );
        let copy_id = element(&body, "snapshotId").unwrap_or_else(|| {
            panic!(
                "case {:?}: CopySnapshot reports the snapshot it minted, got {body}",
                row.case
            )
        });
        assert!(
            super::wire::is_long_id("snap", &copy_id) && copy_id != source.snapshot_id,
            "case {:?}: the copy is a new snapshot with an id of its own, got {copy_id:?}",
            row.case
        );
        // "Any tags applied to the new snapshot" (`CopySnapshotResult$Tags`): the answer
        // carries the tags the request asked for, and a copy the request tagged with none
        // carries no tag set at all.
        let mut answered_tags: Vec<(String, String)> = elements(&body, "key")
            .into_iter()
            .zip(elements(&body, "value"))
            .collect();
        answered_tags.sort();
        let expected_tags: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = row
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            pairs.sort();
            pairs
        };
        assert_eq!(
            answered_tags, expected_tags,
            "case {:?}: the answer reports the tags the copy carries, got {body}",
            row.case
        );

        // Read the copy back over the wire: a later caller sees exactly what the copy was
        // stored as.
        let copy = described(&router, row.case, &copy_id).await;
        let checks: [(&str, String); 6] = [
            // "Creates an exact copy of an Amazon EBS snapshot": the copy holds the source's
            // data, so its volume size is the source's rather than anything the request said.
            ("volumeSize", row.source_size.to_string()),
            ("encrypted", row.encrypted.to_string()),
            ("transferType", row.transfer_type.to_string()),
            (
                "completionDurationMinutes",
                row.completion_duration.to_string(),
            ),
            ("description", row.description.to_string()),
            (
                "storageTier",
                SNAPSHOT_STORAGE_TIER_STANDARD.to_string(),
            ),
        ];
        for (member, expected) in checks {
            assert_eq!(
                copy.member(member),
                expected,
                "case {:?}: expected {member} {expected:?}, got {:?}",
                row.case,
                copy.member(member)
            );
        }
        assert_eq!(
            copy.sorted_tags(),
            expected_tags,
            "case {:?}: the stored copy carries the request's tags, got {:?}",
            row.case,
            copy.sorted_tags()
        );
        // "Snapshots copies have an arbitrary source volume ID. Do not use this volume ID
        // for any purpose" (`CopySnapshot`): the copy reports a volume id of its own, not
        // the one the source was taken of.
        assert!(
            super::wire::is_long_id("vol", copy.member("volumeId"))
                && copy.member("volumeId") != source.volume_id,
            "case {:?}: the copy reports an arbitrary source volume id, got {:?} for a source \
             taken of {:?}",
            row.case,
            copy.member("volumeId"),
            source.volume_id
        );
        assert_eq!(
            copy.member("ownerId"),
            engine::emulator_account_id(),
            "case {:?}: the copy belongs to the account that asked for it, got {:?}",
            row.case,
            copy.member("ownerId")
        );

        // RULE 5: the copy is handed back `pending` with no progress, and the clock — not a
        // caller — moves it to `completed`.
        assert_eq!(
            (copy.member("status"), copy.member("progress")),
            (SNAPSHOT_STATE_PENDING, SNAPSHOT_PROGRESS_PENDING),
            "case {:?}: a fresh copy is pending with no progress, got {:?}",
            row.case,
            copy.members
        );
        past_the_transition().await;
        let completed = described(&router, row.case, &copy_id).await;
        assert_eq!(
            (
                completed.member("status"),
                completed.member("progress")
            ),
            (SNAPSHOT_STATE_COMPLETED, SNAPSHOT_PROGRESS_COMPLETED),
            "case {:?}: the clock completes the copy, got {:?}",
            row.case,
            completed.members
        );

        // The SOURCE is left exactly as it was, apart from the completion the clock owed it:
        // a copy reads its source and writes a new snapshot beside it.
        let source_now = described(&router, row.case, &source.snapshot_id).await;
        for member in ["volumeId", "volumeSize", "encrypted", "description"] {
            assert_eq!(
                source_now.member(member),
                before.member(member),
                "case {:?}: the copy must not change the source's {member}, got {:?}",
                row.case,
                source_now.member(member)
            );
        }
        assert_eq!(
            stored_snapshot_ids(&engine),
            {
                let mut both = vec![source.snapshot_id.clone(), copy_id.clone()];
                both.sort();
                both
            },
            "case {:?}: the store holds the source and its one copy",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: SourceRegion has to name a Region, and names no store of its own
// ---------------------------------------------------------------------------

/// RULE 2: a `SourceRegion` naming a Region of the account's partition is copied from — all
/// of them resolving the SAME stored source, because a stored snapshot carries no Region of
/// its own — and one that names no Region is refused, whichever way it fails to be a Region
/// code.
#[tokio::test]
async fn a_copy_source_region_has_to_name_a_region() {
    let (router, engine) = emulator();
    let source = seed_source(&router, &engine, "the source", 8, false).await;

    // The accepted rows are Regions of the account's partition, which the catalog
    // `ec2::state::regions_in_partition_of` derives, plus the Region the call is signed for;
    // the asserts below hold each row to that catalog rather than to a hand-written list.
    let catalog = regions_in_partition_of(REGION);
    let accepted: [(&str, &str); 4] = [
        ("the Region the call is signed for", REGION),
        ("another Region of the partition", "us-east-1"),
        ("a Region in another geography", "eu-central-1"),
        ("a Region whose code has three words", "ap-northeast-3"),
    ];
    let mut copies = Vec::new();
    for (case, source_region) in accepted {
        assert!(
            catalog.contains(&source_region),
            "case {case:?}: {source_region} has to be in the partition's Region catalog, got \
             {catalog:?}"
        );
        let (status, body) = op(
            &router,
            "CopySnapshot",
            &[
                p("SourceSnapshotId", &source.snapshot_id),
                p("SourceRegion", source_region),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let copy_id = element(&body, "snapshotId")
            .unwrap_or_else(|| panic!("case {case:?}: the copy reports its id, got {body}"));
        // Every accepted Region copied the SAME source, so every copy reports the source's
        // size: `SourceRegion` is validated, never resolved against.
        let copy = described(&router, case, &copy_id).await;
        assert_eq!(
            copy.member("volumeSize"),
            "8",
            "case {case:?}: the copy holds the one stored source's data, got {:?}",
            copy.member("volumeSize")
        );
        copies.push(copy_id);
    }

    let refused: [(&str, &str); 6] = [
        ("no Region at all", ""),
        ("a Region code with no number", "us-east"),
        ("words that are not a Region code", "not-a-region"),
        ("an uppercase Region code", "US-WEST-2"),
        ("an Availability Zone rather than a Region", "us-west-2a"),
        ("a Region of another partition", "cn-north-1"),
    ];
    for (case, source_region) in refused {
        assert!(
            !catalog.contains(&source_region),
            "case {case:?}: {source_region} must not be in the Region catalog"
        );
        let (status, body) = op(
            &router,
            "CopySnapshot",
            &[
                p("SourceSnapshotId", &source.snapshot_id),
                p("SourceRegion", source_region),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Value ({source_region}) for parameter sourceRegion is invalid. \
                 This is not a valid Region name."
            ),
            "case {case:?}: the refusal names the parameter and the value, got {:?}",
            error_message(&body)
        );
    }

    // Four copies were accepted, one per accepted Region, and no refused row copied
    // anything — so the store holds those four beside the source.
    let mut expected = copies;
    expected.push(source.snapshot_id);
    expected.sort();
    assert_eq!(
        stored_snapshot_ids(&engine),
        expected,
        "only the accepted rows copy anything, got {:?}",
        stored_snapshot_ids(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: one answer for a source this copy cannot resolve
// ---------------------------------------------------------------------------

/// RULE 3: every `SourceSnapshotId` this copy cannot resolve gets the SAME answer — 400
/// `InvalidParameterValue` naming the member and not the value — whether the value is not a
/// snapshot id at all or is a well-formed id the account holds no snapshot for.
///
/// The rows span both id forms EC2 has issued (8 and 17 hexadecimal digits), values outside
/// that form, and the id of another kind of resource.
#[tokio::test]
async fn a_source_snapshot_this_copy_cannot_resolve_gets_one_answer() {
    let (router, engine) = emulator();
    // One stored snapshot the account DOES hold, so the refusals below are about the id the
    // request named rather than about an empty store.
    let source = seed_source(&router, &engine, "the source", 8, false).await;

    let rows: [(&str, String); 7] = [
        ("a well-formed long id naming no snapshot", UNKNOWN_SNAPSHOT_ID.to_string()),
        ("a well-formed short id naming no snapshot", "snap-1a2b3c4d".to_string()),
        ("the null id no mint can issue", "snap-00000000000000000".to_string()),
        ("a value that is not a snapshot id at all", "not-a-snapshot".to_string()),
        ("the prefix with no digits", "snap-".to_string()),
        ("an uppercase id", UNKNOWN_SNAPSHOT_ID.to_uppercase()),
        ("the id of another kind of resource", source.volume_id.clone()),
    ];
    for (case, source_snapshot_id) in rows {
        let (status, body) = op(
            &router,
            "CopySnapshot",
            &[
                p("SourceSnapshotId", source_snapshot_id.clone()),
                p("SourceRegion", REGION),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            UNRESOLVABLE_SOURCE_MESSAGE,
            "case {case:?}: expected the one unresolvable-source sentence, got {:?}",
            error_message(&body)
        );
        assert!(
            !error_message(&body).contains(&source_snapshot_id),
            "case {case:?}: the sentence names the member and not the value, got {:?}",
            error_message(&body)
        );
    }

    assert_eq!(
        stored_snapshot_ids(&engine),
        vec![source.snapshot_id],
        "no refused row stores anything, got {:?}",
        stored_snapshot_ids(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 4: a request that cannot be served is refused and stores nothing
// ---------------------------------------------------------------------------

/// RULE 4: every other request this operation cannot serve is refused with EC2's own code
/// and leaves the store exactly as it was.
///
/// The rows differ in WHICH rule refuses them: the one value `Encrypted` cannot carry, a KMS
/// key named for a copy that is not encrypted, a key identifier past the bound AWS KMS
/// publishes, each destination this emulator has none of, a completion duration that is not
/// a whole increment, the modelled range's own bounds one step outside (refused by the range
/// before the handler runs), and a tag specification naming a resource this call does not
/// create.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_ec2s_own_code() {
    let (router, engine) = emulator();
    let source = seed_source(&router, &engine, "the source", 8, false).await;
    let outpost_arn = "arn:aws:outposts:us-west-2:123456789012:outpost/op-0123456789abcdef0";

    let rows: [RefusalRow; 8] = [
        (
            "the one value Encrypted cannot carry",
            vec![p("Encrypted", "false")],
            "InvalidParameterValue",
            "Invalid value 'false' for encrypted. You cannot set Encrypted to false."
                .to_string(),
        ),
        (
            "a KMS key for a copy that is not encrypted",
            vec![p("KmsKeyId", A_KMS_KEY_ID)],
            "InvalidParameterValue",
            "kmsKeyId may be specified only when the snapshot copy is encrypted".to_string(),
        ),
        (
            "a KMS key identifier past the published bound",
            vec![
                p("Encrypted", "true"),
                p("KmsKeyId", "k".repeat(MAX_KMS_KEY_ID_CHARS + 1)),
            ],
            "InvalidParameterValue",
            format!("kmsKeyId accepts at most {MAX_KMS_KEY_ID_CHARS} characters"),
        ),
        (
            "a destination Outpost this emulator has none of",
            vec![p("DestinationOutpostArn", outpost_arn)],
            "InvalidParameterValue",
            format!(
                "Invalid value '{outpost_arn}' for destinationOutpostArn. \
                 The snapshot copy must be created on an Outpost of this Region, \
                 and this Region has no Outpost"
            ),
        ),
        (
            "a destination Local Zone this emulator has none of",
            vec![p("DestinationAvailabilityZone", "us-west-2-lax-1a")],
            "InvalidParameterValue",
            "Invalid value 'us-west-2-lax-1a' for destinationAvailabilityZone. \
             A snapshot copy is placed in a Local Zone only, \
             and this Region has no Local Zone"
                .to_string(),
        ),
        (
            "a completion duration that is not a whole increment",
            vec![p("CompletionDurationMinutes", "20")],
            "InvalidParameterValue",
            "Invalid value '20' for completionDurationMinutes. \
             A completion duration is specified in 15 minute increments"
                .to_string(),
        ),
        (
            "a completion duration one increment past the modelled maximum",
            vec![p("CompletionDurationMinutes", "2895")],
            "ValidationError",
            String::new(),
        ),
        (
            "a tag specification naming a resource this call does not create",
            vec![p("TagSpecification.1.ResourceType", "volume")],
            "InvalidParameterValue",
            "'volume' is not a valid resource type for tagging on CopySnapshot".to_string(),
        ),
    ];

    for (case, extra, expected_code, expected_message) in rows {
        let params = [
            vec![
                p("SourceSnapshotId", &source.snapshot_id),
                p("SourceRegion", REGION),
            ],
            extra,
        ]
        .concat();
        let (status, body) = op(&router, "CopySnapshot", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // The rows refused one step before the handler carry the boundary validator's own
        // wording, which is not this operation's to word, so only the code is asserted for
        // them.
        if !expected_message.is_empty() {
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the refusal's whole sentence, got {:?}",
                error_message(&body)
            );
        }
    }

    // "Checks whether you have the required permissions for the action, without actually
    // making the request" (`CopySnapshotRequest$DryRun`): a request that WOULD have
    // succeeded is reported as such, and copies nothing.
    let (status, body) = op(
        &router,
        "CopySnapshot",
        &[
            p("SourceSnapshotId", &source.snapshot_id),
            p("SourceRegion", REGION),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "a DryRun that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a DryRun that would have succeeded: expected DryRunOperation, got {body}"
    );

    assert_eq!(
        stored_snapshot_ids(&engine),
        vec![source.snapshot_id],
        "no refused row and no DryRun stores anything, got {:?}",
        stored_snapshot_ids(&engine)
    );
}
