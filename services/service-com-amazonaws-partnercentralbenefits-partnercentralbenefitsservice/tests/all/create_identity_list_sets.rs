// SPDX-License-Identifier: CC0-1.0

//! The identity lists `CreateBenefitApplication` stores, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication` and the `ListBenefitApplications` summary, so what these tests
//! pin is the emulator's HTTP behaviour on the persisted record.
//!
//! The rule under test: a list member whose elements are IDENTITIES — a resource ARN in
//! `AssociatedResources`, a `FulfillmentType` value in `FulfillmentTypes` — denotes a set,
//! so the create stores each distinct element once, in the order the request first named
//! it. Every read reports that set, the per-application bound on associations
//! (`MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`) therefore counts distinct resources
//! whichever path stored them, and one `DisassociateBenefitApplicationResource` is enough
//! to unlink a resource the create named any number of times.
//!
//! The tables' rows differ along what the rule generalises over: how many times an element
//! is repeated, where the repeats sit in the list (adjacent, separated, first, last), how
//! many distinct elements the list carries, which element form it uses (every ARN form the
//! modelled pattern admits, every modelled `FulfillmentType`), a list carrying no repeat at
//! all, and elements that merely LOOK alike (ARNs differing only in their region).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::FULFILLMENT_TYPE_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION;

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::disassociate_benefit_application_resource::{associated, body, seed, RESOURCE_ARNS};

/// The string entries of a list member of one response, in the order the service reported
/// them; a member the record never stored is absent, which is the empty vector here.
///
/// Shared with `tests/all/create_canonical_request_members.rs`, which reads the same stored
/// sets back.
pub(crate) fn entries(body: &Value, member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `ListBenefitApplications` summary of one stored application.
async fn summary(harness: &Harness, case: &str, id: &str) -> Value {
    let out = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG }),
        )
        .await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|summary| summary["Id"] == json!(id))
        .unwrap_or_else(|| panic!("case {case:?}: the list reported no summary for {id:?}: {out:?}"))
        .clone()
}

/// A create stores its `AssociatedResources` as the set the list denotes: each distinct ARN
/// once, in the order the request first named it, as reported by both reads.
#[tokio::test]
async fn a_create_stores_its_associated_resources_as_a_set() {
    let harness = Harness::new();
    let [bucket, instance, role, function, access_point, cell] = RESOURCE_ARNS;
    // The same EC2 instance named in another region: a different resource, so not a repeat.
    let other_region = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
    // The same ARN in every slot the member's modelled `@length {max: 10}` admits
    // (`smithy_query(query="shape",shape="CreateBenefitApplicationInput")`).
    let every_slot: Vec<&str> = vec![bucket; MAX_ASSOCIATED_RESOURCES_PER_APPLICATION];
    // Every ARN form the modelled `Arn` pattern admits, each named twice — split over two
    // rows, since twelve entries would exceed the member's modelled maximum length and
    // twice six would not fit one request.
    let twice = |arns: &[&'static str]| -> Vec<&'static str> {
        arns.iter().flat_map(|arn| [*arn, *arn]).collect()
    };

    // (case, the AssociatedResources the create sends, what the record must carry)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no AssociatedResources at all", vec![], vec![]),
        ("one ARN, named once", vec![bucket], vec![bucket]),
        (
            "distinct ARNs only, which the rule leaves alone",
            vec![bucket, instance, role],
            vec![bucket, instance, role],
        ),
        ("one ARN named twice, adjacent", vec![bucket, bucket], vec![bucket]),
        (
            "one ARN named twice, separated by another",
            vec![bucket, instance, bucket],
            vec![bucket, instance],
        ),
        (
            "the LAST of three named again",
            vec![bucket, instance, role, role],
            vec![bucket, instance, role],
        ),
        (
            "two ARNs each named twice, interleaved",
            vec![bucket, instance, bucket, instance],
            vec![bucket, instance],
        ),
        (
            "one ARN named three times",
            vec![function, function, function],
            vec![function],
        ),
        (
            "one ARN in every slot the model admits",
            every_slot,
            vec![bucket],
        ),
        (
            "the first three ARN forms the pattern admits, each named twice",
            twice(&[bucket, instance, role]),
            vec![bucket, instance, role],
        ),
        (
            "the last three ARN forms the pattern admits, each named twice",
            twice(&[function, access_point, cell]),
            vec![function, access_point, cell],
        ),
        (
            // Exact identity: these name two different instances, so both are stored.
            "ARNs differing only in their region",
            vec![instance, other_region, instance],
            vec![instance, other_region],
        ),
    ];

    for (row, (case, sent, expected)) in rows.iter().enumerate() {
        let id = seed(
            &harness,
            case,
            CATALOG,
            &format!("tok-create-set-{row}"),
            sent,
        )
        .await;

        let got = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            associated(&got),
            *expected,
            "case {case:?}: expected the associations {expected:?}, got {got:?}"
        );
        assert_eq!(
            entries(&summary(&harness, case, &id).await, "AssociatedResources"),
            *expected,
            "case {case:?}: the list summary must report the same set as the read"
        );
        assert_eq!(
            text(case, &got, "Revision"),
            "1",
            "case {case:?}: a create stores one revision, got {got:?}"
        );
    }
}

/// A create stores its `FulfillmentTypes` as the set the list denotes: each distinct
/// modelled value once, in the order the request first named it.
///
/// The rows are every value of the modelled `FulfillmentType` enum
/// (`FULFILLMENT_TYPE_VALUES`, read from the crate so the set cannot drift from the model),
/// each repeated on its own, plus the whole enum with every value named twice.
#[tokio::test]
async fn a_create_stores_its_fulfillment_types_as_a_set() {
    let harness = Harness::new();
    let benefit_id = text(
        "seed",
        &harness.active_benefit_of(CATALOG).await,
        "Id",
    );

    let all: Vec<String> = FULFILLMENT_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let mut rows: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    for value in &all {
        rows.push((
            format!("{value} named three times"),
            vec![value.clone(), value.clone(), value.clone()],
            vec![value.clone()],
        ));
    }
    let mut each_twice: Vec<String> = Vec::new();
    for value in &all {
        each_twice.push(value.clone());
        each_twice.push(value.clone());
    }
    rows.push((
        "every modelled value, each named twice".to_string(),
        each_twice,
        all.clone(),
    ));
    rows.push((
        "every modelled value, each named once".to_string(),
        all.clone(),
        all.clone(),
    ));

    for (row, (case, sent, expected)) in rows.iter().enumerate() {
        let created = harness
            .create(
                case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-create-fulfillment-{row}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": "fulfillment-set-subject",
                    "FulfillmentTypes": sent,
                }),
            )
            .await;
        let id = text(case, &created, "Id");

        let got = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            entries(&got, "FulfillmentTypes"),
            *expected,
            "case {case:?}: expected the fulfillment types {expected:?}, got {got:?}"
        );
        assert_eq!(
            entries(&summary(&harness, case, &id).await, "FulfillmentTypes"),
            *expected,
            "case {case:?}: the list summary must report the same set as the read"
        );

        // The stored set is still what the list filter selects on.
        for wanted in expected {
            let listed = harness
                .ok(
                    case,
                    "ListBenefitApplications",
                    &json!({ "Catalog": CATALOG, "FulfillmentTypes": [wanted] }),
                )
                .await;
            let matched = listed["BenefitApplicationSummaries"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|summary| summary["Id"] == json!(id));
            assert!(
                matched,
                "case {case:?}: the application must match the filter {wanted:?}, got {listed:?}"
            );
        }
    }
}

/// The bound on one application's associations counts DISTINCT resources, so a create that
/// named one resource in every slot leaves every other slot free: nine further distinct
/// resources are accepted, the tenth fills the list, and only the one past the bound is
/// refused.
#[tokio::test]
async fn a_repeated_create_resource_leaves_the_remaining_slots_free() {
    let harness = Harness::new();
    let case = "one resource named in every slot the model admits";
    let [bucket, ..] = RESOURCE_ARNS;

    let id = seed(
        &harness,
        case,
        CATALOG,
        "tok-create-set-bound",
        &[bucket; MAX_ASSOCIATED_RESOURCES_PER_APPLICATION],
    )
    .await;
    let created = harness.get_in(case, CATALOG, &id).await;
    assert_eq!(
        associated(&created),
        vec![bucket.to_string()],
        "case {case:?}: expected one distinct resource, got {created:?}"
    );

    // One distinct resource is stored, so the record takes the bound minus one more.
    for slot in 1..MAX_ASSOCIATED_RESOURCES_PER_APPLICATION {
        let arn = format!("arn:aws:s3:::partner-proposals-{slot}");
        harness
            .ok(
                case,
                "AssociateBenefitApplicationResource",
                &body(CATALOG, &id, &arn),
            )
            .await;
    }
    let full = harness.get_in(case, CATALOG, &id).await;
    assert_eq!(
        associated(&full).len(),
        MAX_ASSOCIATED_RESOURCES_PER_APPLICATION,
        "case {case:?}: expected the list filled to the bound, got {full:?}"
    );

    let past = "the association past the bound";
    let (http, out) = harness
        .call(
            "AssociateBenefitApplicationResource",
            &body(CATALOG, &id, "arn:aws:s3:::partner-proposals-past"),
        )
        .await;
    assert_eq!(
        http, 400,
        "case {past:?}: expected 400, got {http} with {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {past:?}: expected ValidationException, got {out:?}"
    );
    let after = harness.get_in(past, CATALOG, &id).await;
    assert_eq!(
        after, full,
        "case {past:?}: a refused association must persist nothing, got {after:?}"
    );
}

/// One disassociation unlinks a resource the create named any number of times: the record
/// held one entry for it, so the resource is gone from the read and from the list filter
/// after a single call, exactly as for a resource an association put there.
#[tokio::test]
async fn one_disassociation_unlinks_a_resource_the_create_named_twice() {
    let harness = Harness::new();
    let [bucket, instance, ..] = RESOURCE_ARNS;

    // (case, the AssociatedResources the create sends)
    let rows: [(&str, Vec<&str>); 3] = [
        ("named twice", vec![bucket, bucket, instance]),
        ("named three times", vec![bucket, instance, bucket, bucket]),
        (
            "named in every slot but one",
            vec![
                bucket, bucket, bucket, bucket, bucket, bucket, bucket, bucket, bucket,
                instance,
            ],
        ),
    ];

    for (row, (case, sent)) in rows.iter().enumerate() {
        let id = seed(
            &harness,
            case,
            CATALOG,
            &format!("tok-create-set-remove-{row}"),
            sent,
        )
        .await;

        harness
            .ok(
                case,
                "DisassociateBenefitApplicationResource",
                &body(CATALOG, &id, bucket),
            )
            .await;
        let after = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            associated(&after),
            vec![instance.to_string()],
            "case {case:?}: one disassociation must unlink the resource entirely, got {after:?}"
        );

        let listed = harness
            .ok(
                case,
                "ListBenefitApplications",
                &json!({ "Catalog": CATALOG, "AssociatedResourceArns": [bucket] }),
            )
            .await;
        let matched = listed["BenefitApplicationSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|summary| summary["Id"] == json!(id));
        assert!(
            !matched,
            "case {case:?}: the application must no longer match the removed resource, got {listed:?}"
        );
    }
}
