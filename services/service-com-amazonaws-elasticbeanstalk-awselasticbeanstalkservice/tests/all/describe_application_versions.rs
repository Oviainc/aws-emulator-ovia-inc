// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationVersions` over this service's awsQuery HTTP surface.
//!
//! The rule under test: the operation REPORTS the application versions the store
//! holds — every stored version whose `ApplicationName` and `VersionLabel` satisfy
//! each filter the request supplies, newest created first, cut into pages of
//! `MaxRecords` that `NextToken` resumes — and changes nothing.
//!
//! Each table below spans that class rather than one recorded request: the rows
//! differ in how many versions an application holds, in the form and the length of
//! the labels, in which optional members were stored, in which filter is applied,
//! and in the page size. The harness (`Emulator`, `Answer`) is the one in
//! `lifecycle.rs`, so these tests drive the same emulator over the same wire.

use crate::lifecycle::{account, Emulator, TEST_REGION};

/// One application and the version labels created under it, oldest first.
struct ApplicationFixture {
    application: &'static str,
    labels: Vec<String>,
}

/// The applications and versions every table below is asserted against.
///
/// `v1` is deliberately reused under two applications: a version label is unique
/// only WITHIN an application, so a `VersionLabels` filter on it must report both.
fn fixtures() -> Vec<ApplicationFixture> {
    vec![
        ApplicationFixture {
            application: "one-version",
            labels: vec!["v1".to_string()],
        },
        ApplicationFixture {
            application: "many-versions",
            labels: vec![
                "v1".to_string(),
                "v2".to_string(),
                "2.1.0-rc.1+build".to_string(),
                // `VersionLabel` is modelled `len 1..100`; this is the published
                // upper bound (smithy_query shape="VersionLabel").
                "l".repeat(100),
            ],
        },
        ApplicationFixture {
            application: "no-versions",
            labels: Vec::new(),
        },
    ]
}

/// Create every fixture, and answer the `(application, label)` pairs in the order
/// they were created.
async fn seed(emulator: &Emulator) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for fixture in fixtures() {
        let application = fixture.application;
        let answer = emulator
            .call("CreateApplication", &[("ApplicationName", application)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {application:?}: expected 200 from CreateApplication, got {} and body {}",
            answer.status, answer.body
        );
        for label in &fixture.labels {
            let answer = emulator
                .call(
                    "CreateApplicationVersion",
                    &[
                        ("ApplicationName", application),
                        ("VersionLabel", label.as_str()),
                        ("Description", "seeded"),
                        ("SourceBundle.S3Bucket", "bundles"),
                        ("SourceBundle.S3Key", "app.zip"),
                    ],
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {application:?}/{label:?}: expected 200 from CreateApplicationVersion, \
                 got {} and body {}",
                answer.status, answer.body
            );
            created.push((application.to_string(), label.clone()));
        }
    }
    created
}

/// The reported versions as `application/label`, in the order the answer lists
/// them. A label identifies a version only together with its application, so the
/// two members are read pairwise.
fn reported(answer: &crate::lifecycle::Answer) -> Vec<String> {
    let applications = answer.values_at("ApplicationVersions/member/ApplicationName");
    let labels = answer.values_at("ApplicationVersions/member/VersionLabel");
    assert_eq!(
        applications.len(),
        labels.len(),
        "every reported version carries both an ApplicationName and a VersionLabel, got {}",
        answer.body
    );
    applications
        .into_iter()
        .zip(labels)
        .map(|(application, label)| format!("{application}/{label}"))
        .collect()
}

/// `application/label` for a `(application, label)` pair.
fn qualified(pair: &(String, String)) -> String {
    format!("{}/{}", pair.0, pair.1)
}

#[tokio::test]
async fn described_application_versions_are_the_stored_versions_newest_first() {
    let emulator = Emulator::new();
    let created = seed(&emulator).await;
    // "sorted in order of creation": the most recently created version first.
    let newest_first: Vec<String> = created.iter().rev().map(qualified).collect();

    let all = emulator.call("DescribeApplicationVersions", &[]).await;
    assert_eq!(
        all.status, 200,
        "case \"no filter\": expected 200, got {} and body {}",
        all.status, all.body
    );
    assert_eq!(
        reported(&all),
        newest_first,
        "case \"no filter\": every stored version, newest first, got {}",
        all.body
    );

    // A read must not change what a later caller sees.
    let again = emulator.call("DescribeApplicationVersions", &[]).await;
    assert_eq!(
        reported(&again),
        newest_first,
        "case \"repeated read\": the same answer, got {}",
        again.body
    );

    // One row per filter the operation declares, plus the filters that match
    // nothing and the label shared by two applications.
    struct FilterCase {
        case: &'static str,
        params: Vec<(&'static str, String)>,
        expected: Vec<String>,
    }
    let long_label = "l".repeat(100);
    let long = format!("many-versions/{long_label}");
    let cases = vec![
        FilterCase {
            case: "ApplicationName with one version",
            params: vec![("ApplicationName", "one-version".to_string())],
            expected: vec!["one-version/v1".to_string()],
        },
        FilterCase {
            case: "ApplicationName with several versions",
            params: vec![("ApplicationName", "many-versions".to_string())],
            expected: vec![
                long.clone(),
                "many-versions/2.1.0-rc.1+build".to_string(),
                "many-versions/v2".to_string(),
                "many-versions/v1".to_string(),
            ],
        },
        FilterCase {
            case: "ApplicationName with no versions",
            params: vec![("ApplicationName", "no-versions".to_string())],
            expected: vec![],
        },
        FilterCase {
            case: "ApplicationName that does not exist",
            params: vec![("ApplicationName", "never-created".to_string())],
            expected: vec![],
        },
        FilterCase {
            case: "VersionLabels shared by two applications",
            params: vec![("VersionLabels.member.1", "v1".to_string())],
            expected: vec![
                "many-versions/v1".to_string(),
                "one-version/v1".to_string(),
            ],
        },
        FilterCase {
            case: "several VersionLabels",
            params: vec![
                ("VersionLabels.member.1", "v2".to_string()),
                ("VersionLabels.member.2", long_label.clone()),
            ],
            expected: vec![long.clone(), "many-versions/v2".to_string()],
        },
        FilterCase {
            case: "ApplicationName and VersionLabels together",
            params: vec![
                ("ApplicationName", "one-version".to_string()),
                ("VersionLabels.member.1", "v1".to_string()),
            ],
            expected: vec!["one-version/v1".to_string()],
        },
        FilterCase {
            case: "VersionLabels of another application",
            params: vec![
                ("ApplicationName", "one-version".to_string()),
                ("VersionLabels.member.1", "v2".to_string()),
            ],
            expected: vec![],
        },
        FilterCase {
            case: "VersionLabels that nothing carries",
            params: vec![("VersionLabels.member.1", "v99".to_string())],
            expected: vec![],
        },
    ];
    for FilterCase {
        case,
        params,
        expected,
    } in &cases
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = emulator
            .call("DescribeApplicationVersions", &params)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer),
            *expected,
            "case {case:?}: unexpected versions in {}",
            answer.body
        );
        assert_eq!(
            answer.value("NextToken"),
            None,
            "case {case:?}: an unpaginated answer carries no NextToken, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_described_version_carries_what_was_stored() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "store")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );

    /// One row per combination of optional members a version can be stored with.
    struct StoredCase {
        case: &'static str,
        label: &'static str,
        description: Option<&'static str>,
        bundle: Option<(&'static str, &'static str)>,
    }
    let cases = [
        StoredCase {
            case: "description and source bundle",
            label: "full",
            description: Some("the whole thing"),
            bundle: Some(("bundles", "full.zip")),
        },
        StoredCase {
            case: "source bundle only",
            label: "bundle-only",
            description: None,
            bundle: Some(("other-bucket", "nested/key.zip")),
        },
        StoredCase {
            case: "neither",
            label: "bare",
            description: None,
            bundle: None,
        },
    ];
    for case in &cases {
        let mut params = vec![
            ("ApplicationName", "store"),
            ("VersionLabel", case.label),
        ];
        if let Some(description) = case.description {
            params.push(("Description", description));
        }
        if let Some((bucket, key)) = case.bundle {
            params.push(("SourceBundle.S3Bucket", bucket));
            params.push(("SourceBundle.S3Key", key));
        }
        let created = emulator.call("CreateApplicationVersion", &params).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
            case.case, created.status, created.body
        );
    }

    for case in &cases {
        let described = emulator
            .call(
                "DescribeApplicationVersions",
                &[
                    ("ApplicationName", "store"),
                    ("VersionLabels.member.1", case.label),
                ],
            )
            .await;
        let name = case.case;
        assert_eq!(
            described.values_at("ApplicationVersions/member/VersionLabel"),
            vec![case.label],
            "case {name:?}: expected exactly the version asked for, got {}",
            described.body
        );
        assert_eq!(
            described.value("ApplicationVersions/member/ApplicationVersionArn"),
            Some(
                format!(
                    "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/store/{}",
                    account(),
                    case.label
                )
                .as_str()
            ),
            "case {name:?}: unexpected ApplicationVersionArn in {}",
            described.body
        );
        assert_eq!(
            described.value("ApplicationVersions/member/Status"),
            Some("UNPROCESSED"),
            "case {name:?}: a version nothing processed is UNPROCESSED, got {}",
            described.body
        );
        assert!(
            described
                .value("ApplicationVersions/member/DateCreated")
                .is_some(),
            "case {name:?}: expected a DateCreated, got {}",
            described.body
        );
        assert_eq!(
            described.value("ApplicationVersions/member/Description"),
            case.description,
            "case {name:?}: the stored Description must come back, got {}",
            described.body
        );
        // Fully scoped under the reported member, so the assertion stays
        // precise even when an answer holds more than one version. A version
        // created with NEITHER a source bundle nor source build information
        // carries the sample application's location: "If neither SourceBundle
        // nor SourceBuildInformation are provided, Elastic Beanstalk uses a
        // sample application."
        let expected_bundle = case.bundle.map_or_else(
            || {
                (
                    format!("elasticbeanstalk-{TEST_REGION}"),
                    "GenericSampleApplication".to_string(),
                )
            },
            |(bucket, key)| (bucket.to_string(), key.to_string()),
        );
        assert_eq!(
            described.value("ApplicationVersions/member/SourceBundle/S3Bucket"),
            Some(expected_bundle.0.as_str()),
            "case {name:?}: the stored S3Bucket must come back, got {}",
            described.body
        );
        assert_eq!(
            described.value("ApplicationVersions/member/SourceBundle/S3Key"),
            Some(expected_bundle.1.as_str()),
            "case {name:?}: the stored S3Key must come back, got {}",
            described.body
        );
    }

    // The report answers from the store: deleting the application takes its
    // versions out of the answer.
    assert_eq!(
        emulator
            .call("DeleteApplication", &[("ApplicationName", "store")])
            .await
            .status,
        200,
        "expected 200 from DeleteApplication"
    );
    let after = emulator
        .call("DescribeApplicationVersions", &[])
        .await;
    assert!(
        after
            .values_at("ApplicationVersions/member/VersionLabel")
            .is_empty(),
        "case \"after DeleteApplication\": the deleted application's versions must be gone, got {}",
        after.body
    );
}

#[tokio::test]
async fn describe_application_versions_pages_in_a_stable_order() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "paged")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    let labels: Vec<String> = (1..=7).map(|n| format!("v{n}")).collect();
    for label in &labels {
        let created = emulator
            .call(
                "CreateApplicationVersion",
                &[("ApplicationName", "paged"), ("VersionLabel", label.as_str())],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
            created.status, created.body
        );
    }
    let newest_first: Vec<String> = labels.iter().rev().cloned().collect();

    // Page sizes spanning the class: one record a page, a size that does not
    // divide the total, the exact total, and more than the total. Each row
    // names the number of pages the walk must take, so an extra empty page
    // with a continuation token fails the row.
    for (page_size, expected_pages) in [(1usize, 7usize), (3, 3), (7, 1), (20, 1)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let size = page_size.to_string();
            let mut params = vec![("MaxRecords", size.as_str())];
            if let Some(token) = token.as_deref() {
                params.push(("NextToken", token));
            }
            let answer = emulator
                .call("DescribeApplicationVersions", &params)
                .await;
            assert_eq!(
                answer.status, 200,
                "case \"MaxRecords={page_size}\": expected 200, got {} and body {}",
                answer.status, answer.body
            );
            let page: Vec<String> = answer
                .values_at("ApplicationVersions/member/VersionLabel")
                .into_iter()
                .map(str::to_string)
                .collect();
            assert!(
                page.len() <= page_size && !page.is_empty(),
                "case \"MaxRecords={page_size}\": a page holds 1..={page_size} versions, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= labels.len() + 1,
                "case \"MaxRecords={page_size}\": pagination did not terminate, saw {seen:?}"
            );
            match answer.value("NextToken") {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, newest_first,
            "case \"MaxRecords={page_size}\": the pages must concatenate to the whole list"
        );
        assert_eq!(
            pages, expected_pages,
            "case \"MaxRecords={page_size}\": expected {expected_pages} page(s) for {seen:?}"
        );
    }

    // `MaxRecords` is modelled `range 1..1000`: both published bounds are
    // accepted and a value past either is refused at the boundary.
    for (case, max_records) in [("lower bound", "1"), ("upper bound", "1000")] {
        let answer = emulator
            .call("DescribeApplicationVersions", &[("MaxRecords", max_records)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: MaxRecords={max_records} is in range, got {} and body {}",
            answer.status, answer.body
        );
    }
    for (case, max_records) in [("below the range", "0"), ("above the range", "1001")] {
        let answer = emulator
            .call("DescribeApplicationVersions", &[("MaxRecords", max_records)])
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: MaxRecords={max_records} must be refused, got {} and body {}",
            answer.status, answer.body
        );
    }

    // A token that names no version of the current answer is refused rather
    // than silently answering from the first page.
    let refused = emulator
        .call(
            "DescribeApplicationVersions",
            &[("MaxRecords", "2"), ("NextToken", "not-a-token")],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"invalid NextToken\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"invalid NextToken\": unexpected error code in {}",
        refused.body
    );
}
