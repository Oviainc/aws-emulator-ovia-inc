// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateSnapshots`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. ONE SNAPSHOT PER EBS VOLUME THE NAMED INSTANCE HOLDS AN ATTACHMENT OF, with every
//!    member of each snapshot read off THAT volume (its id, its size, its encryption state)
//!    or off the request (the description, the tags) — never off another instance's volume
//!    and never off an unattached one;
//! 2. THE TWO EXCLUSIONS NARROW THAT SET: `ExcludeBootVolume` drops the attachment at the
//!    instance's root device, `ExcludeDataVolumeIds` drops the data volumes it names, and
//!    naming the ROOT volume there is refused rather than honoured;
//! 3. A SNAPSHOT IS HANDED BACK `pending` WITH NO PROGRESS, and the CLOCK — not a caller —
//!    moves it to `completed`;
//! 4. A REQUEST THAT CANNOT BE SERVED IS REFUSED WITH EC2'S OWN CODE AND NOTHING IS STORED:
//!    an instance id that resolves to no instance, a location or an Outpost this emulator
//!    has none of, a value outside a modelled enum's catalog (refused by the enum itself,
//!    one step before the handler), and a `DryRun` that reports whether the call would have
//!    worked;
//! 5. EVERY LAUNCH GIVES ITS INSTANCE ONE EBS ROOT VOLUME for the set to cover — off the
//!    AMI's root block device mapping where the account can see that AMI, and off the
//!    crate's default EBS-backed Linux image where it cannot.
//!
//! Each table row is a distinct input along what the rule generalises over — how many
//! volumes the instance holds, each one's size and encryption state, which exclusion names
//! it, which tag source, which refusal — so no row is the specification.
//!
//! The snapshots a call stored are read back out of the engine's own state here, which is
//! what lets a row assert the stored record member by member; what `DescribeSnapshots`
//! REPORTS of them over the wire is the subject of `describe_snapshots.rs`. The VOLUMES the
//! launch creates are read back over the wire through `DescribeVolumes`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LINUX_ROOT_DEVICE_NAME, LINUX_ROOT_VOLUME_SIZE,
    MAX_EXCLUDED_DATA_VOLUME_IDS, SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_PROGRESS_PENDING,
    SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STATE_PENDING, SNAPSHOT_STORAGE_TIER_STANDARD,
    SnapshotView, WINDOWS_ROOT_DEVICE_NAME,
};

use super::ec2_query::{element, elements, error_code, error_message};
use super::fpga_image_tasks_and_volume_io::attach_in_store;
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the tag table: `(case, the tag parameters, the tags every snapshot of the
/// set must carry)`.
type TagRow = (&'static str, Params, Vec<(&'static str, &'static str)>);

/// An instance id of the long form this service mints that no instance of the account
/// carries — every value this operation cannot resolve is refused, so a row needs one.
const UNKNOWN_INSTANCE_ID: &str = "i-0123456789abcdef0";

/// An AMI id of the long form that no image of the account carries. A launch does not
/// resolve the image it names, so an instance launched from this one is still stored — and
/// still gets the ONE root volume every launch creates, off the crate's default EBS-backed
/// Linux image rather than off a mapping no image published.
const UNKNOWN_AMI: &str = "ami-0123456789abcdef0";

/// The `InstanceSpecification` field that drops the instance's boot volume from the set, for
/// the tables whose subject is something other than which volumes the set covers: every
/// instance holds the root volume its launch gave it, so excluding it leaves exactly the
/// volumes the row attached.
fn exclude_boot_volume() -> (String, String) {
    p("InstanceSpecification.ExcludeBootVolume", "true")
}

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

/// Launch one instance from `image_id` and answer its id.
async fn launch_from(router: &MultiServiceRouter, image_id: &str) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "launching from {image_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// Create one volume of `size` GiB in the Region's first zone, encrypted or not, and answer
/// its id.
async fn create_volume(router: &MultiServiceRouter, size: i32, encrypted: bool) -> String {
    let mut params = vec![p("AvailabilityZone", zone_name(0)), p("Size", size.to_string())];
    if encrypted {
        params.push(p("Encrypted", "true"));
    }
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "creating a {size} GiB volume: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "volumeId").expect("CreateVolume reports the volume it minted")
}

/// One snapshot of a reported `<snapshotSet>`: its direct members as the text the response
/// carried for each, and its tags as the pairs they were reported in.
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
}

/// Every snapshot a `CreateSnapshots` answer reported, in the order it reported them.
fn reported_snapshots(case: &str, body: &str) -> Vec<ReportedSnapshot> {
    let document = super::wire::document(case, body);
    let mut snapshots = Vec::new();
    for set in document
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "snapshotSet")
    {
        for item in set.children().filter(roxmltree::Node::is_element) {
            let mut reported = ReportedSnapshot::default();
            for member in item.children().filter(roxmltree::Node::is_element) {
                if member.tag_name().name() == "tagSet" {
                    for tag in member.children().filter(roxmltree::Node::is_element) {
                        let text = |name: &str| {
                            tag.children()
                                .find(|child| {
                                    child.is_element() && child.tag_name().name() == name
                                })
                                .and_then(|child| child.text())
                                .unwrap_or_default()
                                .to_string()
                        };
                        reported.tags.push((text("key"), text("value")));
                    }
                    continue;
                }
                reported.members.insert(
                    member.tag_name().name().to_string(),
                    member.text().unwrap_or_default().to_string(),
                );
            }
            snapshots.push(reported);
        }
    }
    snapshots
}

/// The `InstanceSpecification` form fields naming `instance_id`.
fn instance_spec(instance_id: &str) -> Params {
    vec![p("InstanceSpecification.InstanceId", instance_id)]
}

/// The text of the `name` child element of `node`, or the empty string when it has none.
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(|child| child.text())
        .unwrap_or_default()
        .to_string()
}

/// One Amazon-published AMI of the catalog, read over the wire: `(image id, root device
/// name, root volume size, root snapshot id)` — the facts a launch from it restores its root
/// volume from, so a row names no image id and no size of its own. `narrowing` picks the
/// image a row wants out of the catalog; the FIRST image reported is the one answered.
async fn published_image(
    router: &MultiServiceRouter,
    case: &str,
    narrowing: &[(&str, &[&str])],
) -> (String, String, String, String) {
    let mut params = super::ec2_query::filters(narrowing);
    params.push(p("Owner.1", "amazon"));
    let (status, body) = op(router, "DescribeImages", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeImages must succeed, got {status} with body {body}"
    );
    let document = super::wire::document(case, &body);
    let image = document
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "imagesSet")
        .and_then(|set| set.children().find(roxmltree::Node::is_element))
        .unwrap_or_else(|| panic!("case {case:?}: the account must see a matching AMI: {body}"));
    let ebs = image
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "blockDeviceMapping")
        .and_then(|set| set.children().find(roxmltree::Node::is_element))
        .and_then(|mapping| {
            mapping
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "ebs")
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: an EBS-backed AMI publishes a root mapping: {body}")
        });
    let answered = (
        child_text(image, "imageId"),
        child_text(image, "rootDeviceName"),
        child_text(ebs, "volumeSize"),
        child_text(ebs, "snapshotId"),
    );
    assert!(
        !answered.0.is_empty() && !answered.1.is_empty() && !answered.2.is_empty(),
        "case {case:?}: the AMI must publish its id, root device and root volume size: {body}"
    );
    answered
}

/// Every volume `DescribeVolumes` reports as the instance's, as `(volume id, size,
/// encrypted, device)` in the order the report carried them.
async fn volumes_of(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Vec<(String, i32, bool, String)> {
    let (status, body) = op(
        router,
        "DescribeVolumes",
        &super::ec2_query::filters(&[("attachment.instance-id", &[instance_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeVolumes must succeed, got {status} with body {body}"
    );
    let document = super::wire::document(case, &body);
    document
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "volumeSet")
        .flat_map(|set| set.children().filter(roxmltree::Node::is_element))
        .map(|volume| {
            let device = volume
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "attachmentSet")
                .and_then(|set| set.children().find(roxmltree::Node::is_element))
                .map(|attachment| child_text(attachment, "device"))
                .unwrap_or_default();
            (
                child_text(volume, "volumeId"),
                child_text(volume, "size").parse::<i32>().unwrap_or_else(|_| {
                    panic!("case {case:?}: a volume reports its size in GiB: {body}")
                }),
                child_text(volume, "encrypted") == "true",
                device,
            )
        })
        .collect()
}

/// The ONE volume the LAUNCH gave `instance_id`, as `(volume id, size, encrypted)`: every
/// launch of this emulator creates exactly one root volume, so a snapshot set of the
/// instance covers it unless the request excludes it.
async fn launched_root_volume(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> (String, i32, bool) {
    let reported = volumes_of(router, case, instance_id).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the launch gives the instance exactly one volume, got {reported:?}"
    );
    let (volume_id, size, encrypted, _) = reported[0].clone();
    (volume_id, size, encrypted)
}

/// Every snapshot id the store holds, in store order.
fn stored_snapshot_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.snapshots.keys().cloned().collect()
}

/// One stored snapshot as the store reports it at this instant.
fn stored_snapshot(engine: &engine::InMemoryEngine, snapshot_id: &str) -> SnapshotView {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .snapshots
        .get(snapshot_id)
        .unwrap_or_else(|| panic!("the store must hold the snapshot {snapshot_id}"))
        .view_at(Instant::now())
}

/// RULE 1: one snapshot per volume the instance holds an attachment of, each read off that
/// volume. The rows differ in how many DATA volumes the instance holds beside the root volume
/// its launch created, and in each volume's size and encryption state; every row also holds
/// an unattached volume and another instance (with a root volume of its own), none of which
/// may ever be snapshotted.
#[tokio::test]
async fn one_snapshot_is_made_of_every_volume_the_instance_holds() {
    // `(case, the attached data volumes as (size, encrypted))`
    let rows: Vec<(&str, Vec<(i32, bool)>)> = vec![
        ("an instance holding only its root volume", vec![]),
        ("one unencrypted data volume", vec![(8, false)]),
        (
            "two data volumes of different sizes, one of them encrypted",
            vec![(1, false), (16, true)],
        ),
        (
            "three data volumes, every one a different size",
            vec![(4, true), (32, false), (125, true)],
        ),
    ];

    for (case, attached) in rows {
        let (router, engine) = emulator();
        let instance_id = launch_from(&router, UNKNOWN_AMI).await;
        let other_instance_id = launch_from(&router, UNKNOWN_AMI).await;

        // What the instance holds, as (volume id, size, encrypted), in the id order the
        // store walks — which is the order the answer reports them in. The launch's own root
        // volume is one of them, read back over the wire rather than written here.
        let mut held: Vec<(String, i32, bool)> =
            vec![launched_root_volume(&router, case, &instance_id).await];
        for (size, encrypted) in &attached {
            let volume_id = create_volume(&router, *size, *encrypted).await;
            attach_in_store(&engine, &volume_id, &instance_id, "/dev/sdf");
            held.push((volume_id, *size, *encrypted));
        }
        held.sort_by(|left, right| left.0.cmp(&right.0));
        // None of these is the instance's, so none may be snapshotted: a volume attached to
        // nothing, one of another instance, and that instance's own root volume.
        let (other_root, _, _) = launched_root_volume(&router, case, &other_instance_id).await;
        let unattached = create_volume(&router, 64, false).await;
        let other_volume = create_volume(&router, 2, false).await;
        attach_in_store(&engine, &other_volume, &other_instance_id, "/dev/sdf");

        let mut params = instance_spec(&instance_id);
        params.push(p("Description", "nightly set"));
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let reported = reported_snapshots(case, &body);
        assert_eq!(
            reported.len(),
            held.len(),
            "case {case:?}: expected one snapshot per volume the instance holds ({}), got \
             {:?}",
            held.len(),
            reported
        );
        assert_eq!(
            reported
                .iter()
                .map(|snapshot| snapshot.member("volumeId").to_string())
                .collect::<Vec<String>>(),
            held.iter()
                .map(|(volume_id, _, _)| volume_id.clone())
                .collect::<Vec<String>>(),
            "case {case:?}: the snapshots name the instance's own volumes, got {body}"
        );
        assert!(
            !reported.iter().any(|snapshot| [
                unattached.as_str(),
                other_volume.as_str(),
                other_root.as_str()
            ]
            .contains(&snapshot.member("volumeId"))),
            "case {case:?}: no volume of another instance and no unattached volume is \
             snapshotted, got {body}"
        );

        let account = engine::emulator_account_id();
        let start_times: BTreeSet<String> = reported
            .iter()
            .map(|snapshot| snapshot.member("startTime").to_string())
            .collect();
        assert!(
            start_times.len() <= 1,
            "case {case:?}: every snapshot of one request carries the same startTime, \
             got {start_times:?}"
        );
        for (snapshot, (volume_id, size, encrypted)) in reported.iter().zip(&held) {
            let where_ = format!("case {case:?}, the snapshot of {volume_id}");
            assert_eq!(
                snapshot.member("volumeSize"),
                size.to_string(),
                "{where_}: expected volumeSize {size}, got {:?}",
                snapshot.member("volumeSize")
            );
            assert_eq!(
                snapshot.member("encrypted"),
                encrypted.to_string(),
                "{where_}: expected encrypted {encrypted}, got {:?}",
                snapshot.member("encrypted")
            );
            assert_eq!(
                snapshot.member("state"),
                SNAPSHOT_STATE_PENDING,
                "{where_}: expected state {SNAPSHOT_STATE_PENDING}, got {:?}",
                snapshot.member("state")
            );
            assert_eq!(
                snapshot.member("progress"),
                SNAPSHOT_PROGRESS_PENDING,
                "{where_}: a snapshot that has only just started reports no progress, \
                 got {:?}",
                snapshot.member("progress")
            );
            assert_eq!(
                snapshot.member("ownerId"),
                account,
                "{where_}: expected ownerId {account}, got {:?}",
                snapshot.member("ownerId")
            );
            assert_eq!(
                snapshot.member("description"),
                "nightly set",
                "{where_}: the description is the request's, got {:?}",
                snapshot.member("description")
            );
            assert!(
                !snapshot.member("snapshotId").is_empty(),
                "{where_}: the service mints the snapshot id, got {:?}",
                snapshot.member("snapshotId")
            );
            // The answer is what the store holds: the same volume, size and tier.
            let stored = stored_snapshot(&engine, snapshot.member("snapshotId"));
            assert_eq!(
                (
                    stored.volume_id.clone(),
                    stored.volume_size,
                    stored.storage_tier.clone()
                ),
                (
                    Some(volume_id.clone()),
                    Some(*size),
                    Some(SNAPSHOT_STORAGE_TIER_STANDARD.to_string())
                ),
                "{where_}: the stored snapshot is the one that was answered, got {stored:?}"
            );
        }
        assert_eq!(
            stored_snapshot_ids(&engine).len(),
            held.len(),
            "case {case:?}: the store holds exactly the snapshots the call made"
        );
    }
}

/// RULE 1 and RULE 2 over the volumes a LAUNCH creates: the instance's root volume is the
/// one its AMI's root device names, it is reported by `DescribeVolumes` as the instance's,
/// and the two exclusions narrow the set the snapshot call makes — while naming the root
/// volume as a data volume is refused.
#[tokio::test]
async fn the_exclusions_narrow_the_set_and_the_root_volume_is_not_a_data_volume() {
    let (router, engine) = emulator();

    // One AMI the account can see, and the root device and size it publishes: the facts the
    // launch's root volume is restored from, read off the image rather than written here.
    let (image_id, root_device, root_size, root_snapshot) =
        published_image(&router, "the published catalog", &[]).await;

    let instance_id = launch_from(&router, &image_id).await;

    // The launch created the root volume, and `DescribeVolumes` reports it as the
    // instance's: the wire read-back of what the launch stored.
    let (status, volumes) = op(
        &router,
        "DescribeVolumes",
        &super::ec2_query::filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeVolumes must succeed, got {status} with body {volumes}"
    );
    let reported_volumes = super::wire::document("the instance's volumes", &volumes);
    let volume_items = reported_volumes
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "volumeSet")
        .flat_map(|set| set.children().filter(roxmltree::Node::is_element))
        .count();
    assert_eq!(
        volume_items, 1,
        "the launch gives the instance exactly one volume, got {volumes}"
    );
    let root_volume = element(&volumes, "volumeId")
        .unwrap_or_else(|| panic!("the volume must report its id: {volumes}"));
    assert_eq!(
        (
            elements(&volumes, "size"),
            elements(&volumes, "device"),
            elements(&volumes, "snapshotId")
        ),
        (
            vec![root_size.clone()],
            vec![root_device.clone()],
            vec![root_snapshot.clone()]
        ),
        "the root volume is restored from the AMI's root mapping, got {volumes}"
    );

    // Two data volumes beside it, at devices of their own.
    let first_data = create_volume(&router, 8, false).await;
    let second_data = create_volume(&router, 12, false).await;
    attach_in_store(&engine, &first_data, &instance_id, "/dev/sdf");
    attach_in_store(&engine, &second_data, &instance_id, "/dev/sdg");

    let mut all_three = vec![
        root_volume.clone(),
        first_data.clone(),
        second_data.clone(),
    ];
    all_three.sort();
    let mut data_only = vec![first_data.clone(), second_data.clone()];
    data_only.sort();
    let mut without_first = vec![root_volume.clone(), second_data.clone()];
    without_first.sort();

    // `(case, the exclusion parameters, the volumes the set must cover)`
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no exclusion at all", vec![], all_three.clone()),
        (
            "the boot volume excluded",
            vec![p("InstanceSpecification.ExcludeBootVolume", "true")],
            data_only.clone(),
        ),
        (
            "the boot volume explicitly kept",
            vec![p("InstanceSpecification.ExcludeBootVolume", "false")],
            all_three.clone(),
        ),
        (
            "one data volume excluded",
            vec![p(
                "InstanceSpecification.ExcludeDataVolumeId.1",
                first_data.clone(),
            )],
            without_first.clone(),
        ),
        (
            "both data volumes excluded",
            vec![
                p(
                    "InstanceSpecification.ExcludeDataVolumeId.1",
                    first_data.clone(),
                ),
                p(
                    "InstanceSpecification.ExcludeDataVolumeId.2",
                    second_data.clone(),
                ),
            ],
            vec![root_volume.clone()],
        ),
        (
            "the boot volume and one data volume excluded",
            vec![
                p("InstanceSpecification.ExcludeBootVolume", "true"),
                p(
                    "InstanceSpecification.ExcludeDataVolumeId.1",
                    first_data.clone(),
                ),
            ],
            vec![second_data.clone()],
        ),
    ];

    for (case, exclusions, expected) in rows {
        let mut params = instance_spec(&instance_id);
        params.extend(exclusions);
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported: Vec<String> = reported_snapshots(case, &body)
            .iter()
            .map(|snapshot| snapshot.member("volumeId").to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected snapshots of {expected:?}, got {reported:?} in {body}"
        );
    }

    // RULE 2's refusals: the root volume named as a data volume, a value that is not a
    // volume id, and more ids than the member documents.
    let too_many: Params = (1..=MAX_EXCLUDED_DATA_VOLUME_IDS + 1)
        .map(|index| {
            p(
                &format!("InstanceSpecification.ExcludeDataVolumeId.{index}"),
                format!("vol-{index:017x}"),
            )
        })
        .collect();
    let refusals: Vec<(&str, Params, &str)> = vec![
        (
            "the root volume named as a data volume",
            vec![p(
                "InstanceSpecification.ExcludeDataVolumeId.1",
                root_volume.clone(),
            )],
            "InvalidParameterValue",
        ),
        (
            "a value that is not a volume id",
            vec![p(
                "InstanceSpecification.ExcludeDataVolumeId.1",
                "not-a-volume",
            )],
            "InvalidVolumeID.Malformed",
        ),
        (
            "one more excluded id than the member admits",
            too_many,
            "InvalidParameterValue",
        ),
    ];
    let before = stored_snapshot_ids(&engine);
    for (case, exclusions, expected_code) in refusals {
        let mut params = instance_spec(&instance_id);
        params.extend(exclusions);
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
    assert_eq!(
        stored_snapshot_ids(&engine),
        before,
        "a refused request stores no snapshot"
    );
}

/// RULE 1's tag half: the request's `TagSpecifications` are applied to every snapshot of the
/// set, `CopyTagsFromSource` adds the SOURCE VOLUME's own tags, and the request's tags win
/// where both name one key. A specification naming a resource type this call does not create
/// is refused.
#[tokio::test]
async fn the_snapshots_carry_the_requested_tags_and_optionally_the_volumes_own() {
    let (router, engine) = emulator();
    let instance_id = launch_from(&router, UNKNOWN_AMI).await;
    let volume_id = create_volume(&router, 8, false).await;
    // The source volume's own tags, which `CopyTagsFromSource` copies.
    let (status, body) = op(
        &router,
        "CreateTags",
        &[
            p("ResourceId.1", &volume_id),
            p("Tag.1.Key", "Owner"),
            p("Tag.1.Value", "platform"),
            p("Tag.2.Key", "Purpose"),
            p("Tag.2.Value", "from-the-volume"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging the source volume: expected HTTP 200, got {status} with body {body}"
    );
    attach_in_store(&engine, &volume_id, &instance_id, "/dev/sdf");

    let requested = vec![
        p("TagSpecification.1.ResourceType", "snapshot"),
        p("TagSpecification.1.Tag.1.Key", "Purpose"),
        p("TagSpecification.1.Tag.1.Value", "probe"),
    ];
    let copy = vec![p("CopyTagsFromSource", "volume")];

    // `(case, the tag parameters, the tags every snapshot of the set must carry)`
    let rows: Vec<TagRow> = vec![
        ("no tag at all", vec![], vec![]),
        (
            "the request's tags only",
            requested.clone(),
            vec![("Purpose", "probe")],
        ),
        (
            "the source volume's tags only",
            copy.clone(),
            vec![("Owner", "platform"), ("Purpose", "from-the-volume")],
        ),
        (
            "both, where the request's tag wins the key they share",
            [requested.clone(), copy.clone()].concat(),
            vec![("Owner", "platform"), ("Purpose", "probe")],
        ),
    ];

    for (case, tags, expected) in rows {
        // The subject here is the TAGS, so the boot volume the launch created is excluded
        // and every snapshot of the set is of the one tagged volume.
        let mut params = instance_spec(&instance_id);
        params.push(exclude_boot_volume());
        params.extend(tags);
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_snapshots(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the instance holds one data volume, got {body}"
        );
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported[0].tags, expected,
            "case {case:?}: expected the tags {expected:?}, got {:?} in {body}",
            reported[0].tags
        );
    }

    // A specification naming a resource type this call does not create is refused.
    let mut params = instance_spec(&instance_id);
    params.extend(vec![
        p("TagSpecification.1.ResourceType", "volume"),
        p("TagSpecification.1.Tag.1.Key", "Purpose"),
        p("TagSpecification.1.Tag.1.Value", "probe"),
    ]);
    let (status, body) = op(&router, "CreateSnapshots", &params).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "a tag specification for another resource type is refused, got {status} with \
         body {body}"
    );

    // A stored snapshot is a taggable resource of this account, so `CreateTags` reaches it.
    let snapshot_id = stored_snapshot_ids(&engine)
        .first()
        .cloned()
        .expect("the calls above stored snapshots");
    let (status, body) = op(
        &router,
        "CreateTags",
        &[
            p("ResourceId.1", &snapshot_id),
            p("Tag.1.Key", "Stage"),
            p("Tag.1.Value", "later"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging a stored snapshot: expected HTTP 200, got {status} with body {body}"
    );
    let tags = stored_snapshot(&engine, &snapshot_id).tags.unwrap_or_default();
    assert!(
        tags.iter().any(|tag| {
            tag.key.as_deref() == Some("Stage") && tag.value.as_deref() == Some("later")
        }),
        "the tag lands on the stored snapshot, got {tags:?}"
    );
}

/// RULE 3: the snapshot is handed back `pending` with no progress, and the CLOCK moves it to
/// `completed` with a completion time — no caller does, and a second read before the move
/// falls due still reports `pending`.
#[tokio::test]
async fn the_clock_completes_a_pending_snapshot() {
    let (router, engine) = emulator();
    let instance_id = launch_from(&router, UNKNOWN_AMI).await;
    let volume_id = create_volume(&router, 8, false).await;
    attach_in_store(&engine, &volume_id, &instance_id, "/dev/sdf");

    let (status, body) = op(&router, "CreateSnapshots", &instance_spec(&instance_id)).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200, got {status} with body {body}"
    );
    let snapshot_id = reported_snapshots("the created snapshot", &body)[0]
        .member("snapshotId")
        .to_string();

    let started = stored_snapshot(&engine, &snapshot_id);
    assert_eq!(
        (started.state.clone(), started.progress.clone()),
        (
            Some(SNAPSHOT_STATE_PENDING.to_string()),
            Some(SNAPSHOT_PROGRESS_PENDING.to_string())
        ),
        "a snapshot nothing has waited for is still pending, got {started:?}"
    );
    assert!(
        started.completion_time.is_none(),
        "a pending snapshot has not completed, got {:?}",
        started.completion_time
    );

    past_the_transition().await;

    let completed = stored_snapshot(&engine, &snapshot_id);
    assert_eq!(
        (completed.state.clone(), completed.progress.clone()),
        (
            Some(SNAPSHOT_STATE_COMPLETED.to_string()),
            Some(SNAPSHOT_PROGRESS_COMPLETED.to_string())
        ),
        "the clock completes the snapshot, got {completed:?}"
    );
    assert!(
        completed.completion_time.is_some(),
        "a completed snapshot reports when it completed, got {completed:?}"
    );
    assert_eq!(
        completed.volume_id, started.volume_id,
        "the clock changes nothing else about the snapshot"
    );
}

/// RULE 4: every request this emulator cannot serve is refused with EC2's own code, and
/// nothing is stored — including the `DryRun` probe, which reports that the call would have
/// worked.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_stores_nothing() {
    let (router, engine) = emulator();
    let instance_id = launch_from(&router, UNKNOWN_AMI).await;
    let volume_id = create_volume(&router, 8, false).await;
    attach_in_store(&engine, &volume_id, &instance_id, "/dev/sdf");

    // `(case, the parameters, the status, the error code)`
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "an instance id no instance of the account has",
            instance_spec(UNKNOWN_INSTANCE_ID),
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "a value that is not an instance id at all",
            instance_spec("not-an-instance"),
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "an instance id of the short form that names nothing",
            instance_spec("i-0123456f"),
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "a location only an instance in a Local Zone can ask for",
            [instance_spec(&instance_id), vec![p("Location", "local")]].concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a location outside the modelled catalog, refused by the enum itself",
            [instance_spec(&instance_id), vec![p("Location", "orbital")]].concat(),
            400,
            "ValidationError",
        ),
        (
            "a tag source outside the modelled catalog, refused by the enum itself",
            [
                instance_spec(&instance_id),
                vec![p("CopyTagsFromSource", "instance")],
            ]
            .concat(),
            400,
            "ValidationError",
        ),
        (
            "an Outpost no instance of this emulator is on",
            [
                instance_spec(&instance_id),
                vec![p(
                    "OutpostArn",
                    engine::emulator_arn("outposts", REGION, "outpost/op-0123456789abcdef0"),
                )],
            ]
            .concat(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run of a request that would have worked",
            [instance_spec(&instance_id), vec![p("DryRun", "true")]].concat(),
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
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
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal says why, got {body}"
        );
        assert!(
            stored_snapshot_ids(&engine).is_empty(),
            "case {case:?}: a refused request stores no snapshot, got {:?}",
            stored_snapshot_ids(&engine)
        );
    }

    // This operation words EVERY instance id it cannot resolve the one way: the fixed
    // sentence that reports the id could not be read, which names no value at all — so a
    // well-formed id the account holds no instance for gets exactly the same answer as a
    // value outside the id form.
    let unreadable = "Unable to unobfuscate ID.";
    for id in [UNKNOWN_INSTANCE_ID, "not-an-instance", "i-00000000000000000"] {
        let (_, body) = op(&router, "CreateSnapshots", &instance_spec(id)).await;
        assert_eq!(
            error_message(&body),
            unreadable,
            "case {id:?}: expected the operation's one sentence for an id it cannot resolve, \
             got {body}"
        );
    }

    // Once the request is one this emulator CAN serve, the same instance is snapshotted.
    let (status, body) = op(&router, "CreateSnapshots", &instance_spec(&instance_id)).await;
    assert_eq!(
        status, 200,
        "the same request without the refused members: expected HTTP 200, got {status} \
         with body {body}"
    );
    assert_eq!(
        stored_snapshot_ids(&engine).len(),
        2,
        "one snapshot per volume the instance holds — its root volume and the attached \
         one — was stored, got {:?}",
        stored_snapshot_ids(&engine)
    );
}

/// RULE 5: EVERY launch gives its instance exactly one EBS root volume, and the snapshot set
/// of that instance covers it — the device and the size come off the AMI's root block device
/// mapping when the account can see that AMI, and off the crate's default EBS-backed Linux
/// image when it cannot, so no instance this emulator stores is one whose volumes a snapshot
/// call cannot find. The rows differ in the AMI the launch named: the published Linux catalog
/// image, the published Windows one (a different root device and a bigger root volume) and an
/// id no image of the account has.
#[tokio::test]
async fn every_launch_gets_one_root_volume_the_snapshot_set_covers() {
    let (router, _engine) = emulator();

    // What each catalog AMI publishes, read over the wire, so no row writes a size of its
    // own. The default row's device and size are the crate's constants for a published Linux
    // image ([`LINUX_ROOT_DEVICE_NAME`], [`LINUX_ROOT_VOLUME_SIZE`]) — the one definition the
    // catalog rows above are built from too.
    let (linux_image, linux_device, linux_size, _) = published_image(
        &router,
        "the published Linux image",
        &[("root-device-name", &[LINUX_ROOT_DEVICE_NAME])],
    )
    .await;
    let (windows_image, windows_device, windows_size, _) = published_image(
        &router,
        "the published Windows image",
        // The Windows root device name is the published catalog's instance-store images' too,
        // and those restore no snapshot, so the row narrows to the EBS-backed one it means.
        &[
            ("root-device-name", &[WINDOWS_ROOT_DEVICE_NAME]),
            ("root-device-type", &["ebs"]),
        ],
    )
    .await;
    let gib = |case: &str, size: &str| -> i32 {
        size.parse()
            .unwrap_or_else(|_| panic!("case {case:?}: a published size is a number: {size:?}"))
    };

    // `(case, the AMI the launch names, the device its root volume sits at, its size in GiB)`
    let rows: Vec<(&str, String, String, i32)> = vec![
        (
            "a published Linux AMI",
            linux_image,
            linux_device,
            gib("a published Linux AMI", &linux_size),
        ),
        (
            "a published Windows AMI",
            windows_image,
            windows_device,
            gib("a published Windows AMI", &windows_size),
        ),
        (
            "an AMI the account cannot see",
            UNKNOWN_AMI.to_string(),
            LINUX_ROOT_DEVICE_NAME.to_string(),
            LINUX_ROOT_VOLUME_SIZE,
        ),
    ];

    for (case, image_id, device, size) in rows {
        let instance_id = launch_from(&router, &image_id).await;

        // The launch created exactly one volume, and `DescribeVolumes` reports it as the
        // instance's: the wire read-back of what the launch stored.
        let held = volumes_of(&router, case, &instance_id).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the launch gives the instance exactly one volume, got {held:?}"
        );
        let (volume_id, reported_size, encrypted, reported_device) = held[0].clone();
        assert_eq!(
            (reported_device.as_str(), reported_size, encrypted),
            (device.as_str(), size, false),
            "case {case:?}: expected a {size} GiB unencrypted volume at {device}, got {held:?}"
        );

        // The snapshot set of the instance is exactly that volume, sized off it.
        let (status, body) = op(&router, "CreateSnapshots", &instance_spec(&instance_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_snapshots(case, &body);
        assert_eq!(
            reported
                .iter()
                .map(|snapshot| snapshot.member("volumeId").to_string())
                .collect::<Vec<String>>(),
            vec![volume_id.clone()],
            "case {case:?}: expected one snapshot of {volume_id}, got {body}"
        );
        assert_eq!(
            reported[0].member("volumeSize"),
            size.to_string(),
            "case {case:?}: the snapshot is sized off the volume, got {body}"
        );

        // And that volume is the BOOT volume, so the exclusion drops it: the device the
        // launch used is the one `ExcludeBootVolume` is about.
        let mut params = instance_spec(&instance_id);
        params.push(exclude_boot_volume());
        let (status, body) = op(&router, "CreateSnapshots", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            reported_snapshots(case, &body).is_empty(),
            "case {case:?}: the instance holds nothing but its boot volume, so excluding it \
             leaves no snapshot to make, got {body}"
        );
    }
}
