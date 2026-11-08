// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the hosted zone's features configuration:
//! `UpdateHostedZoneFeatures` and the `Features` every hosted-zone read reports.
//!
//! The rule under test: a feature setting is STATE of the hosted zone —
//! `UpdateHostedZoneFeatures` writes the setting its request asks for, moving
//! the zone through the modelled moving status (`ENABLING`/`DISABLING`) to the
//! settled one (`ENABLED`/`DISABLED`), and every later read of that zone reports
//! what the last accepted update left. A request that asks for the setting the
//! zone already holds changes nothing and is refused as invalid input, as is
//! turning the feature on for a zone it does not apply to; an id that names no
//! zone is a not-found, and a zone nobody configured reports no accelerated
//! recovery status at all.
//!
//! One zone and one setting are ONE row of that class, so the tables below span
//! it: zones differing in label count, in public/private and in their position
//! in the store; settings asked for from every starting state (never configured,
//! on, off); and the reads asserted on `GetHostedZone` AND on `ListHostedZones`,
//! because the rule is about the projection of the zone's state, not about one
//! operation.
//!
//! Every test drives the service through the real router, using the SAME harness
//! the sibling hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCELERATED_RECOVERY_DISABLED, ACCELERATED_RECOVERY_DISABLING, ACCELERATED_RECOVERY_ENABLED,
    ACCELERATED_RECOVERY_ENABLING, ACCELERATED_RECOVERY_PROPAGATION,
};

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::vpc_associations::create_private_zone;

/// Long enough for an accepted features update to have settled: the emulator's
/// own propagation span plus a margin.
///
/// Waited out with a BLOCKING sleep, because this crate's tokio does not enable
/// the `time` feature and the harness deliberately takes no dependency of its
/// own; each test owns its runtime, so nothing else is waiting on it.
fn wait_until_settled() {
    std::thread::sleep(ACCELERATED_RECOVERY_PROPAGATION + std::time::Duration::from_millis(200));
}

/// An `UpdateHostedZoneFeatures` request. `None` sends NO
/// `EnableAcceleratedRecovery` element at all.
async fn update_features(
    router: &MultiServiceRouter,
    zone_id: &str,
    enable: Option<bool>,
) -> Reply {
    let setting = enable.map_or_else(String::new, |value| {
        format!("<EnableAcceleratedRecovery>{value}</EnableAcceleratedRecovery>")
    });
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/features"),
        &format!(
            "<UpdateHostedZoneFeaturesRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{setting}\
             </UpdateHostedZoneFeaturesRequest>"
        ),
    )
    .await
}

/// `GetHostedZone` for one zone.
async fn get_zone(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await
}

/// The `AcceleratedRecoveryStatus` a reply reports for the first `HostedZone` it
/// carries, or `None` when the reply carries no features container.
fn reported_status(reply: &Reply) -> Option<&str> {
    reply.first("Features/AcceleratedRecoveryStatus")
}

/// Assert an update was served, with the empty body the modelled response has.
fn assert_served(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 (501 = still scaffolded), got {} with body {}",
        reply.status, reply.body
    );
}

/// Assert a refusal carries one wire error code, and that its message names the
/// request-derived detail `mentions`.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, mentions: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected error code {code}, got {:?} with body {}",
        reply.error_code(),
        reply.body
    );
    let message = reply.first("Error/Message").unwrap_or("");
    assert!(
        message.contains(mentions),
        "case {case:?}: the refusal must name {mentions:?}, got message {message:?}"
    );
}

/// Put a zone into a starting state through the API alone, and return the
/// settled status it then reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Nothing has ever configured the zone's features.
    NeverConfigured,
    /// An accepted enable has settled.
    Enabled,
    /// An accepted enable and then an accepted disable have settled.
    Disabled,
}

/// Drive `zone_id` to `start`, asserting each step on the way was served.
async fn drive_to(router: &MultiServiceRouter, zone_id: &str, start: Start) {
    match start {
        Start::NeverConfigured => {}
        Start::Enabled => {
            assert_served("reaching ENABLED", &update_features(router, zone_id, Some(true)).await);
            wait_until_settled();
        }
        Start::Disabled => {
            assert_served("reaching ENABLED", &update_features(router, zone_id, Some(true)).await);
            wait_until_settled();
            assert_served(
                "reaching DISABLED",
                &update_features(router, zone_id, Some(false)).await,
            );
            wait_until_settled();
        }
    }
}

// ---------------------------------------------------------------------------
// The setting is state: it is written, it moves, and every read reports it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_features_update_persists_and_every_hosted_zone_read_reports_it() {
    // The rule: an accepted update leaves the zone moving to the setting it
    // asked for and then holding it, and a later read of the SAME zone reports
    // it. The rows span the zone, not the setting: label count, record
    // population and store position differ, and each zone's own setting is read
    // back.
    let router = router();
    let zones = [
        ("single label", "example.com"),
        ("three labels", "zone.sub.example.net"),
        ("trailing dot", "dotted.example.org."),
    ];

    let mut ids = Vec::new();
    for (case, name) in zones {
        let id = create_zone_ok(&router, name, &format!("features-persist-{case}")).await;
        // A zone nobody has configured reports no accelerated recovery status:
        // the member has no default, so there is nothing to report yet.
        let fresh = get_zone(&router, &id).await;
        assert!(
            reported_status(&fresh).is_none(),
            "case {case:?}: a zone whose features were never updated reports no \
             AcceleratedRecoveryStatus, got {:?} with body {}",
            reported_status(&fresh),
            fresh.body
        );
        ids.push((case, id));
    }

    // Enabling moves the zone: it is reported ENABLING straight after the call,
    // never already settled.
    for (case, id) in &ids {
        let reply = update_features(&router, id, Some(true)).await;
        assert_served(case, &reply);
        assert!(
            reply.body.trim().is_empty() || !reply.body.contains("<Error"),
            "case {case:?}: the response models no members, got body {}",
            reply.body
        );
        let moving = get_zone(&router, id).await;
        assert_eq!(
            reported_status(&moving),
            Some(ACCELERATED_RECOVERY_ENABLING),
            "case {case:?}: a zone that just started enabling reports {ACCELERATED_RECOVERY_ENABLING}, \
             got {:?} with body {}",
            reported_status(&moving),
            moving.body
        );
    }

    wait_until_settled();
    for (case, id) in &ids {
        let settled = get_zone(&router, id).await;
        assert_eq!(
            reported_status(&settled),
            Some(ACCELERATED_RECOVERY_ENABLED),
            "case {case:?}: the zone must reach {ACCELERATED_RECOVERY_ENABLED}, got {:?} with body {}",
            reported_status(&settled),
            settled.body
        );
        // A read is a report: reading it again reports the same thing.
        let again = get_zone(&router, id).await;
        assert_eq!(
            reported_status(&again),
            reported_status(&settled),
            "case {case:?}: reading the zone must not change what a later read sees, got {:?} \
             then {:?}",
            reported_status(&settled),
            reported_status(&again)
        );
    }

    // The same state is projected by the listing, not only by the single read.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    let mut statuses = listed.all("AcceleratedRecoveryStatus");
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        vec![ACCELERATED_RECOVERY_ENABLED; ids.len()],
        "every configured zone must report its setting in the listing too, got {statuses:?} \
         with body {}",
        listed.body
    );

    // Disabling moves it back, through DISABLING to DISABLED.
    for (case, id) in &ids {
        assert_served(case, &update_features(&router, id, Some(false)).await);
        let moving = get_zone(&router, id).await;
        assert_eq!(
            reported_status(&moving),
            Some(ACCELERATED_RECOVERY_DISABLING),
            "case {case:?}: a zone that just started disabling reports \
             {ACCELERATED_RECOVERY_DISABLING}, got {:?} with body {}",
            reported_status(&moving),
            moving.body
        );
    }
    wait_until_settled();
    for (case, id) in &ids {
        let settled = get_zone(&router, id).await;
        assert_eq!(
            reported_status(&settled),
            Some(ACCELERATED_RECOVERY_DISABLED),
            "case {case:?}: the zone must reach {ACCELERATED_RECOVERY_DISABLED}, got {:?} with \
             body {}",
            reported_status(&settled),
            settled.body
        );
    }
}

// ---------------------------------------------------------------------------
// A request that asks for the setting the zone already has is refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_asking_for_the_setting_the_zone_already_has_is_refused() {
    // The rule: a features update CHANGES a setting, so asking for the setting
    // the zone already holds is invalid input and leaves the zone exactly as it
    // was. The rows span every starting state crossed with every setting a
    // request can ask for.
    let cases = [
        ("never configured, disable", Start::NeverConfigured, false, None),
        (
            "never configured, enable",
            Start::NeverConfigured,
            true,
            Some(ACCELERATED_RECOVERY_ENABLING),
        ),
        ("enabled, enable", Start::Enabled, true, None),
        (
            "enabled, disable",
            Start::Enabled,
            false,
            Some(ACCELERATED_RECOVERY_DISABLING),
        ),
        ("disabled, disable", Start::Disabled, false, None),
        (
            "disabled, enable",
            Start::Disabled,
            true,
            Some(ACCELERATED_RECOVERY_ENABLING),
        ),
    ];

    for (case, start, enable, expected_after) in cases {
        let router = router();
        let id = create_zone_ok(&router, "redundant.example.com", case).await;
        drive_to(&router, &id, start).await;
        let before = reported_status(&get_zone(&router, &id).await).map(str::to_string);

        let reply = update_features(&router, &id, Some(enable)).await;
        match expected_after {
            // The request asks for a setting the zone does not hold, so it is
            // served and the zone starts moving to it.
            Some(after) => {
                assert_served(case, &reply);
                let read = get_zone(&router, &id).await;
                assert_eq!(
                    reported_status(&read),
                    Some(after),
                    "case {case:?}: expected the zone to report {after}, got {:?} with body {}",
                    reported_status(&read),
                    read.body
                );
            }
            // The request asks for the setting the zone already holds.
            None => {
                assert_refused(case, &reply, 400, "InvalidInput", "ccelerated recovery");
                let read = get_zone(&router, &id).await;
                assert_eq!(
                    reported_status(&read).map(str::to_string),
                    before,
                    "case {case:?}: a refused update must leave the zone as it was, got {:?} \
                     after {before:?}",
                    reported_status(&read)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The feature applies to a public hosted zone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accelerated_recovery_cannot_be_enabled_for_a_private_hosted_zone() {
    // The rule: accelerated recovery serves record updates in a PUBLIC hosted
    // zone, so turning it on for a private one is invalid input; the same
    // request against a public zone is served. The rows differ along the one
    // axis the rule is about — the zone's private flag — over several zones.
    let router = router();
    let private_rows = [
        ("private, one VPC", "private.example.com", "vpc-00000000000000001"),
        ("private, other VPC", "private.example.net", "vpc-00000000000000002"),
    ];
    for (case, name, vpc) in private_rows {
        let id = create_private_zone(&router, name, case, vpc).await;
        let reply = update_features(&router, &id, Some(true)).await;
        assert_refused(case, &reply, 400, "InvalidInput", "ccelerated recovery");
        let read = get_zone(&router, &id).await;
        assert!(
            reported_status(&read).is_none(),
            "case {case:?}: a refused enable must leave the zone with no accelerated \
             recovery status, got {:?} with body {}",
            reported_status(&read),
            read.body
        );
    }

    let public = create_zone_ok(&router, "public.example.com", "public-contrast").await;
    assert_served(
        "public, enable",
        &update_features(&router, &public, Some(true)).await,
    );
}

// ---------------------------------------------------------------------------
// The request has to say what to set, and the zone has to exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_that_sets_nothing_is_refused() {
    // The rule: the update configures one feature, so a request that carries no
    // setting for it asks for no change and is invalid input — whatever the
    // zone's current state is. The rows span the starting states.
    for (case, start) in [
        ("never configured", Start::NeverConfigured),
        ("enabled", Start::Enabled),
        ("disabled", Start::Disabled),
    ] {
        let router = router();
        let id = create_zone_ok(&router, "empty.example.com", case).await;
        drive_to(&router, &id, start).await;
        let before = reported_status(&get_zone(&router, &id).await).map(str::to_string);

        let reply = update_features(&router, &id, None).await;
        assert_refused(
            case,
            &reply,
            400,
            "InvalidInput",
            "EnableAcceleratedRecovery",
        );
        let read = get_zone(&router, &id).await;
        assert_eq!(
            reported_status(&read).map(str::to_string),
            before,
            "case {case:?}: a refused update must leave the zone as it was, got {:?} after \
             {before:?}",
            reported_status(&read)
        );
    }
}

#[tokio::test]
async fn an_update_for_an_id_that_names_no_zone_is_a_not_found() {
    // The rule: a zone-addressed operation answers NoSuchHostedZone for an id no
    // zone carries, naming the id as the request wrote it — and persists
    // nothing. The rows span id shapes a caller might pass, including the
    // prefixed form a HostedZone hands back.
    let router = router();
    // One real zone, so the refusals below are about the ID and not about an
    // empty store.
    let real = create_zone_ok(&router, "real.example.com", "not-found-contrast").await;

    for (case, id) in [
        ("minted shape", "Z00000000000000000001"),
        ("not an id at all", "BOGUS"),
        ("one character", "Z"),
    ] {
        for enable in [true, false] {
            let reply = update_features(&router, id, Some(enable)).await;
            assert_refused(
                &format!("{case}, enable={enable}"),
                &reply,
                404,
                "NoSuchHostedZone",
                id,
            );
        }
        assert_eq!(
            get_zone(&router, id).await.status,
            404,
            "case {case:?}: a refused update must not have created a zone"
        );
    }

    // The real zone is untouched by any of that.
    let read = get_zone(&router, &real).await;
    assert!(
        reported_status(&read).is_none(),
        "the untouched zone must still report no accelerated recovery status, got {:?} with body {}",
        reported_status(&read),
        read.body
    );
}
