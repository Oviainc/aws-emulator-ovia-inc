// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateAlias` call must NOT disturb, over the wire.
//!
//! The update writes the properties its request supplies onto the ONE stored alias its
//! identifier names and reports that record — so two invariants hold around it, whatever
//! the request carries:
//!
//! * a refused update writes nothing, not even the properties of the same request that
//!   were acceptable on their own, so the stored alias reads back exactly as it was;
//! * an accepted update reaches only the alias it names, so every other stored alias reads
//!   back exactly as it was, member for member.
//!
//! The harness (`GameLift`, `gamelift_arn`) is the one `hosting_loop` already drives the
//! production dispatch path with, and the alias helpers are the ones `aliases` and
//! `resource_updates` share.

use serde_json::{json, Value};

use super::aliases::{alias_arn, alias_id_of, create_alias, simple_routing, terminal_routing};
use super::hosting_loop::{gamelift_arn, GameLift};
use super::resource_updates::described_alias;

/// A routing strategy an alias cannot carry, and why: `RoutingStrategy$Type` names both
/// forms and what each one needs — "SIMPLE - The alias resolves to one specific fleet", so
/// a simple strategy names that fleet; "TERMINAL - The alias does not resolve to a fleet
/// but instead can be used to display a message to the user", so a terminal strategy
/// carries that message. The two types come from the modelled `RoutingStrategyType` enum
/// (`smithy_query shape=RoutingStrategyType`), which is also where the absent-type rows
/// below get their "neither type" case.
struct Incomplete {
    label: &'static str,
    /// The strategy the update supplies, missing the member its own type requires.
    routing_strategy: Value,
    /// The properties the SAME request supplies that would each be accepted alone.
    valid_properties: Value,
}

/// An `UpdateAlias` whose routing strategy is not one of the two forms an alias can carry
/// is refused, and the stored alias keeps every property it had — including the ones the
/// refused request supplied alongside the bad strategy.
///
/// The rows differ along which member the strategy is missing, which type it claims (both
/// modelled values, plus a request that claims none), and which otherwise-acceptable
/// properties travel with it.
#[tokio::test]
async fn a_refused_alias_update_writes_none_of_its_request() {
    let cases = [
        Incomplete {
            label: "a simple strategy naming no fleet, beside a new name and description",
            routing_strategy: json!({"Type": "SIMPLE"}),
            valid_properties: json!({"Name": "renamed", "Description": "redescribed"}),
        },
        Incomplete {
            label: "a simple strategy carrying a message instead of a fleet, beside a new name",
            routing_strategy: json!({"Type": "SIMPLE", "Message": "no fleet here"}),
            valid_properties: json!({"Name": "renamed-by-a-refused-update"}),
        },
        Incomplete {
            label: "a terminal strategy carrying no message, beside a new description",
            routing_strategy: json!({"Type": "TERMINAL"}),
            valid_properties: json!({"Description": "redescribed by a refused update"}),
        },
        Incomplete {
            label: "a terminal strategy naming a fleet instead of a message, beside both",
            routing_strategy: json!({"Type": "TERMINAL", "FleetId": "fleet-ignored"}),
            valid_properties: json!({"Name": "renamed", "Description": "redescribed"}),
        },
        Incomplete {
            label: "a strategy with no type at all, beside a new name",
            routing_strategy: json!({}),
            valid_properties: json!({"Name": "renamed-with-a-typeless-strategy"}),
        },
        Incomplete {
            label: "a strategy naming a fleet but no type, beside a new description",
            routing_strategy: json!({"FleetId": "fleet-typeless"}),
            valid_properties: json!({"Description": "redescribed with a typeless strategy"}),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        // Half the rows start terminal and half start simple, so the refusal is observed
        // against both routings an alias can already carry.
        let starting_strategy = if index % 2 == 0 {
            terminal_routing("the message the create supplied")
        } else {
            simple_routing("fleet-as-created")
        };
        let created = create_alias(
            &gamelift,
            json!({
                "Name": format!("refused-update-target-{index}"),
                "Description": "the description the create supplied",
                "RoutingStrategy": starting_strategy,
            }),
        )
        .await;
        let alias_id = alias_id_of(&created);
        let before = described_alias(&gamelift, &alias_id).await;

        let mut request = case.valid_properties.clone();
        request["AliasId"] = json!(&alias_id);
        request["RoutingStrategy"] = case.routing_strategy.clone();
        let (status, code) = gamelift.err("UpdateAlias", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the update must be refused, got {status} {code}"
        );

        // Nothing was written: not the strategy the request was refused for, and not the
        // name or description it carried beside it — including `LastUpdatedTime`, which
        // only a call that actually modified the alias moves.
        let after = described_alias(&gamelift, &alias_id).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused update must write nothing, expected {before}, got {after}"
        );
    }
}

/// `UpdateAlias` names ONE alias, so an accepted update changes that alias and leaves
/// every other stored alias as it was — whether the update selected its target by alias id
/// or by alias ARN.
///
/// The rows differ along which stored alias is the target, which properties the update
/// supplies, both modelled routing types, and whether the identifier is an id or an ARN.
#[tokio::test]
async fn an_accepted_alias_update_reaches_only_the_alias_it_names() {
    /// One alias of the store the update runs against.
    struct Stored {
        name: &'static str,
        routing_strategy: Value,
    }

    struct Case {
        label: &'static str,
        /// Which of the stored aliases below the update names.
        target: usize,
        /// Whether the update selects it by ARN rather than by alias id.
        by_arn: bool,
        /// The update, without its identifier.
        update: Value,
        /// The members the target must report after the update.
        expected: Vec<(&'static str, Value)>,
    }

    let stored = [
        Stored {
            name: "live",
            routing_strategy: simple_routing("fleet-live"),
        },
        Stored {
            name: "retired",
            routing_strategy: terminal_routing("This version is retired."),
        },
        Stored {
            name: "canary",
            routing_strategy: simple_routing("fleet-canary"),
        },
    ];

    let cases = [
        Case {
            label: "the first alias reassigned onto another fleet, by id",
            target: 0,
            by_arn: false,
            update: json!({"RoutingStrategy": {"Type": "SIMPLE", "FleetId": "fleet-next"}}),
            expected: vec![(
                "RoutingStrategy",
                json!({"Type": "SIMPLE", "FleetId": "fleet-next"}),
            )],
        },
        Case {
            label: "the middle alias re-messaged and renamed, by ARN",
            target: 1,
            by_arn: true,
            update: json!({
                "Name": "retired-for-the-season",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "Back next season."},
            }),
            expected: vec![
                ("Name", json!("retired-for-the-season")),
                (
                    "RoutingStrategy",
                    json!({"Type": "TERMINAL", "Message": "Back next season."}),
                ),
            ],
        },
        Case {
            label: "the last alias described anew, by id",
            target: 2,
            by_arn: false,
            update: json!({"Description": "the canary of the week"}),
            expected: vec![("Description", json!("the canary of the week"))],
        },
        Case {
            label: "the last alias turned terminal, by ARN",
            target: 2,
            by_arn: true,
            update: json!({"RoutingStrategy": {"Type": "TERMINAL", "Message": "Canary closed."}}),
            expected: vec![(
                "RoutingStrategy",
                json!({"Type": "TERMINAL", "Message": "Canary closed."}),
            )],
        },
    ];

    let gamelift = GameLift::new();
    let mut alias_ids = Vec::new();
    for alias in &stored {
        let created = create_alias(
            &gamelift,
            json!({
                "Name": alias.name,
                "Description": format!("{} as created", alias.name),
                "RoutingStrategy": alias.routing_strategy.clone(),
            }),
        )
        .await;
        alias_ids.push(alias_id_of(&created));
    }

    for case in &cases {
        let label = case.label;
        let target_id = alias_ids[case.target].clone();
        // Every stored alias as it reads now, so the rows compose: each update is judged
        // against the store the previous one left behind.
        let mut before = Vec::new();
        for alias_id in &alias_ids {
            before.push(described_alias(&gamelift, alias_id).await);
        }

        let selector = if case.by_arn {
            alias_arn(&target_id)
        } else {
            target_id.clone()
        };
        let mut request = case.update.clone();
        request["AliasId"] = json!(&selector);
        let updated = gamelift.ok("UpdateAlias", request).await;

        for (index, alias_id) in alias_ids.iter().enumerate() {
            let after = described_alias(&gamelift, alias_id).await;
            if index == case.target {
                assert_eq!(
                    updated["Alias"], after,
                    "case {label:?}: the update must report the stored alias the describe \
                     serves, got {}",
                    updated["Alias"]
                );
                let mut expected = before[index].clone();
                for (member, value) in &case.expected {
                    expected[*member] = value.clone();
                }
                // `Alias$LastUpdatedTime` is "the time that this data object was last
                // modified", so a call that changed a property is free to move it.
                expected["LastUpdatedTime"] = after["LastUpdatedTime"].clone();
                assert_eq!(
                    after, expected,
                    "case {label:?}: the named alias must change only the properties the \
                     request supplied, expected {expected}, got {after}"
                );
            } else {
                assert_eq!(
                    after, before[index],
                    "case {label:?}: the alias the update did not name must be untouched, \
                     expected {}, got {after}",
                    before[index]
                );
            }
        }
    }

    // An identifier naming no stored alias is refused, and no stored alias is touched by
    // the attempt — the same two selector forms the accepted rows above used.
    for selector in [
        "alias-not-in-this-store".to_string(),
        gamelift_arn("alias/alias-not-in-this-store"),
    ] {
        let mut before = Vec::new();
        for alias_id in &alias_ids {
            before.push(described_alias(&gamelift, alias_id).await);
        }
        let (status, code) = gamelift
            .err(
                "UpdateAlias",
                json!({"AliasId": &selector, "Description": "written by nobody"}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {selector:?}: an identifier naming no stored alias must be refused, got \
             {status} {code}"
        );
        for (index, alias_id) in alias_ids.iter().enumerate() {
            let after = described_alias(&gamelift, alias_id).await;
            assert_eq!(
                after, before[index],
                "case {selector:?}: a refused update must leave every stored alias as it \
                 was, expected {}, got {after}",
                before[index]
            );
        }
    }
}
