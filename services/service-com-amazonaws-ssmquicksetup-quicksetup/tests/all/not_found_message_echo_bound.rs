// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the bound on the ARN this service's not-found sentence
//! echoes.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a refusal this service words renders a value the REQUEST
//! supplied only as far as this crate's single echo bound, so a value no modelled
//! constraint bounds in length comes back as a bounded prefix plus the byte count it
//! was elided from, and one inside the bound comes back exactly as it was sent. The
//! configuration-manager ARN is such a value: the `ManagerArn` pattern of the manager
//! routes and of `ListConfigurations` bounds the ARN's shape but not its length,
//! because the Region segment it admits is `[^:]+`
//! (`smithy_query(query="operation", operation="GetConfigurationManager")`), and the
//! tag plane's `ResourceArn` carries no `@pattern` and no `@length` at all.
//!
//! The rows span what the rule generalises over: the LENGTH of the ARN a request
//! supplies — the shortest the pattern admits, the bound exactly, one byte past it, and
//! far past it — and the encoding of the text that makes it long, since an elision must
//! cut on a character boundary rather than inside a multi-byte character. Each row is
//! driven through every operation that reaches the sentence: the three whose route
//! labels the manager they address and `ListConfigurations`, which takes the ARN as a
//! body member. The tag plane reaches no ARN of its own here — its routes are refused
//! ahead of the handler by the request-authorization stage, with one fixed sentence that
//! renders nothing of the request (`tests/all/target_authorization.rs`) — so the row
//! class below also checks that an oversized `ResourceArn` earns that fixed refusal and
//! no echo.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::reserved_tag_keys::{call, emulator, error_code, error_message, seeded};

/// The longest caller-supplied value a refusal of this service echoes, in bytes.
///
/// Source: the bound the crate states for exactly this (`MAX_ECHOED_VALUE` in
/// `src/state.rs`), written out here rather than read from the crate so the test pins
/// the behaviour itself.
const MAX_ECHOED: usize = 256;

/// The bytes of a `ManagerArn` that are NOT its Region segment: the partition prefix
/// `arn:aws:ssm-quicksetup:`, the `:`-separated 12-digit account, and
/// `:configuration-manager/` with a uuid — every fixed part of the modelled pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`.
const ARN_FIXED_BYTES: usize = 95;

/// A well-formed `ManagerArn` whose Region segment is `region`, so a row can choose the
/// ARN's length while staying inside the modelled pattern: the segment is `[^:]+`, which
/// bounds neither how long it is nor which characters it carries.
fn arn_with_region(region: &str) -> String {
    format!(
        "arn:aws:ssm-quicksetup:{region}:000000000000:configuration-manager/\
         00000000-0000-4000-8000-000000000000"
    )
}

/// A well-formed `ManagerArn` of exactly `bytes` bytes, padded in the Region segment.
fn arn_of_length(bytes: usize) -> String {
    let arn = arn_with_region(&"a".repeat(bytes - ARN_FIXED_BYTES));
    assert_eq!(
        arn.len(),
        bytes,
        "the fixture must build an ARN of {bytes} bytes, got {}",
        arn.len()
    );
    arn
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may carry neither a raw `/` nor any byte outside the
/// unreserved set, so every other byte travels as `%XX`.
fn label(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// How `arn` must appear inside a refusal: itself while it fits the bound, else the
/// longest prefix of WHOLE characters that fits plus the elision the crate's renderer
/// appends. Computed from the ARN here, independently of the crate's own helper.
fn expected_echo(arn: &str) -> String {
    if arn.len() <= MAX_ECHOED {
        return arn.to_string();
    }
    let mut end = 0;
    for (index, character) in arn.char_indices() {
        let candidate = index + character.len_utf8();
        if candidate > MAX_ECHOED {
            break;
        }
        end = candidate;
    }
    format!("{}... [{} bytes]", &arn[..end], arn.len())
}

/// The sentence the service must answer with for `arn` — the fixed text written out here
/// rather than read from the crate, around the ARN as far as the bound renders it.
fn expected_message(arn: &str) -> String {
    format!(
        "Quick setup manager with arn: {} could not be found.",
        expected_echo(arn)
    )
}

/// Every operation that reaches this service's own configuration-manager lookup with an
/// ARN its request supplied.
const MANAGER_ARN_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "DeleteConfigurationManager",
    "UpdateConfigurationManager",
    "ListConfigurations",
];

/// Drive one of [`MANAGER_ARN_OPERATIONS`] against `arn`, on the route and with the
/// payload the model binds for it (`OPERATIONS.md`).
async fn call_naming(router: &MultiServiceRouter, operation: &str, arn: &str) -> (u16, Value) {
    let path = format!("/configurationManager/{}", label(arn));
    match operation {
        "GetConfigurationManager" => call(router, "GET", &path, None).await,
        "DeleteConfigurationManager" => call(router, "DELETE", &path, None).await,
        "UpdateConfigurationManager" => {
            call(router, "PUT", &path, Some(&json!({ "Name": "renamed" }))).await
        }
        "ListConfigurations" => {
            call(
                router,
                "POST",
                "/listConfigurations",
                Some(&json!({ "ManagerArn": arn })),
            )
            .await
        }
        other => panic!("no route is wired for operation {other:?}"),
    }
}

/// The ARNs the rows supply, each pattern-valid and none minted by any create: the
/// shortest Region segment the pattern admits, the echo bound exactly, one byte past it,
/// two sizes far past it, and one made long by multi-byte characters, which the Region
/// segment's `[^:]+` admits and whose elision must cut on a character boundary.
fn oversized_and_bounded_arns() -> Vec<(String, String)> {
    vec![
        (
            "the shortest Region segment the pattern admits".to_string(),
            arn_of_length(ARN_FIXED_BYTES + 1),
        ),
        (
            "an ARN of exactly the echo bound".to_string(),
            arn_of_length(MAX_ECHOED),
        ),
        (
            "an ARN one byte past the echo bound".to_string(),
            arn_of_length(MAX_ECHOED + 1),
        ),
        ("an ARN of a kilobyte".to_string(), arn_of_length(1_024)),
        ("an ARN of four kilobytes".to_string(), arn_of_length(4_096)),
        // A multi-byte Region segment: 'é' is two bytes, so a prefix cut at the bound
        // would land inside a character unless the renderer steps back to a boundary.
        (
            "an ARN made long by multi-byte characters".to_string(),
            arn_with_region(&"é".repeat(200)),
        ),
    ]
}

/// Assert one refusal: the modelled 404, the modelled error shape, and the sentence
/// carrying the ARN this very request supplied, bounded.
fn assert_not_found(case: &str, arn: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    let actual = error_message(answer);
    let expected = expected_message(arn);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    // Whatever the request sent, the sentence it earns stays bounded: the fixed text
    // (49 bytes), at most the echo bound of the ARN, and the elision's byte count.
    assert!(
        actual.len() <= MAX_ECHOED + 80,
        "case {case:?}: the refusal must stay bounded, got {} bytes: {actual:?}",
        actual.len()
    );
}

/// Every operation that names a manager by ARN refuses one naming no stored manager with
/// the not-found sentence, and the ARN it echoes is bounded: an ARN inside the bound
/// comes back exactly as it was sent, and a longer one comes back elided.
#[tokio::test]
async fn an_oversized_arn_is_echoed_only_as_far_as_the_echo_bound() {
    let router = emulator();

    for (arn_case, arn) in oversized_and_bounded_arns() {
        for operation in MANAGER_ARN_OPERATIONS {
            let case = format!("{operation} on {arn_case}");
            let (status, answer) = call_naming(&router, operation, &arn).await;
            assert_not_found(&case, &arn, status, &answer);
            // An oversized ARN comes back elided rather than whole, so one request is
            // not amplified into a response carrying all of it.
            if arn.len() > MAX_ECHOED {
                let message = error_message(&answer);
                assert!(
                    !message.contains(&arn),
                    "case {case:?}: an oversized ARN must not come back whole, got \
                     {} bytes: {message:?}",
                    message.len()
                );
            }
        }
    }
}

/// The tag plane takes the ARN on a member with no modelled constraint at all, so its
/// value is the most unbounded one this service receives: those routes are refused ahead
/// of the handler with the one fixed sentence that renders nothing of the request, so an
/// oversized value earns no echo whatever its length.
#[tokio::test]
async fn an_oversized_resource_arn_earns_no_echo_from_the_tag_plane() {
    let router = emulator();

    for (arn_case, arn) in oversized_and_bounded_arns() {
        let path = format!("/tags/{}", label(&arn));
        let calls: Vec<(&str, (u16, Value))> = vec![
            ("ListTagsForResource", call(&router, "GET", &path, None).await),
            (
                "TagResource",
                call(&router, "PUT", &path, Some(&json!({ "Tags": { "Tier": "gold" } }))).await,
            ),
            (
                "UntagResource",
                call(&router, "DELETE", &format!("{path}?tagKeys=Tier"), None).await,
            ),
        ];
        for (operation, (status, answer)) in calls {
            let case = format!("{operation} on {arn_case}");
            assert_eq!(
                status, 401,
                "case {case:?}: expected the authorization refusal, got {status} / \
                 {answer:?}"
            );
            let message = error_message(&answer);
            // A prefix of whole characters, so a multi-byte row slices nothing in half.
            let prefix: String = arn.chars().take(32).collect();
            assert!(
                !message.contains(&prefix),
                "case {case:?}: the refusal must render nothing of the ARN, got \
                 {message:?}"
            );
            assert!(
                message.len() <= MAX_ECHOED,
                "case {case:?}: the refusal must stay bounded, got {} bytes: {message:?}",
                message.len()
            );
        }
    }
}

/// The bound is a no-op for an ARN this service minted, so the refusal that names a
/// manager that DOES exist — the one a configuration-definition id is resolved against —
/// still carries that manager's whole ARN.
#[tokio::test]
async fn a_minted_arn_is_rendered_whole_in_the_definition_not_found_sentence() {
    let router = emulator();
    let live = super::reserved_tag_keys::create_ok(&router, "a live manager", &seeded()).await;
    assert!(
        live.len() <= MAX_ECHOED,
        "a minted ARN must be far inside the echo bound, got {} bytes: {live:?}",
        live.len()
    );

    let case = "an unknown definition id under a live manager";
    let (status, answer) = call(
        &router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": &live, "ConfigurationDefinitionId": "no-such-def" })),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404, got {status} / {answer:?}"
    );
    let actual = error_message(&answer);
    let expected =
        format!("configuration manager {live} holds no configuration definition with ID no-such-def");
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}
