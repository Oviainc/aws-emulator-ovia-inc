// SPDX-License-Identifier: CC0-1.0

//! The ONE rule set every operation that SETS a feed's whole output list holds, driven
//! end to end through each of the two operations that set one.
//!
//! `CreateFeed` mints a feed's output list and `UpdateFeed` replaces a stored one, so
//! both are held to the same three rules, in the same order, with the same message and
//! the same modelled error class per failure:
//!
//! 1. a feed holds at least one output, so an EMPTY list is refused with the modelled
//!    `ValidationException` (HTTP 400);
//! 2. an output's `name` identifies it within its feed, so a list naming one output
//!    twice is refused with the modelled `ConflictException` (HTTP 409);
//! 3. the list is BOUNDED at `MAX_OUTPUTS_PER_FEED`, so a longer one is refused with the
//!    modelled `ServiceQuotaExceededException` (HTTP 402).
//!
//! The rows below span the three rules, the lists each one admits, and the ORDER the
//! rules are decided in (a list that breaks two of them earns the earlier refusal), and
//! every row is sent through both operations — so one statement of the rules answers for
//! both, and neither can drift from the other without a row here failing.
//!
//! Every case drives the real request path: the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler runs,
//! and the response is serialized as `restJson1`.

use crate::harness::{call, enabled_output, message_of, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    duplicate_output_name_message, too_many_outputs_message, MAX_OUTPUTS_PER_FEED,
    MISSING_OUTPUTS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The name of the one output the fixture feed each case starts from holds.
const HELD_OUTPUT: &str = "held1";

/// Which operation SETS the output list under test — the two that set a feed's whole
/// list.
#[derive(Debug, Clone, Copy)]
enum ListSetter {
    /// `CreateFeed`: the list the new feed is minted with (HTTP 202 when accepted).
    Create,
    /// `UpdateFeed`: the list that replaces the stored one (HTTP 200 when accepted).
    Update,
}

impl ListSetter {
    /// The status this operation answers when it accepts the list.
    fn accepted_status(self) -> u16 {
        match self {
            Self::Create => 202,
            Self::Update => 200,
        }
    }
}

/// What a supplied list earns: the refusal's status, `__type` and message, or `None`
/// when the list is one a feed may hold.
struct Refusal {
    status: u16,
    error_type: &'static str,
    message: String,
}

/// The names of the outputs a feed reports, in the order it reports them.
fn output_names(feed: &Value) -> Vec<String> {
    feed["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|output| output["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Mint the fixture feed every case starts from — one feed holding one output — and
/// return its id.
async fn fixture_feed(router: &MultiServiceRouter, label: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({
            "name": "output-list-feed",
            "outputs": [enabled_output(HELD_OUTPUT, "cropping")]
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: the fixture feed is created, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string()
}

/// Send `outputs` to `setter` against a store holding exactly the fixture feed, and
/// return the response plus that feed's id.
async fn set_outputs(
    router: &MultiServiceRouter,
    setter: ListSetter,
    outputs: &[Value],
    label: &str,
) -> (u16, Value, String) {
    let id = fixture_feed(router, label).await;
    let (status, body) = match setter {
        ListSetter::Create => {
            call(
                router,
                "POST",
                "/v1/feed",
                Some(json!({"name": "minted-feed", "outputs": outputs})),
            )
            .await
        }
        ListSetter::Update => {
            call(
                router,
                "PUT",
                &format!("/v1/feed/{id}"),
                Some(json!({"name": "output-list-feed", "outputs": outputs})),
            )
            .await
        }
    };
    (status, body, id)
}

/// A list of `count` distinct outputs, each named `out<index>`.
fn distinct_outputs(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| enabled_output(&format!("out{index}"), "cropping"))
        .collect()
}

/// The three rules a set output list is held to, and the order they are decided in,
/// asserted over a table of lists through BOTH operations that set the list.
#[tokio::test]
async fn every_setter_of_a_feeds_output_list_holds_the_same_rules() {
    let missing = || Refusal {
        status: 400,
        error_type: "ValidationException",
        message: MISSING_OUTPUTS_MESSAGE.to_string(),
    };
    let duplicate = |name: &str| Refusal {
        status: 409,
        error_type: "ConflictException",
        message: duplicate_output_name_message(name),
    };
    let too_many = || Refusal {
        status: 402,
        error_type: "ServiceQuotaExceededException",
        message: too_many_outputs_message(),
    };

    // The rows differ along what the rules generalise over: the COUNT of entries (none,
    // one, several, the bound, past it), where a repeat sits in the list, and which
    // rules a single list breaks at once (the last two rows pin the ORDER the rules are
    // decided in).
    let rows: Vec<(String, Vec<Value>, Option<Refusal>)> = vec![
        ("an empty list".to_string(), vec![], Some(missing())),
        (
            "one output".to_string(),
            vec![enabled_output("solo1", "cropping")],
            None,
        ),
        (
            "three outputs of three config kinds".to_string(),
            vec![
                enabled_output("crop1", "cropping"),
                enabled_output("subs1", "subtitling"),
                enabled_output("meta1", "contextualMetadata"),
            ],
            None,
        ),
        (
            "two outputs of one name, side by side".to_string(),
            vec![
                enabled_output("twice1", "cropping"),
                enabled_output("twice1", "subtitling"),
            ],
            Some(duplicate("twice1")),
        ),
        (
            "two outputs of one name, with another between them".to_string(),
            vec![
                enabled_output("edge1", "cropping"),
                enabled_output("mid1", "subtitling"),
                enabled_output("edge1", "contextualMetadata"),
            ],
            Some(duplicate("edge1")),
        ),
        (
            "the second name repeated after a distinct first".to_string(),
            vec![
                enabled_output("one1", "cropping"),
                enabled_output("two1", "subtitling"),
                enabled_output("two1", "contextualMetadata"),
            ],
            Some(duplicate("two1")),
        ),
        (
            format!("a list of {} outputs, one under the bound", MAX_OUTPUTS_PER_FEED - 1),
            distinct_outputs(MAX_OUTPUTS_PER_FEED - 1),
            None,
        ),
        (
            format!("a list of {MAX_OUTPUTS_PER_FEED} outputs, exactly the bound"),
            distinct_outputs(MAX_OUTPUTS_PER_FEED),
            None,
        ),
        (
            format!("a list of {} outputs, one past the bound", MAX_OUTPUTS_PER_FEED + 1),
            distinct_outputs(MAX_OUTPUTS_PER_FEED + 1),
            Some(too_many()),
        ),
        (
            format!("a list of {} outputs, well past the bound", MAX_OUTPUTS_PER_FEED + 7),
            distinct_outputs(MAX_OUTPUTS_PER_FEED + 7),
            Some(too_many()),
        ),
        (
            // Breaks rules 2 and 3 at once: the repeated name is reported, because an
            // output that is not addressable is decided before the bound.
            "a list past the bound that also names one output twice".to_string(),
            {
                let mut outputs = distinct_outputs(MAX_OUTPUTS_PER_FEED + 1);
                outputs.push(enabled_output("out0", "subtitling"));
                outputs
            },
            Some(duplicate("out0")),
        ),
    ];

    for (row, outputs, expected) in rows {
        for setter in [ListSetter::Create, ListSetter::Update] {
            let label = format!("{setter:?}: {row}");
            let router = router();
            let (status, body, id) = set_outputs(&router, setter, &outputs, &label).await;

            let Some(refusal) = &expected else {
                assert_eq!(
                    status,
                    setter.accepted_status(),
                    "case {label:?}: a list a feed may hold is accepted with HTTP {}, got \
                     {status} with {body}",
                    setter.accepted_status()
                );
                continue;
            };

            assert_eq!(
                status, refusal.status,
                "case {label:?}: expected HTTP {}, got {status} with {body}",
                refusal.status
            );
            assert_eq!(
                body["__type"],
                json!(refusal.error_type),
                "case {label:?}: expected a {}, got {body}",
                refusal.error_type
            );
            assert_eq!(
                message_of(&body),
                refusal.message,
                "case {label:?}: expected {:?}, got {:?}",
                refusal.message,
                message_of(&body)
            );

            // A refused set stores nothing: no feed was minted, and the fixture feed
            // still holds exactly the output it was created with.
            let (status, listed) = call(&router, "GET", "/v1/feeds", None).await;
            assert_eq!(
                (status, listed["feeds"].as_array().map(Vec::len)),
                (200, Some(1)),
                "case {label:?}: the refused call minted no feed, got {listed}"
            );
            let (status, feed) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetFeed answers 200, got {status} with {feed}"
            );
            assert_eq!(
                output_names(&feed),
                vec![HELD_OUTPUT.to_string()],
                "case {label:?}: the refused call left the stored outputs as they were, got \
                 {:?}",
                feed["outputs"]
            );
        }
    }
}

/// The accepted lists are STORED: a create and an update that pass the shared rules both
/// leave the feed holding exactly the outputs the request named, read back through
/// `GetFeed`.
#[tokio::test]
async fn a_list_the_shared_rules_admit_is_stored_by_both_setters() {
    // One row per shape the rules admit: one output, several, and the bound itself.
    for count in [1usize, 3, MAX_OUTPUTS_PER_FEED] {
        for setter in [ListSetter::Create, ListSetter::Update] {
            let label = format!("{setter:?}: a stored list of {count} outputs");
            let router = router();
            let outputs = distinct_outputs(count);
            let (status, body, fixture_id) =
                set_outputs(&router, setter, &outputs, &label).await;
            assert_eq!(
                status,
                setter.accepted_status(),
                "case {label:?}: expected HTTP {}, got {status} with {body}",
                setter.accepted_status()
            );

            let expected: Vec<String> = (0..count).map(|index| format!("out{index}")).collect();
            assert_eq!(
                output_names(&body),
                expected,
                "case {label:?}: the response repeats the outputs the request named, got {:?}",
                body["outputs"]
            );

            let id = match setter {
                ListSetter::Create => body["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: a minted feed carries an id"))
                    .to_string(),
                ListSetter::Update => fixture_id,
            };
            let (status, feed) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetFeed answers 200, got {status} with {feed}"
            );
            assert_eq!(
                output_names(&feed),
                expected,
                "case {label:?}: the stored feed holds the outputs the request named, got {:?}",
                feed["outputs"]
            );
        }
    }
}
