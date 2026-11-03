// SPDX-License-Identifier: CC0-1.0

//! What a session identifier given as a NAME resolves to, driven end to end over HTTP.
//!
//! The rule under test is about identifier resolution at every level of the hierarchy,
//! not about one application or one rename: a session identifier spelled as a NAME is
//! bound, the first time the data plane resolves it, to the id of the record it resolved
//! to, and that binding is what answers the name afterwards. So a control-plane rename
//! neither breaks the freed name (it keeps serving the record that held it, for a new
//! session and for one already polling) nor hands it to the record that claims it next
//! (which stays reachable only by its id). An identifier spelled as an ID is unaffected:
//! an id addresses one record for as long as that record exists.
//!
//! Every case drives real requests through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine: the hierarchy is created, deployed,
//! renamed and re-created through AppConfig's own operations, exactly as an operator
//! would, so every name a session resolves comes from the emulator's own state.

use bytes::Bytes;

use super::configuration_session_poll::{Emulator, Hierarchy};

/// One level of the AppConfig hierarchy, as a case names the levels it renames and the
/// level whose freed name another record then claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Application,
    Environment,
    ConfigurationProfile,
}

/// A rename to the shortest name AppConfig accepts: `Name` and `LongName` are both
/// `@length min 1` (`smithy_query(query="shape",shape="Name,LongName")` on the AppConfig
/// model).
const SHORTEST_NAME: &str = "A";

/// A rename to the longest name an application or an environment accepts: `Name` is
/// `@length 1..64` (same source as [`SHORTEST_NAME`]).
const LONGEST_NAME: &str =
    "0123456789012345678901234567890123456789012345678901234567890123";

/// A rename to the longest name a configuration profile accepts: its `Name` targets
/// `LongName`, `@length 1..128` (same source as [`SHORTEST_NAME`]).
const LONGEST_PROFILE_NAME: &str = "0123456789012345678901234567890123456789012345678901234567890123\
     4567890123456789012345678901234567890123456789012345678901234567";

// The two rename targets above are the modelled maxima, so a row that drifted off the
// bound would silently stop testing it.
const _: () = assert!(LONGEST_NAME.len() == 64);
const _: () = assert!(LONGEST_PROFILE_NAME.len() == 128);

/// What a session over a triple answered: the configuration its first poll served, or
/// the refusal the session was denied with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Served {
    /// The bytes `GetLatestConfiguration` served the session's first poll.
    Configuration(Vec<u8>),
    /// `StartConfigurationSession` was refused: the HTTP status, the modelled error code
    /// and the whole `Message`.
    Refused {
        status: u16,
        code: String,
        message: String,
    },
}

/// One rename, and what happens to the name it frees. The rows differ along what the
/// rule generalises over: WHICH level is renamed (and how many at once), whether another
/// record then claims the freed name, the shape of the new name (including both ends of
/// the modelled length range), and the form and content type of the configuration
/// served.
struct Case {
    name: &'static str,
    /// The levels the control plane renames.
    renamed: &'static [Level],
    /// The name each renamed level is given.
    rename_to: &'static str,
    /// The level whose freed name a NEW record then claims, with its own deployed
    /// configuration; it is always one of `renamed`.
    reclaimed_by: Option<Level>,
    /// The configuration deployed for the original triple.
    content: &'static [u8],
    content_type: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "the application is renamed, its old name left free",
        renamed: &[Level::Application],
        rename_to: "RenamedApplication",
        reclaimed_by: None,
        content: br#"{"restored":true}"#,
        content_type: "application/json",
    },
    Case {
        name: "the application is renamed to the shortest name and a second application \
               claims the freed one",
        renamed: &[Level::Application],
        rename_to: SHORTEST_NAME,
        reclaimed_by: Some(Level::Application),
        content: b"on",
        content_type: "text/plain",
    },
    Case {
        name: "the environment is renamed to the longest name and a new environment claims \
               the freed one",
        renamed: &[Level::Environment],
        rename_to: LONGEST_NAME,
        reclaimed_by: Some(Level::Environment),
        content: b"feature: on\nextra: 2\n",
        content_type: "application/x-yaml",
    },
    Case {
        name: "the configuration profile is renamed to the longest name and a new profile \
               claims the freed one",
        renamed: &[Level::ConfigurationProfile],
        rename_to: LONGEST_PROFILE_NAME,
        reclaimed_by: Some(Level::ConfigurationProfile),
        content: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        content_type: "application/octet-stream",
    },
    Case {
        name: "all three levels are renamed at once, to one name each level now carries",
        renamed: &[
            Level::Application,
            Level::Environment,
            Level::ConfigurationProfile,
        ],
        rename_to: "Shifted",
        reclaimed_by: None,
        content: br#"{"tier":"one"}"#,
        content_type: "application/json",
    },
];

/// The configuration the record that claims a freed name deploys for itself — different
/// bytes from [`Case::content`], so serving the wrong record is visible.
const RECLAIMED_CONTENT: &[u8] = br#"{"owner":"the record that claimed the name"}"#;

/// The modelled `Message` a refused session carries: fixed text of the form
/// "<ResourceType> not found", interpolating nothing from the request.
const CONFIGURATION_PROFILE_NOT_FOUND: &str = "ConfigurationProfile not found";

/// A renamed record keeps serving the name it used to carry — for a session started
/// afterwards, for the session already polling it, and whichever level was renamed —
/// while the record that claims that freed name does not inherit it.
#[tokio::test]
async fn a_name_keeps_resolving_to_the_record_it_first_resolved_to() {
    for case in CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let original = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        emulator
            .deploy(
                &original,
                &original.profile_id,
                case.content,
                case.content_type,
                None,
            )
            .await;
        let deployed = Served::Configuration(case.content.to_vec());

        // Positive control, and what binds the three names: the triple serves its
        // deployed configuration while the control plane still carries those names.
        let served = serve(
            &emulator,
            &original.application_name,
            &original.environment_name,
            &original.profile_name,
        )
        .await;
        assert_eq!(
            served, deployed,
            "case {name}: before any rename the names must serve the deployed \
             configuration, got {served:?}"
        );

        // A session that is already polling when the rename happens, so the rule is
        // asserted for an in-flight consumer too, not only for a fresh session.
        let active = emulator
            .session_token(
                &original.application_name,
                &original.environment_name,
                &original.profile_name,
            )
            .await;
        let first_poll = emulator.get_latest_configuration(&active).await;
        assert_eq!(
            first_poll.status,
            200,
            "case {name}: the active session's first poll expected 200, got {} body {}",
            first_poll.status,
            first_poll.text()
        );
        let next_token = first_poll.next_token();

        // The control plane renames each level the case names, and reports the new name
        // back, so what follows is asserted against a rename the control plane really
        // performed.
        let renamed = rename_levels(&emulator, &original, case).await;

        // The new names resolve (they are resolved against the control plane for the
        // first time here) ...
        let served = serve(
            &emulator,
            &renamed.application_name,
            &renamed.environment_name,
            &renamed.profile_name,
        )
        .await;
        assert_eq!(
            served, deployed,
            "case {name}: the new names must serve the deployed configuration, \
             got {served:?}"
        );

        // ... the ids resolve, because an id addresses one record for as long as it
        // exists ...
        let served = serve(
            &emulator,
            &original.application_id,
            &original.environment_id,
            &original.profile_id,
        )
        .await;
        assert_eq!(
            served, deployed,
            "case {name}: the id triple must serve the deployed configuration, \
             got {served:?}"
        );

        // ... and so do the names the records no longer carry.
        let served = serve(
            &emulator,
            &original.application_name,
            &original.environment_name,
            &original.profile_name,
        )
        .await;
        assert_eq!(
            served, deployed,
            "case {name}: the freed names must still serve the configuration of the \
             records that held them, got {served:?}"
        );

        // Exactly the names that were resolved are bound: the original three, plus the
        // new name of each renamed level. The id triple bound nothing — an id needs no
        // binding — and re-resolving a name that is already bound bound nothing again.
        let bound = emulator.state().lock().name_bindings().len();
        let expected_bound = 3 + case.renamed.len();
        assert_eq!(
            bound, expected_bound,
            "case {name}: expected {expected_bound} bound names, got {bound}"
        );
        // The session already polling is undisturbed: its next poll is served, and
        // carries no content because the version it last received is still the one in
        // force.
        let poll = emulator.get_latest_configuration(&next_token).await;
        assert_eq!(
            poll.status,
            200,
            "case {name}: a rename must not break an active session's poll, got {} body {}",
            poll.status,
            poll.text()
        );
        assert!(
            poll.body.is_empty(),
            "case {name}: the version is unchanged, so the poll must serve no content, \
             got {:?}",
            poll.body.as_ref()
        );

        let Some(level) = case.reclaimed_by else {
            continue;
        };
        // A NEW record claims the freed name and deploys a configuration of its own. The
        // freed name still answers for the record that held it — never for the claimant,
        // which is reachable by its own id and names only.
        let claimant = reclaim(&emulator, &original, level).await;
        let bound_before = emulator.state().lock().name_bindings().len();
        let served = serve(
            &emulator,
            &original.application_name,
            &original.environment_name,
            &original.profile_name,
        )
        .await;
        assert_eq!(
            served, deployed,
            "case {name}: a reclaimed name must keep serving the record that held it, \
             got {served:?}"
        );
        let served = serve(
            &emulator,
            &claimant.application_name,
            &claimant.environment_name,
            &claimant.profile_name,
        )
        .await;
        let expected = match level {
            // The claimant is a second application, so the freed application name scopes
            // the request to the application that held it, under which the claimant's own
            // environment and configuration profile names resolve to nothing: the
            // configuration profile is the level reported.
            Level::Application => Served::Refused {
                status: 404,
                code: "ResourceNotFoundException".to_string(),
                message: CONFIGURATION_PROFILE_NOT_FOUND.to_string(),
            },
            // The claimant is a sibling under the same application, so the request
            // resolves — to the record that held the name, whose configuration it serves
            // rather than the claimant's.
            Level::Environment | Level::ConfigurationProfile => deployed.clone(),
        };
        assert_eq!(
            served, expected,
            "case {name}: the record claiming the freed name must not inherit it, \
             expected {expected:?}, got {served:?}"
        );
        // Nothing the claimant's own names reached bound anything new: every name in
        // those requests was already bound, and a refused session persists nothing.
        let bound_after = emulator.state().lock().name_bindings().len();
        assert_eq!(
            bound_after, bound_before,
            "case {name}: expected {bound_before} bound names still, got {bound_after}"
        );
    }
}

/// Start a session over a triple and report what it answered: the bytes its first poll
/// served, or the refusal the session was denied with.
async fn serve(emulator: &Emulator, application: &str, environment: &str, profile: &str) -> Served {
    let reply = emulator
        .post_json(
            "/configurationsessions",
            &format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"}}"#
            ),
        )
        .await;
    if reply.status != 201 {
        return Served::Refused {
            status: reply.status,
            code: reply.error_code(),
            message: reply.string("Message"),
        };
    }
    let token = reply.string("InitialConfigurationToken");
    let poll = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        poll.status,
        200,
        "the poll of a minted session expected 200, got {} body {}",
        poll.status,
        poll.text()
    );
    Served::Configuration(poll.body.to_vec())
}

/// Rename every level the case names through AppConfig's own update operations, and
/// answer the hierarchy as the control plane now reports it.
async fn rename_levels(emulator: &Emulator, original: &Hierarchy, case: &Case) -> Hierarchy {
    let application_id = &original.application_id;
    let environment_id = &original.environment_id;
    let profile_id = &original.profile_id;
    let mut renamed = original.clone();
    for level in case.renamed {
        let path = match level {
            Level::Application => format!("/applications/{application_id}"),
            Level::Environment => {
                format!("/applications/{application_id}/environments/{environment_id}")
            }
            Level::ConfigurationProfile => {
                format!("/applications/{application_id}/configurationprofiles/{profile_id}")
            }
        };
        let updated = emulator
            .send(
                "PATCH",
                &path,
                "application/json",
                &[],
                Bytes::copy_from_slice(format!(r#"{{"Name":"{}"}}"#, case.rename_to).as_bytes()),
            )
            .await;
        assert_eq!(
            updated.status,
            200,
            "rename of {level:?} to {}: expected 200, got {} body {}",
            case.rename_to,
            updated.status,
            updated.text()
        );
        // Read the name back through the control plane's own reader: the rename under
        // test is the one AppConfig reports, not the one the request asked for.
        let read_back = emulator
            .send("GET", &path, "application/json", &[], Bytes::new())
            .await;
        assert_eq!(
            read_back.string("Name"),
            case.rename_to,
            "rename of {level:?}: the control plane must report the new name, got {}",
            read_back.text()
        );
        match level {
            Level::Application => renamed.application_name = case.rename_to.to_string(),
            Level::Environment => renamed.environment_name = case.rename_to.to_string(),
            Level::ConfigurationProfile => renamed.profile_name = case.rename_to.to_string(),
        }
    }
    renamed
}

/// Create a new record that claims the name `level` freed, with a deployed configuration
/// of its own ([`RECLAIMED_CONTENT`]), and answer the triple that addresses it by name.
///
/// For the application level the claimant is a whole second hierarchy; for a child level
/// it is a sibling under the same application, so the other two names of the triple are
/// the ones the original hierarchy still answers to.
async fn reclaim(emulator: &Emulator, original: &Hierarchy, level: Level) -> Hierarchy {
    match level {
        Level::Application => {
            let claimant = emulator
                .create_hierarchy(
                    &original.application_name,
                    "ClaimantEnvironment",
                    "ClaimantProfile",
                )
                .await;
            emulator
                .deploy(
                    &claimant,
                    &claimant.profile_id,
                    RECLAIMED_CONTENT,
                    "application/json",
                    None,
                )
                .await;
            claimant
        }
        Level::Environment => {
            let environment_id = emulator
                .create_environment(&original.application_id, &original.environment_name)
                .await;
            let claimant = Hierarchy {
                environment_id,
                ..original.clone()
            };
            emulator
                .deploy(
                    &claimant,
                    &claimant.profile_id,
                    RECLAIMED_CONTENT,
                    "application/json",
                    None,
                )
                .await;
            claimant
        }
        Level::ConfigurationProfile => {
            let profile_id = emulator
                .create_profile(&original.application_id, &original.profile_name)
                .await;
            let claimant = Hierarchy {
                profile_id,
                ..original.clone()
            };
            emulator
                .deploy(
                    &claimant,
                    &claimant.profile_id,
                    RECLAIMED_CONTENT,
                    "application/json",
                    None,
                )
                .await;
            claimant
        }
    }
}
