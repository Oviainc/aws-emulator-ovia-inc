// SPDX-License-Identifier: CC0-1.0

//! EVERY action this model declares is dispatchable at the API version it declares — and
//! the target-health read, which is the action the issue behind this suite recorded, is one
//! row of that table.
//!
//! # The rule
//!
//! On an awsQuery endpoint that publishes two API versions, the `(Action, Version)` pair an
//! envelope names resolves to the model that declares that version, and every action that
//! model declares is served out of it; `InvalidAction` — which tells an SDK the action does
//! not exist — is reserved for a pair no mounted model declares. So a well-formed call to
//! an action this model declares is either served or refused with one of THAT operation's
//! own answers, and never with a refusal that denies the action exists at all.
//!
//! # Why a whole-action-set table
//!
//! The issue recorded ONE action answered `InvalidAction` at this model's API version: the
//! target-health read, which is the only way a client can tell whether the targets it
//! deployed behind a load balancer came up. One action is one row of the rule above, so
//! this suite drives EVERY action the model binds to this service — read from the model
//! itself ([`declared_actions`]) rather than restated here, so the table cannot drift from
//! it — through BOTH front doors a request can arrive on: the `elasticloadbalancing`
//! signing name every AWS SDK ELBv2 client signs (where the envelope's `Version` is what
//! names this model), and the key that addresses this model directly.
//!
//! What a RESOLVED pair then reaches is pinned beside this suite rather than here:
//! `shared_endpoint_routing.rs` covers the dispatch axes (the key, the version, and which
//! of the two models declares the action), and the second test below covers the recorded
//! action's own answer — one description per target the request names, derived from the
//! registrations the store holds, or the operation's own modelled refusal for a target
//! group nothing holds.

use serde_json::Value;

// The two-service router and the one request builder this crate's shared-endpoint suite
// owns: both front doors over one engine, so a request here arrives exactly as an AWS SDK
// client's does. Reused rather than copied — one builder for the crate.
use super::provisioning_loop::{first_text, members, p, VERSION};
use super::shared_endpoint_routing::{call, emulator, REGION, SHARED_KEY, V2_KEY};

/// This crate's own Smithy model, so a test derives the set of actions this service
/// declares from the model itself instead of trusting a copy of it.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json"
));

/// How many operations the model binds to this service, from
/// `smithy_query(query="list_operations")` on it: 51. Asserted against what
/// [`declared_actions`] reads so a model that grew an operation this suite would then not
/// drive fails here rather than going unnoticed.
const DECLARED_ACTIONS: usize = 51;

/// The awsQuery error code AWS answers an action it does not have with, and the one no call
/// to an action this model declares may earn.
const INVALID_ACTION: &str = "InvalidAction";

/// Every action this service declares, as the model states it: the short names of the
/// operations the service shape binds.
///
/// Read from the model file rather than listed here, because a list beside the model is a
/// second statement of the action set and this suite exists to pin that there is only one.
fn declared_actions() -> Vec<String> {
    let model: Value = serde_json::from_str(MODEL).expect("the model is JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("a Smithy model has shapes");
    let mut actions: Vec<String> = shapes
        .values()
        .filter(|shape| shape.get("type").and_then(Value::as_str) == Some("service"))
        .flat_map(|service| {
            service
                .get("operations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|bound| {
            bound
                .get("target")
                .and_then(Value::as_str)
                .and_then(|id| id.rsplit('#').next())
                .map(str::to_string)
        })
        .collect();
    actions.sort();
    actions.dedup();
    actions
}

/// Every action the model declares reaches this model's own operation at this model's API
/// version, whichever front door the request arrives on.
///
/// Each row carries `Action` and `Version` and nothing else, which is all the rule is
/// about: an operation whose input has a required member then answers its own validation
/// refusal, one still scaffolded answers its own `501`, and one that needs nothing answers
/// its document — three different answers, none of which may claim the action does not
/// exist.
#[tokio::test]
async fn every_declared_action_is_dispatchable_at_this_models_api_version() {
    let router = emulator();
    let actions = declared_actions();
    assert_eq!(
        actions.len(),
        DECLARED_ACTIONS,
        "expected the {DECLARED_ACTIONS} operations the model binds to this service, got \
         {}: {actions:?}",
        actions.len()
    );
    assert!(
        actions.iter().any(|action| action == "DescribeTargetHealth"),
        "expected the recorded action among the declared set, got {actions:?}"
    );

    for action in &actions {
        for key in [SHARED_KEY, V2_KEY] {
            let case = format!("{action} on the {key} key");
            let reply = call(&router, key, action, Some(VERSION), &[]).await;
            let (status, body) = (reply.status, reply.body);
            let code = first_text(&body, "Code").unwrap_or_default();
            assert!(
                body.trim_start().starts_with('<') && !body.contains("__type"),
                "case {case:?}: an awsQuery client must get XML, got {status} with {body}"
            );
            assert_ne!(
                code, INVALID_ACTION,
                "case {case:?}: the model declares this action at {VERSION}, so the pair \
                 must resolve to this model's own operation, got {status} with {body}"
            );
            assert!(
                !body.contains("Could not find operation"),
                "case {case:?}: the pair must resolve, got {status} with {body}"
            );
            assert!(
                matches!(status, 200 | 400 | 501),
                "case {case:?}: expected the operation's own answer — served, refused, or \
                 its scaffold's 501 — got {status} with {body}"
            );
        }
    }
}

/// The recorded action's own answer: one description per target the request names, each
/// derived from the registration the store holds for it, or none named and every
/// registration reported.
///
/// The rows differ along what the rule generalises over — how many targets the request
/// names, whether a named target carries the port it was registered on or leaves the
/// group's own port to stand for it, and whether it is registered at all — and each asserts
/// what the read reports rather than what the registration answered. The group has no
/// listener forwarding to it, so every registered target is reported with the state a group
/// no load balancer uses has; the states that follow from a listener and from the settling
/// span are `provisioning_loop.rs`.
#[tokio::test]
async fn the_health_read_reports_one_description_per_target_it_is_asked_about() {
    let router = emulator();
    let created = call(
        &router,
        SHARED_KEY,
        "CreateTargetGroup",
        Some(VERSION),
        &[
            p("Name", "health-filter-tg"),
            p("Protocol", "HTTP"),
            p("Port", "80"),
            p("VpcId", "vpc-0abc1234"),
            p("TargetType", "instance"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the target group must be created, got {} with {}",
        created.status, created.body
    );
    let arn = first_text(&created.body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("no TargetGroupArn in {}", created.body));

    // Three registrations: one on the group's own port, two on ports of their own.
    let registered = call(
        &router,
        SHARED_KEY,
        "RegisterTargets",
        Some(VERSION),
        &[
            p("TargetGroupArn", &arn),
            p("Targets.member.1.Id", "i-0aaaaaaaaaaaaaaa1"),
            p("Targets.member.2.Id", "i-0bbbbbbbbbbbbbbb2"),
            p("Targets.member.2.Port", "8081"),
            p("Targets.member.3.Id", "i-0ccccccccccccccc3"),
            p("Targets.member.3.Port", "9090"),
        ],
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "the targets must be registered, got {} with {}",
        registered.status, registered.body
    );

    // What one description must report: the target's id, the port a health check of it
    // runs against, and the state and reason the stored registration implies. A registered
    // target of a group NO load balancer forwards to has nothing checking it, and a target
    // the group holds no registration for is reported as unregistered — both reason codes
    // the model's `TargetHealthReasonEnum` declares.
    const NOT_IN_USE: (&str, &str) = ("unused", "Target.NotInUse");
    const NOT_REGISTERED: (&str, &str) = ("unused", "Target.NotRegistered");
    type Described<'a> = (&'a str, &'a str, (&'a str, &'a str));
    /// One row of the filter table: the case, the `Targets` filter the request carries,
    /// what the read must then report, and whether that is the order the filter named.
    type FilterCase<'a> = (&'a str, Vec<(String, String)>, Vec<Described<'a>>, bool);

    let cases: [FilterCase<'_>; 5] = [
        (
            "no filter at all reports every registration",
            vec![],
            vec![
                ("i-0aaaaaaaaaaaaaaa1", "80", NOT_IN_USE),
                ("i-0bbbbbbbbbbbbbbb2", "8081", NOT_IN_USE),
                ("i-0ccccccccccccccc3", "9090", NOT_IN_USE),
            ],
            false,
        ),
        (
            "one target named on the group's own port",
            vec![p("Targets.member.1.Id", "i-0aaaaaaaaaaaaaaa1")],
            vec![("i-0aaaaaaaaaaaaaaa1", "80", NOT_IN_USE)],
            true,
        ),
        (
            "one target named with the port it was registered on",
            vec![
                p("Targets.member.1.Id", "i-0bbbbbbbbbbbbbbb2"),
                p("Targets.member.1.Port", "8081"),
            ],
            vec![("i-0bbbbbbbbbbbbbbb2", "8081", NOT_IN_USE)],
            true,
        ),
        (
            "two targets named, in the order the request names them",
            vec![
                p("Targets.member.1.Id", "i-0ccccccccccccccc3"),
                p("Targets.member.1.Port", "9090"),
                p("Targets.member.2.Id", "i-0aaaaaaaaaaaaaaa1"),
            ],
            vec![
                ("i-0ccccccccccccccc3", "9090", NOT_IN_USE),
                ("i-0aaaaaaaaaaaaaaa1", "80", NOT_IN_USE),
            ],
            true,
        ),
        (
            // A target the group holds no registration for is reported as that, not
            // omitted and not refused — and it carries the port a health check of it would
            // run against, which is the group's own port here.
            "a target that was never registered",
            vec![p("Targets.member.1.Id", "i-0dddddddddddddddd4")],
            vec![("i-0dddddddddddddddd4", "80", NOT_REGISTERED)],
            true,
        ),
    ];

    for (case, filter, expected, ordered) in cases {
        let mut params = vec![p("TargetGroupArn", &arn)];
        params.extend(filter);
        let reply = call(
            &router,
            SHARED_KEY,
            "DescribeTargetHealth",
            Some(VERSION),
            &params,
        )
        .await;
        let (status, body) = (reply.status, reply.body);
        assert_eq!(
            status, 200,
            "case {case:?}: the health read must be served, got {status} with {body}"
        );
        let mut reported: Vec<(String, String, (String, String))> =
            members(&body, "TargetHealthDescriptions")
                .iter()
                .map(|description| {
                    (
                        first_text(description, "Id").unwrap_or_default(),
                        first_text(description, "HealthCheckPort").unwrap_or_default(),
                        (
                            first_text(description, "State").unwrap_or_default(),
                            first_text(description, "Reason").unwrap_or_default(),
                        ),
                    )
                })
                .collect();
        let mut wanted: Vec<(String, String, (String, String))> = expected
            .iter()
            .map(|(id, port, (state, reason))| {
                (
                    (*id).to_string(),
                    (*port).to_string(),
                    ((*state).to_string(), (*reason).to_string()),
                )
            })
            .collect();
        // A read that names its targets answers them in the order it named; the unfiltered
        // read reports every registration the store holds, which it keys by id and port, so
        // that row is compared as a set.
        if !ordered {
            reported.sort();
            wanted.sort();
        }
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected one description per target asked about, as \
             (id, health check port, (state, reason)), got {status} with {body}"
        );
    }

    // The operation's OWN modelled refusal for a target group nothing holds — one of the
    // answers the model declares for it, and not a routing refusal.
    let absent = emulator_server::emulator_arn(
        "elasticloadbalancing",
        REGION,
        "targetgroup/no-such-group/0123456789abcdef",
    );
    let reply = call(
        &router,
        SHARED_KEY,
        "DescribeTargetHealth",
        Some(VERSION),
        &[p("TargetGroupArn", &absent)],
    )
    .await;
    let (status, body) = (reply.status, reply.body);
    assert_eq!(
        (status, first_text(&body, "Code").unwrap_or_default().as_str()),
        (400, "TargetGroupNotFound"),
        "a health read for an absent target group must be the operation's own modelled \
         refusal, got {status} with {body}"
    );
    assert!(
        body.contains(&absent),
        "the refusal must name the target group it could not find, got {body}"
    );
}
