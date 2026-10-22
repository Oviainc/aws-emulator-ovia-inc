// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchGetStreamKey`, driven over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with
//! a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! A batch read answers ONE result per requested ARN, in the order the request
//! listed them: an ARN that names a persisted stream key contributes that key,
//! exactly as the call that created it reported it, to `streamKeys`; an ARN that
//! names none contributes a `BatchError` naming that ARN with the code of the error
//! the single-ARN read models for it; and the batch itself succeeds (HTTP 200)
//! whatever the mix. Being a read, it changes nothing a later caller sees.
//!
//! The rows differ along what the rule generalises over: how many ARNs are asked
//! for, which of them resolve, their order, whether they repeat, and which channel
//! owns each key — up to the modelled `StreamKeyArnList` bound of 50 entries.
//!
//! The wire harness is the one `channel_lifecycle` owns, shared with the other
//! suites in this target rather than copied.

use serde_json::{json, Value};

use super::channel_lifecycle::{text, Ivs, STREAM_KEY_ARN_PREFIX};

/// The `BatchError$code` a requested ARN that names no stream key carries: the shape
/// name of the error `GetStreamKey` models for a resource that does not exist
/// (`ResourceNotFoundException`, HTTP 404), because `BatchGetStreamKey` is
/// documented as performing `GetStreamKey` on each ARN.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The modelled `StreamKeyArnList` bound: `length(min: 1, max: 50)`.
const MIN_ARNS: usize = 1;
const MAX_ARNS: usize = 50;

/// One slot of a request's `arns` list: an index into the fixture's persisted keys,
/// or into its ARNs that name no key.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Found(usize),
    Missing(usize),
}

/// The state every row reads: four persisted stream keys, one per channel (a channel's
/// stream-key quota is one key), one of them tagged and two of them minted by
/// `CreateStreamKey` into the slot a delete freed, plus two well-formed `StreamKeyArn`s
/// that name no persisted key — one never minted, one whose key existed and was deleted.
struct Fixture {
    ivs: Ivs,
    keys: Vec<Value>,
    missing: Vec<String>,
}

impl Fixture {
    async fn build() -> Self {
        let ivs = Ivs::new();

        // The key `CreateChannel` mints with its channel.
        let first = ivs.ok("CreateChannel", json!({ "name": "first" })).await;
        let mut keys = vec![first["streamKey"].clone()];

        // Two keys `CreateStreamKey` mints, one tagged: a channel is created WITH its
        // one key, so each of these needs its channel's quota slot freed first — the
        // delete-then-create a client rotates a key with.
        for tags in [json!({}), json!({ "team": "live" })] {
            let owner = ivs.ok("CreateChannel", json!({ "name": "rotated" })).await;
            let owner_arn = text(&owner["channel"], "arn");
            let freed = ivs
                .call(
                    "DeleteStreamKey",
                    &json!({ "arn": text(&owner["streamKey"], "arn") }),
                )
                .await;
            assert!(
                freed.status < 300,
                "case fixture: DeleteStreamKey must free the channel's quota slot, got {} {}",
                freed.status,
                freed.body
            );
            let mut request = json!({ "channelArn": owner_arn });
            if tags.as_object().is_some_and(|tags| !tags.is_empty()) {
                request["tags"] = tags;
            }
            keys.push(ivs.ok("CreateStreamKey", request).await["streamKey"].clone());
        }

        let second = ivs.ok("CreateChannel", json!({ "name": "second" })).await;
        keys.push(second["streamKey"].clone());

        // A key that was persisted and then removed with its channel is as absent as
        // one that never existed, so both forms of "names no key" are covered.
        let doomed = ivs.ok("CreateChannel", json!({ "name": "doomed" })).await;
        let doomed_key = text(&doomed["streamKey"], "arn");
        let deleted = ivs
            .call(
                "DeleteChannel",
                &json!({ "arn": text(&doomed["channel"], "arn") }),
            )
            .await;
        assert!(
            deleted.status < 300,
            "case fixture: DeleteChannel must succeed, got {} {}",
            deleted.status,
            deleted.body
        );

        Self {
            ivs,
            keys,
            missing: vec![format!("{STREAM_KEY_ARN_PREFIX}neverminted0001"), doomed_key],
        }
    }

    /// The ARN each slot asks for.
    fn arns(&self, slots: &[Slot]) -> Vec<String> {
        slots
            .iter()
            .map(|slot| match *slot {
                Slot::Found(index) => text(&self.keys[index], "arn"),
                Slot::Missing(index) => self.missing[index].clone(),
            })
            .collect()
    }

    /// Ask for `slots` and assert the rule holds for this row.
    async fn assert_row(&self, case: &str, slots: &[Slot]) -> Value {
        let arns = self.arns(slots);
        let body = self
            .ivs
            .ok("BatchGetStreamKey", json!({ "arns": arns }))
            .await;

        let got_keys = body["streamKeys"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: streamKeys must be a list, got {body}"));
        let got_errors = body["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: errors must be a list, got {body}"));

        let expected_keys: Vec<&Value> = slots
            .iter()
            .filter_map(|slot| match *slot {
                Slot::Found(index) => Some(&self.keys[index]),
                Slot::Missing(_) => None,
            })
            .collect();
        let expected_missing: Vec<&String> = slots
            .iter()
            .filter_map(|slot| match *slot {
                Slot::Found(_) => None,
                Slot::Missing(index) => Some(&self.missing[index]),
            })
            .collect();

        assert_eq!(
            got_keys.len() + got_errors.len(),
            slots.len(),
            "case {case:?}: expected one result per requested arn ({}), got {body}",
            slots.len()
        );
        assert_eq!(
            got_keys.len(),
            expected_keys.len(),
            "case {case:?}: expected {} resolved keys, got {body}",
            expected_keys.len()
        );
        for (position, expected) in expected_keys.iter().enumerate() {
            assert_eq!(
                &got_keys[position], *expected,
                "case {case:?}: streamKeys[{position}] must be the key its create reported, \
                 got {}",
                got_keys[position]
            );
        }
        assert_eq!(
            got_errors.len(),
            expected_missing.len(),
            "case {case:?}: expected {} per-arn errors, got {body}",
            expected_missing.len()
        );
        for (position, expected) in expected_missing.iter().enumerate() {
            let error = &got_errors[position];
            assert_eq!(
                &text(error, "arn"),
                *expected,
                "case {case:?}: errors[{position}] must name the arn that resolved to nothing, \
                 got {error}"
            );
            assert_eq!(
                text(error, "code"),
                NOT_FOUND_CODE,
                "case {case:?}: errors[{position}] must carry the not-found code, got {error}"
            );
            assert!(
                !text(error, "message").is_empty(),
                "case {case:?}: errors[{position}] must carry a message, got {error}"
            );
        }
        body
    }
}

/// The rule, over a table of requests that differ in size, mix, order, repetition
/// and owning channel.
#[tokio::test]
async fn batch_get_stream_key_answers_one_result_per_requested_arn() {
    let fixture = Fixture::build().await;

    // A row of exactly the modelled maximum: 50 slots, cycling through every
    // persisted key and both absent ARNs.
    let at_the_cap: Vec<Slot> = (0..MAX_ARNS)
        .map(|index| match index % 6 {
            slot @ 0..=3 => Slot::Found(slot),
            4 => Slot::Missing(0),
            _ => Slot::Missing(1),
        })
        .collect();

    let cases: Vec<(&str, Vec<Slot>)> = vec![
        ("one arn, resolved", vec![Slot::Found(0)]),
        ("one arn, never minted", vec![Slot::Missing(0)]),
        ("one arn, deleted with its channel", vec![Slot::Missing(1)]),
        (
            "resolved then absent",
            vec![Slot::Found(1), Slot::Missing(0)],
        ),
        (
            "absent then resolved (reversed order)",
            vec![Slot::Missing(0), Slot::Found(1)],
        ),
        (
            "every persisted key, across two channels",
            vec![
                Slot::Found(0),
                Slot::Found(1),
                Slot::Found(2),
                Slot::Found(3),
            ],
        ),
        (
            "both absent forms, nothing resolved",
            vec![Slot::Missing(1), Slot::Missing(0)],
        ),
        (
            "one arn repeated",
            vec![Slot::Found(2), Slot::Found(2), Slot::Missing(0), Slot::Missing(0)],
        ),
        (
            "interleaved partial failure",
            vec![
                Slot::Found(3),
                Slot::Missing(0),
                Slot::Found(0),
                Slot::Missing(1),
                Slot::Found(2),
            ],
        ),
        ("exactly the modelled maximum of 50 arns", at_the_cap),
    ];

    for (case, slots) in &cases {
        assert!(
            (MIN_ARNS..=MAX_ARNS).contains(&slots.len()),
            "case {case:?}: the row itself must stay inside the modelled bound, got {}",
            slots.len()
        );
        fixture.assert_row(case, slots).await;
    }

    // The tagged key really does carry its tags, so comparing whole key objects
    // above is comparing something richer than an ARN.
    let tagged = fixture
        .assert_row("tagged key", &[Slot::Found(2)])
        .await;
    assert_eq!(
        tagged["streamKeys"][0]["tags"],
        json!({ "team": "live" }),
        "case tagged key: a batch read must answer the tags the create persisted, got {tagged}"
    );
}

/// The batch read is a read: repeating it answers the same thing, and it neither
/// mints nor removes a key.
#[tokio::test]
async fn batch_get_stream_key_changes_nothing() {
    let fixture = Fixture::build().await;
    let slots = [Slot::Found(0), Slot::Missing(0), Slot::Found(1)];

    let first = fixture.assert_row("round 1", &slots).await;
    let second = fixture.assert_row("round 2", &slots).await;
    assert_eq!(
        first, second,
        "case repeated read: two identical batch reads must answer identically, got {first} \
         then {second}"
    );

    // Every key the first channel owns is still exactly the one `CreateChannel` minted
    // with it: a channel's quota is one key, and a read adds none.
    let owner = text(&fixture.keys[0], "channelArn");
    let listed = fixture
        .ivs
        .ok("ListStreamKeys", json!({ "channelArn": owner }))
        .await;
    assert_eq!(
        listed["streamKeys"].as_array().map(Vec::len),
        Some(1),
        "case after reads: a read must not add or remove a stream key, got {listed}"
    );
}

/// The modelled input constraints: `arns` is required, `length(1..50)`, and every
/// member matches the `StreamKeyArn` pattern. Each published bound and one past it.
#[tokio::test]
async fn batch_get_stream_key_refuses_input_outside_the_modelled_bounds() {
    let fixture = Fixture::build().await;
    let good = text(&fixture.keys[0], "arn");

    let one_past_the_top: Vec<String> = (0..=MAX_ARNS).map(|_| good.clone()).collect();
    let cases: Vec<(&str, Value)> = vec![
        ("arns omitted", json!({})),
        ("arns empty, one below the minimum of 1", json!({ "arns": [] })),
        (
            "51 arns, one past the maximum of 50",
            json!({ "arns": one_past_the_top }),
        ),
        (
            "an arn that is not a StreamKeyArn",
            json!({ "arns": ["arn:aws:ivs:us-east-1:000000000000:channel/abcdef"] }),
        ),
        (
            "an arn that is not an arn at all",
            json!({ "arns": ["not-an-arn"] }),
        ),
    ];

    for (case, body) in &cases {
        let reply = fixture.ivs.call("BatchGetStreamKey", body).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // And the published bounds themselves are accepted.
    for count in [MIN_ARNS, MAX_ARNS] {
        let arns: Vec<String> = (0..count).map(|_| good.clone()).collect();
        let body = fixture
            .ivs
            .ok("BatchGetStreamKey", json!({ "arns": arns }))
            .await;
        assert_eq!(
            body["streamKeys"].as_array().map(Vec::len),
            Some(count),
            "case {count} arns at a published bound: expected {count} results, got {body}"
        );
    }
}
