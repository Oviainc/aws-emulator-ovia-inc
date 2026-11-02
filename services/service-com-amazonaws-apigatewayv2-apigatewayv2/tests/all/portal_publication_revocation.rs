// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the whole publish → serve → `DisablePortal` → stop-serving
//! flow of a portal's own domain, driven the way a client drives it: through the address
//! `GetPortal` reports in `EndpointConfigurationResponse$PortalDefaultDomainName`.
//!
//! The rule these cases assert is about WHEN a revocation takes effect, which is not the
//! same question as when its status settles:
//!
//! > A call that REVOKES what an endpoint serves takes effect when it is accepted: the
//! > endpoint stops serving from that instant and keeps refusing on every later probe,
//! > whatever the resource's own status member reports meanwhile. The status reports what
//! > the SERVICE has left to do — `DisablePortal` answers `202`-style asynchronously
//! > (modelled `204` with no content, then `DISABLE_IN_PROGRESS` until the take-down comes
//! > due) — not whether public access is still granted. A call that GRANTS access is the
//! > other way round: `PublishPortal` answers `202`, so what it builds goes live only once
//! > the build the service owes has come due, and until then the domain serves whatever
//! > the last settled publish left.
//!
//! So a caller can tell from the domain itself that `DisablePortal` revoked public access,
//! on the FIRST probe after the call, rather than having to trust `PublishStatus`; and the
//! revocation holds — it does not blip back, not on a later probe and not once the span of
//! the take-down has passed.
//!
//! The rows vary what the rule generalises over: how many publications the portal had
//! before the disable (one, two, three — a disable takes a publication down, so there is
//! always at least one), how the control plane was ADDRESSED when the portal was created —
//! which decides the address the portal announces for its site, an AWS-style generated host
//! or this emulator's own endpoint — and how many times the site is re-probed while nothing
//! is called in between.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    PORTAL_BUILD, PUBLISH_STATUS_DISABLED, PUBLISH_STATUS_DISABLE_IN_PROGRESS,
    PUBLISH_STATUS_PUBLISHED, PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
};

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::portal_data_plane::{fetch, refused, served, FORBIDDEN, FORBIDDEN_MESSAGE};
use crate::portal_lifecycle::portal_content;
use crate::websocket_handshake_data_plane::Answer;

/// The region every request here is signed for, so the generated portal domain names it.
const REGION: &str = "eu-west-1";

/// This service's own AWS host: the endpoint a client that resolves AWS's hosts addresses.
const AWS_ENDPOINT: &str = "apigateway.eu-west-1.amazonaws.com";

/// The emulator's own endpoint, the only address a client on this machine resolves.
const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait on purpose — this workspace's `tokio` carries no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// Split the address the portal ANNOUNCES into the host a probe sends and the path it asks
/// for — what a client does with `https://{PortalDefaultDomainName}/`.
///
/// A generated AWS host carries the site at the root of the domain; the emulator's own
/// endpoint carries the portal's own path behind that host, because a client on this
/// machine resolves no other address.
fn site_address(announced: &str) -> (String, String) {
    match announced.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}/")),
        None => (announced.to_string(), "/".to_string()),
    }
}

/// One unauthenticated probe of the address the portal announced — a plain visit to a
/// website, carrying no AWS credential. The harness is `portal_data_plane`'s, so what a
/// page and a refusal look like is asserted in one place for both suites.
async fn probe(emulator: &MultiServiceRouter, announced: &str) -> Answer {
    let (host, path) = site_address(announced);
    fetch(emulator, &host, &path).await
}

/// Assert a probe found NOTHING served: not a 200 — the property a caller checking a
/// revocation cares about — and specifically the refusal a domain with no live
/// publication answers.
fn serves_nothing(case: &str, answer: &Answer) {
    assert_ne!(
        answer.status, 200,
        "case {case:?}: expected the revoked site not to be served, got {}",
        answer.summary()
    );
    refused(case, answer, FORBIDDEN, FORBIDDEN_MESSAGE);
}

/// `GetPortal`, and the `PublishStatus` it reports.
async fn publish_status(emulator: &MultiServiceRouter, case: &str, portal_id: &str) -> String {
    let (status, body) = call_at_host(
        emulator,
        AWS_ENDPOINT,
        REGION,
        "GET",
        &format!("/v2/portals/{portal_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetPortal, got {status} {body}"
    );
    as_str(&body, "publishStatus")
}

/// `PublishPortal`, asserted to answer the `202` its model declares.
async fn publish(emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
    let (status, body) = call_at_host(
        emulator,
        AWS_ENDPOINT,
        REGION,
        "POST",
        &format!("/v2/portals/{portal_id}/publish"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from PublishPortal, got {status} {body}"
    );
}

/// `DisablePortal`, asserted to answer the `204` its model declares with no body at all.
async fn disable(emulator: &MultiServiceRouter, case: &str, portal_id: &str) {
    let (status, body) = call_at_host(
        emulator,
        AWS_ENDPOINT,
        REGION,
        "DELETE",
        &format!("/v2/portals/{portal_id}/publish"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from DisablePortal, got {status} {body}"
    );
    assert!(
        body.is_null(),
        "case {case:?}: expected no content from DisablePortal, got {body}"
    );
}

/// One row of the flow: how the portal was created and addressed, how many publications
/// it had before the disable, and how hard the revocation is re-probed afterwards.
struct Row {
    case: &'static str,
    /// The endpoint the CREATE was addressed at, which decides the address the portal
    /// announces for its site.
    endpoint_host: &'static str,
    /// How many publishes are started AND left to come due before the disable. At least
    /// one, because a publication is what a disable takes down.
    settled_publishes: usize,
    /// How many times the site is re-probed after the revocation, with nothing called in
    /// between.
    hold_probes: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a settled publication taken down, announced on the generated AWS host",
            endpoint_host: AWS_ENDPOINT,
            settled_publishes: 1,
            hold_probes: 3,
        },
        Row {
            case: "a settled publication taken down, announced on the emulator's endpoint",
            endpoint_host: LOCAL_ENDPOINT,
            settled_publishes: 1,
            hold_probes: 2,
        },
        Row {
            case: "a portal published twice, then taken down",
            endpoint_host: AWS_ENDPOINT,
            settled_publishes: 2,
            hold_probes: 2,
        },
        Row {
            case: "a portal published three times, then taken down, announced on the \
                   emulator's endpoint",
            endpoint_host: LOCAL_ENDPOINT,
            settled_publishes: 3,
            hold_probes: 4,
        },
    ]
}

#[tokio::test]
async fn a_portals_site_is_published_then_revoked_on_the_address_the_portal_announces() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();

        let (status, created) = call_at_host(
            &emulator,
            row.endpoint_host,
            REGION,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Revoked Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let announced = as_str(
            created
                .get("endpointConfiguration")
                .unwrap_or(&Value::Null),
            "portalDefaultDomainName",
        );
        assert!(
            !announced.is_empty(),
            "case {case:?}: the create must report the domain the site is served on, got \
             {created}"
        );

        // Nothing has published this portal, so the address it announces serves nothing.
        let answer = probe(&emulator, &announced).await;
        serves_nothing(&format!("{case}: before the first publish"), &answer);

        // A publish GRANTS access, and its 202 says the service has only accepted the
        // work: nothing it builds is live until that build has come due.
        for publish_number in 1..=row.settled_publishes {
            publish(&emulator, case, &portal_id).await;
            assert_eq!(
                publish_status(&emulator, case, &portal_id).await,
                PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
                "case {case:?}: expected publish {publish_number} to be in progress on the \
                 next read"
            );
            let answer = probe(&emulator, &announced).await;
            if publish_number == 1 {
                // Nothing is live yet, and the accepted build is not live either.
                serves_nothing(
                    &format!("{case}: while the first publish is still being built"),
                    &answer,
                );
            } else {
                // A publish the service has only accepted changes nothing that is already
                // live: the build the LAST settled publish put up is still what is served.
                served(
                    &format!("{case}: while publish {publish_number} is still being built"),
                    &answer,
                    &portal_id,
                );
            }

            sleep(past_the_build());
            assert_eq!(
                publish_status(&emulator, case, &portal_id).await,
                PUBLISH_STATUS_PUBLISHED,
                "case {case:?}: expected publish {publish_number} to come due and report \
                 {PUBLISH_STATUS_PUBLISHED}"
            );
            let answer = probe(&emulator, &announced).await;
            served(
                &format!("{case}: once publish {publish_number} has settled"),
                &answer,
                &portal_id,
            );
        }

        // The take-down REVOKES access, so it applies when it is accepted: the very first
        // probe after the 204 finds nothing served, while the portal's own status still
        // reports the work the service has left to do.
        disable(&emulator, case, &portal_id).await;
        let answer = probe(&emulator, &announced).await;
        serves_nothing(&format!("{case}: the first probe after the disable"), &answer);
        assert_eq!(
            publish_status(&emulator, case, &portal_id).await,
            PUBLISH_STATUS_DISABLE_IN_PROGRESS,
            "case {case:?}: expected the disable to report the work the service has left \
             to do"
        );

        // And the revocation HOLDS: every later probe, with nothing called in between,
        // answers the same.
        for hold_probe in 1..=row.hold_probes {
            let answer = probe(&emulator, &announced).await;
            serves_nothing(
                &format!("{case}: re-probe {hold_probe} of the revoked site"),
                &answer,
            );
        }

        // It holds across the span too: once the take-down has come due the portal reports
        // the terminal status, and the site stays down.
        sleep(past_the_build());
        assert_eq!(
            publish_status(&emulator, case, &portal_id).await,
            PUBLISH_STATUS_DISABLED,
            "case {case:?}: expected the take-down to come due and report \
             {PUBLISH_STATUS_DISABLED}"
        );
        let answer = probe(&emulator, &announced).await;
        serves_nothing(
            &format!("{case}: once the take-down had come due"),
            &answer,
        );

        // A revocation is effective, not permanent: a later publish puts a build back
        // live on the same announced address.
        publish(&emulator, case, &portal_id).await;
        sleep(past_the_build());
        assert_eq!(
            publish_status(&emulator, case, &portal_id).await,
            PUBLISH_STATUS_PUBLISHED,
            "case {case:?}: expected a publish after the revocation to settle in \
             {PUBLISH_STATUS_PUBLISHED}"
        );
        let answer = probe(&emulator, &announced).await;
        served(
            &format!("{case}: republished after the revocation"),
            &answer,
            &portal_id,
        );
    }
}

/// The two halves of the rule side by side, on one portal: what a call GRANTS is live only
/// once the service has finished the work it accepted, while what a call REVOKES is gone
/// from the instant it is accepted — each asserted against the portal's own domain and
/// against the status it reports at the same moment.
///
/// A table over the two calls, so the asymmetry is asserted as a property of what the call
/// does rather than as two unrelated cases.
#[tokio::test]
async fn a_grant_waits_for_the_build_the_service_owes_and_a_revocation_does_not() {
    // (the call, whether what it changes is in effect on the first read after it answers,
    // the status the portal reports then, the status it settles in)
    let calls: [(&str, bool, &str, &str); 2] = [
        (
            "PublishPortal",
            false,
            PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
            PUBLISH_STATUS_PUBLISHED,
        ),
        (
            "DisablePortal",
            true,
            PUBLISH_STATUS_DISABLE_IN_PROGRESS,
            PUBLISH_STATUS_DISABLED,
        ),
    ];

    for (operation, in_effect_at_once, in_progress, settled) in calls {
        let case = format!("{operation} on a portal with a settled publication");
        let case = case.as_str();
        let emulator = emulator();
        let (status, created) = call_at_host(
            &emulator,
            AWS_ENDPOINT,
            REGION,
            "POST",
            "/v2/portals",
            Some(json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Asymmetric Portal")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
        );
        let portal_id = as_str(&created, "portalId");
        let announced = as_str(
            created
                .get("endpointConfiguration")
                .unwrap_or(&Value::Null),
            "portalDefaultDomainName",
        );

        // Both rows start from a portal whose publication is live, so the only difference
        // between them is what the call under test does.
        publish(&emulator, case, &portal_id).await;
        sleep(past_the_build());
        let answer = probe(&emulator, &announced).await;
        served(&format!("{case}: the publication under test"), &answer, &portal_id);

        // The call under test. A publish re-publishes the same content; a disable takes
        // the publication down.
        match operation {
            "PublishPortal" => publish(&emulator, case, &portal_id).await,
            _ => disable(&emulator, case, &portal_id).await,
        }

        // What the domain serves on the FIRST probe, and the status reported at the same
        // moment: the status says the service is still working either way, so it is the
        // domain that tells a caller whether access is still granted.
        let answer = probe(&emulator, &announced).await;
        if in_effect_at_once {
            serves_nothing(&format!("{case}: the first probe after the call"), &answer);
        } else {
            served(
                &format!("{case}: the first probe after the call"),
                &answer,
                &portal_id,
            );
        }
        assert_eq!(
            publish_status(&emulator, case, &portal_id).await,
            in_progress,
            "case {case:?}: expected the portal to report the work the service has left"
        );

        sleep(past_the_build());
        assert_eq!(
            publish_status(&emulator, case, &portal_id).await,
            settled,
            "case {case:?}: expected the clock to settle the portal in {settled}"
        );
        let answer = probe(&emulator, &announced).await;
        if in_effect_at_once {
            serves_nothing(&format!("{case}: once the call had come due"), &answer);
        } else {
            served(
                &format!("{case}: once the call had come due"),
                &answer,
                &portal_id,
            );
        }
    }
}
