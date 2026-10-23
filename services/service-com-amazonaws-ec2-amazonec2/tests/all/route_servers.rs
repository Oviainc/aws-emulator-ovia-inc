// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC route server slice: `CreateRouteServer` and
//! `DescribeRouteServers`, driven over the wire the service actually speaks — `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateRouteServer` MINTS A ROUTE SERVER FROM THE REQUEST and `DescribeRouteServers`
//!    REPORTS THE STORE: every member the request sets round-trips, and the members the
//!    service owns (the id, the SNS topic ARN, the two states) are derived rather than
//!    taken;
//! 2. a route server is `pending` and the CLOCK, not a caller, moves it to `available` —
//!    and route persistence that was asked for is `enabling` until the same clock settles
//!    it;
//! 3. a parameter outside what the model documents is refused with EC2's own error code,
//!    and nothing is stored;
//! 4. one `ClientToken` names one route server;
//! 5. `DryRun` answers `DryRunOperation` and writes nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — the ASN at
//! each published bound and one past it, each persistence action, each duration in the
//! published range, which filter, which page size — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    AMAZON_SIDE_ASN_RANGE, MAX_TAGS_PER_RESOURCE, PERSIST_ROUTES_DURATION_RANGE,
    ROUTE_SERVER_ID_PREFIX, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for; the SNS topic ARN the assertions read is
/// derived from it.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Params, &'static str);

/// One row of the persistence table: `(case, the parameters, the `persistRoutesState` the
/// create reports, the `persistRoutesState` the clock settles on)`.
type PersistRow = (&'static str, Params, &'static str, &'static str);

/// One row of the filter table: `(case, the filter parameters, the ids it must select)`.
type FilterRow = (&'static str, Params, Vec<String>);

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateRouteServer", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeRouteServers", params).await
}

/// A route server id is `rs-` followed by 8 or 17 lowercase hex digits.
fn is_route_server_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix(&format!("{ROUTE_SERVER_ID_PREFIX}-")) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Create one route server with `asn` and whatever extra parameters the case needs.
async fn create_route_server(
    router: &MultiServiceRouter,
    asn: i64,
    extra: Params,
) -> (u16, String) {
    let mut params = vec![p("AmazonSideAsn", asn.to_string())];
    params.extend(extra);
    create(router, &params).await
}

/// Create one route server that must succeed, and answer its id.
async fn created_id(router: &MultiServiceRouter, case: &str, asn: i64, extra: Params) -> String {
    let (status, body) = create_route_server(router, asn, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateRouteServer, got {status} with body {body}"
    );
    element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"))
}

/// The ids `DescribeRouteServers` reports for `params`, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeRouteServers, got {status} \
         with body {body}"
    );
    elements(&body, "routeServerId")
}

/// RULE 1 over the ASN range: every value the member documents is accepted, round-trips
/// through the create response AND through the describe that reads the store back, and the
/// id the service minted is its own.
#[tokio::test]
async fn create_mints_a_route_server_for_every_documented_asn() {
    let (asn_min, asn_max) = AMAZON_SIDE_ASN_RANGE;
    // Source: "Valid values are from 1 to 4294967295. We recommend using a private ASN in
    // the 64512–65534 (16-bit ASN) or 4200000000–4294967294 (32-bit ASN) range"
    // (`CreateRouteServerRequest$AmazonSideAsn`) — both published bounds and one value
    // from each recommended private range.
    let cases: [(&str, i64); 5] = [
        ("the bottom of the published range", asn_min),
        ("a 16-bit private ASN", 64_512),
        ("the top of the 16-bit private range", 65_534),
        ("a 32-bit private ASN", 4_200_000_000),
        ("the top of the published range", asn_max),
    ];

    let router = emulator();
    for (case, asn) in cases {
        let (status, body) = create_route_server(&router, asn, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for amazonSideAsn {asn}, got {status} \
             with body {body}"
        );
        let id = element(&body, "routeServerId").unwrap_or_default();
        assert!(
            is_route_server_id(&id),
            "case {case:?}: expected a minted route server id, got {id:?}"
        );
        let reported = element(&body, "amazonSideAsn").unwrap_or_default();
        assert_eq!(
            reported,
            asn.to_string(),
            "case {case:?}: expected amazonSideAsn {asn} back, got {reported:?}"
        );

        // The store, not the request object, is what a later reader sees.
        let (status, body) = describe(&router, &indexed("RouteServerId", &[&id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeRouteServers, got {status} \
             with body {body}"
        );
        let described = element(&body, "amazonSideAsn").unwrap_or_default();
        assert_eq!(
            described,
            asn.to_string(),
            "case {case:?}: expected DescribeRouteServers to report amazonSideAsn {asn}, \
             got {described:?}"
        );
    }
}

/// RULE 3 over the same range: a value outside the published bounds is refused with EC2's
/// own code, and the store is left as it was.
#[tokio::test]
async fn an_asn_outside_the_published_range_is_refused() {
    let (asn_min, asn_max) = AMAZON_SIDE_ASN_RANGE;
    let cases: [(&str, i64); 3] = [
        ("one below the bottom bound", asn_min - 1),
        ("a negative ASN", -1),
        ("one above the top bound", asn_max + 1),
    ];

    let router = emulator();
    for (case, asn) in cases {
        let (status, body) = create_route_server(&router, asn, Vec::new()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for amazonSideAsn {asn}, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {body}"
        );
    }
    let stored = described_ids(&router, "after every refusal", &[]).await;
    assert!(
        stored.is_empty(),
        "case \"after every refusal\": expected an empty store, got {stored:?}"
    );
}

/// RULE 1 and RULE 2 over the persistence actions: the action the request names decides the
/// `persistRoutesState` the create reports and the one the clock settles on, and the
/// duration round-trips only where the model says it is valid.
#[tokio::test]
async fn persistence_states_follow_the_requested_action() {
    let (duration_min, duration_max) = PERSIST_ROUTES_DURATION_RANGE;
    let mut cases: Vec<PersistRow> = vec![
        ("no persistence asked for", Vec::new(), "disabled", "disabled"),
        (
            "persistence disabled",
            vec![p("PersistRoutes", "disable")],
            "disabled",
            "disabled",
        ),
        (
            "persistence reset on a route server that has none",
            vec![p("PersistRoutes", "reset")],
            "disabled",
            "disabled",
        ),
    ];
    // Every duration the member publishes: "Value must be in the range of 1-5"
    // (`CreateRouteServerRequest$PersistRoutesDuration`).
    for duration in duration_min..=duration_max {
        cases.push((
            "persistence enabled",
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", duration.to_string()),
            ],
            "enabling",
            "enabled",
        ));
    }

    let router = emulator();
    for (case, extra, at_create, after_the_clock) in cases {
        let asked_duration = extra
            .iter()
            .find(|(key, _)| key == "PersistRoutesDuration")
            .map(|(_, value)| value.clone());
        let (status, body) = create_route_server(&router, 64_512, extra.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({extra:?}): expected HTTP 200, got {status} with body {body}"
        );
        let id = element(&body, "routeServerId").unwrap_or_default();
        let reported = element(&body, "persistRoutesState").unwrap_or_default();
        assert_eq!(
            reported, at_create,
            "case {case:?} ({extra:?}): expected persistRoutesState {at_create:?} at create, \
             got {reported:?}"
        );
        let reported_duration = element(&body, "persistRoutesDuration");
        assert_eq!(
            reported_duration, asked_duration,
            "case {case:?} ({extra:?}): expected persistRoutesDuration {asked_duration:?}, \
             got {reported_duration:?}"
        );

        past_the_transition().await;
        let (status, body) = describe(&router, &indexed("RouteServerId", &[&id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeRouteServers, got {status} \
             with body {body}"
        );
        let settled = element(&body, "persistRoutesState").unwrap_or_default();
        assert_eq!(
            settled, after_the_clock,
            "case {case:?} ({extra:?}): expected persistRoutesState {after_the_clock:?} once \
             the clock had settled it, got {settled:?}"
        );
    }
}

/// RULE 2: the route server a create answers with is `pending`, and nothing but the clock
/// moves it to `available`.
#[tokio::test]
async fn the_clock_moves_a_route_server_to_available() {
    let router = emulator();
    let (status, body) = create_route_server(&router, 64_512, Vec::new()).await;
    assert_eq!(
        status, 200,
        "case \"a fresh create\": expected HTTP 200, got {status} with body {body}"
    );
    let id = element(&body, "routeServerId").unwrap_or_default();
    let at_create = element(&body, "state").unwrap_or_default();
    assert_eq!(
        at_create, "pending",
        "case \"a fresh create\": expected state \"pending\", got {at_create:?}"
    );

    // Read straight back: the transient state is still observable, so a caller acting on
    // the create's answer sees the same word the create reported.
    let (_, body) = describe(&router, &indexed("RouteServerId", &[&id])).await;
    let at_once = element(&body, "state").unwrap_or_default();
    assert_eq!(
        at_once, "pending",
        "case \"read back at once\": expected state \"pending\", got {at_once:?}"
    );

    past_the_transition().await;
    let (_, body) = describe(&router, &indexed("RouteServerId", &[&id])).await;
    let after = element(&body, "state").unwrap_or_default();
    assert_eq!(
        after, "available",
        "case \"after the owed span\": expected state \"available\", got {after:?}"
    );
}

/// RULE 3 over the persistence parameters: each combination the model documents as invalid
/// is refused with the code EC2 answers for that kind of refusal.
#[tokio::test]
async fn invalid_persistence_parameters_are_refused() {
    let (duration_min, duration_max) = PERSIST_ROUTES_DURATION_RANGE;
    let cases: [RefusalRow; 6] = [
        (
            "a duration below the published range",
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (duration_min - 1).to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "a duration above the published range",
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (duration_max + 1).to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "persistence enabled with no duration",
            vec![p("PersistRoutes", "enable")],
            "MissingParameter",
        ),
        (
            "a duration with persistence disabled",
            vec![
                p("PersistRoutes", "disable"),
                p("PersistRoutesDuration", duration_min.to_string()),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a duration with no persistence action at all",
            vec![p("PersistRoutesDuration", duration_max.to_string())],
            "InvalidParameterCombination",
        ),
        (
            "a tag specification naming another resource type",
            vec![
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "InvalidParameterValue",
        ),
    ];

    let router = emulator();
    for (case, extra, expected) in cases {
        let (status, body) = create_route_server(&router, 64_512, extra.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({extra:?}): expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?} ({extra:?}): expected {expected}, got {code:?} with body {body}"
        );
    }
    let stored = described_ids(&router, "after every invalid create", &[]).await;
    assert!(
        stored.is_empty(),
        "case \"after every invalid create\": expected an empty store, got {stored:?}"
    );
}

/// RULE 1: the SNS topic is the SERVICE's, named for the route server it belongs to, and it
/// exists only for a route server whose notifications were enabled.
#[tokio::test]
async fn the_sns_topic_is_reported_only_when_notifications_are_enabled() {
    let cases: [(&str, Params, bool); 3] = [
        ("notifications not asked for", Vec::new(), false),
        (
            "notifications off",
            vec![p("SnsNotificationsEnabled", "false")],
            false,
        ),
        (
            "notifications on",
            vec![p("SnsNotificationsEnabled", "true")],
            true,
        ),
    ];

    let router = emulator();
    for (case, extra, enabled) in cases {
        let (status, body) = create_route_server(&router, 64_512, extra.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let id = element(&body, "routeServerId").unwrap_or_default();
        let reported = element(&body, "snsNotificationsEnabled").unwrap_or_default();
        assert_eq!(
            reported,
            enabled.to_string(),
            "case {case:?}: expected snsNotificationsEnabled {enabled}, got {reported:?}"
        );
        let topic = element(&body, "snsTopicArn");
        match enabled {
            true => {
                let topic = topic.unwrap_or_default();
                assert!(
                    topic.starts_with(&format!("arn:aws:sns:{REGION}:"))
                        && topic.ends_with(&id),
                    "case {case:?}: expected an SNS topic ARN of this Region naming {id}, \
                     got {topic:?}"
                );
            }
            false => assert!(
                topic.is_none(),
                "case {case:?}: expected no snsTopicArn, got {topic:?}"
            ),
        }
    }
}

/// RULE 1: `TagSpecifications` tags the route server the call creates, and the describe
/// reads those tags back off the store.
#[tokio::test]
async fn tags_round_trip_through_the_store() {
    let cases: [(&str, Vec<(&str, &str)>); 3] = [
        ("no tags", Vec::new()),
        ("one tag", vec![("Name", "border")]),
        (
            "three tags, one with an empty value",
            vec![("Name", "border"), ("Tier", "edge"), ("Note", "")],
        ),
    ];

    let router = emulator();
    for (case, tags) in cases {
        let mut extra = vec![p("TagSpecification.1.ResourceType", "route-server")];
        for (index, (key, value)) in tags.iter().enumerate() {
            extra.push(p(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                *key,
            ));
            extra.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
        let id = created_id(&router, case, 64_512, extra).await;

        let (_, body) = describe(&router, &indexed("RouteServerId", &[&id])).await;
        let keys = elements(&body, "key");
        let expected: Vec<String> = tags.iter().map(|(key, _)| (*key).to_string()).collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected the tag keys {expected:?} back, got {keys:?}"
        );
    }
    // The documented ceiling, asked on a count no row above reaches.
    let mut too_many = vec![p("TagSpecification.1.ResourceType", "route-server")];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("key-{nth}"),
        ));
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", nth + 1),
            "v",
        ));
    }
    let (status, body) = create_route_server(&router, 64_512, too_many).await;
    assert_eq!(
        status, 400,
        "case \"one tag past the ceiling\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "TagLimitExceeded",
        "case \"one tag past the ceiling\": expected TagLimitExceeded, got body {body}"
    );
}

/// RULE 4: one `ClientToken` names one route server, however many times it is replayed, and
/// two tokens name two.
#[tokio::test]
async fn one_client_token_names_one_route_server() {
    let router = emulator();
    let first = created_id(
        &router,
        "the first call",
        64_512,
        vec![p("ClientToken", "token-a")],
    )
    .await;
    let replay = created_id(
        &router,
        "the replay",
        64_512,
        vec![p("ClientToken", "token-a")],
    )
    .await;
    assert_eq!(
        replay, first,
        "case \"the replay\": expected the token to name {first}, got {replay}"
    );
    let other = created_id(
        &router,
        "a different token",
        64_513,
        vec![p("ClientToken", "token-b")],
    )
    .await;
    assert_ne!(
        other, first,
        "case \"a different token\": expected a second route server, got {other}"
    );

    let stored = described_ids(&router, "after one replay", &[]).await;
    assert_eq!(
        stored.len(),
        2,
        "case \"after one replay\": expected two stored route servers, got {stored:?}"
    );

    // A token is idempotent for the REQUEST it was first given with: replayed with any
    // other parameters it names a route server the caller did not ask for, which EC2
    // refuses. One row per request-owned member the replay compares.
    let mismatches: [(&str, Params); 4] = [
        ("a different ASN", vec![p("AmazonSideAsn", "64600")]),
        (
            "a different persistence choice",
            vec![
                p("AmazonSideAsn", "64512"),
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", "1"),
            ],
        ),
        (
            "a different notification flag",
            vec![
                p("AmazonSideAsn", "64512"),
                p("SnsNotificationsEnabled", "true"),
            ],
        ),
        (
            "a different tag set",
            vec![
                p("AmazonSideAsn", "64512"),
                p("TagSpecification.1.ResourceType", "route-server"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "other"),
            ],
        ),
    ];
    for (case, mut params) in mismatches {
        params.push(p("ClientToken", "token-a"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a replayed token, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {code:?} \
             with body {body}"
        );
    }

    let after = described_ids(&router, "after every mismatch", &[]).await;
    assert_eq!(
        after.len(),
        2,
        "case \"after every mismatch\": expected the store untouched, got {after:?}"
    );
}

/// RULE 5: `DryRun` answers `DryRunOperation` with HTTP 412 on both operations, and the
/// create writes nothing.
#[tokio::test]
async fn dry_run_is_answered_without_a_write() {
    let router = emulator();
    let existing = created_id(&router, "the seed", 64_512, Vec::new()).await;

    let (status, body) = create_route_server(&router, 64_513, vec![p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"a dry-run create\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry-run create\": expected DryRunOperation, got body {body}"
    );

    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"a dry-run describe\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry-run describe\": expected DryRunOperation, got body {body}"
    );

    // A refused dry run names a request that would have been refused anyway with THAT
    // reason, not with DryRunOperation.
    let (status, body) = create_route_server(
        &router,
        0,
        vec![p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dry run of an invalid request\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a dry run of an invalid request\": expected InvalidParameterValue, \
         got body {body}"
    );

    let stored = described_ids(&router, "after the dry runs", &[]).await;
    assert_eq!(
        stored,
        vec![existing],
        "case \"after the dry runs\": expected the store untouched, got {stored:?}"
    );
}

/// RULE 3 on the describe: an id the account holds no route server for is an error, not an
/// absent row — malformed before unknown.
#[tokio::test]
async fn an_unresolvable_route_server_id_is_refused() {
    let router = emulator();
    created_id(&router, "the seed", 64_512, Vec::new()).await;

    let cases: [(&str, &str, &str); 4] = [
        (
            "a well-formed id nothing was created for",
            "rs-0123456789abcdef0",
            "InvalidRouteServerId.NotFound",
        ),
        (
            "the short id form, unknown",
            "rs-01234567",
            "InvalidRouteServerId.NotFound",
        ),
        (
            "an id of another resource kind",
            "vpc-00000000000000001",
            "InvalidRouteServerId.Malformed",
        ),
        (
            "an id with a non-hex suffix",
            "rs-zzzzzzzz",
            "InvalidRouteServerId.Malformed",
        ),
    ];
    for (case, id, expected) in cases {
        let (status, body) = describe(&router, &indexed("RouteServerId", &[id])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {id}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected} for {id}, got {code:?} with body {body}"
        );
    }
}

/// RULE 1 on the describe: each filter selects on the member it names, and a filter name
/// this operation does not answer is refused rather than read as "no results".
#[tokio::test]
async fn filters_select_on_the_members_they_name() {
    let router = emulator();
    let plain = created_id(&router, "the plain route server", 64_512, Vec::new()).await;
    let persisting = created_id(
        &router,
        "the persisting route server",
        64_513,
        vec![
            p("PersistRoutes", "enable"),
            p("PersistRoutesDuration", "2"),
            p("SnsNotificationsEnabled", "true"),
            p("TagSpecification.1.ResourceType", "route-server"),
            p("TagSpecification.1.Tag.1.Key", "Tier"),
            p("TagSpecification.1.Tag.1.Value", "edge"),
        ],
    )
    .await;
    let both = vec![plain.clone(), persisting.clone()];
    // Every row below selects on a member that does not change once the clock has settled
    // both route servers, so the table cannot flake on a loaded runner; the `pending` word
    // itself is pinned by `the_clock_moves_a_route_server_to_available`.
    past_the_transition().await;

    let cases: [FilterRow; 8] = [
        (
            "route-server-id picks one",
            filters(&[("route-server-id", &[persisting.as_str()])]),
            vec![persisting.clone()],
        ),
        (
            "amazon-side-asn picks the one that carries it",
            filters(&[("amazon-side-asn", &["64512"])]),
            vec![plain.clone()],
        ),
        (
            "state matches both once the clock has settled them",
            filters(&[("state", &["available"])]),
            both.clone(),
        ),
        (
            "persist-routes-state tells the two apart",
            filters(&[("persist-routes-state", &["disabled"])]),
            vec![plain.clone()],
        ),
        (
            "persist-routes-duration selects on the stored minutes",
            filters(&[("persist-routes-duration", &["2"])]),
            vec![persisting.clone()],
        ),
        (
            "sns-notifications-enabled compares against the wire boolean",
            filters(&[("sns-notifications-enabled", &["true"])]),
            vec![persisting.clone()],
        ),
        (
            "tag:<key> selects on the tag value",
            filters(&[("tag:Tier", &["edge"])]),
            vec![persisting.clone()],
        ),
        (
            "two filters are joined with AND",
            filters(&[("state", &["available"]), ("amazon-side-asn", &["64513"])]),
            vec![persisting.clone()],
        ),
    ];
    for (case, params, expected) in cases {
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    let (status, body) = describe(&router, &filters(&[("route-server", &["anything"])])).await;
    assert_eq!(
        status, 400,
        "case \"a filter this operation does not answer\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a filter this operation does not answer\": expected InvalidParameterValue, \
         got body {body}"
    );
}

/// RULE 1 on the describe: the page is `RouteServerId`-ordered, a token follows a full page
/// and only then, and a token this service did not mint is refused.
#[tokio::test]
async fn pages_are_served_in_id_order() {
    let router = emulator();
    let mut created: Vec<String> = Vec::new();
    for nth in 0..7u32 {
        created.push(created_id(&router, "the seed", 64_512 + i64::from(nth), Vec::new()).await);
    }
    created.sort();

    // `MaxResults` carries a modelled range of 5..1000
    // (`smithy_query(query="operation",operation="DescribeRouteServers")`), so 5 is the
    // smallest page a client may ask for.
    let cases: [(&str, usize); 2] = [("the smallest page the model admits", 5), ("a full page", 7)];
    for (case, page_size) in cases {
        let params = vec![p("MaxResults", page_size.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let first = elements(&body, "routeServerId");
        let expected: Vec<String> = created.iter().take(page_size).cloned().collect();
        assert_eq!(
            first, expected,
            "case {case:?}: expected the first {page_size} ids in order, got {first:?}"
        );
        let token = element(&body, "nextToken");
        match created.len() > page_size {
            true => {
                let token = token.unwrap_or_else(|| {
                    panic!("case {case:?}: expected a nextToken, got body {body}")
                });
                let rest = described_ids(&router, case, &[p("NextToken", token)]).await;
                let expected_rest: Vec<String> =
                    created.iter().skip(page_size).cloned().collect();
                assert_eq!(
                    rest, expected_rest,
                    "case {case:?}: expected the remaining ids {expected_rest:?}, got {rest:?}"
                );
            }
            false => assert!(
                token.is_none(),
                "case {case:?}: expected no nextToken on the last page, got {token:?}"
            ),
        }
    }

    let (status, body) = describe(&router, &[p("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "case \"a token this service did not mint\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "case \"a token this service did not mint\": expected InvalidPaginationToken, \
         got body {body}"
    );
}
