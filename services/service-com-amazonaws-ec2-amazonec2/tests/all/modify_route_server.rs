// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyRouteServer`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `ModifyRouteServer` WRITES THE REQUEST'S CONFIGURATION ONTO THE STORED ROUTE SERVER
//!    and reports the store, so `DescribeRouteServers` reports what the modify answered
//!    with; a member the request leaves out keeps the stored value;
//! 2. the persistence move the request asks for is reported in the transient state
//!    belonging to that action (`enabling`, `disabling`, `resetting`) and the CLOCK, not a
//!    caller, settles it — as it settles the route server's own `modifying` state;
//! 3. a persist-routes time the operation cannot act on is refused with EC2's own code and
//!    the service's own sentence, whatever the value at fault, and nothing is written;
//! 4. an id no route server of the account answers to is refused rather than created;
//! 5. `DryRun` answers `DryRunOperation` and writes nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which
//! persistence action, the duration at each published bound and past it, which starting
//! configuration, which member the request leaves out — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    PERSIST_ROUTES_DURATION_INVALID_MESSAGE, PERSIST_ROUTES_DURATION_RANGE, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};

/// The Region every call here is signed for; the SNS topic ARN the assertions read is
/// derived from it.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// Create one route server with `extra` beside its ASN, wait out the create's own `pending`
/// span, and answer its id — so a modification is then the only move in flight.
async fn seeded(router: &MultiServiceRouter, case: &str, asn: i64, extra: Params) -> String {
    let mut params = vec![p("AmazonSideAsn", asn.to_string())];
    params.extend(extra);
    let (status, body) = call(router, REGION, "CreateRouteServer", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateRouteServer, got {status} with body {body}"
    );
    let id = element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"));
    past_the_transition().await;
    id
}

async fn modify(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ModifyRouteServer", params).await
}

/// The parameters of one modify of `id`, with the row's own members after it.
fn of(id: &str, extra: &Params) -> Params {
    let mut params = vec![p("RouteServerId", id)];
    params.extend(extra.iter().cloned());
    params
}

/// The body `DescribeRouteServers` answers for `id` alone.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeRouteServers",
        &[p("RouteServerId.1", id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeRouteServers, got {status} \
         with body {body}"
    );
    body
}

/// The members a route server's whole configuration is read as, for comparing one reported
/// route server against another: `(state, persistRoutesState, persistRoutesDuration,
/// snsNotificationsEnabled, snsTopicArn)`.
type Configuration = [Option<String>; 5];

fn configuration(body: &str) -> Configuration {
    [
        element(body, "state"),
        element(body, "persistRoutesState"),
        element(body, "persistRoutesDuration"),
        element(body, "snsNotificationsEnabled"),
        element(body, "snsTopicArn"),
    ]
}

/// One row of the persistence table: `(case, the create's extra members, the modify's
/// members, the `persistRoutesState` the modify reports, the `persistRoutesState` the clock
/// settles on, the `persistRoutesDuration` the store then holds)`.
type PersistRow = (
    &'static str,
    Params,
    Params,
    &'static str,
    &'static str,
    Option<String>,
);

/// RULES 1 and 2 over every persistence action: each is written onto the stored route
/// server, reported in its own transient state, and settled by the clock — and the duration
/// a persisting configuration carries is the one the request named, or the stored one when
/// the request named none.
#[tokio::test]
async fn every_persistence_action_is_written_and_settled_by_the_clock() {
    let (floor, ceiling) = PERSIST_ROUTES_DURATION_RANGE;
    // A value inside the published range that is neither bound, so no row is a boundary.
    let middle = (floor + ceiling) / 2;
    let enabled_at = |duration: i64| -> Params {
        vec![
            p("PersistRoutes", "enable"),
            p("PersistRoutesDuration", duration.to_string()),
        ]
    };

    let rows: Vec<PersistRow> = vec![
        (
            "persistence turned on at the published floor",
            Vec::new(),
            enabled_at(floor),
            "enabling",
            "enabled",
            Some(floor.to_string()),
        ),
        (
            "persistence turned on at the published ceiling",
            Vec::new(),
            enabled_at(ceiling),
            "enabling",
            "enabled",
            Some(ceiling.to_string()),
        ),
        (
            "the duration of a persisting route server changed",
            enabled_at(floor),
            enabled_at(ceiling),
            "enabling",
            "enabled",
            Some(ceiling.to_string()),
        ),
        (
            "persistence re-asked for with no duration keeps the stored one",
            enabled_at(middle),
            vec![p("PersistRoutes", "enable")],
            "enabling",
            "enabled",
            Some(middle.to_string()),
        ),
        (
            "persistence turned off leaves no duration to wait out",
            enabled_at(ceiling),
            vec![p("PersistRoutes", "disable")],
            "disabling",
            "disabled",
            None,
        ),
        (
            "the persisted routes of a persisting route server withdrawn",
            enabled_at(middle),
            vec![p("PersistRoutes", "reset")],
            "resetting",
            "enabled",
            Some(middle.to_string()),
        ),
        (
            "a reset asked of a route server that persists nothing",
            Vec::new(),
            vec![p("PersistRoutes", "reset")],
            "resetting",
            "disabled",
            None,
        ),
        (
            "a modification naming no persistence action leaves persistence off",
            Vec::new(),
            vec![p("SnsNotificationsEnabled", "true")],
            "disabled",
            "disabled",
            None,
        ),
        (
            "a modification naming no persistence action leaves persistence on",
            enabled_at(floor),
            vec![p("SnsNotificationsEnabled", "true")],
            "enabled",
            "enabled",
            Some(floor.to_string()),
        ),
    ];

    for (case, created_with, modified_with, reported_state, settled_state, duration) in rows {
        let router = emulator();
        let id = seeded(&router, case, 64_512, created_with).await;

        let (status, body) = modify(&router, &of(&id, &modified_with)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ModifyRouteServer, got {status} \
             with body {body}"
        );
        let answered_id = element(&body, "routeServerId");
        assert_eq!(
            answered_id.as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the modified route server {id}, got {answered_id:?}"
        );
        // RULE 2: a configuration change is applied over a span, so the route server a
        // caller reads straight after the call is still `modifying`.
        let state = element(&body, "state");
        assert_eq!(
            state.as_deref(),
            Some("modifying"),
            "case {case:?}: expected state \"modifying\" from the modify, got {state:?}"
        );
        let persistence = element(&body, "persistRoutesState");
        assert_eq!(
            persistence.as_deref(),
            Some(reported_state),
            "case {case:?}: expected persistRoutesState {reported_state:?} from the modify, \
             got {persistence:?}"
        );
        let reported_duration = element(&body, "persistRoutesDuration");
        assert_eq!(
            reported_duration, duration,
            "case {case:?}: expected persistRoutesDuration {duration:?} from the modify, \
             got {reported_duration:?}"
        );

        // RULE 1: the describe reads the STORE, so it reports the same route server this
        // modify answered with rather than the request object.
        let read_back = described(&router, case, &id).await;
        assert_eq!(
            configuration(&read_back),
            configuration(&body),
            "case {case:?}: expected DescribeRouteServers to report the modify's own \
             route server, got {read_back}"
        );

        // RULE 2: the clock settles both states, with nobody calling again.
        past_the_transition().await;
        let settled = described(&router, case, &id).await;
        let state = element(&settled, "state");
        assert_eq!(
            state.as_deref(),
            Some("available"),
            "case {case:?}: expected the clock to settle the route server on \"available\", \
             got {state:?} in {settled}"
        );
        let persistence = element(&settled, "persistRoutesState");
        assert_eq!(
            persistence.as_deref(),
            Some(settled_state),
            "case {case:?}: expected the clock to settle persistence on {settled_state:?}, \
             got {persistence:?} in {settled}"
        );
        let reported_duration = element(&settled, "persistRoutesDuration");
        assert_eq!(
            reported_duration, duration,
            "case {case:?}: expected the stored persistRoutesDuration {duration:?} after the \
             clock settled, got {reported_duration:?}"
        );
    }
}

/// One row of the notification table: `(case, the create's extra members, the modify's
/// members, the `snsNotificationsEnabled` the store then holds)`.
type NotificationRow = (&'static str, Params, Params, bool);

/// RULE 1 over `SnsNotificationsEnabled`: the flag the request names is written, a flag it
/// leaves out keeps the stored choice, and the SNS topic is the SERVICE's — reported for a
/// route server whose notifications are on and for no other.
#[tokio::test]
async fn notifications_and_their_topic_follow_the_stored_flag() {
    let notifications_on = || -> Params { vec![p("SnsNotificationsEnabled", "true")] };
    let notifications_off = || -> Params { vec![p("SnsNotificationsEnabled", "false")] };

    let rows: Vec<NotificationRow> = vec![
        (
            "notifications turned on",
            Vec::new(),
            notifications_on(),
            true,
        ),
        (
            "notifications turned off",
            notifications_on(),
            notifications_off(),
            false,
        ),
        (
            "notifications re-asked for",
            notifications_on(),
            notifications_on(),
            true,
        ),
        (
            "a modification naming no flag keeps notifications on",
            notifications_on(),
            vec![p("PersistRoutes", "reset")],
            true,
        ),
        (
            "a modification naming no flag keeps notifications off",
            Vec::new(),
            vec![p("PersistRoutes", "reset")],
            false,
        ),
    ];

    for (case, created_with, modified_with, enabled) in rows {
        let router = emulator();
        let id = seeded(&router, case, 64_513, created_with).await;

        let (status, body) = modify(&router, &of(&id, &modified_with)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ModifyRouteServer, got {status} \
             with body {body}"
        );
        let read_back = described(&router, case, &id).await;
        for (source, reported) in [("the modify", &body), ("the describe", &read_back)] {
            let flag = element(reported, "snsNotificationsEnabled");
            assert_eq!(
                flag.as_deref(),
                Some(enabled.to_string().as_str()),
                "case {case:?}: expected snsNotificationsEnabled {enabled} from {source}, \
                 got {flag:?}"
            );
            let topic = element(reported, "snsTopicArn");
            match enabled {
                true => {
                    let arn = topic.clone().unwrap_or_default();
                    assert!(
                        arn.starts_with(&format!("arn:aws:sns:{REGION}:")) && arn.ends_with(&id),
                        "case {case:?}: expected an SNS topic ARN of this Region naming {id} \
                         from {source}, got {topic:?}"
                    );
                }
                false => assert!(
                    topic.is_none(),
                    "case {case:?}: expected no snsTopicArn from {source}, got {topic:?}"
                ),
            }
        }
    }
}

/// One row of the refusal table: `(case, whether it names the persisting route server, the
/// modify's members, the error code, the message it must carry)`.
type RefusalRow = (&'static str, bool, Params, &'static str, Option<&'static str>);

/// RULES 3 and 4: a persist-routes time the operation cannot act on, a route server id
/// nothing answers to, and a request naming nothing to modify are each refused with EC2's
/// own code — and every stored route server is left exactly as it was.
#[tokio::test]
async fn an_unanswerable_modification_is_refused_and_writes_nothing() {
    let (floor, ceiling) = PERSIST_ROUTES_DURATION_RANGE;
    let router = emulator();
    // One route server that persists routes and one that does not, so a row can ask for
    // the refusal that depends on which.
    let persisting = seeded(
        &router,
        "the persisting seed",
        64_514,
        vec![
            p("PersistRoutes", "enable"),
            p("PersistRoutesDuration", floor.to_string()),
        ],
    )
    .await;
    let plain = seeded(&router, "the plain seed", 64_515, Vec::new()).await;
    let before = [
        configuration(&described(&router, "before every refusal", &persisting).await),
        configuration(&described(&router, "before every refusal", &plain).await),
    ];

    let rows: Vec<RefusalRow> = vec![
        (
            "a duration below the published floor",
            false,
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (floor - 1).to_string()),
            ],
            "InvalidParameterValue",
            Some(PERSIST_ROUTES_DURATION_INVALID_MESSAGE),
        ),
        (
            "a duration above the published ceiling",
            false,
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (ceiling + 1).to_string()),
            ],
            "InvalidParameterValue",
            Some(PERSIST_ROUTES_DURATION_INVALID_MESSAGE),
        ),
        (
            "a duration far above the published ceiling",
            false,
            vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (ceiling * 20).to_string()),
            ],
            "InvalidParameterValue",
            Some(PERSIST_ROUTES_DURATION_INVALID_MESSAGE),
        ),
        (
            "a duration given while persistence is being turned off",
            true,
            vec![
                p("PersistRoutes", "disable"),
                p("PersistRoutesDuration", floor.to_string()),
            ],
            "InvalidParameterValue",
            Some(PERSIST_ROUTES_DURATION_INVALID_MESSAGE),
        ),
        (
            "a duration alone for a route server that persists nothing",
            false,
            vec![p("PersistRoutesDuration", ceiling.to_string())],
            "InvalidParameterValue",
            Some(PERSIST_ROUTES_DURATION_INVALID_MESSAGE),
        ),
        (
            "persistence turned on for a route server with no duration to keep",
            false,
            vec![p("PersistRoutes", "enable")],
            "MissingParameter",
            None,
        ),
        (
            "a request naming nothing to modify",
            true,
            Vec::new(),
            "MissingParameter",
            None,
        ),
    ];

    for (case, on_the_persisting_one, params, code, message) in rows {
        let id = if on_the_persisting_one {
            &persisting
        } else {
            &plain
        };
        let (status, body) = modify(&router, &of(id, &params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ModifyRouteServer, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        if let Some(message) = message {
            assert_eq!(
                error_message(&body),
                message,
                "case {case:?}: expected the message {message:?}, got body {body}"
            );
        }
    }

    // RULE 4: an id the account holds no route server for, and something that is not a
    // route server id at all, are told apart — neither names a route server to modify.
    let unresolvable: [(&str, &str, &str); 3] = [
        (
            "a well-formed id the account holds no route server for",
            "rs-0123456789abcdef0",
            "InvalidRouteServerId.NotFound",
        ),
        (
            "a short-form id the account holds no route server for",
            "rs-01234567",
            "InvalidRouteServerId.NotFound",
        ),
        (
            "something that is not a route server id at all",
            "vpc-0123456789abcdef0",
            "InvalidRouteServerId.Malformed",
        ),
    ];
    for (case, id, code) in unresolvable {
        let (status, body) = modify(
            &router,
            &of(id, &vec![p("SnsNotificationsEnabled", "true")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ModifyRouteServer, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
    }

    // A request that carried the parameter but no value names no route server to modify,
    // which is `MissingParameter` rather than a lookup failure.
    let (status, body) = modify(&router, &of("", &vec![p("SnsNotificationsEnabled", "true")])).await;
    assert_eq!(
        status, 400,
        "case \"an empty routeServerId\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MissingParameter",
        "case \"an empty routeServerId\": expected MissingParameter, got body {body}"
    );

    // RULE 3: not one of those refusals wrote anything.
    let after = [
        configuration(&described(&router, "after every refusal", &persisting).await),
        configuration(&described(&router, "after every refusal", &plain).await),
    ];
    assert_eq!(
        after, before,
        "case \"after every refusal\": expected both route servers untouched, got {after:?}"
    );
}

/// RULE 5: `DryRun` asks whether the request WOULD have worked — a request that would have
/// succeeded is answered `DryRunOperation` and writes nothing, and one that would have been
/// refused is refused with THAT reason instead.
#[tokio::test]
async fn a_dry_run_is_answered_without_a_write() {
    let (floor, ceiling) = PERSIST_ROUTES_DURATION_RANGE;
    let router = emulator();
    let id = seeded(&router, "the seed", 64_516, Vec::new()).await;
    let before = configuration(&described(&router, "before the dry runs", &id).await);

    let (status, body) = modify(
        &router,
        &of(
            &id,
            &vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", floor.to_string()),
                p("DryRun", "true"),
            ],
        ),
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"a dry run that would have worked\": expected HTTP 412, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry run that would have worked\": expected DryRunOperation, got body {body}"
    );

    let (status, body) = modify(
        &router,
        &of(
            &id,
            &vec![
                p("PersistRoutes", "enable"),
                p("PersistRoutesDuration", (ceiling + 1).to_string()),
                p("DryRun", "true"),
            ],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dry run of an unanswerable request\": expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a dry run of an unanswerable request\": expected InvalidParameterValue, \
         got body {body}"
    );

    past_the_transition().await;
    let after = configuration(&described(&router, "after the dry runs", &id).await);
    assert_eq!(
        after, before,
        "case \"after the dry runs\": expected the route server untouched, got {after:?}"
    );
}
