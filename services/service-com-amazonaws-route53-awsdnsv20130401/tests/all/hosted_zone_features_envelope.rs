// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of ONE rule: every response that projects a
//! `HostedZone` serialises the modelled `Features` container for it.
//!
//! The rule under test: `HostedZone$Features` is part of the shape Route 53
//! projects, so every operation answering with a hosted zone carries the
//! container — empty for a zone whose features nobody has configured, and
//! carrying `AcceleratedRecoveryStatus` once an accepted
//! `UpdateHostedZoneFeatures` has configured one. A client deserialising
//! `HostedZone` into a strict shape therefore always finds the member, and
//! "no feature configured" is distinguishable from "not reported".
//!
//! One zone read by one operation is ONE row of that class, so the tables below
//! span it: zones differing in label count, in public/private, in whether they
//! carry a comment and in whether their features were ever configured, each
//! asserted on EVERY operation that projects a `HostedZone` — the create, the
//! single read, the comment update and both listings — because the rule is
//! about the projection of the shape, not about one operation.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the sibling hosted-zone suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCELERATED_RECOVERY_ENABLING, ACCELERATED_RECOVERY_PROPAGATION,
};

use crate::hosted_zone_records::{call, create_zone_body, escape_xml, Reply};
use crate::resource_tags::element_text;

/// How much longer than [`ACCELERATED_RECOVERY_PROPAGATION`] a test waits
/// before it expects an accepted features update to have settled, so the
/// assertion does not race the span itself.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// The members `HostedZoneFeatures` declares, from
/// `smithy_query(query="shape", shape="com.amazonaws.route53#HostedZoneFeatures")`:
/// the accelerated-recovery status and the failure reasons of a features
/// change. Neither has a default, so an unconfigured zone reports the container
/// with neither of them in it.
const FEATURES_MEMBERS: [&str; 2] = ["AcceleratedRecoveryStatus", "FailureReasons"];

/// A router with just this service mounted on a fresh shared engine.
fn router() -> MultiServiceRouter {
    crate::hosted_zone_records::router()
}

/// Create a zone with an arbitrary `HostedZoneConfig`/`VPC` block.
async fn create_zone(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    extra: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(name, caller_reference, extra),
    )
    .await
}

/// The bare (`Z…`) id from a reply carrying a `HostedZone`.
fn bare_zone_id(reply: &Reply) -> String {
    let id = reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body));
    id.trim_start_matches("/hostedzone/").to_string()
}

/// The contents of every `<Features>` container in a body, in document order.
///
/// Read out of the RAW body rather than out of the flattened document, because
/// the question this file asks is whether the CONTAINER is serialised — and an
/// empty container carries no text to flatten. `<Features/>` and
/// `<Features></Features>` both count, as the empty container they are.
fn features_blocks(body: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find("<Features") {
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix("<Features/>") {
            blocks.push("");
            rest = after;
        } else if let Some(inner) = tail.strip_prefix("<Features>") {
            let end = inner.find("</Features>").unwrap_or_else(|| {
                panic!("a Features container must be closed; body {body}")
            });
            blocks.push(&inner[..end]);
            rest = &inner[end + "</Features>".len()..];
        } else {
            panic!("unexpected element starting with <Features; body {body}");
        }
    }
    blocks
}

/// Assert a reply carries exactly `zones` `Features` containers — one per
/// hosted zone it projects — and hand them back.
fn features_of<'a>(case: &str, reply: &'a Reply, zones: usize) -> Vec<&'a str> {
    let blocks = features_blocks(&reply.body);
    assert_eq!(
        blocks.len(),
        zones,
        "case {case:?}: expected one Features container per projected hosted zone ({zones}), \
         got {}; body {}",
        blocks.len(),
        reply.body
    );
    blocks
}

/// Assert one `Features` container reports exactly `status` for
/// `AcceleratedRecoveryStatus` and nothing for the members the zone's state
/// does not fill.
fn assert_features(case: &str, block: &str, status: Option<&str>) {
    let reported = element_text(block, "AcceleratedRecoveryStatus");
    assert_eq!(
        reported.as_deref(),
        status,
        "case {case:?}: expected AcceleratedRecoveryStatus {status:?} in the Features \
         container, got {reported:?}; container {block:?}"
    );
    assert!(
        element_text(block, "FailureReasons").is_none(),
        "case {case:?}: a features change this service accepts reports no FailureReasons, \
         got {:?}; container {block:?}",
        element_text(block, "FailureReasons")
    );
    // Nothing the model does not declare is serialised inside the container.
    for element in element_names(block) {
        assert!(
            FEATURES_MEMBERS.contains(&element),
            "case {case:?}: {element:?} is not a HostedZoneFeatures member; container {block:?}"
        );
    }
}

/// The name of every element opened directly or indirectly inside one stretch
/// of XML, in document order.
fn element_names(block: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = block;
    while let Some(at) = rest.find('<') {
        let tail = &rest[at + 1..];
        let end = tail.find('>').unwrap_or_else(|| {
            panic!("the container has an unterminated tag; container {block:?}")
        });
        let tag = tail[..end].trim_start_matches('/').trim_end_matches('/');
        if let Some(name) = tag.split_whitespace().next() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        rest = &tail[end + 1..];
    }
    names
}

/// `GetHostedZone` for one zone.
async fn get_zone(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/hostedzone/{zone_id}"), "").await
}

/// `UpdateHostedZoneComment` for one zone, which answers with the zone too.
async fn update_comment(router: &MultiServiceRouter, zone_id: &str, comment: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        &format!(
            "<UpdateHostedZoneCommentRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\"><Comment>{}</Comment>\
             </UpdateHostedZoneCommentRequest>",
            escape_xml(comment)
        ),
    )
    .await
}

/// `UpdateHostedZoneFeatures` for one zone.
async fn update_features(router: &MultiServiceRouter, zone_id: &str, enable: bool) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/features"),
        &format!(
            "<UpdateHostedZoneFeaturesRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <EnableAcceleratedRecovery>{enable}</EnableAcceleratedRecovery>\
             </UpdateHostedZoneFeaturesRequest>"
        ),
    )
    .await
}

/// One row of the zone table: how the zone is created, and whether the feature
/// applies to it.
struct ZoneRow {
    case: &'static str,
    name: &'static str,
    /// The `HostedZoneConfig` (and, for a private zone, `VPC`) block sent.
    extra: &'static str,
}

/// The zones the tables below span: label count, public/private and comment
/// differ from row to row.
fn zone_rows() -> [ZoneRow; 6] {
    [
        ZoneRow {
            case: "a public zone created with no HostedZoneConfig",
            name: "features-plain.example.com",
            extra: "",
        },
        ZoneRow {
            case: "a public zone created with a comment",
            name: "features-commented.example.com",
            extra: "<HostedZoneConfig><Comment>commented</Comment></HostedZoneConfig>",
        },
        ZoneRow {
            case: "a three-label public zone",
            name: "zone.sub.features.example.net",
            extra: "",
        },
        ZoneRow {
            case: "a public zone named with a trailing dot",
            name: "features-dotted.example.org.",
            extra: "",
        },
        ZoneRow {
            case: "a private zone with one VPC",
            name: "features-private.example.com",
            extra: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
                    <VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-1a2b3c4d</VPCId></VPC>",
        },
        ZoneRow {
            case: "a private zone with a comment, in another region",
            name: "features-private-commented.example.co.uk",
            extra: "<HostedZoneConfig><Comment>private</Comment>\
                    <PrivateZone>true</PrivateZone></HostedZoneConfig>\
                    <VPC><VPCRegion>eu-west-1</VPCRegion><VPCId>vpc-9f8e7d6c</VPCId></VPC>",
        },
    ]
}

#[tokio::test]
async fn every_hosted_zone_projection_carries_the_features_container() {
    // The rule: the `Features` container is serialised for every projected
    // hosted zone, empty while nothing has configured the zone's features. The
    // rows span the zones; each one is asserted on the create, the single read
    // and the comment update, and the listings are asserted over all of them at
    // once.
    let router = router();
    let rows = zone_rows();
    let mut ids = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let created = create_zone(
            &router,
            row.name,
            &format!("features-envelope-{index}"),
            row.extra,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
            row.case, created.status, created.body
        );
        let blocks = features_of(row.case, &created, 1);
        assert_features(row.case, blocks[0], None);

        let zone_id = bare_zone_id(&created);
        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHostedZone, got {} with body {}",
            row.case, read.status, read.body
        );
        let read_blocks = features_of(row.case, &read, 1);
        assert_eq!(
            read_blocks, blocks,
            "case {:?}: GetHostedZone must report the Features the create did, got {:?} \
             after {:?}; body {}",
            row.case, read_blocks, blocks, read.body
        );

        let updated = update_comment(&router, &zone_id, "a later comment").await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateHostedZoneComment, got {} with body {}",
            row.case, updated.status, updated.body
        );
        let updated_blocks = features_of(row.case, &updated, 1);
        assert_features(row.case, updated_blocks[0], None);

        ids.push(zone_id);
    }

    // The same projection on both LIST paths: one container per listed zone.
    for (case, uri) in [
        ("ListHostedZones", "/2013-04-01/hostedzone"),
        (
            "ListHostedZonesByName",
            "/2013-04-01/hostedzonesbyname",
        ),
    ] {
        let listed = call(&router, "GET", uri, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            listed.status, listed.body
        );
        for block in features_of(case, &listed, ids.len()) {
            assert_features(case, block, None);
        }
    }
}

#[tokio::test]
async fn the_features_container_reports_the_configured_feature_on_every_projection() {
    // The rule's other half: the container is the zone's own state, so once an
    // accepted update has configured a zone, EVERY projection of that zone
    // reports the status in the same always-present container — while the zones
    // beside it still report it empty. The rows are the public zones of the
    // table above, which are the zones accelerated recovery applies to.
    let router = router();
    let rows = zone_rows();
    let mut configured = Vec::new();
    let mut untouched = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let created = create_zone(
            &router,
            row.name,
            &format!("features-configured-{index}"),
            row.extra,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
            row.case, created.status, created.body
        );
        let zone_id = bare_zone_id(&created);
        if row.extra.contains("<PrivateZone>true</PrivateZone>") {
            untouched.push((row.case, zone_id));
        } else {
            configured.push((row.case, zone_id));
        }
    }
    assert!(
        !configured.is_empty() && !untouched.is_empty(),
        "the table must span both configured and unconfigured zones"
    );

    for (case, zone_id) in &configured {
        let reply = update_features(&router, zone_id, true).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from UpdateHostedZoneFeatures, got {} with body {}",
            reply.status, reply.body
        );
        // Straight after the call the zone is still moving to the setting.
        let read = get_zone(&router, zone_id).await;
        let blocks = features_of(case, &read, 1);
        assert_features(case, blocks[0], Some(ACCELERATED_RECOVERY_ENABLING));
    }

    // A zone nobody configured still carries the container, empty.
    for (case, zone_id) in &untouched {
        let read = get_zone(&router, zone_id).await;
        let blocks = features_of(case, &read, 1);
        assert_features(case, blocks[0], None);
    }

    // And the listing projects both kinds: one container per zone, carrying the
    // status exactly for the zones that were configured.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    let blocks = features_of(
        "ListHostedZones",
        &listed,
        configured.len() + untouched.len(),
    );
    let with_status = blocks
        .iter()
        .filter(|block| element_text(block, "AcceleratedRecoveryStatus").is_some())
        .count();
    assert_eq!(
        with_status,
        configured.len(),
        "case {:?}: expected {} of the {} listed zones to report a status, got {with_status}; \
         body {}",
        "ListHostedZones",
        configured.len(),
        blocks.len(),
        listed.body
    );

    // The propagation span is the service's own, and it is never zero: a read
    // before it elapses saw the moving status above, and a read after it sees
    // the settled one in the same container.
    tokio::time::sleep(ACCELERATED_RECOVERY_PROPAGATION + SETTLE_MARGIN).await;
    for (case, zone_id) in &configured {
        let read = get_zone(&router, zone_id).await;
        let blocks = features_of(case, &read, 1);
        let status = element_text(blocks[0], "AcceleratedRecoveryStatus");
        assert!(
            status.is_some_and(|value| value != ACCELERATED_RECOVERY_ENABLING),
            "case {case:?}: the settled zone must report a settled status in its Features \
             container, got {:?}; body {}",
            element_text(blocks[0], "AcceleratedRecoveryStatus"),
            read.body
        );
    }
}
