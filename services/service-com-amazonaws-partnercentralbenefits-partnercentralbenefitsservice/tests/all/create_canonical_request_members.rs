// SPDX-License-Identifier: CC0-1.0

//! WHICH create request a `ClientToken` identifies when the create CANONICALISES a member
//! before storing it, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, `ListBenefitApplications` and `ListTagsForResource`, so what
//! these tests pin is the emulator's HTTP behaviour on the persisted record.
//!
//! The rule under test: a `ClientToken` identifies the REQUEST by the values the operation
//! would STORE for its members, not by the spelling they were written with, so two
//! requests this create would keep as one record are one request and the second is replayed
//! — while a request whose stored record would differ is a different request and earns the
//! modelled `ConflictException` (HTTP 409). The create canonicalises three of its members:
//! `AssociatedResources` and `FulfillmentTypes` are kept as the SETS they denote, and
//! `Tags` as the MAP it denotes. A member the request OMITS is still left out of the
//! request's identity, so dropping one is a different request however the record would
//! read — that boundary is pinned by the second table here and by
//! `tests/all/idempotent_parameter_mismatch.rs`.
//!
//! The rows differ along what the rule generalises over: WHICH canonicalised member
//! carries the difference, how an entry is repeated (adjacent, separated, three times, in
//! every slot the model admits), how many distinct entries the list carries, which element
//! form it uses (every ARN form the modelled `Arn` pattern admits, every modelled
//! `FulfillmentType`), which direction the pair is sent in, the order tag keys are written
//! in, and a tag key written twice — against the spellings that genuinely change the stored
//! record (membership, list order, a tag's value, a member dropped).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::FULFILLMENT_TYPE_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    IDEMPOTENT_PARAMETER_MISMATCH, MAX_ASSOCIATED_RESOURCES_PER_APPLICATION,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::client_token_request_identity::application_ids;
use crate::create_identity_list_sets::entries;
use crate::disassociate_benefit_application_resource::RESOURCE_ARNS;
use crate::resource_tagging::{error_message, tag_payload, tags_of};

/// The members one create request carries beyond the ones every row shares.
type Members = Vec<(&'static str, Value)>;

/// A pair of spellings the create stores IDENTICALLY, with the record each must leave.
struct Replayed {
    case: String,
    first: Members,
    retry: Members,
    /// The `FulfillmentTypes` the stored record must report — the set the list denotes.
    fulfillment: Vec<String>,
    /// The `AssociatedResources` the stored record must report — the set the list denotes.
    resources: Vec<String>,
    /// The tags `ListTagsForResource` must report — the map the list denotes.
    tags: BTreeMap<String, String>,
}

/// A list member's wire value.
fn list(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|value| json!(value)).collect())
}

/// `values` as the owned strings a stored list is compared against.
fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// `pairs` as the tag map a read must report.
fn tag_set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One create request body: the members every row shares, plus `members`.
fn create_body(benefit_id: &str, token: &str, members: &Members) -> Value {
    let mut body = json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "BenefitIdentifier": benefit_id,
        "Name": "canonical-members",
        "Description": "one create, however its canonicalised members are spelled",
    });
    for (member, value) in members {
        body[*member] = value.clone();
    }
    body
}

/// Assert `out` is the refusal a spent `ClientToken` on a different request earns: the
/// modelled `ConflictException`, 409, in AWS's words for this error class.
fn assert_mismatch(case: &str, status: u16, out: &Value) {
    assert_eq!(
        status, 409,
        "case {case:?}: a spent ClientToken on a request the store would keep differently must answer 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(out),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {out:?}"
    );
    assert_eq!(
        error_message(out),
        IDEMPOTENT_PARAMETER_MISMATCH,
        "case {case:?}: expected the idempotency-mismatch message, got {out:?}"
    );
}

/// Every pair of spellings the create keeps as one record.
fn replayed_rows() -> Vec<Replayed> {
    let [bucket, instance, role, function, access_point, cell] = RESOURCE_ARNS;
    let every_slot: Vec<&str> = vec![bucket; MAX_ASSOCIATED_RESOURCES_PER_APPLICATION];
    let resources = |values: &[&str]| -> Members { vec![("AssociatedResources", list(values))] };
    let fulfillment = |values: &[&str]| -> Members { vec![("FulfillmentTypes", list(values))] };
    let tags = |pairs: &[(&str, &str)]| -> Members { vec![("Tags", tag_payload(pairs))] };

    let mut rows: Vec<Replayed> = vec![
        Replayed {
            case: "one resource named twice, retried once".to_string(),
            first: resources(&[bucket, bucket]),
            retry: resources(&[bucket]),
            fulfillment: Vec::new(),
            resources: owned(&[bucket]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "one resource named three times, retried once".to_string(),
            first: resources(&[function, function, function]),
            retry: resources(&[function]),
            fulfillment: Vec::new(),
            resources: owned(&[function]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "a repeat separated by another resource".to_string(),
            first: resources(&[bucket, instance, bucket]),
            retry: resources(&[bucket, instance]),
            fulfillment: Vec::new(),
            resources: owned(&[bucket, instance]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "the last of three resources named again".to_string(),
            first: resources(&[bucket, instance, role, role]),
            retry: resources(&[bucket, instance, role]),
            fulfillment: Vec::new(),
            resources: owned(&[bucket, instance, role]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "two resources each named twice, interleaved".to_string(),
            first: resources(&[bucket, instance, bucket, instance]),
            retry: resources(&[bucket, instance]),
            fulfillment: Vec::new(),
            resources: owned(&[bucket, instance]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "one resource in every slot the model admits".to_string(),
            first: resources(&every_slot),
            retry: resources(&[bucket]),
            fulfillment: Vec::new(),
            resources: owned(&[bucket]),
            tags: BTreeMap::new(),
        },
        Replayed {
            // The remaining ARN forms the modelled `Arn` pattern admits, so the rule is
            // not pinned on one resource shape (`RESOURCE_ARNS` names each form).
            case: "three further ARN forms, each named twice".to_string(),
            first: resources(&[function, function, access_point, access_point, cell, cell]),
            retry: resources(&[function, access_point, cell]),
            fulfillment: Vec::new(),
            resources: owned(&[function, access_point, cell]),
            tags: BTreeMap::new(),
        },
        Replayed {
            case: "tag keys written in another order".to_string(),
            first: tags(&[("team", "alliances"), ("stage", "draft"), ("owner", "ada")]),
            retry: tags(&[("owner", "ada"), ("team", "alliances"), ("stage", "draft")]),
            fulfillment: Vec::new(),
            resources: Vec::new(),
            tags: tag_set(&[("team", "alliances"), ("stage", "draft"), ("owner", "ada")]),
        },
        Replayed {
            case: "one tag key written twice with the same value".to_string(),
            first: tags(&[("team", "alliances"), ("team", "alliances")]),
            retry: tags(&[("team", "alliances")]),
            fulfillment: Vec::new(),
            resources: Vec::new(),
            tags: tag_set(&[("team", "alliances")]),
        },
        Replayed {
            // A key written twice takes the LATER value, so the single entry carrying
            // that value is the same stored map.
            case: "one tag key written twice, the later value winning".to_string(),
            first: tags(&[("team", "alliances"), ("team", "marketing")]),
            retry: tags(&[("team", "marketing")]),
            fulfillment: Vec::new(),
            resources: Vec::new(),
            tags: tag_set(&[("team", "marketing")]),
        },
        Replayed {
            case: "every canonicalised member spelled another way at once".to_string(),
            first: vec![
                ("AssociatedResources", list(&[bucket, bucket, instance])),
                ("FulfillmentTypes", list(&["CASH", "CASH", "ACCESS"])),
                (
                    "Tags",
                    tag_payload(&[("stage", "draft"), ("team", "alliances"), ("team", "alliances")]),
                ),
            ],
            retry: vec![
                ("AssociatedResources", list(&[bucket, instance])),
                ("FulfillmentTypes", list(&["CASH", "ACCESS"])),
                (
                    "Tags",
                    tag_payload(&[("team", "alliances"), ("stage", "draft")]),
                ),
            ],
            fulfillment: owned(&["CASH", "ACCESS"]),
            resources: owned(&[bucket, instance]),
            tags: tag_set(&[("team", "alliances"), ("stage", "draft")]),
        },
    ];

    // One row per value of the modelled `FulfillmentType` enum, read from the crate
    // (`FULFILLMENT_TYPE_VALUES`) so the catalog of values cannot drift from the model,
    // plus the whole enum with every value named twice.
    for value in FULFILLMENT_TYPE_VALUES {
        rows.push(Replayed {
            case: format!("the fulfillment type {value} named three times, retried once"),
            first: fulfillment(&[value, value, value]),
            retry: fulfillment(&[value]),
            fulfillment: owned(&[value]),
            resources: Vec::new(),
            tags: BTreeMap::new(),
        });
    }
    let each_twice: Vec<&str> = FULFILLMENT_TYPE_VALUES
        .iter()
        .flat_map(|value| [*value, *value])
        .collect();
    rows.push(Replayed {
        case: "every modelled fulfillment type, each named twice".to_string(),
        first: fulfillment(&each_twice),
        retry: fulfillment(FULFILLMENT_TYPE_VALUES),
        fulfillment: owned(FULFILLMENT_TYPE_VALUES),
        resources: Vec::new(),
        tags: BTreeMap::new(),
    });

    rows
}

/// A create retry whose canonicalised members denote what the first call STORED is the
/// same request: it answers that call's `Id`, `Arn` and `Revision`, mints no second
/// application and changes nothing — in both directions, so neither spelling is
/// privileged.
#[tokio::test]
async fn a_create_retry_the_store_keeps_identically_is_replayed() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit_of(CATALOG).await, "Id");

    for (index, row) in replayed_rows().iter().enumerate() {
        let case = row.case.as_str();
        for (direction, slug, swapped) in [
            ("as sent, then the retry", "forward", false),
            ("the retry's spelling first", "reverse", true),
        ] {
            let (sent, retried) = if swapped {
                (&row.retry, &row.first)
            } else {
                (&row.first, &row.retry)
            };
            let token = format!("canonical-{index}-{slug}");
            let first = harness
                .create(case, &create_body(&benefit_id, &token, sent))
                .await;
            let id = text(case, &first, "Id");
            let arn = text(case, &first, "Arn");
            let stored = harness.get_in(case, CATALOG, &id).await;
            let stored_tags = tags_of(&harness, case, &arn).await;
            let minted = application_ids(&harness, case, CATALOG).await;

            // Both spellings are stored as the canonical value the rule names, which is
            // why the retry below is the same request.
            assert_eq!(
                entries(&stored, "FulfillmentTypes"),
                row.fulfillment,
                "case {case:?} ({direction}): expected the fulfillment set {:?}, got {stored:?}",
                row.fulfillment
            );
            assert_eq!(
                entries(&stored, "AssociatedResources"),
                row.resources,
                "case {case:?} ({direction}): expected the association set {:?}, got {stored:?}",
                row.resources
            );
            assert_eq!(
                stored_tags, row.tags,
                "case {case:?} ({direction}): expected the tag map {:?}, got {stored_tags:?}",
                row.tags
            );

            let (status, out) = harness
                .call(
                    "CreateBenefitApplication",
                    &create_body(&benefit_id, &token, retried),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({direction}): the retry must be replayed, got {status} {out:?}"
            );
            assert_eq!(
                out, first,
                "case {case:?} ({direction}): the retry must answer the first call's output {first:?}, got {out:?}"
            );
            assert_eq!(
                application_ids(&harness, case, CATALOG).await,
                minted,
                "case {case:?} ({direction}): a replayed create must mint nothing"
            );
            let after = harness.get_in(case, CATALOG, &id).await;
            assert_eq!(
                after, stored,
                "case {case:?} ({direction}): a replayed create must change nothing, expected {stored:?}, got {after:?}"
            );
            let tags_after = tags_of(&harness, case, &arn).await;
            assert_eq!(
                tags_after, stored_tags,
                "case {case:?} ({direction}): a replayed create must leave the tags alone, expected {stored_tags:?}, got {tags_after:?}"
            );
        }
    }
}

/// A create repeating a spent `ClientToken` with a member the store would keep DIFFERENTLY
/// is a different request: it is refused, mints nothing and leaves the first application
/// alone, while the very same request under a fresh token is served.
///
/// The rows are the spellings that do change the record: list membership, list order
/// (which the stored set keeps, first appearance first), a tag's value, a tag key dropped,
/// and a member omitted rather than supplied.
#[tokio::test]
async fn a_create_retry_the_store_would_keep_differently_is_refused() {
    let [bucket, instance, ..] = RESOURCE_ARNS;
    let resources = |values: &[&str]| -> Members { vec![("AssociatedResources", list(values))] };
    let fulfillment = |values: &[&str]| -> Members { vec![("FulfillmentTypes", list(values))] };
    let tags = |pairs: &[(&str, &str)]| -> Members { vec![("Tags", tag_payload(pairs))] };

    // (case, the first create's members, the second request's members)
    let rows: Vec<(&str, Members, Members)> = vec![
        (
            "one resource fewer",
            resources(&[bucket, instance]),
            resources(&[bucket]),
        ),
        (
            "the same resources in another order",
            resources(&[bucket, instance]),
            resources(&[instance, bucket]),
        ),
        (
            "a repeat replaced by another resource",
            resources(&[bucket, bucket]),
            resources(&[bucket, instance]),
        ),
        (
            "the resources omitted rather than named twice",
            resources(&[bucket, bucket]),
            Vec::new(),
        ),
        (
            "an empty resource list rather than none at all",
            resources(&[]),
            Vec::new(),
        ),
        (
            "another fulfillment type",
            fulfillment(&["CASH", "CASH"]),
            fulfillment(&["CREDITS"]),
        ),
        (
            "one more fulfillment type",
            fulfillment(&["CASH", "CASH"]),
            fulfillment(&["CASH", "CREDITS"]),
        ),
        (
            "the same fulfillment types in another order",
            fulfillment(&["CASH", "CREDITS"]),
            fulfillment(&["CREDITS", "CASH"]),
        ),
        (
            "a tag value that differs",
            tags(&[("team", "alliances"), ("team", "alliances")]),
            tags(&[("team", "marketing")]),
        ),
        (
            "a repeated tag key whose winning value differs",
            tags(&[("team", "alliances"), ("team", "marketing")]),
            tags(&[("team", "alliances")]),
        ),
        (
            "a tag key dropped",
            tags(&[("team", "alliances"), ("stage", "draft")]),
            tags(&[("team", "alliances")]),
        ),
    ];

    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit_of(CATALOG).await, "Id");
    for (index, (case, first_members, second_members)) in rows.iter().enumerate() {
        let token = format!("canonical-mismatch-{index}");
        let first = harness
            .create(case, &create_body(&benefit_id, &token, first_members))
            .await;
        let id = text(case, &first, "Id");
        let stored = harness.get_in(case, CATALOG, &id).await;
        let minted = application_ids(&harness, case, CATALOG).await;

        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &create_body(&benefit_id, &token, second_members),
            )
            .await;
        assert_mismatch(case, status, &out);
        assert_eq!(
            application_ids(&harness, case, CATALOG).await,
            minted,
            "case {case:?}: a refused create must mint nothing"
        );
        let after = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            after, stored,
            "case {case:?}: a refused create must leave the first application alone, expected {stored:?}, got {after:?}"
        );

        // The identical request still replays: the mismatch rule does not cost the retry
        // the token exists for.
        let replay = harness
            .create(case, &create_body(&benefit_id, &token, first_members))
            .await;
        assert_eq!(
            replay, first,
            "case {case:?}: the identical request must still replay {first:?}, got {replay:?}"
        );

        // Under a FRESH token the second request is the request it always was.
        let fresh = harness
            .create(
                case,
                &create_body(
                    &benefit_id,
                    &format!("canonical-fresh-{index}"),
                    second_members,
                ),
            )
            .await;
        assert_ne!(
            fresh["Id"], first["Id"],
            "case {case:?}: under a fresh token the second request must mint its own application, got {fresh:?}"
        );
    }
}
