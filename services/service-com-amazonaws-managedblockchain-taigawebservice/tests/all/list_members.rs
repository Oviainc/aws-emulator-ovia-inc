// SPDX-License-Identifier: CC0-1.0

//! `ListMembers`, end to end over HTTP: the page it answers with is cut from the members
//! stored on one network, every summary member comes from that record, each input filter
//! selects on the stored value it names, and a `NetworkId` that names no network is the
//! modelled 404 rather than an empty page.
//!
//! The tables below span the classes the rules generalise over: the member
//! configurations a Fabric network is created with, every value `MemberStatus` declares,
//! both owning-account cases `IsOwned` distinguishes, page sizes from one to past the
//! item count plus the published `MaxResults` bounds and one past each, and the id forms
//! that name no stored network.
//!
//! Only `CreateNetwork` mints a member today (`CreateMember` is still a scaffold), so the
//! rows that need SEVERAL members on one network write them into this service's own store
//! — the same store the handler reads — and then assert over the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    ComAmazonawsManagedblockchainTaigawebserviceState as ServiceState, StoredMember,
    MAX_MEMBERS_PER_NETWORK, MEMBER_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::network_and_node_lifecycle::{
    call, create_network, create_network_body, is_date_time, is_resource_id, member_configuration,
    router_over, Answer, ACCOUNT, PAST_TRANSITION, REGION,
};

/// An account that is not the one under emulation, so a member whose ARN names it is
/// owned by "a different Amazon Web Services account" (`MemberSummary$IsOwned`).
pub(crate) const OTHER_ACCOUNT: &str = "111122223333";

/// Long enough that a `CREATING` member seeded with this much left to run has not come
/// due by the end of a test.
const FAR_FUTURE: Duration = Duration::from_secs(3_600);

/// A router and the engine it is mounted over, so a case can seed the store the handler
/// reads.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// One `ListMembers` call: `query` is the already-encoded query string (empty for none).
async fn list_members(router: &MultiServiceRouter, network_id: &str, query: &str) -> Answer {
    let path = if query.is_empty() {
        format!("/networks/{network_id}/members")
    } else {
        format!("/networks/{network_id}/members?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The `Id` of every member the answer reports, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["Members"]
        .as_array()
        .map(|members| {
            members
                .iter()
                .map(|member| member["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A `CreateNetwork` body whose `MemberConfiguration` is `member`.
fn network_body_with_member(token: &str, network_name: &str, member: Value) -> Value {
    let mut body = create_network_body(token, network_name);
    body["MemberConfiguration"] = member;
    body
}

/// One seeded member: what this row varies from the member `CreateNetwork` stored.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seed {
    pub(crate) name: &'static str,
    /// A `MemberStatus` value.
    pub(crate) status: &'static str,
    /// The account the member's ARN names; `IsOwned` is read off it.
    pub(crate) account: &'static str,
}

/// Write `seeds` into the store as members of `network_id`, and return their ids in the
/// order the store holds them.
///
/// Each seeded record is a copy of the one `CreateNetwork` stored, with only the row's
/// name, status and owning account changed — so a case tests the handler, not a
/// hand-built record that no create could have produced.
pub(crate) fn seed_members(
    engine: &engine::InMemoryEngine,
    network_id: &str,
    seeds: &[Seed],
) -> Vec<String> {
    let state = engine.state::<ServiceState>();
    let mut guard = state.lock();
    let network = guard
        .networks
        .get_mut(network_id)
        .expect("the created network is stored");
    let template = network
        .members
        .values()
        .next()
        .expect("CreateNetwork stored its first member")
        .clone();
    let now = SystemTime::now();
    let mut ids = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        // Zero-padded, so the id ordering the store pages on is the row ordering, and
        // within `ResourceIdString`'s `@length(1, 32)`.
        let id = format!("m-SEED{index:022}");
        let member = StoredMember {
            id: id.clone(),
            network_id: network_id.to_string(),
            name: seed.name.to_string(),
            status: seed.status.to_string(),
            // A member the clock still owes `AVAILABLE` keeps its `CREATING` status for
            // the whole test; every other status is one `settle` never moves.
            available_at: now + FAR_FUTURE,
            arn: format!("arn:aws:managedblockchain:{REGION}:{}:members/{id}", seed.account),
            ..template.clone()
        };
        network.members.insert(id.clone(), member);
        ids.push(id);
    }
    ids
}

/// `ListMembers` reports the member `CreateNetwork` persisted, with every summary member
/// taken from that record — the same record `GetMember` answers from — and repeating the
/// read changes nothing.
#[tokio::test]
async fn list_members_answers_from_the_stored_member() {
    // Rows differ along every member of `MemberConfiguration` the summary is projected
    // from: the name's form, whether a description was given (and the empty string
    // `DescriptionString`'s `@length(0, 128)` allows), and the tag count.
    let cases: &[(&str, Option<&str>, usize)] = &[
        ("org1", None, 0),
        ("Org-Two", Some("the second org"), 1),
        ("org-three-with-a-longer-name", Some(""), 3),
    ];

    for (name, description, tag_count) in cases {
        let router = crate::network_and_node_lifecycle::new_router();
        let mut configuration = member_configuration(name);
        if let Some(description) = description {
            configuration["Description"] = json!(description);
        }
        if *tag_count > 0 {
            let tags: serde_json::Map<String, Value> = (0..*tag_count)
                .map(|i| (format!("key{i}"), json!(format!("value{i}"))))
                .collect();
            configuration["Tags"] = Value::Object(tags);
        }
        let (network_id, member_id) = create_network(
            &router,
            network_body_with_member(&format!("list-{name}"), "listing-net", configuration),
        )
        .await;

        let answer = list_members(&router, &network_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected ListMembers 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer),
            vec![member_id.clone()],
            "case {name}: expected exactly the created member, got {:?}",
            answer.body
        );

        let summary = answer.body["Members"][0].clone();
        assert!(
            is_resource_id(&member_id, "m"),
            "case {name}: expected a member-shaped id, got {member_id:?}"
        );
        assert_eq!(
            summary["Name"].as_str(),
            Some(*name),
            "case {name}: expected the stored member name, got {summary:?}"
        );
        match description {
            Some(text) => assert_eq!(
                summary["Description"].as_str(),
                Some(*text),
                "case {name}: expected the stored description, got {summary:?}"
            ),
            None => assert!(
                summary["Description"].is_null(),
                "case {name}: expected no Description, got {summary:?}"
            ),
        }
        // Created under 500 ms ago, so the transition to `AVAILABLE` is not yet due.
        assert_eq!(
            summary["Status"].as_str(),
            Some("CREATING"),
            "case {name}: expected the freshly created member's status, got {summary:?}"
        );
        assert_eq!(
            summary["IsOwned"].as_bool(),
            Some(true),
            "case {name}: expected a member of this account to be owned, got {summary:?}"
        );
        assert_eq!(
            summary["Arn"].as_str(),
            Some(
                format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:members/{member_id}")
                    .as_str()
            ),
            "case {name}: expected the stored member ARN, got {summary:?}"
        );
        assert!(
            is_date_time(&summary["CreationDate"]),
            "case {name}: expected an ISO-8601 CreationDate, got {summary:?}"
        );
        let mut keys: Vec<String> = summary
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        // The members `MemberSummary` declares (smithy_query: shape MemberSummary), minus
        // the optional one this row left unset.
        let mut expected: Vec<String> = ["Arn", "CreationDate", "Id", "IsOwned", "Name", "Status"]
            .iter()
            .map(|key| (*key).to_string())
            .collect();
        if description.is_some() {
            expected.push("Description".to_string());
        }
        expected.sort();
        assert_eq!(
            keys, expected,
            "case {name}: expected exactly the MemberSummary members the model declares, got {summary:?}"
        );

        // Reading twice answers the same thing: a listing reports, it does not change
        // what a later caller sees.
        let again = list_members(&router, &network_id, "").await;
        assert_eq!(
            again.body, answer.body,
            "case {name}: expected a repeated ListMembers to answer identically, got {:?}",
            again.body
        );

        // Once the clock owes the transition, both readers report it from the one record.
        tokio::time::sleep(PAST_TRANSITION).await;
        let answer = list_members(&router, &network_id, "").await;
        let summary = answer.body["Members"][0].clone();
        assert_eq!(
            summary["Status"].as_str(),
            Some("AVAILABLE"),
            "case {name}: expected the settled status, got {summary:?}"
        );

        let member = call(
            &router,
            "GET",
            &format!("/networks/{network_id}/members/{member_id}"),
            None,
        )
        .await;
        for field in ["Id", "Name", "Description", "Status", "CreationDate", "Arn"] {
            assert_eq!(
                summary[field], member.body["Member"][field],
                "case {name}: expected ListMembers' {field} to match GetMember's, got {summary:?} and {:?}",
                member.body
            );
        }
    }
}

/// Each filter selects on the stored value of the member it names, and an omitted filter
/// selects every member.
#[tokio::test]
async fn each_filter_selects_on_the_stored_value() {
    let (router, engine) = router_and_engine();
    let (network_id, created) =
        create_network(&router, create_network_body("filters-1", "filter-net")).await;

    // Rows: every value `MemberStatus` declares (smithy_query: shape MemberStatus =
    // CREATING | AVAILABLE | CREATE_FAILED | UPDATING | DELETING | DELETED |
    // INACCESSIBLE_ENCRYPTION_KEY), plus one member owned by another account.
    let seeds = &[
        Seed { name: "seed-creating", status: "CREATING", account: ACCOUNT },
        Seed { name: "seed-available", status: "AVAILABLE", account: ACCOUNT },
        Seed { name: "seed-create-failed", status: "CREATE_FAILED", account: ACCOUNT },
        Seed { name: "seed-updating", status: "UPDATING", account: ACCOUNT },
        Seed { name: "seed-deleting", status: "DELETING", account: ACCOUNT },
        Seed { name: "seed-deleted", status: "DELETED", account: ACCOUNT },
        Seed {
            name: "seed-no-key",
            status: "INACCESSIBLE_ENCRYPTION_KEY",
            account: ACCOUNT,
        },
        Seed { name: "seed-foreign", status: "AVAILABLE", account: OTHER_ACCOUNT },
    ];
    let seeded = seed_members(&engine, &network_id, seeds);

    // The member `CreateNetwork` minted has settled by now, so it is `AVAILABLE` too.
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut everything: Vec<String> = seeded.clone();
    everything.push(created.clone());
    everything.sort();

    let answer = list_members(&router, &network_id, &format!("maxResults={MEMBER_MAX_RESULTS}"))
        .await;
    let mut listed = listed_ids(&answer);
    listed.sort();
    assert_eq!(
        listed, everything,
        "case no filter: expected every stored member, got {:?}",
        answer.body
    );

    // `Name`: "The optional name of the member to list".
    for (index, seed) in seeds.iter().enumerate() {
        let answer = list_members(&router, &network_id, &format!("name={}", seed.name)).await;
        assert_eq!(
            listed_ids(&answer),
            vec![seeded[index].clone()],
            "case name={}: expected only the member of that name, got {:?}",
            seed.name,
            answer.body
        );
    }
    let answer = list_members(&router, &network_id, "name=no-such-member").await;
    assert!(
        listed_ids(&answer).is_empty() && answer.status == 200,
        "case name=no-such-member: expected an empty page, got {} {:?}",
        answer.status,
        answer.body
    );

    // `Status`: "only members currently in this status are listed".
    for seed in seeds {
        let answer = list_members(&router, &network_id, &format!("status={}", seed.status)).await;
        let mut expected: Vec<String> = seeds
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.status == seed.status)
            .map(|(index, _)| seeded[index].clone())
            .collect();
        if seed.status == "AVAILABLE" {
            expected.push(created.clone());
        }
        expected.sort();
        let mut listed = listed_ids(&answer);
        listed.sort();
        assert_eq!(
            listed, expected,
            "case status={}: expected only the members in that status, got {:?}",
            seed.status, answer.body
        );
    }

    // `IsOwned`: limited "either to members that the current Amazon Web Services account
    // owns (true) or that other Amazon Web Services accounts own (false)".
    let answer = list_members(&router, &network_id, "isOwned=false").await;
    let foreign = seeded[seeds.len() - 1].clone();
    assert_eq!(
        listed_ids(&answer),
        vec![foreign.clone()],
        "case isOwned=false: expected only the member another account owns, got {:?}",
        answer.body
    );
    let answer = list_members(
        &router,
        &network_id,
        &format!("isOwned=true&maxResults={MEMBER_MAX_RESULTS}"),
    )
    .await;
    let mut listed = listed_ids(&answer);
    listed.sort();
    let expected: Vec<String> = everything
        .iter()
        .filter(|id| **id != foreign)
        .cloned()
        .collect();
    assert_eq!(
        listed, expected,
        "case isOwned=true: expected every member this account owns, got {:?}",
        answer.body
    );
    for member in answer.body["Members"].as_array().unwrap_or(&Vec::new()) {
        assert_eq!(
            member["IsOwned"].as_bool(),
            Some(true),
            "case isOwned=true: expected every reported member to be owned, got {member:?}"
        );
    }

    // Two filters together select the intersection.
    let answer = list_members(&router, &network_id, "status=AVAILABLE&isOwned=false").await;
    assert_eq!(
        listed_ids(&answer),
        vec![foreign],
        "case status=AVAILABLE&isOwned=false: expected the intersection, got {:?}",
        answer.body
    );
    let answer = list_members(&router, &network_id, "status=DELETED&isOwned=false").await;
    assert!(
        listed_ids(&answer).is_empty(),
        "case status=DELETED&isOwned=false: expected an empty page, got {:?}",
        answer.body
    );
}

/// The listing pages by `MaxResults` through `NextToken`: walking the pages reports every
/// member exactly once, and the last page carries no token.
#[tokio::test]
async fn list_members_pages_through_every_member() {
    let (router, engine) = router_and_engine();
    let (network_id, created) =
        create_network(&router, create_network_body("pages-1", "paging-net")).await;
    let seeds: Vec<Seed> = (0..6)
        .map(|_| Seed {
            name: "paged",
            status: "AVAILABLE",
            account: ACCOUNT,
        })
        .collect();
    let seeded = seed_members(&engine, &network_id, &seeds);
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut everything: Vec<String> = seeded;
    everything.push(created);
    everything.sort();

    // Rows: a page per member, sizes either side of the item count, the published
    // `MaxResults` bounds (`@range(1, 20)`) and no `MaxResults` at all.
    let cases: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(everything.len() as i32 - 1),
        Some(everything.len() as i32),
        Some(everything.len() as i32 + 1),
        Some(MEMBER_MAX_RESULTS),
        None,
    ];

    for max_results in cases {
        let case = format!("maxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match max_results {
                Some(limit) => format!("maxResults={limit}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_members(&router, &network_id, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let ids = listed_ids(&answer);
            let limit = max_results.unwrap_or(MEMBER_MAX_RESULTS) as usize;
            assert!(
                ids.len() <= limit && !ids.is_empty(),
                "case {case} page {pages}: expected 1..={limit} members, got {:?}",
                answer.body
            );
            seen.extend(ids);
            next_token = answer.body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= everything.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case}: expected no member on two pages, got {seen:?}"
        );
        assert_eq!(
            sorted, everything,
            "case {case}: expected every stored member across the pages, got {seen:?}"
        );
    }

    // A token this service never issued cannot resume a page.
    let answer = list_members(&router, &network_id, "nextToken=not-a-token").await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "InvalidRequestException"),
        "case a forged nextToken: expected 400 InvalidRequestException, got {} {:?}",
        answer.status,
        answer.body
    );

    // One past each published `MaxResults` bound is refused at the boundary, before the
    // handler runs. Only the status is asserted: which shape the boundary validator
    // refuses with is not this operation's to choose.
    for out_of_range in [0, MEMBER_MAX_RESULTS + 1] {
        let answer =
            list_members(&router, &network_id, &format!("maxResults={out_of_range}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={out_of_range}: expected a refusal, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// A `NetworkId` that names no stored network is the modelled `ResourceNotFoundException`
/// with HTTP 404, never an empty page.
#[tokio::test]
async fn a_network_id_that_names_nothing_stored_is_a_404() {
    let router = crate::network_and_node_lifecycle::new_router();
    let (network_id, _) =
        create_network(&router, create_network_body("missing-1", "present-net")).await;

    // Rows: the id forms a caller can pass that name no network — a well-shaped one, the
    // shortest `ResourceIdString`'s `@length(1, 32)` allows, and one that is not
    // member-shaped at all.
    let cases: &[&str] = &[
        "n-NOSUCHNETWORKNOSUCHNETWOR",
        "x",
        "n-doesnotexist00000000000000000",
    ];

    for unknown in cases {
        let answer = list_members(&router, unknown, "").await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {unknown}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.text(&["Message"]).contains(unknown),
            "case {unknown}: expected a Message naming the resource that was not found, got {:?}",
            answer.body
        );
        // `ResourceNotFoundException$ResourceName` is `ArnString`
        // (`@pattern ^arn:.+:.+:.+:.+:.+$`), and no ARN exists for a network that does
        // not: the member is omitted rather than filled with a bare id.
        assert!(
            answer.body["ResourceName"].is_null(),
            "case {unknown}: expected no ResourceName, got {:?}",
            answer.body
        );
    }

    // And the refusals changed nothing: the network that does exist still lists.
    let answer = list_members(&router, &network_id, "").await;
    assert_eq!(
        (answer.status, listed_ids(&answer).len()),
        (200, 1),
        "case the stored network: expected its one member, got {} {:?}",
        answer.status,
        answer.body
    );
}

/// The member map of a network has a ceiling: the one path that adds a member refuses
/// once the network holds `MAX_MEMBERS_PER_NETWORK` of them, so a caller looping on
/// member creates cannot grow the store without bound. Replacing an id the network
/// already holds is not growth, so it stays allowed.
#[tokio::test]
async fn the_member_map_refuses_growth_past_its_ceiling() {
    let (router, engine) = router_and_engine();
    let (network_id, _) =
        create_network(&router, create_network_body("ceiling-1", "ceiling-net")).await;

    let state = engine.state::<ServiceState>();
    let mut guard = state.lock();
    let network = guard
        .networks
        .get_mut(&network_id)
        .expect("the created network is stored");
    let template = network
        .members
        .values()
        .next()
        .expect("CreateNetwork stored its first member")
        .clone();

    let mut index = 0usize;
    while network.members.len() < MAX_MEMBERS_PER_NETWORK {
        let member = StoredMember {
            id: format!("m-FILL{index:022}"),
            ..template.clone()
        };
        assert!(
            network.insert_member(member).is_ok(),
            "case fill {index}: expected an insert below the ceiling to be accepted, map holds {}",
            network.members.len()
        );
        index += 1;
    }

    let overflow = StoredMember {
        id: format!("m-FILL{index:022}"),
        ..template.clone()
    };
    assert!(
        network.insert_member(overflow).is_err(),
        "case one past the ceiling: expected the insert to be refused, map holds {}",
        network.members.len()
    );
    assert_eq!(
        network.members.len(),
        MAX_MEMBERS_PER_NETWORK,
        "case at the ceiling: expected the map to stay at its ceiling, got {}",
        network.members.len()
    );

    let replacement = StoredMember {
        name: "renamed".to_string(),
        ..template.clone()
    };
    assert!(
        network.insert_member(replacement).is_ok(),
        "case replacing a member already held: expected it to be accepted, map holds {}",
        network.members.len()
    );
    assert_eq!(
        network.members.get(&template.id).map(|held| held.name.clone()),
        Some("renamed".to_string()),
        "case replacing a member already held: expected the stored record to be replaced"
    );
}
