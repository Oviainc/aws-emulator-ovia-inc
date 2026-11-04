// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the published reference catalogues this service reports:
//! `DescribeCertificates`, `DescribeDBEngineVersions`, `DescribeEventCategories`,
//! `DescribeOrderableDBInstanceOptions`, and the parameter metadata
//! `DescribeDBClusterParameters` reports for a family's published parameters.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`Docdb`] harness the sibling suites use, so what is asserted is
//! what a client reads off the wire.
//!
//! The rule each test asserts is driven over a TABLE spanning the class it generalises
//! over, never over one request:
//!
//! * the certificate report is asserted over EVERY entry of the published catalogue
//!   (`state::CA_CERTIFICATES`), by its identifier in both cases, plus an identifier the
//!   catalogue does not carry;
//! * the engine-version report is asserted over every published version
//!   (`state::DOCDB_ENGINE_VERSIONS`) and over each of its four documented filters;
//! * the event-category report is asserted over every source type the model declares,
//!   filtered and unfiltered, plus the source types this service publishes no map for,
//!   which it refuses;
//! * the option report is asserted over the whole version-by-class matrix, each filter, and
//!   a page bound;
//! * the family-scoped `MinimumEngineVersion` rule is asserted over every published family
//!   crossed with every parameter the family publishes.

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{count, error_code, opt_text, text, texts, Docdb, REGION};

/// The `<Code>` the awsQuery envelope carries for a `CertificateNotFoundFault`: the
/// operation's modelled `awsQueryError` code
/// (`smithy_query(query="operation",operation="DescribeCertificates")`).
const CERTIFICATE_NOT_FOUND_CODE: &str = "CertificateNotFound";

/// The `<Code>` a request this service refuses outside its modelled errors carries.
const VALIDATION_CODE: &str = "ValidationException";

/// The `<Code>` the awsQuery envelope carries for a value outside what a member admits.
const INVALID_PARAMETER_VALUE_CODE: &str = "InvalidParameterValue";

/// One row of the filter table: the case, the request's parameters and how many options
/// the filter selects. Named so the row type stays readable where the table is written.
type FilterRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, usize);

/// The catalogue entry at `index`, for a table row that names one.
fn published_certificate(index: usize) -> &'static state::CaCertificate {
    &state::CA_CERTIFICATES[index]
}

/// The `<Parameter>` element whose `ParameterName` is `name`, as its own XML fragment.
///
/// A `Parameters` list carries one element per parameter and the reader in
/// `cluster_lifecycle` reads a tag's text in document order, so a per-parameter assertion
/// has to narrow to the element first.
fn parameter_block(xml: &str, name: &str) -> String {
    let needle = format!("<ParameterName>{name}</ParameterName>");
    let blocks: Vec<&str> = xml.split("<Parameter>").collect();
    blocks
        .into_iter()
        .find(|block| block.contains(needle.as_str()))
        .map(|block| block.to_string())
        .unwrap_or_else(|| panic!("no <Parameter> named {name} in {xml}"))
}

// ---------------------------------------------------------------------------
// Cause 1: the certificate catalogue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_certificates_serves_the_published_authority_catalogue() {
    let docdb = Docdb::new();

    // The unfiltered report is the whole catalogue, in the order it publishes the
    // authorities: every identifier, in that order, and nothing else.
    let listed = docdb.ok("every certificate", "DescribeCertificates", &[]).await;
    let published: Vec<String> = state::CA_CERTIFICATES
        .iter()
        .map(|certificate| certificate.identifier.to_string())
        .collect();
    assert_eq!(
        (
            count(&listed, "Certificate"),
            texts(&listed, "CertificateIdentifier"),
        ),
        (published.len(), published.clone()),
        "case \"every certificate\": expected the published catalogue in publication order, \
         got {listed}"
    );
    assert_eq!(
        opt_text(&listed, "Marker"),
        None,
        "case \"every certificate\": the whole catalogue fits one page, so no Marker is \
         reported, got {listed}"
    );

    // One row per published authority, each asked for by its own identifier — in the case
    // the catalogue holds it and in upper case, because `CertificateIdentifier` "is not
    // case sensitive".
    for index in 0..state::CA_CERTIFICATES.len() {
        let certificate = published_certificate(index);
        for case in [
            certificate.identifier.to_string(),
            certificate.identifier.to_ascii_uppercase(),
        ] {
            let one = docdb
                .ok(
                    &case,
                    "DescribeCertificates",
                    &[("CertificateIdentifier", &case)],
                )
                .await;
            let expected_arn = format!("arn:aws:rds:{REGION}::cert:{}", certificate.identifier);
            assert_eq!(
                (
                    count(&one, "Certificate"),
                    text(&one, "CertificateIdentifier"),
                    text(&one, "CertificateType"),
                    text(&one, "CertificateArn"),
                    text(&one, "ValidFrom"),
                    text(&one, "ValidTill"),
                    opt_text(&one, "Thumbprint"),
                ),
                (
                    1,
                    certificate.identifier.to_string(),
                    state::CA_CERTIFICATE_TYPE.to_string(),
                    expected_arn,
                    certificate.valid_from.to_string(),
                    certificate.valid_till.to_string(),
                    // A fingerprint the catalogue does not carry is reported as no member
                    // at all rather than as an empty string.
                    (!certificate.thumbprint.is_empty())
                        .then(|| certificate.thumbprint.to_string()),
                ),
                "case {case:?}: expected exactly the published authority, got {one}"
            );
        }
    }

    // An identifier the catalogue does not carry is the modelled 404, not an empty page.
    for case in ["rds-ca-1999", "not-a-certificate"] {
        let refused = docdb
            .call(
                "DescribeCertificates",
                &[("CertificateIdentifier", case)],
            )
            .await;
        assert_eq!(
            (refused.status, error_code(&refused.xml).as_str()),
            (404, CERTIFICATE_NOT_FOUND_CODE),
            "case {case:?}: an identifier no authority carries is the modelled 404, got {} \
             with {}",
            refused.status,
            refused.xml
        );
    }

    // A filter cannot be honoured and is refused rather than silently dropped.
    let filtered = docdb
        .call(
            "DescribeCertificates",
            &[
                ("Filters.Filter.1.Name", "certificate-id"),
                ("Filters.Filter.1.Values.Value.1", "rds-ca-rsa2048-g1"),
            ],
        )
        .await;
    assert_eq!(
        (filtered.status, error_code(&filtered.xml).as_str()),
        (400, VALIDATION_CODE),
        "case \"a filter\": an unsupported filter is refused, got {} with {}",
        filtered.status,
        filtered.xml
    );
}

#[tokio::test]
async fn the_published_authorities_carry_their_real_validity_instants() {
    // The catalogue's rule, asserted over every entry: a validity instant is a real
    // second-resolution instant the published authority carries, `ValidFrom` precedes
    // `ValidTill`, and a published authority's validity has NOT passed — an authority whose
    // certificate expired is no longer provided for a cluster's TLS endpoint, so it is
    // published nowhere.
    let offered = state::supported_ca_certificate_identifiers();
    let now = state::now();
    for certificate in state::CA_CERTIFICATES {
        let case = certificate.identifier;
        let from = state::catalog_timestamp(certificate.valid_from);
        let till = state::catalog_timestamp(certificate.valid_till);
        assert!(
            from.secs() < till.secs(),
            "case {case:?}: expected ValidFrom before ValidTill, got {} and {}",
            certificate.valid_from,
            certificate.valid_till
        );
        assert!(
            certificate.is_valid_at(&now),
            "case {case:?}: an authority whose certificate has expired is no longer \
             provided, so it is not published, got ValidTill {}",
            certificate.valid_till
        );
    }

    // The elliptic-curve authority is published and is NOT offered: a DocumentDB cluster
    // endpoint serves an RSA server certificate.
    let elliptic_curve: Vec<String> = state::CA_CERTIFICATES
        .iter()
        .filter(|certificate| {
            matches!(certificate.key, state::CertificateKey::EllipticCurve(_))
        })
        .map(|certificate| certificate.identifier.to_string())
        .collect();
    assert!(
        !elliptic_curve.is_empty()
            && elliptic_curve
                .iter()
                .all(|identifier| !offered.contains(identifier)),
        "case \"elliptic curve\": expected the published ECDSA authorities to be offered to \
         no engine version, got {offered:?} against {elliptic_curve:?}"
    );
}

// ---------------------------------------------------------------------------
// Cause 2: the engine-version catalogue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_db_engine_versions_derives_every_member_from_the_catalogues() {
    let docdb = Docdb::new();

    // The unfiltered report is every published version, oldest first.
    let listed = docdb
        .ok("every version", "DescribeDBEngineVersions", &[])
        .await;
    assert_eq!(
        count(&listed, "DBEngineVersion"),
        state::DOCDB_ENGINE_VERSIONS.len(),
        "case \"every version\": expected one record per published engine version, got {listed}"
    );

    // One row per published version: the family, the description, the supported
    // authorities, the exportable log types and the upgrade targets are each DERIVED, so
    // each is asserted against the catalogue it is derived from rather than against a
    // literal.
    for (index, version) in state::DOCDB_ENGINE_VERSIONS.iter().enumerate() {
        let one = docdb
            .ok(
                version,
                "DescribeDBEngineVersions",
                &[("EngineVersion", version)],
            )
            .await;
        let expected_targets: Vec<String> = state::DOCDB_ENGINE_VERSIONS[index + 1..]
            .iter()
            .map(|target| (*target).to_string())
            .collect();
        let reported_targets: Vec<String> = texts(&one, "EngineVersion")
            .into_iter()
            .skip(1)
            .collect();
        assert_eq!(
            (
                count(&one, "DBEngineVersion"),
                text(&one, "Engine"),
                text(&one, "EngineVersion"),
                text(&one, "DBParameterGroupFamily"),
                text(&one, "DBEngineVersionDescription"),
                text(&one, "SupportsCertificateRotationWithoutRestart"),
                count(&one, "UpgradeTarget"),
                reported_targets.clone(),
            ),
            (
                1,
                state::DOCDB_ENGINE.to_string(),
                (*version).to_string(),
                state::parameter_group_family(version),
                state::engine_version_description(version),
                "true".to_string(),
                expected_targets.len(),
                expected_targets.clone(),
            ),
            "case {version:?}: expected the record derived from the published catalogues, got \
             {one}"
        );
        // Every upgrade target of a published version opens a new major line, so each is a
        // major-version upgrade and none is taken automatically.
        assert_eq!(
            (
                texts(&one, "IsMajorVersionUpgrade"),
                texts(&one, "AutoUpgrade"),
            ),
            (
                vec!["true".to_string(); expected_targets.len()],
                vec!["false".to_string(); expected_targets.len()],
            ),
            "case {version:?}: every published upgrade target is a major-version upgrade that \
             is not taken automatically, got {one}"
        );
        // The authorities are the ones the certificate catalogue offers this engine, and
        // the log types the ones the family's parameter catalogue can switch on. Both lists
        // carry unnamed members, so they are read as one sequence in the order the model
        // declares the two members in: the log types, then the authorities.
        let expected_members: Vec<String> = state::exportable_log_types(version)
            .into_iter()
            .chain(state::supported_ca_certificate_identifiers())
            .collect();
        assert_eq!(
            texts(&one, "member"),
            expected_members,
            "case {version:?}: expected the supported authorities and the exportable log \
             types the catalogues derive, got {one}"
        );
        assert_eq!(
            text(&one, "SupportsLogExportsToCloudwatchLogs"),
            (!state::exportable_log_types(version).is_empty()).to_string(),
            "case {version:?}: a version exports to CloudWatch Logs exactly when it has a log \
             type to export, got {one}"
        );
    }

    // Each documented filter, including the values that select nothing.
    let newest = state::DOCDB_ENGINE_VERSIONS[state::DOCDB_ENGINE_VERSIONS.len() - 1];
    let newest_family = state::parameter_group_family(newest);
    let rows: [FilterRow<'_>; 7] = [
        ("this engine", vec![("Engine", "docdb")], state::DOCDB_ENGINE_VERSIONS.len()),
        ("this engine, mixed case", vec![("Engine", "DocDB")], state::DOCDB_ENGINE_VERSIONS.len()),
        ("another engine", vec![("Engine", "mysql")], 0),
        ("a major line", vec![("EngineVersion", state::engine_major_version(newest))], 1),
        ("a version the engine does not publish", vec![("EngineVersion", "1.2.3")], 0),
        ("a family", vec![("DBParameterGroupFamily", &newest_family)], 1),
        ("the default only", vec![("DefaultOnly", "true")], 1),
    ];
    for (case, params, expected) in rows {
        let listed = docdb.ok(case, "DescribeDBEngineVersions", &params).await;
        assert_eq!(
            count(&listed, "DBEngineVersion"),
            expected,
            "case {case:?}: expected {expected} record(s), got {listed}"
        );
    }

    // `DefaultOnly` reports the latest version, which is the one a create without an
    // `EngineVersion` gets.
    let default_only = docdb
        .ok(
            "the default only",
            "DescribeDBEngineVersions",
            &[("DefaultOnly", "true")],
        )
        .await;
    assert_eq!(
        text(&default_only, "EngineVersion"),
        state::DEFAULT_ENGINE_VERSION.to_string(),
        "case \"the default only\": expected the version a create defaults to, got \
         {default_only}"
    );
}

// ---------------------------------------------------------------------------
// Causes 3 and 5: the event-category catalogue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_event_categories_publishes_every_declared_source_type() {
    let docdb = Docdb::new();

    // Unfiltered: one map per published source type, in the catalogue's order.
    let listed = docdb
        .ok("every source type", "DescribeEventCategories", &[])
        .await;
    let published: Vec<String> = state::EVENT_CATEGORIES
        .iter()
        .map(|(source_type, _)| (*source_type).to_string())
        .collect();
    assert_eq!(
        (
            count(&listed, "EventCategoriesMap"),
            texts(&listed, "SourceType"),
        ),
        (published.len(), published.clone()),
        "case \"every source type\": expected one map per published source type, got {listed}"
    );
    // ...and every category of every map, in one read: the unfiltered report carries the
    // concatenation of the published category lists.
    let every_category: Vec<String> = state::EVENT_CATEGORIES
        .iter()
        .flat_map(|(_, categories)| categories.iter().map(|category| (*category).to_string()))
        .collect();
    assert_eq!(
        texts(&listed, "EventCategory"),
        every_category,
        "case \"every source type\": expected each map's published categories, got {listed}"
    );

    // One row per published source type, asked for by name.
    for (source_type, categories) in state::EVENT_CATEGORIES {
        let one = docdb
            .ok(
                source_type,
                "DescribeEventCategories",
                &[("SourceType", source_type)],
            )
            .await;
        let expected: Vec<String> = categories
            .iter()
            .map(|category| (*category).to_string())
            .collect();
        assert_eq!(
            (
                count(&one, "EventCategoriesMap"),
                text(&one, "SourceType"),
                texts(&one, "EventCategory"),
            ),
            (1, (*source_type).to_string(), expected),
            "case {source_type:?}: expected that source type's published categories, got {one}"
        );
    }

    // The two categories only a CLUSTER has are published for `db-cluster`, which is what a
    // subscription to a migration or a serverless event is built from.
    let cluster = docdb
        .ok(
            "db-cluster",
            "DescribeEventCategories",
            &[("SourceType", "db-cluster")],
        )
        .await;
    let cluster_categories = texts(&cluster, "EventCategory");
    for category in ["migration", "serverless"] {
        assert!(
            cluster_categories.contains(&category.to_string()),
            "case \"db-cluster\": expected {category:?} among the cluster categories, got \
             {cluster_categories:?}"
        );
    }

    // A source type this service publishes no category map for names no event source type
    // at all, so it is REFUSED rather than answered with an empty list: the refusal carries
    // the code the query APIs answer for a value outside what a member admits, and the
    // sentence names the types that ARE valid, built from the same catalogue the report is
    // built from.
    let expected_message = state::invalid_event_source_type_message();
    for case in ["db-proxy", "not-a-source-type", "DB-CLUSTER"] {
        let refused = docdb
            .call("DescribeEventCategories", &[("SourceType", case)])
            .await;
        assert_eq!(
            (
                refused.status,
                error_code(&refused.xml),
                text(&refused.xml, "Message"),
            ),
            (
                400,
                INVALID_PARAMETER_VALUE_CODE.to_string(),
                expected_message.clone(),
            ),
            "case {case:?}: a source type this service publishes no map for is refused, got \
             {} with {}",
            refused.status,
            refused.xml
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 4: the orderable-option catalogue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_orderable_db_instance_options_crosses_versions_with_classes() {
    let docdb = Docdb::new();

    let classes = state::instance_classes();
    let versions = state::DOCDB_ENGINE_VERSIONS.len();
    let expected_total = versions * classes.len();
    let zones = state::default_availability_zones(REGION);

    // The whole matrix, in one page: every published version crossed with every class a
    // cluster member can be ordered in.
    let listed = docdb
        .ok(
            "every option",
            "DescribeOrderableDBInstanceOptions",
            &[("Engine", "docdb"), ("MaxRecords", "100")],
        )
        .await;
    assert_eq!(
        count(&listed, "OrderableDBInstanceOption"),
        expected_total,
        "case \"every option\": expected {versions} version(s) crossed with {} class(es), got \
         {listed}",
        classes.len()
    );
    assert_eq!(
        (
            texts(&listed, "Engine")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            texts(&listed, "LicenseModel")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            texts(&listed, "Vpc")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            texts(&listed, "StorageType")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            texts(&listed, "DBInstanceClass")
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
        ),
        (
            [state::DOCDB_ENGINE.to_string()].into_iter().collect(),
            [state::DOCDB_LICENSE_MODEL.to_string()].into_iter().collect(),
            ["true".to_string()].into_iter().collect(),
            [state::DEFAULT_STORAGE_TYPE.to_string()].into_iter().collect(),
            classes.iter().cloned().collect(),
        ),
        "case \"every option\": every option is this engine's, under its own licence, in a \
         VPC, on the storage a cluster manages, got {listed}"
    );

    // Each option is offered in the zones of the request's Region — the same zones a
    // cluster is placed in, so a caller can create in a zone it read here.
    let one = docdb
        .ok(
            "one option",
            "DescribeOrderableDBInstanceOptions",
            &[
                ("Engine", "docdb"),
                ("DBInstanceClass", &classes[0]),
                ("EngineVersion", state::DEFAULT_ENGINE_VERSION),
            ],
        )
        .await;
    assert_eq!(
        (count(&one, "OrderableDBInstanceOption"), texts(&one, "Name")),
        (1, zones.clone()),
        "case \"one option\": expected the Region's zones, got {one}"
    );

    // Each documented filter, including the values that select nothing.
    let rows: [FilterRow<'_>; 6] = [
        (
            "one class",
            vec![("Engine", "docdb"), ("DBInstanceClass", &classes[0])],
            versions,
        ),
        (
            "one version",
            vec![("Engine", "docdb"), ("EngineVersion", state::DEFAULT_ENGINE_VERSION)],
            classes.len(),
        ),
        (
            "this licence",
            vec![
                ("Engine", "docdb"),
                ("LicenseModel", state::DOCDB_LICENSE_MODEL),
                ("MaxRecords", "100"),
            ],
            expected_total,
        ),
        (
            "another licence",
            vec![("Engine", "docdb"), ("LicenseModel", "license-included")],
            0,
        ),
        (
            "a class this engine does not offer",
            vec![("Engine", "docdb"), ("DBInstanceClass", "db.m5.large")],
            0,
        ),
        (
            "non-VPC offerings",
            vec![("Engine", "docdb"), ("Vpc", "false")],
            0,
        ),
    ];
    for (case, params, expected) in rows {
        let listed = docdb
            .ok(case, "DescribeOrderableDBInstanceOptions", &params)
            .await;
        assert_eq!(
            count(&listed, "OrderableDBInstanceOption"),
            expected,
            "case {case:?}: expected {expected} option(s), got {listed}"
        );
    }

    // A page bound: the smallest `MaxRecords` the member documents, followed by the marker
    // the first page handed back.
    let first = docdb
        .ok(
            "the first page",
            "DescribeOrderableDBInstanceOptions",
            &[("Engine", "docdb"), ("MaxRecords", "20")],
        )
        .await;
    let marker = text(&first, "Marker");
    let second = docdb
        .ok(
            "the second page",
            "DescribeOrderableDBInstanceOptions",
            &[
                ("Engine", "docdb"),
                ("MaxRecords", "20"),
                ("Marker", &marker),
            ],
        )
        .await;
    assert_eq!(
        (
            count(&first, "OrderableDBInstanceOption"),
            count(&second, "OrderableDBInstanceOption"),
        ),
        (20, 20),
        "case \"a page bound\": each page carries the records the caller asked for, got \
         {first} then {second}"
    );
    let first_classes = texts(&first, "DBInstanceClass");
    let second_classes = texts(&second, "DBInstanceClass");
    assert!(
        second_classes
            .iter()
            .zip(first_classes.iter())
            .all(|(second, first)| second != first),
        "case \"a page bound\": the second page starts beyond the first, got \
         {first_classes:?} then {second_classes:?}"
    );

    // `Engine` resolves which catalogue is reported, so a value naming no engine this model
    // publishes is refused rather than answered with an empty catalogue.
    let refused = docdb
        .call(
            "DescribeOrderableDBInstanceOptions",
            &[("Engine", "mysql")],
        )
        .await;
    assert_eq!(
        (refused.status, error_code(&refused.xml).as_str()),
        (400, VALIDATION_CODE),
        "case \"another engine\": expected the request to be refused, got {} with {}",
        refused.status,
        refused.xml
    );
}

// ---------------------------------------------------------------------------
// Cause 6: the parameter metadata a family publishes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn published_parameters_report_metadata_scoped_to_the_group_family() {
    let docdb = Docdb::new();

    // Every family this service publishes, crossed with every parameter that family
    // publishes: the reported metadata is the catalogue's, and `MinimumEngineVersion` is
    // the earliest version the parameter can apply at INSIDE that family — the later of the
    // version the parameter appeared in and the family's own first release.
    for family in state::DOCDB_PARAMETER_GROUP_FAMILIES {
        let group = state::default_parameter_group_name(family);
        let reported = docdb
            .ok(
                family,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", &group),
                    ("Source", "engine-default"),
                ],
            )
            .await;
        let published =
            state::published_parameters(family).expect("every published family has a catalogue");
        assert_eq!(
            count(&reported, "Parameter"),
            published.len(),
            "case {family:?}: expected the family's whole published catalogue, got {reported}"
        );
        for spec in published {
            let block = parameter_block(&reported, spec.name);
            let case = format!("{family}/{}", spec.name);
            assert_eq!(
                (
                    text(&block, "Description"),
                    text(&block, "AllowedValues"),
                    text(&block, "ApplyType"),
                    text(&block, "DataType"),
                    text(&block, "ParameterValue"),
                    text(&block, "MinimumEngineVersion"),
                ),
                (
                    spec.description.to_string(),
                    spec.allowed_values.to_string(),
                    spec.apply_type.to_string(),
                    spec.data_type.to_string(),
                    spec.default_value.to_string(),
                    state::minimum_engine_version_in_family(family, spec),
                ),
                "case {case:?}: expected the metadata the family publishes, got {block}"
            );
            // The family-scoped rule itself: the reported minimum is never earlier than the
            // family's first release, and never earlier than the version the parameter
            // appeared in.
            let reported_minimum = text(&block, "MinimumEngineVersion");
            let earliest = state::family_earliest_engine_version(family)
                .expect("a published family names its first release");
            assert!(
                !state::engine_version_precedes(&reported_minimum, &earliest)
                    && !state::engine_version_precedes(
                        &reported_minimum,
                        spec.minimum_engine_version
                    ),
                "case {case:?}: expected a minimum at or after both {earliest} and {}, got \
                 {reported_minimum}",
                spec.minimum_engine_version
            );
        }
    }

    // The same metadata reaches a caller-created group of the family, through the parameter
    // it set: a create, a modify and the describe that reads it back.
    let family = state::DOCDB_PARAMETER_GROUP_FAMILIES[state::DOCDB_PARAMETER_GROUP_FAMILIES.len() - 1];
    docdb
        .ok(
            "create a group",
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "catalog-probe-cpg"),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await;
    docdb
        .ok(
            "set tls",
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "catalog-probe-cpg"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
            ],
        )
        .await;
    let read_back = docdb
        .ok(
            "read tls back",
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", "catalog-probe-cpg")],
        )
        .await;
    let tls = state::published_parameter(family, "tls").expect("the family publishes tls");
    assert_eq!(
        (
            text(&read_back, "ParameterName"),
            text(&read_back, "ParameterValue"),
            text(&read_back, "Description"),
            text(&read_back, "AllowedValues"),
            text(&read_back, "MinimumEngineVersion"),
        ),
        (
            "tls".to_string(),
            "disabled".to_string(),
            tls.description.to_string(),
            tls.allowed_values.to_string(),
            state::minimum_engine_version_in_family(family, tls),
        ),
        "case \"read tls back\": a caller-created group reports the family's published \
         metadata beside the value it was given, got {read_back}"
    );
}
