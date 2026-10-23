// SPDX-License-Identifier: CC0-1.0

//! What a tag key may be, and that the service keeps it.
//!
//! `TagMap` is `map<TagKey, TagValue>` and the model constrains neither member,
//! so a tag key is stored and returned VERBATIM — no key is filtered, rewritten
//! or dropped, including one carrying the `aws:` prefix AWS reserves elsewhere,
//! colons, punctuation or non-ASCII characters. The only bound the service
//! enforces is the documented tag-key length, which the longest row sits on.
//!
//! Each row is exercised on every path a tag travels: written through
//! `CreateSavingsPlan`'s own `tags` and through `TagResource`, and read back
//! through both `ListTagsForResource` and `DescribeSavingsPlans`.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots,
//! reusing the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR};

#[tokio::test]
async fn a_tag_key_survives_verbatim_on_every_path() {
    let emulator = Emulator::new();
    let offering_id = text(
        &emulator.offering("Compute", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // (case, key) — the key set a caller may use. `MAX_TAG_KEY_CHARS` is the
    // only bound the service enforces, so the longest row sits exactly on it.
    let longest = "k".repeat(state::MAX_TAG_KEY_CHARS);
    let cases: [(&str, &str); 9] = [
        ("a plain key", "env"),
        ("the prefix AWS reserves elsewhere", "aws:tag:domain"),
        ("the bare reserved prefix", "aws:"),
        ("the reserved prefix in upper case", "AWS:Owner"),
        ("colons throughout", "a:b:c"),
        ("an ARN as a key", "arn:aws:iam::000000000000:role/App"),
        ("punctuation", "dots.and-dashes_1/slash@at"),
        ("non-ASCII", "équipe:coût"),
        ("the longest key the service accepts", longest.as_str()),
    ];

    for (case, key) in cases {
        // Path 1: the purchase carries the tag.
        let created = emulator
            .purchase(json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": "0.1",
                "tags": { key: "created" },
            }))
            .await;
        let created_arn = text(&emulator.plan(&created).await, "savingsPlanArn");
        for (reader, tags) in [
            (
                "ListTagsForResource",
                emulator
                    .call("ListTagsForResource", json!({ "resourceArn": created_arn }))
                    .await
                    .get("tags")
                    .clone(),
            ),
            (
                "DescribeSavingsPlans",
                emulator
                    .plan(&created)
                    .await
                    .get("tags")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
        ] {
            assert_eq!(
                tags.get(key).and_then(Value::as_str),
                Some("created"),
                "case {case:?} ({key:?}) written by CreateSavingsPlan: {reader} must report it \
                 verbatim, got {tags:?}"
            );
        }

        // Path 2: TagResource writes the tag onto a plan that does not carry it.
        let tagged = emulator
            .purchase(json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": "0.1",
            }))
            .await;
        let tagged_arn = text(&emulator.plan(&tagged).await, "savingsPlanArn");
        let written = emulator
            .call(
                "TagResource",
                json!({ "resourceArn": tagged_arn, "tags": { key: "written" } }),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?} ({key:?}): TagResource must accept it, got {} {:?}",
            written.status, written.body
        );
        let listed = emulator
            .call("ListTagsForResource", json!({ "resourceArn": tagged_arn }))
            .await;
        assert_eq!(
            listed.get("tags").get(key).and_then(Value::as_str),
            Some("written"),
            "case {case:?} ({key:?}) written by TagResource: ListTagsForResource must report it \
             verbatim, got {:?}",
            listed.body
        );
        assert_eq!(
            listed
                .get("tags")
                .as_object()
                .map(serde_json::Map::len)
                .unwrap_or_default(),
            1,
            "case {case:?} ({key:?}): exactly the one tag written must come back, got {:?}",
            listed.body
        );
    }
}
