// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for taking ONE package off a domain: `DissociatePackage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. Every call path, store read and table of
//! schemas comes from the sibling suites — [`super::domain_lifecycle`],
//! [`super::package_associations`], [`super::package_dissociation`],
//! [`super::package_dependency_lifecycle`], [`super::index_registry`] and
//! [`super::index_deletion`] — so this suite holds no second copy of any of them.
//!
//! The rules asserted here are the ones this wire form alone had no case for:
//!
//! * "The package can't be in use with any OpenSearch index for the dissociation to
//!   succeed": a call naming a package whose files an index of the domain resolves is
//!   accepted and leaves the association in its removal status, the clock then leaves
//!   it in `DISSOCIATION_FAILED` with the service's in-use error details and the
//!   association in place, and the IDENTICAL call takes the package off once no index
//!   resolves it — over the table of ways a schema can name a package's file, and
//!   over a control row whose domain holds no index at all;
//! * "The package is still available in OpenSearch Service for association later":
//!   the package keeps its own record and can be put back on the domain;
//! * this operation and the one that unlinks several packages at once report the SAME
//!   record for the same package, because both go through the store's one removal
//!   decision — over every `PackageType` value.
//!
//! The domain's associations are read back over the wire (`ListPackagesForDomain`),
//! and the store (reached exactly as a handler reaches it) is used only where a case
//! needs the record a status move has not reached yet.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING,
    DOMAIN_PACKAGE_STATUS_DISSOCIATING, DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED, PACKAGE_IN_USE,
};

use super::domain_lifecycle::{Answer, Emulator, SETTLE_WAIT};
use super::index_deletion::deleted_index;
use super::index_registry::created_index;
use super::package_associations::{available_packages, member, stored_status, PACKAGE_TYPES};
use super::package_dependency_lifecycle::{associated, domain_and_package, listed, use_rows};
use super::package_dissociation::{
    associate, dissociate_many, dissociate_one, listed_status, with_associations,
};

/// One dependency a removal is judged against: the index the domain holds, if any,
/// and the schema that names the package's file. The schemas are the sibling suite's
/// table, so the rows differ along the way a schema addresses the file rather than
/// along one recorded setting.
struct Dependency {
    case: &'static str,
    index: Option<&'static str>,
    schema: Option<fn(&str) -> Value>,
}

/// The `DomainPackageDetails` of an accepted single-package call, with the case named
/// on failure.
fn details(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let details = answer.body["DomainPackageDetails"].clone();
    assert!(
        details.is_object(),
        "case {case}: expected a DomainPackageDetails object, got {:?}",
        answer.body
    );
    details
}

#[tokio::test]
async fn a_package_an_index_resolves_is_not_taken_off_until_the_index_is_gone() {
    let emulator = Emulator::new();

    // The rows differ along the way the index's schema names the package's file; the
    // control row creates no index at all, so what the failed removal is attributed to
    // is the dependency rather than the call.
    let mut cases = vec![Dependency {
        case: "no index on the domain",
        index: None,
        schema: None,
    }];
    for row in use_rows() {
        cases.push(Dependency {
            case: row.case,
            index: Some(row.index),
            schema: Some(row.schema),
        });
    }

    for (position, Dependency { case, index, schema }) in cases.into_iter().enumerate() {
        let domain = format!("one-dissoc-use-{position}");
        let package_id =
            domain_and_package(&emulator, &domain, &format!("one-dissoc-package-{position}")).await;
        let path = associated(&emulator, &domain, &package_id, case).await;
        if let (Some(index), Some(schema)) = (index, schema) {
            created_index(&emulator, &domain, index, &schema(&path), case).await;
        }

        // The call is accepted SYNCHRONOUSLY and reports the association on its way
        // off the domain: whether the removal can happen is not this answer's to give.
        let answer = dissociate_one(&emulator, &domain, &package_id).await;
        let reported = details(&answer, case);
        assert_eq!(
            member(&reported, "PackageID", case),
            package_id,
            "case {case}: the record names the package the call unlinked, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "DomainName", case),
            domain,
            "case {case}: the record names the domain the call unlinked it from, got \
             {reported:?}"
        );
        assert_eq!(
            member(&reported, "DomainPackageStatus", case),
            DOMAIN_PACKAGE_STATUS_DISSOCIATING,
            "case {case}: expected the association in its removal status, got {reported:?}"
        );
        assert!(
            reported["ErrorDetails"].is_null(),
            "case {case}: the removal has not failed yet, got {reported:?}"
        );

        // Nothing below asks for the outcome: the clock owes it, and which outcome it
        // is was decided by the indexes the domain holds when it fell due.
        tokio::time::sleep(SETTLE_WAIT).await;
        match index {
            None => {
                assert_eq!(
                    listed_status(&emulator, &domain, &package_id, case).await,
                    None,
                    "case {case}: a package no index resolves comes off the domain"
                );
            }
            Some(index) => {
                let entry = listed(&emulator, &domain, &package_id, case)
                    .await
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: expected the association to survive the failed removal"
                        )
                    });
                let status = member(&entry, "DomainPackageStatus", case);
                assert_eq!(
                    status, DOMAIN_PACKAGE_STATUS_DISSOCIATION_FAILED,
                    "case {case}: expected the failed-removal status, got {status:?}"
                );
                let message = member(&entry["ErrorDetails"], "ErrorMessage", case);
                assert_eq!(
                    message, PACKAGE_IN_USE,
                    "case {case}: expected the service's in-use sentence, got {message:?}"
                );

                // Removing the ONE dependency lifts the block, and the identical call
                // then takes the package off.
                deleted_index(&emulator, &domain, index, case).await;
                let again = dissociate_one(&emulator, &domain, &package_id).await;
                let reported = details(&again, case);
                assert_eq!(
                    member(&reported, "DomainPackageStatus", case),
                    DOMAIN_PACKAGE_STATUS_DISSOCIATING,
                    "case {case}: the identical call is served once nothing uses the package, \
                     got {reported:?}"
                );
                tokio::time::sleep(SETTLE_WAIT).await;
                assert_eq!(
                    listed_status(&emulator, &domain, &package_id, case).await,
                    None,
                    "case {case}: the association is gone once no index resolved the package"
                );
            }
        }

        // "The package is still available in OpenSearch Service for association
        // later": the removal of a link to it left the package itself alone.
        let reassociated = associate(&emulator, &domain, std::slice::from_ref(&package_id)).await;
        assert_eq!(
            reassociated.status, 200,
            "case {case}(reassociate): expected 200, got {} with {:?}",
            reassociated.status, reassociated.body
        );
        assert_eq!(
            stored_status(&emulator, &domain, &package_id).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ASSOCIATING),
            "case {case}(reassociate): the package is being put back on the domain"
        );
    }
}

#[tokio::test]
async fn unlinking_one_package_and_unlinking_several_report_the_same_record() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // The same package, settled on two domains, taken off once by the single-package
    // operation and once by the batch one. Both go through the store's one removal
    // decision, so the two records differ only in the domain they name and the instant
    // they were made.
    for (index, package_id) in ids.iter().enumerate() {
        let case = format!("package {index} ({})", PACKAGE_TYPES[index]);
        let single = format!("one-off-single-{index}");
        let batch = format!("one-off-batch-{index}");
        with_associations(&emulator, &single, &ids, &[index]).await;
        with_associations(&emulator, &batch, &ids, &[index]).await;

        let one = dissociate_one(&emulator, &single, package_id).await;
        let mut one = details(&one, &case);

        let many = dissociate_many(
            &emulator,
            json!({ "DomainName": batch, "PackageList": [package_id] }),
        )
        .await;
        assert_eq!(
            many.status, 200,
            "case {case}: expected 200 from DissociatePackages, got {} with {:?}",
            many.status, many.body
        );
        let mut several = many.body["DomainPackageDetailsList"][0].clone();

        for record in [&mut one, &mut several] {
            // The domain each record names is the domain its call named, and the
            // instant it was made is the instant of that call: everything else is the
            // same removal of the same package.
            record["DomainName"] = Value::Null;
            record["LastUpdated"] = Value::Null;
        }
        assert_eq!(
            one, several,
            "case {case}: both operations report the same removal for the same package, \
             got {one:?} and {several:?}"
        );

        // And both removals land the same way: each domain stops holding the package
        // once the span has passed, with no call having asked for the move.
        tokio::time::sleep(SETTLE_WAIT).await;
        for domain in [&single, &batch] {
            assert_eq!(
                stored_status(&emulator, domain, package_id),
                None,
                "case {case}: {domain} no longer holds the package the clock owed a removal"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_single_package_removal_leaves_every_association_in_place() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "one-off-refused";
    with_associations(&emulator, domain, &ids, &[0, 1]).await;

    // A removal is itself a change to the domain's associations: while it has not
    // landed, the modelled conflict refusal answers every further package call on that
    // domain, and nothing it names moves.
    let accepted = dissociate_one(&emulator, domain, &ids[0]).await;
    details(&accepted, "the first removal");
    for (case, package_id) in [
        ("the same package", &ids[0]),
        ("another package the domain holds", &ids[1]),
    ] {
        let refused = dissociate_one(&emulator, domain, package_id).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            stored_status(&emulator, domain, &ids[1]).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
            "case {case}: the association the refused call did not remove is as it was"
        );
    }

    // The change the refusals raced still lands on its own span, and the one it was
    // about is the only one that comes off.
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        stored_status(&emulator, domain, &ids[0]),
        None,
        "case the first removal: the package it named is off the domain"
    );
    assert_eq!(
        stored_status(&emulator, domain, &ids[1]).as_deref(),
        Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
        "case the first removal: no other association was touched"
    );
}
