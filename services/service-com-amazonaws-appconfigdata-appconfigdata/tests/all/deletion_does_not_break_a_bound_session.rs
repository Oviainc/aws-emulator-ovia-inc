// SPDX-License-Identifier: CC0-1.0

//! What a control-plane deletion does to a configuration session that is already bound,
//! driven end to end over HTTP.
//!
//! The rule under test is about every level of the AppConfig hierarchy and every poll of
//! a session, not about one deletion: resolving the (application, environment,
//! configuration profile) triple is what STARTING a session does, so a token that
//! resolves to a live, unexpired session is served from the configuration that session is
//! bound to — the control plane is re-read only to pick up a version deployed since.
//! A record deleted after the session was bound therefore does not retroactively unmake
//! the session, and `GetLatestConfiguration`'s `ResourceNotFoundException` is left for the
//! session that was bound to no configuration at all.
//!
//! The rows differ along what the rule generalises over: WHICH level the control plane
//! removes (and how many at once), whether the session had already polled when the
//! removal happened, and the form, content type and version label of the configuration it
//! is bound to. Each row also checks the two halves that must NOT change: a session
//! started after the removal is still judged by the control plane (that is where resource
//! resolution belongs), and an untouched hierarchy's session keeps polling throughout.
//!
//! Every request goes through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine: the hierarchy is created, deployed and
//! deleted through AppConfig's own operations, exactly as an operator would.

use bytes::Bytes;

use service_com_amazonaws_appconfigdata_appconfigdata::state::DEFAULT_POLL_INTERVAL_IN_SECONDS;

use super::configuration_session_poll::{is_modelled_token, Emulator, Hierarchy};

/// One level of the hierarchy the control plane removes. AppConfig does not cascade the
/// resources a container owns, so a row that reaches a container lists the children it
/// has to remove first, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// Every hosted configuration version the configuration profile holds.
    HostedVersions,
    ConfigurationProfile,
    Environment,
    Application,
}

/// What a session STARTED after the removal must answer — the half of the rule that does
/// not change, because resolution is `StartConfigurationSession`'s job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterRemoval {
    /// `StartConfigurationSession` is refused with the modelled
    /// `ResourceNotFoundException` naming this `ResourceType`, and the `Message` is that
    /// level's fixed "<ResourceType> not found" sentence.
    SessionRefused { resource_type: &'static str },
    /// The triple still resolves, so the session is minted — and it is bound to no
    /// configuration, which is the one case this operation reports as not found, with
    /// this `ResourceType`.
    PollRefused { resource_type: &'static str },
}

/// One removal, and the configuration the victim session is bound to when it happens.
struct Case {
    name: &'static str,
    /// What the control plane removes, in the order it has to.
    removed: &'static [Removal],
    /// Whether the session had already polled once (so it holds the latest version) when
    /// the removal happened.
    polled_before: bool,
    content: &'static [u8],
    content_type: &'static str,
    version_label: Option<&'static str>,
    after: AfterRemoval,
}

const CASES: &[Case] = &[
    Case {
        name: "the hosted configuration version is deleted before the session's first poll",
        removed: &[Removal::HostedVersions],
        polled_before: false,
        content: br#"{"feature":"on"}"#,
        content_type: "application/json",
        version_label: Some("v-one"),
        after: AfterRemoval::PollRefused {
            resource_type: "Configuration",
        },
    },
    Case {
        name: "the hosted configuration version is deleted after a poll",
        removed: &[Removal::HostedVersions],
        polled_before: true,
        content: b"feature: on\n",
        content_type: "application/x-yaml",
        version_label: None,
        after: AfterRemoval::PollRefused {
            resource_type: "Configuration",
        },
    },
    Case {
        name: "the configuration profile is deleted after a poll",
        removed: &[Removal::HostedVersions, Removal::ConfigurationProfile],
        polled_before: true,
        content: br#"{"victim":true}"#,
        content_type: "application/json",
        version_label: Some("release-1"),
        after: AfterRemoval::SessionRefused {
            resource_type: "ConfigurationProfile",
        },
    },
    Case {
        name: "the configuration profile is deleted before the session's first poll",
        removed: &[Removal::HostedVersions, Removal::ConfigurationProfile],
        polled_before: false,
        content: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        content_type: "application/octet-stream",
        version_label: None,
        after: AfterRemoval::SessionRefused {
            resource_type: "ConfigurationProfile",
        },
    },
    Case {
        name: "the environment is deleted after a poll",
        removed: &[Removal::Environment],
        polled_before: true,
        content: b"on",
        content_type: "text/plain",
        version_label: Some("flag-2"),
        after: AfterRemoval::SessionRefused {
            resource_type: "Environment",
        },
    },
    Case {
        name: "the environment is deleted before the session's first poll",
        removed: &[Removal::Environment],
        polled_before: false,
        content: b"feature: off\nextra: 2\n",
        content_type: "application/x-yaml",
        version_label: None,
        after: AfterRemoval::SessionRefused {
            resource_type: "Environment",
        },
    },
    Case {
        name: "the whole hierarchy is deleted after a poll",
        removed: &[
            Removal::HostedVersions,
            Removal::ConfigurationProfile,
            Removal::Environment,
            Removal::Application,
        ],
        polled_before: true,
        content: &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        content_type: "application/octet-stream",
        version_label: Some("binary-2"),
        after: AfterRemoval::SessionRefused {
            resource_type: "Application",
        },
    },
];

/// The configuration of the hierarchy no row touches, whose session is the positive
/// control: different bytes from every victim row, so serving the wrong session's
/// configuration is visible.
const CONTROL_CONTENT: &[u8] = br#"{"control":true}"#;

/// A session bound to a configuration keeps being served after the control plane removes
/// the records it was bound through, whichever level is removed and whether or not it had
/// already polled — while a session started afterwards is still judged by the control
/// plane, and an untouched hierarchy's session is undisturbed.
#[tokio::test]
async fn a_removal_does_not_break_a_session_already_bound_to_a_configuration() {
    for case in CASES {
        let name = case.name;
        let emulator = Emulator::new();

        // The positive control: a second hierarchy, polling throughout, that no removal
        // in this row touches.
        let control = emulator
            .create_hierarchy("ControlApp", "ControlEnvironment", "ControlProfile")
            .await;
        emulator
            .deploy(
                &control,
                &control.profile_id,
                CONTROL_CONTENT,
                "application/json",
                None,
            )
            .await;
        let control_token = emulator
            .session_token(
                &control.application_name,
                &control.environment_name,
                &control.profile_name,
            )
            .await;
        let control_poll = emulator.get_latest_configuration(&control_token).await;
        assert_eq!(
            control_poll.status,
            200,
            "case {name}: the control session's first poll expected 200, got {} body {}",
            control_poll.status,
            control_poll.text()
        );
        assert_eq!(
            control_poll.body.as_ref(),
            CONTROL_CONTENT,
            "case {name}: the control session must be served its own configuration, got {:?}",
            control_poll.body.as_ref()
        );
        let control_token = control_poll.next_token();

        // The victim: deployed, then a session started over it by NAME, so the removal
        // below also takes away the records its names were bound through.
        let victim = emulator
            .create_hierarchy("VictimApp", "VictimEnvironment", "VictimProfile")
            .await;
        emulator
            .deploy(
                &victim,
                &victim.profile_id,
                case.content,
                case.content_type,
                case.version_label,
            )
            .await;
        let token = emulator
            .session_token(
                &victim.application_name,
                &victim.environment_name,
                &victim.profile_name,
            )
            .await;

        // A row that polls first holds the latest version, so its poll after the removal
        // is the no-change answer; a row that does not is still owed the content it is
        // bound to.
        let (token, expected_content, expected_label): (String, &[u8], Option<&str>) =
            if case.polled_before {
                let poll = emulator.get_latest_configuration(&token).await;
                assert_eq!(
                    poll.status,
                    200,
                    "case {name}: the poll before the removal expected 200, got {} body {}",
                    poll.status,
                    poll.text()
                );
                assert_eq!(
                    poll.body.as_ref(),
                    case.content,
                    "case {name}: the poll before the removal must serve the deployed \
                     content, got {:?}",
                    poll.body.as_ref()
                );
                (poll.next_token(), b"", None)
            } else {
                (token, case.content, case.version_label)
            };

        remove(&emulator, &victim, case.removed).await;

        // THE RULE: the token is live, so the poll is served — never refused because the
        // records the session was bound through are gone.
        let poll = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            poll.status,
            200,
            "case {name}: the poll after the removal expected 200, got {} body {}",
            poll.status,
            poll.text()
        );
        assert_eq!(
            poll.body.as_ref(),
            expected_content,
            "case {name}: expected the configuration the session is bound to ({:?}), got {:?}",
            expected_content,
            poll.body.as_ref()
        );
        assert_eq!(
            poll.header("content-type"),
            Some(case.content_type),
            "case {name}: expected the bound configuration's content type {}, got {:?}",
            case.content_type,
            poll.header("content-type")
        );
        assert_eq!(
            poll.header("version-label"),
            expected_label,
            "case {name}: expected the version label {expected_label:?}, got {:?}",
            poll.header("version-label")
        );
        assert_eq!(
            poll.header("next-poll-interval-in-seconds"),
            Some(DEFAULT_POLL_INTERVAL_IN_SECONDS.to_string().as_str()),
            "case {name}: expected the session's poll interval, got {:?}",
            poll.header("next-poll-interval-in-seconds")
        );
        let next = poll.next_token();
        assert!(
            is_modelled_token(&next) && next != token,
            "case {name}: the poll must rotate the token, got {next:?} for {token:?}"
        );

        // And the poll CHAIN continues: the successor token is served too, and carries
        // nothing new, because the version the client now holds is the bound one.
        let again = emulator.get_latest_configuration(&next).await;
        assert_eq!(
            again.status,
            200,
            "case {name}: the next poll of the chain expected 200, got {} body {}",
            again.status,
            again.text()
        );
        assert!(
            again.body.is_empty(),
            "case {name}: the client already holds the bound version, so the next poll \
             must serve no content, got {:?}",
            again.body.as_ref()
        );
        assert!(
            is_modelled_token(&again.next_token()),
            "case {name}: the next poll must rotate the token too, got {:?}",
            again.next_token()
        );

        // The untouched hierarchy's session is undisturbed, which is what isolates the
        // answers above to the removed records.
        let control_poll = emulator.get_latest_configuration(&control_token).await;
        assert_eq!(
            control_poll.status,
            200,
            "case {name}: the control session's poll expected 200, got {} body {}",
            control_poll.status,
            control_poll.text()
        );
        assert!(
            control_poll.body.is_empty(),
            "case {name}: the control session holds the latest version, so its poll must \
             serve no content, got {:?}",
            control_poll.body.as_ref()
        );

        // A session started AFTER the removal is judged by the control plane: that is
        // where an unresolvable level is refused, and a session bound to no configuration
        // at all is the one case the poll reports as not found.
        assert_after_removal(&emulator, &victim, case).await;
    }
}

/// Remove each level the row names, through AppConfig's own delete operations, and read
/// the removal back so what follows is asserted against a deletion the control plane
/// really performed.
async fn remove(emulator: &Emulator, victim: &Hierarchy, removed: &[Removal]) {
    let application_id = &victim.application_id;
    let environment_id = &victim.environment_id;
    let profile_id = &victim.profile_id;
    for removal in removed {
        let paths = match removal {
            // A profile OWNS its hosted configuration versions and AppConfig does not
            // cascade them away, so each version the control plane reports is deleted by
            // the number IT reports.
            Removal::HostedVersions => hosted_version_numbers(emulator, victim)
                .await
                .into_iter()
                .map(|number| {
                    format!(
                        "/applications/{application_id}/configurationprofiles/{profile_id}\
                         /hostedconfigurationversions/{number}"
                    )
                })
                .collect(),
            Removal::ConfigurationProfile => vec![format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}"
            )],
            Removal::Environment => vec![format!(
                "/applications/{application_id}/environments/{environment_id}"
            )],
            Removal::Application => vec![format!("/applications/{application_id}")],
        };
        for path in paths {
            let deleted = emulator
                .send("DELETE", &path, "application/json", &[], Bytes::new())
                .await;
            assert_eq!(
                deleted.status,
                204,
                "DELETE {path}: expected 204, got {} body {}",
                deleted.status,
                deleted.text()
            );
            let read_back = emulator
                .send("GET", &path, "application/json", &[], Bytes::new())
                .await;
            assert_eq!(
                read_back.status,
                404,
                "GET {path} after its delete: expected 404, got {} body {}",
                read_back.status,
                read_back.text()
            );
        }
    }
}

/// The hosted configuration version numbers the control plane reports for the victim's
/// configuration profile.
async fn hosted_version_numbers(emulator: &Emulator, victim: &Hierarchy) -> Vec<i64> {
    let application_id = &victim.application_id;
    let profile_id = &victim.profile_id;
    let listed = emulator
        .send(
            "GET",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}\
                 /hostedconfigurationversions"
            ),
            "application/json",
            &[],
            Bytes::new(),
        )
        .await;
    assert_eq!(
        listed.status,
        200,
        "ListHostedConfigurationVersions: expected 200, got {} body {}",
        listed.status,
        listed.text()
    );
    listed.json()["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("no Items in {}", listed.text()))
        .iter()
        .map(|item| {
            item["VersionNumber"]
                .as_i64()
                .unwrap_or_else(|| panic!("no VersionNumber in {item}"))
        })
        .collect()
}

/// Start a fresh session over the victim's triple after the removal and assert the row's
/// [`AfterRemoval`]: an unresolvable level is refused by `StartConfigurationSession`, and
/// a session the triple still resolves for is minted and then has nothing to serve.
async fn assert_after_removal(emulator: &Emulator, victim: &Hierarchy, case: &Case) {
    let name = case.name;
    let started = emulator
        .post_json(
            "/configurationsessions",
            &format!(
                r#"{{"ApplicationIdentifier":"{}",
                      "EnvironmentIdentifier":"{}",
                      "ConfigurationProfileIdentifier":"{}"}}"#,
                victim.application_name, victim.environment_name, victim.profile_name
            ),
        )
        .await;
    match case.after {
        AfterRemoval::SessionRefused { resource_type } => {
            assert_eq!(
                started.status,
                404,
                "case {name}: a session over the removed triple expected 404, got {} body {}",
                started.status,
                started.text()
            );
            assert_eq!(
                started.error_code(),
                "ResourceNotFoundException",
                "case {name}: expected ResourceNotFoundException, got {}",
                started.text()
            );
            assert_eq!(
                started.json()["ResourceType"].as_str(),
                Some(resource_type),
                "case {name}: expected ResourceType {resource_type}, got {}",
                started.text()
            );
            assert_eq!(
                started.json()["Message"].as_str(),
                Some(format!("{resource_type} not found").as_str()),
                "case {name}: expected the level's fixed not-found sentence, got {}",
                started.text()
            );
        }
        AfterRemoval::PollRefused { resource_type } => {
            assert_eq!(
                started.status,
                201,
                "case {name}: the triple still resolves, so the session expected 201, got {} \
                 body {}",
                started.status,
                started.text()
            );
            let token = started.string("InitialConfigurationToken");
            let poll = emulator.get_latest_configuration(&token).await;
            assert_eq!(
                poll.status,
                404,
                "case {name}: a session bound to no configuration expected 404, got {} body {}",
                poll.status,
                poll.text()
            );
            assert_eq!(
                poll.error_code(),
                "ResourceNotFoundException",
                "case {name}: expected ResourceNotFoundException, got {}",
                poll.text()
            );
            assert_eq!(
                poll.json()["ResourceType"].as_str(),
                Some(resource_type),
                "case {name}: expected ResourceType {resource_type}, got {}",
                poll.text()
            );
        }
    }
}
