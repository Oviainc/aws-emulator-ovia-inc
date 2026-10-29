// SPDX-License-Identifier: CC0-1.0

//! The ARN form of this service's identifier members, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what these tests pin is the emulator's
//! HTTP behaviour, not an internal helper.
//!
//! The rule under test: an identifier member whose modelled pattern admits both forms
//! (`^(arn:.+|<prefix>-[0-9a-z]{14})$`) names a record through an ARN only when that ARN is
//! the one the service itself issues for it, every field included — so an ARN whose
//! partition, region or account is not the one this service mints names NO record, exactly
//! as one of another catalog, another service or another resource type does not.
//!
//! The rule is one rule for the whole identifier family, so it is asserted on every
//! resource type this service names with such a member (a benefit, a benefit allocation,
//! a benefit application) and on every operation that binds one: the three reads, the
//! `BenefitIdentifier` of the create, and the `BenefitIdentifiers` filters of the two
//! listings.
//!
//! The tables' rows differ along what the rule generalises over: WHICH ARN field differs
//! from the minted one (partition, region, account, several at once), the value it carries
//! (another AWS partition, a partition no AWS ARN uses, another region, no region at all,
//! another twelve-digit account), the resource type the ARN names, the catalog it lives in,
//! and the operation the identifier is given to. Each row's ARN is one the modelled `Arn`
//! pattern still admits, so what a refusal answers is the FIELD rather than a malformed
//! identifier.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, ALLOCATION_KIND, ALLOCATION_RESOURCE, APPLICATION_KIND,
    APPLICATION_RESOURCE, BENEFIT_KIND, BENEFIT_RESOURCE, SEEDED_CATALOGS,
};

use crate::benefit_allocations::{ids, list, stored_allocations};
use crate::benefit_application_lifecycle::{error_code, text, Harness, ACCOUNT, CATALOG, REGION};
use crate::resource_tagging::{error_message, expected_arn};

/// The partition every ARN this emulator mints carries (`engine::emulator_arn` builds
/// `arn:aws:<service>:<region>:<account>:<resource>`).
const PARTITION: &str = "aws";

/// One identifier spelled as an ARN of this service: the three identity fields a row
/// varies, then the resource the ARN names.
///
/// Spelled here rather than through the service's own ARN helper precisely because the
/// point of a row is to carry a field the service does NOT mint.
fn arn_with(
    partition: &str,
    region: &str,
    account: &str,
    catalog: &str,
    resource: &str,
    id: &str,
) -> String {
    format!("arn:{partition}:partnercentral:{region}:{account}:catalog/{catalog}/{resource}/{id}")
}

/// Each row: a case name and the (partition, region, account) it spells an ARN with, none
/// of them the triple this service mints (`aws`, the region the request addressed, the
/// emulator account id).
///
/// Every value is one the modelled ARN pattern of this service admits
/// (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/...$`,
/// from `ListTagsForResource$resourceArn`), the empty region included, so no row is
/// refused for being ill-formed.
fn foreign_fields() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        ("another AWS partition", "aws-cn", REGION, ACCOUNT),
        ("the GovCloud partition", "aws-us-gov", REGION, ACCOUNT),
        ("a partition no AWS ARN uses", "not-aws", REGION, ACCOUNT),
        ("another region", PARTITION, "eu-west-1", ACCOUNT),
        (
            "a region in another geography",
            PARTITION,
            "ap-southeast-2",
            ACCOUNT,
        ),
        ("no region at all", PARTITION, "", ACCOUNT),
        ("another account", PARTITION, REGION, "123456789012"),
        ("another account again", PARTITION, REGION, "210987654321"),
        (
            "another region AND another account",
            PARTITION,
            "eu-west-1",
            "123456789012",
        ),
        (
            "another partition, region and account at once",
            "aws-cn",
            "eu-west-1",
            "123456789012",
        ),
    ]
}

/// One call that must be refused `ResourceNotFoundException` (404) naming the identifier
/// the request carried — the one answer this service gives a lookup that names no record.
async fn refused(
    harness: &Harness,
    case: &str,
    operation: &str,
    body: &Value,
    kind: &str,
    identifier: &str,
) {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected {operation} to answer 404, got {status} with {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        not_found_message(kind, identifier),
        "case {case:?}: expected the service's own lookup message from {operation}, got {out:?}"
    );
}

/// The id and the service-issued ARN of the first benefit of `catalog`, as the service
/// itself reports them.
async fn first_benefit(harness: &Harness, catalog: &str) -> (String, String) {
    let summaries = harness.benefits(catalog).await;
    let summary = summaries
        .first()
        .unwrap_or_else(|| panic!("case {catalog:?}: the seeded catalog holds no benefit"));
    (text(catalog, summary, "Id"), text(catalog, summary, "Arn"))
}

/// The ids `ListBenefitApplications` reports for `body`, as a set.
async fn listed_applications(harness: &Harness, case: &str, body: &Value) -> BTreeSet<String> {
    let out = harness.ok(case, "ListBenefitApplications", body).await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {out:?}"))
        .iter()
        .map(|summary| text(case, summary, "Id"))
        .collect()
}

/// `GetBenefit` answers the ARN it issues for a benefit with that benefit, and an ARN
/// carrying any other partition, region or account with the benefit-lookup 404.
///
/// Rows: every seeded catalog (so the set cannot drift from the store), the bare id and
/// the issued ARN for the positive answer, and every spelling of [`foreign_fields`].
#[tokio::test]
async fn a_benefit_arn_the_service_did_not_issue_names_no_benefit() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        let (id, issued) = first_benefit(&harness, catalog).await;
        assert_eq!(
            issued,
            expected_arn(catalog, BENEFIT_RESOURCE, &id),
            "case {catalog:?}: the service must issue the benefit ARN of the region the \
             request addressed, got {issued:?}"
        );

        let body = |identifier: &str| json!({ "Catalog": catalog, "Identifier": identifier });
        let by_id = harness.ok(catalog, "GetBenefit", &body(&id)).await;
        let by_arn = harness.ok(catalog, "GetBenefit", &body(&issued)).await;
        assert_eq!(
            by_id, by_arn,
            "case {catalog:?}: the bare id and the issued ARN must name the same benefit, \
             got {by_id:?} and {by_arn:?}"
        );

        for (case, partition, region, account) in foreign_fields() {
            let identifier = arn_with(partition, region, account, catalog, BENEFIT_RESOURCE, &id);
            let case = format!("{catalog}: {case}");
            refused(
                &harness,
                &case,
                "GetBenefit",
                &body(&identifier),
                BENEFIT_KIND,
                &identifier,
            )
            .await;
        }
    }
}

/// The same rule on `GetBenefitAllocation`: the ARN the service issues for an allocation
/// names it, and one carrying any other partition, region or account names nothing.
///
/// Rows: every seeded catalog's first stored allocation, read straight from the store so
/// the row set cannot drift, times every spelling of [`foreign_fields`].
#[tokio::test]
async fn an_allocation_arn_the_service_did_not_issue_names_no_allocation() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        let stored = stored_allocations(&harness, catalog);
        let allocation = stored
            .first()
            .unwrap_or_else(|| panic!("case {catalog:?}: the seeded catalog holds no allocation"));
        let issued = expected_arn(catalog, ALLOCATION_RESOURCE, &allocation.id);

        let body = |identifier: &str| json!({ "Catalog": catalog, "Identifier": identifier });
        let by_id = harness
            .ok(catalog, "GetBenefitAllocation", &body(&allocation.id))
            .await;
        let by_arn = harness
            .ok(catalog, "GetBenefitAllocation", &body(&issued))
            .await;
        assert_eq!(
            by_id, by_arn,
            "case {catalog:?}: the bare id and the issued ARN must name the same allocation, \
             got {by_id:?} and {by_arn:?}"
        );

        for (case, partition, region, account) in foreign_fields() {
            let identifier = arn_with(
                partition,
                region,
                account,
                catalog,
                ALLOCATION_RESOURCE,
                &allocation.id,
            );
            let case = format!("{catalog}: {case}");
            refused(
                &harness,
                &case,
                "GetBenefitAllocation",
                &body(&identifier),
                ALLOCATION_KIND,
                &identifier,
            )
            .await;
        }
    }
}

/// The same rule on the third resource type, `GetBenefitApplication`: one identifier
/// family, one answer — the ARN the create minted names the application, and the same
/// spellings that name no benefit and no allocation name no application either.
#[tokio::test]
async fn an_application_arn_the_service_did_not_issue_names_no_application() {
    let harness = Harness::new();
    let (benefit_id, _) = first_benefit(&harness, CATALOG).await;
    let created = harness
        .create(
            "the subject application",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-arn-fields-application",
                "BenefitIdentifier": benefit_id,
                "Name": "an application the ARN-field rule is asserted on",
            }),
        )
        .await;
    let id = text("the subject application", &created, "Id");
    let minted = text("the subject application", &created, "Arn");
    assert_eq!(
        minted,
        expected_arn(CATALOG, APPLICATION_RESOURCE, &id),
        "the create must mint the application ARN of the region it was signed for, got \
         {minted:?}"
    );

    let by_id = harness.get_in("the bare id", CATALOG, &id).await;
    let by_arn = harness.get_in("the minted ARN", CATALOG, &minted).await;
    assert_eq!(
        by_id, by_arn,
        "the bare id and the minted ARN must name the same application, got {by_id:?} and \
         {by_arn:?}"
    );

    for (case, partition, region, account) in foreign_fields() {
        let identifier = arn_with(
            partition,
            region,
            account,
            CATALOG,
            APPLICATION_RESOURCE,
            &id,
        );
        refused(
            &harness,
            case,
            "GetBenefitApplication",
            &json!({ "Catalog": CATALOG, "Identifier": identifier }),
            APPLICATION_KIND,
            &identifier,
        )
        .await;
    }
}

/// `CreateBenefitApplication`'s `BenefitIdentifier` is the same member family, so a create
/// naming a benefit by an ARN this service did not issue is refused the benefit-lookup 404
/// and mints nothing — while the issued ARN creates an application against that benefit,
/// read back through `GetBenefitApplication`.
#[tokio::test]
async fn a_create_naming_a_benefit_arn_the_service_did_not_issue_mints_nothing() {
    let harness = Harness::new();
    let (benefit_id, issued) = first_benefit(&harness, CATALOG).await;
    let whole = json!({ "Catalog": CATALOG });

    let created = harness
        .create(
            "the issued benefit ARN",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-arn-fields-create",
                "BenefitIdentifier": issued,
                "Name": "an application created against the issued benefit ARN",
            }),
        )
        .await;
    let application_id = text("the issued benefit ARN", &created, "Id");
    let stored = harness
        .get_in("the issued benefit ARN", CATALOG, &application_id)
        .await;
    assert_eq!(
        text("the issued benefit ARN", &stored, "BenefitId"),
        benefit_id,
        "the create must apply for the benefit the issued ARN names, got {stored:?}"
    );

    let before = listed_applications(&harness, "before the refused creates", &whole).await;
    for (index, (case, partition, region, account)) in foreign_fields().into_iter().enumerate() {
        let identifier = arn_with(
            partition,
            region,
            account,
            CATALOG,
            BENEFIT_RESOURCE,
            &benefit_id,
        );
        refused(
            &harness,
            case,
            "CreateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": format!("tok-arn-fields-refused-{index}"),
                "BenefitIdentifier": identifier,
                "Name": "an application no refused create may mint",
            }),
            BENEFIT_KIND,
            &identifier,
        )
        .await;
    }
    let after = listed_applications(&harness, "after the refused creates", &whole).await;
    assert_eq!(
        after, before,
        "a refused create must mint nothing, got {after:?} where {before:?} was stored"
    );
}

/// The `BenefitIdentifiers` filters of both listings resolve their entries by the same
/// rule: the issued benefit ARN selects exactly what the bare id selects, and an ARN of
/// any other partition, region or account resolves to no benefit, so the filter the caller
/// supplied selects nothing.
#[tokio::test]
async fn a_benefit_identifiers_filter_resolves_only_the_issued_arn() {
    let harness = Harness::new();
    let (benefit_id, issued) = first_benefit(&harness, CATALOG).await;
    harness
        .create(
            "the filtered application",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-arn-fields-filter",
                "BenefitIdentifier": benefit_id,
                "Name": "an application the filter rule is asserted on",
            }),
        )
        .await;

    let application_body =
        |identifier: &str| json!({ "Catalog": CATALOG, "BenefitIdentifiers": [identifier] });
    let by_id = listed_applications(
        &harness,
        "applications by the bare id",
        &application_body(&benefit_id),
    )
    .await;
    assert!(
        !by_id.is_empty(),
        "the bare id must select the application created for that benefit, got {by_id:?}"
    );
    let by_arn = listed_applications(
        &harness,
        "applications by the issued ARN",
        &application_body(&issued),
    )
    .await;
    assert_eq!(
        by_arn, by_id,
        "the issued ARN must select what the bare id selects, got {by_arn:?} and {by_id:?}"
    );

    let (summaries, _) = list(
        &harness,
        "allocations by the bare id",
        &json!({ "Catalog": CATALOG, "BenefitIdentifiers": [benefit_id] }),
    )
    .await;
    let allocations_by_id = ids(&summaries);
    let (summaries, _) = list(
        &harness,
        "allocations by the issued ARN",
        &json!({ "Catalog": CATALOG, "BenefitIdentifiers": [issued] }),
    )
    .await;
    assert_eq!(
        ids(&summaries),
        allocations_by_id,
        "the issued ARN must select the allocations the bare id selects, got {summaries:?}"
    );

    for (case, partition, region, account) in foreign_fields() {
        let identifier = arn_with(
            partition,
            region,
            account,
            CATALOG,
            BENEFIT_RESOURCE,
            &benefit_id,
        );
        let selected = listed_applications(&harness, case, &application_body(&identifier)).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: a filter naming an ARN this service did not issue must select no \
             application, got {selected:?}"
        );
        let (summaries, token) = list(
            &harness,
            case,
            &json!({ "Catalog": CATALOG, "BenefitIdentifiers": [identifier] }),
        )
        .await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case {case:?}: a filter naming an ARN this service did not issue must select no \
             allocation, got {summaries:?} and {token:?}"
        );
    }
}
