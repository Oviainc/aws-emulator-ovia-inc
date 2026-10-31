// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the per-resource tag limit on the CREATE path.
//!
//! The rule under test: the per-resource tag quota bounds the tag set a resource
//! ENDS UP holding, whichever call writes it — so a create request whose `tags`
//! map carries more than the quota is refused and no resource is created, and
//! every resource a create DOES produce holds a set the tagging path can still
//! work with.
//!
//! The rows span the SIZE of the create-time tag map: none at all, one, one
//! below the quota, exactly the quota, one past it, well past it, and the
//! modelled per-request maximum — which the quota refuses even though the model
//! admits it, because the two bounds measure different things.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, over ONE shared engine, reusing the harness in
//! `harness.rs` — so a tag is only visible to a later request if it really
//! reached state.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    as_map, call, error_code, fault_message, field_list, listed_tags, ok, router,
};

/// The published *Tags per resource* quota, which the model states twice: in
/// `TagResource`'s documentation ("You can associate as many as 50 tags with a
/// resource") and in `TooManyTagsException`'s ("the limit (50)").
///
/// Written out here rather than imported on purpose: this suite pins the
/// PUBLISHED quota, so a change to the implementation constant has to fail this
/// test instead of moving with it.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagMap`'s modelled length maximum, from `smithy_query(query="operation",
/// operation="CreatePolicyStore")`: `tags` is a `TagMap` of length ..200. It
/// bounds ONE request's map, not what a resource may hold, so a map of this many
/// tags passes the model's own constraint and still has to be refused by the
/// quota above.
const MAX_TAGS_PER_REQUEST: usize = 200;

/// The JSON pointer the refusal names, as `src/ops/mod.rs` spells the member.
const TAGS_POINTER: &str = "/tags";

/// A tag map of `count` distinct keys.
fn tag_map(count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("key{index:03}"), json!("v")))
        .collect();
    Value::Object(map)
}

/// `CreatePolicyStore` with the given create-time `tags` (absent when `None`)
/// and an optional `clientToken`.
async fn create(
    router: &MultiServiceRouter,
    tags: Option<&Value>,
    client_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({ "validationSettings": { "mode": "OFF" } });
    if let Some(tags) = tags {
        request["tags"] = tags.clone();
    }
    if let Some(token) = client_token {
        request["clientToken"] = json!(token);
    }
    call(router, "CreatePolicyStore", &request).await
}

/// One row: how many tags the create request carries, and whether a resource may
/// hold that many.
struct Case {
    case: &'static str,
    tags: usize,
    /// `false` when the request carries no `tags` member at all, which is a
    /// different request from one carrying an empty map.
    present: bool,
}

impl Case {
    /// The rule: a map larger than the per-resource quota cannot be admitted.
    fn admitted(&self) -> bool {
        self.tags <= MAX_TAGS_PER_RESOURCE
    }
}

/// Rows differing along the size of the create-time tag map, including both
/// sides of the quota and the modelled per-request maximum.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no tags member at all",
            tags: 0,
            present: false,
        },
        Case {
            case: "an empty tag map",
            tags: 0,
            present: true,
        },
        Case {
            case: "one tag",
            tags: 1,
            present: true,
        },
        Case {
            case: "one below the quota",
            tags: MAX_TAGS_PER_RESOURCE - 1,
            present: true,
        },
        Case {
            case: "exactly the quota",
            tags: MAX_TAGS_PER_RESOURCE,
            present: true,
        },
        Case {
            case: "one past the quota",
            tags: MAX_TAGS_PER_RESOURCE + 1,
            present: true,
        },
        Case {
            case: "well past the quota",
            tags: 60,
            present: true,
        },
        Case {
            case: "the modelled per-request maximum",
            tags: MAX_TAGS_PER_REQUEST,
            present: true,
        },
    ]
}

/// RULE: a create request carrying more tags than one resource may hold is
/// refused 400 `ValidationException` naming `/tags`, and one carrying no more
/// than that is accepted with exactly those tags persisted — readable back
/// through `ListTagsForResource` and `GetPolicyStore`.
#[tokio::test]
async fn create_time_tags_are_bounded_by_the_per_resource_quota() {
    let router = router();

    for case in cases() {
        let tags = tag_map(case.tags);
        let (status, body) = create(&router, case.present.then_some(&tags), None).await;

        if !case.admitted() {
            let case = case.case;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from CreatePolicyStore, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: unexpected error code, got {body}"
            );
            assert_eq!(
                fault_message(&body),
                INVALID_INPUT_MESSAGE,
                "case {case:?}: expected the fixed summary message, got {body}"
            );
            let fields = field_list(&body);
            assert_eq!(
                fields.len(),
                1,
                "case {case:?}: expected one fieldList entry for the offending member, got {fields:?}"
            );
            assert_eq!(
                fields[0].0, TAGS_POINTER,
                "case {case:?}: expected the entry to name {TAGS_POINTER}, got {fields:?}"
            );
            assert!(
                fields[0].1.contains(&MAX_TAGS_PER_RESOURCE.to_string()),
                "case {case:?}: expected the reason to state the quota, got {fields:?}"
            );
            continue;
        }

        let case_name = case.case;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected HTTP 200 from CreatePolicyStore, got {status} {body}"
        );
        let arn = body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case_name:?}: arn is required, got {body}"))
            .to_string();
        let store_id = body["policyStoreId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case_name:?}: policyStoreId is required, got {body}"))
            .to_string();

        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            as_map(&tag_map(case.tags)),
            "case {case_name:?}: expected the {} create-time tags persisted, got {listed:?}",
            case.tags
        );
        let fetched = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": store_id, "tags": true }),
        )
        .await;
        if case.tags > 0 {
            assert_eq!(
                as_map(&fetched["tags"]),
                listed,
                "case {case_name:?}: GetPolicyStore reported a different tag set, got {fetched}"
            );
        }
    }
}

/// RULE (the other half): every store a create DOES produce holds a tag set the
/// tagging path can still work with — a key it already carries can always be
/// written, and a NEW key is accepted exactly while the store is under the
/// quota, so no created store is permanently untaggable.
#[tokio::test]
async fn a_created_store_is_never_left_beyond_what_tagging_allows() {
    let router = router();

    for case in cases().into_iter().filter(Case::admitted) {
        let case_name = case.case;
        let tags = tag_map(case.tags);
        let (status, body) = create(&router, case.present.then_some(&tags), None).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected HTTP 200 from CreatePolicyStore, got {status} {body}"
        );
        let arn = body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case_name:?}: arn is required, got {body}"))
            .to_string();

        // Replacing a key the store already carries never grows the set, so it
        // is accepted whatever the store holds.
        if case.tags > 0 {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": { "key000": "replaced" } }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: expected a replacement of a create-time key to be accepted, \
                 got {status} {body}"
            );
            assert_eq!(
                listed_tags(&router, &arn).await.get("key000").cloned(),
                Some("replaced".to_string()),
                "case {case_name:?}: expected the replaced value persisted, got {:?}",
                listed_tags(&router, &arn).await
            );
        }

        // One NEW key: accepted while the store is under the quota, refused with
        // `TooManyTagsException` exactly at it — the same decision the create
        // path made about the set it admitted.
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": { "added": "v" } }),
        )
        .await;
        let room_for_one_more = case.tags < MAX_TAGS_PER_RESOURCE;
        if room_for_one_more {
            assert_eq!(
                status, 200,
                "case {case_name:?}: a store holding {} tags has room for one more, got {status} {body}",
                case.tags
            );
            assert_eq!(
                listed_tags(&router, &arn).await.len(),
                case.tags + 1,
                "case {case_name:?}: expected {} tags after adding one, got {:?}",
                case.tags + 1,
                listed_tags(&router, &arn).await
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case_name:?}: a store at the quota refuses a new key, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "TooManyTagsException",
                "case {case_name:?}: unexpected error code, got {body}"
            );
            // And the documented way back: free a slot, then the new key fits.
            ok(
                &router,
                "UntagResource",
                &json!({ "resourceArn": arn, "tagKeys": ["key000"] }),
            )
            .await;
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": { "added": "v" } }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: a freed slot must accept a new key, got {status} {body}"
            );
        }
    }
}

/// RULE (nothing is persisted by a refused create): a refused request mints no
/// store and records no idempotency token, so the SAME `clientToken` can be
/// replayed with a tag map the quota admits and succeeds instead of colliding.
#[tokio::test]
async fn a_refused_create_persists_nothing() {
    let router = router();
    let token = "a1b2c3d4-create-time-tags";

    let over = tag_map(MAX_TAGS_PER_RESOURCE + 1);
    let (status, body) = create(&router, Some(&over), Some(token)).await;
    assert_eq!(
        status, 400,
        "case \"over the quota with a client token\": expected HTTP 400, got {status} {body}"
    );

    let within = tag_map(1);
    let (status, body) = create(&router, Some(&within), Some(token)).await;
    assert_eq!(
        status, 200,
        "case \"the same token with an admissible map\": expected HTTP 200, got {status} {body}"
    );
    let arn = body["arn"]
        .as_str()
        .expect("arn is required on the CreatePolicyStore response")
        .to_string();
    let listed = listed_tags(&router, &arn).await;
    assert_eq!(
        listed,
        as_map(&within),
        "case \"the same token with an admissible map\": expected only the admitted tag, got {listed:?}"
    );
}
