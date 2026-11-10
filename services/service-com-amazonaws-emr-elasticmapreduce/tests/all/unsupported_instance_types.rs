// SPDX-License-Identifier: CC0-1.0

//! Launching a cluster resolves every instance type it names against the release it
//! names, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns for the launches
//! and the marker-following one `tests/all/supported_instance_types.rs` owns for the
//! catalog. Nothing reaches into the service's state struct: an accepted launch is read
//! back through `ListInstanceFleets` / `ListInstanceGroups`, and what a release offers is
//! read through `ListSupportedInstanceTypes`, so nothing below restates a catalog the
//! service already answers from.
//!
//! THE RULE: a cluster is provisioned from the instance types the release it runs offers
//! — "the list of instance types that the release specified … supports"
//! (`ListSupportedInstanceTypesOutput$SupportedInstanceTypes` doc) — so `RunJobFlow`
//! resolves EVERY instance type the launch names against that release's list and refuses
//! the whole launch with HTTP 400 `ValidationException` carrying
//! `Instance type '<type>' is not supported.`, writing no cluster. Every position that
//! names one is judged: each `InstanceTypeConfigs` entry of each instance fleet whatever
//! role it fills, each instance group, and the uniform
//! `MasterInstanceType`/`SlaveInstanceType` form. A launch that names no release label
//! names no release to resolve against, so nothing is judged for it.
//!
//! The rows span what the rule generalises over: the position in the request that names
//! the type, HOW a type misses the release's list (a size its family's ladder does not
//! carry, a size past the top of that ladder, a family Amazon EMR does not offer, a name
//! that is not `<family>.<size>` at all, a differently cased name), and the RELEASE, which
//! moves a type from unoffered to offered. Every expectation is cross-checked against what
//! `ListSupportedInstanceTypes` publishes for that release, so the table follows the
//! catalog rather than restating it. The empty string is no row of it: the member's
//! modelled minimum length refuses that at the boundary, before any handler sees it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};
use crate::supported_instance_types::{types, Emr as CatalogEmr};

/// The release every case launches against unless it is about release gating: the one the
/// recorded launch named.
const RELEASE: &str = "emr-5.36.1";

/// A type every catalog release offers, which fills every position of a request a case is
/// not exercising. Asserted against the published list before it is relied on.
const GOOD: &str = "m5.xlarge";

/// Where in the request the type under test is named.
#[derive(Clone, Copy, Debug)]
enum Position {
    /// The PRIMARY (`MASTER`) instance fleet's only instance type — the recorded case.
    PrimaryFleet,
    /// The second `InstanceTypeConfigs` entry of the primary fleet, beside an offered
    /// one: every entry is judged, not only the first.
    PrimaryFleetSecondType,
    /// The `CORE` instance fleet, beside an offered primary.
    CoreFleet,
    /// The `TASK` instance fleet, beside an offered primary and core.
    TaskFleet,
    /// An instance group's `InstanceType`, the other way a launch describes capacity.
    InstanceGroup,
    /// The uniform form's `MasterInstanceType`.
    UniformMaster,
    /// The uniform form's `SlaveInstanceType`.
    UniformSlave,
}

impl Position {
    /// Every position, so a case table spans them without listing them again.
    const ALL: [Self; 7] = [
        Self::PrimaryFleet,
        Self::PrimaryFleetSecondType,
        Self::CoreFleet,
        Self::TaskFleet,
        Self::InstanceGroup,
        Self::UniformMaster,
        Self::UniformSlave,
    ];
}

/// One instance fleet of `role`, provisioned from `instance_types`.
fn fleet(role: &str, instance_types: &[&str]) -> Value {
    json!({
        "Name": format!("{role}-fleet"),
        "InstanceFleetType": role,
        "TargetOnDemandCapacity": 1,
        "InstanceTypeConfigs": instance_types
            .iter()
            .map(|instance_type| json!({ "InstanceType": instance_type }))
            .collect::<Vec<Value>>(),
    })
}

/// A `RunJobFlow` body naming `instance_type` at `position`, with every other position
/// holding [`GOOD`], against `release` when one is given.
fn launch(name: &str, release: Option<&str>, position: Position, instance_type: &str) -> Value {
    let instances = match position {
        Position::PrimaryFleet => json!({
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [fleet("MASTER", &[instance_type]), fleet("CORE", &[GOOD])],
        }),
        Position::PrimaryFleetSecondType => json!({
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [fleet("MASTER", &[GOOD, instance_type])],
        }),
        Position::CoreFleet => json!({
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [fleet("MASTER", &[GOOD]), fleet("CORE", &[instance_type])],
        }),
        Position::TaskFleet => json!({
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [
                fleet("MASTER", &[GOOD]),
                fleet("CORE", &[GOOD]),
                fleet("TASK", &[instance_type]),
            ],
        }),
        Position::InstanceGroup => json!({
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceGroups": [
                { "Name": "primary", "InstanceRole": "MASTER", "InstanceType": GOOD,
                  "InstanceCount": 1 },
                { "Name": "core", "InstanceRole": "CORE", "InstanceType": instance_type,
                  "InstanceCount": 2 },
            ],
        }),
        Position::UniformMaster => json!({
            "InstanceCount": 2,
            "MasterInstanceType": instance_type,
            "SlaveInstanceType": GOOD,
        }),
        Position::UniformSlave => json!({
            "InstanceCount": 2,
            "MasterInstanceType": GOOD,
            "SlaveInstanceType": instance_type,
        }),
    };
    let mut body = json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": instances,
    });
    if let Some(release) = release {
        body["ReleaseLabel"] = json!(release);
    }
    body
}

/// Every instance type `release` offers, as a caller finds them: `ListSupportedInstanceTypes`
/// followed to its last page.
async fn published(release: &str) -> BTreeSet<String> {
    let catalog = CatalogEmr::new();
    types(&catalog.all_supported(release).await)
        .into_iter()
        .collect()
}

/// The cluster ids `ListClusters` reports.
async fn listed_clusters(emr: &Emr) -> Vec<String> {
    let payload = emr.ok("ListClusters", json!({})).await;
    at(&payload, &["Clusters"])
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| cluster["Id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every instance type the cluster `cluster_id` reports provisioning from, read back
/// through the two listings that project its capacity.
async fn provisioned_types(emr: &Emr, cluster_id: &str) -> BTreeSet<String> {
    let fleets = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
        .await;
    let groups = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": cluster_id }))
        .await;
    let from_fleets = at(&fleets, &["InstanceFleets"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|fleet| {
            at(&fleet, &["InstanceTypeSpecifications"])
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|specification| specification["InstanceType"].as_str().map(str::to_string));
    let from_groups = at(&groups, &["InstanceGroups"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|group| group["InstanceType"].as_str().map(str::to_string));
    from_fleets.chain(from_groups).collect()
}

/// Assert the launch `body` is refused for `instance_type`, leaving no cluster behind.
async fn assert_launch_refused(emr: &Emr, case: &str, body: Value, instance_type: &str) {
    let before = listed_clusters(emr).await;
    let (status, payload) = emr.call("RunJobFlow", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an instance type the release does not offer must be refused 400, \
         got {status} with {payload}"
    );
    let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
    assert!(
        code.contains("ValidationException"),
        "case {case:?}: expected ValidationException, got {code:?} in {payload}"
    );
    assert_eq!(
        at(&payload, &["message"])
            .as_str()
            .or_else(|| at(&payload, &["Message"]).as_str()),
        Some(format!("Instance type '{instance_type}' is not supported.").as_str()),
        "case {case:?}: expected the unsupported-instance-type sentence, got {payload}"
    );
    assert!(
        payload.get("JobFlowId").is_none(),
        "case {case:?}: a refused launch hands back no cluster id, got {payload}"
    );
    assert_eq!(
        listed_clusters(emr).await,
        before,
        "case {case:?}: a refused launch must write no cluster"
    );
}

/// Assert the launch `body` is served and the cluster it mints provisions from
/// `instance_type`.
async fn assert_launch_served(emr: &Emr, case: &str, body: Value, instance_type: &str) {
    let (status, payload) = emr.call("RunJobFlow", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an instance type the release offers must be served, got {status} \
         with {payload}"
    );
    let cluster_id = payload["JobFlowId"].as_str().unwrap_or_default().to_string();
    assert!(
        !cluster_id.is_empty(),
        "case {case:?}: a served launch must answer a JobFlowId, got {payload}"
    );
    let provisioned = provisioned_types(emr, &cluster_id).await;
    assert!(
        provisioned.contains(instance_type),
        "case {case:?}: the cluster must report provisioning from {instance_type:?}, got \
         {provisioned:?}"
    );
}

/// Every way an instance type can miss the list a release publishes, and the position in
/// the request it is named at is varied over [`Position::ALL`] for each of them.
///
/// SOURCE for the names: the published Amazon EC2 size ladders the service's instance-type
/// catalog is declared from (`ListSupportedInstanceTypes` reports exactly that catalog) —
/// `m5` steps `large, xlarge, 2xlarge, 4xlarge, …`, so `3xlarge` is no size of it, and
/// Amazon EMR offers no burstable (`t`) family for cluster nodes.
const UNOFFERED: [(&str, &str); 8] = [
    ("a size its family's ladder does not carry", "m5.3xlarge"),
    ("a size past the top of its family's ladder", "m5.64xlarge"),
    ("a family Amazon EMR does not offer", "t3.xlarge"),
    ("a family that does not exist", "nonsuch.xlarge"),
    ("a name with no size component", "m5"),
    ("a name with an empty size component", "m5."),
    ("a size with no family", "xlarge"),
    ("a differently cased name", "M5.XLARGE"),
];

#[tokio::test]
async fn a_launch_naming_an_instance_type_the_release_does_not_offer_is_refused() {
    let offered = published(RELEASE).await;
    assert!(
        offered.contains(GOOD),
        "the release {RELEASE} must offer {GOOD}, got {offered:?}"
    );
    let emr = Emr::new();

    for (label, instance_type) in UNOFFERED {
        assert!(
            !offered.contains(instance_type),
            "case {label:?}: {instance_type:?} must be absent from what {RELEASE} \
             publishes, got {offered:?}"
        );
        for position in Position::ALL {
            let case = format!("{label} ({instance_type:?}) at {position:?}");
            let body = launch("unoffered", Some(RELEASE), position, instance_type);
            assert_launch_refused(&emr, &case, body, instance_type).await;
        }
    }

    // The same positions holding a type the release DOES offer are served, and the
    // cluster reports provisioning from it: the refusal above is about the value, not
    // about the shape of the request.
    for position in Position::ALL {
        let case = format!("offered {GOOD:?} at {position:?}");
        let body = launch("offered", Some(RELEASE), position, GOOD);
        assert_launch_served(&emr, &case, body, GOOD).await;
    }
}

/// THE RULE's release half: the SAME instance type is unoffered by one release and
/// offered by another, and the launch follows the release it names.
///
/// The rows differ along which side of a family's introduction point the release falls,
/// and one is a `3xlarge` — a size `m5` has no entry for and `i3en` does — so the rule is
/// per family ladder rather than a blanket judgement on a size.
#[tokio::test]
async fn the_release_the_launch_names_decides_which_instance_types_it_may_name() {
    // (release, instance type): the expectation for each row is read off what the service
    // publishes for that release rather than written here.
    const ROWS: [(&str, &str); 6] = [
        // Graviton's seventh generation, which the 6.x series reached part way through.
        ("emr-6.14.0", "m7g.xlarge"),
        ("emr-6.15.0", "m7g.xlarge"),
        // The seventh-generation AMD families, which arrived with the 7.x series.
        ("emr-6.15.0", "c7a.8xlarge"),
        ("emr-7.1.0", "c7a.8xlarge"),
        // A `3xlarge` that IS on its family's ladder, beside the one that is not.
        ("emr-6.15.0", "i3en.3xlarge"),
        ("emr-6.15.0", "m5.3xlarge"),
    ];

    let emr = Emr::new();
    for (release, instance_type) in ROWS {
        let case = format!("{release} naming {instance_type:?}");
        let offered = published(release).await.contains(instance_type);
        let body = launch("release-gated", Some(release), Position::PrimaryFleet, instance_type);
        if offered {
            assert_launch_served(&emr, &case, body, instance_type).await;
        } else {
            assert_launch_refused(&emr, &case, body, instance_type).await;
        }
    }
}

/// A launch that names NO release label names no release to resolve its instance types
/// against — "for Amazon EMR releases 4.0 and later … use `AmiVersion` for releases 3.x
/// and 2.x" (`RunJobFlow$ReleaseLabel` doc) — so a type outside the release catalog is
/// accepted there, at every position.
#[tokio::test]
async fn a_launch_naming_no_release_label_resolves_no_instance_type() {
    // A legacy AMI-version cluster's instance type, which the release catalog (5.30 and
    // later) does not publish.
    const LEGACY: &str = "m1.small";

    let offered = published(RELEASE).await;
    assert!(
        !offered.contains(LEGACY),
        "the release catalog must not publish {LEGACY}, got {offered:?}"
    );
    let emr = Emr::new();
    for position in Position::ALL {
        let case = format!("no release label, {LEGACY:?} at {position:?}");
        let mut body = launch("legacy", None, position, LEGACY);
        body["AmiVersion"] = json!("2.4.2");
        assert_launch_served(&emr, &case, body, LEGACY).await;
    }
}
