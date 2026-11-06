// SPDX-License-Identifier: CC0-1.0

//! The HSM configuration lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created HSM configuration is reported and stored exactly as the
//! request configured it, whatever the shape of its identifier, description,
//! HSM address, partition name or certificate text; the two secret-bearing
//! request members reach no response, because the `HsmConfiguration` shape
//! models no member for either, while the store keeps them verbatim; the
//! IDENTIFIER being what an account's configurations are unique on, so a second
//! create under one identifier is refused and overwrites nothing; an identifier
//! that names no configuration being refused by the read; the creation order,
//! the `TagKeys` / `TagValues` filter, the service's `MaxRecords` window and its
//! `Marker` cursor; and the documented tag limit and the reserved tag prefix
//! being refused with nothing stored.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The members of an `HsmConfiguration` this suite compares between a create's
/// answer and a later read's, so the two records cannot disagree.
///
/// Source: `smithy_query(query="shape", shape="HsmConfiguration")` — every
/// member of the shape except `Tags`, which is a list and is compared on its
/// own.
const CONFIGURATION_MEMBERS: [&str; 4] = [
    "HsmConfigurationIdentifier",
    "Description",
    "HsmIpAddress",
    "HsmPartitionName",
];

/// The two `CreateHsmConfiguration` request members the `HsmConfiguration`
/// shape has NO member for, so neither may appear in any response.
///
/// Source: `smithy_query(query="operation", operation="CreateHsmConfiguration")`
/// against `smithy_query(query="shape", shape="HsmConfiguration")`.
const SECRET_MEMBERS: [&str; 2] = ["HsmServerPublicCertificate", "HsmPartitionPassword"];

/// One row of the create table: everything a `CreateHsmConfiguration` request
/// carries.
struct CreateCase {
    case: &'static str,
    identifier: String,
    description: String,
    ip_address: &'static str,
    partition_name: &'static str,
    certificate: String,
    password: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// `CreateHsmConfiguration` with its six required members plus `tags`.
async fn create_configuration(emulator: &Emulator, row: &CreateCase) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        (
            "HsmConfigurationIdentifier".to_string(),
            row.identifier.clone(),
        ),
        ("Description".to_string(), row.description.clone()),
        ("HsmIpAddress".to_string(), row.ip_address.to_string()),
        (
            "HsmPartitionName".to_string(),
            row.partition_name.to_string(),
        ),
        (
            "HsmPartitionPassword".to_string(),
            row.password.to_string(),
        ),
        (
            "HsmServerPublicCertificate".to_string(),
            row.certificate.clone(),
        ),
    ];
    for (position, (key, value)) in row.tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    call_with(emulator, "CreateHsmConfiguration", &params).await
}

/// Drive one action with owned parameters, so a case can build its values.
async fn call_with(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// A minimal create row: the identifier and certificate vary, everything else
/// is one fixed configuration.
fn simple_case(case: &'static str, identifier: &str) -> CreateCase {
    CreateCase {
        case,
        identifier: identifier.to_string(),
        description: format!("HSM connection {identifier}"),
        ip_address: "192.0.2.10",
        partition_name: "redshift-partition",
        certificate: "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----\n"
            .to_string(),
        password: "partition-secret",
        tags: Vec::new(),
    }
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Every compared member of the single configuration a body carries.
fn configuration_members(body: &str) -> Vec<(&'static str, Option<String>)> {
    CONFIGURATION_MEMBERS
        .into_iter()
        .map(|member| (member, all_text(body, member).into_iter().next()))
        .collect()
}

/// The `<HsmConfigurationIdentifier>` of every configuration a body carries, in
/// the order the page lists them.
fn listed_configurations(body: &str) -> Vec<String> {
    all_text(body, "HsmConfigurationIdentifier")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// `DescribeHsmConfigurations` for one identifier.
async fn describe_one(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeHsmConfigurations",
            &[("HsmConfigurationIdentifier", identifier)],
        )
        .await
}

/// The certificate and partition password the store holds for one identifier.
fn stored_secrets(emulator: &Emulator, identifier: &str) -> Option<(String, String)> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .hsm_configuration(identifier)
        .map(|stored| {
            (
                stored.hsm_server_public_certificate.clone(),
                stored.hsm_partition_password.clone(),
            )
        })
}

/// An identifier of `len` characters, for a row that varies only in size.
fn identifier_of_length(len: usize) -> String {
    debug_assert!(len >= 1, "an identifier of length {len} has no first character");
    std::iter::once('h')
        .chain(std::iter::repeat_n('x', len.saturating_sub(1)))
        .collect()
}

/// A create reports the configuration the request describes, stores it for every
/// later read, and keeps the certificate and the partition password off the
/// wire.
///
/// The rows differ along everything the rule generalises over: the form and size
/// of the identifier, the characters and length of the description, the family
/// of the HSM address, the partition name, the shape of the certificate text
/// (one line, a multi-line PEM, one carrying XML metacharacters) and how many
/// tags the request carries.
#[tokio::test]
async fn a_create_reports_and_stores_what_the_request_describes() {
    let emulator = Emulator::new();

    let rows = vec![
        simple_case("the minimal configuration", "hsm-one"),
        CreateCase {
            case: "an identifier with digits and hyphens",
            identifier: "hsm-2-east-01".to_string(),
            description: "second HSM connection".to_string(),
            ip_address: "198.51.100.7",
            partition_name: "partition-2",
            certificate: "QUJDREVG".to_string(),
            password: "second-secret",
            tags: vec![("owner", "admin")],
        },
        CreateCase {
            case: "an identifier in mixed case",
            identifier: "HSM-Mixed-Case".to_string(),
            description: "Mixed case identifier".to_string(),
            ip_address: "203.0.113.42",
            partition_name: "Partition_3",
            certificate: "-----BEGIN CERTIFICATE-----\nR0hJ\nSktM\n-----END CERTIFICATE-----\n"
                .to_string(),
            password: "third-secret",
            tags: vec![("owner", "admin"), ("environment", "test")],
        },
        CreateCase {
            case: "an IPv6 HSM address and a certificate with XML metacharacters",
            identifier: "hsm-ipv6".to_string(),
            // The description and the certificate carry `<`, `>` and `&`, so a
            // round trip also proves the envelope escapes and unescapes them.
            description: "cluster <keys> & more".to_string(),
            ip_address: "2001:db8::1",
            partition_name: "partition-ipv6",
            certificate: "<cert>A&B</cert>".to_string(),
            password: "ipv6-secret",
            tags: vec![("environment", "production")],
        },
        CreateCase {
            case: "a long identifier, description and certificate",
            identifier: identifier_of_length(64),
            description: "d".repeat(512),
            ip_address: "192.0.2.255",
            partition_name: "partition-long",
            certificate: "Q".repeat(4096),
            password: "long-secret",
            tags: vec![("owner", "fleet")],
        },
        CreateCase {
            case: "an empty description",
            identifier: "hsm-empty-description".to_string(),
            // `Description` is required but modelled `len 0..`, so the empty
            // string is a value the service takes and must report back.
            description: String::new(),
            ip_address: "192.0.2.11",
            partition_name: "partition-empty",
            certificate: "QQ".to_string(),
            password: "empty-secret",
            tags: Vec::new(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let created = create_configuration(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );

        let expected: Vec<(&'static str, Option<String>)> = vec![
            (
                "HsmConfigurationIdentifier",
                Some(row.identifier.clone()),
            ),
            ("Description", Some(row.description.clone())),
            ("HsmIpAddress", Some(row.ip_address.to_string())),
            ("HsmPartitionName", Some(row.partition_name.to_string())),
        ];
        assert_eq!(
            configuration_members(&created.body),
            expected,
            "case {case}: the create reported {:?} with body {}",
            configuration_members(&created.body),
            created.body
        );

        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&created.body),
            expected_tags,
            "case {case}: expected tags {expected_tags:?}, got {:?}",
            listed_tags(&created.body)
        );

        // Neither secret-bearing member is modelled on `HsmConfiguration`, so
        // neither the element nor the value may appear in the answer.
        for member in SECRET_MEMBERS {
            assert!(
                all_text(&created.body, member).is_empty(),
                "case {case}: expected no <{member}> in the answer, got body {}",
                created.body
            );
        }
        assert!(
            !created.body.contains(row.password),
            "case {case}: the partition password reached the answer: {}",
            created.body
        );

        // The round trip: the same record every later read reports.
        let read = describe_one(&emulator, &row.identifier).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the created configuration to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            configuration_members(&read.body),
            expected,
            "case {case}: the read reported {:?}, not what the create did",
            configuration_members(&read.body)
        );
        assert_eq!(
            listed_tags(&read.body),
            expected_tags,
            "case {case}: the read reported tags {:?}, not what the create did",
            listed_tags(&read.body)
        );
        for member in SECRET_MEMBERS {
            assert!(
                all_text(&read.body, member).is_empty(),
                "case {case}: expected no <{member}> in the read, got body {}",
                read.body
            );
        }

        // The store keeps both secret-bearing values verbatim: the certificate
        // text a cluster connects to the HSM with is preserved exactly, newlines
        // and metacharacters included.
        let secrets = stored_secrets(&emulator, &row.identifier);
        assert_eq!(
            secrets,
            Some((row.certificate.clone(), row.password.to_string())),
            "case {case}: expected the certificate and password stored verbatim, got {secrets:?}"
        );
    }

    // Every row is one record of the account, in creation order.
    let listed = emulator.call("DescribeHsmConfigurations", &[]).await;
    let expected_order: Vec<String> = rows.iter().map(|row| row.identifier.clone()).collect();
    assert_eq!(
        listed_configurations(&listed.body),
        expected_order,
        "case the whole account: expected the creation order {expected_order:?}, got {:?}",
        listed_configurations(&listed.body)
    );
}

/// The IDENTIFIER is what an account's HSM configurations are unique on, so a
/// second create under one identifier is refused and overwrites nothing.
#[tokio::test]
async fn a_second_create_of_one_identifier_is_refused_and_overwrites_nothing() {
    let emulator = Emulator::new();

    for (case, identifier) in [
        ("a plain identifier", "hsm-dup"),
        ("an identifier with digits", "hsm-dup-2"),
        ("a mixed-case identifier", "HSM-Dup"),
    ] {
        let first = simple_case(case, identifier);
        assert_eq!(
            create_configuration(&emulator, &first).await.status,
            200,
            "case {case}: the first create must be served"
        );

        // A second create differing in every member BUT the identifier: the
        // identifier alone decides, and the stored record is untouched.
        let second = CreateCase {
            case,
            identifier: identifier.to_string(),
            description: "a different description".to_string(),
            ip_address: "203.0.113.99",
            partition_name: "other-partition",
            certificate: "T1RIRVI=".to_string(),
            password: "other-secret",
            tags: vec![("owner", "someone-else")],
        };
        let answer = create_configuration(&emulator, &second).await;
        assert_refused(case, &answer, 400, "HsmConfigurationAlreadyExistsFault");

        let read = describe_one(&emulator, identifier).await;
        assert_eq!(
            configuration_members(&read.body),
            vec![
                (
                    "HsmConfigurationIdentifier",
                    Some(identifier.to_string())
                ),
                ("Description", Some(first.description.clone())),
                ("HsmIpAddress", Some(first.ip_address.to_string())),
                ("HsmPartitionName", Some(first.partition_name.to_string())),
            ],
            "case {case}: the refused create changed the stored record: {}",
            read.body
        );
        assert_eq!(
            stored_secrets(&emulator, identifier),
            Some((first.certificate.clone(), first.password.to_string())),
            "case {case}: the refused create replaced the stored certificate or password"
        );
        assert_eq!(
            listed_configurations(&read.body).len(),
            1,
            "case {case}: expected exactly one record for the identifier, got {:?}",
            listed_configurations(&read.body)
        );
    }
}

/// An `HsmConfigurationIdentifier` that names no configuration is refused by the
/// read, while naming none reports every configuration the account has.
#[tokio::test]
async fn reading_an_identifier_that_names_no_configuration_is_refused() {
    let emulator = Emulator::new();

    // On an empty account every identifier names nothing.
    for (case, identifier) in [
        ("an empty account", "hsm-absent"),
        ("an identifier with separators", "hsm-absent-2"),
    ] {
        let answer = describe_one(&emulator, identifier).await;
        assert_refused(case, &answer, 400, "HsmConfigurationNotFoundFault");
    }

    let present = simple_case("a stored configuration", "hsm-present");
    assert_eq!(
        create_configuration(&emulator, &present).await.status,
        200,
        "case a stored configuration: the create must be served"
    );

    // One stored configuration does not make a different identifier resolve,
    // and the match is exact: a differently-cased spelling names no record.
    for (case, identifier) in [
        ("a neighbouring identifier", "hsm-present-2"),
        ("a prefix of the stored identifier", "hsm-pres"),
        ("the stored identifier in another case", "HSM-PRESENT"),
    ] {
        let answer = describe_one(&emulator, identifier).await;
        assert_refused(case, &answer, 400, "HsmConfigurationNotFoundFault");
    }

    let listed = emulator.call("DescribeHsmConfigurations", &[]).await;
    assert_eq!(
        listed_configurations(&listed.body),
        vec!["hsm-present".to_string()],
        "case naming no identifier: expected the one stored configuration, got {:?}",
        listed_configurations(&listed.body)
    );
}

/// The `TagKeys` / `TagValues` filter selects CONFIGURATIONS: a configuration
/// one of its tags matches reports every tag it carries, and a configuration
/// with no matching tag is left out.
#[tokio::test]
async fn the_tag_filter_selects_whole_configurations() {
    let emulator = Emulator::new();

    for (identifier, tags) in [
        ("hsm-owner-admin", vec![("owner", "admin")]),
        (
            "hsm-owner-admin-test",
            vec![("owner", "admin"), ("environment", "test")],
        ),
        ("hsm-environment-prod", vec![("environment", "production")]),
        ("hsm-untagged", vec![]),
    ] {
        let row = CreateCase {
            case: identifier,
            tags,
            ..simple_case(identifier, identifier)
        };
        assert_eq!(
            create_configuration(&emulator, &row).await.status,
            200,
            "case {identifier}: the create must be served"
        );
    }

    for (case, filter, expected) in [
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            vec!["hsm-owner-admin", "hsm-owner-admin-test"],
        ),
        (
            "one tag value",
            vec![("TagValues.TagValue.1", "production")],
            vec!["hsm-environment-prod"],
        ),
        (
            "two tag keys",
            vec![
                ("TagKeys.TagKey.1", "owner"),
                ("TagKeys.TagKey.2", "environment"),
            ],
            vec![
                "hsm-owner-admin",
                "hsm-owner-admin-test",
                "hsm-environment-prod",
            ],
        ),
        (
            "a key no configuration carries",
            vec![("TagKeys.TagKey.1", "absent")],
            vec![],
        ),
        (
            "no filter at all",
            vec![],
            vec![
                "hsm-owner-admin",
                "hsm-owner-admin-test",
                "hsm-environment-prod",
                "hsm-untagged",
            ],
        ),
    ] {
        let answer = emulator.call("DescribeHsmConfigurations", &filter).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        assert_eq!(
            listed_configurations(&answer.body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed_configurations(&answer.body)
        );
    }

    // A selected configuration reports EVERY tag it carries, not only the tag
    // that matched.
    let answer = emulator
        .call(
            "DescribeHsmConfigurations",
            &[
                ("HsmConfigurationIdentifier", "hsm-owner-admin-test"),
                ("TagKeys.TagKey.1", "owner"),
            ],
        )
        .await;
    assert_eq!(
        listed_tags(&answer.body),
        vec![
            ("owner".to_string(), "admin".to_string()),
            ("environment".to_string(), "test".to_string()),
        ],
        "case a matched configuration: expected every tag it carries, got {:?}",
        listed_tags(&answer.body)
    );
}

/// `DescribeHsmConfigurations` hands out one page of the service's documented
/// `MaxRecords` window at a time, with a `Marker` cursor a later call resumes
/// the same sequence from.
///
/// The window is the one every paginated `Describe*` of this service shares
/// (`state::MAX_RECORDS_MIN` ..= `state::MAX_RECORDS_MAX`, applied by
/// `state::page_bounds`), tested on each bound and one past it.
#[tokio::test]
async fn configurations_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();

    let minimum = state::MAX_RECORDS_MIN;
    let maximum = state::MAX_RECORDS_MAX;
    let total = minimum as usize + 1;
    for ordinal in 0..total {
        let identifier = format!("hsm-page-{ordinal:03}");
        let row = simple_case("a paged configuration", &identifier);
        assert_eq!(
            create_configuration(&emulator, &row).await.status,
            200,
            "case {identifier}: the create must be served"
        );
    }

    for (case, max_records, accepted) in [
        ("one below the window", (minimum - 1).to_string(), false),
        ("at the window's minimum", minimum.to_string(), true),
        ("inside the window", ((minimum + maximum) / 2).to_string(), true),
        ("at the window's maximum", maximum.to_string(), true),
        ("one past the window, which is served", (maximum + 1).to_string(), true),
        ("zero", "0".to_string(), false),
    ] {
        let answer = call_with(
            &emulator,
            "DescribeHsmConfigurations",
            &[("MaxRecords".to_string(), max_records.clone())],
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // The default window takes every configuration in one page, with no cursor
    // left.
    let unpaged = emulator.call("DescribeHsmConfigurations", &[]).await;
    assert_eq!(
        listed_configurations(&unpaged.body).len(),
        total,
        "case the default window: expected all {total} configurations in one page, got {:?}",
        listed_configurations(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page at the window's minimum leaves a cursor the next call resumes
    // from, and the two pages are the unpaged sequence split in order.
    let page_size = minimum.to_string();
    let first_page = call_with(
        &emulator,
        "DescribeHsmConfigurations",
        &[("MaxRecords".to_string(), page_size.clone())],
    )
    .await;
    let first_names = listed_configurations(&first_page.body);
    assert_eq!(
        first_names.len(),
        minimum as usize,
        "case the first page: expected {minimum} configurations, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page: records remain, so the page carries a cursor");
    let second_page = call_with(
        &emulator,
        "DescribeHsmConfigurations",
        &[
            ("MaxRecords".to_string(), page_size.clone()),
            ("Marker".to_string(), marker),
        ],
    )
    .await;
    let second_names = listed_configurations(&second_page.body);
    assert_eq!(
        second_names.len(),
        total - minimum as usize,
        "case the second page: expected the remaining {} configurations, got {second_names:?}",
        total - minimum as usize
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked,
        listed_configurations(&unpaged.body),
        "case the walked pages: expected the unpaged sequence, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    for (case, cursor) in [
        ("not a cursor at all", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeHsmConfigurations", &[("Marker", cursor)])
            .await;
        assert_refused(case, &bogus, 400, "InvalidParameterValue");
    }
}

/// A create carrying more tags than one resource may hold, or a tag the service
/// reserves for itself, is refused and stores nothing.
#[tokio::test]
async fn refused_tags_store_no_configuration() {
    let emulator = Emulator::new();

    let quota = state::MAX_TAGS_PER_RESOURCE;
    let at_quota: Vec<(String, String)> = (0..quota)
        .map(|ordinal| (format!("key-{ordinal}"), format!("value-{ordinal}")))
        .collect();
    let past_quota: Vec<(String, String)> = (0..quota + 1)
        .map(|ordinal| (format!("key-{ordinal}"), format!("value-{ordinal}")))
        .collect();

    // One row of the tag table: the identifier the create names, the tags it
    // carries and the error code it must answer (`None` for a served create).
    struct TagCase {
        case: &'static str,
        identifier: &'static str,
        tags: Vec<(String, String)>,
        refusal: Option<&'static str>,
    }

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "at the documented tag limit",
            identifier: "hsm-tags-at-quota",
            tags: at_quota,
            refusal: None,
        },
        TagCase {
            case: "one tag past the limit",
            identifier: "hsm-tags-past-quota",
            tags: past_quota,
            refusal: Some("TagLimitExceededFault"),
        },
        TagCase {
            case: "a reserved tag key",
            identifier: "hsm-tags-reserved-key",
            tags: vec![("aws:owner".to_string(), "admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a reserved tag value",
            identifier: "hsm-tags-reserved-value",
            tags: vec![("owner".to_string(), "aws:admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a tag key past its length limit",
            identifier: "hsm-tags-long-key",
            tags: vec![("k".repeat(state::TAG_KEY_MAX_LEN + 1), "admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a tag value past its length limit",
            identifier: "hsm-tags-long-value",
            tags: vec![(
                "owner".to_string(),
                "v".repeat(state::TAG_VALUE_MAX_LEN + 1),
            )],
            refusal: Some("InvalidTagFault"),
        },
    ];

    let mut stored: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let mut params: Vec<(String, String)> = vec![
            (
                "HsmConfigurationIdentifier".to_string(),
                row.identifier.to_string(),
            ),
            ("Description".to_string(), format!("tags case {case}")),
            ("HsmIpAddress".to_string(), "192.0.2.12".to_string()),
            (
                "HsmPartitionName".to_string(),
                "redshift-partition".to_string(),
            ),
            ("HsmPartitionPassword".to_string(), "secret".to_string()),
            ("HsmServerPublicCertificate".to_string(), "QUJD".to_string()),
        ];
        for (position, (key, value)) in row.tags.iter().enumerate() {
            params.push((format!("Tags.Tag.{}.Key", position + 1), key.clone()));
            params.push((format!("Tags.Tag.{}.Value", position + 1), value.clone()));
        }
        let answer = call_with(&emulator, "CreateHsmConfiguration", &params).await;
        match row.refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with body {}",
                    answer.status, answer.body
                );
                stored.push(row.identifier.to_string());
            }
            Some(code) => assert_refused(case, &answer, 400, code),
        }
    }

    // Only the admitted row is stored: every refused create stored nothing.
    let listed = emulator.call("DescribeHsmConfigurations", &[]).await;
    assert_eq!(
        listed_configurations(&listed.body),
        stored,
        "case the whole account: expected only {stored:?}, got {:?}",
        listed_configurations(&listed.body)
    );
}

/// `DeleteHsmConfiguration` for one identifier.
async fn delete_one(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DeleteHsmConfiguration",
            &[("HsmConfigurationIdentifier", identifier)],
        )
        .await
}

/// A delete removes the configuration it names: every later read reports it
/// absent, the secret-bearing values it held leave the store with it, and the
/// configurations it did not name are untouched, in creation order.
///
/// The rows differ along everything the rule generalises over: the form and size
/// of the identifier, whether the configuration carries tags, and the position
/// the record holds in the account's creation order (first, middle, last).
#[tokio::test]
async fn a_delete_removes_the_configuration_it_names() {
    let emulator = Emulator::new();

    let rows = vec![
        simple_case("a plain identifier, deleted first", "hsm-delete-one"),
        CreateCase {
            case: "a tagged configuration in the middle of the order",
            identifier: "hsm-2-delete-01".to_string(),
            description: "tagged HSM connection".to_string(),
            ip_address: "198.51.100.8",
            partition_name: "partition-delete-2",
            certificate: "-----BEGIN CERTIFICATE-----\nR0hJ\n-----END CERTIFICATE-----\n"
                .to_string(),
            password: "delete-secret-2",
            tags: vec![("owner", "admin"), ("environment", "test")],
        },
        simple_case("a mixed-case identifier", "HSM-Delete-Mixed"),
        {
            let mut long = simple_case("a long identifier, deleted last", "placeholder");
            long.identifier = identifier_of_length(64);
            long
        },
    ];

    for row in &rows {
        assert_eq!(
            create_configuration(&emulator, row).await.status,
            200,
            "case {}: the create must be served before the delete is exercised",
            row.case
        );
    }

    // Deleting in creation order leaves, after each row, exactly the rows that
    // follow it — so the delete removes ONE record and reorders nothing.
    for (ordinal, row) in rows.iter().enumerate() {
        let case = row.case;
        let deleted = delete_one(&emulator, &row.identifier).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete to be served with 200, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            None,
            "case {case}: expected no fault in the delete's answer, got body {}",
            deleted.body
        );

        // Absent for every later read, and the store holds neither of the two
        // secret-bearing values any more.
        let read = describe_one(&emulator, &row.identifier).await;
        assert_refused(case, &read, 400, "HsmConfigurationNotFoundFault");
        assert!(
            read.text("Message")
                .is_some_and(|message| message.contains(&row.identifier)),
            "case {case}: expected the read's message to name {}, got {:?}",
            row.identifier,
            read.text("Message")
        );
        let secrets = stored_secrets(&emulator, &row.identifier);
        assert_eq!(
            secrets, None,
            "case {case}: expected the stored certificate and password to be gone, got {secrets:?}"
        );

        // A second delete of the same identifier names no configuration, so it
        // is refused exactly as a delete of one that never existed.
        let again = delete_one(&emulator, &row.identifier).await;
        assert_refused(case, &again, 400, "HsmConfigurationNotFoundFault");

        let expected_rest: Vec<String> = rows[ordinal + 1..]
            .iter()
            .map(|rest| rest.identifier.clone())
            .collect();
        let listed = emulator.call("DescribeHsmConfigurations", &[]).await;
        assert_eq!(
            listed_configurations(&listed.body),
            expected_rest,
            "case {case}: expected the remaining account to be {expected_rest:?}, got {:?}",
            listed_configurations(&listed.body)
        );
    }
}

/// An `HsmConfigurationIdentifier` that names no configuration is refused by the
/// delete, and the configurations the account does hold are untouched.
///
/// The rows differ along how the requested identifier misses: a name nothing
/// resembles, a case variant of a stored one, a prefix of it and a name carrying
/// one extra character — the identifier is compared exactly as it was stored.
#[tokio::test]
async fn deleting_an_identifier_that_names_no_configuration_is_refused() {
    let emulator = Emulator::new();

    let present = simple_case("the stored configuration", "hsm-present");
    assert_eq!(
        create_configuration(&emulator, &present).await.status,
        200,
        "the stored configuration must be created before the misses are exercised"
    );

    for (case, missing) in [
        ("an identifier nothing resembles", "hsm-absent"),
        ("an upper-case variant of a stored identifier", "HSM-PRESENT"),
        ("a prefix of a stored identifier", "hsm-pres"),
        ("a stored identifier with one extra character", "hsm-presentx"),
        ("an identifier with a trailing hyphen", "hsm-present-"),
    ] {
        let answer = delete_one(&emulator, missing).await;
        assert_refused(case, &answer, 400, "HsmConfigurationNotFoundFault");
        // The fixed text of the fault as this service words a DELETE's
        // not-found refusal — the resource, then the identifier the REQUEST
        // carried, which is its only variable part (the same shape as
        // `DeleteClusterParameterGroup`'s "ParameterGroup not found:
        // <name>"). The READ of the same identifier words the same fault its
        // own way, so both are pinned here.
        let expected = format!("HSM configuration not found: {missing}");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the message {expected:?}, got {:?}",
            answer.text("Message")
        );
        let expected_read = format!("HSM configuration '{missing}' not found.");
        assert_eq!(
            describe_one(&emulator, missing)
                .await
                .text("Message")
                .as_deref(),
            Some(expected_read.as_str()),
            "case {case}: expected the read's message {expected_read:?}"
        );
    }

    // Every refused delete removed nothing.
    let listed = emulator.call("DescribeHsmConfigurations", &[]).await;
    assert_eq!(
        listed_configurations(&listed.body),
        vec![present.identifier.clone()],
        "case the whole account: expected only {:?}, got {:?}",
        present.identifier,
        listed_configurations(&listed.body)
    );
    assert_eq!(
        stored_secrets(&emulator, &present.identifier),
        Some((present.certificate.clone(), present.password.to_string())),
        "case the whole account: a refused delete dropped the stored certificate or password"
    );
}

/// A configuration a cluster is configured from is still where that cluster
/// stores its database encryption keys, so the delete is refused and the record
/// stays readable; a configuration no cluster uses is removed.
///
/// The rows differ along how many clusters hold the configuration: none, one,
/// and more than one — "in use by one or more Amazon Redshift clusters".
#[tokio::test]
async fn a_configuration_a_cluster_uses_cannot_be_deleted() {
    let emulator = Emulator::new();

    for (case, identifier, clusters) in [
        ("no cluster uses it", "hsm-used-by-none", &[][..]),
        (
            "one cluster uses it",
            "hsm-used-by-one",
            &["hsm-cluster-a"][..],
        ),
        (
            "two clusters use it",
            "hsm-used-by-two",
            &["hsm-cluster-b", "hsm-cluster-c"][..],
        ),
    ] {
        assert_eq!(
            create_configuration(&emulator, &simple_case(case, identifier))
                .await
                .status,
            200,
            "case {case}: the create must be served"
        );
        for cluster in clusters {
            let created = emulator
                .call(
                    "CreateCluster",
                    &[
                        ("ClusterIdentifier", cluster),
                        ("NodeType", "ra3.xlplus"),
                        ("MasterUsername", "admin"),
                        ("HsmConfigurationIdentifier", identifier),
                    ],
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case}: the cluster {cluster} must be created, got {} with body {}",
                created.status, created.body
            );
        }

        let answer = delete_one(&emulator, identifier).await;
        if clusters.is_empty() {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the delete to be served, got {} with body {}",
                answer.status, answer.body
            );
            assert_refused(
                case,
                &describe_one(&emulator, identifier).await,
                400,
                "HsmConfigurationNotFoundFault",
            );
            continue;
        }

        assert_refused(case, &answer, 400, "InvalidHsmConfigurationStateFault");
        let message = answer.text("Message").unwrap_or_default();
        for cluster in clusters {
            assert!(
                message.contains(cluster),
                "case {case}: expected the message to name the cluster {cluster}, got {message:?}"
            );
        }

        // The refused delete removed nothing: the configuration is still the
        // record a read reports, secrets included.
        let read = describe_one(&emulator, identifier).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the configuration to still be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            listed_configurations(&read.body),
            vec![identifier.to_string()],
            "case {case}: expected the read to report {identifier}, got {:?}",
            listed_configurations(&read.body)
        );
        assert!(
            stored_secrets(&emulator, identifier).is_some(),
            "case {case}: the refused delete dropped the stored certificate and password"
        );
    }
}
