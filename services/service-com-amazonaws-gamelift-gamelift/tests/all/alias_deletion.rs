// SPDX-License-Identifier: CC0-1.0

//! Deleting a fleet alias, over the wire: `DeleteAlias` "removes all record of the alias",
//! so the alias the request names — by alias id or by alias ARN — is gone from every later
//! read of the store, a request routed through it finds no alias, the rest of the account's
//! aliases are untouched, and an identifier that resolves to no stored alias is refused with
//! the modelled `NotFoundException` without removing anything.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `create_fleet`, `ec2_fleet`,
//! `wait_for_due_transitions`) is the one `hosting_loop.rs` defines, and the alias helpers
//! are the ones `aliases.rs` already drives `CreateAlias` with.

use serde_json::{json, Value};

use crate::aliases::{alias_arn, alias_id_of, create_alias, simple_routing, terminal_routing};
use crate::hosting_loop::{
    create_fleet, ec2_fleet, error_code, error_message, wait_for_due_transitions, GameLift,
};

/// The one wording this service refuses an alias identifier naming nothing stored with,
/// shared by `DescribeAlias`, `UpdateAlias` and `DeleteAlias`: the resource kind, then the
/// identifier the CALLER supplied, verbatim, in parentheses
/// (`state::alias_not_found_message`).
fn alias_not_found_message(selector: &str) -> String {
    format!("Alias ({selector}) not found")
}

/// The `Alias` a describe reports for `selector`, asserting the call succeeded.
async fn describe_alias(gamelift: &GameLift, selector: &str) -> Value {
    gamelift
        .ok("DescribeAlias", json!({"AliasId": selector}))
        .await["Alias"]
        .clone()
}

/// `DeleteAlias` removes the one alias its identifier resolves to: the delete succeeds with
/// an empty body, every later read of that alias answers the modelled `NotFoundException`,
/// and the other aliases in the store still read back exactly as they did before.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// alias (id or ARN), how many aliases the store holds, which of them the request deletes
/// (the only one, the first, a middle one, the last), which modelled `RoutingStrategyType`
/// the target carries, whether it carries a description, how many tags it carries, and
/// whether `UpdateAlias` revised it before the delete.
#[tokio::test]
async fn deleting_an_alias_removes_the_one_alias_the_identifier_resolves_to() {
    struct Case {
        label: &'static str,
        /// How many aliases the store holds when the delete runs.
        store: usize,
        /// The index, within that store, of the alias the request deletes.
        target: usize,
        /// Whether the request names the target by its ARN rather than by its id.
        by_arn: bool,
        /// Whether the target routes at a fleet (`SIMPLE`) rather than at a message
        /// (`TERMINAL`).
        simple: bool,
        /// Whether the target carries a description.
        described: bool,
        /// How many tags the target carries.
        tags: usize,
        /// Whether `UpdateAlias` revised the target before the delete.
        updated: bool,
    }

    let cases = [
        Case {
            label: "the only alias in the store, named by its id",
            store: 1,
            target: 0,
            by_arn: false,
            simple: true,
            described: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the only alias in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            simple: true,
            described: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the first of three aliases, named by its id",
            store: 3,
            target: 0,
            by_arn: false,
            simple: true,
            described: true,
            tags: 0,
            updated: false,
        },
        Case {
            label: "a middle alias of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
            simple: true,
            described: false,
            tags: 1,
            updated: false,
        },
        Case {
            label: "the last of four aliases, named by its id",
            store: 4,
            target: 3,
            by_arn: false,
            simple: false,
            described: false,
            tags: 3,
            updated: false,
        },
        Case {
            label: "a terminal alias of two, named by its ARN",
            store: 2,
            target: 0,
            by_arn: true,
            simple: false,
            described: true,
            tags: 2,
            updated: false,
        },
        Case {
            label: "an alias whose properties were updated first, named by its id",
            store: 3,
            target: 1,
            by_arn: false,
            simple: true,
            described: false,
            tags: 0,
            updated: true,
        },
        Case {
            label: "an updated terminal alias, named by its ARN",
            store: 2,
            target: 1,
            by_arn: true,
            simple: false,
            described: true,
            tags: 1,
            updated: true,
        },
    ];

    for case in &cases {
        let label = case.label;
        // Each row gets its own store, so what survives the delete is only what this row
        // created.
        let gamelift = GameLift::new();
        let mut aliases = Vec::new();
        for index in 0..case.store {
            let is_target = index == case.target;
            let routing = if is_target && !case.simple {
                terminal_routing(&format!("row {index} is retired"))
            } else {
                simple_routing(&format!("fleet-target-{index}"))
            };
            let mut request = json!({"Name": format!("deletable-{index}"), "RoutingStrategy": routing});
            if is_target && case.described {
                request["Description"] = json!(format!("the alias of row {index}"));
            }
            if is_target && case.tags > 0 {
                request["Tags"] = Value::Array(
                    (0..case.tags)
                        .map(|tag| json!({"Key": format!("key-{tag}"), "Value": format!("value-{tag}")}))
                        .collect(),
                );
            }
            aliases.push(create_alias(&gamelift, request).await);
        }

        if case.updated {
            let target_id = alias_id_of(&aliases[case.target]);
            let updated = gamelift
                .ok(
                    "UpdateAlias",
                    json!({
                        "AliasId": &target_id,
                        "Name": "revised-before-deletion",
                        "RoutingStrategy": simple_routing("fleet-reassigned"),
                    }),
                )
                .await;
            aliases[case.target] = updated["Alias"].clone();
        }

        let target_id = alias_id_of(&aliases[case.target]);
        let target_arn = alias_arn(&target_id);
        let selector = if case.by_arn {
            target_arn.clone()
        } else {
            target_id.clone()
        };

        // Every alias reads back as stored before the delete, so what changes below is the
        // delete's doing and nothing else.
        for (index, alias) in aliases.iter().enumerate() {
            let described = describe_alias(&gamelift, &alias_id_of(alias)).await;
            assert_eq!(
                &described, alias,
                "case {label:?}: alias {index} must read back as created, got {described}"
            );
        }

        let (status, body) = gamelift
            .call("DeleteAlias", &json!({"AliasId": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored alias must succeed, got HTTP {status} with body {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteAlias reports no members, got {body}"
        );

        // The deleted record is gone from the one store every read of the alias is served
        // from: both identifier forms, the describe path, the update path and a second
        // delete.
        for probe in [target_id.clone(), target_arn.clone()] {
            for operation in ["DescribeAlias", "UpdateAlias", "DeleteAlias"] {
                let mut request = json!({"AliasId": &probe});
                if operation == "UpdateAlias" {
                    request["Name"] = json!("after-deletion");
                }
                let (status, body) = gamelift.call(operation, &request).await;
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "NotFoundException"),
                    "case {label:?}: {operation} naming the deleted {probe:?} must be refused, \
                     got {status} {code} with body {body}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message,
                    alias_not_found_message(&probe),
                    "case {label:?}: {operation} naming the deleted {probe:?} must report the \
                     identifier the request carried, got {message:?}"
                );
            }
        }

        // Every other alias of the store is untouched — member for member, by id and by
        // ARN.
        for (index, alias) in aliases.iter().enumerate() {
            if index == case.target {
                continue;
            }
            let survivor_id = alias_id_of(alias);
            for probe in [survivor_id.clone(), alias_arn(&survivor_id)] {
                let described = describe_alias(&gamelift, &probe).await;
                assert_eq!(
                    &described, alias,
                    "case {label:?}: alias {index} must survive the delete unchanged, got {described}"
                );
            }
        }
    }
}

/// `DeleteAlias` acts on ONE named resource, so an identifier that resolves to no stored
/// alias is the modelled `NotFoundException` rather than an empty success — and the aliases
/// that ARE stored are left exactly as they were.
#[tokio::test]
async fn deleting_an_alias_that_is_not_stored_is_refused_and_removes_nothing() {
    let gamelift = GameLift::new();
    let stored = create_alias(
        &gamelift,
        json!({
            "Name": "the-surviving-alias",
            "RoutingStrategy": simple_routing("fleet-survivor"),
            "Tags": [{"Key": "owner", "Value": "liveops"}],
        }),
    )
    .await;
    let stored_id = alias_id_of(&stored);

    // An alias created and then deleted: its identifier is well formed and resolves to
    // nothing, exactly like one that never existed.
    let deleted = create_alias(
        &gamelift,
        json!({"Name": "the-deleted-alias", "RoutingStrategy": terminal_routing("retired")}),
    )
    .await;
    let deleted_id = alias_id_of(&deleted);
    gamelift
        .ok("DeleteAlias", json!({"AliasId": &deleted_id}))
        .await;

    // Rows differ in the form of the identifier (bare id, UUID-shaped id, ARN), in whether
    // the alias never existed or was deleted, and in which account and region the ARN
    // names — each well formed against the modelled `AliasIdOrArn`
    // (`^(alias-\S+|arn:.*:alias\/alias-\S+)$`).
    let cases: Vec<(&str, String)> = vec![
        ("an id no alias carries", "alias-does-not-exist".to_string()),
        (
            "a UUID-shaped id no alias carries",
            "alias-00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("an ARN no alias carries", alias_arn("alias-does-not-exist")),
        ("the id of a deleted alias", deleted_id.clone()),
        ("the ARN of a deleted alias", alias_arn(&deleted_id)),
        (
            "an ARN in another account for a stored id",
            format!("arn:aws:gamelift:us-west-2:999999999999:alias/{stored_id}"),
        ),
        (
            "an ARN in another region for a stored id",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:alias/{stored_id}",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (label, selector) in &cases {
        let (status, body) = gamelift
            .call("DeleteAlias", &json!({"AliasId": selector}))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored alias must be refused, got {status} \
             {code} with body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            alias_not_found_message(selector),
            "case {label:?}: the refusal must name the identifier the request carried, got \
             {message:?}"
        );
    }

    // None of those refusals removed anything: the stored alias still reads back as
    // created.
    let described = describe_alias(&gamelift, &stored_id).await;
    assert_eq!(
        described, stored,
        "the stored alias must survive every refused delete, got {described}"
    );
}

/// "Game clients attempting to access a server process using the deleted alias receive an
/// error": once the alias is gone, an operation that accepts an alias identifier no longer
/// routes through it — the alias is missing, not the fleet it used to name, which is still
/// there and still serves requests that name it directly.
#[tokio::test]
async fn an_operation_that_takes_a_deleted_alias_finds_no_alias() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("aliased-then-unaliased")).await;
    wait_for_due_transitions();

    let alias = create_alias(
        &gamelift,
        json!({"Name": "live-then-deleted", "RoutingStrategy": simple_routing(&fleet_id)}),
    )
    .await;
    let alias_id = alias_id_of(&alias);

    // While the alias is stored it routes at its fleet.
    let session = gamelift
        .ok(
            "CreateGameSession",
            json!({"AliasId": &alias_id, "MaximumPlayerSessionCount": 2}),
        )
        .await;
    assert_eq!(
        session["GameSession"]["FleetId"],
        json!(&fleet_id),
        "a stored simple alias must place the session on the fleet it names, got {}",
        session["GameSession"]
    );

    gamelift
        .ok("DeleteAlias", json!({"AliasId": &alias_id}))
        .await;

    // Both the mutating and the reporting path, by id and by ARN, now find no alias.
    for selector in [alias_id.clone(), alias_arn(&alias_id)] {
        for operation in ["CreateGameSession", "DescribeGameSessions"] {
            let mut request = json!({"AliasId": &selector});
            if operation == "CreateGameSession" {
                request["MaximumPlayerSessionCount"] = json!(2);
            }
            let (status, body) = gamelift.call(operation, &request).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "NotFoundException"),
                "case {operation:?} naming {selector:?}: a deleted alias must be refused, got \
                 {status} {code} with body {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                alias_not_found_message(&selector),
                "case {operation:?} naming {selector:?}: the refusal must name the identifier the \
                 request carried, got {message:?}"
            );
        }
    }

    // The fleet the alias pointed at is NOT touched by the delete: it still serves the
    // session placed through the alias, named directly.
    let described = gamelift
        .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
        .await;
    let placed = described["GameSessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        placed.len(),
        1,
        "deleting an alias must leave the fleet's sessions in place, got {described}"
    );
    let fleet_attributes = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
        .await;
    assert_eq!(
        fleet_attributes["FleetAttributes"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "deleting an alias must leave the fleet it pointed at stored, got {fleet_attributes}"
    );
}

/// An alias record lives until `DeleteAlias` removes it, so the account's alias collection
/// really is one shorter afterwards: with the quota full, the next create is refused, and
/// one delete makes room for exactly one more create.
#[tokio::test]
async fn a_delete_frees_a_slot_in_the_accounts_alias_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_ALIASES_PER_ACCOUNT;
    let gamelift = GameLift::new();
    let mut ids = Vec::new();
    for index in 0..quota {
        let created = create_alias(
            &gamelift,
            json!({
                "Name": format!("quota-{index}"),
                "RoutingStrategy": simple_routing("fleet-quota"),
            }),
        )
        .await;
        ids.push(alias_id_of(&created));
    }

    let (status, code) = gamelift
        .err(
            "CreateAlias",
            json!({
                "Name": "before-the-delete",
                "RoutingStrategy": simple_routing("fleet-quota"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "a create with the collection full must be refused, got {status} {code}"
    );

    gamelift
        .ok("DeleteAlias", json!({"AliasId": &ids[0]}))
        .await;

    let created = create_alias(
        &gamelift,
        json!({
            "Name": "after-the-delete",
            "RoutingStrategy": simple_routing("fleet-quota"),
        }),
    )
    .await;
    let fresh_id = alias_id_of(&created);
    assert_ne!(
        fresh_id, ids[0],
        "the create after a delete must mint a NEW alias id, got the deleted {fresh_id:?}"
    );

    // The collection is full again: the slot the delete freed took exactly one alias.
    let (status, code) = gamelift
        .err(
            "CreateAlias",
            json!({
                "Name": "one-too-many-again",
                "RoutingStrategy": simple_routing("fleet-quota"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "one delete must free exactly one slot, got {status} {code}"
    );

    // The aliases the delete did not name are still there.
    let described = describe_alias(&gamelift, &ids[1]).await;
    assert_eq!(
        described["Name"],
        json!("quota-1"),
        "an alias the delete did not name must be unchanged, got {described}"
    );
}
