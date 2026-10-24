// SPDX-License-Identifier: CC0-1.0

//! A channel's access-log subscription names a log group CloudWatch Logs can hold.
//!
//! The rule under test: a member that names a CloudWatch Logs log group — the log group
//! MediaPackage writes a channel's ingress or egress access logs INTO — is accepted only
//! when the value is a name CloudWatch Logs can actually hold (1 to
//! `state::MAX_LOG_GROUP_NAME_CHARS` characters, each one a letter, a digit or one of
//! `.`, `-`, `_`, `/`, `#`); any other value is refused with
//! `UnprocessableEntityException` (HTTP 422,
//! `smithy_query(query="operation",operation="ConfigureLogs")`) keyed by that member and
//! stating the requirement, and the channel keeps the subscription it already had.
//!
//! The rule is the service's own, not a modelled constraint: the model targets
//! `IngressAccessLogs$LogGroupName` / `EgressAccessLogs$LogGroupName` at the bare
//! `__string` and publishes no `@length` or `@pattern`
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngressAccessLogs,com.amazonaws.mediapackage#EgressAccessLogs",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//! so the framework's constraint walk cannot refuse any of it. The bound and the
//! requirement sentence are read off the service rather than restated, so the suite
//! cannot drift from what it enforces; the accepted CHARACTER SET is written out here
//! (its source is CloudWatch Logs' published log-group-name rule: `a-z`, `A-Z`, `0-9`,
//! `_`, `-`, `/`, `.`, `#`) so the oracle below is independent of the service's own set.
//!
//! The rows differ along what the rule generalises over: the name's length (the shortest,
//! middling, exactly the ceiling, one past it, far past it), which character classes it
//! draws on (digits, upper and lower case, each punctuation character, a nested path),
//! which of the two members carries it, and whether the other direction is sent at all.
//! Each published bound is a row, with one step past it. The last suite is a property
//! test over a seeded alphabet mixing admitted and rejected characters.
//!
//! `tests/all/mutation_persistence.rs` covers what an accepted subscription WRITES;
//! this suite is the member's accepted domain.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code, SeededGenerator};

/// The log group every seeded channel starts subscribed to, in both directions, so a
/// refused request is compared against a subscription nothing else touched.
const SEEDED_GROUP: &str = "/aws/MediaPackage/seeded-subscription";

/// The ceiling CloudWatch Logs holds a log group name within, as the service publishes it.
const MAX_CHARS: usize = state::MAX_LOG_GROUP_NAME_CHARS;

/// The floor, as the service publishes it — so the empty string names no group.
const MIN_CHARS: usize = state::MIN_LOG_GROUP_NAME_CHARS;

/// Names CloudWatch Logs can hold, so this service must subscribe to them.
///
/// Source of the character set: CloudWatch Logs' published log-group-name rule (letters,
/// digits, `_`, `-`, `/`, `.`, `#`). One row per class the rule generalises over, and
/// none of them a name the issue cited.
fn accepted_names() -> Vec<String> {
    vec![
        // The shortest name there is, and each punctuation character on its own.
        "a".repeat(MIN_CHARS),
        "7".to_string(),
        "Z".to_string(),
        "_".to_string(),
        "-".to_string(),
        ".".to_string(),
        "/".to_string(),
        "#".to_string(),
        // Mixed classes, a nested path, and the leading slash AWS's own groups use.
        "mediapackage.access_logs-01#live/eu".to_string(),
        "/aws/vendedlogs/MediaPackage/EgressAccessLogs".to_string(),
        "0123456789".to_string(),
        "UPPER/lower/MiXeD".to_string(),
        // Exactly the ceiling, and one character short of it.
        "g".repeat(MAX_CHARS - 1),
        "g".repeat(MAX_CHARS),
    ]
}

/// Names CloudWatch Logs cannot hold, so this service must refuse them.
///
/// The rows span both halves of the rule: the length (empty, one past the ceiling, far
/// past it) and the character set (space, and each symbol outside the published set,
/// including the `:` and `*` an ARN uses and a non-ASCII letter).
fn refused_names() -> Vec<String> {
    vec![
        String::new(),
        "g".repeat(MAX_CHARS + 1),
        "g".repeat(600),
        "g".repeat(1_024),
        "has a space".to_string(),
        "group!".to_string(),
        "group$".to_string(),
        "group%".to_string(),
        "group*".to_string(),
        "group:name".to_string(),
        "group+name".to_string(),
        "group=name".to_string(),
        "group@name".to_string(),
        "group,name".to_string(),
        "group\\name".to_string(),
        "group\tname".to_string(),
        "group\nname".to_string(),
        "grüße".to_string(),
        "日本語".to_string(),
        "arn:aws:logs:eu-west-1:000000000000:log-group:/aws/mp:*".to_string(),
    ]
}

/// The two members that name a log group, as the wire paths the refusal is keyed by.
const MEMBERS: &[(&str, &str)] = &[
    ("ingressAccessLogs", state::INGRESS_LOG_GROUP_MEMBER),
    ("egressAccessLogs", state::EGRESS_LOG_GROUP_MEMBER),
];

/// A `ConfigureLogs` body subscribing `ingress` and `egress`, leaving out a direction
/// whose name is `None`.
fn configure_body(ingress: Option<&str>, egress: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(ingress) = ingress {
        body["ingressAccessLogs"] = json!({ "logGroupName": ingress });
    }
    if let Some(egress) = egress {
        body["egressAccessLogs"] = json!({ "logGroupName": egress });
    }
    body
}

/// Create `id` and subscribe both of its directions to [`SEEDED_GROUP`], answering the
/// stored record every later assertion compares against.
async fn seeded_channel(router: &MultiServiceRouter, id: &str) -> Value {
    create_channel(router, id).await;
    let (status, seeded) = call(
        router,
        "PUT",
        &format!("/channels/{id}/configure_logs"),
        Some(&configure_body(Some(SEEDED_GROUP), Some(SEEDED_GROUP))),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding {id:?}: the baseline ConfigureLogs expected 200, got {status} {seeded:?}"
    );
    for pointer in [
        "/ingressAccessLogs/logGroupName",
        "/egressAccessLogs/logGroupName",
    ] {
        assert_eq!(
            seeded.pointer(pointer).and_then(Value::as_str),
            Some(SEEDED_GROUP),
            "seeding {id:?}: {pointer} must report the seeded group, got {seeded:?}"
        );
    }
    seeded
}

/// Assert `refusal` is this service's 422, keyed by `member` and stating the member's
/// accepted domain.
fn assert_refused(case: &str, member: &str, status: u16, refusal: &Value) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    let requirement = state::log_group_name_requirement();
    assert_eq!(
        refusal.pointer(&format!("/errors/{member}")),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {member:?} and state the requirement, \
         got {refusal:?}"
    );
    assert_eq!(
        refusal.get("message").and_then(Value::as_str),
        Some(format!("{member} {requirement}").as_str()),
        "case {case:?}: the refusal's sentence must name the member and the requirement, got \
         {refusal:?}"
    );
}

/// A channel id fragment for a row, so each row addresses its own channel: the index, and
/// a short readable tag.
fn channel_id(prefix: &str, index: usize) -> String {
    format!("{prefix}-{index}")
}

#[tokio::test]
async fn a_name_cloud_watch_logs_can_hold_is_subscribed_and_read_back() {
    let router = emulator();

    for (index, name) in accepted_names().into_iter().enumerate() {
        let id = channel_id("accepted", index);
        seeded_channel(&router, &id).await;
        let case = format!(
            "logGroupName of {} characters starting {:?}",
            name.chars().count(),
            name.chars().take(24).collect::<String>()
        );

        // Both directions carry the row's name, so neither member is admitted by a rule
        // the other is not.
        let (status, configured) = call(
            &router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(&configure_body(Some(&name), Some(&name))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ConfigureLogs expected 200, got {status} {configured:?}"
        );
        for (_, member) in MEMBERS {
            let pointer = format!("/{}", member.replace('.', "/"));
            assert_eq!(
                configured.pointer(&pointer).and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: {member} must report the subscribed group, got {configured:?}"
            );
        }

        // Persisted: the later read renders the same record, and so does the listing.
        let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, configured,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );
        let (status, listed) = call(&router, "GET", "/channels?maxResults=50", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListChannels expected 200, got {status} {listed:?}"
        );
        let listed_channel = listed
            .get("channels")
            .and_then(Value::as_array)
            .and_then(|channels| {
                channels
                    .iter()
                    .find(|channel| channel.get("id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            listed_channel, configured,
            "case {case:?}: ListChannels must render the same stored record, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_name_cloud_watch_logs_cannot_hold_is_refused_and_keeps_the_stored_subscription() {
    let router = emulator();

    for (index, name) in refused_names().into_iter().enumerate() {
        // One channel per refused name, carried once by each member, so a refusal on one
        // direction is never confused with a refusal on the other.
        for (member_index, (block, member)) in MEMBERS.iter().enumerate() {
            let id = channel_id(&format!("refused-{member_index}"), index);
            let seeded = seeded_channel(&router, &id).await;
            let case = format!(
                "{block}.logGroupName of {} characters starting {:?}",
                name.chars().count(),
                name.chars().take(24).collect::<String>()
            );

            // The OTHER direction carries a name the service accepts, so the refusal is
            // the row's own member and not the request as a whole.
            let (ingress, egress) = if *block == "ingressAccessLogs" {
                (Some(name.as_str()), Some("/aws/MediaPackage/accepted"))
            } else {
                (Some("/aws/MediaPackage/accepted"), Some(name.as_str()))
            };
            let (status, refusal) = call(
                &router,
                "PUT",
                &format!("/channels/{id}/configure_logs"),
                Some(&configure_body(ingress, egress)),
            )
            .await;
            assert_refused(&case, member, status, &refusal);

            // The channel kept the subscription it already had: neither the refused
            // direction nor the acceptable one was written.
            let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeChannel expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, seeded,
                "case {case:?}: a refused ConfigureLogs must leave the stored subscription \
                 alone, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_first_member_the_request_carries() {
    let router = emulator();
    let id = "both-directions-refused";
    let seeded = seeded_channel(&router, id).await;

    // Both members name a group the service cannot hold: the request is refused by the
    // first member it carries, so one request is always refused the same way.
    let (status, refusal) = call(
        &router,
        "PUT",
        &format!("/channels/{id}/configure_logs"),
        Some(&configure_body(Some(""), Some(&"g".repeat(MAX_CHARS + 1)))),
    )
    .await;
    assert_refused(
        "both directions refused",
        state::INGRESS_LOG_GROUP_MEMBER,
        status,
        &refusal,
    );
    let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeChannel expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described, seeded,
        "a refused ConfigureLogs must leave the stored subscription alone, got {described:?}"
    );
}

#[tokio::test]
async fn a_direction_the_request_leaves_out_is_unsubscribed_rather_than_refused() {
    let router = emulator();

    // The rule judges the names a request CARRIES: a direction left out, or sent with no
    // `LogGroupName` at all, names no group and unsubscribes that direction.
    let rows: Vec<(&str, Value)> = vec![
        ("neither member", json!({})),
        (
            "members carrying no logGroupName",
            json!({ "ingressAccessLogs": {}, "egressAccessLogs": {} }),
        ),
        (
            "ingress only",
            json!({ "ingressAccessLogs": { "logGroupName": "kept/ingress" } }),
        ),
        (
            "egress only",
            json!({ "egressAccessLogs": { "logGroupName": "kept/egress" } }),
        ),
    ];
    for (index, (case, body)) in rows.into_iter().enumerate() {
        let id = channel_id("omitted", index);
        seeded_channel(&router, &id).await;
        let (status, configured) = call(
            &router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ConfigureLogs expected 200, got {status} {configured:?}"
        );
        for (block, _) in MEMBERS {
            let expected = body
                .pointer(&format!("/{block}/logGroupName"))
                .and_then(Value::as_str);
            assert_eq!(
                configured
                    .pointer(&format!("/{block}/logGroupName"))
                    .and_then(Value::as_str),
                expected,
                "case {case:?}: {block} must report {expected:?}, got {configured:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_length_bound_is_the_published_one_at_each_edge() {
    let router = emulator();
    let id = "length-walk";
    seeded_channel(&router, id).await;

    // Each published bound, and one step past it: the floor (0, 1, 2) and the ceiling
    // (one short, exactly, one past), plus the length the issue's symptom used.
    for length in [
        MIN_CHARS - 1,
        MIN_CHARS,
        MIN_CHARS + 1,
        MAX_CHARS - 1,
        MAX_CHARS,
        MAX_CHARS + 1,
        600,
    ] {
        let name = "n".repeat(length);
        let case = format!("logGroupName of {length} characters");
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(&configure_body(Some(&name), None)),
        )
        .await;
        if (MIN_CHARS..=MAX_CHARS).contains(&length) {
            assert_eq!(
                status, 200,
                "case {case:?}: the service must accept it, got {status} {body:?}"
            );
            assert_eq!(
                body.pointer("/ingressAccessLogs/logGroupName")
                    .and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: the accepted name must be stored, got {body:?}"
            );
        } else {
            assert_refused(&case, state::INGRESS_LOG_GROUP_MEMBER, status, &body);
        }
    }
}

/// The characters a log group name may carry, written out from CloudWatch Logs' published
/// log-group-name rule (`a-z`, `A-Z`, `0-9`, `_`, `-`, `/`, `.`, `#`) — the property
/// test's own oracle, independent of the service's set.
fn admitted_by_cloud_watch(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '#')
}

/// The alphabet the generated names are drawn from: admitted characters mixed with
/// characters outside the set, so a generated name lands either side of the rule.
const ALPHABET: &[char] = &[
    'a', 'b', 'Z', '0', '9', '_', '-', '/', '.', '#', ' ', '!', '$', '*', ':', '+', '=', ',', '@',
    'é',
];

#[tokio::test]
async fn acceptance_follows_the_published_log_group_name_domain() {
    let router = emulator();

    // A bounded number of generated names per seed, each judged against the published
    // domain rather than against the service's own answer: the invariant is that the
    // service accepts exactly the names CloudWatch Logs can hold, and stores every one it
    // accepts. A failing case reproduces from the seed printed below.
    for seed in [3u64, 29, 1_013] {
        let id = format!("generated-{seed}");
        let seeded = seeded_channel(&router, &id).await;
        let mut rng = SeededGenerator::new(seed);

        for case_index in 0..24u32 {
            // Lengths either side of both bounds: mostly short, with the occasional name
            // straddling the ceiling.
            let length = match case_index % 4 {
                0 => rng.next_in(4) - 1,
                1 => rng.next_in(40),
                2 => MAX_CHARS - rng.next_in(2) + 1,
                _ => MAX_CHARS + rng.next_in(3) - 1,
            };
            let name: String = (0..length)
                .map(|_| ALPHABET[rng.next_in(ALPHABET.len() as u64) - 1])
                .collect();
            let acceptable = (MIN_CHARS..=MAX_CHARS).contains(&name.chars().count())
                && name.chars().all(admitted_by_cloud_watch);
            let case = format!(
                "seed {seed}, case {case_index}: {} characters, acceptable={acceptable}",
                name.chars().count()
            );

            let (status, body) = call(
                &router,
                "PUT",
                &format!("/channels/{id}/configure_logs"),
                Some(&configure_body(Some(&name), Some(SEEDED_GROUP))),
            )
            .await;
            if acceptable {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a name CloudWatch Logs can hold must be accepted, got \
                     {status} {body:?}"
                );
                assert_eq!(
                    body.pointer("/ingressAccessLogs/logGroupName")
                        .and_then(Value::as_str),
                    Some(name.as_str()),
                    "case {case:?}: the accepted name must be stored, got {body:?}"
                );
                // Put the baseline back, so the next refused case is compared against it.
                let (status, restored) = call(
                    &router,
                    "PUT",
                    &format!("/channels/{id}/configure_logs"),
                    Some(&configure_body(Some(SEEDED_GROUP), Some(SEEDED_GROUP))),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: restoring the baseline expected 200, got {status} \
                     {restored:?}"
                );
            } else {
                assert_refused(&case, state::INGRESS_LOG_GROUP_MEMBER, status, &body);
                let (status, described) =
                    call(&router, "GET", &format!("/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DescribeChannel expected 200, got {status} {described:?}"
                );
                assert_eq!(
                    described, seeded,
                    "case {case:?}: a refused ConfigureLogs must leave the stored \
                     subscription alone, got {described:?}"
                );
            }
        }
    }
}
