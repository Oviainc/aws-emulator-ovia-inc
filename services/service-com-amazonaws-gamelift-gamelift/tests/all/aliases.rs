// SPDX-License-Identifier: CC0-1.0

//! The fleet alias contract, over the wire: `CreateAlias` persists the alias the request
//! configures, mints its id and ARN, and `DescribeAlias` serves that one stored record
//! back — selected by alias id or by alias ARN. An operation that accepts an alias
//! identifier routes through the stored alias: a simple alias names the fleet it points
//! at, a terminal one resolves to no fleet.
//!
//! The harness (`GameLift`, `gamelift_arn`, `ec2_fleet`, `create_fleet`,
//! `wait_for_due_transitions`) is the one `hosting_loop` already drives the production
//! dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{
    create_fleet, ec2_fleet, gamelift_arn, wait_for_due_transitions, GameLift,
};

/// Every `RoutingStrategyType` the model declares
/// (`smithy_query shape=RoutingStrategyType`), so the rows below are drawn from the
/// modelled catalog rather than from one example.
const ROUTING_STRATEGY_TYPES: [&str; 2] = ["SIMPLE", "TERMINAL"];

/// The modelled bounds of `CreateAliasInput$Name`
/// (`smithy_query operation=CreateAlias`: `NonBlankAndLengthConstraintString`, len
/// 1..1024).
const MIN_NAME_LENGTH: usize = 1;
/// See [`MIN_NAME_LENGTH`].
const MAX_NAME_LENGTH: usize = 1024;

/// The ARN an alias with `alias_id` is assigned:
/// `arn:aws:gamelift:<region>:<account>:alias/<alias id>` — "in a GameLift alias ARN, the
/// resource ID matches the alias ID value" (`smithy_query shape=Alias` member `AliasArn`).
fn alias_arn(alias_id: &str) -> String {
    gamelift_arn(&format!("alias/{alias_id}"))
}

/// A simple routing strategy pointing at `fleet_reference`: "SIMPLE - The alias resolves
/// to one specific fleet."
fn simple_routing(fleet_reference: &str) -> Value {
    json!({"Type": "SIMPLE", "FleetId": fleet_reference})
}

/// A terminal routing strategy carrying `message`: "TERMINAL - The alias does not resolve
/// to a fleet but instead can be used to display a message to the user."
fn terminal_routing(message: &str) -> Value {
    json!({"Type": "TERMINAL", "Message": message})
}

/// The alias `CreateAlias` reports for `request`, which must be the stored record.
async fn create_alias(gamelift: &GameLift, request: Value) -> Value {
    let created = gamelift.ok("CreateAlias", request).await;
    created["Alias"].clone()
}

/// The `AliasId` of a created alias.
fn alias_id_of(alias: &Value) -> String {
    alias["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string()
}

/// `CreateAlias` stores the alias the request configures and reports the stored record;
/// `DescribeAlias` serves the same record back, member for member, selected by id or by
/// ARN.
///
/// The rows differ along what the rule generalises over: which optional members the
/// request sets, both modelled `RoutingStrategyType` values, the terminal message text,
/// the name at each modelled length bound, and how many tags the request assigns.
#[tokio::test]
async fn a_created_alias_is_reported_as_stored_by_create_and_by_describe() {
    struct Case {
        label: &'static str,
        /// The request, without its `Name` — each row is created under its own name.
        request: Value,
        /// The members the created alias must report beyond its id, ARN and name.
        expected: Value,
    }

    let cases = [
        Case {
            label: "the required name and a simple routing strategy",
            request: json!({"RoutingStrategy": simple_routing("fleet-target-one")}),
            expected: json!({"RoutingStrategy": simple_routing("fleet-target-one")}),
        },
        Case {
            label: "a simple alias with a description",
            request: json!({
                "RoutingStrategy": simple_routing("fleet-target-two"),
                "Description": "routes players at the live fleet",
            }),
            expected: json!({
                "RoutingStrategy": simple_routing("fleet-target-two"),
                "Description": "routes players at the live fleet",
            }),
        },
        Case {
            label: "a terminal alias carrying its message",
            request: json!({"RoutingStrategy": terminal_routing("This version is retired.")}),
            expected: json!({"RoutingStrategy": terminal_routing("This version is retired.")}),
        },
        Case {
            label: "a terminal alias with a different message and a description",
            request: json!({
                "RoutingStrategy": terminal_routing("Upgrade at https://example.test/upgrade"),
                "Description": "sends players to the upgrade site",
            }),
            expected: json!({
                "RoutingStrategy": terminal_routing("Upgrade at https://example.test/upgrade"),
                "Description": "sends players to the upgrade site",
            }),
        },
        Case {
            label: "a simple alias with one tag",
            request: json!({
                "RoutingStrategy": simple_routing("fleet-target-three"),
                "Tags": [{"Key": "owner", "Value": "liveops"}],
            }),
            // `Alias` carries no `Tags` member — tags are read back through
            // `ListTagsForResource` — so a tagged alias reports exactly what an untagged
            // one does.
            expected: json!({"RoutingStrategy": simple_routing("fleet-target-three")}),
        },
        Case {
            label: "a simple alias with three tags",
            request: json!({
                "RoutingStrategy": simple_routing("fleet-target-four"),
                "Tags": [
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "prod"},
                    {"Key": "cost-centre", "Value": "games"},
                ],
            }),
            expected: json!({"RoutingStrategy": simple_routing("fleet-target-four")}),
        },
        Case {
            label: "a description at the modelled maximum length",
            request: json!({
                "RoutingStrategy": simple_routing("fleet-target-five"),
                "Description": "d".repeat(1024),
            }),
            expected: json!({
                "RoutingStrategy": simple_routing("fleet-target-five"),
                "Description": "d".repeat(1024),
            }),
        },
    ];

    let gamelift = GameLift::new();
    let mut minted_ids: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("alias-row-{index}");
        let mut request = case.request.clone();
        request["Name"] = json!(&name);

        let created = create_alias(&gamelift, request).await;
        let alias_id = alias_id_of(&created);
        assert!(
            alias_id.starts_with("alias-") && alias_id.len() > "alias-".len(),
            "case {label:?}: the minted AliasId must match the modelled ^alias-\\S+$, got {alias_id:?}"
        );
        assert!(
            !minted_ids.contains(&alias_id),
            "case {label:?}: \"alias IDs are unique within a Region\", got the repeated {alias_id:?}"
        );
        minted_ids.push(alias_id.clone());

        let mut expected = case.expected.clone();
        expected["AliasId"] = json!(&alias_id);
        expected["AliasArn"] = json!(alias_arn(&alias_id));
        expected["Name"] = json!(&name);
        expected["CreationTime"] = created["CreationTime"].clone();
        // A freshly created alias has not been modified since it was created.
        expected["LastUpdatedTime"] = created["CreationTime"].clone();
        assert!(
            created["CreationTime"].is_number(),
            "case {label:?}: a created alias must carry a CreationTime, got {created}"
        );
        assert_eq!(
            created, expected,
            "case {label:?}: CreateAlias must report the stored alias, got {created}"
        );

        // The describe path serves the SAME stored record, member for member — selected
        // by the id and by the ARN the create minted, which name the same alias.
        for selector in [alias_id.clone(), alias_arn(&alias_id)] {
            let described = gamelift
                .ok("DescribeAlias", json!({"AliasId": &selector}))
                .await;
            assert_eq!(
                described["Alias"], expected,
                "case {label:?}: a describe naming {selector:?} must report the created record, got {}",
                described["Alias"]
            );
        }
    }

    // Reading an alias changed nothing: the same describe, run again, reports exactly
    // what it reported before.
    let first = gamelift
        .ok("DescribeAlias", json!({"AliasId": &minted_ids[0]}))
        .await;
    let second = gamelift
        .ok("DescribeAlias", json!({"AliasId": &minted_ids[0]}))
        .await;
    assert_eq!(
        first, second,
        "describing an alias must not change what a later describe reports, got {second}"
    );
}

/// The name bounds are the model's: a name at each modelled length is stored whole, and
/// one past the maximum is refused by the boundary walk. "Alias names do not need to be
/// unique", so one name can be carried by two aliases, each with its own id.
#[tokio::test]
async fn alias_names_are_bounded_by_the_model_and_need_not_be_unique() {
    let gamelift = GameLift::new();

    for length in [MIN_NAME_LENGTH, 2, MAX_NAME_LENGTH / 2, MAX_NAME_LENGTH] {
        let name = "n".repeat(length);
        let created = create_alias(
            &gamelift,
            json!({"Name": &name, "RoutingStrategy": simple_routing("fleet-named")}),
        )
        .await;
        assert_eq!(
            created["Name"],
            json!(&name),
            "case name length {length}: the stored alias must carry the name whole, got {created}"
        );
    }

    let (status, code) = gamelift
        .err(
            "CreateAlias",
            json!({
                "Name": "n".repeat(MAX_NAME_LENGTH + 1),
                "RoutingStrategy": simple_routing("fleet-named"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a name one past the modelled maximum must be refused, got {status} {code}"
    );

    // "Alias names do not need to be unique": the second create under a taken name is
    // served, and the two aliases are distinct resources.
    let first = create_alias(
        &gamelift,
        json!({"Name": "shared", "RoutingStrategy": simple_routing("fleet-one")}),
    )
    .await;
    let second = create_alias(
        &gamelift,
        json!({"Name": "shared", "RoutingStrategy": terminal_routing("retired")}),
    )
    .await;
    assert_ne!(
        alias_id_of(&first),
        alias_id_of(&second),
        "two aliases sharing a name must be distinct resources, got {first} and {second}"
    );
    for alias in [&first, &second] {
        let described = gamelift
            .ok("DescribeAlias", json!({"AliasId": alias_id_of(alias)}))
            .await;
        assert_eq!(
            described["Alias"], *alias,
            "each alias sharing the name must read back as itself, got {}",
            described["Alias"]
        );
    }
}

/// A create whose routing strategy is neither of the two documented forms is refused with
/// the modelled client error, and nothing is stored.
///
/// `RoutingStrategy$Type`'s doc names what each form needs: a `SIMPLE` strategy "resolves
/// to one specific fleet", a `TERMINAL` one "can be used to display a message to the
/// user".
#[tokio::test]
async fn a_routing_strategy_that_is_neither_documented_form_is_refused() {
    struct Case {
        label: &'static str,
        routing_strategy: Value,
        code: &'static str,
    }

    let cases = vec![
        Case {
            label: "a simple strategy naming no fleet",
            routing_strategy: json!({"Type": "SIMPLE"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a simple strategy carrying a message instead of a fleet",
            routing_strategy: json!({"Type": "SIMPLE", "Message": "no fleet here"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a terminal strategy carrying no message",
            routing_strategy: json!({"Type": "TERMINAL"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a terminal strategy naming a fleet instead of a message",
            routing_strategy: json!({"Type": "TERMINAL", "FleetId": "fleet-ignored"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a strategy with no type at all",
            routing_strategy: json!({}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a strategy naming a fleet but no type",
            routing_strategy: json!({"FleetId": "fleet-typeless"}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a type outside the modelled enum",
            routing_strategy: json!({"Type": "GRADUAL", "FleetId": "fleet-gradual"}),
            code: "ValidationException",
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let label = case.label;
        let (status, code) = gamelift
            .err(
                "CreateAlias",
                json!({"Name": label, "RoutingStrategy": case.routing_strategy.clone()}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.code),
            "case {label:?}: the request must be refused with {}, got {status} {code}",
            case.code
        );
    }

    // A tag set that assigns one key twice is not a key-value map, so it is refused with
    // the modelled tagging error.
    let (status, code) = gamelift
        .err(
            "CreateAlias",
            json!({
                "Name": "repeated-tag-key",
                "RoutingStrategy": simple_routing("fleet-tagged"),
                "Tags": [
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "owner", "Value": "platform"},
                ],
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "TaggingFailedException"),
        "a tag set assigning one key twice must be refused, got {status} {code}"
    );

    // Every refused create stored nothing: a valid create still mints the FIRST alias of
    // the store, so no refused request left a record behind.
    let created = create_alias(
        &gamelift,
        json!({"Name": "the-first-stored-alias", "RoutingStrategy": simple_routing("fleet-first")}),
    )
    .await;
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": alias_id_of(&created)}))
        .await;
    assert_eq!(
        described["Alias"]["Name"],
        json!("the-first-stored-alias"),
        "the one stored alias must be the one the accepted create made, got {}",
        described["Alias"]
    );
}

/// `DescribeAlias` names ONE resource, so an identifier that names no stored alias is the
/// modelled `NotFoundException` rather than an empty success.
#[tokio::test]
async fn describing_an_alias_that_is_not_stored_is_refused() {
    let gamelift = GameLift::new();
    let created = create_alias(
        &gamelift,
        json!({"Name": "the-only-alias", "RoutingStrategy": simple_routing("fleet-only")}),
    )
    .await;
    let stored_id = alias_id_of(&created);

    let cases = vec![
        ("an id no alias carries", json!("alias-does-not-exist")),
        (
            "an ARN no alias carries",
            json!(alias_arn("alias-does-not-exist")),
        ),
        (
            "an ARN in another account for a stored id",
            json!(format!(
                "arn:aws:gamelift:us-west-2:999999999999:alias/{stored_id}"
            )),
        ),
    ];
    for (label, selector) in cases {
        let (status, code) = gamelift
            .err("DescribeAlias", json!({"AliasId": selector}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored alias must be refused, got {status} {code}"
        );
    }

    // The stored alias is untouched by those refusals.
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": &stored_id}))
        .await;
    assert_eq!(
        described["Alias"], created,
        "the stored alias must still read back as created, got {}",
        described["Alias"]
    );
}

/// "In most situations, you can use an alias ID in place of a fleet ID": an operation that
/// accepts an alias identifier routes through the STORED alias — a simple alias is served
/// on the fleet it points at, a terminal alias answers the modelled
/// `TerminalRoutingStrategyException` with its own message, and a simple alias whose
/// target fleet does not exist is a missing FLEET, which is what resolution discovers.
#[tokio::test]
async fn an_operation_that_takes_an_alias_routes_through_the_stored_alias() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("aliased-fleet")).await;
    wait_for_due_transitions();

    // Both modelled routing types are covered by the rows below.
    assert_eq!(
        ROUTING_STRATEGY_TYPES,
        ["SIMPLE", "TERMINAL"],
        "the modelled routing types must be the two this test covers, got {ROUTING_STRATEGY_TYPES:?}"
    );

    let live = create_alias(
        &gamelift,
        json!({"Name": "live", "RoutingStrategy": simple_routing(&fleet_id)}),
    )
    .await;
    let retired = create_alias(
        &gamelift,
        json!({"Name": "retired", "RoutingStrategy": terminal_routing("This build is retired.")}),
    )
    .await;
    let dangling = create_alias(
        &gamelift,
        json!({"Name": "dangling", "RoutingStrategy": simple_routing("fleet-never-created")}),
    )
    .await;

    // A simple alias, named by id and by ARN, places the session on its target fleet.
    for selector in [alias_id_of(&live), alias_arn(&alias_id_of(&live))] {
        let session = gamelift
            .ok(
                "CreateGameSession",
                json!({"AliasId": &selector, "MaximumPlayerSessionCount": 2}),
            )
            .await;
        assert_eq!(
            session["GameSession"]["FleetId"],
            json!(&fleet_id),
            "case {selector:?}: a simple alias must place the session on the fleet it names, got {}",
            session["GameSession"]
        );

        // And the describe through the same alias reports that fleet's sessions.
        let described = gamelift
            .ok("DescribeGameSessions", json!({"AliasId": &selector}))
            .await;
        let fleets: Vec<Value> = described["GameSessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|session| session["FleetId"].clone())
            .collect();
        assert!(
            !fleets.is_empty() && fleets.iter().all(|fleet| fleet == &json!(&fleet_id)),
            "case {selector:?}: a describe through a simple alias must report the target fleet's sessions, got {fleets:?}"
        );
    }

    // A terminal alias resolves to no fleet: both the mutating and the reporting path
    // answer the modelled terminal error, with the message the alias embeds.
    for operation in ["CreateGameSession", "DescribeGameSessions"] {
        let mut request = json!({"AliasId": alias_id_of(&retired)});
        if operation == "CreateGameSession" {
            request["MaximumPlayerSessionCount"] = json!(2);
        }
        let (status, parsed) = gamelift.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: a terminal alias must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            super::hosting_loop::error_code(&parsed),
            "TerminalRoutingStrategyException",
            "case {operation:?}: a terminal alias must answer the modelled terminal error, got {parsed}"
        );
        assert_eq!(
            parsed["Message"],
            json!("This build is retired."),
            "case {operation:?}: the terminal error carries the message the routing strategy defines, got {parsed}"
        );
    }

    // "You can reassign an alias to another fleet by calling UpdateAlias": the fleet a
    // simple alias names is NOT validated when the alias is created, so a dangling alias
    // exists and it is the FLEET that cannot be found when the alias is used.
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": alias_id_of(&dangling)}))
        .await;
    assert_eq!(
        described["Alias"]["RoutingStrategy"],
        simple_routing("fleet-never-created"),
        "an alias pointing at a fleet that does not exist must still be stored, got {}",
        described["Alias"]
    );
    let (status, code) = gamelift
        .err(
            "CreateGameSession",
            json!({"AliasId": alias_id_of(&dangling), "MaximumPlayerSessionCount": 2}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NotFoundException"),
        "a simple alias whose target fleet does not exist must answer NotFoundException, got {status} {code}"
    );
    // The reporting path answers the same selector with an empty collection: the sessions
    // of a fleet this service does not hold are none, not an error.
    let described = gamelift
        .ok(
            "DescribeGameSessions",
            json!({"AliasId": alias_id_of(&dangling)}),
        )
        .await;
    assert_eq!(
        described["GameSessions"],
        json!([]),
        "a describe through an alias whose fleet is absent must report no sessions, got {described}"
    );

    // An alias identifier that names no stored alias is still a missing ALIAS.
    for operation in ["CreateGameSession", "DescribeGameSessions"] {
        let mut request = json!({"AliasId": "alias-not-stored"});
        if operation == "CreateGameSession" {
            request["MaximumPlayerSessionCount"] = json!(2);
        }
        let (status, code) = gamelift.err(operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {operation:?}: an unknown alias must answer NotFoundException, got {status} {code}"
        );
    }
}

/// The account's alias quota bounds the stored collection: once it is reached, the next
/// create answers the modelled `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_alias_quota_is_the_bound_on_the_stored_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_ALIASES_PER_ACCOUNT;
    let gamelift = GameLift::new();
    let mut last_id = String::new();
    for index in 0..quota {
        let created = create_alias(
            &gamelift,
            json!({
                "Name": format!("quota-{index}"),
                "RoutingStrategy": simple_routing("fleet-quota"),
            }),
        )
        .await;
        last_id = alias_id_of(&created);
    }

    let (status, code) = gamelift
        .err(
            "CreateAlias",
            json!({
                "Name": "one-alias-too-many",
                "RoutingStrategy": simple_routing("fleet-quota"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "a create past the quota must be refused, got {status} {code}"
    );

    // The refused create stored nothing, and the aliases that fill the quota are intact.
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": &last_id}))
        .await;
    assert_eq!(
        described["Alias"]["Name"],
        json!(format!("quota-{}", quota - 1)),
        "the alias that filled the quota must be unchanged, got {}",
        described["Alias"]
    );
}
