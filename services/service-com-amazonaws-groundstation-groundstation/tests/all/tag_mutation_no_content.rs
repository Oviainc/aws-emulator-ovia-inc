// SPDX-License-Identifier: CC0-1.0

//! The SUCCESS STATUS and the framing of this service's TAG MUTATIONS — `TagResource` and
//! `UntagResource` — end to end over HTTP through the same front door as the rest of this
//! target's suites.
//!
//! The RULE under test: an operation of this service whose modelled output carries NO
//! member leaves the caller nothing to parse, so its success is answered 204 No Content
//! with an empty body rather than with the 200 the protocol leaves defaulted. The two
//! mutations of the tagging plane are this service's whole class of such operations:
//! `TagResourceResponse` and `UntagResourceResponse` declare no member at all
//! (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse",\
//! model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
//! com-amazonaws-groundstation-groundstation.json")`) and neither operation states an
//! `@http` code, so the status is this service's behaviour rather than the model's. Every
//! other answer keeps its own status: an operation whose output carries members answers
//! 200, and a refused mutation answers its modelled refusal.
//!
//! The suites assert that over SETS of inputs rather than one recorded call, the rows
//! differing along what the rule generalises over:
//!
//! * every legal tag map of [`crate::tag_rules::legal_tag_maps`] — the empty map, keys and
//!   values at each published bound, multi-byte spellings, the documented character set and
//!   the per-resource tag ceiling — written and then removed again, each call made twice so
//!   the status cannot depend on whether it changed anything;
//! * every taggable resource this service hands out (a config of each capability type, a
//!   mission profile, a dataflow endpoint group, a contact, an ephemeris and an agent), so
//!   the status belongs to the mutation and not to the kind of resource it addressed;
//! * removals naming a stored key, a key the resource never carried, and the same key twice.
//!
//! What the two operations DO to the tag map is [`crate::resource_tagging`]'s and
//! [`crate::tag_removal`]'s subject; this suite owns the status and the framing. Each
//! mutation is still read back through `ListTagsForResource`, so the status asserted here is
//! the status of a write that landed rather than of a refusal dressed as a success.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tagging::{
    fresh_config, label, list_tags, reported_tags, taggable_resources, tags_of, Taggable,
};
use crate::scheduling_loop::{
    harness, harness_serving, send, send_http, ABSENT_CONFIG_ARN, STATION,
};
use crate::tag_removal::encode_query_value;
use crate::tag_rules::legal_tag_maps;

/// The status a SUCCESSFUL tag mutation answers: 204 No Content, because both
/// `TagResourceResponse` and `UntagResourceResponse` carry no members.
///
/// `pub(crate)` and stated once here: [`crate::resource_tagging`] and
/// [`crate::tag_removal`] drive the same mutations while asserting what they do to the tag
/// map, so they name this rule rather than each spelling a status of their own.
pub(crate) const TAG_MUTATION_SUCCESS_STATUS: u16 = 204;

/// The status an operation whose output DOES carry members answers, for the contrast the
/// last suite draws.
const BODY_FOLLOWS_STATUS: u16 = 200;

/// A key no row of this suite ever writes, for the removals that name a key the resource
/// does not carry.
const NEVER_STORED_KEY: &str = "never-stored";

/// `POST /tags/{resourceArn}` — `TagResource` — as the WHOLE response, so the status, the
/// body bytes and the headers can all be asserted.
async fn tag_http(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &Value,
) -> http::Response<bytes::Bytes> {
    send_http(
        router,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({"tags": tags})),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource` — as the whole response.
/// `tagKeys` is bound to `@httpQuery("tagKeys")`, so each key travels as its own pair.
async fn untag_http(
    router: &MultiServiceRouter,
    arn: &str,
    keys: &[String],
) -> http::Response<bytes::Bytes> {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_query_value(key)))
        .collect();
    let path = if query.is_empty() {
        format!("/tags/{}", label(arn))
    } else {
        format!("/tags/{}?{}", label(arn), query.join("&"))
    };
    send_http(router, "DELETE", &path, None).await
}

/// Assert one mutation answered the no-content success: the status, and zero body bytes —
/// a 204 that carried a body would be the protocol inconsistency the status rules out.
fn assert_no_content(case: &str, operation: &str, response: &http::Response<bytes::Bytes>) {
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, TAG_MUTATION_SUCCESS_STATUS,
        "case {case:?}/{operation}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {status} {body:?}"
    );
    assert!(
        response.body().is_empty(),
        "case {case:?}/{operation}: a no-content answer carries no body bytes, got {body:?}"
    );
}

/// The keys of a tag map, as a removal names them.
fn keys_of(tags: &Value) -> Vec<String> {
    tags.as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn every_successful_tag_mutation_answers_no_content() {
    let router = harness();
    let rows = legal_tag_maps();
    assert!(
        rows.len() >= 5,
        "the tag-map table must span the class, got {} rows",
        rows.len()
    );

    for (index, (case, tags)) in rows.into_iter().enumerate() {
        // `tags` is `@required` on `TagResourceRequest` (`smithy_query(query="operation",
        // operation="TagResource")`), so the row that carries no map at all is refused by
        // the model's own walk and has no success status to assert; the refusal it does get
        // is pinned by `crate::resource_tagging`.
        let Some(tags) = tags else {
            continue;
        };
        // A resource of its own per row, carrying no tag, so what a read reports afterwards
        // is exactly what these two calls wrote and removed.
        let arn = fresh_config(&router, &format!("no-content row {index}")).await;

        // Twice: the status says "nothing to parse", which does not depend on whether the
        // call changed anything, so the second, no-op assignment answers the same way.
        for attempt in 1..=2 {
            let response = tag_http(&router, &arn, &tags).await;
            assert_no_content(&case, &format!("TagResource attempt {attempt}"), &response);
        }

        // The write LANDED: the status above is the status of a real mutation.
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.field("tags"),
            &tags,
            "case {case:?}: the assignment must persist before its status means anything, \
             got {}",
            listed.body
        );

        // The removal names every key the row wrote PLUS one the resource never carried, so
        // the status is asserted for a removal that drops something and for one that drops
        // nothing.
        let mut removed = keys_of(&tags);
        removed.push(NEVER_STORED_KEY.to_string());
        for attempt in 1..=2 {
            let response = untag_http(&router, &arn, &removed).await;
            assert_no_content(&case, &format!("UntagResource attempt {attempt}"), &response);
        }

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            reported_tags(&case, &listed).len(),
            0,
            "case {case:?}: the removal must persist too, got {}",
            listed.body
        );

        // A removal naming the same key TWICE is one removal, and still a no-content
        // success: the operation is `@idempotent` (`smithy_query(query="operation",
        // operation="UntagResource")`).
        let doubled: Vec<String> = removed.iter().chain(removed.iter()).cloned().collect();
        let response = untag_http(&router, &arn, &doubled).await;
        assert_no_content(&case, "UntagResource naming each key twice", &response);
    }
}

#[tokio::test]
async fn the_no_content_status_holds_for_every_taggable_resource() {
    // The taggable resources include a reserved contact, so the account is served by the
    // station that reservation is held at.
    let router = harness_serving(STATION);

    for resource in taggable_resources(&router).await {
        let Taggable {
            label: what,
            arn,
            created_with,
            ..
        } = resource;
        let written = tags_of(&[("framing", "no-content")]);
        let response = tag_http(&router, &arn, &json!(written)).await;
        assert_no_content(&what, "TagResource", &response);

        let listed = list_tags(&router, &arn).await;
        let mut expected = created_with.clone();
        expected.extend(written);
        assert_eq!(
            reported_tags(&what, &listed),
            expected,
            "case {what:?}: the assignment must persist before its status means anything, \
             got {}",
            listed.body
        );

        let response = untag_http(&router, &arn, &["framing".to_string()]).await;
        assert_no_content(&what, "UntagResource", &response);

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            reported_tags(&what, &listed),
            created_with,
            "case {what:?}: the removal must leave the tags the create stored, got {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn every_other_answer_of_the_tagging_plane_keeps_its_own_status() {
    let router = harness();
    let arn = fresh_config(&router, "the contrast").await;
    assert_no_content(
        "the mutation the contrast is drawn against",
        "TagResource",
        &tag_http(&router, &arn, &json!({"framing": "no-content"})).await,
    );

    // The read of the same plane carries members, so it keeps the status that says a body
    // follows — and carries one.
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.status, BODY_FOLLOWS_STATUS,
        "case \"ListTagsForResource\": an answer carrying members keeps its {BODY_FOLLOWS_STATUS}, \
         got {} {}",
        listed.status, listed.body
    );
    assert!(
        listed.body.is_object(),
        "case \"ListTagsForResource\": an answer carrying members must carry a body, got {}",
        listed.body
    );

    // So does a listing of this service, and the resource's own read.
    for (case, route) in [
        ("ListConfigs", "/config".to_string()),
        ("ListTagsForResource", format!("/tags/{}", label(&arn))),
    ] {
        let reply = send(&router, "GET", &route, None).await;
        assert_eq!(
            reply.status, BODY_FOLLOWS_STATUS,
            "case {case:?}: an answer carrying members keeps its {BODY_FOLLOWS_STATUS}, got {} {}",
            reply.status, reply.body
        );
    }

    // A REFUSED mutation keeps the status and the body of its refusal: the no-content
    // answer is the success alone. An ARN resolving to nothing is the tagging plane's
    // not-found refusal (`crate::tagging_not_found_refusal` owns its wording), and a map
    // outside the platform tagging rules is the modelled invalid-parameter refusal
    // (`crate::tag_rules` owns those rows).
    for (case, target, tags, expected) in [
        (
            "an ARN resolving to nothing",
            ABSENT_CONFIG_ARN.to_string(),
            json!({"framing": "no-content"}),
            434_u16,
        ),
        (
            "a key in the reserved namespace",
            arn.clone(),
            json!({"aws:createdBy": "cloudformation"}),
            431_u16,
        ),
    ] {
        let response = tag_http(&router, &target, &tags).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, expected,
            "case {case:?}: a refusal keeps its modelled status, got {status} {body:?}"
        );
        assert!(
            !response.body().is_empty(),
            "case {case:?}: a refusal carries its error shape, got {body:?}"
        );
    }
}
