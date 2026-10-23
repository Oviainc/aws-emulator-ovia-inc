// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplicationVersion` over the service's awsQuery HTTP surface.
//!
//! The rule under test: an update writes exactly the properties the request
//! supplies — a property the request omits keeps its stored value, a property
//! supplied as the empty string is cleared — leaves every other member of the
//! stored record as it was, and refuses with `InvalidParameterValue` when the
//! named application or application version does not exist.
//!
//! The wire harness ([`Emulator`], [`Answer`]) is the one `lifecycle.rs` already
//! owns: both suites compile into the single `tests/all.rs` target, so this file
//! reuses it rather than standing up a second copy. Nothing here reaches into
//! the store: every value asserted is one the emulator answered over HTTP, and
//! the stored state is read back through a later call.

use crate::lifecycle::{account, instant, Emulator, TEST_REGION};

/// The modelled maximum length of `Description`
/// (`smithy_query(query="operation",operation="UpdateApplicationVersion")`:
/// `Description` targets `Description` with `length { min: 0, max: 200 }`).
const DESCRIPTION_MAX_LEN: usize = 200;

/// One row of the description-update rule: what the version was created with,
/// what the update supplies, and what the record must then report.
struct DescriptionCase {
    /// The version label, which doubles as the case name.
    label: &'static str,
    created_with: Option<String>,
    /// `None` = the member is absent from the request.
    supplied: Option<String>,
    /// `None` = the property is reported as absent (cleared or never set).
    expected: Option<String>,
}

/// The rows, differing along what the rule generalises over: whether a
/// description was stored, whether one is supplied, whether the supplied value
/// is empty (the clearing form), its size at the modelled bound, its character
/// set, and whether it equals the value already stored.
fn description_cases() -> Vec<DescriptionCase> {
    let at_max = "d".repeat(DESCRIPTION_MAX_LEN);
    vec![
        DescriptionCase {
            label: "absent-then-set",
            created_with: None,
            supplied: Some("a first description".to_string()),
            expected: Some("a first description".to_string()),
        },
        DescriptionCase {
            label: "stored-then-omitted",
            created_with: Some("the original description".to_string()),
            supplied: None,
            expected: Some("the original description".to_string()),
        },
        DescriptionCase {
            label: "stored-then-cleared",
            created_with: Some("the original description".to_string()),
            // "To clear properties, specify an empty string." — the lower bound
            // of the modelled 0..200 length range.
            supplied: Some(String::new()),
            expected: None,
        },
        DescriptionCase {
            label: "absent-stays-absent",
            created_with: None,
            supplied: None,
            expected: None,
        },
        DescriptionCase {
            label: "replaced-at-the-modelled-maximum",
            created_with: Some("short".to_string()),
            supplied: Some(at_max.clone()),
            expected: Some(at_max),
        },
        DescriptionCase {
            label: "replaced-with-non-ascii-text",
            created_with: Some("ascii".to_string()),
            // No `&` or `<`: the reader below slices raw XML text, so an escaped
            // character would be compared in its escaped form.
            supplied: Some("описание — ünïcode ✓".to_string()),
            expected: Some("описание — ünïcode ✓".to_string()),
        },
        DescriptionCase {
            label: "replaced-with-the-value-it-already-holds",
            created_with: Some("steady".to_string()),
            supplied: Some("steady".to_string()),
            expected: Some("steady".to_string()),
        },
    ]
}

#[tokio::test]
async fn an_update_writes_only_the_properties_the_request_supplies() {
    let emulator = Emulator::new();
    let application = "update-version-app";
    let created_application = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created_application.status, 200,
        "setup: expected 200 from CreateApplication, got {} and body {}",
        created_application.status, created_application.body
    );

    for case in description_cases() {
        let mut create_params: Vec<(&str, &str)> = vec![
            ("ApplicationName", application),
            ("VersionLabel", case.label),
            ("SourceBundle.S3Bucket", "a-bucket"),
            ("SourceBundle.S3Key", "bundle.zip"),
        ];
        if let Some(description) = case.created_with.as_deref() {
            create_params.push(("Description", description));
        }
        let created = emulator
            .call("CreateApplicationVersion", &create_params)
            .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
            case.label,
            created.status,
            created.body
        );
        let date_created = created
            .expect("ApplicationVersion/DateCreated", case.label)
            .to_string();
        let date_updated_at_create = created
            .expect("ApplicationVersion/DateUpdated", case.label)
            .to_string();

        let mut update_params: Vec<(&str, &str)> = vec![
            ("ApplicationName", application),
            ("VersionLabel", case.label),
        ];
        if let Some(description) = case.supplied.as_deref() {
            update_params.push(("Description", description));
        }
        let updated = emulator
            .call("UpdateApplicationVersion", &update_params)
            .await;
        assert_eq!(
            updated.status,
            200,
            "case {:?}: expected 200 from UpdateApplicationVersion, got {} and body {}",
            case.label,
            updated.status,
            updated.body
        );
        assert_eq!(
            updated.value("ApplicationVersion/Description"),
            case.expected.as_deref(),
            "case {:?}: expected Description {:?}, got body {}",
            case.label,
            case.expected,
            updated.body
        );

        // Everything the update did not name comes back as it was stored.
        assert_eq!(
            updated.expect("ApplicationVersion/VersionLabel", case.label),
            case.label,
            "case {:?}: the VersionLabel must be the one updated, got {}",
            case.label,
            updated.body
        );
        assert_eq!(
            updated.expect("ApplicationVersion/ApplicationName", case.label),
            application,
            "case {:?}: the ApplicationName must be the one updated, got {}",
            case.label,
            updated.body
        );
        assert_eq!(
            updated.expect("ApplicationVersion/ApplicationVersionArn", case.label),
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/{application}/{}",
                account(),
                case.label
            ),
            "case {:?}: unexpected ApplicationVersionArn in {}",
            case.label,
            updated.body
        );
        assert_eq!(
            updated.expect("SourceBundle/S3Key", case.label),
            "bundle.zip",
            "case {:?}: the SourceBundle must survive the update, got {}",
            case.label,
            updated.body
        );
        assert_eq!(
            updated.expect("ApplicationVersion/Status", case.label),
            "UNPROCESSED",
            "case {:?}: an update processes nothing, so Status stays UNPROCESSED, got {}",
            case.label,
            updated.body
        );
        assert_eq!(
            updated.expect("ApplicationVersion/DateCreated", case.label),
            date_created,
            "case {:?}: DateCreated must not move on an update, got {}",
            case.label,
            updated.body
        );
        let date_updated = updated
            .expect("ApplicationVersion/DateUpdated", case.label)
            .to_string();
        // Compared as INSTANTS, not as strings: the serialized fractional second
        // drops trailing zeroes, so "…45.2581Z" and "…45.258199Z" do not order
        // lexicographically the way the instants they denote do.
        assert!(
            instant(&date_updated, case.label) >= instant(&date_created, case.label),
            "case {:?}: expected DateUpdated at or after DateCreated {date_created:?}, \
             got {date_updated:?}",
            case.label
        );
        if case.expected == case.created_with {
            // Nothing about the record changed, so `DateUpdated` — which records
            // when the record last CHANGED — must still be the creation-time
            // value, byte for byte.
            assert_eq!(
                date_updated, date_updated_at_create,
                "case {:?}: a call that changes no property must leave DateUpdated at {:?}, got {:?}",
                case.label, date_updated_at_create, date_updated
            );
        }

        // The update PERSISTED: a later call that supplies no property reports
        // the same record, read back out of the store rather than echoed from
        // the request that wrote it.
        let read_back = emulator
            .call(
                "UpdateApplicationVersion",
                &[
                    ("ApplicationName", application),
                    ("VersionLabel", case.label),
                ],
            )
            .await;
        assert_eq!(
            read_back.value("ApplicationVersion/Description"),
            case.expected.as_deref(),
            "case {:?}: the stored Description must still be {:?}, got body {}",
            case.label,
            case.expected,
            read_back.body
        );
        assert_eq!(
            read_back.expect("ApplicationVersion/DateUpdated", case.label),
            date_updated,
            "case {:?}: a call that changes nothing must not move DateUpdated, got {}",
            case.label,
            read_back.body
        );
    }
}

/// One row of the refusal rule: which application and version label the request
/// names, and whether each exists in the store.
struct MissingCase {
    name: &'static str,
    application_name: &'static str,
    version_label: &'static str,
}

#[tokio::test]
async fn an_update_of_something_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    // Two applications, so a label that exists under one but not the other is a
    // row of its own: a version is keyed by (application, label).
    for application in ["refusal-app", "other-app"] {
        let created = emulator
            .call("CreateApplication", &[("ApplicationName", application)])
            .await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateApplication for {application:?}, got {} and body {}",
            created.status, created.body
        );
    }
    let created_version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "other-app"),
                ("VersionLabel", "only-here"),
                ("Description", "untouched"),
            ],
        )
        .await;
    assert_eq!(
        created_version.status, 200,
        "setup: expected 200 from CreateApplicationVersion, got {} and body {}",
        created_version.status, created_version.body
    );

    let cases = [
        MissingCase {
            name: "no such application",
            application_name: "never-created",
            version_label: "only-here",
        },
        MissingCase {
            name: "application exists, label does not",
            application_name: "refusal-app",
            version_label: "never-created",
        },
        MissingCase {
            name: "label exists only under another application",
            application_name: "refusal-app",
            version_label: "only-here",
        },
    ];

    for case in &cases {
        let refused = emulator
            .call(
                "UpdateApplicationVersion",
                &[
                    ("ApplicationName", case.application_name),
                    ("VersionLabel", case.version_label),
                    ("Description", "a new description"),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400, got {} and body {}",
            case.name, refused.status, refused.body
        );
        // The model declares no error shape for this operation; Elastic
        // Beanstalk's own documentation names `InvalidParameterValue` for a
        // name or label that matches nothing.
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {:?}: expected InvalidParameterValue, got {}",
            case.name,
            refused.body
        );
    }

    // A refused update wrote nothing: the one stored version still holds its
    // original description.
    let intact = emulator
        .call(
            "UpdateApplicationVersion",
            &[
                ("ApplicationName", "other-app"),
                ("VersionLabel", "only-here"),
            ],
        )
        .await;
    assert_eq!(
        intact.value("ApplicationVersion/Description"),
        Some("untouched"),
        "a refused update must not change a stored version, got {}",
        intact.body
    );
}

#[tokio::test]
async fn an_update_touches_only_the_named_application_s_version() {
    let emulator = Emulator::new();
    // The same version label under two applications: the pair is the key, so
    // updating one must leave the other exactly as it was.
    let applications = ["app-one", "app-two"];
    for application in applications {
        let created = emulator
            .call("CreateApplication", &[("ApplicationName", application)])
            .await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateApplication for {application:?}, got {} and body {}",
            created.status, created.body
        );
        let version = emulator
            .call(
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", application),
                    ("VersionLabel", "v1"),
                    ("Description", "as created"),
                ],
            )
            .await;
        assert_eq!(
            version.status, 200,
            "setup: expected 200 from CreateApplicationVersion for {application:?}, \
             got {} and body {}",
            version.status, version.body
        );
    }

    let updated = emulator
        .call(
            "UpdateApplicationVersion",
            &[
                ("ApplicationName", "app-one"),
                ("VersionLabel", "v1"),
                ("Description", "updated for app-one"),
            ],
        )
        .await;
    assert_eq!(
        updated.value("ApplicationVersion/Description"),
        Some("updated for app-one"),
        "the named version must take the new description, got {}",
        updated.body
    );

    let untouched = emulator
        .call(
            "UpdateApplicationVersion",
            &[("ApplicationName", "app-two"), ("VersionLabel", "v1")],
        )
        .await;
    assert_eq!(
        untouched.value("ApplicationVersion/Description"),
        Some("as created"),
        "the namesake version under another application must be unchanged, got {}",
        untouched.body
    );
}

#[tokio::test]
async fn a_description_past_the_modelled_maximum_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let application = "bounds-app";
    let created_application = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created_application.status, 200,
        "setup: expected 200 from CreateApplication, got {} and body {}",
        created_application.status, created_application.body
    );
    let created = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", application),
                ("VersionLabel", "v1"),
                ("Description", "within bounds"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateApplicationVersion, got {} and body {}",
        created.status, created.body
    );

    // One past the modelled upper bound: the boundary validator refuses it
    // before the handler runs, so the stored record must be untouched. Only the
    // status is asserted — which code a boundary refusal carries on the query
    // protocol is this service's own open question, not this operation's.
    let too_long = "d".repeat(DESCRIPTION_MAX_LEN + 1);
    let refused = emulator
        .call(
            "UpdateApplicationVersion",
            &[
                ("ApplicationName", application),
                ("VersionLabel", "v1"),
                ("Description", too_long.as_str()),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: a Description of {} characters must be refused, got {} and body {}",
        "one past the modelled maximum",
        too_long.len(),
        refused.status,
        refused.body
    );

    let intact = emulator
        .call(
            "UpdateApplicationVersion",
            &[("ApplicationName", application), ("VersionLabel", "v1")],
        )
        .await;
    assert_eq!(
        intact.value("ApplicationVersion/Description"),
        Some("within bounds"),
        "case {:?}: a refused update must leave the stored Description alone, got {}",
        "one past the modelled maximum",
        intact.body
    );
}
