// SPDX-License-Identifier: CC0-1.0

//! `ValidateConfiguration` resolves the resources its request names BEFORE it reports any
//! verdict about a configuration.
//!
//! The rule under test, in one sentence: an operation addressed by a path of nested
//! resource identifiers resolves each level against the account's state, outermost level
//! first, before doing the work it reports on, and refuses a level that resolves to nothing
//! with `ResourceNotFoundException` (HTTP 404) naming that resource kind, every identifier
//! the request scoped the lookup with, and the account it was scoped to — so no validator
//! runs and no other refusal is reported for a request whose resources do not exist.
//!
//! `ValidateConfiguration` names three such levels — `ApplicationId`, then
//! `ConfigurationProfileId` under it, then `ConfigurationVersion` within that profile
//! (`smithy_query(query="operation",operation="ValidateConfiguration")`) — and the table
//! below spans what the rule generalises over rather than one misnamed request: WHICH level
//! is unresolvable, the form of the identifier that fails (one of the modelled id shape, a
//! single character, each member's own `@length` maximum, a case variant of the real id, a
//! version label, a number past the latest version, the number no profile issues), whether
//! a resource carrying that same identifier exists under a DIFFERENT parent, and whether
//! the profile carries a validator that would refuse the content — which it never gets to,
//! because resolution comes first.
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses,
//! and each one ends by asserting that the correctly addressed call still reports the
//! verdict the profile's validators give, so a 404 is never a verdict in disguise.

use crate::appconfig_workflow::{account, create_application, router, Answer};
use crate::validate_configuration_verdict::{given_profile, given_version, validate_configuration};

/// A schema admitting only a boolean document, so it refuses every JSON object: the
/// validator a row carries to show that an unresolved resource is reported INSTEAD of the
/// verdict this validator would otherwise give.
const REFUSES_ANY_OBJECT: &str = r#"{"type":"boolean"}"#;

/// The content every row stores: a JSON object, which [`REFUSES_ANY_OBJECT`] refuses and a
/// profile carrying no validator admits.
const CONTENT: &[u8] = br#"{"enabled":true}"#;

/// Which level of the request's path the row makes unresolvable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// `ApplicationId`, the outermost level.
    Application,
    /// `ConfigurationProfileId`, under the application.
    Profile,
    /// `ConfigurationVersion`, within the profile.
    Version,
}

/// One row: the level the request misnames, the identifier it carries there, and the
/// validators the row's profile holds.
struct Row {
    case: &'static str,
    level: Level,
    /// Built per row, because some rows derive it from the ids the emulator minted.
    identifier: String,
    /// The `JSON_SCHEMA` documents the row's profile carries.
    validators: &'static [&'static str],
}

impl Row {
    /// A row whose profile carries no validator.
    fn new(case: &'static str, level: Level, identifier: impl Into<String>) -> Self {
        Self {
            case,
            level,
            identifier: identifier.into(),
            validators: &[],
        }
    }

    /// The same row, with a profile whose validator refuses the stored content.
    fn with_refusing_validator(mut self) -> Self {
        self.validators = &[REFUSES_ANY_OBJECT];
        self
    }
}

/// The `Message` of a refusal body, or the body itself when it carries none — which is what
/// a failing assertion needs to show.
fn message(answer: &Answer) -> String {
    answer
        .string("Message")
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// The not-found sentence AppConfig states for each of the three levels, filled in from the
/// request's own identifiers and the account the lookup was scoped to.
fn not_found_message(level: Level, application: &str, profile: &str, version: &str) -> String {
    match level {
        Level::Application => format!(
            "Application with Id {application} could not be found for account {}.",
            account()
        ),
        Level::Profile => format!(
            "Configuration Profile with Application Id {application} and Configuration Profile \
             Id {profile} could not be found for account {}.",
            account()
        ),
        Level::Version => format!(
            "Configuration version '{version}' of configuration profile '{profile}' is not held \
             in the AppConfig hosted configuration store."
        ),
    }
}

/// `ValidateConfiguration` refuses a request naming a resource the account does not hold
/// with the 404 that names that resource, whichever level of the path it is and whatever
/// the profile's validators would have said about the content.
#[tokio::test]
async fn an_unresolvable_resource_is_reported_before_any_validator_runs() {
    let router = router();
    let application = create_application(&router, "resolved").await;
    // A second application holding its own profile, so a row can name a profile id that
    // exists — under a parent that is not the one the request scoped the lookup to.
    let other_application = create_application(&router, "elsewhere").await;
    let other_profile = given_profile(
        &router,
        "a profile of another application",
        &other_application,
        "elsewhere-profile",
        &[],
    )
    .await;

    let rows = [
        // `ApplicationId` is `Name`, `length 1..64`.
        Row::new(
            "an application id of the modelled id shape that was never minted",
            Level::Application,
            "zzzzzzz",
        ),
        Row::new(
            "a single-character application id",
            Level::Application,
            "z",
        ),
        Row::new(
            "an application id at the member's length maximum of 64",
            Level::Application,
            "z".repeat(64),
        ),
        Row::new(
            "an application id differing from a real one only in case",
            Level::Application,
            application.to_uppercase(),
        ),
        Row::new(
            "an application that does not exist, under a profile-refusing validator",
            Level::Application,
            "zzzzzzz",
        )
        .with_refusing_validator(),
        // `ConfigurationProfileId` is `LongName`, `length 1..128`.
        Row::new(
            "a profile id of the modelled id shape the application does not hold",
            Level::Profile,
            "yyyyyyy",
        ),
        Row::new("a single-character profile id", Level::Profile, "y"),
        Row::new(
            "a profile id at the member's length maximum of 128",
            Level::Profile,
            "y".repeat(128),
        ),
        Row::new(
            "the id of a profile that belongs to another application",
            Level::Profile,
            other_profile.clone(),
        ),
        Row::new(
            "a profile id differing from a real one only in case",
            Level::Profile,
            // Filled in per row below from the profile the row creates.
            String::new(),
        ),
        Row::new(
            "a profile the application does not hold, carrying a refusing validator",
            Level::Profile,
            "yyyyyyy",
        )
        .with_refusing_validator(),
        // `ConfigurationVersion` is `Version`, `length 1..1024`.
        Row::new(
            "a version number one past the latest the profile issued",
            Level::Version,
            "2",
        ),
        Row::new(
            "a version number far past the latest the profile issued",
            Level::Version,
            "9999",
        ),
        Row::new(
            "the version number zero, which no profile issues",
            Level::Version,
            "0",
        ),
        // `VersionLabel` is `pattern [^0-9]`, so a label carries a non-digit.
        Row::new(
            "a version label no stored version carries",
            Level::Version,
            "no-such-label",
        ),
        Row::new(
            "a version identifier at the member's length maximum of 1024",
            Level::Version,
            "v".repeat(1024),
        ),
        Row::new(
            "a version the profile does not hold, where a validator would refuse the content",
            Level::Version,
            "9999",
        )
        .with_refusing_validator(),
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One profile per row, holding exactly one version, so each row's version numbering
        // starts in the same place. Names are indexed because `Name` is `length 1..64`.
        let profile = given_profile(
            &router,
            case,
            &application,
            &format!("resolved-{index}"),
            row.validators,
        )
        .await;
        let version =
            given_version(&router, case, &application, &profile, CONTENT, Some("stored")).await;

        // The row's identifier replaces exactly one level of the correctly addressed
        // request; a row that left it empty derives it from the id the emulator minted.
        let misnamed = if row.identifier.is_empty() {
            match row.level {
                Level::Application => application.to_uppercase(),
                Level::Profile => profile.to_uppercase(),
                Level::Version => version.to_uppercase(),
            }
        } else {
            row.identifier.clone()
        };
        let (requested_application, requested_profile, requested_version) = match row.level {
            Level::Application => (misnamed.as_str(), profile.as_str(), version.as_str()),
            Level::Profile => (application.as_str(), misnamed.as_str(), version.as_str()),
            Level::Version => (application.as_str(), profile.as_str(), misnamed.as_str()),
        };

        let refused = validate_configuration(
            &router,
            requested_application,
            requested_profile,
            requested_version,
        )
        .await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: a resource the account does not hold is not found, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            message(&refused),
            not_found_message(
                row.level,
                requested_application,
                requested_profile,
                requested_version
            ),
            "case {case:?}: the refusal must state the template for the level that resolved to \
             nothing, got {:?}",
            message(&refused)
        );
        // `ResourceName` names the identifier of the level that failed, echoed as the
        // request carried it — not the one the account does hold.
        assert_eq!(
            refused.string("ResourceName").as_deref(),
            Some(misnamed.as_str()),
            "case {case:?}: ResourceName must echo the identifier that resolved to nothing, got \
             {:?}",
            refused.string("ResourceName")
        );

        // The resolution refusal was about the resource, not about the content: addressed
        // correctly, the same call reports the verdict the profile's validators give.
        let addressed = validate_configuration(&router, &application, &profile, &version).await;
        let expected_status = if row.validators.is_empty() { 204 } else { 400 };
        assert_eq!(
            addressed.status,
            expected_status,
            "case {case:?}: the correctly addressed call must report the validators' verdict \
             ({expected_status}), got {} / {}",
            addressed.status,
            String::from_utf8_lossy(&addressed.body)
        );
    }
}

/// With every level of the path unresolvable at once, the refusal names the OUTERMOST one:
/// a lookup scoped by an application that does not exist never reaches the profile, and one
/// scoped by a profile that does not exist never reaches a version.
#[tokio::test]
async fn the_outermost_unresolvable_level_is_the_one_reported() {
    let router = router();
    let application = create_application(&router, "outermost").await;
    let profile = given_profile(&router, "outermost", &application, "outermost", &[]).await;
    let version = given_version(
        &router,
        "outermost",
        &application,
        &profile,
        CONTENT,
        None,
    )
    .await;

    // Rows differ along how many levels are unresolvable and which; the expected level is
    // always the outermost of them.
    let rows = [
        (
            "an unknown application, profile and version",
            "zzzzzzz",
            "yyyyyyy",
            "9999",
            Level::Application,
        ),
        (
            "an unknown application and version, with a real profile id",
            "zzzzzzz",
            profile.as_str(),
            "9999",
            Level::Application,
        ),
        (
            "a real application with an unknown profile and version",
            application.as_str(),
            "yyyyyyy",
            "9999",
            Level::Profile,
        ),
        (
            "a real application and profile with an unknown version",
            application.as_str(),
            profile.as_str(),
            "9999",
            Level::Version,
        ),
    ];

    for (case, requested_application, requested_profile, requested_version, level) in rows {
        let refused = validate_configuration(
            &router,
            requested_application,
            requested_profile,
            requested_version,
        )
        .await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            message(&refused),
            not_found_message(
                level,
                requested_application,
                requested_profile,
                requested_version
            ),
            "case {case:?}: the outermost unresolvable level ({level:?}) is the one reported, got \
             {:?}",
            message(&refused)
        );
    }

    // None of those refusals disturbed the resources that do exist: the correctly addressed
    // call still answers the modelled empty 204.
    let addressed = validate_configuration(&router, &application, &profile, &version).await;
    assert_eq!(
        addressed.status,
        204,
        "the correctly addressed call must still answer the modelled 204, got {} / {}",
        addressed.status,
        String::from_utf8_lossy(&addressed.body)
    );
}
