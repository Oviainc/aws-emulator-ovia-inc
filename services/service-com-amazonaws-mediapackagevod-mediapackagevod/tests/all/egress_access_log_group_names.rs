// SPDX-License-Identifier: CC0-1.0

//! A PackagingGroup's egress access-log subscription names a log group CloudWatch Logs
//! can hold.
//!
//! The rule under test: an input member that names a CloudWatch Logs log group — the group
//! MediaPackage VOD writes a packaging group's egress access logs INTO — is accepted only
//! when its value is a name CloudWatch Logs can actually hold (`state::MIN_LOG_GROUP_NAME_CHARS`
//! to `state::MAX_LOG_GROUP_NAME_CHARS` characters, each one a letter, a digit or one of
//! `.`, `-`, `_`, `/`, `#`); any other value is refused with `UnprocessableEntityException`
//! (HTTP 422, `smithy_query(query="operation",operation="ConfigureLogs")`) keyed by that
//! member and stating the requirement, and the addressed group keeps the subscription it
//! already had.
//!
//! The rule is the service's own, not a modelled constraint: the model targets
//! `EgressAccessLogs$LogGroupName` at the bare `__string` and publishes no `@length` or
//! `@pattern` (`smithy_query(query="shape",shape="EgressAccessLogs")` reports the single
//! `LogGroupName` member, `jsonName: logGroupName`, documented as "Customize the log group
//! name."), so the framework's constraint walk cannot refuse any of it. The bounds and the
//! requirement sentence are read off the service rather than restated, so this suite cannot
//! drift from what it enforces; the accepted CHARACTER SET is written out here (its source
//! is CloudWatch Logs' published log-group-name rule: `a-z`, `A-Z`, `0-9`, `_`, `-`, `/`,
//! `.`, `#`) so the oracle is independent of the service's own set.
//!
//! The rows differ along what the rule generalises over: the name's length (the shortest,
//! middling, exactly the ceiling, one past it, far past it), which character classes it
//! draws on (digits, upper and lower case, each punctuation character, a nested path), and
//! which OPERATION carries the member — `ConfigureLogs` and `CreatePackagingGroup` are the
//! two this model binds it on, and both are judged by the one rule. Each published bound is
//! a row, with one step past it, and the last suite is a property test over a seeded
//! alphabet mixing admitted and rejected characters.
//!
//! `configure_logs.rs` covers what an accepted subscription WRITES; this suite is the
//! member's accepted domain.
//!
//! Which refusal wins when a request is wrong twice: `ConfigureLogs` resolves the group it
//! ADDRESSES first, so an `Id` this service holds no group under is answered 404 whatever
//! the request's log group name is — the member rule judges a request that reached a group.
//! `configure_logs.rs::configure_logs_refuses_an_id_no_group_is_stored_under` pins that.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagevod_mediapackagevod as vod;
use smithy_server_runtime::MultiServiceRouter;
use vod::state;

/// The region every case signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The modelled refusal of an invalid request parameter: error code and `httpError`
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// The modelled refusal of an id this service holds no group under.
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// The log group every seeded group starts subscribed to, so a refused request is compared
/// against a subscription nothing else touched.
const SEEDED_GROUP: &str = "/aws/MediaPackage/vod-seeded-subscription";

/// The ceiling CloudWatch Logs holds a log group name within, as the service publishes it.
const MAX_CHARS: usize = state::MAX_LOG_GROUP_NAME_CHARS;

/// The floor, as the service publishes it — so the empty string names no group.
const MIN_CHARS: usize = state::MIN_LOG_GROUP_NAME_CHARS;

/// The ceiling as CloudWatch Logs PUBLISHES it, written out here so the bound this suite
/// holds the service to does not come from the service itself.
///
/// Source: CloudWatch Logs' own log-group-name rule, which this repo also carries in that
/// service's model — `services/service-com-amazonaws-cloudwatchlogs-logs-20140328`
/// documents `logGroupName` as `len 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$`.
const PUBLISHED_MAX_CHARS: usize = 512;

/// The floor as CloudWatch Logs publishes it; see [`PUBLISHED_MAX_CHARS`].
const PUBLISHED_MIN_CHARS: usize = 1;

/// A deterministic generator for the property suite below: a 64-bit linear congruential
/// step, which is the shape this repo's other seeded suites use (the MediaPackage v1
/// access-log suite and the b2bi mapping suite each carry one). Nothing here needs
/// cryptographic randomness — it needs the SAME sequence from the same seed, in every
/// build, with no lockfile entry.
struct SeededGenerator {
    state: u64,
}

impl SeededGenerator {
    /// The generator for `seed`. Two seeds that differ by one still start far apart,
    /// because the seed is mixed before the first value is drawn.
    fn new(seed: u64) -> Self {
        let mut generator = Self { state: seed };
        generator.step();
        generator
    }

    /// Advance the state one step (the multiplier and increment are Knuth's MMIX values).
    fn step(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    /// The next value in `0..bound`, taken from the high bits where the sequence mixes
    /// best. `bound` must not be zero.
    fn next_below(&mut self, bound: usize) -> usize {
        assert!(bound > 0, "a generator bound must be positive, got {bound}");
        usize::try_from(self.step() >> 33).expect("a 31-bit value fits a usize") % bound
    }
}

/// What one answer says about itself: the HTTP status, the error shape `x-amzn-errortype`
/// names, and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The `message` member, or `"<unset>"` when the body carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The per-attribute `errors` map, or `Value::Null` when the body carries none.
    fn errors(&self) -> Value {
        self.body.get("errors").cloned().unwrap_or(Value::Null)
    }

    /// The reported `egressAccessLogs.logGroupName`, or `None` when either is absent.
    fn log_group_name(&self) -> Option<&str> {
        self.body
            .pointer("/egressAccessLogs/logGroupName")
            .and_then(Value::as_str)
    }
}

/// A router carrying exactly this service over one shared engine — the same assembly
/// `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(vod::EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
    router
}

/// Send one request and report what it answered.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<unset>")
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// `ConfigureLogs` on `id`, carrying the `egressAccessLogs` block `logs` (`None` leaves the
/// member out of the request body altogether).
async fn configure_logs(router: &MultiServiceRouter, id: &str, logs: Option<Value>) -> Answer {
    let body = match logs {
        Some(logs) => json!({ "egressAccessLogs": logs }),
        None => json!({}),
    };
    call(
        router,
        "PUT",
        &format!("/packaging_groups/{id}/configure_logs"),
        Some(body),
    )
    .await
}

/// `DescribePackagingGroup` on `id`.
async fn describe_group(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/packaging_groups/{id}"), None).await
}

/// Create `id` and subscribe it to [`SEEDED_GROUP`], answering the stored representation
/// every later assertion compares against.
async fn seeded_group(router: &MultiServiceRouter, id: &str) -> Value {
    let created = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(
        created.status, 200,
        "seeding {id:?}: CreatePackagingGroup expected 200, got {} with body {}",
        created.status, created.body
    );
    let seeded = configure_logs(router, id, Some(json!({ "logGroupName": SEEDED_GROUP }))).await;
    assert_eq!(
        seeded.status, 200,
        "seeding {id:?}: the baseline ConfigureLogs expected 200, got {} with body {}",
        seeded.status, seeded.body
    );
    assert_eq!(
        seeded.log_group_name(),
        Some(SEEDED_GROUP),
        "seeding {id:?}: the baseline subscription must be reported, got {}",
        seeded.body
    );
    seeded.body
}

/// Assert `answer` is this service's 422, keyed by the log-group member and stating the
/// member's whole accepted domain.
fn assert_refused(case: &str, answer: &Answer) {
    let (code, status) = UNPROCESSABLE;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.errortype, code,
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype
    );
    let requirement = state::log_group_name_requirement();
    assert_eq!(
        answer.errors(),
        json!({ state::EGRESS_LOG_GROUP_MEMBER: [requirement.clone()] }),
        "case {case:?}: the refusal must be keyed by {} and state the requirement, got {}",
        state::EGRESS_LOG_GROUP_MEMBER,
        answer.errors()
    );
    assert_eq!(
        answer.message(),
        format!("egress_access_logs.log_group_name {requirement}"),
        "case {case:?}: the refusal must name the member and the requirement, got {:?}",
        answer.message()
    );
}

/// A case label naming the name under test by its length and its opening characters, so a
/// 600-character row does not print 600 characters.
fn label(prefix: &str, name: &str) -> String {
    format!(
        "{prefix}: logGroupName of {} characters starting {:?}",
        name.chars().count(),
        name.chars().take(24).collect::<String>()
    )
}

/// Names CloudWatch Logs can hold, so this service must subscribe to them.
///
/// Source of the character set: CloudWatch Logs' published log-group-name rule (letters,
/// digits, `_`, `-`, `/`, `.`, `#`). One row per class the rule generalises over, and none
/// of them a name the issue cited.
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
        "mediapackage.vod_egress-01#live/eu".to_string(),
        "/aws/vendedlogs/MediaPackageVod/EgressAccessLogs".to_string(),
        "0123456789".to_string(),
        "UPPER/lower/MiXeD".to_string(),
        // One character short of the ceiling, and exactly the ceiling.
        "g".repeat(MAX_CHARS - 1),
        "g".repeat(MAX_CHARS),
    ]
}

/// Names CloudWatch Logs cannot hold, so this service must refuse them.
///
/// The rows span both halves of the rule: the length (empty, one past the ceiling, far past
/// it) and the character set (a space, each symbol outside the published set — including the
/// `:` and `*` an ARN uses — and a non-ASCII letter).
fn refused_names() -> Vec<String> {
    vec![
        String::new(),
        "g".repeat(MAX_CHARS + 1),
        "g".repeat(600),
        "g".repeat(4_096),
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

#[tokio::test]
async fn a_name_cloud_watch_logs_can_hold_is_subscribed_and_read_back() {
    let router = router();

    for (index, name) in accepted_names().into_iter().enumerate() {
        let id = format!("accepted-{index}");
        seeded_group(&router, &id).await;
        let case = label("ConfigureLogs", &name);

        let configured =
            configure_logs(&router, &id, Some(json!({ "logGroupName": name.clone() }))).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            configured.status, configured.body
        );
        assert_eq!(
            configured.log_group_name(),
            Some(name.as_str()),
            "case {case:?}: the subscribed group must be reported, got {}",
            configured.body
        );

        // Persisted, not merely echoed: the read renders the stored record, and so does
        // the listing.
        let described = describe_group(&router, &id).await;
        assert_eq!(
            described.body, configured.body,
            "case {case:?}: the read must render the stored record, got {}",
            described.body
        );
        let listed = call(&router, "GET", "/packaging_groups", None).await;
        let entry = listed
            .body
            .get("packagingGroups")
            .and_then(Value::as_array)
            .and_then(|groups| {
                groups
                    .iter()
                    .find(|group| group.get("id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            entry.pointer("/egressAccessLogs/logGroupName"),
            Some(&json!(name)),
            "case {case:?}: the listing must render the stored subscription, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_name_cloud_watch_logs_cannot_hold_is_refused_and_keeps_the_stored_subscription() {
    let router = router();

    for (index, name) in refused_names().into_iter().enumerate() {
        let id = format!("refused-{index}");
        let seeded = seeded_group(&router, &id).await;
        let case = label("ConfigureLogs", &name);

        let answer = configure_logs(&router, &id, Some(json!({ "logGroupName": name }))).await;
        assert_refused(&case, &answer);

        // The group kept the subscription it already had: the refused name was not stored.
        let described = describe_group(&router, &id).await;
        assert_eq!(
            described.body, seeded,
            "case {case:?}: a refused ConfigureLogs must leave the stored group alone, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_length_bound_is_the_published_one_at_each_edge() {
    let router = router();
    let id = "length-walk";
    let seeded = seeded_group(&router, id).await;

    // The bounds the service publishes ARE the ones CloudWatch Logs publishes, so the walk
    // below is a walk over the published numbers rather than over whatever the service
    // happens to hold.
    assert_eq!(
        (MIN_CHARS, MAX_CHARS),
        (PUBLISHED_MIN_CHARS, PUBLISHED_MAX_CHARS),
        "the service's bounds must be CloudWatch Logs' published ones, got ({MIN_CHARS}, \
         {MAX_CHARS})"
    );

    // Each published bound, and one step past it: the floor (0, 1, 2) and the ceiling (one
    // short, exactly, one past) written as the published LITERALS, plus a length far past
    // the ceiling — so a service that moved its own ceiling would fail here.
    for length in [0, 1, 2, 511, 512, 513, 600] {
        let name = "n".repeat(length);
        let case = format!("logGroupName of {length} characters");
        let answer =
            configure_logs(&router, id, Some(json!({ "logGroupName": name.clone() }))).await;
        if (PUBLISHED_MIN_CHARS..=PUBLISHED_MAX_CHARS).contains(&length) {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the service must accept it, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.log_group_name(),
                Some(name.as_str()),
                "case {case:?}: the accepted name must be stored, got {}",
                answer.body
            );
            // Put the baseline back, so the next refused case is compared against it.
            let restored =
                configure_logs(&router, id, Some(json!({ "logGroupName": SEEDED_GROUP }))).await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: restoring the baseline expected 200, got {} with body {}",
                restored.status, restored.body
            );
        } else {
            assert_refused(&case, &answer);
            let described = describe_group(&router, id).await;
            assert_eq!(
                described.body, seeded,
                "case {case:?}: a refused ConfigureLogs must leave the stored group alone, \
                 got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn create_packaging_group_judges_the_same_member() {
    // The rule is about the MEMBER, not about the operation carrying it: the other
    // operation this model binds `EgressAccessLogs` on judges a name the same way, and a
    // refused create stores no group at all.
    for (index, name) in refused_names().into_iter().enumerate() {
        let router = router();
        let id = format!("created-{index}");
        let case = label("CreatePackagingGroup", &name);
        let answer = call(
            &router,
            "POST",
            "/packaging_groups",
            Some(json!({
                "id": id.clone(),
                "egressAccessLogs": { "logGroupName": name },
            })),
        )
        .await;
        assert_refused(&case, &answer);

        let described = describe_group(&router, &id).await;
        assert_eq!(
            described.status, NOT_FOUND.1,
            "case {case:?}: a refused create must store no group, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            described.errortype, NOT_FOUND.0,
            "case {case:?}: expected x-amzn-errortype {}, got {:?}",
            NOT_FOUND.0, described.errortype
        );
    }

    // Not a blanket refusal: a name CloudWatch Logs can hold is stored by the create and
    // reported by the following read.
    for (index, name) in accepted_names().into_iter().enumerate() {
        let router = router();
        let id = format!("created-ok-{index}");
        let case = label("CreatePackagingGroup", &name);
        let answer = call(
            &router,
            "POST",
            "/packaging_groups",
            Some(json!({
                "id": id.clone(),
                "egressAccessLogs": { "logGroupName": name.clone() },
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let described = describe_group(&router, &id).await;
        assert_eq!(
            described.log_group_name(),
            Some(name.as_str()),
            "case {case:?}: the created group must report the subscription, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_log_group_is_unsubscribed_rather_than_refused() {
    let router = router();

    // The rule judges the name a request CARRIES: a block sent without a `LogGroupName`,
    // and a request leaving the member out altogether, name no group and so violate
    // nothing — they unsubscribe the group.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("a block carrying no logGroupName", Some(json!({}))),
        ("no egressAccessLogs member at all", None),
    ];
    for (index, (case, logs)) in rows.into_iter().enumerate() {
        let id = format!("unsubscribed-{index}");
        seeded_group(&router, &id).await;
        let answer = configure_logs(&router, &id, logs.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("egressAccessLogs").cloned(),
            logs.clone(),
            "case {case:?}: the response must report the block that was sent, got {}",
            answer.body
        );
        let described = describe_group(&router, &id).await;
        assert_eq!(
            described.body.get("egressAccessLogs").cloned(),
            logs,
            "case {case:?}: the stored group must report the block that was sent, got {}",
            described.body
        );
    }
}

/// The characters a log group name may carry, written out from CloudWatch Logs' published
/// log-group-name rule (`a-z`, `A-Z`, `0-9`, `_`, `-`, `/`, `.`, `#`) — the property test's
/// own oracle, independent of the service's set.
fn admitted_by_cloud_watch(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '#')
}

/// The alphabet generated names are drawn from: admitted characters mixed with characters
/// outside the set, so a generated name lands either side of the rule.
const ALPHABET: &[char] = &[
    'a', 'b', 'Z', '0', '9', '_', '-', '/', '.', '#', ' ', '!', '$', '*', ':', '+', '=', ',', '@',
    'é',
];

#[tokio::test]
async fn acceptance_follows_the_published_log_group_name_domain() {
    let router = router();

    // A bounded number of generated names per seed, each judged against the PUBLISHED
    // domain rather than against the service's own answer: the invariant is that the
    // service accepts exactly the names CloudWatch Logs can hold, stores every one it
    // accepts and stores none it refuses. The seeds are fixed and named in every case
    // label, so a failing case reproduces exactly.
    for seed in [3u64, 29, 1_013] {
        let id = format!("generated-{seed}");
        let seeded = seeded_group(&router, &id).await;
        let mut rng = SeededGenerator::new(seed);

        for case_index in 0..24u32 {
            // Lengths either side of both bounds: mostly short, with the occasional name
            // straddling the ceiling.
            let length = match case_index % 4 {
                0 => rng.next_below(4),
                1 => 1 + rng.next_below(39),
                2 => MAX_CHARS - rng.next_below(2),
                _ => MAX_CHARS + rng.next_below(3),
            };
            let name: String = (0..length)
                .map(|_| ALPHABET[rng.next_below(ALPHABET.len())])
                .collect();
            let acceptable = (MIN_CHARS..=MAX_CHARS).contains(&name.chars().count())
                && name.chars().all(admitted_by_cloud_watch);
            let case = format!(
                "seed {seed}, case {case_index}: {} characters, acceptable={acceptable}",
                name.chars().count()
            );

            let answer =
                configure_logs(&router, &id, Some(json!({ "logGroupName": name.clone() }))).await;
            if acceptable {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a name CloudWatch Logs can hold must be accepted, got {} \
                     with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.log_group_name(),
                    Some(name.as_str()),
                    "case {case:?}: the accepted name must be stored, got {}",
                    answer.body
                );
                let restored =
                    configure_logs(&router, &id, Some(json!({ "logGroupName": SEEDED_GROUP })))
                        .await;
                assert_eq!(
                    restored.status, 200,
                    "case {case:?}: restoring the baseline expected 200, got {} with body {}",
                    restored.status, restored.body
                );
            } else {
                assert_refused(&case, &answer);
                let described = describe_group(&router, &id).await;
                assert_eq!(
                    described.body, seeded,
                    "case {case:?}: a refused ConfigureLogs must leave the stored group alone, \
                     got {}",
                    described.body
                );
            }
        }
    }
}
