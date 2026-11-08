// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for linking a package to a domain: `AssociatePackages`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * one call makes ONE association per package it names, carrying the package's
//!   currently available version, over a table of five calls spanning every
//!   `PackageType` value, batches of one, three and four packages, prerequisites
//!   taken from the same call and from an earlier one, and the association
//!   configuration present (enabled and disabled) and absent;
//! * an association is made in a transient status and the clock owes it the status
//!   putting the package on the domain leaves it in: every row is `ASSOCIATING` the
//!   instant the call returns and `ACTIVE` once the span has passed, without any
//!   call having asked for the move;
//! * a prerequisite is satisfied only by an association the domain already has or
//!   one the same call makes, over the five ways a prerequisite can be named;
//! * a package whose content is not in place yet cannot be associated, and the same
//!   call is served once the copy has left it available;
//! * an association with a change in flight is not overtaken, and re-associating a
//!   settled one moves the domain onto the version the package has available then;
//! * a refused request associates NOTHING: the domain's associations are exactly
//!   what they were before it.
//!
//! The domain's associations are read back both through the wire listing that reports
//! them (`ListPackagesForDomain`, asserted in
//! [`super::package_association_listings`]) and, where a case needs the record a
//! status move has not reached yet, through the store the handler writes — reached
//! exactly as a handler reaches it.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::PACKAGE_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING, PACKAGE_QUOTA,
    PACKAGE_TYPE_TXT_DICTIONARY,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// A role ARN for the key-store configuration: `RoleArn` is `@length(min: 20, max:
/// 2048)` with pattern `^arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b):iam::[0-9]+:role\/`.
pub(crate) const KEY_ACCESS_ROLE_ARN: &str =
    "arn:aws:iam::000000000000:role/opensearch-key-store";

/// A `PackageID` of the form the model's pattern admits
/// (`^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`) that no create in these tests minted.
pub(crate) const UNKNOWN_PACKAGE_ID: &str = "pkg-00000000000000000000000000000000";

/// A `PackageID` of the OTHER form that pattern admits: the form that addresses a
/// package the SERVICE itself publishes, which a create cannot mint, so this account
/// holds none of them.
pub(crate) const SERVICE_PACKAGE_ID: &str = "F123456789";

/// `AssociatePackages`: `POST /2021-01-01/packages/associateMultiple`.
async fn associate(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(body),
        )
        .await
}

/// The Amazon S3 bucket every package of this suite imports its content from, and the
/// key one package's content sits at.
///
/// Named here rather than inside the create below because a suite about what a package's
/// CONTENT does ([`super::domain_analysis`]) puts the object there first: the create
/// imports the object this names, so the two have to name one location.
pub(crate) const PACKAGE_BUCKET: &str = "my-package-bucket";

/// The key the package `name`'s content is imported from. See [`PACKAGE_BUCKET`].
pub(crate) fn package_key(name: &str) -> String {
    format!("packages/{name}")
}

/// `CreatePackage` for `name` with type `package_type`, answering with the
/// `PackageID` it minted.
pub(crate) async fn create_package(emulator: &Emulator, name: &str, package_type: &str) -> String {
    let case = format!("CreatePackage({name:?}, {package_type:?})");
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages"),
            Some(json!({
                "PackageName": name,
                "PackageType": package_type,
                "PackageSource": {
                    "S3BucketName": PACKAGE_BUCKET,
                    "S3Key": package_key(name),
                },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetails"]["PackageID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a PackageID, got {:?}", answer.body))
        .to_string()
}

/// The `DomainPackageDetailsList` of an accepted call, with the case named on
/// failure.
fn associated(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainPackageDetailsList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// A required string member of a `DomainPackageDetails`, with the case named on
/// failure.
pub(crate) fn member(details: &Value, name: &str, case: &str) -> String {
    details[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {name}, got {details:?}"))
        .to_string()
}

/// The `DomainPackageStatus` the store holds for `package_id` on `domain`, or `None`
/// where the domain has no association with that package at all.
///
/// Read through the store the handler writes, reached the way a handler reaches it: a
/// case that asserts what a REFUSED call left behind needs the absence of a record,
/// which a listing reports as an empty set either way. It passes the current instant,
/// so a status move the clock already owed is materialised by the read exactly as a
/// handler's read of it would be.
pub(crate) fn stored_status(emulator: &Emulator, domain: &str, package_id: &str) -> Option<String> {
    let store = emulator.store();
    let mut guard = store.lock();
    let now = SystemTime::now();
    let domain = guard.settled_mut(domain, now)?;
    domain
        .package_associations
        .get(package_id)
        .map(|association| association.status.clone())
}

/// One package to associate within a call: which created package, what it names as
/// its prerequisites, and the `AssociationConfiguration` to send.
struct Entry {
    package: usize,
    prerequisites: Vec<usize>,
    configuration: Option<Value>,
}

/// One `AssociatePackages` call to make: the domain it names and the packages it
/// associates, in request order.
struct Row {
    case: &'static str,
    domain: &'static str,
    entries: Vec<Entry>,
}

/// A package with no prerequisites and no configuration.
fn plain(package: usize) -> Entry {
    Entry {
        package,
        prerequisites: Vec::new(),
        configuration: None,
    }
}

/// The packages every table in this suite creates, in the order their ids are held:
/// one per `PackageType` value, so a rule asserted over the table is asserted over
/// every type a package can have. The value set is this service's Smithy model's
/// (`smithy_query(query="shape",shape="PackageType")`), and
/// [`every_package_type_is_covered`] pins it against `PACKAGE_TYPE_VALUES`.
pub(crate) const PACKAGE_TYPES: [&str; 4] = [
    "TXT-DICTIONARY",
    "ZIP-PLUGIN",
    "PACKAGE-CONFIG",
    "PACKAGE-LICENSE",
];

/// Create one AVAILABLE package per [`PACKAGE_TYPES`] entry and answer with their
/// ids in that order.
///
/// The wait is the copy span `CreatePackage` leaves a package in `COPYING` for: a
/// package can be put on a domain only once its content is in place, which
/// [`a_package_whose_content_is_not_in_place_yet_cannot_be_associated`] asserts.
pub(crate) async fn available_packages(emulator: &Emulator) -> Vec<String> {
    let mut ids = Vec::with_capacity(PACKAGE_TYPES.len());
    for (index, package_type) in PACKAGE_TYPES.iter().enumerate() {
        ids.push(create_package(emulator, &format!("package-{index}"), package_type).await);
    }
    tokio::time::sleep(SETTLE_WAIT).await;
    ids
}

/// The body of one row's call, resolving each entry's package index to the id the
/// create minted.
fn body(row: &Row, ids: &[String]) -> Value {
    let package_list: Vec<Value> = row
        .entries
        .iter()
        .map(|entry| {
            let mut element = json!({ "PackageID": ids[entry.package] });
            if !entry.prerequisites.is_empty() {
                element["PrerequisitePackageIDList"] = json!(entry
                    .prerequisites
                    .iter()
                    .map(|index| ids[*index].clone())
                    .collect::<Vec<String>>());
            }
            if let Some(configuration) = &entry.configuration {
                element["AssociationConfiguration"] = configuration.clone();
            }
            element
        })
        .collect();
    json!({ "DomainName": row.domain, "PackageList": package_list })
}

/// The association table: one row per way an accepted call differs from another —
/// a single package, a batch of three with a prerequisite taken from the same call,
/// a batch spanning every package type, and the configuration container with key
/// store access enabled, disabled and absent.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one dictionary package, no prerequisites",
            domain: "assoc-single",
            entries: vec![plain(0)],
        },
        Row {
            case: "one plugin package with key store access enabled",
            domain: "assoc-key-store",
            entries: vec![Entry {
                package: 1,
                prerequisites: Vec::new(),
                configuration: Some(json!({
                    "KeyStoreAccessOption": {
                        "KeyStoreAccessEnabled": true,
                        "KeyAccessRoleArn": KEY_ACCESS_ROLE_ARN,
                    },
                })),
            }],
        },
        Row {
            case: "one package with key store access disabled",
            domain: "assoc-no-key-store",
            entries: vec![Entry {
                package: 3,
                prerequisites: Vec::new(),
                configuration: Some(
                    json!({ "KeyStoreAccessOption": { "KeyStoreAccessEnabled": false } }),
                ),
            }],
        },
        Row {
            case: "a batch of three, one taking its prerequisite from the same call",
            domain: "assoc-batch-three",
            entries: vec![
                plain(0),
                Entry {
                    package: 1,
                    prerequisites: vec![0],
                    configuration: None,
                },
                plain(2),
            ],
        },
        Row {
            case: "a batch spanning every package type",
            domain: "assoc-every-type",
            entries: vec![plain(0), plain(1), plain(2), plain(3)],
        },
    ]
}

/// The table spans the whole `PackageType` catalog, so a value added to the model is
/// not silently left untested.
#[test]
fn every_package_type_is_covered() {
    for package_type in PACKAGE_TYPE_VALUES {
        assert!(
            PACKAGE_TYPES.contains(package_type),
            "case PackageType={package_type}: the association table covers every modelled \
             package type, got {PACKAGE_TYPES:?}"
        );
    }
}

#[tokio::test]
async fn every_named_package_is_associated_and_reported_from_the_stored_record() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    for row in rows() {
        create_baseline(&emulator, row.domain).await;
        let answer = associate(&emulator, body(&row, &ids)).await;
        let reported = associated(&answer, row.case);

        assert_eq!(
            reported.len(),
            row.entries.len(),
            "case {}: one association per package the call named, got {reported:?}",
            row.case
        );

        for (entry, details) in row.entries.iter().zip(&reported) {
            let case = format!("{} / package {}", row.case, entry.package);
            let package_id = &ids[entry.package];
            let package_type = PACKAGE_TYPES[entry.package];

            assert_eq!(
                member(details, "PackageID", &case),
                *package_id,
                "case {case}: the association names the package it links to, got {details:?}"
            );
            assert_eq!(
                member(details, "PackageName", &case),
                format!("package-{}", entry.package),
                "case {case}: the association reports the package's own name, got {details:?}"
            );
            assert_eq!(
                member(details, "PackageType", &case),
                package_type,
                "case {case}: the association reports the package's own type, got {details:?}"
            );
            assert_eq!(
                member(details, "DomainName", &case),
                row.domain,
                "case {case}: the association names the domain it links, got {details:?}"
            );
            assert_eq!(
                member(details, "DomainPackageStatus", &case),
                DOMAIN_PACKAGE_STATUS_ASSOCIATING,
                "case {case}: a new association is being put in place, got {details:?}"
            );
            // The packages were created once and never updated, so the version
            // available to associate is the first one.
            assert_eq!(
                member(details, "PackageVersion", &case),
                "1",
                "case {case}: the association carries the package's available version, \
                 got {details:?}"
            );
            assert!(
                details["LastUpdated"].is_number(),
                "case {case}: expected a LastUpdated timestamp, got {details:?}"
            );
            assert!(
                details["ErrorDetails"].is_null(),
                "case {case}: an association that is not in an error state reports no \
                 ErrorDetails, got {details:?}"
            );

            // "The relative path of the package on the OpenSearch Service cluster
            // nodes. This is `synonym_path` when the package is for synonym files":
            // dictionary content is the kind an index setting names by path.
            if package_type == PACKAGE_TYPE_TXT_DICTIONARY {
                assert_eq!(
                    member(details, "ReferencePath", &case),
                    format!("analyzers/{package_id}"),
                    "case {case}: a dictionary package is referenced by path, got {details:?}"
                );
            } else {
                assert!(
                    details["ReferencePath"].is_null(),
                    "case {case}: content loaded onto the nodes has no reference path, \
                     got {details:?}"
                );
            }

            let prerequisites: Vec<String> = entry
                .prerequisites
                .iter()
                .map(|index| ids[*index].clone())
                .collect();
            if prerequisites.is_empty() {
                assert!(
                    details["PrerequisitePackageIDList"].is_null(),
                    "case {case}: an association with no prerequisites reports no list, \
                     got {details:?}"
                );
            } else {
                assert_eq!(
                    details["PrerequisitePackageIDList"],
                    json!(prerequisites),
                    "case {case}: the association reports the prerequisites it was given, \
                     got {details:?}"
                );
            }

            match &entry.configuration {
                None => assert!(
                    details["AssociationConfiguration"].is_null(),
                    "case {case}: a call carrying no configuration stores none, got {details:?}"
                ),
                Some(configuration) => assert_eq!(
                    details["AssociationConfiguration"], *configuration,
                    "case {case}: the association reports the configuration it was given, \
                     got {details:?}"
                ),
            }

            assert_eq!(
                stored_status(&emulator, row.domain, package_id).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ASSOCIATING),
                "case {case}: the association the call answered with is the one persisted"
            );
        }
    }
}

#[tokio::test]
async fn an_association_settles_from_associating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    let table = rows();
    for row in &table {
        create_baseline(&emulator, row.domain).await;
        let answer = associate(&emulator, body(row, &ids)).await;
        associated(&answer, row.case);
    }

    // Nothing below asks for the move: the span the association was made with falls
    // due on its own, and the next read materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for row in &table {
        for entry in &row.entries {
            let case = format!("{} / package {}", row.case, entry.package);
            assert_eq!(
                stored_status(&emulator, row.domain, &ids[entry.package]).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
                "case {case}: the association the clock owed is in place once its span passed"
            );
        }
    }
}

#[tokio::test]
async fn a_prerequisite_is_satisfied_only_by_an_association_the_domain_has_or_the_call_makes() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // One row per way a prerequisite can be named: already on the domain, made by
    // the same call, a stored package the domain does not have, a package nothing
    // created, and the associating package itself. `ValidationException` is modelled at
    // HTTP 400 for this operation, and a prerequisite id that resolves to no package is
    // a bad input value like the rest.
    let table: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a prerequisite the domain already has",
            json!([{ "PackageID": ids[1], "PrerequisitePackageIDList": [ids[0]] }]),
            200,
            "",
        ),
        (
            "a prerequisite the same call associates",
            json!([
                { "PackageID": ids[2] },
                { "PackageID": ids[3], "PrerequisitePackageIDList": [ids[2]] },
            ]),
            200,
            "",
        ),
        (
            "a prerequisite the same call associates, named before it",
            json!([
                { "PackageID": ids[3], "PrerequisitePackageIDList": [ids[2]] },
                { "PackageID": ids[2] },
            ]),
            200,
            "",
        ),
        (
            "a stored package the domain is not given",
            json!([{ "PackageID": ids[1], "PrerequisitePackageIDList": [ids[3]] }]),
            400,
            "ValidationException",
        ),
        (
            "a package nothing created",
            json!([{ "PackageID": ids[1], "PrerequisitePackageIDList": [UNKNOWN_PACKAGE_ID] }]),
            400,
            "ValidationException",
        ),
        (
            "the associating package itself",
            json!([{ "PackageID": ids[1], "PrerequisitePackageIDList": [ids[1]] }]),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, package_list, status, error)) in table.into_iter().enumerate() {
        // Each row is tried on its own domain, so one row's writes are not another's
        // precondition. Every domain starts with package 0 associated by a call of
        // its own, which is what the "already has" row is satisfied by.
        let row_domain = format!("assoc-pre-{index}");
        create_baseline(&emulator, &row_domain).await;
        let seed = associate(
            &emulator,
            json!({ "DomainName": row_domain, "PackageList": [{ "PackageID": ids[0] }] }),
        )
        .await;
        associated(&seed, case);
        tokio::time::sleep(SETTLE_WAIT).await;

        let mut body = json!({ "DomainName": row_domain });
        body["PackageList"] = package_list.clone();
        let answer = associate(&emulator, body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 200 {
            let reported = associated(&answer, case);
            assert_eq!(
                reported.len(),
                package_list.as_array().map(Vec::len).unwrap_or_default(),
                "case {case}: one association per package the call named, got {reported:?}"
            );
            continue;
        }
        assert_eq!(
            error_code(&answer),
            error,
            "case {case}: expected {error}, got {:?}",
            answer.body
        );
        // A refused request associates nothing: the only association the domain has
        // is the one the seed call made.
        assert_eq!(
            stored_status(&emulator, &row_domain, &ids[1]),
            None,
            "case {case}: a refused request leaves the domain's associations as they were"
        );
    }
}

#[tokio::test]
async fn a_package_whose_content_is_not_in_place_yet_cannot_be_associated() {
    let emulator = Emulator::new();
    let domain = "assoc-copying";
    create_baseline(&emulator, domain).await;

    for package_type in PACKAGE_TYPES {
        let case = format!("AssociatePackages(COPYING {package_type})");
        let name = format!("copying-{}", package_type.to_lowercase());
        let package_id = create_package(&emulator, &name, package_type).await;
        let body = json!({
            "DomainName": domain,
            "PackageList": [{ "PackageID": package_id }],
        });

        // The content is still being copied the instant the create returns, so there
        // is no version of it to put on the domain.
        let refused = associate(&emulator, body.clone()).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case}: expected ConflictException, got {:?}",
            refused.body
        );
        assert_eq!(
            stored_status(&emulator, domain, &package_id),
            None,
            "case {case}: a refused request associates nothing"
        );

        // The same call, once the copy span has passed, is served.
        tokio::time::sleep(SETTLE_WAIT).await;
        let accepted = associate(&emulator, body).await;
        let reported = associated(&accepted, &case);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: the call associates the one package it named, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_association_in_flight_is_not_overtaken_and_a_settled_one_takes_the_current_version() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "assoc-reassociate";
    create_baseline(&emulator, domain).await;

    let body = json!({ "DomainName": domain, "PackageList": [{ "PackageID": ids[0] }] });
    let first = associate(&emulator, body.clone()).await;
    let reported = associated(&first, "AssociatePackages(first)");
    assert_eq!(
        member(&reported[0], "PackageVersion", "AssociatePackages(first)"),
        "1",
        "case AssociatePackages(first): the first association takes version one, got {reported:?}"
    );

    // The move the first call started has not landed yet, so a second call that
    // would change the same association is refused rather than overtaking it.
    let overtaking = associate(&emulator, body.clone()).await;
    assert_eq!(
        overtaking.status, 409,
        "case AssociatePackages(in flight): expected 409, got {} with {:?}",
        overtaking.status, overtaking.body
    );
    assert_eq!(
        error_code(&overtaking),
        "ConflictException",
        "case AssociatePackages(in flight): expected ConflictException, got {:?}",
        overtaking.body
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        stored_status(&emulator, domain, &ids[0]).as_deref(),
        Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
        "case AssociatePackages(settled): the association landed on the clock"
    );

    // A newer version of the package's content: `UpdatePackage`, the operation that
    // adds one, is still a stub, so the store is put into that state the way a
    // handler reaches it.
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
    let again = associate(&emulator, body).await;
    let reported = associated(&again, "AssociatePackages(again)");
    assert_eq!(
        member(&reported[0], "PackageVersion", "AssociatePackages(again)"),
        "2",
        "case AssociatePackages(again): a re-association takes the version available now, \
         got {reported:?}"
    );
    assert_eq!(
        member(
            &reported[0],
            "DomainPackageStatus",
            "AssociatePackages(again)"
        ),
        DOMAIN_PACKAGE_STATUS_ASSOCIATING,
        "case AssociatePackages(again): the newer content is being put in place, got {reported:?}"
    );
}

#[tokio::test]
async fn a_request_that_cannot_be_served_associates_nothing() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "assoc-refusals";
    create_baseline(&emulator, domain).await;

    // One row per way a request cannot be served, each with the status and error
    // code the model declares for it: `ValidationException` at HTTP 400. A `DomainName`
    // that names no domain and a `PackageID` that resolves to no package are both
    // CLIENT-INPUT refusals, exactly as they are for a call that links one package:
    // linking resolves both as part of reading the request.
    let table: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a domain nothing created",
            json!({
                "DomainName": "assoc-no-such-domain",
                "PackageList": [{ "PackageID": ids[0] }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a package nothing created",
            json!({
                "DomainName": domain,
                "PackageList": [{ "PackageID": UNKNOWN_PACKAGE_ID }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a batch whose last package nothing created",
            json!({
                "DomainName": domain,
                "PackageList": [
                    { "PackageID": ids[0] },
                    { "PackageID": ids[1] },
                    { "PackageID": UNKNOWN_PACKAGE_ID },
                ],
            }),
            400,
            "ValidationException",
        ),
        (
            "the same package named twice",
            json!({
                "DomainName": domain,
                "PackageList": [{ "PackageID": ids[0] }, { "PackageID": ids[0] }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a domain nothing created, naming no package at all",
            json!({ "DomainName": "assoc-no-such-domain", "PackageList": [] }),
            400,
            "ValidationException",
        ),
        (
            "more packages than the account holds",
            json!({
                "DomainName": domain,
                "PackageList": (0..=PACKAGE_QUOTA)
                    .map(|index| json!({ "PackageID": format!("pkg-{index:032x}") }))
                    .collect::<Vec<Value>>(),
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, status, error) in table {
        let answer = associate(&emulator, body).await;
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
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(
                stored_status(&emulator, domain, id),
                None,
                "case {case}: no association was made for package {index}"
            );
        }
    }
}
