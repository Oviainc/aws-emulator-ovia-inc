// SPDX-License-Identifier: CC0-1.0

//! What a game session queue's removal makes of the operations that name it, over the
//! wire: `DeleteGameSessionQueue` takes the record out of the store, and the queue's own
//! update path then refuses the name as an invalid request parameter.
//!
//! The rule asserted here: an `UpdateGameSessionQueue` whose `Name` resolves to no stored
//! queue — for ANY reason the identifier fails to resolve — is refused with HTTP 400, the
//! modelled `InvalidRequestException`, and the one sentence "Queue name <identifier> does
//! not exist", whose only variable part is the `GameSessionQueueNameOrArn` the request
//! supplied, copied verbatim. The model defaults both that error and `NotFoundException`
//! to HTTP 400 (`smithy_query operation=UpdateGameSessionQueue`), so the status alone does
//! not distinguish them: the error CODE is the requirement.
//!
//! Each row is one way the identifier can fail to resolve, and the suite keeps the control
//! that the same update SUCCEEDS while the queue is stored, so the refusals are about the
//! queue being gone and not a blanket answer. A missing queue is not worded the same way
//! by every operation that names one, so the per-operation sentences the delete and
//! placement paths answer with are pinned here too.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `REGION`) is the one
//! `hosting_loop.rs` defines, and the queue helpers are `game_session_queues.rs`'s.

use serde_json::{json, Value};

use crate::game_session_queues::{describe_queues, destinations, queue_arn, MAX_TIMEOUT};
use crate::hosting_loop::{GameLift, REGION};
// The refusal assertion `resource_deletion.rs` already drives the delete paths with: the
// suites under `tests/all/` are modules of ONE test binary, so it is shared rather than
// copied.
use crate::resource_deletion::assert_refused;

/// The sentence the queue update path refuses an identifier it cannot resolve with: fixed
/// text either side of the identifier the request supplied, and no trailing period.
fn unresolvable_queue_name_message(identifier: &str) -> String {
    format!("Queue name {identifier} does not exist")
}

/// Every way an `UpdateGameSessionQueue` `Name` can resolve to no stored queue earns the
/// same refusal: `InvalidRequestException` with the identifier the request supplied in the
/// sentence, whichever of the two forms `GameSessionQueueNameOrArn` accepts it took.
///
/// The rows differ along what the rule generalises over: whether the name never existed or
/// was deleted, which identifier form the request used, whether the ARN points at another
/// account or another Region for a name that IS stored, the LENGTH of the identifier at the
/// bounds the member allows (len 1..256), and which settings the refused update carried.
/// The control rows are the stored queue, which the same update changes.
#[tokio::test]
async fn an_update_naming_an_unresolvable_queue_is_refused_as_an_invalid_queue_name() {
    let gamelift = GameLift::new();
    let stored = gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({"Name": "the-surviving-queue", "Destinations": destinations(2)}),
        )
        .await["GameSessionQueue"]
        .clone();
    gamelift
        .ok("CreateGameSessionQueue", json!({"Name": "the-deleted-queue"}))
        .await;
    gamelift
        .ok(
            "DeleteGameSessionQueue",
            json!({"Name": "the-deleted-queue"}),
        )
        .await;

    // One identifier per row, each with the update body it is refused while carrying — a
    // refusal for the name the request makes, not for the settings it asks for.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a name no queue ever carried",
            "no-such-queue".to_string(),
            json!({"TimeoutInSeconds": MAX_TIMEOUT}),
        ),
        (
            "an ARN no queue ever carried",
            queue_arn("no-such-queue"),
            json!({"TimeoutInSeconds": 60}),
        ),
        (
            "the name of a deleted queue",
            "the-deleted-queue".to_string(),
            json!({"TimeoutInSeconds": 60}),
        ),
        (
            "the ARN of a deleted queue",
            queue_arn("the-deleted-queue"),
            json!({"Destinations": destinations(1)}),
        ),
        (
            "an ARN in another account for a stored name",
            format!("arn:aws:gamelift:{REGION}:999999999999:gamesessionqueue/the-surviving-queue"),
            json!({"TimeoutInSeconds": 60}),
        ),
        (
            "an ARN in another Region for a stored name",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:gamesessionqueue/the-surviving-queue",
                engine::emulator_account_id()
            ),
            json!({"Destinations": []}),
        ),
        (
            // `GameSessionQueueNameOrArn` is len 1..256, pattern
            // `^([a-zA-Z0-9-]+|arn:.*:gamesessionqueue\/[a-zA-Z0-9-]+)$`
            // (`smithy_query operation=UpdateGameSessionQueue`), so one character is the
            // shortest identifier a request may carry...
            "one character, the shortest identifier the member allows",
            "q".to_string(),
            json!({"TimeoutInSeconds": 60}),
        ),
        (
            // ...and 256 the longest.
            "256 characters, the longest identifier the member allows",
            "q".repeat(256),
            json!({"CustomEventData": "unreachable"}),
        ),
        (
            "a name differing from the stored one by one character",
            "the-surviving-queu".to_string(),
            json!({"TimeoutInSeconds": 60}),
        ),
    ];

    for (label, identifier, update) in &cases {
        let mut request = update.clone();
        request["Name"] = json!(identifier);
        assert_refused(
            &gamelift,
            "UpdateGameSessionQueue",
            request,
            "InvalidRequestException",
            &unresolvable_queue_name_message(identifier),
            label,
        )
        .await;

        // A refused update writes nothing: the queue that IS stored is exactly as created.
        let described = describe_queues(&gamelift, json!({"Names": ["the-surviving-queue"]})).await;
        assert_eq!(
            described,
            json!([stored.clone()]),
            "case {label:?}: a refused update must leave the stored queue as it was, got \
             {described}"
        );
    }

    // The control: the queue that IS stored takes the same update, so none of the rows
    // above is a blanket refusal.
    let updated = gamelift
        .ok(
            "UpdateGameSessionQueue",
            json!({"Name": "the-surviving-queue", "TimeoutInSeconds": MAX_TIMEOUT}),
        )
        .await;
    assert_eq!(
        updated["GameSessionQueue"]["TimeoutInSeconds"],
        json!(MAX_TIMEOUT),
        "the stored queue must take the update the unresolvable names were refused for, got \
         {updated}"
    );
}

/// A queue takes updates for as long as it is stored, and `DeleteGameSessionQueue` is what
/// ends that: after the delete the record is gone from the queue collection and the update
/// path refuses the identifier as one that does not exist.
///
/// The rows differ along which identifier form the delete and the later update name the
/// queue by, how many queues the store holds, which of them is deleted, and what the queue
/// carried when it was still stored.
#[tokio::test]
async fn a_queue_takes_updates_until_it_is_deleted_and_is_unresolvable_afterwards() {
    struct Case {
        label: &'static str,
        /// How many queues the store holds before the delete.
        store: usize,
        /// Which of them the request deletes.
        target: usize,
        /// Whether the delete and the later update name the queue by its ARN.
        by_arn: bool,
        /// How many destinations the deleted queue carried.
        destinations: usize,
        /// The timeout the surviving update set before the delete.
        timeout: i64,
    }

    let cases = [
        Case {
            label: "the only queue in the store, named by its name",
            store: 1,
            target: 0,
            by_arn: false,
            destinations: 0,
            timeout: 60,
        },
        Case {
            label: "the only queue in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            destinations: 2,
            timeout: MAX_TIMEOUT,
        },
        Case {
            label: "a middle queue of three, named by its name",
            store: 3,
            target: 1,
            by_arn: false,
            destinations: 1,
            timeout: 300,
        },
        Case {
            label: "the last of four queues, named by its ARN",
            store: 4,
            target: 3,
            by_arn: true,
            destinations: 3,
            timeout: 120,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        for index in 0..case.store {
            let name = format!("queue-{index}");
            let mut request = json!({"Name": &name});
            if index == case.target && case.destinations > 0 {
                request["Destinations"] = destinations(case.destinations);
            }
            gamelift.ok("CreateGameSessionQueue", request).await;
        }

        let name = format!("queue-{}", case.target);
        let selector = if case.by_arn {
            queue_arn(&name)
        } else {
            name.clone()
        };

        // While the queue is stored the update is served, and the new setting reads back
        // through the describe.
        let updated = gamelift
            .ok(
                "UpdateGameSessionQueue",
                json!({"Name": &selector, "TimeoutInSeconds": case.timeout}),
            )
            .await;
        assert_eq!(
            updated["GameSessionQueue"]["TimeoutInSeconds"],
            json!(case.timeout),
            "case {label:?}: a stored queue must take the update, got {updated}"
        );
        let described = describe_queues(&gamelift, json!({"Names": [&selector]})).await;
        assert_eq!(
            described[0]["TimeoutInSeconds"],
            json!(case.timeout),
            "case {label:?}: the updated timeout must read back, got {described}"
        );

        // The delete removes the record, so the name stops resolving...
        let (status, body) = gamelift
            .call("DeleteGameSessionQueue", &json!({"Name": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored queue must succeed, got HTTP {status} with \
             body {body}"
        );
        let described = describe_queues(&gamelift, json!({})).await;
        let remaining = described.as_array().map_or(0, Vec::len);
        assert_eq!(
            remaining,
            case.store - 1,
            "case {label:?}: the collection must be one queue shorter, got {described}"
        );
        assert_eq!(
            describe_queues(&gamelift, json!({"Names": [&selector]})).await,
            json!([]),
            "case {label:?}: the deleted queue must be gone from the collection"
        );

        // ...and the update that was served a moment ago is now refused as a name that
        // does not exist, by either identifier form.
        for probe in [name.clone(), queue_arn(&name)] {
            assert_refused(
                &gamelift,
                "UpdateGameSessionQueue",
                json!({"Name": &probe, "TimeoutInSeconds": case.timeout}),
                "InvalidRequestException",
                &unresolvable_queue_name_message(&probe),
                label,
            )
            .await;

            // The other operations that name a queue keep the sentence each of THEM words
            // a missing queue with: AWS does not refuse it the same way everywhere.
            assert_refused(
                &gamelift,
                "DeleteGameSessionQueue",
                json!({"Name": &probe}),
                "NotFoundException",
                &format!("Game session queue {probe} does not exist."),
                label,
            )
            .await;
            assert_refused(
                &gamelift,
                "StartGameSessionPlacement",
                json!({
                    "PlacementId": format!("placement-{}", case.target),
                    "GameSessionQueueName": &probe,
                    "MaximumPlayerSessionCount": 4,
                }),
                "NotFoundException",
                &format!("Game session queue {probe} does not exist."),
                label,
            )
            .await;
        }

        // The queues the delete did not name still take updates.
        for index in (0..case.store).filter(|index| *index != case.target) {
            let other = format!("queue-{index}");
            let updated = gamelift
                .ok(
                    "UpdateGameSessionQueue",
                    json!({"Name": &other, "TimeoutInSeconds": 90}),
                )
                .await;
            assert_eq!(
                updated["GameSessionQueue"]["TimeoutInSeconds"],
                json!(90),
                "case {label:?}: queue {other} was not deleted and must still take an update, \
                 got {updated}"
            );
        }
    }
}
