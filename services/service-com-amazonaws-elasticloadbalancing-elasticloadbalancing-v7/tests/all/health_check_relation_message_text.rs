// SPDX-License-Identifier: CC0-1.0

//! The TEXT of the refusal `ConfigureHealthCheck` states for the timeout/interval
//! relation, byte for byte on the wire.
//!
//! RULE: a refusal's message is part of the wire contract, so what the service answers is
//! the sentence it words for that refusal EXACTLY — every character of the fixed text, the
//! trailing space of this one included. One error code answers every rule this operation
//! enforces, so the message is the only thing that tells them apart: a client, fixture or
//! recorded-response comparison that matches this refusal by its text sees a message one
//! character short as a different message.
//!
//! `super::documented_rule_refusals` tables the relation itself — which pairs of values are
//! accepted and which are refused — through a reader that trims each element's character
//! data, which is what comparing a sentence wants and exactly what asserting its text
//! cannot do. These rows read the message between the tags instead
//! (`super::wire::error_message_bytes`), over several pairs that break the relation at
//! different points of the two members' own modelled ranges, because the text is the rule's
//! and not one pair's.

use super::wire::{call, emulator, error_message_bytes, param, Wire};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The error code this service answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The refusal a health check whose timeout is not below its interval is answered with,
/// written out here as the wire carries it: the whole fixed sentence, closing with the
/// space after its period.
const TIMEOUT_RELATION_MESSAGE: &str = "HealthCheck timeout must be less than interval. ";

/// `ConfigureHealthCheck` for one target and one interval/timeout pair.
fn health_check(name: &str, target: &str, interval: i32, timeout: i32) -> Vec<(String, String)> {
    vec![
        param("LoadBalancerName", name),
        param("HealthCheck.Target", target),
        param("HealthCheck.Interval", &interval.to_string()),
        param("HealthCheck.Timeout", &timeout.to_string()),
        param("HealthCheck.UnhealthyThreshold", "2"),
        param("HealthCheck.HealthyThreshold", "2"),
    ]
}

async fn configure(
    router: &MultiServiceRouter,
    name: &str,
    target: &str,
    interval: i32,
    timeout: i32,
) -> Wire {
    call(
        router,
        REGION,
        "ConfigureHealthCheck",
        &health_check(name, target, interval, timeout),
    )
    .await
}

#[tokio::test]
async fn the_timeout_relation_refusal_states_its_sentence_byte_for_byte() {
    let router = emulator();
    let name = "hc-relation-text-elb";
    let created = call(
        &router,
        REGION,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", name),
            param("AvailabilityZones.member.1", "us-west-2a"),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {}",
        created.body
    );

    // (case, the target, the interval, the timeout) — pairs that break the relation at
    // several points of the modelled ranges (`Interval` 5..300, `Timeout` 2..60), by
    // equality and by exceeding it, since the sentence states the rule rather than the
    // values.
    let rows: [(&str, &str, i32, i32); 6] = [
        ("a timeout equal to the smallest interval", "HTTP:80/ok", 5, 5),
        ("a timeout equal to the interval", "HTTP:80/ok", 10, 10),
        ("a timeout one above the interval", "HTTP:80/ok", 10, 11),
        ("the largest timeout with an interval equal to it", "TCP:80", 60, 60),
        ("a timeout far above the interval", "SSL:443", 5, 60),
        ("a timeout above the interval on an HTTPS target", "HTTPS:443/up", 30, 59),
    ];

    for (case, target, interval, timeout) in rows {
        let answered = configure(&router, name, target, interval, timeout).await;
        assert_eq!(
            (answered.status, answered.error_code().as_str()),
            (400, VALIDATION_ERROR),
            "case {case:?}: expected 400 {VALIDATION_ERROR}, got {} with {}",
            answered.status,
            answered.body
        );
        let stated = error_message_bytes(&answered);
        assert_eq!(
            stated, TIMEOUT_RELATION_MESSAGE,
            "case {case:?}: expected the message {TIMEOUT_RELATION_MESSAGE:?}, got {stated:?}"
        );
    }

    // The other side of the relation: a timeout BELOW the interval is accepted, so the
    // sentence above is the refusal of a broken rule and not of every health check.
    for (case, target, interval, timeout) in [
        ("a timeout one below the interval", "HTTP:80/ok", 10, 9),
        ("the smallest interval with the smallest timeout", "TCP:80", 5, 2),
        ("the largest timeout one below its interval", "SSL:443", 61, 60),
    ] {
        let answered = configure(&router, name, target, interval, timeout).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: a timeout below the interval must be accepted, got {} with {}",
            answered.status, answered.body
        );
        assert!(
            !answered.body.contains("HealthCheck timeout"),
            "case {case:?}: an accepted check states no relation refusal, got {}",
            answered.body
        );
    }
}
