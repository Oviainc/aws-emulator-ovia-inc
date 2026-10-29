// SPDX-License-Identifier: CC0-1.0

//! `GetPolicyStore`'s `tags` output member: present only when the request asked
//! for the tags AND the store carries at least one.
//!
//! The model documents both halves on `GetPolicyStoreInput$tags`: including the
//! parameter returns the tags, and including it for a store with no tags leaves
//! the `tags` response member OMITTED. "Absent" and "present but empty" are
//! therefore different answers, and an empty map is never one of them.
//!
//! Every assertion goes over the wire through the shared harness, so a tag set a
//! row builds is only visible to the read if it really reached state.

use serde_json::{json, Value};

use crate::harness::{as_map, call, listed_tags, ok, router};

/// One row: how a store's tag set was built, and the set it ends up holding.
struct Case {
    case: &'static str,
    /// Extra members merged into the `CreatePolicyStore` request (`tags`, when
    /// the row creates the store already tagged).
    create_tags: Option<Value>,
    /// A `TagResource` merge applied after the create, when the row has one.
    tag: Option<Value>,
    /// An `UntagResource` key list applied last, when the row has one.
    untag: Option<Value>,
    /// The tag set the store holds once those steps are done.
    expected: Value,
}

/// `tags: true` reports exactly the persisted set, and omits the member entirely
/// when that set is empty — however the set came to be empty.
#[tokio::test]
async fn get_policy_store_omits_tags_for_a_store_that_carries_none() {
    // Rows differ along how many tags the store holds (none, one, several), the
    // shape of the values (ordinary, empty string), and how the set got there
    // (create-time, added later, all removed, partly removed).
    let cases = [
        Case {
            case: "never tagged",
            create_tags: None,
            tag: None,
            untag: None,
            expected: json!({}),
        },
        Case {
            case: "created with an empty tag map",
            create_tags: Some(json!({})),
            tag: None,
            untag: None,
            expected: json!({}),
        },
        Case {
            case: "one create-time tag",
            create_tags: Some(json!({ "env": "test" })),
            tag: None,
            untag: None,
            expected: json!({ "env": "test" }),
        },
        Case {
            case: "several create-time tags",
            create_tags: Some(json!({ "env": "prod", "team": "photos", "tier": "gold" })),
            tag: None,
            untag: None,
            expected: json!({ "env": "prod", "team": "photos", "tier": "gold" }),
        },
        Case {
            case: "a tag value that is the empty string",
            create_tags: Some(json!({ "owner": "" })),
            tag: None,
            untag: None,
            expected: json!({ "owner": "" }),
        },
        Case {
            case: "tagged after the create",
            create_tags: None,
            tag: Some(json!({ "team": "videos" })),
            untag: None,
            expected: json!({ "team": "videos" }),
        },
        Case {
            case: "every tag removed again",
            create_tags: Some(json!({ "env": "test" })),
            tag: Some(json!({ "team": "videos" })),
            untag: Some(json!(["env", "team"])),
            expected: json!({}),
        },
        Case {
            case: "some tags removed, one left",
            create_tags: Some(json!({ "env": "test", "team": "videos" })),
            tag: None,
            untag: Some(json!(["env"])),
            expected: json!({ "team": "videos" }),
        },
    ];

    let router = router();
    for case in &cases {
        let name = case.case;
        let mut request = json!({ "validationSettings": { "mode": "OFF" } });
        if let Some(tags) = &case.create_tags {
            request["tags"] = tags.clone();
        }
        let created = ok(&router, "CreatePolicyStore", &request).await;
        let id = created["policyStoreId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        if let Some(tags) = &case.tag {
            ok(
                &router,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": tags }),
            )
            .await;
        }
        if let Some(keys) = &case.untag {
            ok(
                &router,
                "UntagResource",
                &json!({ "resourceArn": arn, "tagKeys": keys }),
            )
            .await;
        }

        let expected = as_map(&case.expected);

        // The tag set really is persisted: the operation whose whole job is to
        // report it answers with the same set (and, unlike GetPolicyStore, with
        // an empty map when there is none).
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: ListTagsForResource reported a different set, got {listed:?}"
        );

        let fetched = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": id, "tags": true }),
        )
        .await;
        if expected.is_empty() {
            assert!(
                fetched.get("tags").is_none(),
                "case {name:?}: a store with no tags must omit the tags member, got {fetched}"
            );
        } else {
            assert!(
                fetched.get("tags").is_some(),
                "case {name:?}: expected the tags member to be present, got {fetched}"
            );
            assert_eq!(
                as_map(&fetched["tags"]),
                expected,
                "case {name:?}: GetPolicyStore reported a different tag set, got {fetched}"
            );
        }

        // Not asking for the tags omits the member whatever the store holds —
        // whether the parameter is left out or sent as false.
        for (ask_case, body) in [
            ("tags omitted", json!({ "policyStoreId": id })),
            ("tags: false", json!({ "policyStoreId": id, "tags": false })),
        ] {
            let (status, unasked) = call(&router, "GetPolicyStore", &body).await;
            assert_eq!(
                status, 200,
                "case {name:?}/{ask_case:?}: expected HTTP 200, got {status} {unasked}"
            );
            assert!(
                unasked.get("tags").is_none(),
                "case {name:?}/{ask_case:?}: tags must be absent unless asked for, got {unasked}"
            );
        }
    }
}
