// SPDX-License-Identifier: CC0-1.0

//! `ListBenefitApplications`' list filter members, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what these tests pin is the emulator's
//! HTTP behaviour on the persisted records, not an internal helper.
//!
//! The rule under test: a list filter the caller SUPPLIED selects only the applications
//! matching one of the values its entries resolve to, so entries that resolve to no value
//! this listing can compare narrow the result to nothing; only a filter the caller omitted
//! or sent empty selects every application. It holds over every list filter member of the
//! operation, so it is asserted over the resource members (`AssociatedResourceArns` and the
//! `@deprecated` `AssociatedResources`, whose entries resolve to their optional
//! `ResourceArn`) and over `BenefitIdentifiers` (whose entries resolve to a benefit of the
//! catalog).
//!
//! The tables' rows differ along what the rule generalises over: which member carries the
//! filter, how many entries it has, which of the entry's optional members are present,
//! whether the value an entry resolves to is one a stored application carries, how many
//! resources the application carries, and whether two filter members are combined.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::RESOURCE_TYPE_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::BENEFIT_RESOURCE;

use crate::benefit_application_lifecycle::{text, Harness, CATALOG};
use crate::disassociate_benefit_application_resource::{associated, RESOURCE_ARNS};
use crate::resource_tagging::expected_arn;

/// A benefit id of the modelled `BenefitId` form (`^(arn:.+|ben-[0-9a-z]{14})$`) that no
/// catalog holds, so a filter naming it resolves to nothing.
const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// An ARN of the modelled `Arn` form that no application is associated with.
const UNASSOCIATED_ARN: &str = "arn:aws:s3:::no-partner-proposals";

/// Create one application of `benefit_identifier` carrying `resources`, read it back
/// through `GetBenefitApplication` to prove the associations were persisted, and return its
/// id.
async fn seed(
    harness: &Harness,
    case: &str,
    token: &str,
    benefit_identifier: &str,
    resources: &[&str],
) -> String {
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_identifier,
                "Name": "filter-subject",
                "Description": "an application the list filter rule is asserted on",
                "AssociatedResources": resources,
            }),
        )
        .await;
    let id = text(case, &created, "Id");
    let stored = harness.get_in(case, CATALOG, &id).await;
    let expected: Vec<String> = resources.iter().map(|arn| (*arn).to_string()).collect();
    assert_eq!(
        associated(&stored),
        expected,
        "case {case:?}: expected the stored application to carry {expected:?}, got {stored:?}"
    );
    id
}

/// One `ListBenefitApplications` body: the required `Catalog` plus the filter members the
/// row supplies.
fn request(filter: &Value) -> Value {
    let mut body = json!({ "Catalog": CATALOG });
    for (member, value) in filter
        .as_object()
        .expect("each row's filter is a JSON object of request members")
    {
        body[member] = value.clone();
    }
    body
}

/// The ids a page reports, as a set so a row states which applications are selected without
/// restating the catalog's ordering (which `tests/all/pagination_cursor_issuance.rs` pins).
fn listed(case: &str, out: &Value) -> BTreeSet<String> {
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {out:?}"))
        .iter()
        .map(|summary| text(case, summary, "Id"))
        .collect()
}

/// The expected set of a row, from the ids it names.
fn expect(ids: &[&String]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).clone()).collect()
}

/// A supplied resource filter selects only the applications carrying one of the ARNs its
/// entries resolve to; only an omitted or empty one selects every application.
///
/// The rows span both resource members and both at once, an empty and a one- and
/// two-entry filter, each subset of the `@deprecated` entry's three optional members
/// (`ResourceArn` alone, with the others, and every combination carrying none), an ARN no
/// application carries, and applications carrying none, one and two resources.
#[tokio::test]
async fn a_supplied_resource_filter_selects_only_the_applications_carrying_its_arns() {
    let harness = Harness::new();
    let [bucket, instance, role, ..] = RESOURCE_ARNS;
    let benefit = text(
        "seed",
        &harness.active_benefit_of(CATALOG).await,
        "Id",
    );

    let one = seed(&harness, "one resource", "tok-filter-one", &benefit, &[bucket]).await;
    let two = seed(
        &harness,
        "two resources",
        "tok-filter-two",
        &benefit,
        &[instance, role],
    )
    .await;
    let none = seed(&harness, "no resources", "tok-filter-none", &benefit, &[]).await;
    let all = expect(&[&one, &two, &none]);

    // `ResourceType` is the modelled `ResourceType` enum
    // (smithy_query(query="shape",shape="ResourceType"): OPPORTUNITY, BENEFIT_ALLOCATION,
    // mirrored by `crate::ops::types::RESOURCE_TYPE_VALUES`), and `ResourceIdentifier` is
    // the `@deprecated` free-form identifier of a resource inside its own service.
    let resource_type = RESOURCE_TYPE_VALUES[0];
    let other_type = RESOURCE_TYPE_VALUES[1];
    let identifier = "i-0123456789abcdef0";

    // (case, the filter members the request carries, the applications it must select)
    let rows: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("no resource filter at all", json!({}), all.clone()),
        (
            "an empty AssociatedResourceArns",
            json!({ "AssociatedResourceArns": [] }),
            all.clone(),
        ),
        (
            "an empty AssociatedResources",
            json!({ "AssociatedResources": [] }),
            all.clone(),
        ),
        (
            "both resource members empty",
            json!({ "AssociatedResourceArns": [], "AssociatedResources": [] }),
            all.clone(),
        ),
        (
            "one ARN, carried by one application",
            json!({ "AssociatedResourceArns": [bucket] }),
            expect(&[&one]),
        ),
        (
            "one ARN of an application carrying two resources",
            json!({ "AssociatedResourceArns": [role] }),
            expect(&[&two]),
        ),
        (
            "two ARNs, each carried by a different application",
            json!({ "AssociatedResourceArns": [bucket, instance] }),
            expect(&[&one, &two]),
        ),
        (
            "two ARNs, both carried by the same application",
            json!({ "AssociatedResourceArns": [instance, role] }),
            expect(&[&two]),
        ),
        (
            "an ARN no application carries",
            json!({ "AssociatedResourceArns": [UNASSOCIATED_ARN] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying only its ResourceArn",
            json!({ "AssociatedResources": [{ "ResourceArn": bucket }] }),
            expect(&[&one]),
        ),
        (
            "a deprecated entry carrying all three of its members",
            json!({ "AssociatedResources": [{
                "ResourceArn": bucket,
                "ResourceType": resource_type,
                "ResourceIdentifier": identifier,
            }] }),
            expect(&[&one]),
        ),
        (
            "two deprecated entries naming the two resources of one application",
            json!({ "AssociatedResources": [
                { "ResourceArn": instance },
                { "ResourceArn": role },
            ] }),
            expect(&[&two]),
        ),
        (
            "a deprecated entry naming an ARN no application carries",
            json!({ "AssociatedResources": [{ "ResourceArn": UNASSOCIATED_ARN }] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying only a ResourceType",
            json!({ "AssociatedResources": [{ "ResourceType": resource_type }] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying only a ResourceIdentifier",
            json!({ "AssociatedResources": [{ "ResourceIdentifier": identifier }] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying a ResourceType and a ResourceIdentifier",
            json!({ "AssociatedResources": [{
                "ResourceType": other_type,
                "ResourceIdentifier": identifier,
            }] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying none of its optional members",
            json!({ "AssociatedResources": [{}] }),
            BTreeSet::new(),
        ),
        (
            "several deprecated entries, none carrying a ResourceArn",
            json!({ "AssociatedResources": [
                { "ResourceType": resource_type },
                { "ResourceIdentifier": identifier },
                {},
            ] }),
            BTreeSet::new(),
        ),
        (
            "a deprecated entry carrying an ARN beside one that does not",
            json!({ "AssociatedResources": [
                { "ResourceArn": bucket },
                { "ResourceType": resource_type },
            ] }),
            expect(&[&one]),
        ),
        (
            "both resource members, each naming a different application",
            json!({
                "AssociatedResourceArns": [bucket],
                "AssociatedResources": [{ "ResourceArn": instance }],
            }),
            expect(&[&one, &two]),
        ),
        (
            "both resource members, the deprecated one carrying no ARN",
            json!({
                "AssociatedResourceArns": [bucket],
                "AssociatedResources": [{ "ResourceType": resource_type }],
            }),
            expect(&[&one]),
        ),
        (
            "an empty AssociatedResourceArns beside a deprecated entry carrying no ARN",
            json!({
                "AssociatedResourceArns": [],
                "AssociatedResources": [{ "ResourceIdentifier": identifier }],
            }),
            BTreeSet::new(),
        ),
    ];

    for (case, filter, expected) in &rows {
        let out = harness
            .ok(case, "ListBenefitApplications", &request(filter))
            .await;
        let selected = listed(case, &out);
        assert_eq!(
            &selected, expected,
            "case {case:?}: expected the filter to select {expected:?}, got {selected:?}"
        );
    }
}

/// The same rule over `BenefitIdentifiers`, including a row where it is combined with a
/// resource filter: each filter member narrows the listing, and a member the caller
/// supplied that resolves to nothing selects no application.
///
/// The rows span an omitted and an empty filter, both identifier forms the modelled
/// `BenefitId` admits (a bare id and a benefit ARN of the catalog), each of two benefits
/// the catalog holds, a benefit id no catalog holds, and the member combined with a
/// resource filter that agrees and one that does not.
#[tokio::test]
async fn a_supplied_benefit_identifier_filter_selects_only_its_benefit_s_applications() {
    let harness = Harness::new();
    let [bucket, instance, ..] = RESOURCE_ARNS;
    let benefits = harness.benefits(CATALOG).await;
    let active: Vec<String> = benefits
        .iter()
        .filter(|benefit| benefit["Status"] == json!("ACTIVE"))
        .map(|benefit| text("seed", benefit, "Id"))
        .collect();
    assert!(
        active.len() >= 2,
        "case {CATALOG:?}: expected at least two ACTIVE benefits to filter by, got {active:?}"
    );
    let (first, second) = (&active[0], &active[1]);

    let of_first = seed(&harness, "first benefit", "tok-benefit-one", first, &[bucket]).await;
    let of_second = seed(
        &harness,
        "second benefit",
        "tok-benefit-two",
        second,
        &[instance],
    )
    .await;
    let all = expect(&[&of_first, &of_second]);
    let first_arn = expected_arn(CATALOG, BENEFIT_RESOURCE, first);

    // (case, the filter members the request carries, the applications it must select)
    let rows: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("no benefit filter at all", json!({}), all.clone()),
        (
            "an empty BenefitIdentifiers",
            json!({ "BenefitIdentifiers": [] }),
            all.clone(),
        ),
        (
            "the bare id of the first benefit",
            json!({ "BenefitIdentifiers": [first] }),
            expect(&[&of_first]),
        ),
        (
            "the ARN of the first benefit",
            json!({ "BenefitIdentifiers": [first_arn] }),
            expect(&[&of_first]),
        ),
        (
            "the bare id of the second benefit",
            json!({ "BenefitIdentifiers": [second] }),
            expect(&[&of_second]),
        ),
        (
            "a benefit id no catalog holds",
            json!({ "BenefitIdentifiers": [UNKNOWN_BENEFIT] }),
            BTreeSet::new(),
        ),
        (
            "a benefit filter and a resource filter that agree",
            json!({
                "BenefitIdentifiers": [first],
                "AssociatedResourceArns": [bucket],
            }),
            expect(&[&of_first]),
        ),
        (
            "a benefit filter and a resource filter that disagree",
            json!({
                "BenefitIdentifiers": [second],
                "AssociatedResourceArns": [bucket],
            }),
            BTreeSet::new(),
        ),
        (
            "a benefit filter beside a deprecated resource entry carrying no ARN",
            json!({
                "BenefitIdentifiers": [first],
                "AssociatedResources": [{ "ResourceType": RESOURCE_TYPE_VALUES[0] }],
            }),
            BTreeSet::new(),
        ),
    ];

    for (case, filter, expected) in &rows {
        let out = harness
            .ok(case, "ListBenefitApplications", &request(filter))
            .await;
        let selected = listed(case, &out);
        assert_eq!(
            &selected, expected,
            "case {case:?}: expected the filter to select {expected:?}, got {selected:?}"
        );
    }
}
