// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for linking ONE package to a domain: `AssociatePackage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * one call makes ONE association with the package it names, carrying that
//!   package's currently available version, over a table of six calls spanning every
//!   `PackageType` value, no prerequisites, one and two prerequisites the domain
//!   already has, and the association configuration present (enabled and disabled)
//!   and absent;
//! * the association is made in a transient status and the clock owes it the status
//!   putting the package on the domain leaves it in: every row is `ASSOCIATING` the
//!   instant the call returns and `ACTIVE` once the span has passed, without any call
//!   having asked for the move;
//! * a prerequisite of a single-package call is satisfied ONLY by an association the
//!   domain already has, over the four ways one can be named;
//! * a request that cannot be served associates nothing, over the five ways it cannot
//!   be served, each with the status and error code the model declares;
//! * an associate call is refused while another change to one of the domain's
//!   associations has not landed yet — for the same package and for any other — and
//!   re-associating a settled one moves the domain onto the version the package has
//!   available then;
//! * this operation and the one that links several packages at once make the SAME
//!   record for the same package, because both go through the store's one decision.
//!
//! The domain's associations are read back through the store the handler writes —
//! reached exactly as a handler reaches it — because `ListPackagesForDomain`, the
//! wire read path for them, is still a scaffold stub; every rule that a call CAN
//! observe is asserted over the wire.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING, PACKAGE_TYPE_TXT_DICTIONARY,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{
    available_packages, create_package, member, stored_status, KEY_ACCESS_ROLE_ARN, PACKAGE_TYPES,
    UNKNOWN_PACKAGE_ID,
};

/// `AssociatePackage`: `POST /2021-01-01/packages/associate/{PackageID}/{DomainName}`,
/// with `PrerequisitePackageIDList` and `AssociationConfiguration` in the body.
async fn associate(emulator: &Emulator, domain: &str, package_id: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associate/{package_id}/{domain}"),
            Some(body),
        )
        .await
}

/// The `DomainPackageDetails` of an accepted call, with the case named on failure.
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

/// One `AssociatePackage` call to make: which created package it links, which
/// created packages must already be on the domain, and the `AssociationConfiguration`
/// it sends.
struct Row {
    case: &'static str,
    domain: &'static str,
    package: usize,
    prerequisites: Vec<usize>,
    configuration: Option<Value>,
}

/// The association table: one row per way an accepted call differs from another — a
/// package of each of the four types, no prerequisites, one and two the domain
/// already has, and the configuration container with key store access enabled,
/// disabled and absent.
///
/// The package indexes are [`PACKAGE_TYPES`] indexes, whose set
/// `super::package_associations::every_package_type_is_covered` pins against the
/// model's own `PackageType` values.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a dictionary package, no prerequisites",
            domain: "one-dictionary",
            package: 0,
            prerequisites: Vec::new(),
            configuration: None,
        },
        Row {
            case: "a plugin package with key store access enabled",
            domain: "one-key-store",
            package: 1,
            prerequisites: Vec::new(),
            configuration: Some(json!({
                "KeyStoreAccessOption": {
                    "KeyStoreAccessEnabled": true,
                    "KeyAccessRoleArn": KEY_ACCESS_ROLE_ARN,
                },
            })),
        },
        Row {
            case: "a config package with key store access disabled",
            domain: "one-no-key-store",
            package: 2,
            prerequisites: Vec::new(),
            configuration: Some(
                json!({ "KeyStoreAccessOption": { "KeyStoreAccessEnabled": false } }),
            ),
        },
        Row {
            case: "a license package with one prerequisite the domain already has",
            domain: "one-prerequisite",
            package: 3,
            prerequisites: vec![0],
            configuration: None,
        },
        Row {
            case: "a dictionary package with two prerequisites the domain already has",
            domain: "two-prerequisites",
            package: 0,
            prerequisites: vec![1, 2],
            configuration: None,
        },
        Row {
            case: "a plugin package with a prerequisite and a configuration",
            domain: "one-both",
            package: 1,
            prerequisites: vec![3],
            configuration: Some(json!({
                "KeyStoreAccessOption": {
                    "KeyStoreAccessEnabled": true,
                    "KeyAccessRoleArn": KEY_ACCESS_ROLE_ARN,
                },
            })),
        },
    ]
}

/// The body of one row's call, resolving each prerequisite index to the id the
/// create minted.
fn body(row: &Row, ids: &[String]) -> Value {
    let mut body = json!({});
    if !row.prerequisites.is_empty() {
        body["PrerequisitePackageIDList"] = json!(row
            .prerequisites
            .iter()
            .map(|index| ids[*index].clone())
            .collect::<Vec<String>>());
    }
    if let Some(configuration) = &row.configuration {
        body["AssociationConfiguration"] = configuration.clone();
    }
    body
}

/// Create the row's domain and put every package it names as a prerequisite on that
/// domain, through this same operation, each one settled before the next: the
/// service runs one change to a domain's associations at a time, which
/// [`a_change_in_flight_on_the_domain_is_not_overtaken`] asserts.
async fn seed(emulator: &Emulator, row: &Row, ids: &[String]) {
    create_baseline(emulator, row.domain).await;
    for index in &row.prerequisites {
        let answer = associate(emulator, row.domain, &ids[*index], json!({})).await;
        details(&answer, row.case);
        tokio::time::sleep(SETTLE_WAIT).await;
    }
}

#[tokio::test]
async fn the_named_package_is_associated_and_reported_from_the_stored_record() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    for row in rows() {
        seed(&emulator, &row, &ids).await;

        let answer = associate(&emulator, row.domain, &ids[row.package], body(&row, &ids)).await;
        let reported = details(&answer, row.case);
        let case = row.case;
        let package_id = &ids[row.package];
        let package_type = PACKAGE_TYPES[row.package];

        assert_eq!(
            member(&reported, "PackageID", case),
            *package_id,
            "case {case}: the association names the package it links to, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "PackageName", case),
            format!("package-{}", row.package),
            "case {case}: the association reports the package's own name, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "PackageType", case),
            package_type,
            "case {case}: the association reports the package's own type, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "DomainName", case),
            row.domain,
            "case {case}: the association names the domain it links, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "DomainPackageStatus", case),
            DOMAIN_PACKAGE_STATUS_ASSOCIATING,
            "case {case}: a new association is being put in place, got {reported:?}"
        );
        // The packages were created once and never updated, so the version available
        // to associate is the first one.
        assert_eq!(
            member(&reported, "PackageVersion", case),
            "1",
            "case {case}: the association carries the package's available version, \
             got {reported:?}"
        );
        assert!(
            reported["LastUpdated"].is_number(),
            "case {case}: expected a LastUpdated timestamp, got {reported:?}"
        );
        assert!(
            reported["ErrorDetails"].is_null(),
            "case {case}: an association that is not in an error state reports no \
             ErrorDetails, got {reported:?}"
        );

        // "The relative path of the package on the OpenSearch Service cluster nodes.
        // This is `synonym_path` when the package is for synonym files": dictionary
        // content is the kind an index setting names by path.
        if package_type == PACKAGE_TYPE_TXT_DICTIONARY {
            assert_eq!(
                member(&reported, "ReferencePath", case),
                format!("analyzers/{package_id}"),
                "case {case}: a dictionary package is referenced by path, got {reported:?}"
            );
        } else {
            assert!(
                reported["ReferencePath"].is_null(),
                "case {case}: content loaded onto the nodes has no reference path, \
                 got {reported:?}"
            );
        }

        let prerequisites: Vec<String> = row
            .prerequisites
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        if prerequisites.is_empty() {
            assert!(
                reported["PrerequisitePackageIDList"].is_null(),
                "case {case}: an association with no prerequisites reports no list, \
                 got {reported:?}"
            );
        } else {
            assert_eq!(
                reported["PrerequisitePackageIDList"],
                json!(prerequisites),
                "case {case}: the association reports the prerequisites it was given, \
                 got {reported:?}"
            );
        }

        match &row.configuration {
            None => assert!(
                reported["AssociationConfiguration"].is_null(),
                "case {case}: a call carrying no configuration stores none, got {reported:?}"
            ),
            Some(configuration) => assert_eq!(
                reported["AssociationConfiguration"], *configuration,
                "case {case}: the association reports the configuration it was given, \
                 got {reported:?}"
            ),
        }

        assert_eq!(
            stored_status(&emulator, row.domain, package_id).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ASSOCIATING),
            "case {case}: the association the call answered with is the one persisted"
        );
    }
}

#[tokio::test]
async fn the_association_settles_from_associating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    let table = rows();
    for row in &table {
        seed(&emulator, row, &ids).await;
        let answer = associate(&emulator, row.domain, &ids[row.package], body(row, &ids)).await;
        details(&answer, row.case);
    }

    // Nothing below asks for the move: the span the association was made with falls
    // due on its own, and the next read materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for row in &table {
        assert_eq!(
            stored_status(&emulator, row.domain, &ids[row.package]).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
            "case {}: the association the clock owed is in place once its span passed",
            row.case
        );
    }
}

#[tokio::test]
async fn a_prerequisite_is_satisfied_only_by_an_association_the_domain_already_has() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // One row per way a prerequisite can be named: already on the domain, a stored
    // package the domain does not have, a package nothing created, and the
    // associating package itself. A call that links a SINGLE package associates
    // nothing else, so nothing but an association the domain already has can satisfy
    // one. `ResourceNotFoundException` is modelled at HTTP 409 for this operation,
    // `ValidationException` at 400.
    let table: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a prerequisite the domain already has",
            json!([ids[0]]),
            200,
            "",
        ),
        (
            "a stored package the domain is not given",
            json!([ids[3]]),
            400,
            "ValidationException",
        ),
        (
            "a package nothing created",
            json!([UNKNOWN_PACKAGE_ID]),
            409,
            "ResourceNotFoundException",
        ),
        (
            "the associating package itself",
            json!([ids[1]]),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, prerequisites, status, error)) in table.into_iter().enumerate() {
        // Each row is tried on its own domain, so one row's writes are not another's
        // precondition. Every domain starts with package 0 associated by a call of its
        // own, which is what the "already has" row is satisfied by.
        let domain = format!("single-pre-{index}");
        create_baseline(&emulator, &domain).await;
        let seeded = associate(&emulator, &domain, &ids[0], json!({})).await;
        details(&seeded, case);
        tokio::time::sleep(SETTLE_WAIT).await;

        let answer = associate(
            &emulator,
            &domain,
            &ids[1],
            json!({ "PrerequisitePackageIDList": prerequisites }),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 200 {
            let reported = details(&answer, case);
            assert_eq!(
                reported["PrerequisitePackageIDList"], prerequisites,
                "case {case}: the association reports the prerequisites it was given, \
                 got {reported:?}"
            );
            continue;
        }
        assert_eq!(
            error_code(&answer),
            error,
            "case {case}: expected {error}, got {:?}",
            answer.body
        );
        // A refused request associates nothing: the only association the domain has is
        // the one the seeding call made.
        assert_eq!(
            stored_status(&emulator, &domain, &ids[1]),
            None,
            "case {case}: a refused request leaves the domain's associations as they were"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_associates_nothing() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "single-refusals";
    create_baseline(&emulator, domain).await;

    // A package whose content is still being copied: the create's copy span has not
    // passed, so there is no version of it to put on a domain.
    let copying = create_package(&emulator, "still-copying", PACKAGE_TYPES[0]).await;

    // One row per way a request cannot be served, each with the status and error code
    // the model declares for it: `ResourceNotFoundException` at HTTP 409,
    // `ConflictException` at 409 and `ValidationException` at 400.
    let table: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        (
            "a domain nothing created",
            "single-no-such-domain",
            &ids[0],
            json!({}),
            409,
            "ResourceNotFoundException",
        ),
        (
            "a package nothing created",
            domain,
            UNKNOWN_PACKAGE_ID,
            json!({}),
            409,
            "ResourceNotFoundException",
        ),
        (
            "a package whose content is still being copied",
            domain,
            &copying,
            json!({}),
            409,
            "ConflictException",
        ),
        (
            "a prerequisite the domain does not have",
            domain,
            &ids[0],
            json!({ "PrerequisitePackageIDList": [ids[1]] }),
            400,
            "ValidationException",
        ),
        (
            "a prerequisite that is the package itself",
            domain,
            &ids[0],
            json!({ "PrerequisitePackageIDList": [ids[0]] }),
            400,
            "ValidationException",
        ),
    ];

    for (case, row_domain, package_id, body, status, error) in table {
        let answer = associate(&emulator, row_domain, package_id, body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            error,
            "case {case}: expected {error}, got {:?}",
            answer.body
        );
        for (index, id) in ids.iter().chain(std::iter::once(&copying)).enumerate() {
            assert_eq!(
                stored_status(&emulator, domain, id),
                None,
                "case {case}: no association was made for package {index}"
            );
        }
    }

    // The same call for the copied package is served once its copy span has passed,
    // so the refusal above is the package's status rather than the call.
    tokio::time::sleep(SETTLE_WAIT).await;
    let accepted = associate(&emulator, domain, &copying, json!({})).await;
    let reported = details(&accepted, "a package whose copy has finished");
    assert_eq!(
        member(&reported, "PackageID", "a package whose copy has finished"),
        copying,
        "case a package whose copy has finished: the call associates the package it \
         named, got {reported:?}"
    );
}

#[tokio::test]
async fn a_change_in_flight_on_the_domain_is_not_overtaken_and_a_settled_one_takes_the_current_version(
) {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "single-reassociate";
    create_baseline(&emulator, domain).await;

    let first = associate(&emulator, domain, &ids[0], json!({})).await;
    let reported = details(&first, "AssociatePackage(first)");
    assert_eq!(
        member(&reported, "PackageVersion", "AssociatePackage(first)"),
        "1",
        "case AssociatePackage(first): the first association takes version one, got {reported:?}"
    );

    // The change the first call started has not landed yet, so the service runs no
    // second one on this domain: neither a call for the same package nor one for any
    // other package of the account.
    for (case, package) in [
        ("the same package", &ids[0]),
        ("another package", &ids[1]),
        ("a third package", &ids[2]),
    ] {
        let overtaking = associate(&emulator, domain, package, json!({})).await;
        assert_eq!(
            overtaking.status, 409,
            "case AssociatePackage(in flight / {case}): expected 409, got {} with {:?}",
            overtaking.status, overtaking.body
        );
        assert_eq!(
            error_code(&overtaking),
            "ConflictException",
            "case AssociatePackage(in flight / {case}): expected ConflictException, got {:?}",
            overtaking.body
        );
        assert_eq!(
            stored_status(&emulator, domain, package).as_deref(),
            (package == &ids[0]).then_some(DOMAIN_PACKAGE_STATUS_ASSOCIATING),
            "case AssociatePackage(in flight / {case}): the refused call changed nothing"
        );
    }

    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        stored_status(&emulator, domain, &ids[0]).as_deref(),
        Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
        "case AssociatePackage(settled): the association landed on the clock"
    );

    // A newer version of the package's content: `UpdatePackage`, the operation that
    // adds one, is still a stub, so the store is put into that state the way a handler
    // reaches it.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let package = guard
            .packages
            .get_mut(&ids[0])
            .expect("the created package is in the store");
        package.version = 2;
    }

    // Re-associating a settled association is what moves the domain onto the version
    // the package has available now.
    let again = associate(&emulator, domain, &ids[0], json!({})).await;
    let reported = details(&again, "AssociatePackage(again)");
    assert_eq!(
        member(&reported, "PackageVersion", "AssociatePackage(again)"),
        "2",
        "case AssociatePackage(again): a re-association takes the version available now, \
         got {reported:?}"
    );
    assert_eq!(
        member(&reported, "DomainPackageStatus", "AssociatePackage(again)"),
        DOMAIN_PACKAGE_STATUS_ASSOCIATING,
        "case AssociatePackage(again): the newer content is being put in place, got {reported:?}"
    );
}

#[tokio::test]
async fn linking_one_package_and_linking_several_make_the_same_record() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // The same package, with the same prerequisites and configuration, linked to two
    // domains: once by the single-package operation and once by the batch one. Both
    // go through the store's one decision, so the two records differ only in the
    // domain they name and the instant they were made.
    for (index, package_id) in ids.iter().enumerate() {
        let case = format!("package {index}");
        let single = format!("agree-single-{index}");
        let batch = format!("agree-batch-{index}");
        create_baseline(&emulator, &single).await;
        create_baseline(&emulator, &batch).await;

        let configuration = json!({
            "KeyStoreAccessOption": {
                "KeyStoreAccessEnabled": true,
                "KeyAccessRoleArn": KEY_ACCESS_ROLE_ARN,
            },
        });
        let one = associate(
            &emulator,
            &single,
            package_id,
            json!({ "AssociationConfiguration": configuration }),
        )
        .await;
        let mut one = details(&one, &case);

        let many = emulator
            .call(
                "POST",
                &format!("{PREFIX}/packages/associateMultiple"),
                Some(json!({
                    "DomainName": batch,
                    "PackageList": [{
                        "PackageID": package_id,
                        "AssociationConfiguration": configuration,
                    }],
                })),
            )
            .await;
        assert_eq!(
            many.status, 200,
            "case {case}: expected 200 from AssociatePackages, got {} with {:?}",
            many.status, many.body
        );
        let mut several = many.body["DomainPackageDetailsList"][0].clone();

        for record in [&mut one, &mut several] {
            // The domain each record names is the domain its call named, and the
            // instant it was made is the instant of that call: everything else is the
            // same association of the same package.
            record["DomainName"] = Value::Null;
            record["LastUpdated"] = Value::Null;
        }
        assert_eq!(
            one, several,
            "case {case}: both operations report the same association for the same \
             package, got {one:?} and {several:?}"
        );
    }
}
