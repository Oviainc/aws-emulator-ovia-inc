// SPDX-License-Identifier: CC0-1.0

//! The tag-allowance refusal — `AddTagsToResource` past the tag limit — driven end to end
//! over HTTP, with `ListTagsForResource` as the reader that proves nothing was stored.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`).
//!
//! The rule under test, asserted over a TABLE rather than one recorded call: a tag write
//! that would leave a resource carrying more than the published allowance is refused with
//! `TagLimitExceededException` whose diagnostic is this service's own FIXED sentence —
//! `Tag limit exceeded.`, with the request-id suffix every refusal of this service carries
//! — and nothing of the request or of the stored tags is quoted into it, however many tags
//! the call carried, whichever of the two bounds it broke (the request's own list, or the
//! merge of it with the tags the resource already holds) and whichever resource it
//! addressed. A refused call stores nothing, and a write that lands exactly ON the
//! allowance is served.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, service, string, tag_map, PAST_ONE_SPAN,
    TAG_QUOTA,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The sentence Directory Service states a broken tag allowance with: FIXED text, naming
/// neither the count the call carried nor the bound it broke.
const TAG_LIMIT_EXCEEDED: &str = "Tag limit exceeded.";

/// `count` well-formed tags, keyed `k<index>` from `first`, so a row can ask for a list of
/// any size and two rows can ask for disjoint keys.
fn tags(first: usize, count: usize) -> Vec<Value> {
    (first..first + count)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
        .collect()
}

/// The tags `ListTagsForResource` reports for `resource_id`.
async fn stored_tags(
    router: &MultiServiceRouter,
    resource_id: &str,
    case: &str,
) -> std::collections::BTreeMap<String, String> {
    let answer = ok(
        router,
        "ListTagsForResource",
        json!({ "ResourceId": resource_id }),
        case,
    )
    .await;
    tag_map(&answer, case)
}

/// Assert that `body` is refused with the modelled `TagLimitExceededException` carrying
/// this service's fixed diagnostic and the call's own request id.
async fn refused_for_the_allowance(router: &MultiServiceRouter, body: Value, case: &str) {
    let (status, parsed) = call(router, "AddTagsToResource", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the tag write to answer 400, got {status} with {parsed}"
    );
    assert_eq!(
        parsed["__type"],
        json!("TagLimitExceededException"),
        "case {case:?}: expected the modelled tag-allowance refusal, got {parsed}"
    );
    let request_id = string(&parsed, "RequestId", case);
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"));
    assert_eq!(
        message,
        format!("{TAG_LIMIT_EXCEEDED} : RequestId: {request_id}"),
        "case {case:?}: expected this service's fixed diagnostic, got {parsed}"
    );
}

/// One row of the over-limit table: how many tags the resource already carries, the tag
/// list the request sends, and what makes the row different from the others.
struct Row {
    case: &'static str,
    /// Tags written before the refused call, which must survive it.
    stored: Vec<Value>,
    /// The tag list the refused call carries.
    requested: Vec<Value>,
}

/// The table: rows differing along WHICH bound the call breaks and by how much — the
/// request's own list alone, and the merge of a shorter list with the tags the resource
/// already holds, at the first count past the allowance and well past it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one tag past the allowance in one request",
            stored: Vec::new(),
            requested: tags(0, TAG_QUOTA + 1),
        },
        Row {
            case: "far past the allowance in one request",
            stored: Vec::new(),
            requested: tags(0, TAG_QUOTA * 3),
        },
        Row {
            case: "a single tag that would take a full resource past the allowance",
            stored: tags(0, TAG_QUOTA),
            requested: tags(TAG_QUOTA, 1),
        },
        Row {
            case: "a two-tag write onto a resource one short of the allowance",
            stored: tags(0, TAG_QUOTA - 1),
            requested: tags(TAG_QUOTA, 2),
        },
        Row {
            case: "a write whose own list is within the allowance but whose merge is not",
            stored: tags(0, TAG_QUOTA / 2 + 1),
            requested: tags(TAG_QUOTA, TAG_QUOTA / 2),
        },
    ]
}

#[tokio::test]
async fn an_over_limit_tag_write_states_this_services_fixed_diagnostic() {
    let router = service();
    let rows = rows();

    // One directory per row, and a Managed Microsoft AD row among them, so the diagnostic
    // is shown to be the service's rather than a sentence about one resource.
    let mut subjects: Vec<(&Row, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let id = if index % 2 == 0 {
            create_directory(
                &router,
                minimal_create(&format!("tagged{index}.example.com")),
                row.case,
            )
            .await
        } else {
            create_microsoft_ad(
                &router,
                minimal_microsoft_ad(&format!("tagged{index}.example.com")),
                row.case,
            )
            .await
        };
        subjects.push((row, id));
    }
    // A tag write is served by a directory that has finished being built, so the chain the
    // creates owe is waited out once.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, id) in &subjects {
        let case = row.case;
        if !row.stored.is_empty() {
            ok(
                &router,
                "AddTagsToResource",
                json!({ "ResourceId": id, "Tags": row.stored }),
                case,
            )
            .await;
        }
        let before = stored_tags(&router, id, case).await;

        refused_for_the_allowance(
            &router,
            json!({ "ResourceId": id, "Tags": row.requested }),
            case,
        )
        .await;

        // The refused call stored nothing: the resource carries exactly the tags it did.
        let after = stored_tags(&router, id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused write to leave the stored tags alone, got \
             {after:?}"
        );
    }
}

#[tokio::test]
async fn a_write_that_lands_on_the_allowance_is_served() {
    let router = service();
    let case = "on the allowance";
    let id = create_directory(&router, minimal_create("bound.example.com"), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Exactly the allowance in one request: the published bound itself is served.
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": tags(0, TAG_QUOTA) }),
        case,
    )
    .await;
    let full = stored_tags(&router, &id, case).await;
    assert_eq!(
        full.len(),
        TAG_QUOTA,
        "case {case:?}: expected the resource to carry the whole allowance, got {full:?}"
    );

    // A write that only OVERWRITES keys the resource already carries does not grow it, so
    // it is served even though the resource is full.
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": [{ "Key": "k0", "Value": "rewritten" }] }),
        case,
    )
    .await;
    let rewritten = stored_tags(&router, &id, case).await;
    assert_eq!(
        rewritten.len(),
        TAG_QUOTA,
        "case {case:?}: expected an overwrite to leave the count at the allowance, got \
         {rewritten:?}"
    );
    assert_eq!(
        rewritten.get("k0").map(String::as_str),
        Some("rewritten"),
        "case {case:?}: expected the overwritten value to be stored, got {rewritten:?}"
    );

    // One key past the allowance is refused, and removing a key makes room for it again —
    // so the refusal follows the stored count rather than the resource being blacklisted.
    refused_for_the_allowance(
        &router,
        json!({ "ResourceId": &id, "Tags": tags(TAG_QUOTA, 1) }),
        case,
    )
    .await;
    ok(
        &router,
        "RemoveTagsFromResource",
        json!({ "ResourceId": &id, "TagKeys": ["k1"] }),
        case,
    )
    .await;
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": tags(TAG_QUOTA, 1) }),
        case,
    )
    .await;
    let after = stored_tags(&router, &id, case).await;
    assert_eq!(
        after.len(),
        TAG_QUOTA,
        "case {case:?}: expected the freed room to be usable, got {after:?}"
    );
    assert!(
        after.contains_key(&format!("k{TAG_QUOTA}")) && !after.contains_key("k1"),
        "case {case:?}: expected the new key in place of the removed one, got {after:?}"
    );
}
