// SPDX-License-Identifier: CC0-1.0

//! How a refused tag write is REPORTED: the split between the service's two
//! tag refusals.
//!
//! The rule these suites pin: a tag write refused because the resource would
//! end up carrying more tags than the per-resource tag allowance has exhausted
//! a quota, so every operation that declares `ServiceQuotaExceededException`
//! answers it with that error (HTTP 402); a tag whose key or value falls
//! outside the length the service accepts is a malformed input parameter, so it
//! stays `ValidationException` (HTTP 400). Either refusal stores nothing.
//!
//! The rows vary what makes the write exceed the allowance — how many tags the
//! resource already carries, how many the request adds, whether the request
//! overwrites keys instead of adding them, and whether it arrives one tag at a
//! time or all at once — and are exercised on both paths a tag set is written:
//! `TagResource` and `CreateSavingsPlan`'s own `tags`. Each suite also sits a
//! row exactly ON the allowance and the length bounds, so a refusal that fires
//! one tag early would fail it.
//!
//! Everything here goes over the wire through the same `MultiServiceRouter` the
//! emulator boots, reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Map, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

/// What the service must answer a row with: the HTTP status and the wire error
/// code (empty on success).
type Expected = (u16, &'static str);

/// One row of the `TagResource` allowance table: the case, how many tags the
/// plan already carries, how many the write ADDS (`None` to overwrite the
/// seeded keys instead of adding any), and what the service must answer.
type AllowanceRow = (&'static str, usize, Option<usize>, Expected);

/// `count` distinct tags, keyed `<prefix>0..<count>`.
fn tags(prefix: &str, count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("{prefix}{index}"), json!("v"));
    }
    Value::Object(map)
}

/// A plan to tag, and its ARN.
async fn fresh_plan_arn(emulator: &Emulator, offering_id: &str) -> String {
    let savings_plan_id = emulator
        .purchase(json!({ "savingsPlanOfferingId": offering_id, "commitment": "0.1" }))
        .await;
    text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn")
}

/// The tag set `ListTagsForResource` reports for `arn`.
async fn stored_tags(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator
        .call("ListTagsForResource", json!({ "resourceArn": arn }))
        .await;
    assert_eq!(
        answer.status, 200,
        "listing the tags of {arn}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.get("tags").clone()
}

/// How many tags `arn` carries.
async fn stored_tag_count(emulator: &Emulator, arn: &str) -> usize {
    stored_tags(emulator, arn)
        .await
        .as_object()
        .map_or(0, serde_json::Map::len)
}

/// `TagResource` is bounded by the per-resource tag allowance, and a write that
/// does not fit it is the operation's modelled quota failure — not a
/// constraint one.
///
/// Each row says how many tags the plan already carries, what the write adds,
/// and what the service must answer; `seeded` keys are reused by a row that
/// overwrites rather than grows, which is why a write at the allowance can
/// still be accepted.
#[tokio::test]
async fn a_tag_write_that_does_not_fit_the_allowance_is_a_quota_failure() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let limit = state::MAX_TAGS_PER_PLAN;

    // (case, how many tags to seed, the write, the expected (status, code)).
    // `None` as the write means "overwrite every seeded key", which is not
    // growth. The seeded tags are written in ONE TagResource call and the write
    // under test in the next, so each row exercises a merge against stored tags.
    let rows: Vec<AllowanceRow> = vec![
        ("an empty plan, one tag", 0, Some(1), (200, "")),
        ("an empty plan, one short of the allowance", 0, Some(limit - 1), (200, "")),
        ("an empty plan, exactly the allowance", 0, Some(limit), (200, "")),
        (
            "an empty plan, one past the allowance in one request",
            0,
            Some(limit + 1),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "an empty plan, twice the allowance in one request",
            0,
            Some(limit * 2),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "one short of the allowance, the tag that reaches it",
            limit - 1,
            Some(1),
            (200, ""),
        ),
        (
            "one short of the allowance, two more tags",
            limit - 1,
            Some(2),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "at the allowance, one more tag",
            limit,
            Some(1),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "at the allowance, a whole further allowance of tags",
            limit,
            Some(limit),
            (402, "ServiceQuotaExceededException"),
        ),
        ("at the allowance, overwriting every key", limit, None, (200, "")),
    ];

    for (case, seeded, added, expected) in rows {
        let arn = fresh_plan_arn(&emulator, &offering_id).await;
        if seeded > 0 {
            let answer = emulator
                .call(
                    "TagResource",
                    json!({ "resourceArn": arn, "tags": tags("seed", seeded) }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: seeding {seeded} tags must be accepted, got {} {:?}",
                answer.status, answer.body
            );
        }
        let before = stored_tags(&emulator, &arn).await;
        let write = match added {
            Some(count) => tags("add", count),
            None => tags("seed", seeded),
        };
        let write_len = write.as_object().map_or(0, serde_json::Map::len);

        let answer = emulator
            .call("TagResource", json!({ "resourceArn": arn, "tags": write }))
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {} {:?}",
            answer.status,
            answer.body
        );

        let after = stored_tags(&emulator, &arn).await;
        if expected.0 == 200 {
            let expected_count = match added {
                Some(count) => seeded + count,
                None => seeded,
            };
            assert_eq!(
                after.as_object().map_or(0, serde_json::Map::len),
                expected_count,
                "case {case:?}: an accepted write of {write_len} tags must leave \
                 {expected_count} stored, got {after:?}"
            );
        } else {
            assert_eq!(
                after, before,
                "case {case:?}: a refused write of {write_len} tags must leave the stored \
                 tags unchanged, got {after:?}"
            );
        }
    }
}

/// A tag whose key or value is outside the length the service accepts is a
/// CONSTRAINT failure, so it stays a 400 `ValidationException` even though the
/// operation also models a quota error — and a tag sitting exactly on either
/// bound is accepted.
///
/// The lengths are counted in characters, which is why a row uses a multi-byte
/// character: `state::MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS` are the
/// documented key and value lengths.
#[tokio::test]
async fn a_tag_outside_its_length_stays_a_constraint_failure() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let key_max = state::MAX_TAG_KEY_CHARS;
    let value_max = state::MAX_TAG_VALUE_CHARS;

    // (case, key, value, the expected (status, code)) — each bound, one past
    // each bound, and a multi-byte spelling of each.
    let rows: Vec<(&str, String, String, Expected)> = vec![
        ("a short key and value", "env".to_string(), "test".to_string(), (200, "")),
        (
            "a key exactly at the bound",
            "k".repeat(key_max),
            "v".to_string(),
            (200, ""),
        ),
        (
            "a key one character past the bound",
            "k".repeat(key_max + 1),
            "v".to_string(),
            (400, "ValidationException"),
        ),
        (
            "a multi-byte key exactly at the bound",
            "é".repeat(key_max),
            "v".to_string(),
            (200, ""),
        ),
        (
            "a multi-byte key one character past the bound",
            "é".repeat(key_max + 1),
            "v".to_string(),
            (400, "ValidationException"),
        ),
        (
            "a value exactly at the bound",
            "k".to_string(),
            "v".repeat(value_max),
            (200, ""),
        ),
        (
            "a value one character past the bound",
            "k".to_string(),
            "v".repeat(value_max + 1),
            (400, "ValidationException"),
        ),
        (
            "a multi-byte value one character past the bound",
            "k".to_string(),
            "é".repeat(value_max + 1),
            (400, "ValidationException"),
        ),
        (
            "an over-long key and an over-long value",
            "k".repeat(key_max + 1),
            "v".repeat(value_max + 1),
            (400, "ValidationException"),
        ),
    ];

    for (case, key, value, expected) in rows {
        let arn = fresh_plan_arn(&emulator, &offering_id).await;
        let answer = emulator
            .call(
                "TagResource",
                json!({ "resourceArn": arn, "tags": { key.clone(): value.clone() } }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {} {:?}",
            answer.status,
            answer.body
        );

        let stored = stored_tags(&emulator, &arn).await;
        if expected.0 == 200 {
            assert_eq!(
                stored.get(&key).and_then(Value::as_str),
                Some(value.as_str()),
                "case {case:?}: an accepted tag must be readable back, got {stored:?}"
            );
        } else {
            assert_eq!(
                stored.as_object().map_or(0, serde_json::Map::len),
                0,
                "case {case:?}: a refused tag write must store nothing, got {stored:?}"
            );
        }
    }

    // And the shape is what the refusal names even when the same request would
    // also not fit the allowance: a malformed tag is refused as malformed
    // however many tags came with it.
    let arn = fresh_plan_arn(&emulator, &offering_id).await;
    let seeded = emulator
        .call(
            "TagResource",
            json!({ "resourceArn": arn, "tags": tags("seed", state::MAX_TAGS_PER_PLAN) }),
        )
        .await;
    assert_eq!(
        seeded.status, 200,
        "case \"at the allowance, then an over-long key\": seeding must be accepted, got {} {:?}",
        seeded.status, seeded.body
    );
    let answer = emulator
        .call(
            "TagResource",
            json!({ "resourceArn": arn, "tags": { "k".repeat(key_max + 1): "v" } }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (400, "ValidationException"),
        "case \"at the allowance, then an over-long key\": expected a 400 ValidationException, \
         got {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        stored_tag_count(&emulator, &arn).await,
        state::MAX_TAGS_PER_PLAN,
        "case \"at the allowance, then an over-long key\": the stored tags must be unchanged, \
         got {:?}",
        stored_tags(&emulator, &arn).await
    );
}

/// `CreateSavingsPlan` carries its own `tags`, and is held to the same split:
/// a purchase whose tag set does not fit the allowance is the 402 quota
/// failure, a malformed tag the 400 constraint one, and a purchase at the
/// allowance succeeds with every tag readable back.
#[tokio::test]
async fn a_purchase_is_held_to_the_same_split() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let limit = state::MAX_TAGS_PER_PLAN;
    let long_key = "k".repeat(state::MAX_TAG_KEY_CHARS + 1);
    let long_value = "v".repeat(state::MAX_TAG_VALUE_CHARS + 1);

    // (case, the purchase's tags, the expected (status, code)).
    let rows: Vec<(&str, Value, Expected)> = vec![
        ("no tags at all", json!({}), (200, "")),
        ("one short of the allowance", tags("t", limit - 1), (200, "")),
        ("exactly the allowance", tags("t", limit), (200, "")),
        (
            "one past the allowance",
            tags("t", limit + 1),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "twice the allowance",
            tags("t", limit * 2),
            (402, "ServiceQuotaExceededException"),
        ),
        (
            "an over-long key",
            json!({ long_key.clone(): "v" }),
            (400, "ValidationException"),
        ),
        (
            "an over-long value",
            json!({ "short": long_value.clone() }),
            (400, "ValidationException"),
        ),
    ];

    for (case, purchase_tags, expected) in rows {
        let expected_count = purchase_tags.as_object().map_or(0, serde_json::Map::len);
        let answer = emulator
            .call(
                "CreateSavingsPlan",
                json!({
                    "savingsPlanOfferingId": offering_id,
                    "commitment": "0.1",
                    "tags": purchase_tags,
                }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {} {:?}",
            answer.status,
            answer.body
        );
        if expected.0 != 200 {
            continue;
        }
        // Round trip: the tags the accepted purchase carried are what the plan
        // reports afterwards.
        let savings_plan_id = answer
            .get("savingsPlanId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let arn = text(&emulator.plan(&savings_plan_id).await, "savingsPlanArn");
        assert_eq!(
            stored_tag_count(&emulator, &arn).await,
            expected_count,
            "case {case:?}: the purchase's {expected_count} tags must be readable back, got {:?}",
            stored_tags(&emulator, &arn).await
        );
    }
}

/// The operations that cannot grow a tag set never report the quota failure:
/// `UntagResource` only shrinks it, so even a request naming more keys than the
/// allowance is accepted — which is the asymmetry the model draws by declaring
/// `ServiceQuotaExceededException` on the tag WRITE alone.
#[tokio::test]
async fn removing_tags_can_never_exceed_the_allowance() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let limit = state::MAX_TAGS_PER_PLAN;
    let arn = fresh_plan_arn(&emulator, &offering_id).await;
    let seeded = emulator
        .call(
            "TagResource",
            json!({ "resourceArn": arn, "tags": tags("seed", limit) }),
        )
        .await;
    assert_eq!(
        seeded.status, 200,
        "seeding {limit} tags: expected 200, got {} {:?}",
        seeded.status, seeded.body
    );

    // (case, the keys to remove, how many tags must remain).
    let every_seeded: Vec<String> = (0..limit).map(|index| format!("seed{index}")).collect();
    let mut more_keys_than_the_allowance = every_seeded.clone();
    more_keys_than_the_allowance.extend((0..limit).map(|index| format!("absent{index}")));
    let rows: Vec<(&str, Vec<String>, usize)> = vec![
        ("one key the plan carries", vec!["seed0".to_string()], limit - 1),
        ("a key the plan never carried", vec!["absent".to_string()], limit - 1),
        (
            "more keys than the allowance, most of them absent",
            more_keys_than_the_allowance,
            0,
        ),
        ("every remaining key again", every_seeded, 0),
    ];

    for (case, tag_keys, remaining) in rows {
        let answer = emulator
            .call(
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": tag_keys }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (200, ""),
            "case {case:?}: removing tags must be accepted, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            stored_tag_count(&emulator, &arn).await,
            remaining,
            "case {case:?}: expected {remaining} tags left, got {:?}",
            stored_tags(&emulator, &arn).await
        );
    }
}
