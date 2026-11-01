// SPDX-License-Identifier: CC0-1.0

//! The update contract the GameLift resource updates share, over the wire: an `Update*`
//! call writes every property its request supplies onto the STORED resource, leaves every
//! property the request omits as it was, and reports that one stored record — so the
//! matching `Describe*` serves the updated values back, and a property nobody updated
//! still reads as it was created.
//!
//! The suites below drive `UpdateAlias`, `UpdateBuild`,
//! `UpdateMatchmakingConfiguration` and `UpdateScript` against `DescribeAlias`,
//! `DescribeBuild`, `DescribeMatchmakingConfigurations` and `DescribeScript`, selecting
//! the resource by its id or name and by its ARN.
//!
//! The harness (`GameLift`, `gamelift_arn`) is the one `hosting_loop` already drives the
//! production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{gamelift_arn, GameLift, REGION};

/// The modelled maximum of the metadata strings these updates carry
/// (`smithy_query operation=UpdateAlias`/`UpdateBuild`/`UpdateScript`:
/// `NonZeroAndMaxString`, len 1..1024).
const MAX_METADATA_LENGTH: usize = 1024;

/// The modelled bounds of `UpdateMatchmakingConfigurationInput$RequestTimeoutSeconds`
/// (`smithy_query operation=UpdateMatchmakingConfiguration`:
/// `MatchmakingRequestTimeoutInteger`, range 1..43200).
const MIN_REQUEST_TIMEOUT_SECONDS: i64 = 1;
/// See [`MIN_REQUEST_TIMEOUT_SECONDS`].
const MAX_REQUEST_TIMEOUT_SECONDS: i64 = 43_200;

/// The modelled bounds of `UpdateMatchmakingConfigurationInput$AcceptanceTimeoutSeconds`
/// (`MatchmakingAcceptanceTimeoutInteger`, range 1..600).
const MIN_ACCEPTANCE_TIMEOUT_SECONDS: i64 = 1;
/// See [`MIN_ACCEPTANCE_TIMEOUT_SECONDS`].
const MAX_ACCEPTANCE_TIMEOUT_SECONDS: i64 = 600;

/// A caller-owned S3 storage location naming the three members a script's storage location
/// documents as required: "the storage location must specify the Amazon S3 bucket name,
/// the zip file name (the "key"), and a role ARN that allows Amazon GameLift Servers to
/// access the Amazon S3 storage location".
fn caller_location(bucket: &str, key: &str) -> Value {
    json!({
        "Bucket": bucket,
        "Key": key,
        "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
    })
}

/// Where a Realtime script uploaded from a local zip file is held: the service's own
/// script bucket for the Region the upload was served in, under the account that owns the
/// script and the script that holds the files.
pub(crate) fn service_owned_script_location(script_id: &str) -> Value {
    json!({
        "Bucket": format!("prod-gamescale-scripts-{REGION}"),
        "Key": format!("{}/{script_id}", engine::emulator_account_id()),
    })
}

/// `len` deterministic bytes standing in for a zipped Realtime script, opening with the
/// local-file signature a zip archive carries (`PK\x03\x04`) when there is room for it.
fn zip_bytes(len: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = b"PK\x03\x04".to_vec();
    bytes.truncate(len);
    while bytes.len() < len {
        bytes.push((bytes.len() % 251) as u8);
    }
    bytes
}

/// A blob travels base64-encoded on awsJson1_1, which is what the wire serializer does
/// with it.
fn zip_file_member(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// `before` with each member of `changes` replaced by the value the update supplied: what
/// the resource must read back as when the update changed exactly those members and
/// nothing else.
pub(crate) fn only_changed(before: &Value, changes: &[(&str, Value)]) -> Value {
    let mut expected = before.clone();
    for (member, value) in changes {
        expected[*member] = value.clone();
    }
    expected
}

/// The members `changes` names, for an assertion message.
fn changed_members<'a>(changes: &[(&'a str, Value)]) -> Vec<&'a str> {
    changes.iter().map(|(member, _)| *member).collect()
}

/// The alias `DescribeAlias` reports for `selector`, an alias id or an alias ARN.
pub(crate) async fn described_alias(gamelift: &GameLift, selector: &str) -> Value {
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": selector}))
        .await;
    described["Alias"].clone()
}

/// The build `DescribeBuild` reports for `selector`, a build id or a build ARN.
async fn described_build(gamelift: &GameLift, selector: &str) -> Value {
    let described = gamelift
        .ok("DescribeBuild", json!({"BuildId": selector}))
        .await;
    described["Build"].clone()
}

/// The script `DescribeScript` reports for `selector`, a script id or a script ARN.
async fn described_script(gamelift: &GameLift, selector: &str) -> Value {
    let described = gamelift
        .ok("DescribeScript", json!({"ScriptId": selector}))
        .await;
    described["Script"].clone()
}

/// The ONE matchmaking configuration `DescribeMatchmakingConfigurations` reports for
/// `selector`, a configuration name or a configuration ARN.
pub(crate) async fn described_configuration(gamelift: &GameLift, selector: &str) -> Value {
    let described = gamelift
        .ok(
            "DescribeMatchmakingConfigurations",
            json!({"Names": [selector]}),
        )
        .await;
    let configurations = described["Configurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        configurations.len(),
        1,
        "the describe of {selector:?} must report exactly that configuration, got {described}"
    );
    configurations[0].clone()
}

/// A terminal alias carrying a creation description and message, which the updates below
/// change: "TERMINAL - The alias does not resolve to a fleet but instead can be used to
/// display a message to the user."
async fn create_terminal_alias(gamelift: &GameLift, name: &str) -> String {
    let created = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": name,
                "Description": "the description the create supplied",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "the message the create supplied"},
            }),
        )
        .await;
    created["Alias"]["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string()
}

/// A build whose files are uploaded to the service-owned location, carrying the metadata
/// the updates below change.
async fn create_build(gamelift: &GameLift, name: &str) -> String {
    let created = gamelift
        .ok(
            "CreateBuild",
            json!({
                "Name": name,
                "Version": "1.0.0",
                "OperatingSystem": "AMAZON_LINUX_2023",
            }),
        )
        .await;
    created["Build"]["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string()
}

/// A Realtime script uploaded from a local zip file, carrying the metadata the updates
/// below change.
async fn create_script(gamelift: &GameLift, name: &str) -> String {
    let created = gamelift
        .ok(
            "CreateScript",
            json!({
                "Name": name,
                "Version": "1.0.0",
                "ZipFile": zip_file_member(&zip_bytes(256)),
            }),
        )
        .await;
    created["Script"]["ScriptId"]
        .as_str()
        .expect("CreateScript must report a ScriptId")
        .to_string()
}

/// A rule set named `name` and a matchmaking configuration on it, matching the one the
/// updates below change. Answers the configuration's name.
pub(crate) async fn create_matchmaker(gamelift: &GameLift, name: &str) -> String {
    let body = json!({
        "name": "solo-duel",
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string();
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": format!("{name}-rules"), "RuleSetBody": &body}),
        )
        .await;
    gamelift
        .ok(
            "CreateMatchmakingConfiguration",
            json!({
                "Name": name,
                "RuleSetName": format!("{name}-rules"),
                "RequestTimeoutSeconds": 60,
                "AcceptanceRequired": false,
                "Description": "the description the create supplied",
                "FlexMatchMode": "STANDALONE",
            }),
        )
        .await;
    name.to_string()
}

/// The instant a modelled timestamp member carries, as epoch seconds.
fn instant(record: &Value, member: &str) -> f64 {
    record[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be a modelled timestamp, got {record}"))
}

/// `UpdateAlias` writes the properties the request supplies onto the stored alias and
/// leaves the rest as they were, so `DescribeAlias` serves the updated alias.
///
/// The rows differ along what the rule generalises over: which properties the request
/// supplies, both modelled `RoutingStrategyType` values (so a routing strategy is both
/// re-messaged and reassigned onto a fleet), the text and length of the strings supplied
/// (including the modelled maximum), and a request that supplies no property at all.
#[tokio::test]
async fn update_alias_writes_the_supplied_properties_onto_the_stored_alias() {
    struct Case {
        label: &'static str,
        update: Value,
        /// The members the update must change, as they must read back.
        expected: Vec<(&'static str, Value)>,
    }

    let cases = [
        Case {
            label: "the description alone",
            update: json!({"Description": "retired for the season"}),
            expected: vec![("Description", json!("retired for the season"))],
        },
        Case {
            label: "the name alone",
            update: json!({"Name": "renamed-by-the-update"}),
            expected: vec![("Name", json!("renamed-by-the-update"))],
        },
        Case {
            label: "a new terminal message, the text players are shown",
            update: json!({
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "Upgrade at https://example.test/upgrade"},
            }),
            expected: vec![(
                "RoutingStrategy",
                json!({"Type": "TERMINAL", "Message": "Upgrade at https://example.test/upgrade"}),
            )],
        },
        Case {
            label: "reassigned onto a fleet, the other modelled routing type",
            update: json!({"RoutingStrategy": {"Type": "SIMPLE", "FleetId": "fleet-reassigned"}}),
            expected: vec![(
                "RoutingStrategy",
                json!({"Type": "SIMPLE", "FleetId": "fleet-reassigned"}),
            )],
        },
        Case {
            label: "a description at the modelled maximum length",
            update: json!({"Description": "d".repeat(MAX_METADATA_LENGTH)}),
            expected: vec![("Description", json!("d".repeat(MAX_METADATA_LENGTH)))],
        },
        Case {
            label: "a one-character description, the modelled minimum",
            update: json!({"Description": "d"}),
            expected: vec![("Description", json!("d"))],
        },
        Case {
            label: "every updatable property at once",
            update: json!({
                "Name": "all-at-once",
                "Description": "all at once",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "all at once"},
            }),
            expected: vec![
                ("Name", json!("all-at-once")),
                ("Description", json!("all at once")),
                (
                    "RoutingStrategy",
                    json!({"Type": "TERMINAL", "Message": "all at once"}),
                ),
            ],
        },
        Case {
            label: "no property at all",
            update: json!({}),
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let alias_id = create_terminal_alias(&gamelift, &format!("update-target-{index}")).await;
        let before = described_alias(&gamelift, &alias_id).await;

        let mut request = case.update.clone();
        request["AliasId"] = json!(&alias_id);
        let updated = gamelift.ok("UpdateAlias", request).await;
        let after = described_alias(&gamelift, &alias_id).await;
        assert_eq!(
            updated["Alias"], after,
            "case {label:?}: UpdateAlias must report the stored alias the next describe serves, got {}",
            updated["Alias"]
        );

        // `Alias$LastUpdatedTime` is "the time that this data object was last modified": it
        // moves for an update that changed a property, and an update that changed nothing
        // leaves it where it was.
        if case.expected.is_empty() {
            assert_eq!(
                after["LastUpdatedTime"], before["LastUpdatedTime"],
                "case {label:?}: an update that changes nothing must leave LastUpdatedTime, got {after}"
            );
        } else {
            let (was, now) = (
                instant(&before, "LastUpdatedTime"),
                instant(&after, "LastUpdatedTime"),
            );
            assert!(
                now >= was,
                "case {label:?}: LastUpdatedTime must move forward for a modified alias, got {now} after {was}"
            );
        }

        let mut expected = only_changed(&before, &case.expected);
        expected["LastUpdatedTime"] = after["LastUpdatedTime"].clone();
        assert_eq!(
            after,
            expected,
            "case {label:?}: the update must change only {:?}, got {after}",
            changed_members(&case.expected)
        );
    }
}

/// `UpdateBuild` writes the metadata the request supplies onto the stored build and leaves
/// the rest as it was, so `DescribeBuild` serves the updated build.
///
/// The rows differ along what the rule generalises over: which members the request
/// supplies, the form of the version string (a semantic version, a build number, a dated
/// label), its length at the modelled bounds, and a request that supplies nothing.
#[tokio::test]
async fn update_build_writes_the_supplied_metadata_onto_the_stored_build() {
    struct Case {
        label: &'static str,
        update: Value,
        expected: Vec<(&'static str, Value)>,
    }

    let cases = [
        Case {
            label: "the version alone, a patch release",
            update: json!({"Version": "1.0.1"}),
            expected: vec![("Version", json!("1.0.1"))],
        },
        Case {
            label: "the version alone, a build number",
            update: json!({"Version": "4821"}),
            expected: vec![("Version", json!("4821"))],
        },
        Case {
            label: "the version alone, a dated label",
            update: json!({"Version": "release-2026.03.14"}),
            expected: vec![("Version", json!("release-2026.03.14"))],
        },
        Case {
            label: "the name alone",
            update: json!({"Name": "renamed-build"}),
            expected: vec![("Name", json!("renamed-build"))],
        },
        Case {
            label: "the name and the version together",
            update: json!({"Name": "promoted-build", "Version": "2.0.0"}),
            expected: vec![
                ("Name", json!("promoted-build")),
                ("Version", json!("2.0.0")),
            ],
        },
        Case {
            label: "a version at the modelled maximum length",
            update: json!({"Version": "v".repeat(MAX_METADATA_LENGTH)}),
            expected: vec![("Version", json!("v".repeat(MAX_METADATA_LENGTH)))],
        },
        Case {
            label: "a one-character version, the modelled minimum",
            update: json!({"Version": "9"}),
            expected: vec![("Version", json!("9"))],
        },
        Case {
            label: "no metadata at all",
            update: json!({}),
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let build_id = create_build(&gamelift, &format!("update-target-{index}")).await;
        let before = described_build(&gamelift, &build_id).await;

        let mut request = case.update.clone();
        request["BuildId"] = json!(&build_id);
        let updated = gamelift.ok("UpdateBuild", request).await;
        let after = described_build(&gamelift, &build_id).await;
        assert_eq!(
            updated["Build"], after,
            "case {label:?}: UpdateBuild must report the stored build the next describe serves, got {}",
            updated["Build"]
        );
        assert_eq!(
            after,
            only_changed(&before, &case.expected),
            "case {label:?}: the update must change only {:?}, got {after}",
            changed_members(&case.expected)
        );
    }
}

/// `UpdateMatchmakingConfiguration` merges every setting the request supplies into the
/// stored configuration and leaves the rest as they were, so
/// `DescribeMatchmakingConfigurations` serves the updated configuration.
///
/// The rows differ along what the rule generalises over: which settings the request
/// supplies, the type of each one (string, integer, boolean, enum, list), the integers at
/// each modelled bound, how many game properties the list carries, and a request that
/// supplies no setting at all.
#[tokio::test]
async fn update_matchmaking_configuration_merges_every_supplied_setting() {
    struct Case {
        label: &'static str,
        update: Value,
        expected: Vec<(&'static str, Value)>,
    }

    let cases = vec![
        Case {
            label: "the description alone",
            update: json!({"Description": "the description the update supplied"}),
            expected: vec![(
                "Description",
                json!("the description the update supplied"),
            )],
        },
        Case {
            label: "player acceptance turned on with the time players are given",
            update: json!({"AcceptanceRequired": true, "AcceptanceTimeoutSeconds": 120}),
            expected: vec![
                ("AcceptanceRequired", json!(true)),
                ("AcceptanceTimeoutSeconds", json!(120)),
            ],
        },
        Case {
            label: "the acceptance timeout at the modelled minimum",
            update: json!({
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": MIN_ACCEPTANCE_TIMEOUT_SECONDS,
            }),
            expected: vec![
                ("AcceptanceRequired", json!(true)),
                (
                    "AcceptanceTimeoutSeconds",
                    json!(MIN_ACCEPTANCE_TIMEOUT_SECONDS),
                ),
            ],
        },
        Case {
            label: "the acceptance timeout at the modelled maximum",
            update: json!({
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": MAX_ACCEPTANCE_TIMEOUT_SECONDS,
            }),
            expected: vec![
                ("AcceptanceRequired", json!(true)),
                (
                    "AcceptanceTimeoutSeconds",
                    json!(MAX_ACCEPTANCE_TIMEOUT_SECONDS),
                ),
            ],
        },
        Case {
            label: "the request timeout at the modelled minimum",
            update: json!({"RequestTimeoutSeconds": MIN_REQUEST_TIMEOUT_SECONDS}),
            expected: vec![(
                "RequestTimeoutSeconds",
                json!(MIN_REQUEST_TIMEOUT_SECONDS),
            )],
        },
        Case {
            label: "the request timeout at the modelled maximum",
            update: json!({"RequestTimeoutSeconds": MAX_REQUEST_TIMEOUT_SECONDS}),
            expected: vec![(
                "RequestTimeoutSeconds",
                json!(MAX_REQUEST_TIMEOUT_SECONDS),
            )],
        },
        Case {
            label: "slots kept open for future players",
            update: json!({"AdditionalPlayerCount": 2}),
            expected: vec![("AdditionalPlayerCount", json!(2))],
        },
        Case {
            label: "no slot kept open, the modelled minimum of a whole number",
            update: json!({"AdditionalPlayerCount": 0}),
            expected: vec![("AdditionalPlayerCount", json!(0))],
        },
        Case {
            label: "the backfill mode, an enum setting",
            update: json!({"BackfillMode": "MANUAL"}),
            expected: vec![("BackfillMode", json!("MANUAL"))],
        },
        Case {
            label: "one game property, a list setting",
            update: json!({"GameProperties": [{"Key": "difficulty", "Value": "novice"}]}),
            expected: vec![(
                "GameProperties",
                json!([{"Key": "difficulty", "Value": "novice"}]),
            )],
        },
        Case {
            label: "three game properties",
            update: json!({
                "GameProperties": [
                    {"Key": "difficulty", "Value": "hard"},
                    {"Key": "map", "Value": "canyon"},
                    {"Key": "mode", "Value": "ranked"},
                ],
            }),
            expected: vec![(
                "GameProperties",
                json!([
                    {"Key": "difficulty", "Value": "hard"},
                    {"Key": "map", "Value": "canyon"},
                    {"Key": "mode", "Value": "ranked"},
                ]),
            )],
        },
        Case {
            label: "the custom event data and the game session data",
            update: json!({
                "CustomEventData": "liveops-2026",
                "GameSessionData": "{\"mode\":\"ranked\"}",
            }),
            expected: vec![
                ("CustomEventData", json!("liveops-2026")),
                ("GameSessionData", json!("{\"mode\":\"ranked\"}")),
            ],
        },
        Case {
            label: "the notification target",
            update: json!({"NotificationTarget": "arn:aws:sns:us-west-2:123456789012:matchmaking"}),
            expected: vec![(
                "NotificationTarget",
                json!("arn:aws:sns:us-west-2:123456789012:matchmaking"),
            )],
        },
        Case {
            label: "every setting the recorded divergence names, at once",
            update: json!({
                "Description": "all four at once",
                "RequestTimeoutSeconds": 300,
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": 90,
            }),
            expected: vec![
                ("Description", json!("all four at once")),
                ("RequestTimeoutSeconds", json!(300)),
                ("AcceptanceRequired", json!(true)),
                ("AcceptanceTimeoutSeconds", json!(90)),
            ],
        },
        Case {
            label: "no setting at all",
            update: json!({}),
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = create_matchmaker(&gamelift, &format!("update-target-{index}")).await;
        let before = described_configuration(&gamelift, &name).await;

        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let updated = gamelift
            .ok("UpdateMatchmakingConfiguration", request)
            .await;
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            updated["Configuration"], after,
            "case {label:?}: the update must report the configuration the next describe serves, got {}",
            updated["Configuration"]
        );
        assert_eq!(
            after,
            only_changed(&before, &case.expected),
            "case {label:?}: the update must change only {:?}, got {after}",
            changed_members(&case.expected)
        );
    }
}

/// A matchmaking configuration can be moved onto another rule set that exists, which
/// changes both the name and the ARN it reports; a rule set that does not exist is
/// refused and the stored configuration is untouched.
#[tokio::test]
async fn update_matchmaking_configuration_moves_only_onto_a_rule_set_that_exists() {
    let gamelift = GameLift::new();
    let name = create_matchmaker(&gamelift, "rule-set-move").await;
    let before = described_configuration(&gamelift, &name).await;

    // Another rule set, created the same way the matchmaker's own was.
    let other = create_matchmaker(&gamelift, "other-matchmaker").await;
    let other_rules = format!("{other}-rules");

    for selector in [other_rules.clone(), gamelift_arn(&format!("matchmakingruleset/{other_rules}"))] {
        let updated = gamelift
            .ok(
                "UpdateMatchmakingConfiguration",
                json!({"Name": &name, "RuleSetName": &selector}),
            )
            .await;
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            updated["Configuration"], after,
            "case {selector:?}: the update must report the stored configuration, got {}",
            updated["Configuration"]
        );
        assert_eq!(
            after,
            only_changed(
                &before,
                &[
                    ("RuleSetName", json!(&other_rules)),
                    (
                        "RuleSetArn",
                        json!(gamelift_arn(&format!("matchmakingruleset/{other_rules}"))),
                    ),
                ],
            ),
            "case {selector:?}: the configuration must name the rule set it was moved onto, got {after}"
        );
    }

    let (status, code) = gamelift
        .err(
            "UpdateMatchmakingConfiguration",
            json!({"Name": &name, "RuleSetName": "no-such-rule-set"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a move onto a rule set that does not exist must be refused, got {status} {code}"
    );
    let after = described_configuration(&gamelift, &name).await;
    assert_eq!(
        after["RuleSetName"],
        json!(&other_rules),
        "the refused update must have written nothing, got {after}"
    );
}

/// An update that would leave the configuration outside a documented combination is
/// refused, and nothing is written — the rules hold over the configuration the update
/// WOULD leave behind, not only over the settings one request happens to name.
#[tokio::test]
async fn update_matchmaking_configuration_refuses_a_combination_the_service_documents_against() {
    struct Case {
        label: &'static str,
        update: Value,
    }

    let cases = vec![
        Case {
            label: "two queues, where a configuration supports only one",
            update: json!({
                "FlexMatchMode": "WITH_QUEUE",
                "GameSessionQueueArns": [
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one",
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/two",
                ],
            }),
        },
        Case {
            label: "a queue on a configuration that stays standalone",
            update: json!({
                "GameSessionQueueArns": [
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one",
                ],
            }),
        },
        Case {
            label: "standalone mode turned on for a queue the update itself supplies",
            update: json!({
                "FlexMatchMode": "STANDALONE",
                "GameSessionQueueArns": [
                    "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one",
                ],
            }),
        },
    ];

    let gamelift = GameLift::new();
    let name = create_matchmaker(&gamelift, "documented-combinations").await;
    let before = described_configuration(&gamelift, &name).await;

    for case in &cases {
        let label = case.label;
        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let (status, code) = gamelift
            .err("UpdateMatchmakingConfiguration", request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the update must be refused, got {status} {code}"
        );
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused update must write nothing, got {after}"
        );
    }

    // The same configuration takes a queue once it is moved out of standalone mode in the
    // SAME request: the rule is about the configuration the update leaves behind.
    let queue_arn = "arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one";
    let updated = gamelift
        .ok(
            "UpdateMatchmakingConfiguration",
            json!({
                "Name": &name,
                "FlexMatchMode": "WITH_QUEUE",
                "GameSessionQueueArns": [queue_arn],
                "BackfillMode": "AUTOMATIC",
            }),
        )
        .await;
    let after = described_configuration(&gamelift, &name).await;
    assert_eq!(
        updated["Configuration"], after,
        "the accepted update must report the stored configuration, got {}",
        updated["Configuration"]
    );
    assert_eq!(
        after,
        only_changed(
            &before,
            &[
                ("FlexMatchMode", json!("WITH_QUEUE")),
                ("GameSessionQueueArns", json!([queue_arn])),
                ("BackfillMode", json!("AUTOMATIC")),
            ],
        ),
        "a configuration moved out of standalone mode must carry the queue and the backfill mode, got {after}"
    );
}

/// `UpdateScript` writes the metadata and the content source the request supplies onto the
/// stored script and leaves the rest as they were, so `DescribeScript` serves the updated
/// script.
///
/// The rows differ along what the rule generalises over: which members the request
/// supplies, the form and length of the version string, which of the two documented
/// content sources the revised zip file comes from, and the size of a locally uploaded zip
/// file — "when files are uploaded from an S3 location, this value remains at 0", so only
/// the local upload reports a size.
#[tokio::test]
async fn update_script_writes_the_supplied_metadata_and_content_onto_the_stored_script() {
    /// Where a row's revised content comes from, which decides the storage location and
    /// the size the script reports.
    enum Content {
        /// No new content: the stored location and size stay as they were.
        Unchanged,
        /// A local zip file of this many bytes, sent in `ZipFile`.
        LocalZip(usize),
        /// An S3 object the caller owns, named by `StorageLocation`.
        CallerBucket(&'static str, &'static str),
    }

    struct Case {
        label: &'static str,
        update: Value,
        content: Content,
        /// The metadata members the update must change, as they must read back.
        expected: Vec<(&'static str, Value)>,
    }

    let cases = [
        Case {
            label: "the version alone, a patch release",
            update: json!({"Version": "1.0.1"}),
            content: Content::Unchanged,
            expected: vec![("Version", json!("1.0.1"))],
        },
        Case {
            label: "the version alone, a rollout label",
            update: json!({"Version": "canary-7"}),
            content: Content::Unchanged,
            expected: vec![("Version", json!("canary-7"))],
        },
        Case {
            label: "the name alone",
            update: json!({"Name": "renamed-script"}),
            content: Content::Unchanged,
            expected: vec![("Name", json!("renamed-script"))],
        },
        Case {
            label: "the name and the version together",
            update: json!({"Name": "rolled-out", "Version": "2.0.0"}),
            content: Content::Unchanged,
            expected: vec![("Name", json!("rolled-out")), ("Version", json!("2.0.0"))],
        },
        Case {
            label: "a version at the modelled maximum length",
            update: json!({"Version": "v".repeat(MAX_METADATA_LENGTH)}),
            content: Content::Unchanged,
            expected: vec![("Version", json!("v".repeat(MAX_METADATA_LENGTH)))],
        },
        Case {
            label: "a revised local zip file, with the version it carries",
            update: json!({"Version": "3.0.0"}),
            content: Content::LocalZip(1_024),
            expected: vec![("Version", json!("3.0.0"))],
        },
        Case {
            label: "a revised local zip file of one byte",
            update: json!({}),
            content: Content::LocalZip(1),
            expected: vec![],
        },
        Case {
            label: "a revised zip file in the caller's own bucket",
            update: json!({"Version": "4.0.0"}),
            content: Content::CallerBucket("my-own-game-scripts", "scripts/realtime.zip"),
            expected: vec![("Version", json!("4.0.0"))],
        },
        Case {
            label: "a revised zip file at another key of another bucket",
            update: json!({}),
            content: Content::CallerBucket("studio-artifacts", "builds/realtime-v5.zip"),
            expected: vec![],
        },
        Case {
            label: "no member at all",
            update: json!({}),
            content: Content::Unchanged,
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let script_id = create_script(&gamelift, &format!("update-target-{index}")).await;
        let before = described_script(&gamelift, &script_id).await;

        let mut request = case.update.clone();
        request["ScriptId"] = json!(&script_id);
        let mut expected = case.expected.clone();
        match &case.content {
            Content::Unchanged => {}
            Content::LocalZip(len) => {
                request["ZipFile"] = zip_file_member(&zip_bytes(*len));
                // A local upload is held at the service-owned location the create path
                // also uses, which names the script that holds it, and its size is known.
                expected.push(("SizeOnDisk", json!(*len)));
            }
            Content::CallerBucket(bucket, key) => {
                request["StorageLocation"] = caller_location(bucket, key);
                expected.push(("StorageLocation", caller_location(bucket, key)));
                expected.push(("SizeOnDisk", json!(0)));
            }
        }

        let updated = gamelift.ok("UpdateScript", request).await;
        let after = described_script(&gamelift, &script_id).await;
        assert_eq!(
            updated["Script"], after,
            "case {label:?}: UpdateScript must report the stored script the next describe serves, got {}",
            updated["Script"]
        );

        if matches!(case.content, Content::LocalZip(_)) {
            assert_eq!(
                after["StorageLocation"],
                service_owned_script_location(&script_id),
                "case {label:?}: a revised local upload must be held at the service-owned location for this script, got {}",
                after["StorageLocation"]
            );
            expected.push((
                "StorageLocation",
                service_owned_script_location(&script_id),
            ));
        }

        assert_eq!(
            after,
            only_changed(&before, &expected),
            "case {label:?}: the update must change only {:?}, got {after}",
            changed_members(&expected)
        );
    }
}

/// The two content sources are the two alternatives of one choice, so a request that names
/// both is refused and writes nothing; so is a storage location missing one of the members
/// a location must specify.
#[tokio::test]
async fn update_script_refuses_a_content_source_outside_the_documented_contract() {
    struct Case {
        label: &'static str,
        update: Value,
    }

    let cases = vec![
        Case {
            label: "both a local zip file and a storage location",
            update: json!({
                "ZipFile": zip_file_member(&zip_bytes(64)),
                "StorageLocation": caller_location("my-own-game-scripts", "scripts/realtime.zip"),
            }),
        },
        Case {
            label: "a storage location naming no bucket",
            update: json!({
                "StorageLocation": {
                    "Key": "scripts/realtime.zip",
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
                },
            }),
        },
        Case {
            label: "a storage location naming no key",
            update: json!({
                "StorageLocation": {
                    "Bucket": "my-own-game-scripts",
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftScriptAccess",
                },
            }),
        },
        Case {
            label: "a storage location carrying no role ARN",
            update: json!({
                "StorageLocation": {
                    "Bucket": "my-own-game-scripts",
                    "Key": "scripts/realtime.zip",
                },
            }),
        },
    ];

    let gamelift = GameLift::new();
    let script_id = create_script(&gamelift, "content-contract").await;
    let before = described_script(&gamelift, &script_id).await;

    for case in &cases {
        let label = case.label;
        let mut request = case.update.clone();
        request["ScriptId"] = json!(&script_id);
        let (status, code) = gamelift.err("UpdateScript", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the update must be refused, got {status} {code}"
        );
        let after = described_script(&gamelift, &script_id).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused update must write nothing, got {after}"
        );
    }
}

/// Each update selects its resource by id or name AND by ARN — "the resource ID matches
/// the <resource> value" — so both forms reach the same stored record.
#[tokio::test]
async fn an_update_reaches_the_same_resource_by_id_and_by_arn() {
    let gamelift = GameLift::new();

    let alias_id = create_terminal_alias(&gamelift, "arn-selected-alias").await;
    let updated = gamelift
        .ok(
            "UpdateAlias",
            json!({
                "AliasId": gamelift_arn(&format!("alias/{alias_id}")),
                "Description": "selected by ARN",
            }),
        )
        .await;
    assert_eq!(
        updated["Alias"]["Description"],
        json!("selected by ARN"),
        "an alias selected by ARN must be updated, got {}",
        updated["Alias"]
    );
    assert_eq!(
        described_alias(&gamelift, &alias_id).await["Description"],
        json!("selected by ARN"),
        "the alias the ARN names is the one the id names"
    );

    let build_id = create_build(&gamelift, "arn-selected-build").await;
    gamelift
        .ok(
            "UpdateBuild",
            json!({
                "BuildId": gamelift_arn(&format!("build/{build_id}")),
                "Version": "1.0.1",
            }),
        )
        .await;
    assert_eq!(
        described_build(&gamelift, &build_id).await["Version"],
        json!("1.0.1"),
        "a build selected by ARN must be updated"
    );

    let script_id = create_script(&gamelift, "arn-selected-script").await;
    gamelift
        .ok(
            "UpdateScript",
            json!({
                "ScriptId": gamelift_arn(&format!("script/{script_id}")),
                "Version": "1.0.1",
            }),
        )
        .await;
    assert_eq!(
        described_script(&gamelift, &script_id).await["Version"],
        json!("1.0.1"),
        "a script selected by ARN must be updated"
    );

    let name = create_matchmaker(&gamelift, "arn-selected-matchmaker").await;
    gamelift
        .ok(
            "UpdateMatchmakingConfiguration",
            json!({
                "Name": gamelift_arn(&format!("matchmakingconfiguration/{name}")),
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": 60,
            }),
        )
        .await;
    let after = described_configuration(&gamelift, &name).await;
    assert_eq!(
        (
            after["AcceptanceRequired"].clone(),
            after["AcceptanceTimeoutSeconds"].clone()
        ),
        (json!(true), json!(60)),
        "a configuration selected by ARN must be updated, got {after}"
    );
}

/// A setting the SERVICE decides, because the create left it out, is part of the stored
/// configuration: every read reports it, and an update that does not name it keeps it.
///
/// The rows differ along which of the two defaulted settings the create supplies and which
/// it leaves to the service, and along the mode the configuration runs in.
#[tokio::test]
async fn a_service_applied_default_is_stored_and_survives_an_update() {
    struct Case {
        label: &'static str,
        /// The create request, without its name and rule set.
        create: Value,
        /// What the stored configuration must report for the two defaulted settings.
        additional_player_count: Value,
        backfill_mode: Value,
    }

    let cases = [
        Case {
            label: "neither setting supplied, so the service decides both",
            create: json!({"FlexMatchMode": "STANDALONE"}),
            additional_player_count: json!(0),
            backfill_mode: json!("MANUAL"),
        },
        Case {
            label: "slots kept open by the request, the backfill mode left to the service",
            create: json!({"FlexMatchMode": "STANDALONE", "AdditionalPlayerCount": 3}),
            additional_player_count: json!(3),
            backfill_mode: json!("MANUAL"),
        },
        Case {
            label: "the backfill mode supplied, the player count left to the service",
            create: json!({"FlexMatchMode": "STANDALONE", "BackfillMode": "MANUAL"}),
            additional_player_count: json!(0),
            backfill_mode: json!("MANUAL"),
        },
        Case {
            label: "a queued matchmaker that backfills automatically",
            create: json!({
                "FlexMatchMode": "WITH_QUEUE",
                "BackfillMode": "AUTOMATIC",
                "GameSessionQueueArns": ["arn:aws:gamelift:us-west-2:123456789012:gamesessionqueue/one"],
            }),
            additional_player_count: json!(0),
            backfill_mode: json!("AUTOMATIC"),
        },
    ];

    let gamelift = GameLift::new();
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({
                "Name": "defaults-rules",
                "RuleSetBody": json!({
                    "ruleLanguageVersion": "1.0",
                    "teams": [{"name": "solo", "maxPlayers": 2}],
                })
                .to_string(),
            }),
        )
        .await;

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("defaults-{index}");
        let mut request = case.create.clone();
        request["Name"] = json!(&name);
        request["RuleSetName"] = json!("defaults-rules");
        request["RequestTimeoutSeconds"] = json!(60);
        request["AcceptanceRequired"] = json!(false);
        let created = gamelift
            .ok("CreateMatchmakingConfiguration", request)
            .await;
        for (member, expected) in [
            ("AdditionalPlayerCount", &case.additional_player_count),
            ("BackfillMode", &case.backfill_mode),
        ] {
            assert_eq!(
                &created["Configuration"][member],
                expected,
                "case {label:?}: the created configuration must report {member} as {expected}, got {}",
                created["Configuration"]
            );
        }
        let described = described_configuration(&gamelift, &name).await;
        assert_eq!(
            described, created["Configuration"],
            "case {label:?}: the describe must report the created configuration, got {described}"
        );

        // An update that names neither setting keeps both: they are part of the stored
        // configuration, not of the request that happened to create it.
        let updated = gamelift
            .ok(
                "UpdateMatchmakingConfiguration",
                json!({"Name": &name, "Description": "updated without naming the defaults"}),
            )
            .await;
        let after = described_configuration(&gamelift, &name).await;
        assert_eq!(
            updated["Configuration"], after,
            "case {label:?}: the update must report the stored configuration, got {}",
            updated["Configuration"]
        );
        assert_eq!(
            after,
            only_changed(
                &described,
                &[(
                    "Description",
                    json!("updated without naming the defaults")
                )],
            ),
            "case {label:?}: the update must keep every setting it did not name, got {after}"
        );
    }
}

/// An update names ONE resource, so an identifier that names no stored resource is the
/// modelled `NotFoundException` rather than a success that writes nothing.
#[tokio::test]
async fn updating_a_resource_that_is_not_stored_is_refused() {
    struct Case {
        operation: &'static str,
        /// The identifying member, and the identifiers no stored resource carries.
        member: &'static str,
        selectors: Vec<String>,
    }

    let cases = vec![
        Case {
            operation: "UpdateAlias",
            member: "AliasId",
            selectors: vec![
                "alias-does-not-exist".to_string(),
                gamelift_arn("alias/alias-does-not-exist"),
            ],
        },
        Case {
            operation: "UpdateBuild",
            member: "BuildId",
            selectors: vec![
                "build-does-not-exist".to_string(),
                gamelift_arn("build/build-does-not-exist"),
            ],
        },
        Case {
            operation: "UpdateScript",
            member: "ScriptId",
            selectors: vec![
                "script-does-not-exist".to_string(),
                gamelift_arn("script/script-does-not-exist"),
            ],
        },
        Case {
            operation: "UpdateMatchmakingConfiguration",
            member: "Name",
            selectors: vec![
                "no-such-configuration".to_string(),
                gamelift_arn("matchmakingconfiguration/no-such-configuration"),
            ],
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let operation = case.operation;
        for selector in &case.selectors {
            let mut request = json!({});
            request[case.member] = json!(selector);
            let (status, code) = gamelift.err(operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NotFoundException"),
                "case {operation} {selector:?}: an identifier naming no stored resource must be refused, got {status} {code}"
            );
        }
    }
}
