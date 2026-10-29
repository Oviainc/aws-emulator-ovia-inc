// SPDX-License-Identifier: CC0-1.0

//! The tag-capacity rule, driven through BOTH operations that apply it —
//! `CreateWorkspaceInstance` and `TagResource` — over the wire this service really
//! speaks (awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule, stated once and asserted over a SET of inputs:
//!
//! > whether a request may stand is decided by the tag set it would LEAVE the
//! > WorkSpace Instance holding, against the one bound the modelled `TagList` can
//! > report (`wsinst::state::MAX_TAGS_PER_INSTANCE`): a set at or below it is stored
//! > whole, a set above it is refused with the same status, the same modelled code,
//! > the same reason and the same wording whichever operation would have grown it —
//! > only the member named differs, because only the member that carried the tags
//! > differs — and a refused request stores nothing.
//!
//! The rows differ along what that rule generalises over: how many keys the resulting
//! set holds (each side of the bound, the bound itself, and far past it), how those
//! keys are split between the set already present and the set the request carries,
//! and how many of the request's keys are overwrites that do not grow the set at all.
//!
//! Nothing here can pass against a canned answer: each accepted set is read back
//! through `ListTagsForResource`, each pair of refusals is compared field by field
//! rather than against text written here, and each refusal is followed by a read that
//! shows the store untouched.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::workspace_instance_lifecycle::{
    call, create, error_code, launchable_managed_instance, list_all_ids, ok, router, stored_tags,
};

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// What a refusal of the tag-capacity rule says, as a client reads it, EXCEPT the
/// member it names: the status, the modelled error code, the reason and the wording.
/// Compared whole, so the two operations cannot answer the same over-large set
/// differently in any of them.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    code: String,
    reason: String,
    message: String,
}

/// Read a [`Refusal`] out of one answer, and the members its `FieldList` names,
/// failing the case when the answer is not a refusal of the shape this service's
/// validation refusals have.
fn refusal(case: &str, operation: &str, status: u16, payload: &Value) -> (Refusal, Vec<String>) {
    let members: Vec<String> = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no FieldList in {payload}"))
        .iter()
        .map(|field| {
            field["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Name in {payload}"))
                .to_string()
        })
        .collect();
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Message in {payload}"))
        .to_string();
    let refused = Refusal {
        status,
        code: error_code(payload),
        reason: payload["Reason"].as_str().unwrap_or_default().to_string(),
        message,
    };
    (refused, members)
}

/// A `TagList` body from pairs.
fn tag_list(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// One shape of request, described by the set it would leave the resource holding.
///
/// `base` keys are the ones already present before the request — carried by a
/// `TagSpecification` on the create path, and already stored on the tagging path.
/// The request itself re-sends `overwrites` of them (which cannot grow the set) and
/// adds `fresh` keys it has never seen (which do).
struct Row {
    case: String,
    base: usize,
    overwrites: usize,
    fresh: usize,
}

impl Row {
    /// The keys already present, with the values they were first stored under.
    fn base_pairs(&self) -> Vec<(String, String)> {
        (0..self.base)
            .map(|i| (format!("base-{i:02}"), format!("first-{i:02}")))
            .collect()
    }

    /// The `TagList` the request carries: the overwrites first, then the fresh keys.
    fn request_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = (0..self.overwrites)
            .map(|i| (format!("base-{i:02}"), format!("again-{i:02}")))
            .collect();
        pairs.extend(
            (0..self.fresh).map(|i| (format!("fresh-{i:02}"), format!("value-{i:02}"))),
        );
        pairs
    }

    /// The set the request would leave the resource holding: an overwritten key keeps
    /// its key and takes the new value, a fresh key is added.
    fn resulting_tags(&self) -> BTreeMap<String, String> {
        let mut tags: BTreeMap<String, String> = self.base_pairs().into_iter().collect();
        for (key, value) in self.request_pairs() {
            tags.insert(key, value);
        }
        tags
    }

    /// Whether the resulting set is one the modelled `TagList` could report.
    fn within_bound(&self) -> bool {
        self.resulting_tags().len() <= wsinst::state::MAX_TAGS_PER_INSTANCE
    }
}

/// The rows the shared rule is asserted over, generated from the bound the crate reads
/// off the model (`wsinst::state::MAX_TAGS_PER_INSTANCE`) so a change to it changes the
/// rows with it.
///
/// One `TagList` carries at most that many entries at the boundary, so no row sends
/// more than the bound in a single member: a set past the bound is always reached by
/// a request that is itself well formed.
fn rows() -> Vec<Row> {
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;
    let half = limit / 2;
    vec![
        Row {
            case: "no tags at all".to_string(),
            base: 0,
            overwrites: 0,
            fresh: 0,
        },
        Row {
            case: format!("every one of the {limit} keys in the request alone, at the bound"),
            base: 0,
            overwrites: 0,
            fresh: limit,
        },
        Row {
            case: "split evenly between the set present and the request, at the bound".to_string(),
            base: half,
            overwrites: 0,
            fresh: limit - half,
        },
        Row {
            case: "one fresh key reaching the bound exactly".to_string(),
            base: limit - 1,
            overwrites: 0,
            fresh: 1,
        },
        Row {
            case: "a full set and a request that only overwrites it".to_string(),
            base: limit,
            overwrites: limit,
            fresh: 0,
        },
        Row {
            case: "a full set and an empty request".to_string(),
            base: limit,
            overwrites: 0,
            fresh: 0,
        },
        Row {
            case: "one fresh key past the bound".to_string(),
            base: limit,
            overwrites: 0,
            fresh: 1,
        },
        Row {
            case: "overwrites plus one fresh key, past the bound".to_string(),
            base: limit,
            overwrites: half,
            fresh: 1,
        },
        Row {
            case: "split between the set present and the request, one past the bound".to_string(),
            base: half,
            overwrites: 0,
            fresh: limit - half + 1,
        },
        Row {
            case: format!("{limit} keys past the bound"),
            base: limit,
            overwrites: 0,
            fresh: limit,
        },
    ]
}

/// The create body for `row`: the keys already present arrive in a `TagSpecification`
/// for the instance, the request's own keys in the top-level `Tags` member.
fn create_body(row: &Row) -> Value {
    let mut managed_instance = launchable_managed_instance();
    managed_instance["TagSpecifications"] = json!([
        { "ResourceType": "instance", "Tags": tag_list(&row.base_pairs()) }
    ]);
    json!({
        "ManagedInstance": managed_instance,
        "Tags": tag_list(&row.request_pairs()),
    })
}

#[tokio::test]
async fn one_rule_decides_the_same_tag_set_on_create_and_on_tag_resource() {
    let router = router();
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;
    let rows = rows();

    // Both sides of the bound take part, so the row set cannot pass while one of them
    // is never exercised.
    assert!(
        rows.iter().any(Row::within_bound) && rows.iter().any(|row| !row.within_bound()),
        "case catalog: expected rows on both sides of the bound, got {} rows",
        rows.len()
    );

    for row in &rows {
        let case = row.case.as_str();
        let expected = row.resulting_tags();

        // The create path: the whole set is built from the two members of one request.
        let (create_status, create_payload) = {
            let before = list_all_ids(&router, json!({})).await.len();
            let (status, payload) = call(&router, "CreateWorkspaceInstance", create_body(row)).await;
            let after = list_all_ids(&router, json!({})).await.len();
            let grew = if row.within_bound() { 1 } else { 0 };
            assert_eq!(
                after - before,
                grew,
                "case {case:?}: expected the instance count to grow by {grew}, got {before} then \
                 {after} with {payload}"
            );
            (status, payload)
        };

        // The tagging path: the same set, reached by growing one already stored.
        let instance = create(
            &router,
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": tag_list(&row.base_pairs()),
            }),
        )
        .await;
        let stored_before = stored_tags(&router, &instance).await;
        let (tag_status, tag_payload) = call(
            &router,
            "TagResource",
            json!({
                "WorkspaceInstanceId": &instance,
                "Tags": tag_list(&row.request_pairs()),
            }),
        )
        .await;

        if row.within_bound() {
            assert_eq!(
                (create_status, tag_status),
                (200, 200),
                "case {case:?}: a set of {} tags is one the model can report, so both operations \
                 must accept it, got {create_payload} and {tag_payload}",
                expected.len()
            );
            // Read back through the operation that reports a tag set: both paths leave
            // the resource holding exactly the set the rule computed.
            let created_id = create_payload["WorkspaceInstanceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create returned no id: {create_payload}"));
            let created_tags = stored_tags(&router, created_id).await;
            let tagged_tags = stored_tags(&router, &instance).await;
            assert_eq!(
                created_tags, expected,
                "case {case:?}: expected the create to store {expected:?}, got {created_tags:?}"
            );
            assert_eq!(
                tagged_tags, expected,
                "case {case:?}: expected the tagging call to store {expected:?}, got {tagged_tags:?}"
            );
            continue;
        }

        // Past the bound: one refusal, answered the same way by both operations.
        let (created, create_members) =
            refusal(case, "CreateWorkspaceInstance", create_status, &create_payload);
        let (tagged, tag_members) = refusal(case, "TagResource", tag_status, &tag_payload);
        assert_eq!(
            created,
            Refusal {
                status: 400,
                code: "ValidationException".to_string(),
                reason: FIELD_VALIDATION_FAILED.to_string(),
                message: created.message.clone(),
            },
            "case {case:?}: expected the modelled validation refusal, got {create_payload}"
        );
        assert_eq!(
            tagged, created,
            "case {case:?}: TagResource must refuse the set exactly as CreateWorkspaceInstance \
             does, got {tag_payload} against {create_payload}"
        );
        assert!(
            created.message.contains(&limit.to_string()),
            "case {case:?}: expected the wording to name the bound {limit}, got {:?}",
            created.message
        );

        // Only the member named differs: each refusal names the member that carried
        // the tags of the request it refused.
        assert_eq!(
            create_members,
            vec!["TagSpecifications".to_string()],
            "case {case:?}: expected the create's FieldList to name the member its tags came in, \
             got {create_members:?}"
        );
        assert_eq!(
            tag_members,
            vec!["Tags".to_string()],
            "case {case:?}: expected the tagging call's FieldList to name the member its tags \
             came in, got {tag_members:?}"
        );

        // A refused request stores nothing: the stored set is the one from before it.
        let stored_after = stored_tags(&router, &instance).await;
        assert_eq!(
            stored_after, stored_before,
            "case {case:?}: a refused tagging call changed the stored set, got {stored_after:?}"
        );
    }
}

#[tokio::test]
async fn the_bound_the_rule_compares_against_is_the_one_the_model_can_report() {
    let router = router();
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;

    // A set at the bound is reported WHOLE, which is what makes the bound the right
    // one: the resource may hold exactly as many tags as `ListTagsForResource` can
    // carry, and no more.
    let full: Vec<(String, String)> = (0..limit)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    let instance = create(
        &router,
        json!({
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tag_list(&full),
        }),
    )
    .await;
    let reported = stored_tags(&router, &instance).await;
    assert_eq!(
        reported.len(),
        limit,
        "case a set at the bound: expected all {limit} tags to be reported, got {reported:?}"
    );

    // Removing one key makes room for exactly one more, through the operation that
    // grows a stored set — the bound is on the resulting set, not on the request.
    ok(
        &router,
        "UntagResource",
        json!({ "WorkspaceInstanceId": &instance, "TagKeys": ["key-00"] }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "TagResource",
        json!({
            "WorkspaceInstanceId": &instance,
            "Tags": tag_list(&[("room-for-one".to_string(), "yes".to_string())]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case one key removed then one added: expected HTTP 200, got {status} with {payload}"
    );
    let reported = stored_tags(&router, &instance).await;
    assert_eq!(
        reported.len(),
        limit,
        "case one key removed then one added: expected {limit} tags, got {reported:?}"
    );
    assert!(
        reported.contains_key("room-for-one") && !reported.contains_key("key-00"),
        "case one key removed then one added: expected the new key in place of the removed one, \
         got {reported:?}"
    );
}
