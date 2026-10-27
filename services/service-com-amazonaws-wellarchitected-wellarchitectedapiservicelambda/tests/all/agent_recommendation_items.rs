// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAgentRecommendationItems`: the listing that "lists
//! recommendation items for a specific recommendation", each item representing "an Amazon
//! Web Services resource or recommendation affected by the optimization recommendation".
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. The items of a recommendation are DERIVED from what the recommendation itself
//!    reports: one `AWS_RESOURCE` item per Amazon Web Services service its `awsServices`
//!    names, and one `RECOMMENDATION` item per other recommendation of the SAME generation
//!    whose pillar its `crossPillarBenefits` or `tradeOffs` name. Every expectation here is
//!    built from `GetAgentRecommendation` and `ListAgentRecommendations`, so the suite pins
//!    the relation between the operations rather than reading the content back out of the
//!    code under test.
//! 2. An item is a SNAPSHOT taken when the recommendation was generated: it carries the
//!    recommendation's own `createdBy` and `createdAt` and no `lastModifiedBy` /
//!    `lastModifiedAt`, moving the recommendation's status leaves it exactly where it was,
//!    and a LATER generation over the same pillars adds nothing to it.
//! 3. `type` is a filter: a listing that names one reports exactly the items of that type,
//!    the two published types partition the whole listing, and a listing that names none
//!    reports every item.
//! 4. A listing pages over every item exactly once in a stable order, for every page size,
//!    filtered or not; `maxResults` is held to the range its member publishes.
//! 5. A request the service cannot serve changes nothing: a `recommendationArn` that names
//!    no recommendation is `404 ResourceNotFoundException` carrying the ARN as sent and the
//!    missing resource's type, a `nextToken` this service did not issue is `400
//!    ValidationException` with reason `CANNOT_PARSE` naming the member, and a `type` or
//!    ARN the model refuses never reaches the handler.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the profile and the
//! generation whose completion produces the recommendations come from the suites beside
//! this one.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{absent_profile_arn, profile_arn_of, PILLARS};
use super::agent_recommendations::{
    recommendations_of, start_generation, text, until_complete, update_status,
    RECOMMENDATION_TYPES,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `RecommendationItemType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationItemType")`). Spelled out here so the
/// suite pins the published set rather than reading it back out of the code under test.
const ITEM_TYPES: [&str; 2] = ["AWS_RESOURCE", "RECOMMENDATION"];

/// The published bounds of `ListAgentRecommendationItemsRequest$maxResults`
/// (`@range(min: 1, max: 1000)` — `smithy_query(query="operation",
/// operation="ListAgentRecommendationItems")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// The members of a finding that relate it to another pillar's recommendation, in the order
/// an item's `relatedThrough` reports them.
const RELATING_MEMBERS: [&str; 2] = ["crossPillarBenefits", "tradeOffs"];

/// `GET /api/v1/agent-recommendations/{recommendationArn}/items`, with `query` carrying the
/// filter and paging members the case exercises (already percent-encoded, empty for a
/// request that names none). The ARN is a non-greedy `@httpLabel`, so it travels
/// percent-encoded in one path segment, exactly as an SDK sends it.
async fn list_items(client: &Client, recommendation_arn: &str, query: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-recommendations/{}/items{query}",
                encode_path_segment(recommendation_arn)
            ),
            None,
        )
        .await
}

/// `GET /api/v1/agent-recommendations/{recommendationArn}` — the single read every
/// expectation in this suite is derived from.
async fn get_recommendation(client: &Client, recommendation_arn: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-recommendations/{}",
                encode_path_segment(recommendation_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {recommendation_arn:?}: expected GetAgentRecommendation 200, got {answer:?}"
    );
    answer.body
}

/// The `items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListAgentRecommendationItems 200, got {answer:?}"
    );
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: items is modelled @required, got {answer:?}"))
        .clone()
}

/// Every item of one recommendation, in the listing's own order, taken in one page.
async fn items_of(client: &Client, recommendation_arn: &str, case: &str) -> Vec<Value> {
    let answer = list_items(client, recommendation_arn, "?maxResults=1000").await;
    items(&answer, case)
}

/// One string list of a read, or an empty list when it carries none.
fn strings(body: &Value, name: &str) -> Vec<String> {
    body.get(name)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The pillars one list of related findings names (`crossPillarBenefits`, `tradeOffs`).
fn related_pillars(body: &Value, name: &str) -> Vec<String> {
    body.get(name)
        .and_then(Value::as_array)
        .map(|entries| entries.iter().map(|entry| text(entry, "pillar")).collect())
        .unwrap_or_default()
}

/// The id-safe form of a published name, as an item id carries it: lower-cased, with every
/// run of characters an id does not carry collapsed to one `-`. Reimplemented here so the
/// suite states the rule rather than calling the code under test.
fn id_part(name: &str) -> String {
    let mut id = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.extend(character.to_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_string()
}

/// One item as this suite compares it: its id, its type and its metadata as sorted pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Projection {
    id: String,
    item_type: String,
    metadata: Vec<(String, Value)>,
}

/// The projection of one item the service reported.
fn reported(item: &Value) -> Projection {
    let metadata = item
        .get("metadata")
        .and_then(Value::as_object)
        .map(|members| {
            members
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<BTreeMap<String, Value>>()
        })
        .unwrap_or_default();
    Projection {
        id: text(item, "id"),
        item_type: text(item, "type"),
        metadata: metadata.into_iter().collect(),
    }
}

/// The projections the items of one recommendation MUST have, derived from the single read
/// of that recommendation and from the recommendations its generation produced beside it
/// (`pillar -> (arn, title)`).
fn expected(detail: &Value, siblings: &BTreeMap<String, (String, String)>) -> Vec<Projection> {
    let pillar = text(detail, "pillar");
    let mut expected: Vec<Projection> = strings(detail, "awsServices")
        .iter()
        .map(|service| Projection {
            id: format!("aws-resource/{}", id_part(service)),
            item_type: ITEM_TYPES[0].to_string(),
            metadata: vec![
                ("pillar".to_string(), json!(pillar)),
                ("service".to_string(), json!(service)),
            ],
        })
        .collect();

    let benefits = related_pillars(detail, "crossPillarBenefits");
    let trade_offs = related_pillars(detail, "tradeOffs");
    for (sibling_pillar, (arn, title)) in siblings {
        if *sibling_pillar == pillar {
            continue;
        }
        let through: Vec<String> = RELATING_MEMBERS
            .iter()
            .filter(|member| match **member {
                "crossPillarBenefits" => benefits.contains(sibling_pillar),
                _ => trade_offs.contains(sibling_pillar),
            })
            .map(|member| (*member).to_string())
            .collect();
        if through.is_empty() {
            continue;
        }
        expected.push(Projection {
            id: format!("recommendation/{}", id_part(sibling_pillar)),
            item_type: ITEM_TYPES[1].to_string(),
            metadata: vec![
                ("pillar".to_string(), json!(sibling_pillar)),
                ("recommendationArn".to_string(), json!(arn)),
                ("relatedThrough".to_string(), json!(through)),
                ("title".to_string(), json!(title)),
            ],
        });
    }

    expected.sort_by(|left, right| left.id.cmp(&right.id));
    expected
}

/// The recommendations one profile holds, as `pillar -> (arn, title)`.
async fn by_pillar(client: &Client, profile_arn: &str, case: &str) -> BTreeMap<String, (String, String)> {
    recommendations_of(client, profile_arn, case)
        .await
        .iter()
        .map(|item| {
            (
                text(item, "pillar"),
                (
                    text(item, "recommendationArn"),
                    text(item, "title"),
                ),
            )
        })
        .collect()
}

/// A profile whose one generation over `pillars` has completed.
async fn generated(client: &Client, name: &str, pillars: &[&str], recommendation_type: &str) -> String {
    let profile_arn = profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, pillars, recommendation_type).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;
    profile_arn
}

/// One row of the table: which pillars a generation scopes, and which kind of
/// recommendation it was started for.
struct Row {
    case: &'static str,
    pillars: Vec<&'static str>,
    recommendation_type: &'static str,
}

/// The rows: a scope of one pillar (no sibling to relate to at all), of two whose findings
/// relate to each other, of three, of a pillar no other row scopes, and of every published
/// pillar — the modelled `Scope$pillars` `@length(1, 5)` bounds among them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-pillar",
            pillars: vec!["SECURITY"],
            recommendation_type: RECOMMENDATION_TYPES[0],
        },
        Row {
            case: "two-related-pillars",
            pillars: vec!["COST_OPTIMIZATION", "RESILIENCE"],
            recommendation_type: RECOMMENDATION_TYPES[1],
        },
        Row {
            case: "three-pillars",
            pillars: vec!["PERFORMANCE", "SECURITY", "RESILIENCE"],
            recommendation_type: RECOMMENDATION_TYPES[2],
        },
        Row {
            case: "a-pillar-no-other-row-scopes",
            pillars: vec!["OPERATIONAL_EXCELLENCE"],
            recommendation_type: RECOMMENDATION_TYPES[0],
        },
        Row {
            case: "every-published-pillar",
            pillars: PILLARS.to_vec(),
            recommendation_type: RECOMMENDATION_TYPES[1],
        },
    ]
}

/// Rule 1 and 2: the items of a recommendation are the services its finding applies to and
/// the recommendations of the same generation it relates to, each carrying the
/// recommendation's own identity and instant.
#[tokio::test]
async fn items_are_the_resources_and_recommendations_the_finding_affects() {
    let client = Client::new();

    // Each row gets its own profile, so a listing reports exactly what that row's
    // generation produced; every generation is started before the one sleep.
    let mut started: Vec<(Row, String)> = Vec::new();
    for row in rows() {
        let profile_arn =
            profile_arn_of(&client, &format!("items-{}", row.case), 1).await;
        let answer =
            start_generation(&client, &profile_arn, &row.pillars, row.recommendation_type).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}",
            row.case
        );
        started.push((row, profile_arn));
    }
    tokio::time::sleep(until_complete()).await;

    // Across the rows: the same pillar scoped in two generations publishes the same
    // AWS_RESOURCE items, because those belong to the finding rather than to the request.
    let mut resource_items_by_pillar: BTreeMap<String, Vec<Projection>> = BTreeMap::new();

    for (row, profile_arn) in &started {
        let siblings = by_pillar(&client, profile_arn, row.case).await;
        assert_eq!(
            siblings.len(),
            row.pillars.len(),
            "case {:?}: a completed generation produces one recommendation per scoped \
             pillar ({:?}), got {siblings:?}",
            row.case,
            row.pillars
        );

        for (pillar, (arn, _)) in &siblings {
            let case = format!("{}/{pillar}", row.case);
            let detail = get_recommendation(&client, arn).await;
            let listed = items_of(&client, arn, &case).await;

            let reported_projections: Vec<Projection> = {
                let mut projections: Vec<Projection> = listed.iter().map(reported).collect();
                projections.sort_by(|left, right| left.id.cmp(&right.id));
                projections
            };
            assert_eq!(
                reported_projections,
                expected(&detail, &siblings),
                "case {case:?}: the items must be the services the finding applies to and \
                 the recommendations of the same generation it relates to, got {listed:?}"
            );

            // Every item is reported in id order, and no id is served twice.
            let ids: Vec<String> = listed.iter().map(|item| text(item, "id")).collect();
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                ids, sorted,
                "case {case:?}: the items must be enumerated in id order, each once, got \
                 {ids:?}"
            );

            for item in &listed {
                assert_eq!(
                    text(item, "recommendationArn"),
                    *arn,
                    "case {case:?}: an item names the recommendation it belongs to, got \
                     {item:?}"
                );
                assert!(
                    ITEM_TYPES.contains(&text(item, "type").as_str()),
                    "case {case:?}: type must be a published RecommendationItemType, got \
                     {item:?}"
                );
                assert_eq!(
                    item.get("createdBy"),
                    detail.get("createdBy"),
                    "case {case:?}: a snapshot carries the recommendation's createdBy, got \
                     {item:?}"
                );
                assert_eq!(
                    item.get("createdAt"),
                    detail.get("createdAt"),
                    "case {case:?}: a snapshot is taken when the recommendation was \
                     generated, got {item:?}"
                );
                assert!(
                    item.get("lastModifiedBy").is_none_or(Value::is_null)
                        && item.get("lastModifiedAt").is_none_or(Value::is_null),
                    "case {case:?}: nothing modifies an item after the generation that took \
                     it, got {item:?}"
                );
            }

            let resources: Vec<Projection> = listed
                .iter()
                .map(reported)
                .filter(|projection| projection.item_type == ITEM_TYPES[0])
                .collect();
            assert!(
                !resources.is_empty(),
                "case {case:?}: a finding applies to at least one service, so it has at \
                 least one AWS_RESOURCE item, got {listed:?}"
            );
            match resource_items_by_pillar.get(pillar) {
                Some(seen) => assert_eq!(
                    resources, *seen,
                    "case {case:?}: the AWS_RESOURCE items belong to the pillar's finding, \
                     so they are the same wherever that pillar is scoped, got {resources:?}"
                ),
                None => {
                    resource_items_by_pillar.insert(pillar.clone(), resources);
                }
            }

            // A generation that scoped every pillar produced a recommendation for every
            // pillar this finding relates to, so each of its recommendations affects at
            // least one other: the RECOMMENDATION half of the listing is not vacuous.
            if row.pillars.len() == PILLARS.len() {
                assert!(
                    listed
                        .iter()
                        .any(|item| text(item, "type") == ITEM_TYPES[1]),
                    "case {case:?}: a finding relates to another pillar the generation \
                     scoped, so it affects that pillar's recommendation, got {listed:?}"
                );
            }
        }

        // Rule 1's other half: a generation scoping ONE pillar produced no other
        // recommendation, so its finding relates to none of this analysis's results.
        if row.pillars.len() == 1 {
            let (arn, _) = siblings.values().next().expect("one recommendation");
            let listed = items_of(&client, arn, row.case).await;
            assert!(
                listed
                    .iter()
                    .all(|item| text(item, "type") == ITEM_TYPES[0]),
                "case {:?}: a generation that produced one recommendation leaves it \
                 relating to no other, got {listed:?}",
                row.case
            );
        }
    }
}

/// Rule 2: a listing is a REPORT. Moving the recommendation's status leaves its items
/// exactly where they were, and a LATER generation over the same pillars adds none.
#[tokio::test]
async fn items_are_a_snapshot_taken_at_generation() {
    let client = Client::new();
    let profile_arn = generated(
        &client,
        "item-snapshots",
        &PILLARS,
        RECOMMENDATION_TYPES[0],
    )
    .await;
    let recommendations = by_pillar(&client, &profile_arn, "item-snapshots").await;

    let mut before: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (pillar, (arn, _)) in &recommendations {
        before.insert(pillar.clone(), items_of(&client, arn, pillar).await);
    }

    // Repeating the listing reports the same items: a read changes nothing.
    for (pillar, (arn, _)) in &recommendations {
        let again = items_of(&client, arn, pillar).await;
        assert_eq!(
            again, before[pillar],
            "case {pillar:?}: repeating the listing must report the same items, got \
             {again:?}"
        );
    }

    // Each published status, and a reason with one of them: the status lifecycle belongs to
    // the recommendation, not to the snapshot its items are.
    for (index, (pillar, (arn, _))) in recommendations.iter().enumerate() {
        let body = match index % 2 {
            0 => json!({ "status": "COMPLETED", "updateReason": "done in this quarter" }),
            _ => json!({ "status": "SUPPRESSED" }),
        };
        let updated = update_status(&client, arn, body).await;
        assert_eq!(
            updated.status, 200,
            "case {pillar:?}: expected UpdateAgentRecommendationStatus 200, got {updated:?}"
        );
    }
    for (pillar, (arn, _)) in &recommendations {
        let after = items_of(&client, arn, pillar).await;
        assert_eq!(
            after, before[pillar],
            "case {pillar:?}: moving a recommendation's status must leave its items where \
             they were, got {after:?}"
        );
    }

    // A SECOND generation over the same pillars produces its own recommendations; the
    // items of the first generation's are a snapshot of that generation, so they do not
    // grow.
    let started = start_generation(&client, &profile_arn, &PILLARS, RECOMMENDATION_TYPES[1]).await;
    assert_eq!(
        started.status, 200,
        "case \"a-second-generation\": expected StartAgentRecommendationGeneration 200, got \
         {started:?}"
    );
    tokio::time::sleep(until_complete()).await;
    let now_held = recommendations_of(&client, &profile_arn, "a-second-generation").await;
    assert_eq!(
        now_held.len(),
        PILLARS.len() * 2,
        "case \"a-second-generation\": two generations over every pillar produce two \
         recommendations per pillar, got {now_held:?}"
    );
    for (pillar, (arn, _)) in &recommendations {
        let after = items_of(&client, arn, pillar).await;
        assert_eq!(
            after, before[pillar],
            "case {pillar:?}: a later generation's recommendations are no items of an \
             earlier one, got {after:?}"
        );
    }
}

/// Rule 3: the `type` filter selects exactly the items of that type, and the published
/// types partition the listing.
#[tokio::test]
async fn the_type_filter_selects_only_items_of_that_type() {
    let client = Client::new();
    let profile_arn =
        generated(&client, "item-type-filter", &PILLARS, RECOMMENDATION_TYPES[2]).await;
    let recommendations = by_pillar(&client, &profile_arn, "item-type-filter").await;

    for (pillar, (arn, _)) in &recommendations {
        let all = items_of(&client, arn, pillar).await;
        let mut partitioned: Vec<Value> = Vec::new();
        for item_type in ITEM_TYPES {
            let case = format!("{pillar}/{item_type}");
            let answer =
                list_items(&client, arn, &format!("?maxResults=1000&type={item_type}")).await;
            let filtered = items(&answer, &case);
            let selected: Vec<Value> = all
                .iter()
                .filter(|item| text(item, "type") == item_type)
                .cloned()
                .collect();
            assert_eq!(
                filtered, selected,
                "case {case:?}: the filter selects exactly the items of that type, got \
                 {filtered:?}"
            );
            partitioned.extend(filtered);
        }
        assert_eq!(
            partitioned.len(),
            all.len(),
            "case {pillar:?}: the two published types partition the listing, got \
             {partitioned:?} of {all:?}"
        );
    }

    // A value the enum does not publish names no item type at all, so it is refused rather
    // than read as "no filter".
    let (arn, _) = recommendations
        .values()
        .next()
        .expect("a recommendation under test");
    let answer = list_items(&client, arn, "?type=AWS_ACCOUNT").await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case \"an unpublished item type\": expected a 400 ValidationException, got \
         {answer:?}"
    );
}

/// Rule 4: a listing pages over every item exactly once, for every page size, filtered or
/// not — and `maxResults` is held to its published range.
#[tokio::test]
async fn a_listing_pages_over_every_item_exactly_once() {
    let client = Client::new();
    let profile_arn = generated(&client, "item-paging", &PILLARS, RECOMMENDATION_TYPES[0]).await;
    let recommendations = by_pillar(&client, &profile_arn, "item-paging").await;
    let (arn, _) = recommendations
        .values()
        .next()
        .expect("a recommendation under test");
    let all = items_of(&client, arn, "every-item").await;
    assert!(
        all.len() > 1,
        "case \"every-item\": the recommendation must hold more than one item for a page to \
         be cut, got {all:?}"
    );

    // The rows differ along the page size and along whether a filter narrows the walk, so
    // the paging is pinned over the filtered enumeration too.
    let filters: [(&str, &str); 3] = [
        ("unfiltered", ""),
        ("aws-resource", "&type=AWS_RESOURCE"),
        ("recommendation", "&type=RECOMMENDATION"),
    ];
    for (filter_case, filter) in filters {
        let wanted: Vec<Value> = match filter_case {
            "unfiltered" => all.clone(),
            "aws-resource" => all
                .iter()
                .filter(|item| text(item, "type") == ITEM_TYPES[0])
                .cloned()
                .collect(),
            _ => all
                .iter()
                .filter(|item| text(item, "type") == ITEM_TYPES[1])
                .cloned()
                .collect(),
        };
        assert!(
            !wanted.is_empty(),
            "case {filter_case:?}: the walk must have items to page over, got {all:?}"
        );
        for page_size in [1_usize, 2, 3, 1_000] {
            let case = format!("{filter_case}/maxResults={page_size}");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let query = match &token {
                    Some(token) => {
                        format!("?maxResults={page_size}{filter}&nextToken={token}")
                    }
                    None => format!("?maxResults={page_size}{filter}"),
                };
                let answer = list_items(&client, arn, &query).await;
                let page = items(&answer, &case);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page must never exceed maxResults, got {page:?}"
                );
                walked.extend(page);
                token = answer
                    .body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert!(
                    walked.len() <= wanted.len(),
                    "case {case:?}: a cursor must not re-serve an item, got {walked:?}"
                );
            }
            assert_eq!(
                walked, wanted,
                "case {case:?}: the pages must cover every selected item exactly once, in \
                 the listing's order, got {walked:?}"
            );
        }
    }

    // Each published bound serves the page it asks for; one past either bound is refused at
    // the boundary.
    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let answer = list_items(&client, arn, &format!("?maxResults={max_results}")).await;
        let served = items(&answer, &format!("maxResults={max_results}")).len();
        let ceiling = usize::try_from(max_results).expect("a positive published bound");
        assert_eq!(
            served,
            ceiling.min(all.len()),
            "case maxResults={max_results}: expected {} of the {} items, got {served}",
            ceiling.min(all.len()),
            all.len()
        );
    }
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = list_items(&client, arn, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults={max_results}: a value past a published bound must be refused, \
             got {answer:?}"
        );
    }
}

/// Rule 5: a request the service cannot serve is refused, and changes nothing.
#[tokio::test]
async fn a_request_the_service_cannot_serve_changes_nothing() {
    let client = Client::new();
    let profile_arn = generated(&client, "item-refusals", &PILLARS, RECOMMENDATION_TYPES[0]).await;
    let recommendations = by_pillar(&client, &profile_arn, "item-refusals").await;
    let (arn, _) = recommendations
        .values()
        .next()
        .expect("a recommendation under test");
    let before = items_of(&client, arn, "before-the-refusals").await;

    // A well-formed `AgentRecommendationArn` no recommendation carries.
    let absent_arn = format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/\
         abcdefab-0000-4a00-8b00-00000000001d",
        engine::emulator_account_id()
    );
    let answer = list_items(&client, &absent_arn, "").await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"an absent recommendation\": expected a 404 ResourceNotFoundException, got \
         {answer:?}"
    );
    assert_eq!(
        answer.body.get("ResourceId").and_then(Value::as_str),
        Some(absent_arn.as_str()),
        "case \"an absent recommendation\": the 404 names the ARN as sent, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("ResourceType").and_then(Value::as_str),
        Some(state::AGENT_RECOMMENDATION_RESOURCE_TYPE),
        "case \"an absent recommendation\": the 404 names the missing resource's type, got \
         {answer:?}"
    );

    // An agent profile's ARN is not a recommendation's: the modelled pattern refuses it
    // before any lookup.
    let refusals: Vec<(&str, Answer)> = vec![
        (
            "a profile ARN in the recommendation's place",
            list_items(&client, &absent_profile_arn("not-a-recommendation"), "").await,
        ),
        (
            "an item type outside the published enum",
            list_items(&client, arn, "?type=RESOURCE").await,
        ),
    ];
    for (case, answer) in refusals {
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {answer:?}"
        );
    }

    // Every row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary hands it to the handler: what is under test is the handler's own refusal. The
    // cursor of ANOTHER paginated operation of this service is such a token, collected from
    // a real page rather than composed by hand.
    let sibling_page = client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/recommendations?maxResults=1",
                encode_path_segment(&profile_arn)
            ),
            None,
        )
        .await;
    let sibling_cursor = sibling_page
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"a-recommendation-cursor\": expected a cursor in {sibling_page:?}")
        })
        .to_string();
    let tokens: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        ("an-arn-like-token", "arn-like_token=".to_string()),
        ("a-recommendation-cursor", sibling_cursor),
    ];
    for (case, token) in tokens {
        let answer = list_items(&client, arn, &format!("?nextToken={token}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a token this service did not issue must be refused, got \
             {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: expected reason CANNOT_PARSE, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .and_then(|fields| fields.first())
                .and_then(|field| field.get("Name"))
                .and_then(Value::as_str),
            Some("NextToken"),
            "case {case:?}: the finding must name the NextToken member, got {answer:?}"
        );
    }

    // The refusals changed nothing: the listing answers exactly what it did before them.
    let after = items_of(&client, arn, "after-the-refusals").await;
    assert_eq!(
        after, before,
        "case \"after-the-refusals\": a refused request must leave the listing reporting \
         what it did before, got {after:?}"
    );
}
