// SPDX-License-Identifier: CC0-1.0

//! The FlexMatch control plane, over the wire: a rule set is parsed before it is stored,
//! a matchmaker can only be created on a rule set that exists, a matchmaking request can
//! only be started on a matchmaker that exists, and each stored resource reads back
//! through its own describe — where an identifier that names nothing is refused rather
//! than reported as an empty page.
//!
//! The harness (`GameLift`, `gamelift_arn`, `wait_for_due_transitions`) is the one
//! `hosting_loop` already drives the production dispatch path with.

use std::time::Duration;

use serde_json::{json, Value};

use crate::hosting_loop::{gamelift_arn, wait_for_due_transitions, GameLift};

/// The ARN a rule set named `name` is assigned: `MatchmakingRuleSet$RuleSetArn`'s doc —
/// "Format is `arn:aws:gamelift:<region>::matchmakingruleset/<ruleset name>`".
pub(crate) fn rule_set_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingruleset/{name}"))
}

/// The ARN a matchmaking configuration named `name` is assigned:
/// `MatchmakingConfiguration$ConfigurationArn`'s doc — "Format is
/// `arn:aws:gamelift:<region>::matchmakingconfiguration/<matchmaking configuration
/// name>`".
pub(crate) fn configuration_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingconfiguration/{name}"))
}

/// A rule set body that conforms to the FlexMatch rule language: the version it is
/// written in, and the teams a match is built from.
pub(crate) fn conformant_body() -> String {
    json!({
        "name": "solo-duel",
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// Create a rule set and a matchmaking configuration on it, and return the
/// configuration's name.
pub(crate) async fn matchmaker(
    gamelift: &GameLift,
    name: &str,
    request_timeout_seconds: i64,
) -> String {
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": format!("{name}-rules"), "RuleSetBody": conformant_body()}),
        )
        .await;
    gamelift
        .ok(
            "CreateMatchmakingConfiguration",
            json!({
                "Name": name,
                "RuleSetName": format!("{name}-rules"),
                "RequestTimeoutSeconds": request_timeout_seconds,
                "AcceptanceRequired": false,
                "FlexMatchMode": "STANDALONE",
            }),
        )
        .await;
    name.to_string()
}

/// A body that IS a FlexMatch rule set is accepted by `ValidateMatchmakingRuleSet` with
/// `Valid` = true and stored verbatim by `CreateMatchmakingRuleSet`; a body that is not
/// is refused by BOTH with the modelled `InvalidRequestException`, and nothing is
/// stored.
///
/// The rows differ along what the rule generalises over: the JSON type of the body
/// (object, array, string, not JSON at all), which required element is missing or
/// mistyped, the shape of a team definition, and the player counts a team declares.
#[tokio::test]
async fn a_rule_set_body_is_validated_the_same_way_by_validate_and_by_create() {
    struct Case {
        label: &'static str,
        body: String,
        valid: bool,
    }

    let cases = vec![
        Case {
            label: "the smallest conforming body: a version and one team",
            body: json!({"ruleLanguageVersion": "1.0", "teams": [{"name": "solo", "maxPlayers": 1}]})
                .to_string(),
            valid: true,
        },
        Case {
            label: "a named rule set with minPlayers and maxPlayers",
            body: conformant_body(),
            valid: true,
        },
        Case {
            label: "three teams, each with its own player counts",
            body: json!({
                "name": "three-teams",
                "ruleLanguageVersion": "1.0",
                "teams": [
                    {"name": "red", "minPlayers": 2, "maxPlayers": 5},
                    {"name": "blue", "minPlayers": 2, "maxPlayers": 5},
                    {"name": "spectators", "minPlayers": 0, "maxPlayers": 10},
                ],
            })
            .to_string(),
            valid: true,
        },
        Case {
            label: "the optional player attributes and rules the language allows",
            body: json!({
                "name": "with-rules",
                "ruleLanguageVersion": "1.0",
                "playerAttributes": [{"name": "skill", "type": "number", "default": 10}],
                "teams": [{"name": "solo", "maxPlayers": 4}],
                "rules": [{
                    "name": "FairSkill",
                    "description": "players of a similar skill",
                    "type": "distance",
                    "measurements": ["avg(teams[*].players.attributes[skill])"],
                    "referenceValue": "avg(flatten(teams[*].players.attributes[skill]))",
                    "maxDistance": 10,
                }],
                "expansions": [],
            })
            .to_string(),
            valid: true,
        },
        Case {
            label: "minPlayers equal to maxPlayers, the boundary the counts allow",
            body: json!({
                "ruleLanguageVersion": "1.0",
                "teams": [{"name": "duo", "minPlayers": 2, "maxPlayers": 2}],
            })
            .to_string(),
            valid: true,
        },
        Case {
            label: "not JSON at all",
            body: "{not valid json at all".to_string(),
            valid: false,
        },
        Case {
            label: "an empty body of whitespace",
            body: " ".to_string(),
            valid: false,
        },
        Case {
            label: "JSON that is a list rather than a rule set object",
            body: json!([{"ruleLanguageVersion": "1.0"}]).to_string(),
            valid: false,
        },
        Case {
            label: "JSON that is a bare string",
            body: json!("ruleLanguageVersion").to_string(),
            valid: false,
        },
        Case {
            label: "no teams element",
            body: json!({"name": "missing_teams", "ruleLanguageVersion": "1.0"}).to_string(),
            valid: false,
        },
        Case {
            label: "a teams element that is not a list",
            body: json!({"ruleLanguageVersion": "1.0", "teams": {"name": "solo"}}).to_string(),
            valid: false,
        },
        Case {
            label: "an empty teams list",
            body: json!({"ruleLanguageVersion": "1.0", "teams": []}).to_string(),
            valid: false,
        },
        Case {
            label: "no ruleLanguageVersion",
            body: json!({"teams": [{"name": "solo", "maxPlayers": 2}]}).to_string(),
            valid: false,
        },
        Case {
            label: "a ruleLanguageVersion that is not a string",
            body: json!({"ruleLanguageVersion": 1.0, "teams": [{"name": "s", "maxPlayers": 2}]})
                .to_string(),
            valid: false,
        },
        Case {
            label: "a team without a name",
            body: json!({"ruleLanguageVersion": "1.0", "teams": [{"maxPlayers": 2}]}).to_string(),
            valid: false,
        },
        Case {
            label: "a team without maxPlayers",
            body: json!({"ruleLanguageVersion": "1.0", "teams": [{"name": "solo"}]}).to_string(),
            valid: false,
        },
        Case {
            label: "a team that holds no player at all",
            body: json!({"ruleLanguageVersion": "1.0", "teams": [{"name": "s", "maxPlayers": 0}]})
                .to_string(),
            valid: false,
        },
        Case {
            label: "a team wanting more players than it holds",
            body: json!({
                "ruleLanguageVersion": "1.0",
                "teams": [{"name": "s", "minPlayers": 5, "maxPlayers": 2}],
            })
            .to_string(),
            valid: false,
        },
        Case {
            label: "one team name used twice",
            body: json!({
                "ruleLanguageVersion": "1.0",
                "teams": [
                    {"name": "red", "maxPlayers": 2},
                    {"name": "red", "maxPlayers": 2},
                ],
            })
            .to_string(),
            valid: false,
        },
        Case {
            label: "a rules element that is not a list",
            body: json!({
                "ruleLanguageVersion": "1.0",
                "teams": [{"name": "solo", "maxPlayers": 2}],
                "rules": {"name": "FairSkill"},
            })
            .to_string(),
            valid: false,
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("ruleset-row-{index}");

        // `ValidateMatchmakingRuleSet` reports on the body alone.
        if case.valid {
            let validated = gamelift
                .ok(
                    "ValidateMatchmakingRuleSet",
                    json!({"RuleSetBody": &case.body}),
                )
                .await;
            assert_eq!(
                validated,
                json!({"Valid": true}),
                "case {label:?}: a conforming body must validate, got {validated}"
            );
        } else {
            let (status, code) = gamelift
                .err(
                    "ValidateMatchmakingRuleSet",
                    json!({"RuleSetBody": &case.body}),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidRequestException"),
                "case {label:?}: a body that is not a rule set must be refused with a 400, not \
                 reported as Valid=false, got {status} {code}"
            );
        }

        // `CreateMatchmakingRuleSet` admits exactly the bodies that validate.
        let request = json!({"Name": &name, "RuleSetBody": &case.body});
        if case.valid {
            let created = gamelift.ok("CreateMatchmakingRuleSet", request).await;
            assert_eq!(
                created["RuleSet"]["RuleSetName"],
                json!(&name),
                "case {label:?}: the created rule set must carry the requested name, got {created}"
            );
            assert_eq!(
                created["RuleSet"]["RuleSetArn"],
                json!(rule_set_arn(&name)),
                "case {label:?}: the created rule set must carry its ARN, got {created}"
            );
            assert_eq!(
                created["RuleSet"]["RuleSetBody"],
                json!(&case.body),
                "case {label:?}: the body must be stored verbatim, got {created}"
            );
            assert!(
                created["RuleSet"]["CreationTime"].is_number(),
                "case {label:?}: the created rule set must carry a CreationTime, got {created}"
            );

            // The describe path serves the SAME stored record — selected by name and by
            // the ARN the create minted, which name the same rule set.
            for selector in [name.clone(), rule_set_arn(&name)] {
                let described = gamelift
                    .ok("DescribeMatchmakingRuleSets", json!({"Names": [&selector]}))
                    .await;
                assert_eq!(
                    described["RuleSets"],
                    json!([created["RuleSet"].clone()]),
                    "case {label:?}: a describe naming {selector:?} must report the created \
                     record, got {described}"
                );
            }
        } else {
            let (status, code) = gamelift.err("CreateMatchmakingRuleSet", request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidRequestException"),
                "case {label:?}: a create with a body that is not a rule set must be refused, \
                 got {status} {code}"
            );
            // Nothing was stored, so the describe refuses the name.
            let (status, code) = gamelift
                .err("DescribeMatchmakingRuleSets", json!({"Names": [&name]}))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidRequestException"),
                "case {label:?}: a refused create must store nothing, got {status} {code}"
            );
        }
    }

    // A name a rule set already holds cannot be taken a second time.
    let taken = json!({"Name": "ruleset-row-0", "RuleSetBody": conformant_body()});
    let (status, code) = gamelift.err("CreateMatchmakingRuleSet", taken).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a repeated rule set name must be refused, got {status} {code}"
    );
}

/// A matchmaking configuration is created only on a rule set that exists, and only when
/// its cross-member rules hold; a created one reads back through
/// `DescribeMatchmakingConfigurations`, member for member.
///
/// The refusal rows differ along which rule the request breaks and the form of the
/// identifier it names (name or ARN); the acceptance rows along which optional members
/// the request sets.
#[tokio::test]
async fn a_configuration_is_created_only_on_a_rule_set_that_exists() {
    let gamelift = GameLift::new();
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "live-rules", "RuleSetBody": conformant_body()}),
        )
        .await;
    gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({"Name": "matchmaker-queue", "TimeoutInSeconds": 60}),
        )
        .await;
    let queue_arn = gamelift_arn("gamesessionqueue/matchmaker-queue");

    // Rows that must be refused: each breaks one documented rule.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a rule set name that names nothing",
            json!({
                "Name": "no-such-ruleset",
                "RuleSetName": "absent-rules",
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
            }),
        ),
        (
            "a rule set ARN that names nothing",
            json!({
                "Name": "no-such-ruleset-arn",
                "RuleSetName": rule_set_arn("absent-rules"),
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
            }),
        ),
        (
            "two queues, where a configuration supports only one",
            json!({
                "Name": "two-queues",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
                "GameSessionQueueArns": [&queue_arn, gamelift_arn("gamesessionqueue/other")],
            }),
        ),
        (
            "a queue set together with the STANDALONE mode",
            json!({
                "Name": "standalone-with-queue",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
                "FlexMatchMode": "STANDALONE",
                "GameSessionQueueArns": [&queue_arn],
            }),
        ),
        (
            "automatic backfill together with the STANDALONE mode",
            json!({
                "Name": "standalone-with-backfill",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
                "FlexMatchMode": "STANDALONE",
                "BackfillMode": "AUTOMATIC",
            }),
        ),
    ];
    for (label, request) in &refused {
        let (status, code) = gamelift
            .err("CreateMatchmakingConfiguration", request.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the request must be refused, got {status} {code}"
        );
        // Nothing was stored under that name.
        let name = request["Name"].as_str().unwrap_or_default();
        let (status, code) = gamelift
            .err("DescribeMatchmakingConfigurations", json!({"Names": [name]}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a refused create must store nothing, got {status} {code}"
        );
    }

    // Rows that must be accepted, with the members each one must report back.
    let accepted: Vec<(&str, Value, Value)> = vec![
        (
            "the required members only, naming the rule set by name",
            json!({
                "Name": "minimal",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 1,
                "AcceptanceRequired": false,
            }),
            json!({
                "RequestTimeoutSeconds": 1,
                "AcceptanceRequired": false,
                "GameSessionQueueArns": [],
                "GameProperties": [],
            }),
        ),
        (
            "the rule set named by its ARN",
            json!({
                "Name": "by-arn",
                "RuleSetName": rule_set_arn("live-rules"),
                "RequestTimeoutSeconds": 43200,
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": 600,
            }),
            json!({
                "RequestTimeoutSeconds": 43200,
                "AcceptanceRequired": true,
                "AcceptanceTimeoutSeconds": 600,
                "GameSessionQueueArns": [],
                "GameProperties": [],
            }),
        ),
        (
            "a matchmaker that starts sessions through a queue",
            json!({
                "Name": "with-queue",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 120,
                "AcceptanceRequired": false,
                "FlexMatchMode": "WITH_QUEUE",
                "BackfillMode": "AUTOMATIC",
                "GameSessionQueueArns": [&queue_arn],
                "Description": "the live matchmaker",
                "AdditionalPlayerCount": 2,
                "CustomEventData": "matchmaker-event-data",
                "GameProperties": [{"Key": "map", "Value": "arena"}],
                "GameSessionData": "session-data",
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:matchmaking",
                "Tags": [{"Key": "owner", "Value": "liveops"}],
            }),
            json!({
                "RequestTimeoutSeconds": 120,
                "AcceptanceRequired": false,
                "FlexMatchMode": "WITH_QUEUE",
                "BackfillMode": "AUTOMATIC",
                "GameSessionQueueArns": [&queue_arn],
                "Description": "the live matchmaker",
                "AdditionalPlayerCount": 2,
                "CustomEventData": "matchmaker-event-data",
                "GameProperties": [{"Key": "map", "Value": "arena"}],
                "GameSessionData": "session-data",
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:matchmaking",
            }),
        ),
    ];
    for (label, request, expected) in &accepted {
        let name = request["Name"].as_str().unwrap_or_default().to_string();
        let created = gamelift
            .ok("CreateMatchmakingConfiguration", request.clone())
            .await;
        let configuration = created["Configuration"].clone();
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &configuration[member], value,
                "case {label:?}: {member} must be reported as {value}, got {configuration}"
            );
        }
        assert_eq!(
            configuration["Name"],
            json!(&name),
            "case {label:?}: the configuration must carry its name, got {configuration}"
        );
        assert_eq!(
            configuration["ConfigurationArn"],
            json!(configuration_arn(&name)),
            "case {label:?}: the configuration must carry its ARN, got {configuration}"
        );
        // The rule set is reported by NAME and by ARN, whichever form the request used.
        assert_eq!(
            (
                configuration["RuleSetName"].clone(),
                configuration["RuleSetArn"].clone()
            ),
            (json!("live-rules"), json!(rule_set_arn("live-rules"))),
            "case {label:?}: the configuration must report the resolved rule set, got \
             {configuration}"
        );

        // The describe path serves the SAME record, selected by name and by ARN.
        for selector in [name.clone(), configuration_arn(&name)] {
            let described = gamelift
                .ok(
                    "DescribeMatchmakingConfigurations",
                    json!({"Names": [&selector]}),
                )
                .await;
            assert_eq!(
                described["Configurations"],
                json!([configuration.clone()]),
                "case {label:?}: a describe naming {selector:?} must report the created record, \
                 got {described}"
            );
        }
    }

    // And the rule-set filter selects every configuration that uses it, while a rule set
    // nothing uses selects none.
    let by_rule_set = gamelift
        .ok(
            "DescribeMatchmakingConfigurations",
            json!({"RuleSetName": "live-rules"}),
        )
        .await;
    assert_eq!(
        by_rule_set["Configurations"].as_array().map(Vec::len),
        Some(accepted.len()),
        "the rule-set filter must select every configuration that uses it, got {by_rule_set}"
    );
    let by_other = gamelift
        .ok(
            "DescribeMatchmakingConfigurations",
            json!({"RuleSetName": "absent-rules"}),
        )
        .await;
    assert_eq!(
        by_other["Configurations"],
        json!([]),
        "a rule set no configuration uses must select none, got {by_other}"
    );

    // A name a configuration already holds cannot be taken a second time.
    let (status, code) = gamelift
        .err(
            "CreateMatchmakingConfiguration",
            json!({
                "Name": "minimal",
                "RuleSetName": "live-rules",
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a repeated configuration name must be refused, got {status} {code}"
    );
}

/// `StartMatchmaking` persists the ticket it accepts and refuses a configuration that
/// names nothing; `DescribeMatchmaking` reports the stored tickets and refuses a ticket
/// id that names nothing.
#[tokio::test]
async fn a_matchmaking_ticket_is_started_only_on_a_configuration_that_exists() {
    let gamelift = GameLift::new();
    let configuration = matchmaker(&gamelift, "ticket-matchmaker", 43200).await;

    // A configuration that names nothing is refused with NotFoundException, in either
    // identifier form, and nothing is persisted for the ticket id the request carried.
    for (label, selector) in [
        ("a configuration name that names nothing", "absent-config".to_string()),
        (
            "a configuration ARN that names nothing",
            configuration_arn("absent-config"),
        ),
    ] {
        let ticket_id = format!("ticket-for-{}", selector.len());
        let (status, code) = gamelift
            .err(
                "StartMatchmaking",
                json!({
                    "TicketId": &ticket_id,
                    "ConfigurationName": &selector,
                    "Players": [{"PlayerId": "player-1"}],
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an unknown matchmaker must be refused with NotFoundException, got \
             {status} {code}"
        );
        let (status, code) = gamelift
            .err("DescribeMatchmaking", json!({"TicketIds": [&ticket_id]}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a ticket the refused start did not create must be refused with \
             InvalidRequestException, got {status} {code}"
        );
    }

    // A request that breaks a documented rule about its players is refused too.
    let refused: Vec<(&str, Value)> = vec![
        (
            "no players to match",
            json!({"ConfigurationName": &configuration, "Players": []}),
        ),
        (
            "a player without an id",
            json!({"ConfigurationName": &configuration, "Players": [{"Team": "red"}]}),
        ),
        (
            "one player named twice",
            json!({
                "ConfigurationName": &configuration,
                "Players": [{"PlayerId": "p1"}, {"PlayerId": "p1"}],
            }),
        ),
    ];
    for (label, request) in &refused {
        let (status, code) = gamelift.err("StartMatchmaking", request.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the request must be refused, got {status} {code}"
        );
    }

    // The rows differ along how many players the request carries, what each one brings
    // (attributes, latencies, a team), and whether the caller chose the ticket id.
    let accepted: Vec<(&str, Option<&str>, Value)> = vec![
        (
            "one player, a caller-chosen ticket id",
            Some("ticket-one-player"),
            json!([{"PlayerId": "player-1"}]),
        ),
        (
            "one player with attributes and latencies",
            Some("ticket-attributes"),
            json!([{
                "PlayerId": "player-2",
                "PlayerAttributes": {"skill": {"N": 42.5}, "role": {"S": "medic"}},
                "LatencyInMs": {"us-west-2": 30},
            }]),
        ),
        (
            "three players matched together",
            Some("ticket-party"),
            json!([
                {"PlayerId": "player-3"},
                {"PlayerId": "player-4"},
                {"PlayerId": "player-5"},
            ]),
        ),
        (
            "no ticket id at all, which the service mints",
            None,
            json!([{"PlayerId": "player-6"}]),
        ),
    ];

    let mut started: Vec<(String, Value)> = Vec::new();
    for (label, ticket_id, players) in &accepted {
        let mut request = json!({"ConfigurationName": &configuration, "Players": players});
        if let Some(ticket_id) = ticket_id {
            request["TicketId"] = json!(ticket_id);
        }
        let response = gamelift.ok("StartMatchmaking", request).await;
        let ticket = response["MatchmakingTicket"].clone();
        let reported_id = ticket["TicketId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the ticket must carry an id, got {ticket}"))
            .to_string();
        match ticket_id {
            Some(ticket_id) => assert_eq!(
                reported_id, *ticket_id,
                "case {label:?}: the ticket must carry the id the request chose, got {ticket}"
            ),
            None => assert!(
                !reported_id.is_empty(),
                "case {label:?}: a ticket with no requested id must be given one, got {ticket}"
            ),
        }
        // "If successful, a matchmaking ticket is returned with status set to QUEUED."
        assert_eq!(
            ticket["Status"],
            json!("QUEUED"),
            "case {label:?}: a new ticket must be QUEUED, got {ticket}"
        );
        assert_eq!(
            (
                ticket["ConfigurationName"].clone(),
                ticket["ConfigurationArn"].clone()
            ),
            (
                json!(&configuration),
                json!(configuration_arn(&configuration))
            ),
            "case {label:?}: the ticket must name the matchmaker it was started on, got {ticket}"
        );
        assert_eq!(
            ticket["Players"], *players,
            "case {label:?}: the ticket must report the players the request supplied, got {ticket}"
        );
        assert!(
            ticket["EndTime"].is_null(),
            "case {label:?}: a ticket still in process carries no EndTime, got {ticket}"
        );

        // The describe path serves the SAME stored record.
        let described = gamelift
            .ok("DescribeMatchmaking", json!({"TicketIds": [&reported_id]}))
            .await;
        assert_eq!(
            described["TicketList"],
            json!([ticket.clone()]),
            "case {label:?}: the describe must report the stored ticket, got {described}"
        );
        started.push((reported_id, ticket));
    }

    // A ticket id a ticket already holds cannot be taken a second time.
    let (status, code) = gamelift
        .err(
            "StartMatchmaking",
            json!({
                "TicketId": "ticket-one-player",
                "ConfigurationName": &configuration,
                "Players": [{"PlayerId": "player-7"}],
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a repeated ticket id must be refused, got {status} {code}"
    );

    // One describe reports every ticket the request names, in that order.
    let ids: Vec<String> = started.iter().map(|(id, _)| id.clone()).collect();
    let described = gamelift
        .ok("DescribeMatchmaking", json!({"TicketIds": &ids}))
        .await;
    let reported: Vec<String> = described["TicketList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|ticket| ticket["TicketId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, ids,
        "a describe must report the tickets it names, in order, got {described}"
    );

    // A list with one id that names nothing is refused whole: an absent ticket is never
    // reported as an empty list.
    let mut with_absent = ids.clone();
    with_absent.push("ticket-absent".to_string());
    let (status, code) = gamelift
        .err("DescribeMatchmaking", json!({"TicketIds": with_absent}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a describe naming an absent ticket must be refused, got {status} {code}"
    );

    // "Provide a list of up to 10 ticket IDs": one past the documented maximum is
    // refused, and exactly ten is served.
    let ten: Vec<String> = (0..10).map(|_| ids[0].clone()).collect();
    let served = gamelift
        .ok("DescribeMatchmaking", json!({"TicketIds": ten.clone()}))
        .await;
    assert_eq!(
        served["TicketList"].as_array().map(Vec::len),
        Some(10),
        "ten ticket ids must be served, got {served}"
    );
    let mut eleven = ten;
    eleven.push(ids[0].clone());
    let (status, code) = gamelift
        .err("DescribeMatchmaking", json!({"TicketIds": eleven}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "more ticket ids than the documented maximum must be refused, got {status} {code}"
    );
}

/// The clock carries a ticket forward: an accepted request is picked up and SEARCHING,
/// and one for which no match is formed within the matchmaker's `RequestTimeoutSeconds`
/// ends TIMED_OUT with the reason it did — never COMPLETED for a match nothing formed.
#[tokio::test]
async fn a_ticket_is_searched_for_and_then_times_out() {
    let gamelift = GameLift::new();
    // The rows differ along the matchmaker's request timeout: one that cannot elapse
    // during the test, and the shortest the model allows (`MatchmakingRequestTimeoutInteger`,
    // range 1..43200).
    let patient = matchmaker(&gamelift, "patient-matchmaker", 43200).await;
    let impatient = matchmaker(&gamelift, "impatient-matchmaker", 1).await;

    for (label, configuration, ticket_id) in [
        ("a matchmaker with a long request timeout", &patient, "ticket-patient"),
        ("a matchmaker with a one-second timeout", &impatient, "ticket-impatient"),
    ] {
        let started = gamelift
            .ok(
                "StartMatchmaking",
                json!({
                    "TicketId": ticket_id,
                    "ConfigurationName": configuration,
                    "Players": [{"PlayerId": "player-1"}],
                }),
            )
            .await;
        assert_eq!(
            started["MatchmakingTicket"]["Status"],
            json!("QUEUED"),
            "case {label:?}: the ticket must start QUEUED, got {started}"
        );
    }

    // Past the span the pick-up is due on, the ticket whose request cannot have timed out
    // is SEARCHING.
    wait_for_due_transitions();
    let described = gamelift
        .ok("DescribeMatchmaking", json!({"TicketIds": ["ticket-patient"]}))
        .await;
    let ticket = described["TicketList"][0].clone();
    assert_eq!(
        ticket["Status"],
        json!("SEARCHING"),
        "a picked-up ticket must be SEARCHING, got {ticket}"
    );
    assert!(
        ticket["EndTime"].is_null(),
        "a ticket still in process carries no EndTime, got {ticket}"
    );

    // Past its matchmaker's request timeout, the other ticket has timed out: this
    // emulator forms no matches, so a ticket ends in the modelled failure status with a
    // status reason that says so.
    std::thread::sleep(Duration::from_millis(1_100));
    let described = gamelift
        .ok(
            "DescribeMatchmaking",
            json!({"TicketIds": ["ticket-impatient"]}),
        )
        .await;
    let ticket = described["TicketList"][0].clone();
    assert_eq!(
        ticket["Status"],
        json!("TIMED_OUT"),
        "a ticket past its request timeout must be TIMED_OUT, got {ticket}"
    );
    assert!(
        ticket["EndTime"].is_number(),
        "a ticket that stopped being processed must carry an EndTime, got {ticket}"
    );
    assert!(
        ticket["StatusMessage"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "a timed-out ticket must say why, got {ticket}"
    );

    // Reading the tickets changed nothing: a second read reports the same records.
    let first = gamelift
        .ok(
            "DescribeMatchmaking",
            json!({"TicketIds": ["ticket-patient", "ticket-impatient"]}),
        )
        .await;
    let second = gamelift
        .ok(
            "DescribeMatchmaking",
            json!({"TicketIds": ["ticket-patient", "ticket-impatient"]}),
        )
        .await;
    assert_eq!(
        first, second,
        "describing the tickets must not change what a later describe reports, got {second}"
    );
}

/// `DescribeMatchmakingRuleSets` pages the collection, and refuses both a name that names
/// nothing and a token it did not issue.
#[tokio::test]
async fn the_rule_set_describe_pages_and_refuses_what_it_did_not_issue() {
    let gamelift = GameLift::new();
    let names: Vec<String> = (0..5).map(|index| format!("paged-rules-{index}")).collect();
    for name in &names {
        gamelift
            .ok(
                "CreateMatchmakingRuleSet",
                json!({"Name": name, "RuleSetBody": conformant_body()}),
            )
            .await;
    }

    // `Limit` is modelled `RuleSetLimit` with range 1..10, so every page size here is
    // one the model allows.
    for page_size in [1_usize, 2, 5, 10] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"Limit": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = gamelift.ok("DescribeMatchmakingRuleSets", request).await;
            let page = response["RuleSets"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most Limit rule sets, got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|rule_set| rule_set["RuleSetName"].as_str().unwrap_or_default().to_string()),
            );
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, names,
            "case page size {page_size}: the paged walk must report every rule set once, in \
             order, got {walked:?}"
        );
    }

    for (label, request) in [
        (
            "a name no rule set carries",
            json!({"Names": ["absent-rules"]}),
        ),
        (
            "an ARN no rule set carries",
            json!({"Names": [rule_set_arn("absent-rules")]}),
        ),
        (
            "a stored name beside an absent one",
            json!({"Names": [&names[0], "absent-rules"]}),
        ),
        (
            "a token this collection did not issue",
            json!({"NextToken": "not-a-token-this-service-issued"}),
        ),
    ] {
        let (status, code) = gamelift.err("DescribeMatchmakingRuleSets", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the request must be refused, got {status} {code}"
        );
    }
}
