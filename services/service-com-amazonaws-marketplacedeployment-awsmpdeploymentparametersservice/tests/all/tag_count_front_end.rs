// SPDX-License-Identifier: CC0-1.0

//! The refusal a tagging request earns for the NUMBER of tags it would leave on a
//! resource.
//!
//! The rule under test: AWS's per-resource tag limit is the tagging front end's,
//! not this service's modelled input — so a request whose resultant tag set would
//! carry more entries than one resource may hold (`state::MAX_TAGS_PER_RESOURCE`)
//! is refused with HTTP 400, the UNMODELLED error code
//! `BadRequestException` (`scaffolded::BAD_REQUEST_CODE`) and the fixed sentence
//! `"The resultant tag set must not have more than <max> user tags."`, rather than
//! with the modelled `ValidationException` the service answers a violation of its
//! own input with. The distinction is client-visible on restJson1, where the error
//! SHAPE is resolved from the wire code.
//!
//! The rule holds for every member of this service that carries a tag map — the
//! `tags` of a deployment-parameter write (which targets the model's constrained
//! `TagsMap`, `length max 50`) and the `tags` of a `TagResource` (which targets the
//! map the model leaves unconstrained) — and for both halves of the limit: a set
//! already past it on its own, and one past it only once merged onto the tags a
//! record already holds. The sentence carries nothing from the request: not the
//! count it sent, not a key, and not the member that carried it.
//!
//! At and under the limit the count is no refusal at all, which is what makes the
//! rows above a bound and not a blanket.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::{
    BAD_REQUEST_CODE, BAD_REQUEST_STATUS,
};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, tag, tag_map, tags_of,
    ACCOUNT, CATALOG, REGION, SIGNING_NAME,
};

/// The agreement and product every write below is targeted at.
const AGREEMENT: &str = "agmt-tag-count";
const PRODUCT: &str = "prod-tag-count";
/// The one tag the stored record is created with, which no refused call may change.
const STORED_KEY: &str = "env";
const STORED_VALUE: &str = "dev";

/// The sentence the front end states for that limit: fixed text, with the
/// per-resource maximum the service keeps as its only number.
fn too_many_tags_sentence() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// `count` well-formed pairs, every one of them inside every other bound a tag has
/// to satisfy, so the only thing a row varies is how many there are.
fn pairs(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}-{index:03}"), format!("value-{index:03}")))
        .collect()
}

/// A tag map as the JSON object a request body carries.
fn as_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// One emulator holding one deployment parameter carrying one tag, and that
/// record's ARN.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "TagCount", "secretString": "s" },
            "tags": { STORED_KEY: STORED_VALUE },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

/// Assert `answer` is the front end's count refusal and carries nothing of the
/// request.
fn assert_count_refusal(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, BAD_REQUEST_STATUS,
        "case {case:?}: expected HTTP {BAD_REQUEST_STATUS}, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        BAD_REQUEST_CODE,
        "case {case:?}: expected {BAD_REQUEST_CODE}, got {answer:?}"
    );
    let reported = message(answer);
    assert_eq!(
        reported,
        too_many_tags_sentence(),
        "case {case:?}: expected the front end's sentence, got {reported:?}"
    );
    // Not the modelled shape, so it carries no `fieldName` — the member a modelled
    // refusal names.
    assert_eq!(
        field_name(answer),
        "<no fieldName>",
        "case {case:?}: the unmodelled refusal must carry no fieldName, got {answer:?}"
    );
}

#[tokio::test]
async fn a_put_past_the_tag_count_is_the_front_ends_bad_request() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);

    // (case, the product addressed, how many tags). The rows span how far past the
    // limit a set is and whether the target itself resolves: the count bound belongs
    // to the layer ahead of the handler, so a set past it is refused for its size
    // whatever the store holds.
    let rows: Vec<(&str, &str, usize)> = vec![
        ("one entry past the limit, target held", PRODUCT, MAX_TAGS_PER_RESOURCE + 1),
        (
            "one entry past the limit, target not held",
            "prod-nothing-holds",
            MAX_TAGS_PER_RESOURCE + 1,
        ),
        ("twice the limit, target held", PRODUCT, MAX_TAGS_PER_RESOURCE * 2),
        (
            "far past the limit, target not held",
            "prod-nothing-holds",
            MAX_TAGS_PER_RESOURCE * 4,
        ),
    ];

    for (case, product_id, count) in rows {
        let tags = pairs("key", count);
        assert_eq!(
            tags.len(),
            count,
            "case {case:?}: the row must carry {count} distinct keys, got {}",
            tags.len()
        );
        let (status, answer) = put(
            &router,
            CATALOG,
            product_id,
            &json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "TooManyTags", "secretString": "s" },
                "tags": as_json(&tags),
            }),
        )
        .await;
        assert_count_refusal(case, status, &answer);
        // Nothing the request sent is echoed: not the count, not a key.
        let reported = message(&answer);
        assert!(
            !reported.contains(&count.to_string()) && !reported.contains("key-000"),
            "case {case:?}: the refusal must carry nothing of the request, got {reported:?}"
        );
    }

    // The bound from below: a set AT the limit is served, so the rows above are a
    // bound and not a blanket refusal of every tagged write.
    let at_the_limit = pairs("fits", MAX_TAGS_PER_RESOURCE);
    let (status, created) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "AtTheLimit", "secretString": "s" },
            "tags": as_json(&at_the_limit),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a set exactly at the limit must be written, got {status} {created:?}"
    );
    assert_eq!(
        tags_of(&created).len(),
        MAX_TAGS_PER_RESOURCE,
        "the created record must carry every tag at the limit, got {created:?}"
    );

    // And the record reads back with exactly that set, so the write was stored and
    // not merely answered.
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "the written record must be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        at_the_limit,
        "the stored tag set must be the one the create carried, got {listed:?}"
    );
}

#[tokio::test]
async fn a_tag_resource_past_the_tag_count_is_the_front_ends_bad_request() {
    let (router, live) = emulator_with_one_parameter().await;
    let stored = tag_map(&[(STORED_KEY, STORED_VALUE)]);
    // An ARN of this service's vendor that names no stored record: the count is the
    // request's own fault, so it is refused the same way whether the ARN resolves.
    let unknown =
        format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffffffffffff");

    // (case, the ARN addressed, the set sent). The merge row is the half of the
    // limit that needs the record: every entry of it is new, so merging onto the one
    // tag the record already holds is what carries the result past the limit.
    let rows: Vec<(&str, &str, BTreeMap<String, String>)> = vec![
        (
            "one entry past the limit, against the stored record",
            live.as_str(),
            pairs("over", MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            "one entry past the limit, against an ARN naming no record",
            unknown.as_str(),
            pairs("over", MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            "far past the limit, against the stored record",
            live.as_str(),
            pairs("over", MAX_TAGS_PER_RESOURCE * 3),
        ),
        (
            "a set at the limit that the merge carries past it",
            live.as_str(),
            pairs("merge", MAX_TAGS_PER_RESOURCE),
        ),
    ];

    for (case, resource_arn, tags) in rows {
        let (status, answer) = tag(&router, resource_arn, &tags).await;
        assert_count_refusal(case, status, &answer);
        let (status, listed) = list_tags(&router, &live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored record must still read back, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            stored,
            "case {case:?}: a refused merge must store nothing, got {listed:?}"
        );
    }

    // The bound from below, through the merge: a set landing the record exactly ON
    // the limit is accepted.
    let onto_the_limit = pairs("fill", MAX_TAGS_PER_RESOURCE - 1);
    let (status, answer) = tag(&router, &live, &onto_the_limit).await;
    assert_eq!(
        status, 204,
        "a merge landing exactly on the limit must be accepted, got {status} {answer:?}"
    );
    let (_, listed) = list_tags(&router, &live).await;
    assert_eq!(
        tags_of(&listed).len(),
        MAX_TAGS_PER_RESOURCE,
        "expected the record to hold exactly the limit, got {listed:?}"
    );

    // One more entry onto a record already at the limit is the same refusal, read off
    // the record rather than off the request.
    let (status, answer) = tag(&router, &live, &tag_map(&[("one-too-many", "v")])).await;
    assert_count_refusal("one entry onto a record already at the limit", status, &answer);
}

#[tokio::test]
async fn a_tag_fault_this_service_models_keeps_the_modelled_identity() {
    // The complement of the rule: only the COUNT belongs to the front end. A tag the
    // service refuses on what a tag IS stays the modelled `ValidationException`, with
    // the member it is about — so the two refusals are told apart by their identity
    // and not merely by both being a 400.
    let (router, live) = emulator_with_one_parameter().await;
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");

    let (status, answer) = tag(&router, &live, &tag_map(&[(reserved.as_str(), "mystack")])).await;
    assert_eq!(
        status, 400,
        "a reserved tag key must be refused, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "a fault this service models must keep the modelled identity, got {answer:?}"
    );
    assert_eq!(
        field_name(&answer),
        "tags",
        "the modelled refusal must name the member it is about, got {answer:?}"
    );
    assert_ne!(
        message(&answer),
        too_many_tags_sentence(),
        "a reserved key is not the count refusal, got {answer:?}"
    );

    // And a set past the count, on the same ARN, is the other identity.
    let (status, answer) = tag(&router, &live, &pairs("over", MAX_TAGS_PER_RESOURCE + 1)).await;
    assert_count_refusal("the same ARN, a set past the count", status, &answer);
}
