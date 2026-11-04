// SPDX-License-Identifier: CC0-1.0

//! The alias listing contract, over the wire: `ListAliases` reports the stored aliases,
//! narrowed by each filter the request carries and handed out in sequential pages.
//!
//! The rule the rows below assert: a reporting list answers the stored collection
//! narrowed by every filter the request sets — each matching the corresponding member of
//! the stored record exactly — and pages that narrowed collection, where a `NextToken`
//! resumes at the boundary the previous page ended on and a token the collection did not
//! issue is refused.
//!
//! The harness (`GameLift`) is the one `hosting_loop` already drives the production
//! dispatch path with; the alias helpers are `aliases`'.

use serde_json::{json, Value};

use super::aliases::{alias_id_of, create_alias, simple_routing, terminal_routing};
use super::hosting_loop::GameLift;

/// The modelled maximum length of `CreateAliasInput$Name`
/// (`smithy_query operation=CreateAlias`: `NonBlankAndLengthConstraintString`, len
/// 1..1024), so one row filters on a name at that bound.
const MAX_NAME_LENGTH: usize = 1024;

/// The aliases `ListAliases` reports for `request`.
async fn list_aliases(gamelift: &GameLift, request: Value) -> Vec<Value> {
    let listed = gamelift.ok("ListAliases", request).await;
    listed["Aliases"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListAliases must report a list, got {listed}"))
}

/// The `AliasId`s `ListAliases` reports for `request`, in the order it reports them.
async fn listed_ids(gamelift: &GameLift, request: Value) -> Vec<String> {
    list_aliases(gamelift, request)
        .await
        .iter()
        .map(alias_id_of)
        .collect()
}

/// One alias to store before the filter rows run: the name it carries and the routing
/// strategy it is created with.
struct Stored {
    label: &'static str,
    name: String,
    routing_strategy: Value,
}

/// The aliases every filter row below is asserted against. They differ along what the
/// filters narrow over: two aliases SHARE a name ("alias names do not need to be
/// unique"), BOTH values of the modelled `RoutingStrategyType` catalog (`smithy_query
/// shape=RoutingStrategyType`: `SIMPLE`, `TERMINAL`) appear under that shared name, one
/// name is a strict extension of another (so an exact-match filter cannot behave like a
/// prefix one), and one name sits at the modelled maximum length.
fn stored_aliases() -> Vec<Stored> {
    vec![
        Stored {
            label: "a simple alias under a shared name",
            name: "live".to_string(),
            routing_strategy: simple_routing("fleet-live"),
        },
        Stored {
            label: "a terminal alias under the same shared name",
            name: "live".to_string(),
            routing_strategy: terminal_routing("This build is retired."),
        },
        Stored {
            label: "a name that extends another name",
            name: "live-extra".to_string(),
            routing_strategy: simple_routing("fleet-extra"),
        },
        Stored {
            label: "a terminal alias under its own name",
            name: "retired".to_string(),
            routing_strategy: terminal_routing("Upgrade at https://example.test/upgrade"),
        },
        Stored {
            label: "a simple alias under its own name",
            name: "staging".to_string(),
            routing_strategy: simple_routing("fleet-staging"),
        },
        Stored {
            label: "a name at the modelled maximum length",
            name: "n".repeat(MAX_NAME_LENGTH),
            routing_strategy: simple_routing("fleet-longest-name"),
        },
    ]
}

/// `ListAliases` reports the stored collection narrowed by each filter the request sets,
/// and the record it reports for an alias is the one `CreateAlias` stored and
/// `DescribeAlias` reads back.
#[tokio::test]
async fn the_list_reports_the_stored_aliases_narrowed_by_each_filter() {
    struct Case {
        label: &'static str,
        request: Value,
        /// The labels of [`stored_aliases`] rows the request must report, in no
        /// particular order ("returned aliases are not listed in any particular order").
        expected: Vec<&'static str>,
    }

    let gamelift = GameLift::new();

    // A fresh store holds no aliases, so the list is an empty collection rather than a
    // refusal or a missing member.
    assert_eq!(
        list_aliases(&gamelift, json!({})).await,
        Vec::<Value>::new(),
        "a store holding no aliases must report an empty collection"
    );

    let stored = stored_aliases();
    let mut created: Vec<Value> = Vec::new();
    for alias in &stored {
        created.push(
            create_alias(
                &gamelift,
                json!({"Name": &alias.name, "RoutingStrategy": alias.routing_strategy.clone()}),
            )
            .await,
        );
    }
    let id_of_label = |label: &str| -> String {
        let index = stored
            .iter()
            .position(|alias| alias.label == label)
            .unwrap_or_else(|| panic!("no stored alias is labelled {label:?}"));
        alias_id_of(&created[index])
    };
    let longest_name = "n".repeat(MAX_NAME_LENGTH);

    let cases = vec![
        Case {
            label: "no filter at all reports every stored alias",
            request: json!({}),
            expected: stored.iter().map(|alias| alias.label).collect(),
        },
        Case {
            label: "a name two aliases share reports both of them",
            request: json!({"Name": "live"}),
            expected: vec![
                "a simple alias under a shared name",
                "a terminal alias under the same shared name",
            ],
        },
        Case {
            label: "a name is matched whole, not as a prefix",
            request: json!({"Name": "live-extra"}),
            expected: vec!["a name that extends another name"],
        },
        Case {
            label: "a name no alias carries reports nothing",
            request: json!({"Name": "liv"}),
            expected: vec![],
        },
        Case {
            label: "a name at the modelled maximum length",
            request: json!({"Name": &longest_name}),
            expected: vec!["a name at the modelled maximum length"],
        },
        Case {
            label: "the SIMPLE routing type reports only the simple aliases",
            request: json!({"RoutingStrategyType": "SIMPLE"}),
            expected: vec![
                "a simple alias under a shared name",
                "a name that extends another name",
                "a simple alias under its own name",
                "a name at the modelled maximum length",
            ],
        },
        Case {
            label: "the TERMINAL routing type reports only the terminal aliases",
            request: json!({"RoutingStrategyType": "TERMINAL"}),
            expected: vec![
                "a terminal alias under the same shared name",
                "a terminal alias under its own name",
            ],
        },
        Case {
            label: "both filters together narrow a shared name to one routing type",
            request: json!({"Name": "live", "RoutingStrategyType": "SIMPLE"}),
            expected: vec!["a simple alias under a shared name"],
        },
        Case {
            label: "both filters together narrow it to the other routing type",
            request: json!({"Name": "live", "RoutingStrategyType": "TERMINAL"}),
            expected: vec!["a terminal alias under the same shared name"],
        },
        Case {
            label: "a stored name with a routing type it does not carry reports nothing",
            request: json!({"Name": "retired", "RoutingStrategyType": "SIMPLE"}),
            expected: vec![],
        },
    ];

    for case in &cases {
        let label = case.label;
        let mut reported = listed_ids(&gamelift, case.request.clone()).await;
        reported.sort();
        let mut expected: Vec<String> = case.expected.iter().map(|row| id_of_label(row)).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {label:?}: the list must report exactly the matching aliases, got {reported:?}"
        );
    }

    // Each reported record is the STORED one: what the create reported, member for
    // member, and what a describe of the same alias reports.
    for listed in list_aliases(&gamelift, json!({})).await {
        let alias_id = alias_id_of(&listed);
        let stored_record = created
            .iter()
            .find(|alias| alias_id_of(alias) == alias_id)
            .unwrap_or_else(|| panic!("the list reported an alias nothing created: {alias_id:?}"));
        assert_eq!(
            &listed, stored_record,
            "case {alias_id:?}: a listed alias must be the created record, got {listed}"
        );
        let described = gamelift
            .ok("DescribeAlias", json!({"AliasId": &alias_id}))
            .await;
        assert_eq!(
            described["Alias"], listed,
            "case {alias_id:?}: the list and the describe must serve the one stored record, got {}",
            described["Alias"]
        );
    }

    // Listing the aliases changed nothing: the same list, run again, reports exactly the
    // same collection.
    let first = list_aliases(&gamelift, json!({})).await;
    let second = list_aliases(&gamelift, json!({})).await;
    assert_eq!(
        first, second,
        "listing the aliases must not change what a later list reports, got {second:?}"
    );
}

/// `ListAliases` pages the collection it reports: a `Limit` bounds the page, the
/// `NextToken` it hands back resumes exactly where that page ended, and a token this
/// collection did not issue is refused.
#[tokio::test]
async fn the_list_pages_the_collection_and_refuses_a_foreign_token() {
    let gamelift = GameLift::new();
    // Nine aliases, so a page size divides the collection evenly for some rows and
    // leaves a partial last page for others. Every third one is terminal, so a filtered
    // walk pages a narrowed collection too.
    for index in 0..9 {
        let routing = if index % 3 == 0 {
            terminal_routing(&format!("row {index} is retired"))
        } else {
            simple_routing(&format!("fleet-paged-{index}"))
        };
        create_alias(
            &gamelift,
            json!({"Name": format!("paged-{index}"), "RoutingStrategy": routing}),
        )
        .await;
    }

    // The order the unpaged list reports is the order a paged walk must reproduce.
    let filters = [
        ("no filter", json!({})),
        ("the SIMPLE routing type", json!({"RoutingStrategyType": "SIMPLE"})),
        ("one name", json!({"Name": "paged-4"})),
    ];
    for (filter_label, filter) in &filters {
        let whole = listed_ids(&gamelift, filter.clone()).await;
        for page_size in [1_usize, 2, 3, 9, 50] {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut request = filter.clone();
                request["Limit"] = json!(page_size);
                if let Some(token) = &token {
                    request["NextToken"] = json!(token);
                }
                let response = gamelift.ok("ListAliases", request).await;
                let page = response["Aliases"].as_array().cloned().unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case {filter_label:?} page size {page_size}: a page must hold at most Limit \
                     aliases, got {}",
                    page.len()
                );
                walked.extend(page.iter().map(alias_id_of));
                match response["NextToken"].as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                walked, whole,
                "case {filter_label:?} page size {page_size}: the paged walk must report every \
                 matching alias once, in the order the unpaged list reports, got {walked:?}"
            );
        }
    }

    // A token minted for another collection is not an offset into this one, and neither
    // is a value this service never issued.
    gamelift
        .ok("CreateLocation", json!({"LocationName": "custom-aliases"}))
        .await;
    let other = gamelift.ok("ListLocations", json!({"Limit": 1})).await;
    let mut tokens = vec!["not-a-token-this-service-issued".to_string()];
    tokens.extend(other["NextToken"].as_str().map(str::to_string));
    for token in tokens {
        let (status, code) = gamelift
            .err("ListAliases", json!({"NextToken": token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case token {token:?}: a token this collection did not issue must be refused, got \
             {status} {code}"
        );
    }

    // `Limit` is modelled `PositiveInteger` with range min 1, so the bound is the
    // model's: one below it is refused by the boundary walk.
    for limit in [0, -1] {
        let (status, code) = gamelift.err("ListAliases", json!({"Limit": limit})).await;
        assert_eq!(
            status, 400,
            "case Limit {limit}: a Limit below the modelled minimum must be refused with 400, got \
             {status} {code}"
        );
    }

    // The refused calls stored nothing and reported nothing: the collection is the nine
    // aliases the creates made.
    assert_eq!(
        listed_ids(&gamelift, json!({})).await.len(),
        9,
        "the refused calls must leave the collection as it was"
    );
}
