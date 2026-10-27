// SPDX-License-Identifier: CC0-1.0

//! The TEXT of the two health-check refusals, end to end over the router.
//!
//! Both rules are about how a refusal's `Message` is BUILT, so each is asserted
//! over a table spanning what the rule generalises over rather than the one
//! request that exposed it:
//!
//! * an `UpdateHealthCheck` whose `HealthCheckVersion` is not the stored one is
//!   `HealthCheckVersionMismatch` (HTTP 409) whose message is REQUEST- and
//!   STATE-derived: it names the health check the request addressed, the version
//!   the health check currently holds and the version the request presented, so
//!   the read-modify-write loop the member documents is resolvable from the
//!   refusal alone;
//! * a `CreateHealthCheck` whose `CallerReference` already names a health check
//!   configured differently is `HealthCheckAlreadyExists` (HTTP 409) whose
//!   message is a CONSTANT carrying no request value at all — including the part
//!   a caller needs, that the reference stays held for some days after the
//!   health check that used it is deleted.
//!
//! The rows therefore vary the health check addressed, how the presented version
//! came to be stale (never current, one past, far past, stale after one or
//! several intervening updates) and, for the create, WHICH setting differs from
//! the health check the reference already names — across the health-check types
//! whose configuration carries that setting.
//!
//! The harness (`router`, `Reply`) and every request builder come from the
//! suites that own those operations, so these tests drive the service through
//! the same router a socket-served request goes through.

use service_com_amazonaws_route53_awsdnsv20130401::state::FIRST_HEALTH_CHECK_VERSION;

use crate::health_check_updates::{reported_version, update_health_check};
use crate::health_checks::{create_health_check, get_health_check, health_check_id};
use crate::hosted_zone_records::{router, Reply};
use crate::refusal_message_rules::message_of;

/// The error code a presented version that is not the stored one carries.
const VERSION_MISMATCH: &str = "HealthCheckVersionMismatch";

/// The error code a caller reference that already names a differently
/// configured health check carries.
const ALREADY_EXISTS: &str = "HealthCheckAlreadyExists";

/// The form a version-mismatch refusal is worded in, with the request's and the
/// state's own values filled in: the id the request addressed, the version the
/// health check holds, and the version the request presented.
fn version_mismatch_message(health_check_id: &str, held: i64, presented: i64) -> String {
    format!(
        "Health check with id {health_check_id} has version {held}, but expected to have \
         version {presented}. Could not proceed with update."
    )
}

/// The sentence a caller-reference collision is refused with — a CONSTANT, so it
/// is written out here once and every row is asserted against it.
const ALREADY_EXISTS_MESSAGE: &str =
    "A different health check has already been created with the specified caller reference. \
     If that health check has been deleted, Route 53 will still maintain the caller reference \
     for a number of days.";

/// An endpoint health check a row addresses, created fresh.
async fn fresh_health_check(
    router: &smithy_server_runtime::MultiServiceRouter,
    caller_reference: &str,
    case: &str,
) -> String {
    let created = create_health_check(
        router,
        caller_reference,
        "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from CreateHealthCheck, got {} with body {}",
        created.status, created.body
    );
    health_check_id(&created, case)
}

/// One refused update: how many updates are applied before it, and the version
/// it presents as an offset from the version stored at that point.
struct MismatchRow {
    case: &'static str,
    /// Updates applied before the refused one, so the stored version — and with
    /// it the message's middle value — differs per row.
    intervening: i64,
    /// What the refused request presents, as an offset from the stored version:
    /// negative for a version that went stale, positive for one the health check
    /// has not reached.
    offset: i64,
}

#[tokio::test]
async fn a_version_mismatch_names_the_health_check_and_both_versions() {
    // The rows differ along the stored version (a fresh check and one carrying
    // one, two and five applied updates) and along how far the presented
    // version is from it in both directions, including a version from before
    // several updates and one far ahead of anything the check has reached.
    // `HealthCheckVersion` is modelled with a minimum of 1
    // (smithy_query(query="shape",shape="HealthCheckVersion")), so no row
    // presents a value below that.
    const ROWS: &[MismatchRow] = &[
        MismatchRow {
            case: "one version past a freshly created health check",
            intervening: 0,
            offset: 1,
        },
        MismatchRow {
            case: "far past a freshly created health check",
            intervening: 0,
            offset: 999,
        },
        MismatchRow {
            case: "a version that went stale after one update",
            intervening: 1,
            offset: -1,
        },
        MismatchRow {
            case: "a version that went stale after two updates",
            intervening: 2,
            offset: -2,
        },
        MismatchRow {
            case: "the first version, five updates later",
            intervening: 5,
            offset: -5,
        },
        MismatchRow {
            case: "a version far ahead of a health check that has been updated",
            intervening: 3,
            offset: 999,
        },
    ];

    // One router for the whole table, so every row addresses a DIFFERENT health
    // check and a message built from a fixed id would fail.
    let router = router();
    let mut seen: Vec<String> = Vec::new();
    for row in ROWS {
        let id = fresh_health_check(&router, row.case, row.case).await;
        for step in 0..row.intervening {
            let applied =
                update_health_check(&router, &id, &format!("<Port>{}</Port>", 9000 + step)).await;
            assert_eq!(
                applied.status, 200,
                "case {:?}: the intervening update must succeed, got {} with body {}",
                row.case, applied.status, applied.body
            );
        }

        let held = reported_version(&get_health_check(&router, &id).await, row.case);
        let expected_held = FIRST_HEALTH_CHECK_VERSION + row.intervening;
        assert_eq!(
            held, expected_held,
            "case {:?}: expected the stored HealthCheckVersion to be {expected_held}, got {held}",
            row.case
        );
        let presented = held + row.offset;

        let refused = update_health_check(
            &router,
            &id,
            &format!(
                "<HealthCheckVersion>{presented}</HealthCheckVersion><Port>4433</Port>"
            ),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected HTTP 409, got {} with body {}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            VERSION_MISMATCH,
            "case {:?}: expected {VERSION_MISMATCH}, got {:?}; body {}",
            row.case,
            refused.error_code(),
            refused.body
        );

        let got = message_of(&refused, row.case);
        let expected = version_mismatch_message(&id, held, presented);
        assert_eq!(
            got, expected,
            "case {:?}: expected message {expected:?}, got {got:?}",
            row.case
        );
        // The id and the stored version are what the caller cannot know from
        // its own request, so they are the part that makes the refusal
        // actionable.
        assert!(
            got.contains(&id) && got.contains(&held.to_string()),
            "case {:?}: the message must name the health check {id} and its stored version \
             {held}, got {got:?}",
            row.case
        );
        assert!(
            !seen.contains(&got),
            "case {:?}: the message must be built per request, but {got:?} was already \
             answered to another row",
            row.case
        );
        seen.push(got);

        // The refusal changed nothing, so the version it quoted is still the
        // one the health check holds — which is what makes the quoted value
        // usable as the next request's token.
        let after = get_health_check(&router, &id).await;
        let still = reported_version(&after, row.case);
        assert_eq!(
            still, held,
            "case {:?}: the refused update must leave HealthCheckVersion at {held}, got {still}; \
             body {}",
            row.case, after.body
        );
        let retried = update_health_check(
            &router,
            &id,
            &format!("<HealthCheckVersion>{held}</HealthCheckVersion><Port>4433</Port>"),
        )
        .await;
        assert_eq!(
            retried.status, 200,
            "case {:?}: presenting the version the refusal quoted must be applied, got {} with \
             body {}",
            row.case, retried.status, retried.body
        );
    }
}

/// One caller-reference collision: the configuration the reference already
/// names, and the DIFFERENT configuration a second create sends under it.
struct CollisionRow {
    case: &'static str,
    /// The `CallerReference` both creates carry.
    caller_reference: &'static str,
    created: &'static str,
    differing: &'static str,
}

/// The collisions this suite drives, differing along WHICH setting of the
/// configuration the second create changed and along the health-check type that
/// carries it.
const COLLISIONS: &[CollisionRow] = &[
    CollisionRow {
        case: "a different Port on an HTTP check",
        caller_reference: "collision-port",
        created: "<Type>HTTP</Type><FullyQualifiedDomainName>a.example.com\
                  </FullyQualifiedDomainName><Port>80</Port>",
        differing: "<Type>HTTP</Type><FullyQualifiedDomainName>a.example.com\
                    </FullyQualifiedDomainName><Port>8080</Port>",
    },
    CollisionRow {
        case: "a different Type and Port",
        caller_reference: "collision-type",
        created: "<Type>HTTP</Type><FullyQualifiedDomainName>b.example.com\
                  </FullyQualifiedDomainName><Port>80</Port>",
        differing: "<Type>HTTPS</Type><FullyQualifiedDomainName>b.example.com\
                    </FullyQualifiedDomainName><Port>443</Port>",
    },
    CollisionRow {
        case: "a different IPAddress on a TCP check",
        caller_reference: "collision-address",
        created: "<Type>TCP</Type><IPAddress>192.0.2.10</IPAddress><Port>80</Port>",
        differing: "<Type>TCP</Type><IPAddress>192.0.2.11</IPAddress><Port>80</Port>",
    },
    CollisionRow {
        case: "a different ResourcePath",
        caller_reference: "collision-path",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.12</IPAddress><Port>80</Port>\
                  <ResourcePath>/first</ResourcePath>",
        differing: "<Type>HTTP</Type><IPAddress>192.0.2.12</IPAddress><Port>80</Port>\
                    <ResourcePath>/second</ResourcePath>",
    },
    CollisionRow {
        case: "a different FailureThreshold",
        caller_reference: "collision-threshold",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.13</IPAddress><Port>80</Port>\
                  <FailureThreshold>3</FailureThreshold>",
        differing: "<Type>HTTP</Type><IPAddress>192.0.2.13</IPAddress><Port>80</Port>\
                    <FailureThreshold>7</FailureThreshold>",
    },
    CollisionRow {
        case: "a different SearchString on a string-match check",
        caller_reference: "collision-search",
        created: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.14</IPAddress><Port>80</Port>\
                  <SearchString>healthy</SearchString>",
        differing: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.14</IPAddress><Port>80</Port>\
                    <SearchString>ready</SearchString>",
    },
    CollisionRow {
        case: "a different CloudWatch alarm",
        caller_reference: "collision-alarm",
        created: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                  <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
        differing: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>eu-west-1</Region>\
                    <Name>latency-high</Name></AlarmIdentifier>",
    },
];

/// The `Id` of the health check a reply reports, or `""`.
fn reported_id(reply: &Reply) -> &str {
    reply.first("HealthCheck/Id").unwrap_or("")
}

#[tokio::test]
async fn a_caller_reference_collision_is_refused_with_one_constant_sentence() {
    let router = router();
    for row in COLLISIONS {
        let created = create_health_check(&router, row.caller_reference, row.created).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);

        let refused = create_health_check(&router, row.caller_reference, row.differing).await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected HTTP 409, got {} with body {}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            ALREADY_EXISTS,
            "case {:?}: expected {ALREADY_EXISTS}, got {:?}; body {}",
            row.case,
            refused.error_code(),
            refused.body
        );

        let got = message_of(&refused, row.case);
        assert_eq!(
            got, ALREADY_EXISTS_MESSAGE,
            "case {:?}: expected message {ALREADY_EXISTS_MESSAGE:?}, got {got:?}",
            row.case
        );
        // A constant: whatever the request carried and whichever health check
        // the reference already names, neither appears in the sentence.
        assert!(
            !got.contains(row.caller_reference) && !got.contains(&id),
            "case {:?}: the message must name neither the caller reference {} nor the health \
             check {id}, got {got:?}",
            row.case,
            row.caller_reference
        );

        // The refusal left the health check the reference names exactly as it
        // was, so the sentence is the whole of what the collision did.
        let after = get_health_check(&router, &id).await;
        let version = reported_version(&after, row.case);
        assert_eq!(
            version, FIRST_HEALTH_CHECK_VERSION,
            "case {:?}: the refused create must leave HealthCheckVersion at \
             {FIRST_HEALTH_CHECK_VERSION}, got {version}; body {}",
            row.case, after.body
        );

        // The positive control: the SAME reference with the SAME configuration
        // is the request that was already served, so it reports that health
        // check rather than the collision sentence.
        let replayed = create_health_check(&router, row.caller_reference, row.created).await;
        assert_eq!(
            replayed.status, 201,
            "case {:?}: the identical create must be served, got {} with body {}",
            row.case, replayed.status, replayed.body
        );
        assert_eq!(
            reported_id(&replayed),
            id,
            "case {:?}: the identical create must report the health check {id}, got {:?}; body {}",
            row.case,
            reported_id(&replayed),
            replayed.body
        );
    }
}
