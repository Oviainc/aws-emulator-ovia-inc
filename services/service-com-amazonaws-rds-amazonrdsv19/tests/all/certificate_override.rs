// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account's ROOT CA OVERRIDE: `ModifyCertificates` sets or
//! removes it, and `DescribeCertificates` and `CreateDBInstance` are the two reads that
//! must agree with it.
//!
//! Every call goes over the wire through the same [`MultiServiceRouter`] the production
//! router uses: a real `awsQuery` form POST in, the XML the emulator answers with out. The
//! harness (`emulator`, `call`, `call_in`, `expect_ok`, `expect_error` and the element
//! readers) is the [`crate::db_instances`] suite's, reused rather than copied.
//!
//! The rules these tables pin, and the classes they span:
//!
//! * a published CA certificate's ARN names no owning account (`arn:aws:rds:<region>::
//!   cert:<id>`), because the certificate is published by the service rather than created
//!   by an account — spanned over every certificate in the published catalog, in every
//!   report that carries one;
//! * `CustomerOverride` is true for exactly the certificate the account's override pins
//!   and false for every other published one, and `CustomerOverrideValidTill` is that
//!   certificate's own `ValidTill` (an override cannot outlive the certificate it names)
//!   — spanned over every certificate in the catalog, set under two spellings of its
//!   identifier, including one whose validity has already passed;
//! * `DefaultCertificateForNewLaunches` is "either the root CA override set on your
//!   Amazon Web Services account or the system default CA for the Region if no override
//!   exists", and it is the CA a DB instance created without a `CACertificateIdentifier`
//!   is issued under — spanned over an override set, removed, and never set, and over two
//!   regions;
//! * a value outside the published set, and a request naming neither or both of the two
//!   things this operation can do, are refused with the status and code AWS answers for
//!   them rather than silently accepted.

use service_com_amazonaws_rds_amazonrdsv19::state;
use service_com_amazonaws_rds_amazonrdsv19::state::MAX_REGIONS;

use crate::db_instances::{
    call, call_in, emu_region, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    REGION,
};

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: &[(&str, &str)] = &[];

/// One row of the refusal table: the case name, the parameters the request carries, and
/// the HTTP status and wire `<Code>` it must be refused with.
type RefusalRow = (&'static str, Vec<(&'static str, &'static str)>, u16, &'static str);

/// A second region, so an override is shown to be scoped to the one it was set in:
/// `ModifyCertificates` is documented per Region ("RDS has already moved to a new default
/// CA certificate for the specified Amazon Web Services Region").
const OTHER_REGION: &str = "eu-west-1";

/// The `<Certificate>` subtree of `body` that reports `identifier`.
fn certificate_of(case: &str, body: &str, identifier: &str) -> String {
    subtrees(body, "Certificate")
        .into_iter()
        .find(|certificate| text(certificate, "CertificateIdentifier") == identifier)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Certificate for {identifier} in {body}")
        })
}

/// Assert the override members of every certificate in `body`: `pinned` is the identifier
/// the account's override names, or `None` where none is set.
fn assert_override_members(case: &str, body: &str, pinned: Option<&str>) {
    for certificate in subtrees(body, "Certificate") {
        let identifier = text(&certificate, "CertificateIdentifier");
        let overridden = pinned == Some(identifier.as_str());
        assert_eq!(
            text(&certificate, "CustomerOverride"),
            overridden.to_string(),
            "case {case:?}: with the override on {pinned:?}, {identifier} must report \
             CustomerOverride {overridden}, got {certificate}"
        );
        let valid_till = text(&certificate, "ValidTill");
        let override_valid_till = optional_text(&certificate, "CustomerOverrideValidTill");
        if overridden {
            // The override pins THIS certificate, so it expires when the certificate
            // does: the member is the entry's own ValidTill, read out of the same answer.
            assert_eq!(
                override_valid_till.as_deref(),
                Some(valid_till.as_str()),
                "case {case:?}: the override on {identifier} expires when that certificate \
                 does, got {override_valid_till:?} against ValidTill {valid_till:?}"
            );
        } else {
            assert_eq!(
                override_valid_till, None,
                "case {case:?}: {identifier} is under no override, so it reports no \
                 CustomerOverrideValidTill, got {override_valid_till:?}"
            );
        }
        // A CA certificate is service-owned: its ARN carries an EMPTY account field.
        let arn = text(&certificate, "CertificateArn");
        assert_eq!(
            arn,
            format!("arn:aws:rds:{REGION}::cert:{identifier}"),
            "case {case:?}: expected {identifier} to carry its service-owned ARN, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn an_override_is_set_read_back_and_removed() {
    let router = emulator();

    // (case, the spelling of the identifier the override is set with, the published
    // identifier it names). Every published certificate is a row, under its own spelling
    // and under an upper-cased one, because `CertificateIdentifier` "isn't
    // case-sensitive".
    let mut rows: Vec<(String, String, &'static str)> = Vec::new();
    for certificate in state::CA_CERTIFICATES {
        rows.push((
            format!("override on {}", certificate.identifier),
            certificate.identifier.to_string(),
            certificate.identifier,
        ));
        rows.push((
            format!("override on {} spelled in upper case", certificate.identifier),
            certificate.identifier.to_ascii_uppercase(),
            certificate.identifier,
        ));
    }
    assert!(
        rows.len() >= 6,
        "the published catalog must carry several certificates for this table to span \
         anything, got {} rows",
        rows.len()
    );

    for (case, requested, published) in rows {
        let case = case.as_str();
        let body = expect_ok(
            case,
            call(
                &router,
                "ModifyCertificates",
                &[("CertificateIdentifier", requested.as_str())],
            )
            .await,
        );
        // The answer reports the certificate the override now names, under its PUBLISHED
        // identifier rather than the spelling the request used.
        assert_eq!(
            text(&body, "CertificateIdentifier"),
            published,
            "case {case:?}: expected the published identifier, got {body}"
        );
        assert_eq!(
            text(&body, "CustomerOverride"),
            "true",
            "case {case:?}: the certificate this call pinned is under an override: {body}"
        );
        assert_eq!(
            optional_text(&body, "CustomerOverrideValidTill").as_deref(),
            Some(text(&body, "ValidTill").as_str()),
            "case {case:?}: the override expires when the certificate does: {body}"
        );

        // Read it back: the override is reported on that one certificate, and it is the
        // default for new launches.
        let read = expect_ok(case, call(&router, "DescribeCertificates", NO_PARAMS).await);
        assert_eq!(
            text(&read, "DefaultCertificateForNewLaunches"),
            published,
            "case {case:?}: the override is the default for new launches: {read}"
        );
        assert_override_members(case, &read, Some(published));

        // A read does not change what a later caller sees: the same request twice
        // answers the same catalog and the same override. (The `ResponseMetadata` request
        // id is per response, so the comparison is of the reported certificates.)
        let again = expect_ok(case, call(&router, "DescribeCertificates", NO_PARAMS).await);
        assert_eq!(
            subtrees(&read, "Certificates"),
            subtrees(&again, "Certificates"),
            "case {case:?}: reading the catalog must not change the override it reports"
        );

        // Narrowing to the overridden certificate alone reports the same members. (The
        // first element of a Query answer carries the envelope's `xmlns`, so the
        // comparison is member by member rather than of the two subtrees verbatim.)
        let one = expect_ok(
            case,
            call(
                &router,
                "DescribeCertificates",
                &[("CertificateIdentifier", published)],
            )
            .await,
        );
        let narrowed = certificate_of(case, &one, published);
        let whole = certificate_of(case, &read, published);
        for member in [
            "CertificateIdentifier",
            "CertificateType",
            "ValidFrom",
            "ValidTill",
            "CertificateArn",
            "CustomerOverride",
            "CustomerOverrideValidTill",
        ] {
            assert_eq!(
                optional_text(&narrowed, member),
                optional_text(&whole, member),
                "case {case:?}: one certificate on its own must report the {member} the \
                 whole catalog does, got {narrowed} against {whole}"
            );
        }

        // And removing the override puts the system default back.
        let removed = expect_ok(
            case,
            call(
                &router,
                "ModifyCertificates",
                &[("RemoveCustomerOverride", "true")],
            )
            .await,
        );
        assert_eq!(
            text(&removed, "CertificateIdentifier"),
            state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
            "case {case:?}: removing the override reports the system default CA: {removed}"
        );
        assert_eq!(
            text(&removed, "CustomerOverride"),
            "false",
            "case {case:?}: nothing is overridden after the override is removed: {removed}"
        );
        let read = expect_ok(case, call(&router, "DescribeCertificates", NO_PARAMS).await);
        assert_eq!(
            text(&read, "DefaultCertificateForNewLaunches"),
            state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
            "case {case:?}: with no override the Region's system default stands: {read}"
        );
        assert_override_members(case, &read, None);
    }
}

#[tokio::test]
async fn a_new_db_instance_is_issued_under_the_default_for_new_launches() {
    let router = emulator();

    // (case, the identifier the override is set to before the instance is created, the
    // CA the instance must then be issued under). The rows span: no override at all, an
    // override onto each of two published certificates, and an override removed again.
    let overridden = state::CA_CERTIFICATES
        .iter()
        .map(|certificate| certificate.identifier)
        .find(|identifier| *identifier != state::DEFAULT_CA_CERTIFICATE_IDENTIFIER)
        .expect("the catalog publishes more than the system default");

    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        (
            "no override: the Region's system default",
            None,
            state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        ),
        ("an override onto another published CA", Some(overridden), overridden),
        (
            "an override onto the system default itself",
            Some(state::DEFAULT_CA_CERTIFICATE_IDENTIFIER),
            state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        ),
    ];

    for (index, (case, pinned, expected)) in rows.into_iter().enumerate() {
        match pinned {
            Some(identifier) => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "ModifyCertificates",
                        &[("CertificateIdentifier", identifier)],
                    )
                    .await,
                );
            }
            None => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "ModifyCertificates",
                        &[("RemoveCustomerOverride", "true")],
                    )
                    .await,
                );
            }
        }

        let identifier = format!("emu-ca-default-{index}");
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t4g.micro"),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&created, "CACertificateIdentifier"),
            expected,
            "case {case:?}: an instance created without a CACertificateIdentifier is \
             issued under the default for new launches: {created}"
        );
        // Read it back through the describe: the stored CA is the one the create
        // reported, not a value recomputed per read.
        let read = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&read, "CACertificateIdentifier"),
            expected,
            "case {case:?}: the stored CA of {identifier} must be the one it was created \
             under: {read}"
        );
        assert_eq!(
            text(&read, "CAIdentifier"),
            expected,
            "case {case:?}: CertificateDetails names the same CA: {read}"
        );
    }

    // An instance that NAMES a certificate is issued under that one, whatever the
    // override says: the override is the default, not an override of the request.
    expect_ok(
        "an override in force",
        call(
            &router,
            "ModifyCertificates",
            &[("CertificateIdentifier", overridden)],
        )
        .await,
    );
    let created = expect_ok(
        "the request names its own CA",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-ca-named"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t4g.micro"),
                (
                    "CACertificateIdentifier",
                    state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
                ),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "CACertificateIdentifier"),
        state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        "the CA the request names stands over the account's override: {created}"
    );
}

#[tokio::test]
async fn an_override_is_scoped_to_the_region_it_was_set_in() {
    let router = emulator();

    let pinned = state::CA_CERTIFICATES
        .iter()
        .map(|certificate| certificate.identifier)
        .find(|identifier| *identifier != state::DEFAULT_CA_CERTIFICATE_IDENTIFIER)
        .expect("the catalog publishes more than the system default");

    expect_ok(
        "set the override in the signed region",
        call(
            &router,
            "ModifyCertificates",
            &[("CertificateIdentifier", pinned)],
        )
        .await,
    );

    // The other region is untouched: its default is still the system default, and no
    // certificate there reports an override.
    let other = expect_ok(
        "another region",
        call_in(&router, OTHER_REGION, "DescribeCertificates", NO_PARAMS).await,
    );
    assert_eq!(
        text(&other, "DefaultCertificateForNewLaunches"),
        state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        "an override set in one region leaves another region's default alone: {other}"
    );
    for certificate in subtrees(&other, "Certificate") {
        let identifier = text(&certificate, "CertificateIdentifier");
        assert_eq!(
            text(&certificate, "CustomerOverride"),
            "false",
            "no override is set in {OTHER_REGION}, so {identifier} must report false: \
             {certificate}"
        );
        let arn = text(&certificate, "CertificateArn");
        assert_eq!(
            arn,
            format!("arn:aws:rds:{OTHER_REGION}::cert:{identifier}"),
            "a certificate ARN is scoped to the region it is reported for, got {arn:?}"
        );
    }

    // Removing the override in the other region leaves the one in the signed region
    // standing.
    expect_ok(
        "remove an override that region never had",
        call_in(
            &router,
            OTHER_REGION,
            "ModifyCertificates",
            &[("RemoveCustomerOverride", "true")],
        )
        .await,
    );
    let signed = expect_ok(
        "the region the override was set in",
        call(&router, "DescribeCertificates", NO_PARAMS).await,
    );
    assert_eq!(
        text(&signed, "DefaultCertificateForNewLaunches"),
        pinned,
        "the override of the signed region must still stand: {signed}"
    );
}

#[tokio::test]
async fn an_override_cannot_open_more_region_buckets_than_the_store_holds() {
    let router = emulator();

    // An override is a record stored under the region the request is SIGNED for, and
    // nothing authenticates that region, so setting one gates on the same store-wide
    // region capacity every other record-creating operation gates on: one override in
    // each of `MAX_REGIONS` regions fills it, and the next region is refused rather than
    // growing the store once per distinct region-shaped string a caller signs for.
    let pinned = state::CA_CERTIFICATES
        .iter()
        .map(|certificate| certificate.identifier)
        .find(|identifier| *identifier != state::DEFAULT_CA_CERTIFICATE_IDENTIFIER)
        .expect("the catalog publishes more than the system default");

    for index in 0..MAX_REGIONS {
        let region = emu_region(index);
        expect_ok(
            &region,
            call_in(
                &router,
                &region,
                "ModifyCertificates",
                &[("CertificateIdentifier", pinned)],
            )
            .await,
        );
    }

    let body = expect_error(
        "one region past the store's region capacity",
        call_in(
            &router,
            "ap-emuextra-1",
            "ModifyCertificates",
            &[("CertificateIdentifier", pinned)],
        )
        .await,
        400,
        "ValidationException",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains(&MAX_REGIONS.to_string()),
        "expected the refusal to name the region capacity, got {message:?}"
    );

    // A region that already holds the override still has room for it, and REMOVING an
    // override stores nothing, so it is not gated at all.
    let region = emu_region(0);
    expect_ok(
        "the same override again in a region already stored",
        call_in(
            &router,
            &region,
            "ModifyCertificates",
            &[("CertificateIdentifier", pinned)],
        )
        .await,
    );
    expect_ok(
        "removing an override past the region capacity",
        call_in(
            &router,
            "ap-emuextra-1",
            "ModifyCertificates",
            &[("RemoveCustomerOverride", "true")],
        )
        .await,
    );
    // The region past the capacity reports the system default, because nothing was
    // stored for it.
    let read = expect_ok(
        "the region past the capacity",
        call_in(&router, "ap-emuextra-1", "DescribeCertificates", NO_PARAMS).await,
    );
    assert_eq!(
        text(&read, "DefaultCertificateForNewLaunches"),
        state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        "a refused override must leave that region on the system default: {read}"
    );
}

#[tokio::test]
async fn a_request_that_names_no_change_this_operation_can_make_is_refused() {
    let router = emulator();

    // (case, parameters, expected status, expected wire code)
    let refusals: Vec<RefusalRow> = vec![
        (
            "an identifier no published CA carries",
            vec![("CertificateIdentifier", "rds-ca-2042")],
            404,
            "CertificateNotFound",
        ),
        (
            "an identifier that is a prefix of a published one",
            vec![("CertificateIdentifier", "rds-ca")],
            404,
            "CertificateNotFound",
        ),
        (
            "both things this operation can do at once",
            vec![
                ("CertificateIdentifier", state::DEFAULT_CA_CERTIFICATE_IDENTIFIER),
                ("RemoveCustomerOverride", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "neither of them",
            vec![],
            400,
            "InvalidParameterCombination",
        ),
        (
            "RemoveCustomerOverride explicitly false and nothing else",
            vec![("RemoveCustomerOverride", "false")],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, params, status, code) in refusals {
        expect_error(
            case,
            call(&router, "ModifyCertificates", &params).await,
            status,
            code,
        );
    }

    // A refused request changed nothing: the Region still reports its system default.
    let read = expect_ok(
        "after the refusals",
        call(&router, "DescribeCertificates", NO_PARAMS).await,
    );
    assert_eq!(
        text(&read, "DefaultCertificateForNewLaunches"),
        state::DEFAULT_CA_CERTIFICATE_IDENTIFIER,
        "a refused ModifyCertificates must leave the default alone: {read}"
    );
    assert_override_members("after the refusals", &read, None);
}
