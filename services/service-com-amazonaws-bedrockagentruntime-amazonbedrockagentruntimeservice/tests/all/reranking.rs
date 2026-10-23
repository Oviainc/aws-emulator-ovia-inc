// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Rerank` (`POST /rerank`).
//!
//! Every test drives real HTTP requests through the framework router over the shared
//! engine, reusing the request harness of `tests/all/sessions.rs` rather than a second
//! copy of it, so the wire protocol (restJson1), the URI bindings, the boundary
//! validator and the handler are all exercised and every assertion reads a response
//! BODY.
//!
//! THE RULE UNDER TEST: `Rerank` answers one permutation of the supplied `sources`,
//! ordered by each source document's relevance to the single query, highest first —
//! every source contributing exactly one result carrying its POSITION in the request's
//! `sources` list, a `relevanceScore` in `[0, 1]` that is a pure function of that one
//! (query, document) pair, and `numberOfResults` keeping the leading `n` of that order.
//!
//! The scored quantity is the lexical relevance of the pair (the cosine similarity of
//! their sublinear term-frequency vectors — see the module docs of
//! `src/ops/rerank.rs`), so the rows below span what that generalises over: how many
//! sources there are, where in the list the relevant one sits, whether a document is
//! `TEXT` or `JSON`, its length, its case and punctuation, its script, and how often it
//! repeats a term.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use crate::sessions::{call, error_code, router};

/// A reranker model named the way a caller usually names one: the bare model id, which
/// the modelled `BedrockModelArn` pattern admits beside a full ARN.
const MODEL: &str = "amazon.rerank-v1:0";

/// How close two scores this emulator computed for the SAME pair must be to count as
/// the same number: they are reported as the modelled `Float` (32-bit), so a comparison
/// against a value computed in `f64` here carries single-precision rounding.
const TOLERANCE: f64 = 1e-6;

/// One `TEXT` source carrying `text`.
fn text_source(text: &str) -> Value {
    json!({
        "type": "INLINE",
        "inlineDocumentSource": {
            "type": "TEXT",
            "textDocument": { "text": text },
        },
    })
}

/// One `JSON` source carrying `document`.
fn json_source(document: Value) -> Value {
    json!({
        "type": "INLINE",
        "inlineDocumentSource": {
            "type": "JSON",
            "jsonDocument": document,
        },
    })
}

/// The `Rerank` request body for one query over `sources`, with `numberOfResults`
/// omitted the way an SDK omits an absent member when `number_of_results` is `None`.
fn rerank_body(query: &str, sources: &[Value], number_of_results: Option<i64>) -> Value {
    let mut configuration = json!({ "modelConfiguration": { "modelArn": MODEL } });
    if let Some(requested) = number_of_results {
        configuration["numberOfResults"] = json!(requested);
    }
    json!({
        "queries": [{ "type": "TEXT", "textQuery": { "text": query } }],
        "sources": sources,
        "rerankingConfiguration": {
            "type": "BEDROCK_RERANKING_MODEL",
            "bedrockRerankingConfiguration": configuration,
        },
    })
}

/// Rerank `sources` against `query`.
async fn rerank(
    router: &MultiServiceRouter,
    query: &str,
    sources: &[Value],
    number_of_results: Option<i64>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/rerank",
        rerank_body(query, sources, number_of_results),
    )
    .await
}

/// Rerank `sources` against `query`, asserting the call succeeded, and return the
/// `(index, relevanceScore)` pairs in the order they were reported.
async fn ranking(
    router: &MultiServiceRouter,
    case: &str,
    query: &str,
    sources: &[Value],
    number_of_results: Option<i64>,
) -> Vec<(i64, f64)> {
    let (status, body) = rerank(router, query, sources, number_of_results).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Rerank to answer 200, got {status} {body:?}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case {case:?}: expected no nextToken, got {body:?}"
    );
    let results = body["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no results in {body:?}"));
    results
        .iter()
        .map(|result| {
            let index = result["index"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: no index in {result:?}"));
            let score = result["relevanceScore"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: no relevanceScore in {result:?}"));
            assert!(
                result.get("document").is_none(),
                "case {case:?}: expected no document echoed back, got {result:?}"
            );
            (index, score)
        })
        .collect()
}

/// Every invariant of one answer that does not depend on which documents were sent:
/// the result count, the indices being a permutation of the positions reported, the
/// scores descending, and every score inside the reported range.
fn assert_well_formed(case: &str, ranking: &[(i64, f64)], sources: usize, expected: usize) {
    assert_eq!(
        ranking.len(),
        expected,
        "case {case:?}: expected {expected} of {sources} sources ranked, got {ranking:?}"
    );
    let indices: BTreeSet<i64> = ranking.iter().map(|(index, _)| *index).collect();
    assert_eq!(
        indices.len(),
        ranking.len(),
        "case {case:?}: expected every source to be ranked at most once, got {ranking:?}"
    );
    for (index, score) in ranking {
        assert!(
            (0..sources as i64).contains(index),
            "case {case:?}: index {index} is not a position in a list of {sources} sources, \
             got {ranking:?}"
        );
        assert!(
            (0.0..=1.0).contains(score),
            "case {case:?}: relevanceScore {score} is outside [0, 1], got {ranking:?}"
        );
    }
    for pair in ranking.windows(2) {
        assert!(
            pair[0].1 >= pair[1].1,
            "case {case:?}: expected scores to descend, got {} before {} in {ranking:?}",
            pair[0].1,
            pair[1].1
        );
    }
}

/// RULE: one call ranks every supplied source exactly once, by relevance to the query,
/// highest first.
///
/// The rows differ along the source count, the document types in the list, the document
/// lengths, duplicate documents and documents sharing nothing with the query.
#[tokio::test]
async fn every_source_is_ranked_exactly_once_and_the_scores_descend() {
    let router = router();
    let cases: Vec<(&str, &str, Vec<Value>)> = vec![
        (
            "a single source",
            "settlement latency",
            vec![text_source("Settlement latency fell to two seconds.")],
        ),
        (
            "three text sources, one relevant",
            "ice core drilling",
            vec![
                text_source("The bakery opens at six."),
                text_source("Ice core drilling resumed on the plateau."),
                text_source("Train 9 is delayed."),
            ],
        ),
        (
            "text and json sources mixed",
            "quarterly revenue growth",
            vec![
                text_source("The kitchen renovation finished early."),
                json_source(json!({ "metric": "quarterly revenue growth", "value": 12 })),
                json_source(json!({ "metric": "headcount", "value": 480 })),
            ],
        ),
        (
            "two identical documents",
            "identical twins",
            vec![
                text_source("Identical twins share a genome."),
                text_source("Identical twins share a genome."),
            ],
        ),
        (
            "no source shares a term with the query",
            "zzyzx",
            vec![
                text_source("One."),
                text_source("Two."),
                text_source("Three."),
            ],
        ),
        (
            "a nested json document",
            "grafana dashboard refresh",
            vec![
                json_source(json!({
                    "panel": { "title": "grafana dashboard", "refresh": ["30s", "1m"] },
                    "enabled": true,
                })),
                json_source(json!({ "panel": { "title": "payroll export" } })),
            ],
        ),
        (
            "a long document and a short one",
            "vector search",
            vec![
                text_source("Vector search ranks nearest neighbours."),
                text_source(
                    "This document discusses databases, indexes, storage engines, \
                     replication, backups, vector search, sharding, caching, compression \
                     and networking at length.",
                ),
            ],
        ),
        (
            "sixty-four sources",
            "needle",
            (0..64)
                .map(|position| text_source(&format!("haystack straw number {position}")))
                .collect::<Vec<Value>>(),
        ),
    ];

    for (case, query, sources) in cases {
        let ranking = ranking(&router, case, query, &sources, None).await;
        assert_well_formed(case, &ranking, sources.len(), sources.len());
    }
}

/// RULE: the source whose document is most relevant to the query is ranked first,
/// wherever in the request's list it was sent.
///
/// The rows differ along where the relevant source sits, the document type that carries
/// it, document length, case and punctuation, and script.
#[tokio::test]
async fn the_most_relevant_source_is_ranked_first() {
    let router = router();
    let cases: Vec<(&str, &str, Vec<Value>, i64)> = vec![
        (
            "the relevant source in the middle",
            "What is the capital of France?",
            vec![
                text_source("Bananas are a popular tropical fruit."),
                text_source("Paris is the capital of France."),
                text_source("The capital of a company is its equity."),
            ],
            1,
        ),
        (
            "the relevant source last",
            "tropical fruit harvest",
            vec![
                text_source("Snow fell on the mountain pass."),
                text_source("Diesel engines power the ship."),
                text_source("The banana is a tropical fruit; harvest season peaks in June."),
            ],
            2,
        ),
        (
            "the relevant source first",
            "ledger reconciliation",
            vec![
                text_source("Ledger reconciliation runs nightly."),
                text_source("The office plants were watered."),
            ],
            0,
        ),
        (
            "a json document carries the relevant text",
            "quarterly revenue growth",
            vec![
                text_source("The kitchen renovation finished early."),
                json_source(json!({ "metric": "quarterly revenue growth", "value": 12 })),
            ],
            1,
        ),
        (
            "a focused document beats one that merely mentions the query",
            "vector search",
            vec![
                text_source("Vector search ranks nearest neighbours."),
                text_source(
                    "This document discusses databases, indexes, storage engines, \
                     replication, backups, vector search, sharding, caching, compression \
                     and networking at length.",
                ),
            ],
            0,
        ),
        (
            "case and punctuation do not hide a match",
            "GPU-accelerated inference!",
            vec![
                text_source("Cold storage archives tapes."),
                text_source("gpu accelerated inference, explained."),
            ],
            1,
        ),
        (
            "a non-ASCII script",
            "café au lait",
            vec![
                text_source("Tea leaves steep."),
                text_source("Le café au lait est chaud."),
            ],
            1,
        ),
    ];

    for (case, query, sources, expected_top) in cases {
        let ranking = ranking(&router, case, query, &sources, None).await;
        assert_well_formed(case, &ranking, sources.len(), sources.len());
        let (top, score) = ranking[0];
        assert_eq!(
            top, expected_top,
            "case {case:?}: expected source {expected_top} ranked first, got {ranking:?}"
        );
        assert!(
            score > 0.0,
            "case {case:?}: expected the relevant source to score above zero, got {score}"
        );
    }
}

/// RULE: a document whose terms are the query's in the same proportions is maximally
/// relevant, and one sharing no term with the query is not relevant at all — so the
/// score's two extremes are reached by the documents that earn them.
///
/// The rows differ along how the document restates the query: verbatim, reordered,
/// repeated, padded, and disjoint.
#[tokio::test]
async fn the_ends_of_the_score_range_are_the_documents_that_earn_them() {
    let router = router();
    const QUERY: &str = "alpha beta gamma";
    // (case, document text, the score the rule requires, or None for "strictly between")
    let cases: [(&str, &str, Option<f64>); 5] = [
        ("the query verbatim", "alpha beta gamma", Some(1.0)),
        ("the query reordered", "gamma alpha beta", Some(1.0)),
        (
            "every query term repeated equally",
            "alpha beta gamma alpha beta gamma",
            Some(1.0),
        ),
        ("no term in common", "delta epsilon", Some(0.0)),
        ("one query term among others", "alpha delta epsilon", None),
    ];

    for (case, document, expected) in cases {
        let ranking = ranking(&router, case, QUERY, &[text_source(document)], None).await;
        let (_, score) = ranking[0];
        match expected {
            Some(required) => assert!(
                (score - required).abs() < TOLERANCE,
                "case {case:?}: expected a relevanceScore of {required}, got {score}"
            ),
            None => assert!(
                score > 0.0 && score < 1.0,
                "case {case:?}: expected a relevanceScore strictly between 0 and 1, got {score}"
            ),
        }
    }
}

/// RULE: a (query, document) pair's score depends on that pair alone, so the same
/// document scores the same however the rest of the candidate list is composed — the
/// consistency a caller relies on when it reranks overlapping candidate lists.
///
/// The rows differ along what else is in the list: nothing, the same candidates in
/// another order, extra candidates, a duplicate of the document itself, and a list that
/// keeps only the irrelevant ones beside it.
#[tokio::test]
async fn a_pairs_score_does_not_depend_on_the_rest_of_the_candidate_list() {
    let router = router();
    const QUERY: &str = "hydrothermal vent chemistry";
    let subject = text_source("Hydrothermal vent chemistry drives these ecosystems.");
    let filler = [
        text_source("The printer is out of toner."),
        text_source("Chemistry class meets on Tuesday."),
        text_source("Vent covers were replaced last spring."),
    ];

    // (case, the list sent, the position the subject was sent at)
    let cases: Vec<(&str, Vec<Value>, usize)> = vec![
        ("the document alone", vec![subject.clone()], 0),
        (
            "with three other candidates after it",
            vec![
                subject.clone(),
                filler[0].clone(),
                filler[1].clone(),
                filler[2].clone(),
            ],
            0,
        ),
        (
            "with the same candidates before it",
            vec![
                filler[2].clone(),
                filler[1].clone(),
                filler[0].clone(),
                subject.clone(),
            ],
            3,
        ),
        (
            "beside a duplicate of itself",
            vec![subject.clone(), subject.clone()],
            0,
        ),
        (
            "with one other candidate",
            vec![filler[1].clone(), subject.clone()],
            1,
        ),
    ];

    let mut scores: Vec<(&str, f64)> = Vec::new();
    for (case, sources, position) in cases {
        let ranking = ranking(&router, case, QUERY, &sources, None).await;
        assert_well_formed(case, &ranking, sources.len(), sources.len());
        let score = ranking
            .iter()
            .find(|(index, _)| *index == position as i64)
            .unwrap_or_else(|| panic!("case {case:?}: source {position} is not in {ranking:?}"))
            .1;
        scores.push((case, score));
    }

    let (first_case, first_score) = scores[0];
    for (case, score) in &scores[1..] {
        assert!(
            (score - first_score).abs() < TOLERANCE,
            "case {case:?}: expected the same score as case {first_case:?} ({first_score}), \
             got {score}"
        );
    }
}

/// RULE: the same request is answered the same way every time — reranking reads nothing
/// that a previous call could have changed, and writes nothing a later one could see.
#[tokio::test]
async fn the_same_request_is_answered_the_same_way_every_time() {
    let router = router();
    let sources = vec![
        text_source("Contrails form when exhaust condenses."),
        json_source(json!({ "topic": "contrail formation", "altitude_m": 10_500 })),
        text_source("Cirrus clouds are made of ice crystals."),
    ];
    let body = rerank_body("how do contrails form", &sources, None);

    let (status, first) = call(&router, "POST", "/rerank", body.clone()).await;
    assert_eq!(status, 200, "the first call: expected 200, got {status} {first:?}");
    for attempt in 2..=4 {
        let (status, again) = call(&router, "POST", "/rerank", body.clone()).await;
        assert_eq!(
            (status, &again),
            (200, &first),
            "call {attempt}: expected the first answer {first:?}, got {status} {again:?}"
        );
    }
}

/// RULE: `numberOfResults` keeps the leading `n` of the ranking, and a request that
/// names none is answered with the whole of it.
///
/// The rows differ along the value: absent, one, a middle value, exactly the source
/// count, and more than the sources sent.
#[tokio::test]
async fn number_of_results_keeps_the_leading_n_of_the_ranking() {
    let router = router();
    const QUERY: &str = "monsoon onset date";
    let sources = vec![
        text_source("The monsoon onset date shifted later this year."),
        text_source("Onset of the dry season is in October."),
        text_source("A date palm needs little water."),
        text_source("The cricket match was rained off."),
    ];
    let whole = ranking(&router, "no numberOfResults", QUERY, &sources, None).await;
    assert_well_formed("no numberOfResults", &whole, sources.len(), sources.len());

    // (case, numberOfResults, how many results the rule requires)
    let cases: [(&str, i64, usize); 4] = [
        ("one result", 1, 1),
        ("two of four", 2, 2),
        ("exactly the source count", 4, 4),
        ("more than the sources sent", 10, 4),
    ];
    for (case, requested, expected) in cases {
        let ranking = ranking(&router, case, QUERY, &sources, Some(requested)).await;
        assert_well_formed(case, &ranking, sources.len(), expected);
        assert_eq!(
            ranking,
            whole[..expected],
            "case {case:?}: expected the leading {expected} of {whole:?}, got {ranking:?}"
        );
    }
}

/// RULE: a request this service cannot interpret as naming a query, a source document
/// or a configuration it serves is refused as `400 ValidationException` — never
/// answered with a ranking of something it had to guess at.
///
/// The rows differ along which member is wrong and how: a payload that does not match
/// its document `type`, a missing query text, a type outside the model's enum, and a
/// pagination token no call issues.
#[tokio::test]
async fn a_request_this_service_cannot_interpret_is_refused() {
    let router = router();
    let good_source = text_source("Anything at all.");

    // (case, the body sent)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a TEXT document with no textDocument",
            rerank_body(
                "query",
                &[json!({
                    "type": "INLINE",
                    "inlineDocumentSource": { "type": "TEXT" },
                })],
                None,
            ),
        ),
        (
            "a TEXT document carrying only a jsonDocument",
            rerank_body(
                "query",
                &[json!({
                    "type": "INLINE",
                    "inlineDocumentSource": {
                        "type": "TEXT",
                        "jsonDocument": { "text": "over here" },
                    },
                })],
                None,
            ),
        ),
        (
            "a JSON document with no jsonDocument",
            rerank_body(
                "query",
                &[json!({
                    "type": "INLINE",
                    "inlineDocumentSource": { "type": "JSON" },
                })],
                None,
            ),
        ),
        (
            "the second of two sources is the broken one",
            rerank_body(
                "query",
                &[
                    good_source.clone(),
                    json!({
                        "type": "INLINE",
                        "inlineDocumentSource": { "type": "JSON" },
                    }),
                ],
                None,
            ),
        ),
        (
            "a document type outside the model's enum",
            rerank_body(
                "query",
                &[json!({
                    "type": "INLINE",
                    "inlineDocumentSource": {
                        "type": "PDF",
                        "textDocument": { "text": "over here" },
                    },
                })],
                None,
            ),
        ),
        (
            "a source type outside the model's enum",
            rerank_body(
                "query",
                &[json!({
                    "type": "S3",
                    "inlineDocumentSource": {
                        "type": "TEXT",
                        "textDocument": { "text": "over here" },
                    },
                })],
                None,
            ),
        ),
        (
            "a TEXT query with no text",
            json!({
                "queries": [{ "type": "TEXT", "textQuery": {} }],
                "sources": [good_source.clone()],
                "rerankingConfiguration": {
                    "type": "BEDROCK_RERANKING_MODEL",
                    "bedrockRerankingConfiguration": {
                        "modelConfiguration": { "modelArn": MODEL },
                    },
                },
            }),
        ),
        (
            "a query content type outside the model's enum",
            json!({
                "queries": [{ "type": "IMAGE", "textQuery": { "text": "query" } }],
                "sources": [good_source.clone()],
                "rerankingConfiguration": {
                    "type": "BEDROCK_RERANKING_MODEL",
                    "bedrockRerankingConfiguration": {
                        "modelConfiguration": { "modelArn": MODEL },
                    },
                },
            }),
        ),
        (
            "a reranking configuration type outside the model's enum",
            json!({
                "queries": [{ "type": "TEXT", "textQuery": { "text": "query" } }],
                "sources": [good_source.clone()],
                "rerankingConfiguration": {
                    "type": "COHERE_RERANKING_MODEL",
                    "bedrockRerankingConfiguration": {
                        "modelConfiguration": { "modelArn": MODEL },
                    },
                },
            }),
        ),
        (
            "a pagination token no call issues",
            {
                let mut body = rerank_body("query", std::slice::from_ref(&good_source), None);
                body["nextToken"] = json!("cGFnZS0y");
                body
            },
        ),
    ];

    for (case, body) in cases {
        let (status, refused) = call(&router, "POST", "/rerank", body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
    }
}

/// A deterministic byte stream for the generated cases below: SHA-256 over
/// `"<seed>:<counter>"`, a block at a time.
///
/// The generator needs no entropy and nothing on disk — `sha2` is already this crate's
/// dev-dependency — and the seed is quoted in every assertion message below, so a
/// failing case is reproducible from the failure alone.
struct Stream {
    seed: u64,
    counter: u64,
    block: [u8; 32],
    used: usize,
}

impl Stream {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: 0,
            block: [0; 32],
            used: 32,
        }
    }

    fn byte(&mut self) -> u8 {
        if self.used == self.block.len() {
            self.counter += 1;
            self.block
                .copy_from_slice(&Sha256::digest(format!("{}:{}", self.seed, self.counter)));
            self.used = 0;
        }
        self.used += 1;
        self.block[self.used - 1]
    }

    /// A value in `0..bound`. The modulo's bias is irrelevant to a test generator: the
    /// cases only have to span the space, not sample it uniformly.
    fn below(&mut self, bound: usize) -> usize {
        let drawn = u32::from_be_bytes([self.byte(), self.byte(), self.byte(), self.byte()]);
        (drawn as usize) % bound.max(1)
    }
}

/// PROPERTY: over generated candidate lists, every answer is a permutation of the
/// sources ranked by a descending score in `[0, 1]`, and re-scoring any one of those
/// documents on its own reports the same score it had in the list.
///
/// 48 bounded cases, each a fresh query and candidate list drawn from a small shared
/// vocabulary so that overlaps — the thing being ranked — actually occur.
#[tokio::test]
async fn the_ranking_invariants_hold_over_generated_candidate_lists() {
    /// The seed every generated case below is derived from. Quoted in each assertion
    /// message, so a failure names the case that produced it.
    const SEED: u64 = 0x5265_7261_6e6b_3031;
    /// Bounded case count: enough to span the space, small enough to stay fast.
    const CASES: usize = 48;
    /// The words cases are built from, so a generated query and document can share
    /// terms. Deliberately short: a wide vocabulary would make every overlap empty.
    const VOCABULARY: [&str; 12] = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        "lambda", "mu",
    ];

    let router = router();
    let mut stream = Stream::new(SEED);
    for case in 0..CASES {
        let query: Vec<&str> = (0..1 + stream.below(4))
            .map(|_| VOCABULARY[stream.below(VOCABULARY.len())])
            .collect();
        let query = query.join(" ");
        let sources: Vec<Value> = (0..1 + stream.below(8))
            .map(|_| {
                let words: Vec<&str> = (0..1 + stream.below(10))
                    .map(|_| VOCABULARY[stream.below(VOCABULARY.len())])
                    .collect();
                text_source(&words.join(" "))
            })
            .collect();

        let label = format!("seed {SEED:#x} case {case} query {query:?}");
        let ranked = ranking(&router, &label, &query, &sources, None).await;
        assert_well_formed(&label, &ranked, sources.len(), sources.len());

        // Pair locality: one of the ranked documents, scored on its own, keeps its
        // score.
        let picked = stream.below(sources.len());
        let in_list = ranked
            .iter()
            .find(|(index, _)| *index == picked as i64)
            .unwrap_or_else(|| panic!("{label}: source {picked} is not in {ranked:?}"))
            .1;
        let alone = ranking(
            &router,
            &label,
            &query,
            &[sources[picked].clone()],
            None,
        )
        .await[0]
            .1;
        assert!(
            (alone - in_list).abs() < TOLERANCE,
            "{label}: source {picked} scored {in_list} in the list of {} and {alone} alone",
            sources.len()
        );
    }
}
