// SPDX-License-Identifier: CC0-1.0

//! One rule: `Rescore` scores a SCORING UNIT, and submitted documents sharing a
//! `GroupId` are one unit.
//!
//! `Document$GroupId` (`length 1..2048`, from
//! `smithy_query(query="shape",shape="Document")`) is documented as "the optional group
//! identifier of the document from the search service. Documents with the same group
//! identifier are grouped together and processed as one document within the service", and
//! `RescoreResult$ResultItems` as "a list of result items for documents with new relevancy
//! scores. The results are in descending order." Together: the text of the documents
//! sharing a group identifier is pooled into ONE scored document, so every member of a
//! group reports that one score and the group holds ONE position in the descending
//! ordering, while every submitted document still gets its own result item.
//!
//! The table varies what the rule generalises over: group size, how many groups a request
//! carries, whether grouped and ungrouped documents are mixed, where in the request a
//! group's members sit, which text form (`Title`/`TokenizedTitle`, `Body`/`TokenizedBody`)
//! carries the query's terms, the members' `OriginalScore`s, and the group identifier's
//! own value (its case and its length, at both published bounds).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    create_active_plan, emulator, rescore, rescore_items as result_items, SCORE_MAX, SCORE_MIN,
    SCORE_TOLERANCE,
};

/// One row of the grouping table: a query and the documents submitted under it, some of
/// which carry a `GroupId`.
struct GroupCase {
    /// What this row varies.
    case: &'static str,
    query: &'static str,
    documents: Value,
}

fn group_cases() -> Vec<GroupCase> {
    // A group identifier at each published bound of `GroupId`'s length (1..2048).
    let shortest_group = "g".to_string();
    let longest_group = "z".repeat(2048);
    vec![
        GroupCase {
            case: "a group whose second member carries none of the query's terms",
            query: "electric car batteries",
            documents: json!([
                { "Id": "chunk-1", "GroupId": "doc-a", "Title": "Electric vehicle batteries",
                  "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.2 },
                { "Id": "chunk-2", "GroupId": "doc-a", "Title": "Appendix",
                  "Body": "Table of contents.", "OriginalScore": 0.2 },
                { "Id": "solo", "Title": "Unrelated", "Body": "Gardening tips.", "OriginalScore": 0.2 },
            ]),
        },
        GroupCase {
            case: "a group of three, each member carrying one term of the query",
            query: "alpha beta gamma",
            documents: json!([
                { "Id": "m-1", "GroupId": "trio", "Body": "alpha", "OriginalScore": 0.0 },
                { "Id": "m-2", "GroupId": "trio", "Body": "beta", "OriginalScore": 0.0 },
                { "Id": "m-3", "GroupId": "trio", "Body": "gamma", "OriginalScore": 0.0 },
                { "Id": "whole", "Body": "alpha beta gamma", "OriginalScore": 0.0 },
            ]),
        },
        GroupCase {
            case: "two groups and an ungrouped document, members submitted non-adjacently",
            query: "cloud search",
            documents: json!([
                { "Id": "a-1", "GroupId": "left", "Title": "cloud", "Body": "notes", "OriginalScore": 0.1 },
                { "Id": "b-1", "GroupId": "right", "Body": "unrelated prose", "OriginalScore": 0.4 },
                { "Id": "loner", "Body": "cloud search tuning", "OriginalScore": 0.0 },
                { "Id": "a-2", "GroupId": "left", "Body": "search tuning", "OriginalScore": 0.3 },
                { "Id": "b-2", "GroupId": "right", "Title": "index", "Body": "more prose", "OriginalScore": 0.2 },
            ]),
        },
        GroupCase {
            case: "every submitted document belongs to one group",
            query: "single unit",
            documents: json!([
                { "Id": "one", "GroupId": "all", "Title": "single", "Body": "a note", "OriginalScore": -0.5 },
                { "Id": "two", "GroupId": "all", "Body": "unit", "OriginalScore": 0.9 },
            ]),
        },
        GroupCase {
            case: "the pre-tokenized forms of a group's text pool like the text forms",
            query: "lithium ion",
            documents: json!([
                { "Id": "tok-title", "GroupId": "mixed", "TokenizedTitle": ["Lithium"],
                  "Body": "a note", "OriginalScore": 0.25 },
                { "Id": "tok-body", "GroupId": "mixed", "TokenizedBody": ["ION", "cells"],
                  "OriginalScore": 0.25 },
                { "Id": "plain", "Body": "a note", "OriginalScore": 0.25 },
            ]),
        },
        GroupCase {
            case: "no document carries a group identifier, so each is its own unit",
            query: "nothing grouped",
            documents: json!([
                { "Id": "p", "Body": "nothing", "OriginalScore": 0.1 },
                { "Id": "q", "Body": "grouped", "OriginalScore": 0.1 },
            ]),
        },
        GroupCase {
            case: "group identifiers differing only in case are different groups",
            query: "case sensitive grouping",
            documents: json!([
                { "Id": "lower", "GroupId": "g", "Body": "case sensitive", "OriginalScore": 0.0 },
                { "Id": "upper", "GroupId": "G", "Body": "grouping", "OriginalScore": 0.0 },
            ]),
        },
        GroupCase {
            case: "group identifiers at both published length bounds",
            query: "bounded identifiers",
            documents: json!([
                { "Id": "short-1", "GroupId": shortest_group, "Body": "bounded", "OriginalScore": 0.0 },
                { "Id": "short-2", "GroupId": shortest_group, "Body": "identifiers", "OriginalScore": 0.0 },
                { "Id": "long-1", "GroupId": longest_group, "Body": "bounded", "OriginalScore": 0.0 },
                { "Id": "long-2", "GroupId": longest_group, "Body": "prose", "OriginalScore": 0.0 },
            ]),
        },
        GroupCase {
            case: "a group whose members carry original scores at both published bounds",
            query: "extremes",
            documents: json!([
                { "Id": "hi", "GroupId": "extreme", "Body": "a note", "OriginalScore": 100000.0 },
                { "Id": "lo", "GroupId": "extreme", "Body": "extremes", "OriginalScore": -100000.0 },
                { "Id": "mid", "Body": "extremes", "OriginalScore": 0.0 },
            ]),
        },
    ]
}

#[tokio::test]
async fn a_group_of_documents_is_scored_and_ranked_as_one_document() {
    let router = emulator();
    let plan = create_active_plan(&router, "grouping-plan").await;

    for case in group_cases() {
        let submitted = case
            .documents
            .as_array()
            .expect("the case's documents")
            .clone();
        let (status, answer) = rescore(&router, &plan, case.query, &case.documents).await;
        assert_eq!(status, 200, "case {:?}: got {answer}", case.case);
        let items = result_items(&answer);

        // Every submitted document still gets its own result item.
        let returned: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_submitted: Vec<String> = submitted
            .iter()
            .map(|doc| doc["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        sorted_submitted.sort();
        assert_eq!(
            sorted_returned, sorted_submitted,
            "case {:?}: one result item per submitted document, got {items:?}",
            case.case
        );

        // Descending order, with every score inside the range the model gives `Score`.
        for window in items.windows(2) {
            assert!(
                window[0].1 >= window[1].1,
                "case {:?}: the results must be in descending order, got {items:?}",
                case.case
            );
        }
        for (id, score) in &items {
            assert!(
                (SCORE_MIN..=SCORE_MAX).contains(score),
                "case {:?}: the score of {id:?} must stay inside the modelled range, \
                 got {score}",
                case.case
            );
        }

        // One score per group, and one position: a group's members carry the same score
        // and sit next to each other in the ordering.
        let position: BTreeMap<&str, usize> = returned
            .iter()
            .enumerate()
            .map(|(at, id)| (id.as_str(), at))
            .collect();
        let mut members_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for doc in &submitted {
            if let Some(group) = doc["GroupId"].as_str() {
                members_of
                    .entry(group)
                    .or_default()
                    .push(doc["Id"].as_str().unwrap_or_default());
            }
        }
        for (group, members) in &members_of {
            let mut scored: Vec<(usize, f64)> = members
                .iter()
                .map(|id| {
                    let at = position[*id];
                    (at, items[at].1)
                })
                .collect();
            scored.sort_by_key(|(at, _)| *at);
            let first = scored[0].1;
            for (at, score) in &scored {
                assert!(
                    (score - first).abs() < SCORE_TOLERANCE,
                    "case {:?}: every member of group {group:?} must carry the group's \
                     one score {first}, got {score} at position {at} in {items:?}",
                    case.case
                );
            }
            let spanned = scored[scored.len() - 1].0 - scored[0].0 + 1;
            assert_eq!(
                spanned,
                scored.len(),
                "case {:?}: group {group:?} must hold ONE position in the ordering, but \
                 its {} members span {spanned} positions in {items:?}",
                case.case,
                scored.len()
            );
        }

        // The same request answers the same ranking: the units are rebuilt from the
        // request, never from anything a previous call left behind.
        let (status, again) = rescore(&router, &plan, case.query, &case.documents).await;
        assert_eq!(status, 200, "case {:?} repeated: got {again}", case.case);
        assert_eq!(
            result_items(&again),
            items,
            "case {:?}: the ranking must be deterministic",
            case.case
        );
    }
}

/// One row of the pooling table: the members of a single group.
struct PoolCase {
    /// What this row varies.
    case: &'static str,
    query: &'static str,
    members: Value,
}

/// The ONE document a group is processed as: its text is the members' text pooled (in
/// the pre-tokenized form, whose terms are exactly those of the text form), and its
/// original score the highest its members carry.
fn pooled_document(members: &[Value]) -> Value {
    let mut titles: Vec<String> = Vec::new();
    let mut bodies: Vec<String> = Vec::new();
    let mut original_score = f64::NEG_INFINITY;
    for member in members {
        for (field, sink) in [("Title", &mut titles), ("Body", &mut bodies)] {
            if let Some(text) = member[field].as_str() {
                sink.push(text.to_string());
            }
            if let Some(tokens) = member[format!("Tokenized{field}")].as_array() {
                sink.extend(
                    tokens
                        .iter()
                        .map(|token| token.as_str().unwrap_or_default().to_string()),
                );
            }
        }
        original_score = original_score.max(member["OriginalScore"].as_f64().unwrap_or_default());
    }
    let mut pooled = json!({ "Id": "pooled", "OriginalScore": original_score });
    if !titles.is_empty() {
        pooled["TokenizedTitle"] = json!(titles);
    }
    pooled["TokenizedBody"] = json!(bodies);
    pooled
}

#[tokio::test]
async fn a_group_scores_as_the_one_document_its_members_pool_into() {
    let router = emulator();
    let plan = create_active_plan(&router, "pooling-plan").await;

    // The rows vary group size, which field carries the query's terms, the text vs.
    // pre-tokenized form, and the members' original scores.
    let cases = vec![
        PoolCase {
            case: "two members, the query's terms split across their bodies",
            query: "lithium ion batteries",
            members: json!([
                { "Id": "c-1", "Body": "Lithium ion cells.", "OriginalScore": 0.2 },
                { "Id": "c-2", "Body": "Batteries for cars.", "OriginalScore": 0.2 },
            ]),
        },
        PoolCase {
            case: "two members, one of which carries nothing of the query",
            query: "electric car batteries",
            members: json!([
                { "Id": "c-1", "Title": "Electric vehicle batteries",
                  "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.2 },
                { "Id": "c-2", "Title": "Appendix", "Body": "Table of contents.", "OriginalScore": 0.2 },
            ]),
        },
        PoolCase {
            case: "three members, the query's terms split across titles and bodies",
            query: "alpha beta gamma",
            members: json!([
                { "Id": "c-1", "Title": "alpha", "Body": "a note", "OriginalScore": -0.4 },
                { "Id": "c-2", "Body": "beta", "OriginalScore": 0.6 },
                { "Id": "c-3", "TokenizedTitle": ["Gamma"], "TokenizedBody": ["more", "prose"],
                  "OriginalScore": 0.1 },
            ]),
        },
        PoolCase {
            case: "a group of one, which pools to the document itself",
            query: "lonely",
            members: json!([{ "Id": "c-1", "Title": "lonely", "Body": "a note", "OriginalScore": 0.5 }]),
        },
        PoolCase {
            case: "members whose original scores sit at both published bounds",
            query: "extremes",
            members: json!([
                { "Id": "c-1", "Body": "extremes", "OriginalScore": 100000.0 },
                { "Id": "c-2", "Body": "a note", "OriginalScore": -100000.0 },
            ]),
        },
    ];

    for case in cases {
        let members = case.members.as_array().expect("the case's members").clone();
        let grouped: Vec<Value> = members
            .iter()
            .map(|member| {
                let mut doc = member.clone();
                doc["GroupId"] = json!("one-group");
                doc
            })
            .collect();
        let (status, answer) = rescore(&router, &plan, case.query, &json!(grouped)).await;
        assert_eq!(status, 200, "case {:?}: got {answer}", case.case);
        let group_items = result_items(&answer);
        assert_eq!(
            group_items.len(),
            members.len(),
            "case {:?}: one result item per submitted document, got {group_items:?}",
            case.case
        );
        let group_score = group_items[0].1;

        let pooled = pooled_document(&members);
        let (status, answer) = rescore(&router, &plan, case.query, &json!([pooled.clone()])).await;
        assert_eq!(
            status, 200,
            "case {:?} as one document: got {answer}",
            case.case
        );
        let pooled_score = result_items(&answer)[0].1;

        assert!(
            (group_score - pooled_score).abs() < SCORE_TOLERANCE,
            "case {:?}: a group must score as the one document {pooled} it is processed \
             as, expected {pooled_score}, got {group_score}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_group_identifier_changes_the_ranking_it_is_carried_in() {
    let router = emulator();
    let plan = create_active_plan(&router, "grouping-effect-plan").await;

    // The same documents, once grouped and once not: a member whose own text carries
    // none of the query must be scored differently when its sibling's text joins it, and
    // the two must then be ranked as one.
    let chunks = json!([
        { "Id": "chunk-1", "Title": "Electric vehicle batteries",
          "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.2 },
        { "Id": "chunk-2", "Title": "Appendix", "Body": "Table of contents.", "OriginalScore": 0.2 },
        { "Id": "other", "Title": "Unrelated", "Body": "Gardening tips.", "OriginalScore": 0.2 },
    ]);
    let query = "electric car batteries";

    let (status, ungrouped) = rescore(&router, &plan, query, &chunks).await;
    assert_eq!(status, 200, "ungrouped: got {ungrouped}");
    let ungrouped: BTreeMap<String, f64> = result_items(&ungrouped).into_iter().collect();

    let grouped_documents: Vec<Value> = chunks
        .as_array()
        .expect("the chunks")
        .iter()
        .map(|doc| {
            let mut doc = doc.clone();
            if doc["Id"].as_str().unwrap_or_default().starts_with("chunk-") {
                doc["GroupId"] = json!("doc-a");
            }
            doc
        })
        .collect();
    let (status, answer) = rescore(&router, &plan, query, &json!(grouped_documents)).await;
    assert_eq!(status, 200, "grouped: got {answer}");
    let items = result_items(&answer);
    let grouped: BTreeMap<String, f64> = items.clone().into_iter().collect();

    assert!(
        (grouped["chunk-2"] - ungrouped["chunk-2"]).abs() > SCORE_TOLERANCE,
        "a grouped document must be scored with its group's text, not alone: ungrouped \
         {:?} vs grouped {:?} in {items:?}",
        ungrouped["chunk-2"],
        grouped["chunk-2"]
    );
    assert!(
        (grouped["chunk-1"] - grouped["chunk-2"]).abs() < SCORE_TOLERANCE,
        "the group's members must share one score, got {items:?}"
    );
    assert!(
        (grouped["other"] - ungrouped["other"]).abs() < SCORE_TOLERANCE,
        "a document outside every group must score as it does ungrouped, got {items:?}"
    );
    assert_eq!(
        items.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["chunk-1", "chunk-2", "other"],
        "the group holds one position, ahead of the document carrying none of the query"
    );
}
