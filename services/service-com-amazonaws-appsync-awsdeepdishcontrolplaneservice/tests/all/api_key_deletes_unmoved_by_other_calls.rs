// SPDX-License-Identifier: CC0-1.0

//! The deletion time `ListApiKeys` projects for a key is a STORED property of that key,
//! end to end over HTTP: it is derived once, by the write that sets the key's
//! expiration, and no other call takes part in it.
//!
//! The rule the arithmetic itself obeys — a `da2` key's `deletes` is its `expires` plus
//! the service's post-expiry retention, floored to the hour — and the fact that the
//! create, the update and the listing all report that one value are asserted by
//! [`crate::api_key_deletion_window`], whose constants, helpers and rule assertion this
//! suite reuses rather than restating. What this suite adds is the dimension the listing
//! is read along in practice: the ROUTE the inventory takes between the write that set a
//! key's expiration and the listing that reads it back. A listing recomputing the value
//! off anything other than the key's own stored expiration — the reading clock, the
//! API's configuration, a neighbour key's expiration, the page the key lands on —
//! would answer a different deletion time after one of these calls than the create
//! handed the client.
//!
//! The rows differ along that one thing: the KIND of call that happens in between. A
//! neighbour key minted, moved to a new expiration and removed; the parent API renamed;
//! its tags set and cleared; its environment variables written; a key minted under
//! another API; and the listing repeated with nothing in between at all. Every row is
//! asserted over the same several subject keys, which differ along the only input the
//! derivation reads — their expirations, on and off an hour boundary and at each
//! published validity bound.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, expected_deletes, floor_to_hour, unix_now, update_key,
    BOUND_SLACK, DAY, HOUR, MAX_VALIDITY, MIN_VALIDITY,
};
use crate::error_contract::{call, create_api, router};
use crate::resource_tags::encode_label;
use crate::tag_resource::TAG_WRITE_STATUS;

/// The tag one row sets on the parent API and the next row removes again.
const TAG_KEY: &str = "team";

/// One subject row: its label, and the expiration it submits, as a function of the
/// clock the row itself reads.
type SubjectRow = (&'static str, fn(i64) -> i64);

/// The subject keys every row is asserted over: a label, and the expiration the key is
/// created with, as a function of the clock the create itself reads. They differ along
/// the only input the derivation reads — the expiration — covering an instant already on
/// an hour boundary, one mid-hour, and each published validity bound.
const SUBJECTS: &[SubjectRow] = &[
    ("a key expiring on an hour boundary", |now| {
        floor_to_hour(now + 30 * DAY)
    }),
    ("a key expiring mid-hour", |now| now + 45 * DAY + 1_234),
    ("a key just inside the minimum validity", |now| {
        now + MIN_VALIDITY + BOUND_SLACK
    }),
    ("a key just inside the maximum validity", |now| {
        now + MAX_VALIDITY - BOUND_SLACK
    }),
];

/// What one row does to the store between the write that set a subject key's expiration
/// and the listing that reads it back.
#[derive(Debug, Clone, Copy)]
enum Intervening {
    /// `CreateApiKey` mints another key under the same API.
    MintNeighbour,
    /// `UpdateApiKey` moves that neighbour onto a new expiration, so a neighbour's
    /// derivation is freshly recomputed while the subjects are not written at all.
    MoveNeighbour,
    /// `DeleteApiKey` removes the neighbour, which re-cuts the pages the subjects sit on.
    RemoveNeighbour,
    /// `UpdateGraphqlApi` replaces the parent API's configuration.
    UpdateTheApi,
    /// `TagResource` sets a tag on the parent API.
    TagTheApi,
    /// `UntagResource` removes it again.
    UntagTheApi,
    /// `PutGraphqlApiEnvironmentVariables` writes state held under the parent API.
    WriteEnvironmentVariables,
    /// `CreateApiKey` mints a key under a DIFFERENT API, with its own expiration.
    MintUnderAnotherApi,
    /// Nothing at all: the listing itself is the only call, so reading is a read.
    NothingButAnotherListing,
}

/// The rows, in the order they are applied — the neighbour rows move a key the row
/// before them left in place.
const ROWS: &[(&str, Intervening)] = &[
    ("another key minted under the same API", Intervening::MintNeighbour),
    (
        "that neighbour key moved onto a new expiration",
        Intervening::MoveNeighbour,
    ),
    ("that neighbour key removed", Intervening::RemoveNeighbour),
    ("the parent API's configuration replaced", Intervening::UpdateTheApi),
    ("a tag set on the parent API", Intervening::TagTheApi),
    ("that tag removed again", Intervening::UntagTheApi),
    (
        "the parent API's environment variables written",
        Intervening::WriteEnvironmentVariables,
    ),
    (
        "a key minted under another API",
        Intervening::MintUnderAnotherApi,
    ),
    (
        "nothing but another listing",
        Intervening::NothingButAnotherListing,
    ),
];

/// One subject key: the case that minted it, its id, and the projection its create
/// reported — the value every row below must still find in the listing.
struct Subject {
    case: &'static str,
    key_id: String,
    created: Value,
}

/// The `ApiKey` objects `ListApiKeys` reports for `api_id`, in the order it reports them.
async fn list_keys(router: &MultiServiceRouter, case: &str, api_id: &str) -> Vec<Value> {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListApiKeys 200, got {answer:?}"
    );
    answer.body["apiKeys"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARN `GetGraphqlApi` reports for `api_id` — what the tagging operations bind as
/// their `resourceArn` label.
async fn api_arn(router: &MultiServiceRouter, api_id: &str) -> String {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case read the API's ARN: expected GetGraphqlApi 200, got {answer:?}"
    );
    answer.body["graphqlApi"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A listed key's deletion time is the one its own expiration derives, and the one its
/// create reported, whatever call happened in between.
#[tokio::test]
async fn a_listed_keys_deletion_time_is_unmoved_by_every_other_call() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletes_unmoved").await;
    let other_api_id = create_api(&router, "Api_key_deletes_unmoved_other").await;
    let arn = api_arn(&router, &api_id).await;

    let mut subjects: Vec<Subject> = Vec::new();
    for (case, instant) in SUBJECTS {
        let submitted = instant(unix_now());
        let answer = create_key(
            &router,
            &api_id,
            json!({ "description": *case, "expires": submitted }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateApiKey 200, got {answer:?}"
        );
        let created = answer.body["apiKey"].clone();
        assert_retention_span(case, "CreateApiKey", &created);
        assert_eq!(
            created["deletes"].as_i64(),
            Some(expected_deletes(floor_to_hour(submitted))),
            "case {case:?}: expected deletes {} (the submitted expiration floored to the \
             hour, plus the retention), got {created:?}",
            expected_deletes(floor_to_hour(submitted))
        );
        subjects.push(Subject {
            case,
            key_id: created["id"].as_str().unwrap_or_default().to_string(),
            created,
        });
    }

    // The neighbour key the first three rows mint, move and remove.
    let mut neighbour: Option<String> = None;

    for (row, step) in ROWS {
        let case = format!("after {row}");
        match step {
            Intervening::MintNeighbour => {
                let answer = create_key(
                    &router,
                    &api_id,
                    json!({ "description": "neighbour", "expires": unix_now() + 7 * DAY }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected CreateApiKey 200, got {answer:?}"
                );
                let minted = answer.body["apiKey"].clone();
                assert_retention_span(&case, "CreateApiKey", &minted);
                neighbour = Some(minted["id"].as_str().unwrap_or_default().to_string());
            }
            Intervening::MoveNeighbour => {
                let key_id = neighbour
                    .clone()
                    .expect("case after that neighbour key moved: an earlier row minted it");
                let submitted = unix_now() + 200 * DAY + 11 * HOUR + 7;
                let answer =
                    update_key(&router, &api_id, &key_id, json!({ "expires": submitted })).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected UpdateApiKey 200, got {answer:?}"
                );
                let updated = answer.body["apiKey"].clone();
                assert_retention_span(&case, "UpdateApiKey", &updated);
                // The write moves the key it names, onto the value ITS expiration
                // derives — which is what the subjects below must not have taken on.
                assert_eq!(
                    updated["deletes"].as_i64(),
                    Some(expected_deletes(floor_to_hour(submitted))),
                    "case {case:?}: expected the neighbour's own derivation {}, got {updated:?}",
                    expected_deletes(floor_to_hour(submitted))
                );
            }
            Intervening::RemoveNeighbour => {
                let key_id = neighbour
                    .take()
                    .expect("case after that neighbour key removed: an earlier row minted it");
                let answer = call(
                    &router,
                    "DELETE",
                    &format!("/v1/apis/{api_id}/apikeys/{key_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected DeleteApiKey 200, got {answer:?}"
                );
            }
            Intervening::UpdateTheApi => {
                let answer = call(
                    &router,
                    "POST",
                    &format!("/v1/apis/{api_id}"),
                    Some(json!({
                        "name": "Api_key_deletes_unmoved_renamed",
                        "authenticationType": "AWS_IAM",
                        "queryDepthLimit": 5,
                        "resolverCountLimit": 50,
                        "xrayEnabled": true,
                    })),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected UpdateGraphqlApi 200, got {answer:?}"
                );
            }
            Intervening::TagTheApi => {
                let answer = call(
                    &router,
                    "POST",
                    &format!("/v1/tags/{}", encode_label(&arn)),
                    Some(json!({ "tags": { TAG_KEY: "orders" } })),
                )
                .await;
                assert_eq!(
                    answer.status, TAG_WRITE_STATUS,
                    "case {case:?}: expected TagResource {TAG_WRITE_STATUS}, got {answer:?}"
                );
            }
            Intervening::UntagTheApi => {
                let answer = call(
                    &router,
                    "DELETE",
                    &format!(
                        "/v1/tags/{}?tagKeys={}",
                        encode_label(&arn),
                        encode_label(TAG_KEY)
                    ),
                    None,
                )
                .await;
                assert_eq!(
                    answer.status, TAG_WRITE_STATUS,
                    "case {case:?}: expected UntagResource {TAG_WRITE_STATUS}, got {answer:?}"
                );
            }
            Intervening::WriteEnvironmentVariables => {
                let answer = call(
                    &router,
                    "PUT",
                    &format!("/v1/apis/{api_id}/environmentVariables"),
                    Some(json!({ "environmentVariables": { "STAGE": "beta" } })),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected PutGraphqlApiEnvironmentVariables 200, \
                     got {answer:?}"
                );
            }
            Intervening::MintUnderAnotherApi => {
                let submitted = unix_now() + 120 * DAY + 59;
                let answer = create_key(
                    &router,
                    &other_api_id,
                    json!({ "description": "another API's key", "expires": submitted }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected CreateApiKey 200, got {answer:?}"
                );
                let minted = answer.body["apiKey"].clone();
                assert_retention_span(&case, "CreateApiKey", &minted);
                assert_eq!(
                    minted["deletes"].as_i64(),
                    Some(expected_deletes(floor_to_hour(submitted))),
                    "case {case:?}: expected the other API's key to carry its own \
                     derivation {}, got {minted:?}",
                    expected_deletes(floor_to_hour(submitted))
                );
            }
            Intervening::NothingButAnotherListing => {
                let first = list_keys(&router, &case, &api_id).await;
                let second = list_keys(&router, &case, &api_id).await;
                assert_eq!(
                    first, second,
                    "case {case:?}: expected two listings with nothing in between to \
                     report the same keys, got {first:?} then {second:?}"
                );
            }
        }

        // Whatever the row did, every subject key is still listed with the deletion time
        // its OWN expiration derives, exactly as its create reported it.
        let listed = list_keys(&router, &case, &api_id).await;
        for subject in &subjects {
            let case = format!("{} {case}", subject.case);
            let found = listed
                .iter()
                .find(|candidate| candidate["id"].as_str() == Some(subject.key_id.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the listing to hold {}, got {listed:?}",
                        subject.key_id
                    )
                });
            assert_retention_span(&case, "ListApiKeys", found);
            assert_eq!(
                found, &subject.created,
                "case {case:?}: expected the listed key to carry the projection its \
                 create reported ({:?}), got {found:?}",
                subject.created
            );
        }
    }

    // The keys of the two APIs never pooled: the other API lists the one key minted
    // under it, with the derivation that key's own expiration owes.
    let other = list_keys(&router, "the other API's listing", &other_api_id).await;
    assert_eq!(
        other.len(),
        1,
        "case the other API's listing: expected the single key minted under it, got {other:?}"
    );
    assert_retention_span(
        "the other API's listing",
        "ListApiKeys",
        &other[0],
    );
}
