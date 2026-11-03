// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `DissociatePackages` call does to the domain's OTHER
//! associations: the call acts on exactly the set of packages it names, and the clock
//! then owes every one of those removals and none of the rest.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the package helpers from [`super::package_associations`] and the call, read and
//! precondition helpers from [`super::package_dissociation`] — including that suite's
//! `with_associations`, driven here with the whole set of packages — so this suite holds
//! no second copy of any of them; the record a dissociation reports member for member,
//! the refusals the call answers and the serialization of a second change to one domain
//! are that suite's.
//!
//! The rules asserted here, over a table whose rows differ in HOW MANY of the domain's
//! associations one call names, WHICH of them and in what ORDER the request lists them:
//!
//! * one call reports exactly one record per package it named, and that set of records
//!   names exactly the packages the request named;
//! * the associations the call did NOT name are untouched — still in place and still
//!   reported by the domain's listing — both while the removals are in flight and once
//!   they have landed;
//! * the clock owes EVERY removal the one call asked for, so once the span has passed
//!   the domain's listing reports exactly the associations the call left alone;
//! * the identical call made again is then refused, because the links it names are gone,
//!   and that refusal leaves the remaining associations as they are.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_DISSOCIATING,
};

use super::domain_lifecycle::{error_code, Emulator, SETTLE_WAIT};
use super::package_associations::{available_packages, member, stored_status};
use super::package_dissociation::{dissociate_many, packages_of_domain, with_associations};

/// One `DissociatePackages` call to make: the domain it names, and which of the four
/// packages the domain holds an association with it names, in request order.
struct Row {
    case: &'static str,
    domain: &'static str,
    named: Vec<usize>,
}

/// The table: a call naming one association, two, three and all four, named in
/// ascending, descending and interleaved order, so what a row varies is the SIZE of the
/// set one call acts on, WHICH members it holds and the ORDER the request lists them in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one of four",
            domain: "dissoc-set-one",
            named: vec![2],
        },
        Row {
            case: "two of four, ascending",
            domain: "dissoc-set-two-asc",
            named: vec![0, 3],
        },
        Row {
            case: "two of four, descending",
            domain: "dissoc-set-two-desc",
            named: vec![3, 0],
        },
        Row {
            case: "three of four, interleaved",
            domain: "dissoc-set-three",
            named: vec![1, 3, 0],
        },
        Row {
            case: "all four, descending",
            domain: "dissoc-set-all",
            named: vec![3, 2, 1, 0],
        },
    ]
}

/// The `PackageID`s the domain's listing reports, as a set: what the domain holds, read
/// over the wire.
async fn listed_ids(emulator: &Emulator, domain: &str, case: &str) -> BTreeSet<String> {
    packages_of_domain(emulator, domain, case)
        .await
        .iter()
        .map(|details| member(details, "PackageID", case))
        .collect()
}

#[tokio::test]
async fn a_call_acts_on_exactly_the_packages_it_names_and_the_clock_owes_every_one() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // Every row's domain holds an association with EVERY created package, so what the
    // row varies is only the set one call names out of them. The precondition is the
    // sibling suite's, driven with the whole set.
    let every_package: Vec<usize> = (0..ids.len()).collect();
    for row in rows() {
        with_associations(&emulator, row.domain, &ids, &every_package).await;
        let named: Vec<String> = row.named.iter().map(|index| ids[*index].clone()).collect();
        let named_set: BTreeSet<String> = named.iter().cloned().collect();
        let left_set: BTreeSet<String> = ids
            .iter()
            .filter(|package_id| !named_set.contains(*package_id))
            .cloned()
            .collect();

        let body = json!({ "DomainName": row.domain, "PackageList": named });
        let answer = dissociate_many(&emulator, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let reported = answer.body["DomainPackageDetailsList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected a DomainPackageDetailsList, got {:?}",
                    row.case, answer.body
                )
            })
            .clone();

        // One record per package the call named, and the records name exactly those
        // packages: a call acts on the set it names, neither collapsing it nor reaching
        // past it.
        assert_eq!(
            reported.len(),
            named_set.len(),
            "case {}: one record per package the call named, got {reported:?}",
            row.case
        );
        let reported_set: BTreeSet<String> = reported
            .iter()
            .map(|details| member(details, "PackageID", row.case))
            .collect();
        assert_eq!(
            reported_set, named_set,
            "case {}: the records name exactly the packages the request named, got \
             {reported_set:?}",
            row.case
        );
        for details in &reported {
            assert_eq!(
                member(details, "DomainName", row.case),
                row.domain,
                "case {}: the record names the domain the call acted on, got {details:?}",
                row.case
            );
            assert_eq!(
                member(details, "DomainPackageStatus", row.case),
                DOMAIN_PACKAGE_STATUS_DISSOCIATING,
                "case {}: the package is being taken off the domain, got {details:?}",
                row.case
            );
        }

        // While the removals are in flight the domain still holds every association: the
        // ones the call named, in their removal status, and the ones it left alone, in
        // the status they were already in.
        for package_id in &named_set {
            assert_eq!(
                stored_status(&emulator, row.domain, package_id).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_DISSOCIATING),
                "case {}: the call left package {package_id} being taken off",
                row.case
            );
        }
        for package_id in &left_set {
            assert_eq!(
                stored_status(&emulator, row.domain, package_id).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
                "case {}: the association with package {package_id} was not named and is as \
                 it was",
                row.case
            );
        }

        // The clock owes EVERY removal the one call asked for: once the span has passed
        // the domain's listing reports exactly the associations the call left alone,
        // without any call having asked for the moves.
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_eq!(
            listed_ids(&emulator, row.domain, row.case).await,
            left_set,
            "case {}: the domain holds exactly the associations the call did not name",
            row.case
        );

        // The links the call named are gone, so the IDENTICAL call is now refused — and
        // that refusal takes nothing else off the domain.
        let again = dissociate_many(&emulator, body).await;
        assert_eq!(
            again.status, 400,
            "case {}(again): expected 400, got {} with {:?}",
            row.case, again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ValidationException",
            "case {}(again): expected the modelled validation refusal, got {:?}",
            row.case, again.body
        );
        assert_eq!(
            listed_ids(&emulator, row.domain, row.case).await,
            left_set,
            "case {}(again): a refused request dissociates nothing",
            row.case
        );
    }
}
