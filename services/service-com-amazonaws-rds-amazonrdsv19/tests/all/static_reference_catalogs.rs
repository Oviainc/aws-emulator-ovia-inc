// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that PIN the two static reference catalogs this front door
//! publishes: the CA certificate catalog `DescribeCertificates` and
//! `ModifyCertificates` report, and the account-quota catalog
//! `DescribeAccountAttributes` reports.
//!
//! The sibling suites assert those reports against `crate::state` itself, so they cannot
//! tell whether the catalogs carry the values the service publishes. The tables below
//! restate the published values LITERALLY and assert them over the wire, so a catalog row
//! that drifts fails here.
//!
//! The rules under test:
//!
//! * the certificate catalog is published strongest authority first — every
//!   elliptic-curve authority before every RSA one, and within a kind the longest key
//!   first — holding only the authorities the service still provides, and each entry
//!   reports its identifier, its `CA` type, its service-owned ARN, its published validity
//!   window and its published SHA-1 fingerprint as `Thumbprint`;
//! * the same projection answers the mutation path: the certificate `ModifyCertificates`
//!   pins is reported with the fingerprint and validity window the catalog publishes for
//!   it, and `DescribeCertificates` reads the same row back;
//! * the account-quota report carries every quota the service publishes, each with the
//!   maximum its own create path enforces and a `Used` counted out of the records the
//!   signed region holds — including the parameter-group quotas and the integration quota.
//!
//! The rows span what those rules generalise over: every entry of the certificate catalog
//! (elliptic-curve and RSA, a 2048-bit and a 4096-bit modulus, a validity that runs for
//! forty years and one that runs for a hundred), each read whole and one at a time, and
//! three quotas whose usage is a count of a different record kind, at zero, one and two
//! records.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION, DB_PARAMETER_GROUP_QUOTA_PER_REGION,
    INTEGRATION_QUOTA_PER_REGION,
};

use crate::db_instances::{
    call, emulator, expect_ok, optional_text, subtrees, text, texts, REGION,
};

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: &[(&str, &str)] = &[];

/// An Amazon Redshift namespace ARN, the kind of target `CreateIntegration$TargetArn`
/// takes.
const REDSHIFT_TARGET_ARN: &str =
    "arn:aws:redshift-serverless:us-west-2:000000000000:namespace/zero-etl";

/// One published certificate authority, as the published catalog states it.
struct PublishedCertificate {
    /// `Certificate$CertificateIdentifier`.
    identifier: &'static str,
    /// `Certificate$ValidFrom`.
    valid_from: &'static str,
    /// `Certificate$ValidTill`.
    valid_till: &'static str,
    /// `Certificate$Thumbprint` — the authority's SHA-1 fingerprint. EMPTY for an
    /// authority whose fingerprint the published catalog does not state, which is then
    /// reported with no `Thumbprint` member at all rather than with an invented digest.
    thumbprint: &'static str,
}

/// The published CA certificate catalog, in the order the service publishes it: the
/// elliptic-curve authority, then the RSA ones from the longest modulus to the shortest.
/// An authority whose certificate has expired is no longer provided, so it is no row.
const PUBLISHED_CERTIFICATES: &[PublishedCertificate] = &[
    PublishedCertificate {
        identifier: "rds-ca-ecc384-g1",
        valid_from: "2021-05-24T22:06:59Z",
        valid_till: "2121-05-24T23:06:59Z",
        thumbprint: "2ee3dcc06e50192559b13929e73484354f23387d",
    },
    PublishedCertificate {
        identifier: "rds-ca-rsa4096-g1",
        valid_from: "2021-05-24T22:03:20Z",
        valid_till: "2121-05-24T23:03:20Z",
        thumbprint: "19da4f2af579a8ae1f6a0fa77aa5befd874b4cab",
    },
    PublishedCertificate {
        identifier: "rds-ca-rsa2048-g1",
        valid_from: "2021-05-24T21:59:00Z",
        valid_till: "2061-05-24T22:59:00Z",
        thumbprint: "7c40cb42714b6fdb2b296f9bbd0e8bb364436a76",
    },
];

/// One row of the read table: the case name, the parameters the read carries, and the
/// identifiers the answer must report, in order.
type ReadRow = (String, Vec<(&'static str, String)>, Vec<&'static str>);

/// Assert one `<Certificate>` subtree reports what the catalog publishes for `published`.
fn assert_published_certificate(case: &str, certificate: &str, published: &PublishedCertificate) {
    let case = format!("{case} certificate {}", published.identifier);
    for (tag, expected) in [
        ("CertificateIdentifier", published.identifier),
        ("CertificateType", "CA"),
        ("ValidFrom", published.valid_from),
        ("ValidTill", published.valid_till),
    ] {
        let actual = text(certificate, tag);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} in {certificate}"
        );
    }
    // A CA certificate is published BY the service, so its ARN names no owning account.
    let arn = text(certificate, "CertificateArn");
    assert_eq!(
        arn,
        format!("arn:aws:rds:{REGION}::cert:{}", published.identifier),
        "case {case:?}: expected the service-owned ARN, got {arn:?}"
    );
    let thumbprint = optional_text(certificate, "Thumbprint");
    if published.thumbprint.is_empty() {
        assert!(
            thumbprint.is_none(),
            "case {case:?}: the published catalog states no fingerprint for this authority, \
             so no <Thumbprint> member is due, got {thumbprint:?} in {certificate}"
        );
    } else {
        assert_eq!(
            thumbprint.as_deref(),
            Some(published.thumbprint),
            "case {case:?}: expected the published fingerprint {:?}, got {thumbprint:?} in \
             {certificate}",
            published.thumbprint
        );
    }
}

/// The `<Certificate>` subtree of `body` that reports `identifier`, which must be
/// reported exactly once.
fn certificate_of(case: &str, body: &str, identifier: &str) -> String {
    let found: Vec<String> = subtrees(body, "Certificate")
        .into_iter()
        .filter(|certificate| text(certificate, "CertificateIdentifier") == identifier)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <Certificate> for {identifier}, got {} in {body}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

#[tokio::test]
async fn the_certificate_catalog_is_published_in_order_with_its_fingerprints() {
    let router = emulator();

    // (case, the parameters of the read). The whole catalog, and then each entry on its
    // own — under its own spelling and an upper-cased one, because
    // `CertificateIdentifier` "isn't case-sensitive".
    let mut rows: Vec<ReadRow> = vec![(
        "every published CA".to_string(),
        Vec::new(),
        PUBLISHED_CERTIFICATES
            .iter()
            .map(|published| published.identifier)
            .collect(),
    )];
    for published in PUBLISHED_CERTIFICATES {
        for spelling in [
            published.identifier.to_string(),
            published.identifier.to_ascii_uppercase(),
        ] {
            rows.push((
                format!("only {} as {spelling}", published.identifier),
                vec![("CertificateIdentifier", spelling)],
                vec![published.identifier],
            ));
        }
    }

    for (case, params, expected) in rows {
        let body = expect_ok(&case, call(&router, "DescribeCertificates", &params).await);
        let reported = texts(&body, "CertificateIdentifier");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the published catalog {expected:?} in publication order, \
             got {reported:?} in {body}"
        );
        for identifier in &expected {
            let published = PUBLISHED_CERTIFICATES
                .iter()
                .find(|published| published.identifier == *identifier)
                .expect("the table names the catalog's own identifiers");
            let certificate = certificate_of(&case, &body, identifier);
            assert_published_certificate(&case, &certificate, published);
        }
    }
}

#[tokio::test]
async fn pinning_a_certificate_reports_its_published_fingerprint_on_both_paths() {
    for published in PUBLISHED_CERTIFICATES {
        // A fresh emulator per row, so each override is set on an account that has none.
        let router = emulator();
        let case = format!("override on {}", published.identifier);
        let modified = expect_ok(
            &case,
            call(
                &router,
                "ModifyCertificates",
                &[("CertificateIdentifier", published.identifier)],
            )
            .await,
        );
        let pinned = certificate_of(&case, &modified, published.identifier);
        assert_published_certificate(&format!("{case} via ModifyCertificates"), &pinned, published);
        assert_eq!(
            text(&pinned, "CustomerOverride"),
            "true",
            "case {case:?}: the certificate the override pins reports it: {pinned}"
        );

        // The read path reports the same projection of the same catalog row.
        let described = expect_ok(
            &case,
            call(&router, "DescribeCertificates", NO_PARAMS).await,
        );
        let read_back = certificate_of(&case, &described, published.identifier);
        assert_published_certificate(
            &format!("{case} via DescribeCertificates"),
            &read_back,
            published,
        );
        let default = text(&described, "DefaultCertificateForNewLaunches");
        assert_eq!(
            default, published.identifier,
            "case {case:?}: the override is the default CA for new databases, got {default:?}"
        );
    }
}

/// One quota row this suite pins: its published name, the maximum the service publishes
/// for it, and the usage expected before and after the records below are created.
struct QuotaRow {
    name: &'static str,
    /// The published maximum, restated literally here and checked against the constant
    /// the create path enforces, so the report and the refusal cannot drift apart.
    max: i64,
    enforced: i64,
    used_after_setup: i64,
}

#[tokio::test]
async fn the_quota_report_carries_the_group_and_integration_quotas_with_their_usage() {
    // Amazon RDS's published default quotas: "DB parameter groups" 50 per Region, "DB
    // cluster parameter groups" 50 per Region, and 100 zero-ETL integrations per Region.
    let rows = [
        QuotaRow {
            name: "DBParameterGroups",
            max: 50,
            enforced: DB_PARAMETER_GROUP_QUOTA_PER_REGION as i64,
            used_after_setup: 2,
        },
        QuotaRow {
            name: "DBClusterParameterGroups",
            max: 50,
            enforced: DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION as i64,
            used_after_setup: 1,
        },
        QuotaRow {
            name: "Integrations",
            max: 100,
            enforced: INTEGRATION_QUOTA_PER_REGION as i64,
            used_after_setup: 2,
        },
    ];

    let router = emulator();
    for row in &rows {
        assert_eq!(
            row.max, row.enforced,
            "case {:?}: the published maximum and the one the create path enforces are one \
             number, got {} and {}",
            row.name, row.max, row.enforced
        );
    }

    // A region holding none of these records uses nothing toward any of them.
    for row in &rows {
        let (used, max) = quota_of(&report(&router).await, row.name);
        assert_eq!(
            (used, max),
            (0, row.max),
            "case {:?}: expected a region holding nothing to report 0 of {}, got {used} of \
             {max}",
            row.name,
            row.max
        );
    }

    // Then the records each quota counts: two DB parameter groups, one cluster parameter
    // group, and two integrations off one stored cluster.
    for name in ["emu-catalog-pg-1", "emu-catalog-pg-2"] {
        expect_ok(
            name,
            call(
                &router,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", name),
                    ("DBParameterGroupFamily", "mysql8.0"),
                    ("Description", "a published-quota fixture"),
                ],
            )
            .await,
        );
    }
    expect_ok(
        "emu-catalog-cluster-pg",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-catalog-cluster-pg"),
                ("DBParameterGroupFamily", "aurora-mysql8.0"),
                ("Description", "a published-quota fixture"),
            ],
        )
        .await,
    );
    expect_ok(
        "emu-catalog-cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-catalog-cluster"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    let source_arn = format!("arn:aws:rds:{REGION}:000000000000:cluster:emu-catalog-cluster");
    for name in ["emu-catalog-zero-etl-1", "emu-catalog-zero-etl-2"] {
        expect_ok(
            name,
            call(
                &router,
                "CreateIntegration",
                &[
                    ("IntegrationName", name),
                    ("SourceArn", source_arn.as_str()),
                    ("TargetArn", REDSHIFT_TARGET_ARN),
                ],
            )
            .await,
        );
    }

    let after = report(&router).await;
    for row in &rows {
        let (used, max) = quota_of(&after, row.name);
        assert_eq!(
            used, row.used_after_setup,
            "case {:?}: expected the usage counted off the region's records ({}), got {used}",
            row.name, row.used_after_setup
        );
        assert_eq!(
            max, row.max,
            "case {:?}: the maximum is catalog data and does not move with usage, got {max}",
            row.name
        );
        assert!(
            (0..=max).contains(&used),
            "case {:?}: expected usage within 0..={max}, got {used}",
            row.name
        );
    }
}

/// `DescribeAccountAttributes` in the signed region, as its `<AccountQuota>` entries.
async fn report(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<(String, i64, i64)> {
    let body = expect_ok(
        "DescribeAccountAttributes",
        call(router, "DescribeAccountAttributes", NO_PARAMS).await,
    );
    subtrees(&body, "AccountQuota")
        .iter()
        .map(|entry| {
            (
                text(entry, "AccountQuotaName"),
                number(entry, "Used"),
                number(entry, "Max"),
            )
        })
        .collect()
}

/// The `(Used, Max)` the report carries for the quota named `name`, which it must carry.
fn quota_of(report: &[(String, i64, i64)], name: &str) -> (i64, i64) {
    report
        .iter()
        .find(|(quota, _, _)| quota == name)
        .map(|(_, used, max)| (*used, *max))
        .unwrap_or_else(|| panic!("the report must carry a {name} quota, got {report:?}"))
}

/// One `<tag>` of an entry, as the `Long` it is modelled as.
fn number(entry: &str, tag: &str) -> i64 {
    let value = text(entry, tag);
    value
        .parse()
        .unwrap_or_else(|error| panic!("expected <{tag}> to be a number, got {value:?}: {error}"))
}
