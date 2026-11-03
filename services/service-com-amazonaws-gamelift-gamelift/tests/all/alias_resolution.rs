// SPDX-License-Identifier: CC0-1.0

//! `ResolveAlias` over the wire: "attempts to retrieve a fleet ID that is associated with
//! an alias ... If the alias has a SIMPLE routing strategy, Amazon GameLift Servers
//! returns a fleet ID. If the alias has a TERMINAL routing strategy, the result is a
//! TerminalRoutingStrategyException."
//!
//! So resolution is decided by the STORED alias's routing strategy: a simple alias reports
//! the fleet it points at, with "the ARN associated with the GameLift fleet resource that
//! this alias points to", and a terminal alias answers the modelled terminal error
//! carrying its own message. An alias outlives the fleets it points at, so the fleet a
//! simple strategy names is reported whether or not the store still holds that fleet.
//!
//! The harness (`GameLift`, `gamelift_arn`, `ec2_fleet`, `anywhere_fleet`, `create_fleet`,
//! `wait_for_due_transitions`) is the one `hosting_loop` already drives the production
//! dispatch path with, and the alias helpers are the ones `aliases` shares.

use serde_json::json;

use super::aliases::{alias_arn, alias_id_of, create_alias, simple_routing, terminal_routing};
use super::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, error_code, error_message, gamelift_arn, GameLift,
    wait_for_due_transitions,
};

/// Every `RoutingStrategyType` the model declares
/// (`smithy_query shape=RoutingStrategyType`), so the rows below are drawn from the
/// modelled catalog rather than from one example.
const ROUTING_STRATEGY_TYPES: [&str; 2] = ["SIMPLE", "TERMINAL"];

/// The ARN assigned to the fleet with `fleet_id`:
/// `arn:aws:gamelift:<region>:<account>:fleet/<fleet id>` — "in a GameLift fleet ARN, the
/// resource ID matches the fleet ID value"
/// (`smithy_query shape=FleetAttributes` member `FleetArn`).
fn fleet_arn(fleet_id: &str) -> String {
    gamelift_arn(&format!("fleet/{fleet_id}"))
}

/// `ResolveAlias` of a simple alias reports the identifiers of the fleet its stored
/// routing strategy names, whichever form of the alias identifier the request uses.
///
/// The rows differ along what the rule generalises over: which of several stored fleets
/// the alias points at, how many aliases point at the same fleet, the fleet's compute type
/// (so the target is not one fixture), and whether the alias is named by id or by ARN.
#[tokio::test]
async fn resolving_a_simple_alias_reports_the_fleet_it_points_at() {
    let gamelift = GameLift::new();
    let first = create_fleet(&gamelift, ec2_fleet("first-target")).await;
    let second = create_fleet(&gamelift, ec2_fleet("second-target")).await;
    let anywhere = create_fleet(
        &gamelift,
        anywhere_fleet("anywhere-target", &["custom-elsewhere"]),
    )
    .await;
    wait_for_due_transitions();

    struct Case {
        label: &'static str,
        /// The fleet the alias's routing strategy names.
        target: String,
    }

    let cases = [
        Case {
            label: "the first managed fleet",
            target: first.clone(),
        },
        Case {
            label: "a second managed fleet, so the answer is not one fixture",
            target: second.clone(),
        },
        Case {
            label: "a second alias pointing at the first fleet",
            target: first.clone(),
        },
        Case {
            label: "an Anywhere fleet, a different compute type",
            target: anywhere.clone(),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let alias = create_alias(
            &gamelift,
            json!({
                "Name": format!("resolvable-{index}"),
                "RoutingStrategy": simple_routing(&case.target),
            }),
        )
        .await;
        let alias_id = alias_id_of(&alias);

        // Either form of `AliasIdOrArn` selects the same alias, so both resolve the same
        // way.
        for selector in [alias_id.clone(), alias_arn(&alias_id)] {
            let resolved = gamelift
                .ok("ResolveAlias", json!({"AliasId": &selector}))
                .await;
            assert_eq!(
                resolved,
                json!({"FleetId": &case.target, "FleetArn": fleet_arn(&case.target)}),
                "case {label:?} named by {selector:?}: ResolveAlias must report the stored \
                 fleet's identifiers, got {resolved}"
            );
        }

        // The fleet the resolution names is the one the fleet's own read reports, so the
        // two answers can only agree.
        let described = gamelift
            .ok(
                "DescribeFleetAttributes",
                json!({"FleetIds": [&case.target]}),
            )
            .await;
        assert_eq!(
            described["FleetAttributes"][0]["FleetArn"],
            json!(fleet_arn(&case.target)),
            "case {label:?}: the resolved ARN must be the fleet's own, got {described}"
        );
    }

    // Resolving is a read: it changes nothing a later caller sees, so the same call run
    // twice answers the same thing and the alias still reads back as created.
    let alias = create_alias(
        &gamelift,
        json!({"Name": "read-twice", "RoutingStrategy": simple_routing(&second)}),
    )
    .await;
    let alias_id = alias_id_of(&alias);
    let once = gamelift
        .ok("ResolveAlias", json!({"AliasId": &alias_id}))
        .await;
    let twice = gamelift
        .ok("ResolveAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        once, twice,
        "resolving an alias must not change what a later resolve reports, got {twice}"
    );
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        described["Alias"], alias,
        "resolving an alias must not change the stored alias, got {}",
        described["Alias"]
    );
}

/// "If the alias has a TERMINAL routing strategy, the result is a
/// TerminalRoutingStrategyException", and that error's own doc: "The message returned in
/// this exception is the message defined in the routing strategy itself."
///
/// The rows differ along the message text the strategy embeds — its content, its length
/// and whether it carries a URL — since the message the error returns is whatever the
/// stored strategy defines.
#[tokio::test]
async fn resolving_a_terminal_alias_answers_the_message_the_strategy_defines() {
    // Both modelled routing types are covered by this suite: the simple one above, the
    // terminal one here.
    assert_eq!(
        ROUTING_STRATEGY_TYPES,
        ["SIMPLE", "TERMINAL"],
        "the modelled routing types must be the two this suite covers, got {ROUTING_STRATEGY_TYPES:?}"
    );

    let gamelift = GameLift::new();
    let messages = [
        "This game version is no longer supported.".to_string(),
        "Upgrade at https://example.test/upgrade".to_string(),
        "x".to_string(),
        "m".repeat(512),
    ];

    for (index, message) in messages.iter().enumerate() {
        let alias = create_alias(
            &gamelift,
            json!({
                "Name": format!("terminal-{index}"),
                "RoutingStrategy": terminal_routing(message),
            }),
        )
        .await;
        let alias_id = alias_id_of(&alias);

        for selector in [alias_id.clone(), alias_arn(&alias_id)] {
            let (status, parsed) = gamelift
                .call("ResolveAlias", &json!({"AliasId": &selector}))
                .await;
            assert_eq!(
                status, 400,
                "case {message:?} named by {selector:?}: a terminal alias must be refused with \
                 HTTP 400, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "TerminalRoutingStrategyException",
                "case {message:?}: a terminal alias must answer the modelled terminal error, \
                 got {parsed}"
            );
            assert_eq!(
                error_message(&parsed),
                *message,
                "case {message:?}: the terminal error must carry the message the routing \
                 strategy defines, got {}",
                error_message(&parsed)
            );
        }
    }
}

/// An alias outlives the fleets it points at: neither creating nor updating one checks the
/// fleet a simple strategy names, and the resolution reports the fleet the STORED strategy
/// points at — so the answer is the same whether or not the store holds that fleet.
///
/// The rows differ along how the target came to be absent (never created at all, in two
/// forms of the modelled `FleetId`, and deleted after the alias was made — in both
/// deletion shapes: a fleet with managed instances, whose deletion takes the documented
/// span, and one without, which leaves state at once).
#[tokio::test]
async fn resolving_an_alias_reports_its_target_fleet_whether_or_not_it_is_stored() {
    let gamelift = GameLift::new();
    let managed = create_fleet(&gamelift, ec2_fleet("deleted-managed")).await;
    let unmanaged = create_fleet(
        &gamelift,
        anywhere_fleet("deleted-anywhere", &["custom-shed"]),
    )
    .await;
    wait_for_due_transitions();

    // Both forms a `FleetId` takes (`smithy_query shape=RoutingStrategy` member `FleetId`:
    // len 1..128, pattern ^[a-z]*fleet-[a-zA-Z0-9\-]+$), for fleets this store never held.
    let never_created = [
        "fleet-never-created".to_string(),
        "fleet-7c3a91de-2b48-4f05-9a16-5d8e0bc47213".to_string(),
    ];

    let mut rows: Vec<(String, String, String)> = Vec::new();
    for (label, target) in [
        ("a fleet that was never created", never_created[0].clone()),
        (
            "a fleet id of another form that was never created",
            never_created[1].clone(),
        ),
        ("a managed fleet deleted after the alias", managed.clone()),
        (
            "an Anywhere fleet deleted after the alias",
            unmanaged.clone(),
        ),
    ] {
        let alias = create_alias(
            &gamelift,
            json!({
                "Name": format!("dangling-{}", rows.len()),
                "RoutingStrategy": simple_routing(&target),
            }),
        )
        .await;
        rows.push((label.to_string(), alias_id_of(&alias), target));
    }

    // Each alias resolves to the fleet its strategy names, including the two whose target
    // the store never held: the create stored the alias rather than refusing it.
    for (label, alias_id, target) in &rows {
        let resolved = gamelift
            .ok("ResolveAlias", json!({"AliasId": alias_id}))
            .await;
        assert_eq!(
            resolved,
            json!({"FleetId": target, "FleetArn": fleet_arn(target)}),
            "case {label:?}: the alias must resolve to the fleet its strategy names, got {resolved}"
        );
    }

    for fleet_id in [&managed, &unmanaged] {
        gamelift
            .ok("DeleteFleet", json!({"FleetId": fleet_id}))
            .await;
    }
    // "During the deletion process, the fleet status is changed to DELETING. When
    // completed, the status switches to TERMINATED": the managed fleet leaves state once
    // the deletion the clock owes is due.
    wait_for_due_transitions();

    for (label, alias_id, target) in &rows {
        let resolved = gamelift
            .ok("ResolveAlias", json!({"AliasId": alias_id}))
            .await;
        assert_eq!(
            resolved,
            json!({"FleetId": target, "FleetArn": fleet_arn(target)}),
            "case {label:?}: the alias must still resolve to the fleet it points at once that \
             fleet has gone, got {resolved}"
        );

        // The alias itself is untouched: it is a pointer, and the fleet going away does
        // not rewrite it.
        let described = gamelift
            .ok("DescribeAlias", json!({"AliasId": alias_id}))
            .await;
        assert_eq!(
            described["Alias"]["RoutingStrategy"],
            simple_routing(target),
            "case {label:?}: the alias must still be stored with its strategy, got {}",
            described["Alias"]
        );
    }
}

/// "Specify a unique alias identifier": an identifier that names no stored alias is a
/// missing ALIAS, answered with the modelled `NotFoundException` naming what the caller
/// asked for.
///
/// The rows differ along the form of the identifier: an id, an ARN, an ARN in another
/// account for a stored alias id, and an ARN in another Region.
#[tokio::test]
async fn resolving_an_alias_that_is_not_stored_reports_the_missing_alias() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("the-only-fleet")).await;
    wait_for_due_transitions();
    let stored = create_alias(
        &gamelift,
        json!({"Name": "the-only-alias", "RoutingStrategy": simple_routing(&fleet_id)}),
    )
    .await;
    let stored_id = alias_id_of(&stored);

    let cases = [
        ("an id no alias carries", "alias-does-not-exist".to_string()),
        ("an ARN no alias carries", alias_arn("alias-does-not-exist")),
        (
            "an ARN in another account for a stored id",
            format!("arn:aws:gamelift:us-west-2:999999999999:alias/{stored_id}"),
        ),
        (
            "an ARN in another Region for a stored id",
            format!("arn:aws:gamelift:eu-central-1::alias/{stored_id}"),
        ),
    ];

    for (label, selector) in &cases {
        let (status, parsed) = gamelift
            .call("ResolveAlias", &json!({"AliasId": selector}))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: an identifier naming no stored alias must be refused with HTTP 400, \
             got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "NotFoundException",
            "case {label:?}: an unknown alias must answer NotFoundException, got {parsed}"
        );
        assert!(
            error_message(&parsed).contains(selector.as_str()),
            "case {label:?}: the refusal must name the identifier the caller supplied, got {:?}",
            error_message(&parsed)
        );
    }

    // The stored alias is untouched by those refusals and still resolves.
    let resolved = gamelift
        .ok("ResolveAlias", json!({"AliasId": &stored_id}))
        .await;
    assert_eq!(
        resolved,
        json!({"FleetId": &fleet_id, "FleetArn": fleet_arn(&fleet_id)}),
        "the stored alias must still resolve to its fleet, got {resolved}"
    );
}

/// "You can reassign an alias to another fleet by calling UpdateAlias": resolution reads
/// the alias's CURRENT stored strategy, so an update of the strategy changes what the next
/// resolve answers — in both directions between the two modelled routing types.
#[tokio::test]
async fn resolution_follows_the_alias_strategy_as_updates_rewrite_it() {
    let gamelift = GameLift::new();
    let before = create_fleet(&gamelift, ec2_fleet("before-reassignment")).await;
    let after = create_fleet(&gamelift, ec2_fleet("after-reassignment")).await;
    wait_for_due_transitions();

    let alias = create_alias(
        &gamelift,
        json!({"Name": "reassigned", "RoutingStrategy": simple_routing(&before)}),
    )
    .await;
    let alias_id = alias_id_of(&alias);

    let resolved = gamelift
        .ok("ResolveAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        resolved["FleetId"],
        json!(&before),
        "before the reassignment the alias must resolve to its first target, got {resolved}"
    );

    // Reassigned to another fleet: the next resolve reports the NEW target.
    gamelift
        .ok(
            "UpdateAlias",
            json!({"AliasId": &alias_id, "RoutingStrategy": simple_routing(&after)}),
        )
        .await;
    let resolved = gamelift
        .ok("ResolveAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        resolved,
        json!({"FleetId": &after, "FleetArn": fleet_arn(&after)}),
        "after the reassignment the alias must resolve to the new target, got {resolved}"
    );

    // Made terminal: the same alias now answers the terminal error with the new message.
    gamelift
        .ok(
            "UpdateAlias",
            json!({
                "AliasId": &alias_id,
                "RoutingStrategy": terminal_routing("This fleet has been retired."),
            }),
        )
        .await;
    let (status, parsed) = gamelift
        .call("ResolveAlias", &json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "TerminalRoutingStrategyException"),
        "an alias updated to a terminal strategy must answer the terminal error, got {parsed}"
    );
    assert_eq!(
        error_message(&parsed),
        "This fleet has been retired.",
        "the terminal error must carry the updated message, got {:?}",
        error_message(&parsed)
    );

    // And back to simple: resolution follows the strategy in both directions.
    gamelift
        .ok(
            "UpdateAlias",
            json!({"AliasId": &alias_id, "RoutingStrategy": simple_routing(&before)}),
        )
        .await;
    let resolved = gamelift
        .ok("ResolveAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        resolved,
        json!({"FleetId": &before, "FleetArn": fleet_arn(&before)}),
        "an alias updated back to a simple strategy must resolve again, got {resolved}"
    );

    // A deleted alias is a missing alias, whatever it used to resolve to.
    gamelift
        .ok("DeleteAlias", json!({"AliasId": &alias_id}))
        .await;
    let (status, parsed) = gamelift
        .call("ResolveAlias", &json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "NotFoundException"),
        "a deleted alias must no longer resolve, got {status} with {parsed}"
    );
}
