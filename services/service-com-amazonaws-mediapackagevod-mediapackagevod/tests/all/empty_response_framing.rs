// SPDX-License-Identifier: CC0-1.0

//! How this service frames an answer that carries NO content.
//!
//! RULE under test: a response this service answers `204 No Content` carries no body at
//! all and STATES the length it has — `content-length: 0` — rather than an empty JSON
//! object and no framing header; a response that carries content is framed from the body
//! it carries and never states a length that is not its own.
//!
//! The answers with no content are the ones whose output shape has no members: each of the
//! three deletes, which serve the recorded `204 No Content` (`DeleteAsset`,
//! `DeletePackagingConfiguration`, `DeletePackagingGroup`), and `UntagResource`, modelled
//! at HTTP 204 (`smithy_query(query="digest")`). The rule is asserted over all four rather
//! than over the one a recording happened to cover; `TagResource` is the fifth such
//! operation in the model and is still a stub, so it answers an envelope with a body and is
//! asserted below with the other bodied answers.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: which
//! operation answers, which resource type it addresses, the FORM of the resource id, how
//! many tags the resource carries, and whether the request changed anything at all (a key
//! the resource does not carry removes nothing) — the framing is a property of the
//! RESPONSE having no body, not of the work it did.
//!
//! Every case drives real HTTP through the router the production binary builds, which is
//! what the in-process front door (the inter-service callback path, the SDK connector)
//! hands its caller unchanged. An answer served over HTTP/1.1 reaches the client without
//! the header: the vendored transport's writer removes `content-length` from a `204`,
//! which is below anything this crate decides, so what is pinned here is the framing the
//! service itself states.
//!
//! The harness (`router`, `call`, `dispatch`, `KINDS`, `tags_route`, `tag_map`, the
//! creates) is the one `tests/all/resource_tag_listing.rs` owns for the tag slice.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tag_listing::{
    call, create_parent_group, create_with_tags, dispatch, expected_map, tag_map, tags_route, KINDS,
    PARENT_GROUP, REGION,
};
use crate::resource_tag_removal::untag_route;

/// The length an answer with no body states.
const EMPTY_BODY_LENGTH: &str = "0";

/// The status every bodyless answer of this service carries: the modelled 204 of
/// `UntagResource` and the recorded 204 of each delete.
const NO_CONTENT: u16 = 204;

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: u16 = 404;

/// Assert `response` is a bodyless answer with `expected_status`, framed with its length.
fn assert_empty_and_framed(
    case: &str,
    response: &http::Response<bytes::Bytes>,
    expected_status: u16,
) {
    let status = response.status().as_u16();
    let body = response.body();
    assert_eq!(
        status,
        expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} body {:?}",
        String::from_utf8_lossy(body)
    );
    assert!(
        body.is_empty(),
        "case {case:?}: this answer carries no body, got {:?}",
        String::from_utf8_lossy(body)
    );
    let stated = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok());
    assert_eq!(
        stated,
        Some(EMPTY_BODY_LENGTH),
        "case {case:?}: an answer with no body must state its length, got {stated:?}"
    );
}

/// The tag map `ListTagsForResource` reports for `arn`, asserting the read succeeded.
async fn listed_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let listed = call(router, REGION, "GET", &tags_route(arn), None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {} body {}",
        listed.status, listed.body
    );
    tag_map(&listed.body)
}

/// One row: the resources it creates, the tags they carry and the keys its untags remove.
struct Row {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The id stem of this row's resources — the rows differ in its FORM.
    id: &'static str,
    /// The tags each of this row's resources is created with (`None` omits the member).
    tags: Option<Value>,
    /// The keys each `UntagResource` of this row names.
    remove: &'static [&'static str],
}

/// The rows span the shape of the tag map (absent, one entry, several, non-ASCII), the
/// relation of the removal to it (the only key, some of several, a key that is not there,
/// a key named twice), and the form of the resource ids — because the rule is about the
/// answer carrying no body, not about any of those.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one tag, its only key removed",
            id: "frame-one",
            tags: Some(json!({ "environment": "production" })),
            remove: &["environment"],
        },
        Row {
            case: "several tags, two keys removed",
            id: "Frame.Two_2",
            tags: Some(json!({ "a": "1", "b": "2", "c": "3", "d": "4" })),
            remove: &["a", "c"],
        },
        Row {
            case: "no tags at all, a key the resource does not carry",
            id: "frame_three",
            tags: None,
            remove: &["environment"],
        },
        Row {
            case: "non-ASCII tags, one key removed",
            id: "frame-4",
            tags: Some(json!({ "região": "são-paulo", "プロジェクト": "配信" })),
            remove: &["região"],
        },
        Row {
            case: "a key named twice in one request, another kept",
            id: "f5",
            tags: Some(json!({ "keep": "1", "drop": "2" })),
            remove: &["drop", "drop"],
        },
    ]
}

/// Every answer of this service that carries no body states `content-length: 0` — the four
/// implemented bodyless operations, over every row — and each of those answers really did
/// the work it reports.
#[tokio::test]
async fn every_bodyless_answer_states_its_length() {
    let router = crate::resource_tag_listing::router();
    create_parent_group(&router, REGION).await;

    for row in rows() {
        // One resource of each type this service tags: the group is its own (so deleting
        // it is not refused for a configuration still naming it), while the configuration
        // and the asset hang off the shared parent group.
        let group = format!("{}-group", row.id);
        let config = format!("{}-cfg", row.id);
        let asset = format!("{}-asset", row.id);
        let group_arn =
            create_with_tags(&router, REGION, KINDS[0], &group, row.tags.as_ref(), row.case).await;
        let config_arn =
            create_with_tags(&router, REGION, KINDS[1], &config, row.tags.as_ref(), row.case).await;
        let asset_arn =
            create_with_tags(&router, REGION, KINDS[2], &asset, row.tags.as_ref(), row.case).await;

        // `UntagResource`: a bodyless 204 for each resource type.
        let mut expected = expected_map(row.tags.as_ref());
        for key in row.remove {
            expected.remove(*key);
        }
        for (kind, arn) in KINDS.iter().zip([&group_arn, &config_arn, &asset_arn]) {
            let case = format!("{}/{}/UntagResource", row.case, kind.name);
            let answer = dispatch(
                &router,
                REGION,
                "DELETE",
                &untag_route(arn, row.remove),
                None,
            )
            .await;
            assert_empty_and_framed(&case, &answer, NO_CONTENT);
            // The 204 reported a removal rather than being an empty answer to nothing.
            let remaining = listed_tags(&router, arn, &case).await;
            assert_eq!(
                remaining, expected,
                "case {case:?}: after removing {:?} expected {expected:?}, got {remaining:?}",
                row.remove
            );
        }

        // The three deletes: the configuration before the group it names, so each delete
        // is an accepted one. Every answer is bodyless, and the resource is gone after it.
        for (operation, route, describe) in [
            (
                "DeleteAsset",
                format!("/assets/{asset}"),
                format!("/assets/{asset}"),
            ),
            (
                "DeletePackagingConfiguration",
                format!("/packaging_configurations/{config}"),
                format!("/packaging_configurations/{config}"),
            ),
            (
                "DeletePackagingGroup",
                format!("/packaging_groups/{group}"),
                format!("/packaging_groups/{group}"),
            ),
        ] {
            let case = format!("{}/{operation}", row.case);
            let answer = dispatch(&router, REGION, "DELETE", &route, None).await;
            assert_empty_and_framed(&case, &answer, NO_CONTENT);
            let gone = call(&router, REGION, "GET", &describe, None).await;
            assert_eq!(
                gone.status, NOT_FOUND,
                "case {case:?}: the deleted resource must read back {NOT_FOUND}, got {} body {}",
                gone.status, gone.body
            );
        }
    }

    // The parent group outlives every row and is still readable, so the deletes above
    // removed exactly what they addressed.
    let parent = call(
        &router,
        REGION,
        "GET",
        &format!("/packaging_groups/{PARENT_GROUP}"),
        None,
    )
    .await;
    assert_eq!(
        parent.status, 200,
        "the shared parent group must survive every delete, got {} body {}",
        parent.status, parent.body
    );
}

/// RULE, the other half: a response that carries content is left exactly as the model's
/// protocol renders it — on the success path, on the error path and on the
/// not-yet-implemented path alike. This service adds no framing of its own to such an
/// answer (the protocol states no length on a bodied response and leaves the framing to
/// the transport), and a length that IS stated is the body's own, never the zero of an
/// answer with no content.
#[tokio::test]
async fn an_answer_with_a_body_is_framed_from_the_body_it_carries() {
    let router = crate::resource_tag_listing::router();
    create_parent_group(&router, REGION).await;
    let tags = json!({ "environment": "production", "team": "video" });
    let group_arn = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "bodied-group",
        Some(&tags),
        "the group the bodied answers are read from",
    )
    .await;

    // Rows: a create (200), a read, a listing, a tag listing, a read of a resource this
    // service does not hold (404), a refused delete (404) and the one modelled bodyless
    // operation that is still a stub, which answers the 501 envelope with a body.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreatePackagingGroup",
            "POST",
            "/packaging_groups".to_string(),
            Some(json!({ "id": "bodied-group-2", "tags": tags.clone() })),
        ),
        (
            "DescribePackagingGroup",
            "GET",
            "/packaging_groups/bodied-group".to_string(),
            None,
        ),
        (
            "ListPackagingGroups",
            "GET",
            "/packaging_groups".to_string(),
            None,
        ),
        ("ListAssets", "GET", "/assets".to_string(), None),
        (
            "ListTagsForResource",
            "GET",
            tags_route(&group_arn),
            None,
        ),
        (
            "DescribeAsset of an asset that does not exist",
            "GET",
            "/assets/no-such-asset".to_string(),
            None,
        ),
        (
            "DeleteAsset of an asset that does not exist",
            "DELETE",
            "/assets/no-such-asset".to_string(),
            None,
        ),
        (
            "TagResource, still a stub",
            "POST",
            tags_route(&group_arn),
            Some(json!({ "tags": { "environment": "production" } })),
        ),
    ];

    let mut responses = Vec::new();
    for (case, method, route, body) in cases {
        responses.push((case, dispatch(&router, REGION, method, &route, body).await));
    }

    for (case, response) in &responses {
        let body = response.body();
        assert!(
            !body.is_empty(),
            "case {case:?}: this answer carries a body, got HTTP {} with none",
            response.status()
        );
        let stated = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok());
        // A bodied answer is left exactly as the model's protocol renders it, which frames
        // it from the body itself and states no length of its own (`content-length` survives
        // only where a handler set one — see `http_response_from_runtime_api` in the
        // vendored router). The two things that must hold are therefore that this service
        // added none of its own framing, and that any length stated is the body's — never
        // the zero of an answer with no content.
        assert_ne!(
            stated,
            Some(EMPTY_BODY_LENGTH),
            "case {case:?}: an answer carrying {} bytes must not state a length of zero",
            body.len()
        );
        assert!(
            stated.is_none_or(|stated| stated == body.len().to_string()),
            "case {case:?}: a stated length must be the body's own, got {stated:?} for {} \
             bytes",
            body.len()
        );
    }
}
