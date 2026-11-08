// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a package's CONTENT VERSIONS: `UpdatePackage`, which puts the
//! next numbered version of a package's content in place, and
//! `GetPackageVersionHistory`, the read path it is proved through.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The READ path has its own suite ([`super::package_version_history`], which seeds
//! versions directly to fix their instants): what this one asserts is the rule
//! `UpdatePackage` follows, proved through that read over a history built by real
//! update calls.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an update records the NEXT numbered version of the package's content and leaves
//!   the package copying it, over a table of four updates spanning the commit message
//!   absent and at both ends of its length constraint, each optional member carried
//!   and left out, and several Amazon S3 locations — a member the request leaves out
//!   changes nothing, and a version already recorded is never rewritten;
//! * the version the copy puts in place becomes the version the package reports as
//!   available, once the copy span has passed and without any call asking for it;
//! * a package whose copy is still in flight takes no second update, so concurrent
//!   updates of one package are serialised: exactly one of them records a version;
//! * an id that names no package of the account is refused on both paths, over the
//!   id forms the model's pattern admits;
//! * the history is reported newest version first and pages through the whole log
//!   once whatever page size is asked for, with the published page bound and one past
//!   it, and refuses a token that names no version of the package;
//! * a package's history is bounded: versions are recorded up to the quota and the
//!   one past it is refused.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PACKAGE_NOT_FOUND, PACKAGE_STATUS_AVAILABLE, PACKAGE_STATUS_COPYING,
    PACKAGE_TYPE_TXT_DICTIONARY, PACKAGE_VERSION_QUOTA,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// `CreatePackage`: `POST /2021-01-01/packages`.
async fn create(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages"), Some(body))
        .await
}

/// `UpdatePackage`: `POST /2021-01-01/packages/update`.
async fn update(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/update"), Some(body))
        .await
}

/// `GetPackageVersionHistory`: `GET /2021-01-01/packages/{PackageID}/history`, with
/// the query members the request carries and none it does not.
async fn history(emulator: &Emulator, package_id: &str, query: &[(&str, String)]) -> Answer {
    let mut uri = format!("{PREFIX}/packages/{package_id}/history");
    for (index, (name, value)) in query.iter().enumerate() {
        let separator = if index == 0 { '?' } else { '&' };
        uri.push(separator);
        uri.push_str(&format!("{name}={value}"));
    }
    emulator.call("GET", &uri, None).await
}

/// `DescribePackages` narrowed to one package: `POST /2021-01-01/packages/describe`.
async fn describe_one(emulator: &Emulator, package_id: &str, case: &str) -> Value {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/describe"),
            Some(json!({
                "Filters": [{ "Name": "PackageID", "Value": [package_id] }],
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from DescribePackages, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a PackageDetailsList, got {:?}", answer.body))
        .clone();
    assert_eq!(
        reported.len(),
        1,
        "case {case}: exactly the one package is reported, got {reported:?}"
    );
    reported[0].clone()
}

/// The `PackageDetails` of a successful answer, with the case named on failure.
fn details(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let details = &answer.body["PackageDetails"];
    assert!(
        details.is_object(),
        "case {case}: expected a PackageDetails object, got {:?}",
        answer.body
    );
    details.clone()
}

/// The `PackageVersionHistoryList` of a successful read, with the case named on
/// failure.
fn reported_history(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageVersionHistoryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageVersionHistoryList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The reported versions, in the order the history reported them.
fn reported_versions(answer: &Answer, case: &str) -> Vec<String> {
    reported_history(answer, case)
        .iter()
        .map(|entry| {
            entry["PackageVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a PackageVersion, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The create every case starts from: a dictionary package with a description and a
/// configuration, so an update that carries neither is observed to change neither.
fn create_body(name: &str) -> Value {
    json!({
        "PackageName": name,
        "PackageType": PACKAGE_TYPE_TXT_DICTIONARY,
        "PackageDescription": "synonyms of the catalogue",
        "PackageSource": { "S3BucketName": "package-bucket", "S3Key": "synonyms/v1.txt" },
        "PackageConfiguration": {
            "LicenseRequirement": "REQUIRED",
            "ConfigurationRequirement": "REQUIRED",
        },
    })
}

/// A created package whose first copy has landed, so it is available for an update.
async fn available_package(emulator: &Emulator, name: &str) -> String {
    let created = details(&create(emulator, create_body(name)).await, name);
    tokio::time::sleep(SETTLE_WAIT).await;
    created["PackageID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: expected a PackageID, got {created:?}"))
        .to_string()
}

/// An update that only replaces the package's content, from the Amazon S3 location
/// named.
fn source_only_update(package_id: &str, bucket: &str, key: &str) -> Value {
    json!({
        "PackageID": package_id,
        "PackageSource": { "S3BucketName": bucket, "S3Key": key },
    })
}

/// One update to make.
struct Row {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// `CommitMessage`, absent where `None`.
    commit_message: Option<String>,
    /// `PackageDescription`, absent where `None`.
    description: Option<&'static str>,
    /// `PackageConfiguration`, absent where `None`.
    configuration: Option<Value>,
    /// `PackageEncryptionOptions`, absent where `None`.
    encryption_options: Option<Value>,
    /// `PackageSource`: the bucket and key the content comes from.
    source: (&'static str, &'static str),
}

impl Row {
    /// This row as an `UpdatePackage` request body: every member the row carries, and
    /// none it does not.
    fn body(&self, package_id: &str) -> Value {
        let (bucket, key) = self.source;
        let mut body = json!({
            "PackageID": package_id,
            "PackageSource": { "S3BucketName": bucket, "S3Key": key },
        });
        if let Some(commit_message) = &self.commit_message {
            body["CommitMessage"] = json!(commit_message);
        }
        if let Some(description) = self.description {
            body["PackageDescription"] = json!(description);
        }
        if let Some(configuration) = &self.configuration {
            body["PackageConfiguration"] = configuration.clone();
        }
        if let Some(encryption_options) = &self.encryption_options {
            body["PackageEncryptionOptions"] = encryption_options.clone();
        }
        body
    }
}

/// One row per way an update differs, along what the rule generalises over: whether a
/// commit message is carried and its length at both ends of the modelled constraint
/// (`CommitMessage` is `@length(min: 0, max: 160)`), whether each optional member is
/// carried or left out, and the Amazon S3 location the content comes from. The rows
/// are applied in order to ONE package, so each one is also a case of "the version
/// before it is left exactly as it was".
fn update_rows() -> Vec<Row> {
    vec![
        // Nothing optional at all: the content is replaced and nothing else changes.
        Row {
            case: "nothing but the source",
            commit_message: None,
            description: None,
            configuration: None,
            encryption_options: None,
            source: ("package-bucket", "synonyms/v2.txt"),
        },
        // The shortest commit message the constraint allows, beside a new description.
        Row {
            case: "an empty commit message and a new description",
            commit_message: Some(String::new()),
            description: Some("synonyms of the catalogue, revised"),
            configuration: None,
            encryption_options: None,
            source: ("package-bucket", "synonyms/v3.txt"),
        },
        // The longest commit message the constraint allows, beside a configuration
        // that replaces the one the create carried.
        Row {
            case: "the longest commit message and a new configuration",
            // `CommitMessage` is `@length(min: 0, max: 160)`: this is its upper bound,
            // as the empty message above is its lower one.
            commit_message: Some("c".repeat(160)),
            description: None,
            configuration: Some(json!({
                "LicenseRequirement": "OPTIONAL",
                "LicenseFilepath": "licenses/LICENSE.txt",
                "ConfigurationRequirement": "NONE",
                "RequiresRestartForConfigurationUpdate": true,
            })),
            encryption_options: None,
            source: ("another-package-bucket", "dictionaries/synonyms.txt"),
        },
        // Every optional member at once, from a different bucket.
        Row {
            case: "every optional member",
            commit_message: Some("encrypted content".to_string()),
            description: Some("synonyms of the catalogue, encrypted"),
            configuration: Some(json!({
                "LicenseRequirement": "NONE",
                "ConfigurationRequirement": "OPTIONAL",
            })),
            encryption_options: Some(json!({
                "EncryptionEnabled": true,
                "KmsKeyIdentifier": "alias/opensearch-packages",
            })),
            source: ("another-package-bucket", "dictionaries/synonyms-v2.txt"),
        },
    ]
}

/// An update records the next numbered version of the package's content, carrying the
/// commit message and configuration of that version; a member the request leaves out
/// changes nothing; and the history reports every version recorded so far, newest
/// first, never rewriting one it already reported.
#[tokio::test]
async fn an_update_records_the_next_version_and_the_history_reports_it() {
    let emulator = Emulator::new();
    let package_id = available_package(&emulator, "pkg-versions-table").await;

    // What the stored record carries now: the create's own values, each replaced only
    // by a row that names it.
    let mut expected_description = json!("synonyms of the catalogue");
    let mut expected_configuration = json!({
        "LicenseRequirement": "REQUIRED",
        "ConfigurationRequirement": "REQUIRED",
    });
    let mut expected_encryption = Value::Null;
    let mut previous_history = reported_history(
        &history(&emulator, &package_id, &[]).await,
        "GetPackageVersionHistory(created)",
    );
    assert_eq!(
        previous_history.len(),
        1,
        "case GetPackageVersionHistory(created): the create's content is the first \
         version, got {previous_history:?}"
    );

    for (index, row) in update_rows().iter().enumerate() {
        let case = format!("UpdatePackage({})", row.case);
        let version = index + 2;
        let answered = details(&update(&emulator, row.body(&package_id)).await, &case);

        // The new content is being copied, so the package reports no version as
        // available until that copy lands.
        assert_eq!(
            answered["PackageStatus"].as_str(),
            Some(PACKAGE_STATUS_COPYING),
            "case {case}: the new content is being copied, got {answered:?}"
        );
        assert_eq!(
            answered["AvailablePackageVersion"],
            Value::Null,
            "case {case}: no version is available while the copy is in flight, got {answered:?}"
        );

        // A member the request named is the package's new value; one it left out
        // leaves the stored value standing.
        if let Some(description) = row.description {
            expected_description = json!(description);
        }
        if let Some(configuration) = &row.configuration {
            expected_configuration = configuration.clone();
        }
        if let Some(encryption_options) = &row.encryption_options {
            expected_encryption = encryption_options.clone();
        }
        assert_eq!(
            answered["PackageDescription"], expected_description,
            "case {case}: the description is the one the update left in place, got {answered:?}"
        );
        assert_eq!(
            answered["AvailablePackageConfiguration"], expected_configuration,
            "case {case}: the configuration is the one the update left in place, got {answered:?}"
        );
        assert_eq!(
            answered["PackageEncryptionOptions"], expected_encryption,
            "case {case}: the encryption options are the ones the update left in place, \
             got {answered:?}"
        );

        // The copy the update started is owed its outcome by the clock: no call asks
        // for the move, and once the span has passed the version it copied is the one
        // the package reports as available.
        tokio::time::sleep(SETTLE_WAIT).await;
        let settled = describe_one(&emulator, &package_id, &case).await;
        assert_eq!(
            settled["PackageStatus"].as_str(),
            Some(PACKAGE_STATUS_AVAILABLE),
            "case {case}: the copy has fallen due, so the package is available, got {settled:?}"
        );
        assert_eq!(
            settled["AvailablePackageVersion"].as_str(),
            Some(version.to_string().as_str()),
            "case {case}: the copy put version {version} in place, got {settled:?}"
        );

        let read = history(&emulator, &package_id, &[]).await;
        let reported = reported_history(&read, &case);
        assert_eq!(
            reported.len(),
            version,
            "case {case}: the history holds every version recorded so far, got {reported:?}"
        );

        // Newest version first, and every version numbered one past the one before it.
        let expected_versions: Vec<String> =
            (1..=version).rev().map(|number| number.to_string()).collect();
        assert_eq!(
            reported_versions(&read, &case),
            expected_versions,
            "case {case}: the history is reported newest version first, got {reported:?}"
        );

        // The newest entry is this update's: its commit message, the configuration
        // the package carries for this version, and the instant the version was
        // recorded.
        let newest = &reported[0];
        assert_eq!(
            newest["CommitMessage"],
            row.commit_message
                .as_ref()
                .map_or(Value::Null, |message| json!(message)),
            "case {case}: the newest version carries the commit message the update gave it, \
             got {newest:?}"
        );
        assert_eq!(
            newest["PackageConfiguration"], expected_configuration,
            "case {case}: the newest version carries the configuration of that version, \
             got {newest:?}"
        );
        assert!(
            newest["CreatedAt"].is_number(),
            "case {case}: the newest version carries the instant it was created, got {newest:?}"
        );

        // Reported newest first, so each entry was created at or before the one above
        // it — the history is chronological in the order it is reported.
        for pair in reported.windows(2) {
            let newer = pair[0]["CreatedAt"].as_f64().unwrap_or_default();
            let older = pair[1]["CreatedAt"].as_f64().unwrap_or_default();
            assert!(
                newer >= older,
                "case {case}: a version is reported before the ones created earlier, \
                 got {newer} then {older}"
            );
        }

        // A version already recorded is never rewritten: everything below the newest
        // entry is exactly what the previous read reported.
        assert_eq!(
            reported[1..].to_vec(),
            previous_history,
            "case {case}: the versions recorded before this update are unchanged, \
             got {reported:?}"
        );
        previous_history = reported;
    }
}

/// A package whose copy is still in flight takes no second update: the refusal
/// carries the service's own wording, nothing is recorded, and concurrent updates of
/// one package are therefore serialised — exactly one of them records a version.
#[tokio::test]
async fn a_copy_in_flight_serialises_updates_of_one_package() {
    let emulator = Emulator::new();
    let package_id = available_package(&emulator, "pkg-versions-serial").await;

    let case = "UpdatePackage(first)";
    details(
        &update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v2.txt"),
        )
        .await,
        case,
    );

    // The copy the first update started has not landed yet, so a second update is
    // refused rather than overtaking it.
    let case = "UpdatePackage(in flight)";
    let refused = update(
        &emulator,
        source_only_update(&package_id, "package-bucket", "synonyms/v3.txt"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "BaseException",
        "case {case}: expected BaseException, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some("Operation not allowed. The package is not in desired state."),
        "case {case}: the refusal says the package is not in a state to be updated, got {:?}",
        refused.body
    );
    assert_eq!(
        reported_versions(&history(&emulator, &package_id, &[]).await, case),
        vec!["2".to_string(), "1".to_string()],
        "case {case}: the refused update recorded no version"
    );

    // Once the copy has landed the package takes another update, so the refusal above
    // is about the status and not about the package.
    tokio::time::sleep(SETTLE_WAIT).await;
    let case = "UpdatePackage(after the copy landed)";
    details(
        &update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v3.txt"),
        )
        .await,
        case,
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    // Four updates of one package made at once: whichever of them is served first
    // takes the next version number, and the rest find the copy it started in flight.
    let case = "UpdatePackage(concurrent)";
    let (first, second, third, fourth) = tokio::join!(
        update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v4a.txt")
        ),
        update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v4b.txt")
        ),
        update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v4c.txt")
        ),
        update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/v4d.txt")
        ),
    );
    let answers = [first, second, third, fourth];
    let served = answers
        .iter()
        .filter(|answer| answer.status == 200)
        .count();
    assert_eq!(
        served,
        1,
        "case {case}: exactly one of four concurrent updates records a version, got {:?}",
        answers.iter().map(|answer| answer.status).collect::<Vec<_>>()
    );
    for answer in answers.iter().filter(|answer| answer.status != 200) {
        assert_eq!(
            error_code(answer),
            "BaseException",
            "case {case}: an update that found the copy in flight is refused as such, got {:?}",
            answer.body
        );
    }
    assert_eq!(
        reported_versions(&history(&emulator, &package_id, &[]).await, case),
        vec![
            "4".to_string(),
            "3".to_string(),
            "2".to_string(),
            "1".to_string()
        ],
        "case {case}: exactly one version was added by the four concurrent updates"
    );
}

/// An id that names no package of this account is refused as a bad input value on both
/// paths that resolve one, over the id forms the model's pattern admits.
#[tokio::test]
async fn an_unknown_package_id_is_not_found_on_every_path_that_resolves_one() {
    let emulator = Emulator::new();
    // The package this account does hold, so the refusals below are about the id and
    // not about an empty store.
    let held = available_package(&emulator, "pkg-versions-held").await;

    // `PackageID` is pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`: the `pkg-` form a
    // caller's own package takes, and the `[FG][0-9]+` form the service's published
    // packages take. Neither names a package of this account.
    let unknown = [
        "pkg-00000000000000000000000000000000",
        "pkg-abcdef0123456789",
        "F123456",
        "G7",
    ];

    for id in unknown {
        assert_ne!(id, held, "the unknown ids are not the created package's id");

        let case = format!("UpdatePackage({id})");
        let refused = update(
            &emulator,
            source_only_update(id, "package-bucket", "synonyms/v2.txt"),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(PACKAGE_NOT_FOUND),
            "case {case}: the refusal carries the fixed id-free sentence, got {:?}",
            refused.body
        );

        let case = format!("GetPackageVersionHistory({id})");
        let refused = history(&emulator, id, &[]).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // The package the account does hold still reports its own history, so nothing of
    // the refusals above touched it.
    assert_eq!(
        reported_versions(
            &history(&emulator, &held, &[]).await,
            "GetPackageVersionHistory(held)"
        ),
        vec!["1".to_string()],
        "case GetPackageVersionHistory(held): the stored package's history stands"
    );
}

/// The history pages through the whole log exactly once, newest version first,
/// whatever page size is asked for; the published page bound is accepted and one past
/// it refused; and a token that names no version of the package is refused.
#[tokio::test]
async fn the_history_pages_the_whole_log_once() {
    let emulator = Emulator::new();
    let package_id = available_package(&emulator, "pkg-versions-paged").await;

    // Three updates, each waited out, so the log holds four versions.
    for version in 2..=4 {
        let case = format!("UpdatePackage(v{version})");
        details(
            &update(
                &emulator,
                source_only_update(
                    &package_id,
                    "package-bucket",
                    &format!("synonyms/v{version}.txt"),
                ),
            )
            .await,
            &case,
        );
        tokio::time::sleep(SETTLE_WAIT).await;
    }

    let whole_log: Vec<String> = vec![
        "4".to_string(),
        "3".to_string(),
        "2".to_string(),
        "1".to_string(),
    ];

    // Every page size from one result to past the whole log, plus the largest page
    // `MaxResults`'s `@range(max: 100)` allows and the default (no page size), walks
    // the log once: every version, in the one order, with no repeats and no gaps.
    for page_size in ["1", "2", "3", "4", "5", "100", "0"] {
        let case = format!("GetPackageVersionHistory(maxResults={page_size})");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=whole_log.len() {
            let mut query = vec![("maxResults", page_size.to_string())];
            if let Some(next) = &token {
                query.push(("nextToken", next.clone()));
            }
            let page = history(&emulator, &package_id, &query).await;
            walked.extend(reported_versions(&page, &case));
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case}: the last page of the log carries no token"
        );
        assert_eq!(
            walked, whole_log,
            "case {case}: paging walks the whole log once, newest version first"
        );
    }

    // `MaxResults` is `@range(max: 100)`: the bound itself is a page size, and one
    // past it — like a negative page size, which is no page size at all — is a
    // malformed request rather than a page.
    for page_size in ["101", "-1"] {
        let case = format!("GetPackageVersionHistory(maxResults={page_size})");
        let refused = history(
            &emulator,
            &package_id,
            &[("maxResults", page_size.to_string())],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // A token names the version the next page starts at: one that names no version of
    // this package names no place in its history.
    for token in ["5", "0", "not-a-version"] {
        let case = format!("GetPackageVersionHistory(nextToken={token})");
        let refused = history(&emulator, &package_id, &[("nextToken", token.to_string())]).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
    }
}

/// A package's version history is bounded: versions are recorded up to the quota and
/// the update past it is refused, with the stored log left at the quota.
#[tokio::test]
async fn a_packages_version_history_is_bounded() {
    let emulator = Emulator::new();
    let package_id = available_package(&emulator, "pkg-versions-quota").await;

    // The log is brought to one version short of the quota the way a handler reaches
    // the store: filling it through the wire path would take one copy span per
    // version, and what is under test is the bound rather than the span.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let package = guard
            .packages
            .get_mut(&package_id)
            .expect("the created package is in the store");
        let now = SystemTime::now();
        while package.versions.len() < PACKAGE_VERSION_QUOTA - 1 {
            package.add_version(None, None, now, None);
        }
    }

    // The version that reaches the quota is recorded.
    let case = "UpdatePackage(at the quota)";
    details(
        &update(
            &emulator,
            source_only_update(&package_id, "package-bucket", "synonyms/at-quota.txt"),
        )
        .await,
        case,
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        reported_history(&history(&emulator, &package_id, &[]).await, case).len(),
        PACKAGE_VERSION_QUOTA,
        "case {case}: the log holds the quota's worth of versions"
    );

    // The one past it is refused, and records nothing.
    let case = "UpdatePackage(past the quota)";
    let refused = update(
        &emulator,
        source_only_update(&package_id, "package-bucket", "synonyms/past-quota.txt"),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case {case}: expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case {case}: expected LimitExceededException, got {:?}",
        refused.body
    );
    assert_eq!(
        reported_history(&history(&emulator, &package_id, &[]).await, case).len(),
        PACKAGE_VERSION_QUOTA,
        "case {case}: the refused update left the log at the quota"
    );
}
