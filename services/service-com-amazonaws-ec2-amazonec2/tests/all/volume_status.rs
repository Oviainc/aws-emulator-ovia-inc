// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVolumeStatus`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `DescribeVolumeStatus` REPORTS THE STATUS CHECKS OF THE STORED VOLUMES: one item per
//!    volume the account holds, carrying that volume's own id, zone and attachments, with
//!    the result of its checks — `io-enabled` reporting that volume's stored I/O state, so
//!    the overall status is `ok` while it passes and `impaired` when it fails;
//! 2. the status is NOT the volume's state: "volume status is based on the volume status
//!    checks, and does not reflect the volume state", so the clock-owed `creating` ->
//!    `available` move does not change what this operation reports;
//! 3. the request narrows what is reported: `VolumeIds` resolves or is refused with EC2's
//!    own code, each filter this operation DOCUMENTS selects on the value the item carries
//!    for it (and an undocumented one is refused), and `MaxResults`/`NextToken` page the
//!    answer in `VolumeId` order;
//! 4. it is a READ: two calls report the same thing and the store is untouched.
//!
//! Each table row is a distinct input along what the rule generalises over — the volume
//! type, the zone, which filter, which values, how many attachments, which page size — so
//! no single row is the specification.

use std::time::Duration;

use ec2::ops::types::AttachVolumeOutput;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_FILTERS, MAX_FILTER_VALUES,
    MAX_FILTER_VALUE_BYTES, MAX_PAGE_SIZE, MAX_RESOURCE_IDS, TRANSITION_DELAY,
    VOLUME_IOPS_RANGES, VOLUME_SIZE_RANGES, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};
// The ec2Query XML is read through the document, with the helpers the VPC suite already
// shares, so a nested `<status>` is never mistaken for the outer one.
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for; the zone names and ids the assertions use are
/// derived from it.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the status, the
/// error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// One row of a filter table: `(case, the filter parameters, the ids it must select)`.
type FilterRow = (&'static str, Params, Vec<String>);

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The zone of the signed Region at `index`.
fn zone_name(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_name.clone()
}

fn zone_id(index: usize) -> String {
    availability_zones_in_region(REGION)[index].zone_id.clone()
}

async fn describe_status(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVolumeStatus", params).await
}

/// The IOPS a volume type needs on creation, as `Iops` parameters: "Required for io1 and
/// io2 volumes" (`CreateVolumeRequest$Iops`), at the bottom of that type's range.
fn required_iops(volume_type: &str) -> Params {
    VOLUME_IOPS_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type && matches!(*name, "io1" | "io2"))
        .map(|(_, min, _)| vec![p("Iops", min.to_string())])
        .unwrap_or_default()
}

/// Create one volume of `volume_type`, at `size` GiB, in the Region's `zone` and answer the
/// id `CreateVolume` minted for it.
async fn create_volume(
    router: &MultiServiceRouter,
    case: &str,
    volume_type: &str,
    size: i32,
    zone: usize,
) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(zone)),
        p("VolumeType", volume_type),
        p("Size", size.to_string()),
    ];
    params.extend(required_iops(volume_type));
    let (status, body) = call(router, REGION, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
    );
    element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume answered no volumeId: {body}"))
}

// ---------------------------------------------------------------------------
// Reading the ec2Query answer back
// ---------------------------------------------------------------------------

/// One `VolumeStatusItem`, as the response renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusRow {
    volume_id: String,
    availability_zone: String,
    availability_zone_id: String,
    /// `VolumeStatus$Status` — the overall status.
    status: String,
    /// `VolumeStatus$Details`, as `(name, status)` sorted by name.
    details: Vec<(String, String)>,
    /// `AttachmentStatuses`, as `(instance id, io performance)`.
    attachments: Vec<(String, String)>,
    /// How many entries the `Events` set carries, or `None` when the response carries no
    /// such set at all.
    events: Option<usize>,
    /// How many entries the `Actions` set carries, or `None` when the response carries no
    /// such set at all.
    actions: Option<usize>,
}

fn status_row(item: roxmltree::Node<'_, '_>) -> StatusRow {
    let volume_status = child(item, "volumeStatus");
    let mut details: Vec<(String, String)> = volume_status
        .map(|status| set_items(status, "details"))
        .unwrap_or_default()
        .into_iter()
        .map(|detail| {
            (
                child_text(detail, "name").unwrap_or_default(),
                child_text(detail, "status").unwrap_or_default(),
            )
        })
        .collect();
    details.sort();
    StatusRow {
        volume_id: child_text(item, "volumeId").unwrap_or_default(),
        availability_zone: child_text(item, "availabilityZone").unwrap_or_default(),
        availability_zone_id: child_text(item, "availabilityZoneId").unwrap_or_default(),
        // The overall status is a CHILD of `volumeStatus`, which also holds one `<status>`
        // per `<details>` entry; reading the tree tells the two apart.
        status: volume_status
            .and_then(|status| child_text(status, "status"))
            .unwrap_or_default(),
        details,
        attachments: set_items(item, "attachmentStatuses")
            .into_iter()
            .map(|attachment| {
                (
                    child_text(attachment, "instanceId").unwrap_or_default(),
                    child_text(attachment, "ioPerformance").unwrap_or_default(),
                )
            })
            .collect(),
        // `None` when the set element itself is missing, so a response that simply does not
        // carry `eventsSet`/`actionsSet` cannot read as "the set is empty".
        events: child(item, "eventsSet").map(|_| set_items(item, "eventsSet").len()),
        actions: child(item, "actionsSet").map(|_| set_items(item, "actionsSet").len()),
    }
}

/// Every status item in a `DescribeVolumeStatus` response, in the order it reported them.
fn status_rows(body: &str) -> Vec<StatusRow> {
    let answer = document("a DescribeVolumeStatus answer", body);
    set_items(answer.root_element(), "volumeStatusSet")
        .into_iter()
        .map(status_row)
        .collect()
}

/// The `nextToken` a paged answer carries, read as a child of the response envelope.
fn next_token(body: &str) -> Option<String> {
    let answer = document("a DescribeVolumeStatus answer", body);
    child_text(answer.root_element(), "nextToken")
}

/// The `VolumeState` a `DescribeVolumes` answer reports for its one volume — the member
/// the model calls `State` and the wire calls `status`, read off the volume's own element
/// so no other `<status>` in the body can stand in for it.
fn described_volume_state(body: &str) -> String {
    let answer = document("a DescribeVolumes answer", body);
    set_items(answer.root_element(), "volumeSet")
        .into_iter()
        .filter_map(|volume| child_text(volume, "status"))
        .next()
        .unwrap_or_default()
}

/// The ids `DescribeVolumeStatus` reports for `params`, in the order it reported them.
async fn reported_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe_status(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumeStatus, got {status} with body {body}"
    );
    status_rows(&body)
        .into_iter()
        .map(|row| row.volume_id)
        .collect()
}

/// The checks every volume of this emulator passes: `io-enabled` `passed`, and the
/// `io-performance` check reported as not performed, sorted by name the way [`status_row`]
/// sorts them.
fn passing_checks() -> Vec<(String, String)> {
    vec![
        ("io-enabled".to_string(), "passed".to_string()),
        ("io-performance".to_string(), "not-applicable".to_string()),
    ]
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// RULE 1: the store's volumes, with the result of their checks
// ---------------------------------------------------------------------------

/// Every stored volume is reported with ITS OWN id and zone and the result of its checks,
/// whatever volume type and zone it was created in.
///
/// The table spans the modelled volume-type catalog ([`VOLUME_SIZE_RANGES`], whose source
/// is `CreateVolumeRequest$Size`) at the smallest size each type accepts, alternating the
/// Availability Zone so neither the type nor the zone is fixed across the rows.
#[tokio::test]
async fn every_stored_volume_reports_its_own_facts_and_its_passing_checks() {
    let (router, _engine) = emulator();

    let mut expected: Vec<(String, String, usize)> = Vec::new();
    for (index, (volume_type, min_size, _)) in VOLUME_SIZE_RANGES.iter().enumerate() {
        let zone = index % availability_zones_in_region(REGION).len().min(3);
        let case = format!("a {volume_type} volume of {min_size} GiB in zone {zone}");
        let volume_id = create_volume(&router, &case, volume_type, *min_size, zone).await;
        expected.push((case, volume_id, zone));
    }

    for (case, volume_id, zone) in &expected {
        let params = indexed("VolumeId", &[volume_id.as_str()]);
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let rows = status_rows(&body);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected exactly the named volume, got {rows:?} from {body}"
        );
        let expected_row = StatusRow {
            volume_id: volume_id.clone(),
            availability_zone: zone_name(*zone),
            availability_zone_id: zone_id(*zone),
            status: "ok".to_string(),
            details: passing_checks(),
            // Nothing is attached to a fresh volume, and this emulator records no volume
            // event, so there is no action to take either — both sets are reported, and
            // empty.
            attachments: Vec::new(),
            events: Some(0),
            actions: Some(0),
        };
        assert_eq!(
            rows[0], expected_row,
            "case {case:?}: expected {expected_row:?}, got {:?}",
            rows[0]
        );
    }

    // Unnarrowed, the operation reports the whole store, in `VolumeId` order.
    let mut all_ids: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();
    all_ids.sort();
    let reported = reported_ids(&router, "every stored volume", &[]).await;
    assert_eq!(
        reported, all_ids,
        "every stored volume: expected {all_ids:?} in id order, got {reported:?}"
    );
}

/// `AttachmentStatuses` is one entry per attachment the STORE holds for the volume: none,
/// one, or one per attachment of a Multi-Attach volume.
///
/// The attachments are written into the store directly, the way `AttachVolume` will when it
/// is implemented (it is still a scaffold stub), because the rule generalises over HOW MANY
/// attachments a volume has and no API path here can add one yet.
#[tokio::test]
async fn attachment_statuses_are_one_entry_per_stored_attachment() {
    let (router, engine) = emulator();

    let unattached = create_volume(&router, "unattached", "gp3", 10, 0).await;
    let attached = create_volume(&router, "attached", "io2", 8, 0).await;
    let multi_attached = create_volume(&router, "multi-attached", "io1", 8, 1).await;

    let instances = ["i-0123456789abcdef0", "i-00112233445566778"];
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let attachment = |volume_id: &str, instance_id: &str| AttachVolumeOutput {
            volume_id: Some(volume_id.to_string()),
            instance_id: Some(instance_id.to_string()),
            device: Some("/dev/sdf".to_string()),
            state: Some("attached".to_string()),
            ..AttachVolumeOutput::default()
        };
        guard
            .volumes
            .get_mut(&attached)
            .expect("the attached volume is stored")
            .view
            .attachments = Some(vec![attachment(&attached, instances[0])]);
        guard
            .volumes
            .get_mut(&multi_attached)
            .expect("the multi-attached volume is stored")
            .view
            .attachments = Some(vec![
            attachment(&multi_attached, instances[0]),
            attachment(&multi_attached, instances[1]),
        ]);
    }

    let table: Vec<(&str, &String, Vec<String>)> = vec![
        ("no attachment", &unattached, Vec::new()),
        ("one attachment", &attached, vec![instances[0].to_string()]),
        (
            "two attachments",
            &multi_attached,
            vec![instances[0].to_string(), instances[1].to_string()],
        ),
    ];
    for (case, volume_id, expected_instances) in table {
        let params = indexed("VolumeId", &[volume_id.as_str()]);
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let rows = status_rows(&body);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected one item, got {rows:?}"
        );
        let reported: Vec<String> = rows[0]
            .attachments
            .iter()
            .map(|(instance_id, _)| instance_id.clone())
            .collect();
        assert_eq!(
            reported, expected_instances,
            "case {case:?}: expected the stored attachments' instances {expected_instances:?}, \
             got {:?}",
            rows[0]
        );
        for (instance_id, io_performance) in &rows[0].attachments {
            assert_eq!(
                io_performance, "not-applicable",
                "case {case:?}: nothing measures the I/O of {instance_id}'s attachment, so the \
                 check is not performed, got {io_performance:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the checks are not the volume's state
// ---------------------------------------------------------------------------

/// A volume's status checks do not move with its state: the same `ok` before and after the
/// clock-owed `creating` -> `available` transition, which `DescribeVolumes` does report.
#[tokio::test]
async fn the_status_checks_do_not_follow_the_volume_state() {
    let (router, _engine) = emulator();
    let volume_id = create_volume(&router, "a volume across its transition", "gp2", 1, 0).await;
    let params = indexed("VolumeId", &[volume_id.as_str()]);

    let (status, body) = describe_status(&router, &params).await;
    assert_eq!(
        status, 200,
        "while creating: expected HTTP 200, got {status} with body {body}"
    );
    let while_creating = status_rows(&body);

    let (volumes_status, volumes_body) =
        call(&router, REGION, "DescribeVolumes", &params).await;
    assert_eq!(
        volumes_status, 200,
        "while creating: expected HTTP 200 from DescribeVolumes, got {volumes_status} with body \
         {volumes_body}"
    );
    let state_while_creating = described_volume_state(&volumes_body);
    assert_eq!(
        state_while_creating, "creating",
        "while creating: expected the volume to still be creating, got \
         {state_while_creating:?} in {volumes_body}"
    );

    past_the_transition().await;

    let (later_status, later_body) = describe_status(&router, &params).await;
    assert_eq!(
        later_status, 200,
        "once available: expected HTTP 200, got {later_status} with body {later_body}"
    );
    let (volumes_status, volumes_body) =
        call(&router, REGION, "DescribeVolumes", &params).await;
    assert_eq!(
        volumes_status, 200,
        "once available: expected HTTP 200 from DescribeVolumes, got {volumes_status} with body \
         {volumes_body}"
    );
    let state_once_available = described_volume_state(&volumes_body);
    assert_eq!(
        state_once_available, "available",
        "once available: expected the clock to have moved the volume, got \
         {state_once_available:?} in {volumes_body}"
    );

    let once_available = status_rows(&later_body);
    assert_eq!(
        once_available, while_creating,
        "across the transition: the status checks are not the volume state, so the report must \
         be unchanged; got {once_available:?} after {while_creating:?}"
    );
    assert_eq!(
        once_available
            .first()
            .map(|row| row.status.clone())
            .unwrap_or_default(),
        "ok",
        "across the transition: expected a volume whose checks pass to be ok, got \
         {once_available:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the request narrows what is reported
// ---------------------------------------------------------------------------

/// A `VolumeIds` entry this account holds no volume for is an ERROR, not an absent row —
/// malformed before unknown — and `DryRun` is answered only once the request would have
/// succeeded.
#[tokio::test]
async fn unresolvable_ids_and_dry_run_are_refused_with_ec2s_own_codes() {
    let (router, _engine) = emulator();
    let volume_id = create_volume(&router, "the stored volume", "gp3", 4, 0).await;

    let table: Vec<RefusalRow> = vec![
        (
            "an unknown long id",
            indexed("VolumeId", &["vol-0123456789abcdef0"]),
            400,
            "InvalidVolume.NotFound",
        ),
        (
            "an unknown short id",
            indexed("VolumeId", &["vol-01234567"]),
            400,
            "InvalidVolume.NotFound",
        ),
        (
            "an id that is not hex",
            indexed("VolumeId", &["vol-zzzzzzzz"]),
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "an id of the wrong length",
            indexed("VolumeId", &["vol-0123456789abcde"]),
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "an id with no prefix",
            indexed("VolumeId", &["0123456789abcdef0"]),
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "an empty id",
            indexed("VolumeId", &[""]),
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "a known id beside an unknown one",
            indexed("VolumeId", &[volume_id.as_str(), "vol-0123456789abcdef0"]),
            400,
            "InvalidVolume.NotFound",
        ),
        (
            "DryRun on a request that would have succeeded",
            {
                let mut params = indexed("VolumeId", &[volume_id.as_str()]);
                params.push(p("DryRun", "true"));
                params
            },
            412,
            "DryRunOperation",
        ),
        (
            "DryRun on a request that would have been refused",
            {
                let mut params = indexed("VolumeId", &["vol-0123456789abcdef0"]);
                params.push(p("DryRun", "true"));
                params
            },
            400,
            "InvalidVolume.NotFound",
        ),
        (
            "a page size below one",
            vec![p("MaxResults", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a page size above the maximum",
            vec![p("MaxResults", (MAX_PAGE_SIZE + 1).to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
    ];

    for (case, params, expected_status, expected_code) in table {
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
    }

    // A refused request reports nothing, and the volume is still readable afterwards.
    let reported = reported_ids(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported,
        vec![volume_id.clone()],
        "after the refusals: expected the stored volume, got {reported:?}"
    );
}

/// The caps on what ONE request may ask for are refused with EC2's `InvalidParameterValue`,
/// one row per cap, each at the first value past it: the matching cost is the product of
/// the ids, the filters, their values and their length, so each factor has a bound and each
/// bound is a checked contract rather than a convention.
#[tokio::test]
async fn the_request_size_caps_are_refused_one_past_each_bound() {
    let (router, _engine) = emulator();
    let stored_volume = create_volume(&router, "the stored volume", "gp2", 1, 0).await;

    // One id past `MAX_RESOURCE_IDS`, all well-formed, so the cap is what refuses them.
    let too_many_ids: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("vol-{index:017x}"))
        .collect();
    let too_many_id_params = indexed(
        "VolumeId",
        &too_many_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
    );

    // One filter past `MAX_FILTERS`, each a name this operation documents.
    let too_many_filters: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("volume-status.status", &["ok"][..]))
        .collect();

    // One value past `MAX_FILTER_VALUES`, in a single documented filter.
    let too_many_values: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("ok{index}"))
        .collect();
    let too_many_value_refs: Vec<&str> = too_many_values.iter().map(String::as_str).collect();

    // One byte past `MAX_FILTER_VALUE_BYTES` in one value.
    let oversize_value = "o".repeat(MAX_FILTER_VALUE_BYTES + 1);

    let table: Vec<(&str, Params)> = vec![
        ("one id past MAX_RESOURCE_IDS", too_many_id_params),
        ("one filter past MAX_FILTERS", filters(&too_many_filters)),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("volume-status.status", &too_many_value_refs)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("volume-status.status", &[oversize_value.as_str()])]),
        ),
    ];
    for (case, params) in table {
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {code:?} with body {body}"
        );
    }

    // The same request one value SHORT of each bound is served, so the caps refuse only
    // what is past them. `MAX_RESOURCE_IDS` ids are every one the stored volume's, so the
    // list is at the cap AND every id resolves: what the row tests is the cap, not the
    // lookup.
    let ids_at_the_bound: Vec<String> = vec![stored_volume.clone(); MAX_RESOURCE_IDS];
    let at_the_bound: Vec<(&str, Params)> = vec![
        (
            "MAX_RESOURCE_IDS ids",
            indexed(
                "VolumeId",
                &ids_at_the_bound
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>(),
            ),
        ),
        (
            "MAX_FILTERS filters",
            filters(&too_many_filters[..MAX_FILTERS]),
        ),
        (
            "MAX_FILTER_VALUES values",
            filters(&[(
                "volume-status.status",
                &too_many_value_refs[..MAX_FILTER_VALUES],
            )]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("volume-status.status", &[&oversize_value[1..]])]),
        ),
    ];
    for (case, params) in at_the_bound {
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the bound, got {status} with body {body}"
        );
    }
}

/// Each filter this operation DOCUMENTS selects on the value the item carries for it, and
/// a name it does not document is refused rather than read as "no results".
///
/// The documented names are `DescribeVolumeStatusRequest$Filters`; the undocumented rows
/// include `volume-id` and `tag:<key>`, which `DescribeVolumes` documents and THIS
/// operation does not.
#[tokio::test]
async fn the_documented_filters_select_and_the_rest_are_refused() {
    let (router, _engine) = emulator();
    let first_zone = create_volume(&router, "zone 0, gp2", "gp2", 1, 0).await;
    let second_zone = create_volume(&router, "zone 1, io1", "io1", 4, 1).await;
    let third = create_volume(&router, "zone 0, gp3", "gp3", 1, 0).await;

    let mut all: Vec<String> = vec![first_zone.clone(), second_zone.clone(), third.clone()];
    all.sort();
    let mut in_first_zone: Vec<String> = vec![first_zone.clone(), third.clone()];
    in_first_zone.sort();
    let in_second_zone = vec![second_zone.clone()];
    let none: Vec<String> = Vec::new();

    let table: Vec<FilterRow> = vec![
        (
            "volume-status.status selects the volumes whose checks pass",
            filters(&[("volume-status.status", &["ok"])]),
            all.clone(),
        ),
        (
            "volume-status.status impaired selects nothing here",
            filters(&[("volume-status.status", &["impaired"])]),
            none.clone(),
        ),
        (
            "volume-status.status insufficient-data selects nothing here",
            filters(&[("volume-status.status", &["insufficient-data"])]),
            none.clone(),
        ),
        (
            "volume-status.status takes EC2's single-character wildcard",
            filters(&[("volume-status.status", &["o?"])]),
            all.clone(),
        ),
        (
            "volume-status.status ORs its values",
            filters(&[("volume-status.status", &["warning", "ok"])]),
            all.clone(),
        ),
        (
            "a filter with no values selects nothing",
            filters(&[("volume-status.status", &[])]),
            none.clone(),
        ),
        (
            "volume-status.details-name io-enabled",
            filters(&[("volume-status.details-name", &["io-enabled"])]),
            all.clone(),
        ),
        (
            "volume-status.details-name io-performance",
            filters(&[("volume-status.details-name", &["io-performance"])]),
            all.clone(),
        ),
        (
            "volume-status.details-name initialization-state selects nothing here",
            filters(&[("volume-status.details-name", &["initialization-state"])]),
            none.clone(),
        ),
        (
            "volume-status.details-status passed",
            filters(&[("volume-status.details-status", &["passed"])]),
            all.clone(),
        ),
        (
            "volume-status.details-status not-applicable",
            filters(&[("volume-status.details-status", &["not-applicable"])]),
            all.clone(),
        ),
        (
            "volume-status.details-status failed selects nothing here",
            filters(&[("volume-status.details-status", &["failed"])]),
            none.clone(),
        ),
        (
            "volume-status.details-status degraded selects nothing here",
            filters(&[("volume-status.details-status", &["degraded"])]),
            none.clone(),
        ),
        (
            "availability-zone selects the volumes of one zone",
            filters(&[("availability-zone", &[&zone_name(0)])]),
            in_first_zone.clone(),
        ),
        (
            "availability-zone selects the volumes of another zone",
            filters(&[("availability-zone", &[&zone_name(1)])]),
            in_second_zone.clone(),
        ),
        (
            "availability-zone of a zone no volume is in",
            filters(&[("availability-zone", &[&zone_name(2)])]),
            none.clone(),
        ),
        (
            "availability-zone takes EC2's multi-character wildcard",
            filters(&[("availability-zone", &[&format!("{REGION}*")])]),
            all.clone(),
        ),
        (
            "two filters are ANDed",
            filters(&[
                ("volume-status.status", &["ok"]),
                ("availability-zone", &[&zone_name(1)]),
            ]),
            in_second_zone.clone(),
        ),
        (
            "an AND whose second filter matches nothing selects nothing",
            filters(&[
                ("volume-status.status", &["ok"]),
                ("availability-zone", &[&zone_name(2)]),
            ]),
            none.clone(),
        ),
        // No path in this emulator records a volume event, so every event and action
        // filter compares against an empty list and selects nothing — rather than being
        // refused, which is what an UNDOCUMENTED name gets.
        (
            "event.event-type",
            filters(&[("event.event-type", &["io-enabled:passed"])]),
            none.clone(),
        ),
        (
            "event.event-id",
            filters(&[("event.event-id", &["evol-01234567"])]),
            none.clone(),
        ),
        (
            "event.description",
            filters(&[("event.description", &["*"])]),
            none.clone(),
        ),
        (
            "event.not-before",
            filters(&[("event.not-before", &["2026-01-01T00:00:00Z"])]),
            none.clone(),
        ),
        (
            "event.not-after",
            filters(&[("event.not-after", &["2026-01-01T00:00:00Z"])]),
            none.clone(),
        ),
        (
            "action.code",
            filters(&[("action.code", &["enable-volume-io"])]),
            none.clone(),
        ),
        (
            "action.description",
            filters(&[("action.description", &["*"])]),
            none.clone(),
        ),
        (
            "action.event-id",
            filters(&[("action.event-id", &["evol-01234567"])]),
            none.clone(),
        ),
    ];

    for (case, params, expected) in table {
        let reported = reported_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    let refused: Vec<(&str, Params)> = vec![
        (
            "volume-id, which only DescribeVolumes documents",
            filters(&[("volume-id", &[first_zone.as_str()])]),
        ),
        (
            "a tag filter, which this operation documents none of",
            filters(&[("tag:Name", &["anything"])]),
        ),
        ("tag-key", filters(&[("tag-key", &["Name"])])),
        (
            "status, which is DescribeVolumes' name for the state",
            filters(&[("status", &["ok"])]),
        ),
        (
            "a documented name in the wrong case",
            filters(&[("Volume-Status.Status", &["ok"])]),
        ),
        (
            "a misspelled documented name",
            filters(&[("volume-status.detail-name", &["io-enabled"])]),
        ),
        ("an empty filter name", filters(&[("", &["ok"])])),
    ];
    for (case, params) in refused {
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an undocumented filter, got {status} with body \
             {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {code:?} with body {body}"
        );
    }
}

/// `MaxResults`/`NextToken` page the answer in `VolumeId` order: every page size reports
/// every volume exactly once, in the same order, and the last page carries no token.
#[tokio::test]
async fn pages_report_every_volume_once_in_id_order() {
    let (router, _engine) = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let case = format!("volume {index}");
        created.push(create_volume(&router, &case, "gp3", 1 + index, index as usize % 3).await);
    }
    created.sort();

    for page_size in [1, 2, 3, 5, 6] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: pagination did not terminate, saw {seen:?}"
            );
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe_status(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let rows = status_rows(&body);
            assert!(
                rows.len() <= page_size,
                "case {case:?}: expected at most {page_size} items, got {rows:?}"
            );
            seen.extend(rows.into_iter().map(|row| row.volume_id));
            token = next_token(&body);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: expected every volume once in id order, got {seen:?}"
        );
    }

    // The ids a page reports are the ids `DescribeVolumeStatus` reports unpaged, so a
    // narrowed request and a paged one agree.
    let unpaged = reported_ids(&router, "unpaged", &[]).await;
    assert_eq!(
        unpaged, created,
        "unpaged: expected every volume in id order, got {unpaged:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: it is a read
// ---------------------------------------------------------------------------

/// Describing a volume's status changes nothing a later caller sees: the second call
/// reports exactly what the first did, and `DescribeVolumes` still reports the same volume.
#[tokio::test]
async fn describing_the_status_changes_nothing() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router, "a volume read twice", "st1", 125, 0).await;
    past_the_transition().await;

    let (first_status, first_body) = describe_status(&router, &[]).await;
    assert_eq!(
        first_status, 200,
        "first read: expected HTTP 200, got {first_status} with body {first_body}"
    );
    let (second_status, second_body) = describe_status(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second_body}"
    );
    assert_eq!(
        status_rows(&second_body),
        status_rows(&first_body),
        "second read: expected the same report, got {second_body} after {first_body}"
    );

    let (volumes_status, volumes_body) = call(&router, REGION, "DescribeVolumes", &[]).await;
    assert_eq!(
        volumes_status, 200,
        "after the reads: expected HTTP 200 from DescribeVolumes, got {volumes_status} with body \
         {volumes_body}"
    );
    assert_eq!(
        elements(&volumes_body, "volumeId"),
        vec![volume_id.clone()],
        "after the reads: expected the volume to still be stored, got {volumes_body}"
    );
    let stored = engine.state::<Ec2State>().lock().volumes.len();
    assert_eq!(
        stored, 1,
        "after the reads: expected the store to hold just the one volume, got {stored}"
    );
}

/// `IncludeManagedResources` narrows nothing here: "the output includes resources that are
/// managed by Amazon Web Services services"
/// (`DescribeVolumeStatusRequest$IncludeManagedResources`), and no volume this emulator
/// stores is managed by a service — `CreateVolume` reports no `Operator` — so every stored
/// volume is reported whichever way the flag is set, rather than the flag being refused or
/// silently dropping volumes.
#[tokio::test]
async fn include_managed_resources_reports_the_same_volumes_either_way() {
    let (router, _engine) = emulator();

    let mut created: Vec<String> = Vec::new();
    for (index, (volume_type, min_size)) in [("gp3", 1), ("sc1", 125), ("io1", 4)]
        .into_iter()
        .enumerate()
    {
        let case = format!("a {volume_type} volume of {min_size} GiB");
        created.push(create_volume(&router, &case, volume_type, min_size, index % 2).await);
    }
    created.sort();

    // The flag's own values, plus the request that omits it: all three must report the
    // same set, so the answer cannot depend on how the flag was written.
    let table: Vec<(&str, Params)> = vec![
        ("omitted", Vec::new()),
        ("false", vec![p("IncludeManagedResources", "false")]),
        ("true", vec![p("IncludeManagedResources", "true")]),
    ];
    for (case, params) in table {
        let reported = reported_ids(&router, case, &params).await;
        assert_eq!(
            reported, created,
            "case {case:?}: expected every stored volume {created:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1 (continued): the `io-enabled` check is the volume's OWN stored fact
// ---------------------------------------------------------------------------

/// The `io-enabled` check reports the stored `VolumeRecord::io_enabled` of the volume it
/// is about, and the overall status is derived from the checks: a volume whose I/O is
/// enabled is `passed`/`ok`, one whose I/O is disabled is `failed`/`impaired` — "if the
/// check fails, the overall status is impaired". `EnableVolumeIO` turning the I/O back on
/// moves the check back, so the report follows the store rather than a constant.
#[tokio::test]
async fn the_io_enabled_check_reports_the_volumes_stored_io_state() {
    let (router, engine) = emulator();

    // One volume per row, so each row's report is about its own stored fact and the rows
    // differ along exactly what the rule generalises over: whether that volume's I/O is
    // enabled. The types also differ, so neither answer is tied to one volume type.
    let enabled = create_volume(&router, "io enabled", "gp3", 1, 0).await;
    let disabled = create_volume(&router, "io disabled", "st1", 125, 1).await;
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard
            .volumes
            .get_mut(&disabled)
            .expect("the volume whose I/O is disabled is stored")
            .io_enabled = false;
    }

    let table: Vec<(&str, &String, &str, &str)> = vec![
        ("I/O enabled", &enabled, "passed", "ok"),
        ("I/O disabled", &disabled, "failed", "impaired"),
    ];
    for (case, volume_id, expected_check, expected_status) in table {
        let params = indexed("VolumeId", &[volume_id.as_str()]);
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let rows = status_rows(&body);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected one item, got {rows:?}"
        );
        assert_eq!(
            rows[0].details,
            vec![
                ("io-enabled".to_string(), expected_check.to_string()),
                ("io-performance".to_string(), "not-applicable".to_string()),
            ],
            "case {case:?}: expected the io-enabled check {expected_check:?}, got {:?}",
            rows[0]
        );
        assert_eq!(
            rows[0].status, expected_status,
            "case {case:?}: expected the overall status {expected_status:?}, got {:?}",
            rows[0]
        );
    }

    // `EnableVolumeIO` turns the I/O back on, and the check follows the store: the volume
    // reports `passed`/`ok` again, which is the round trip through the writer.
    let (enable_status, enable_body) = call(
        &router,
        REGION,
        "EnableVolumeIO",
        &[p("VolumeId", disabled.clone())],
    )
    .await;
    assert_eq!(
        enable_status, 200,
        "case \"EnableVolumeIO\": expected HTTP 200, got {enable_status} with body {enable_body}"
    );
    let params = indexed("VolumeId", &[disabled.as_str()]);
    let (status, body) = describe_status(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"after EnableVolumeIO\": expected HTTP 200, got {status} with body {body}"
    );
    let rows = status_rows(&body);
    assert_eq!(
        rows.len(),
        1,
        "case \"after EnableVolumeIO\": expected one item, got {rows:?}"
    );
    assert_eq!(
        rows[0].details,
        passing_checks(),
        "case \"after EnableVolumeIO\": expected the io-enabled check to pass again, got {:?}",
        rows[0]
    );
    assert_eq!(
        rows[0].status, "ok",
        "case \"after EnableVolumeIO\": expected the overall status ok, got {:?}",
        rows[0]
    );
}
