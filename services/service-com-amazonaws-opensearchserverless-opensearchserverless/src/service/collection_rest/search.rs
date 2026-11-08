// SPDX-License-Identifier: CC0-1.0

//! The READS of an index over the collection's own REST endpoint: the search, the count,
//! and the refresh a client runs before either.
//!
//! Every hit is a document the index holds and every score is computed from that
//! document's own stored members — the terms a text field carries, or the distance between
//! a stored vector and the one the query carries — so a search reports what was written
//! and nothing else. The query vocabulary served is bounded and stated in [`selector`]; a
//! query outside it is refused the way the engine refuses a query it has no parser for,
//! rather than answered with hits that were not selected.

use super::*;

/// The `size` a search answers when the request names none, which is the engine's own
/// default page of hits.
const DEFAULT_SEARCH_SIZE: i64 = 10;

/// The furthest hit a search may page to (`index.max_result_window`'s default), and so the
/// most hits one request can build an answer out of.
const MAX_RESULT_WINDOW: i64 = 10_000;

/// The deepest a `bool` query may nest, which is what bounds the recursion of
/// [`selector`] over a caller-written query.
///
/// EMULATOR RESOURCE GUARD: the nesting is the caller's to choose, and a Rust stack
/// overflow aborts the process rather than failing one request.
const MAX_QUERY_DEPTH: usize = 20;

/// The most values one `terms` or `ids` clause may name, and the most dimensions a `knn`
/// query vector may carry — each a caller-chosen factor in the cost of evaluating one
/// clause against every document.
const MAX_CLAUSE_VALUES: usize = 1_024;

/// The most leaf clauses one query body may carry, counted across a `bool`'s BREADTH as
/// well as its depth — the engine's own `indices.query.bool.max_clause_count`.
const MAX_QUERY_CLAUSES: usize = 1_024;

/// The most value comparisons one search or count may perform: the documents it scans
/// times the values it compares in each of them.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit, and the bound that matters here: each factor
/// being bounded on its own ([`MAX_QUERY_CLAUSES`], [`MAX_CLAUSE_VALUES`],
/// [`state::MAX_DOCUMENTS_PER_INDEX`], [`state::MAX_DOCUMENT_SOURCE_NODES`]) still leaves
/// their product to the caller, so the product is what is bounded. It is the same standard
/// this crate's own [`state::MAX_RESOURCE_FILTER_WORK`] holds a caller-written filter to.
///
/// It is spent TWICE over, so the bound does not rest on a cost model being accurate:
/// [`refused_for_work`] refuses a query whose QUERY-side factors alone are already past it
/// before any document is read, and [`Budget`] is then spent down by the comparisons the
/// scan actually performs — including the ones whose count the STORED document decides
/// (how many values a field holds, how many tokens a text carries) — and aborts the scan
/// the moment it runs out. Ten million scalar comparisons is past any real query (a
/// 1 536-dimension k-NN search over a full index is about 1.5 million) and is tens of
/// milliseconds of work, which is what keeps the one critical section this plane takes
/// short.
const MAX_SEARCH_WORK: usize = 10_000_000;

/// The engine's error type for a query carrying more clauses than it will evaluate.
const TOO_MANY_CLAUSES: &str = "too_many_clauses";

/// What one search or count may still spend on comparing values, counted DOWN as the scan
/// performs them. See [`MAX_SEARCH_WORK`] for why the budget is spent during the scan and
/// not only estimated before it.
struct Budget {
    remaining: usize,
}

impl Budget {
    /// Spend `units` comparisons — at least one, so every clause costs something.
    /// `false` once the budget is exhausted, which aborts the scan rather than letting one
    /// request keep comparing.
    fn spend(&mut self, units: usize) -> bool {
        self.remaining = self.remaining.saturating_sub(units.max(1));
        self.remaining > 0
    }
}

/// The refusal for a query whose evaluation is past what one request may perform.
fn work_refusal(scanned: usize) -> RtResponse {
    error(
        400,
        TOO_MANY_CLAUSES,
        &format!(
            "too_many_clauses: evaluating this query over the [{scanned}] documents of the index \
             would compare more than the [{MAX_SEARCH_WORK}] values compared for one request"
        ),
    )
}

/// The refusal for a query whose QUERY-side factors alone already exceed the budget,
/// answered before a document is read; `None` when the scan may start.
fn refused_for_work(index: &state::IndexRecord, selector: &Selector) -> Option<RtResponse> {
    let work = selector
        .clause_cost()
        .saturating_mul(index.documents.len());
    (work > MAX_SEARCH_WORK).then(|| work_refusal(index.documents.len()))
}

/// `GET|POST /{index}/_search`, and the collection-wide form that names no index — the
/// hits the query selects, in score order.
pub(super) fn search(
    collection: &CollectionRecord,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    if !matches!(request.method, "GET" | "POST") {
        return wrong_method(request, "GET, POST");
    }
    let body = match request.json() {
        Ok(body) => body.unwrap_or_else(|| json!({})),
        Err(refusal) => return *refusal,
    };
    if let Some(refusal) = only_evaluated_members(&body, "search", EVALUATED_SEARCH_MEMBERS) {
        return *refusal;
    }
    let targets = match targets(collection, index_name) {
        Ok(targets) => targets,
        Err(refusal) => return *refusal,
    };
    let size = numbered(&body, request, "size", DEFAULT_SEARCH_SIZE);
    let from = numbered(&body, request, "from", 0);
    if size < 0 || from < 0 {
        let (name, found) = if size < 0 { ("size", size) } else { ("from", from) };
        return error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("[{name}] parameter cannot be negative, found [{found}]"),
        );
    }
    if from.saturating_add(size) > MAX_RESULT_WINDOW {
        return error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "Result window is too large, from + size must be less than or equal to: \
                 [{MAX_RESULT_WINDOW}] but was [{}]",
                from.saturating_add(size)
            ),
        );
    }
    let (mut selected, neighbours) = match evaluated(&targets, &body) {
        Ok(evaluated) => evaluated,
        Err(refusal) => return *refusal,
    };
    if let Some(k) = neighbours {
        selected.truncate(k);
    }
    let total = selected.len();
    let max_score = selected.first().map(|hit| hit.score);
    let hits: Vec<Value> = selected
        .into_iter()
        .skip(usize::try_from(from).unwrap_or(usize::MAX))
        .take(usize::try_from(size).unwrap_or(usize::MAX))
        .map(|hit| {
            json!({
                "_index": hit.index,
                "_id": hit.id,
                "_score": hit.score,
                "_source": hit.source,
            })
        })
        .collect();
    ok(&json!({
        "took": request.took(),
        "timed_out": false,
        "_shards": shards(),
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": max_score,
            "hits": hits,
        },
    }))
}

/// `GET|POST /{index}/_count`, and the collection-wide form that names no index — how many
/// documents the query selects.
pub(super) fn count(
    collection: &CollectionRecord,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    if !matches!(request.method, "GET" | "POST") {
        return wrong_method(request, "GET, POST");
    }
    let body = match request.json() {
        Ok(body) => body.unwrap_or_else(|| json!({})),
        Err(refusal) => return *refusal,
    };
    if let Some(refusal) = only_evaluated_members(&body, "count", EVALUATED_COUNT_MEMBERS) {
        return *refusal;
    }
    let targets = match targets(collection, index_name) {
        Ok(targets) => targets,
        Err(refusal) => return *refusal,
    };
    match evaluated(&targets, &body) {
        Ok((selected, _)) => ok(&json!({
            "count": selected.len(),
            "_shards": shards(),
        })),
        Err(refusal) => *refusal,
    }
}

/// The indexes a read is over: the one it names, or EVERY index of the collection for a
/// read that names none — or names `_all` or `*`, the two the engine reads as every index.
fn targets<'c>(
    collection: &'c CollectionRecord,
    index_name: Option<&str>,
) -> Result<Vec<(&'c String, &'c state::IndexRecord)>, Box<RtResponse>> {
    let Some(index_name) = index_name.filter(|named| !EVERY_INDEX.contains(named)) else {
        return Ok(collection.indexes.iter().collect());
    };
    collection
        .indexes
        .get_key_value(index_name)
        .map(|target| vec![target])
        .ok_or_else(|| Box::new(index_not_found(index_name)))
}

/// The names the engine reads as EVERY index of the cluster, which an `IndexName` can
/// never be ([`state::validate_index_name`] refuses a leading `_`, and `*` is not a name).
pub(super) const EVERY_INDEX: &[&str] = &["_all", "*"];

/// The hits a query selects across `targets`, and the neighbour count a `knn` query asked
/// for, in the order a search reports them.
///
/// ONE [`Budget`] is spent across every index a read covers, so a read of the whole
/// collection cannot perform more work than a read of one index may.
fn evaluated(
    targets: &[(&String, &state::IndexRecord)],
    body: &Value,
) -> Result<(Vec<Hit>, Option<usize>), Box<RtResponse>> {
    let mut budget = Budget {
        remaining: MAX_SEARCH_WORK,
    };
    let mut selected: Vec<Hit> = Vec::new();
    let mut neighbours: Option<usize> = Option::None;
    for (index_name, index) in targets {
        // The selector is built per index because a `knn` clause reads the dimension and
        // the space type from THAT index's own mapping.
        let selector = selector(index, body.get("query"), 1, &mut 0)?;
        if let Some(refusal) = refused_for_work(index, &selector) {
            return Err(Box::new(refusal));
        }
        if let Selector::Knn { k, .. } = &selector {
            neighbours = Some(neighbours.map_or(*k, |held: usize| held.max(*k)));
        }
        selected.extend(selected_in(index_name, index, &selector, &mut budget)?);
    }
    selected.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(::std::cmp::Ordering::Equal)
            .then_with(|| left.index.cmp(&right.index))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok((selected, neighbours))
}

/// `GET|POST /{index}/_refresh`, and the whole-collection form — what a client calls to
/// make its writes searchable.
///
/// A write to this plane lands in the index's own document store inside the call that made
/// it, so every document is searchable the moment its write is answered and a refresh has
/// nothing left to make visible. It reports which shards it reached, as the engine does.
pub(super) fn refresh(
    collection: &CollectionRecord,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    if !matches!(request.method, "GET" | "POST") {
        return wrong_method(request, "GET, POST");
    }
    if let Some(index_name) = index_name.filter(|named| !EVERY_INDEX.contains(named)) {
        if !collection.indexes.contains_key(index_name) {
            return index_not_found(index_name);
        }
    }
    ok(&json!({ "_shards": shards() }))
}

/// A number the request carries for `name`: the body's member, else the query parameter,
/// else `default`. A value that is not a number is read as `default`, which is what the
/// engine does for a search body that names none.
fn numbered(body: &Value, request: &Request<'_>, name: &str, default: i64) -> i64 {
    if let Some(found) = body.get(name).and_then(Value::as_i64) {
        return found;
    }
    request
        .parameter(name)
        .and_then(|found| found.parse::<i64>().ok())
        .unwrap_or(default)
}

/// One selected document: the index holding it, the score the query gave it, its id and
/// its source.
struct Hit {
    index: String,
    id: String,
    score: f64,
    source: Value,
}

/// The documents of ONE index that `selector` selects, each with its score.
///
/// The scan spends `budget` as it performs comparisons and is ABORTED, with the refusal a
/// query past the budget earns, the moment it runs out — so the work one request can ask
/// for is bounded by what it actually does and not only by what its query looked like (see
/// [`MAX_SEARCH_WORK`]).
fn selected_in(
    index_name: &str,
    index: &state::IndexRecord,
    selector: &Selector,
    budget: &mut Budget,
) -> Result<Vec<Hit>, Box<RtResponse>> {
    let mut selected: Vec<Hit> = Vec::new();
    for (id, held) in &index.documents {
        let source = json_from_document(&held.source);
        if let Some(score) = selector.score(id, &source, budget) {
            selected.push(Hit {
                index: index_name.to_string(),
                id: id.clone(),
                score,
                source,
            });
        }
        if budget.remaining == 0 {
            return Err(Box::new(work_refusal(index.documents.len())));
        }
    }
    Ok(selected)
}

/// The query vocabulary this plane evaluates.
///
/// Each variant is one of the query clauses the OpenSearch query DSL defines, and each is
/// evaluated over the stored document alone. A clause outside this set — and a member of
/// one this plane does not evaluate — is refused by [`selector`] rather than ignored, so a
/// client is never told its query matched when the clause it wrote was not applied.
enum Selector {
    /// `match_all` — every document of the index, each at score 1.
    All,
    /// `match_none` — no document at all.
    None,
    /// `ids` — the documents whose id is one of those named.
    Ids(Vec<String>),
    /// `term` — the documents one of whose values at the field equals the value named.
    Term { field: String, value: Value },
    /// `terms` — `term` against any of several values.
    Terms { field: String, values: Vec<Value> },
    /// `match` — the documents whose field carries the tokens of the text named, as many
    /// of them as the clause's `operator` requires, scored by how much of the field they
    /// are; and `match_phrase`, which requires them IN ORDER and adjacent.
    Match {
        field: String,
        tokens: Vec<String>,
        /// Whether the tokens must appear as a phrase (`match_phrase`) rather than in any
        /// order and any position (`match`).
        phrase: bool,
        /// Whether EVERY token must be present (`operator: and`) rather than any one of
        /// them, which is the engine's default.
        all: bool,
    },
    /// `exists` — the documents that carry any value at the field.
    Exists { field: String },
    /// `knn` — the documents nearest the query vector, scored by the space type the
    /// index's mapping declares for the field.
    Knn {
        field: String,
        vector: Vec<f64>,
        space: Space,
        /// How many neighbours the query asked for, which caps the hits the clause
        /// contributes when it is the whole query.
        k: usize,
    },
    /// `bool` — the clauses that must, must not, should and filter.
    Bool {
        must: Vec<Selector>,
        filter: Vec<Selector>,
        should: Vec<Selector>,
        must_not: Vec<Selector>,
        /// How many of the `should` clauses a document must match, as
        /// `minimum_should_match` names it — defaulted to the engine's own default: one
        /// when nothing else is required, none otherwise.
        minimum_should_match: usize,
    },
}

/// The vector space a `knn` field's distances are measured in, from the space types
/// OpenSearch's k-NN plugin defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    /// `l1`: the sum of the absolute differences.
    L1,
    /// `l2`: the sum of the squared differences, the space a `knn_vector` field takes by
    /// default.
    L2,
    /// `linf`: the largest absolute difference.
    LInf,
    /// `cosinesimil`: one minus the cosine of the angle between the vectors.
    CosineSimilarity,
    /// `innerproduct`: the negated dot product.
    InnerProduct,
}

impl Space {
    /// The space type a wire value names, or `None` for one the plugin does not define.
    fn named(named: &str) -> Option<Self> {
        match named {
            "l1" => Some(Self::L1),
            "l2" => Some(Self::L2),
            "linf" => Some(Self::LInf),
            "cosinesimil" => Some(Self::CosineSimilarity),
            "innerproduct" => Some(Self::InnerProduct),
            _ => None,
        }
    }

    /// The score a pair of vectors earns in this space, as the k-NN plugin's own
    /// "supported spaces" table converts a distance into a score: `1 / (1 + distance)`
    /// for every distance-shaped space, and the two-branch conversion for the inner
    /// product, whose value grows with similarity rather than with distance.
    fn score(self, stored: &[f64], queried: &[f64]) -> f64 {
        let pairs = || stored.iter().zip(queried.iter());
        match self {
            Self::L1 => 1.0 / (1.0 + pairs().map(|(held, asked)| (held - asked).abs()).sum::<f64>()),
            Self::L2 => {
                let distance: f64 = pairs().map(|(held, asked)| (held - asked).powi(2)).sum();
                1.0 / (1.0 + distance)
            }
            Self::LInf => {
                let distance = pairs()
                    .map(|(held, asked)| (held - asked).abs())
                    .fold(0.0_f64, f64::max);
                1.0 / (1.0 + distance)
            }
            Self::CosineSimilarity => {
                let dot: f64 = pairs().map(|(held, asked)| held * asked).sum();
                let held_norm = stored.iter().map(|held| held * held).sum::<f64>().sqrt();
                let asked_norm = queried.iter().map(|asked| asked * asked).sum::<f64>().sqrt();
                let cosine = if held_norm == 0.0 || asked_norm == 0.0 {
                    0.0
                } else {
                    dot / (held_norm * asked_norm)
                };
                1.0 / (1.0 + (1.0 - cosine))
            }
            Self::InnerProduct => {
                let product: f64 = pairs().map(|(held, asked)| held * asked).sum();
                if product < 0.0 {
                    1.0 / (1.0 - product)
                } else {
                    1.0 + product
                }
            }
        }
    }
}

impl Selector {
    /// What evaluating this selector against ONE document costs on the QUERY side, in
    /// values compared: one per leaf clause, plus the values that clause carries. The
    /// product of this and the documents an index holds is what [`refused_for_work`]
    /// refuses a query on before the scan begins; what the scan ACTUALLY compares, which
    /// also depends on how many values the stored document holds, is spent down from
    /// [`Budget`] as it goes.
    fn clause_cost(&self) -> usize {
        match self {
            Self::All | Self::None | Self::Exists { .. } | Self::Term { .. } => 1,
            Self::Ids(named) => 1 + named.len(),
            Self::Terms { values, .. } => 1 + values.len(),
            Self::Match { tokens, .. } => 1 + tokens.len(),
            Self::Knn { vector, .. } => 1 + vector.len(),
            Self::Bool {
                must,
                filter,
                should,
                must_not,
                ..
            } => [must, filter, should, must_not]
                .into_iter()
                .flatten()
                .map(Self::clause_cost)
                .fold(1, usize::saturating_add),
        }
    }

    /// The score this selector gives the document stored under `id` with `source`, or
    /// `None` when it selects the document not at all — or when `budget` ran out, which
    /// the caller reads off the budget rather than off this answer.
    ///
    /// Every arm CHARGES the budget for the comparisons it is about to perform, from the
    /// counts it already knows (how many values the field holds, how many the clause
    /// names), and answers `None` without performing them once the budget is spent — so
    /// the comparisons actually made never exceed what was charged, and no single document
    /// can be scanned past the bound.
    fn score(&self, id: &str, source: &Value, budget: &mut Budget) -> Option<f64> {
        match self {
            Self::All => budget.spend(1).then_some(1.0),
            Self::None => {
                budget.spend(1);
                Option::None
            }
            // An `ids` clause is about the document's id, which is the key the index holds
            // it under rather than a member of its source.
            Self::Ids(named) => {
                if !budget.spend(named.len()) {
                    return Option::None;
                }
                named.iter().any(|named| named == id).then_some(1.0)
            }
            Self::Term { field, value } => {
                let held = field_values(source, field);
                if !budget.spend(held.len()) {
                    return Option::None;
                }
                held.iter()
                    .any(|held| same_value(held, value))
                    .then_some(1.0)
            }
            Self::Terms { field, values } => {
                let held = field_values(source, field);
                if !budget.spend(held.len().saturating_mul(values.len())) {
                    return Option::None;
                }
                values
                    .iter()
                    .any(|value| held.iter().any(|held| same_value(held, value)))
                    .then_some(1.0)
            }
            Self::Match {
                field,
                tokens,
                phrase,
                all,
            } => {
                let values = field_values(source, field);
                if !budget.spend(values.len()) {
                    return Option::None;
                }
                let held: Vec<String> = values
                    .iter()
                    .flat_map(|value| text_tokens(&scalar_text(value)))
                    .collect();
                if !budget.spend(held.len().saturating_mul(tokens.len().max(1))) {
                    return Option::None;
                }
                if tokens.is_empty() {
                    return Option::None;
                }
                let present = tokens.iter().filter(|token| held.contains(token)).count();
                let matches = if *phrase {
                    // A phrase requires the tokens to appear IN ORDER and adjacent, which
                    // is the whole difference between `match_phrase` and `match`.
                    held.windows(tokens.len()).any(|window| window == tokens)
                } else if *all {
                    present == tokens.len()
                } else {
                    // The engine's default operator is `or`: any one token is a hit.
                    present > 0
                };
                if !matches {
                    return Option::None;
                }
                // The score is how much of the field the tokens that MATCHED are, which is
                // a value computed from the stored document rather than a constant: a
                // document matching more of them scores higher.
                let matched = f64::from(u32::try_from(present.max(1)).unwrap_or(u32::MAX));
                let carried = f64::from(u32::try_from(held.len().max(1)).unwrap_or(u32::MAX));
                Some(matched / carried)
            }
            Self::Exists { field } => {
                let held = field_values(source, field);
                if !budget.spend(held.len()) {
                    return Option::None;
                }
                (!held.is_empty()).then_some(1.0)
            }
            Self::Knn {
                field,
                vector,
                space,
                ..
            } => {
                if !budget.spend(vector.len()) {
                    return Option::None;
                }
                let held = field_at(source, field)
                    .into_iter()
                    .find_map(numbers)
                    .filter(|held| held.len() == vector.len())?;
                Some(space.score(&held, vector))
            }
            Self::Bool {
                must,
                filter,
                should,
                must_not,
                minimum_should_match,
            } => {
                // Each child charges its own comparisons; the loops stop as soon as the
                // budget is gone, so a `bool` cannot spend past it by one more clause.
                for clause in must_not {
                    if clause.score(id, source, budget).is_some() || budget.remaining == 0 {
                        return Option::None;
                    }
                }
                let mut score = 0.0;
                // `must` contributes its own score; `filter` contributes only the
                // requirement that it match.
                for clause in must {
                    score += clause.score(id, source, budget)?;
                }
                for clause in filter {
                    clause.score(id, source, budget)?;
                }
                if budget.remaining == 0 {
                    return Option::None;
                }
                let mut matched: Vec<f64> = Vec::new();
                for clause in should {
                    if budget.remaining == 0 {
                        return Option::None;
                    }
                    if let Some(scored) = clause.score(id, source, budget) {
                        matched.push(scored);
                    }
                }
                if matched.len() < *minimum_should_match {
                    return Option::None;
                }
                score += matched.iter().sum::<f64>();
                Some(score)
            }
        }
    }
}

/// The selector a query body names, or the refusal for a query this plane does not
/// evaluate.
///
/// `depth` is the nesting this call is at, bounded by [`MAX_QUERY_DEPTH`], and `leaves`
/// counts the leaf clauses read so far, bounded by [`MAX_QUERY_CLAUSES`]: the structure is
/// the caller's to choose, so the walk's recursion is bounded by the first and the breadth
/// of every `bool` by the second.
fn selector(
    index: &state::IndexRecord,
    query: Option<&Value>,
    depth: usize,
    leaves: &mut usize,
) -> Result<Selector, Box<RtResponse>> {
    // A search body naming no query at all selects every document, as the engine's own
    // default query does.
    let Some(query) = query else {
        return Ok(Selector::All);
    };
    if depth > MAX_QUERY_DEPTH {
        return Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!("a query must not nest deeper than {MAX_QUERY_DEPTH} levels"),
        )));
    }
    *leaves = leaves.saturating_add(1);
    if *leaves > MAX_QUERY_CLAUSES {
        return Err(Box::new(error(
            400,
            TOO_MANY_CLAUSES,
            &format!("too_many_clauses: maxClauseCount is set to {MAX_QUERY_CLAUSES}"),
        )));
    }
    let Some(named) = query.as_object() else {
        return Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            "failed to parse: expected the query to be an object",
        )));
    };
    let mut clauses = named.iter();
    let Some((name, clause)) = clauses.next() else {
        return Ok(Selector::All);
    };
    // A query object names exactly ONE clause. A second member is the engine's own
    // malformed-query refusal, and refusing it is what keeps a clause this plane never
    // read from being silently dropped.
    if let Some((extra, _)) = clauses.next() {
        return Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!(
                "[{name}] malformed query, expected [END_OBJECT] but found [FIELD_NAME] [{extra}]"
            ),
        )));
    }
    match name.as_str() {
        "match_all" => Ok(Selector::All),
        "match_none" => Ok(Selector::None),
        "ids" => {
            if let Some(refusal) = only_evaluated_members(clause, "ids", &["values"]) {
                return Err(refusal);
            }
            let values = clause
                .get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| unparsed("ids", "an [ids] query names its [values]"))?;
            if values.len() > MAX_CLAUSE_VALUES {
                return Err(too_many_values("ids"));
            }
            Ok(Selector::Ids(
                values.iter().map(scalar_text).collect::<Vec<String>>(),
            ))
        }
        "exists" => {
            if let Some(refusal) = only_evaluated_members(clause, "exists", &["field"]) {
                return Err(refusal);
            }
            let field = clause
                .get("field")
                .and_then(Value::as_str)
                .ok_or_else(|| unparsed("exists", "an [exists] query names its [field]"))?;
            Ok(Selector::Exists {
                field: field.to_string(),
            })
        }
        "term" => {
            let (field, value) = single_field(clause, "term")?;
            if let Some(refusal) = only_evaluated_members(&value, "term", &["value"]) {
                return Err(refusal);
            }
            let value = value
                .get("value")
                .cloned()
                .unwrap_or_else(|| value.clone());
            Ok(Selector::Term { field, value })
        }
        "terms" => {
            let (field, value) = single_field(clause, "terms")?;
            let values = value
                .as_array()
                .ok_or_else(|| unparsed("terms", "a [terms] query names a list of values"))?;
            if values.len() > MAX_CLAUSE_VALUES {
                return Err(too_many_values("terms"));
            }
            Ok(Selector::Terms {
                field,
                values: values.clone(),
            })
        }
        "match" | "match_phrase" => {
            let (field, value) = single_field(clause, name)?;
            if let Some(refusal) = only_evaluated_members(&value, name, EVALUATED_MATCH_MEMBERS) {
                return Err(refusal);
            }
            let text = value
                .get("query")
                .map_or_else(|| scalar_text(&value), scalar_text);
            Ok(Selector::Match {
                field,
                tokens: text_tokens(&text),
                // `match_phrase` requires the tokens in order and adjacent; `match`
                // requires as many of them as its `operator` says.
                phrase: name == "match_phrase",
                all: match_operator(&value)?,
            })
        }
        "knn" => {
            let (field, value) = single_field(clause, "knn")?;
            knn_selector(index, field, &value)
        }
        "bool" => {
            let mut built = [
                ("must", Vec::new()),
                ("filter", Vec::new()),
                ("should", Vec::new()),
                ("must_not", Vec::new()),
            ];
            for (name, clauses) in &mut built {
                for written in clause_list(clause.get(*name)) {
                    clauses.push(selector(index, Some(written), depth + 1, leaves)?);
                }
            }
            let [(_, must), (_, filter), (_, should), (_, must_not)] = built;
            let minimum_should_match = minimum_should_match(clause, &must, &filter, &should)?;
            Ok(Selector::Bool {
                must,
                filter,
                should,
                must_not,
                minimum_should_match,
            })
        }
        unknown => Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!("no [query] registered for [{unknown}]"),
        ))),
    }
}

/// How many `should` clauses a document must match: the `minimum_should_match` the query
/// names, else the engine's own default — one when nothing else is required, none
/// otherwise.
///
/// Only the whole-number spelling is evaluated. A percentage or one of the engine's
/// combination expressions is REFUSED rather than ignored, so a document is never reported
/// as a hit under a constraint this plane did not apply.
fn minimum_should_match(
    clause: &Value,
    must: &[Selector],
    filter: &[Selector],
    should: &[Selector],
) -> Result<usize, Box<RtResponse>> {
    let default = usize::from(must.is_empty() && filter.is_empty() && !should.is_empty());
    let Some(named) = clause.get("minimum_should_match") else {
        return Ok(default);
    };
    let written = match named {
        Value::Number(number) => number.as_u64().and_then(|named| usize::try_from(named).ok()),
        Value::String(text) => text.trim().parse::<usize>().ok(),
        _ => Option::None,
    };
    written.ok_or_else(|| {
        Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!(
                "failed to parse setting [minimum_should_match] with value [{named}]: only a \
                 whole number of clauses is evaluated"
            ),
        ))
    })
}

/// The members of a `match` or `match_phrase` clause this plane evaluates: the text and
/// the operator over its tokens.
const EVALUATED_MATCH_MEMBERS: &[&str] = &["query", "operator"];

/// Whether a `match` requires ALL of its tokens: what its `operator` says.
///
/// The engine's default is `or` — a document carrying ANY of the tokens is a hit — and
/// `and` requires every one of them. Any other value is refused rather than read as one of
/// the two.
fn match_operator(clause: &Value) -> Result<bool, Box<RtResponse>> {
    let Some(named) = clause.get("operator") else {
        return Ok(false);
    };
    match named.as_str().map(str::to_ascii_lowercase).as_deref() {
        Some("or") => Ok(false),
        Some("and") => Ok(true),
        _ => Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!("failed to parse setting [operator] with value [{named}]: expected [or] or [and]"),
        ))),
    }
}

/// The members of ONE clause this plane evaluates. A clause naming anything else carries
/// a constraint that would not be applied, so it is refused rather than served
/// ([`only_evaluated_members`]).
///
/// A `knn` clause's `filter`, `min_score` and `max_distance` are the members this names
/// for: each of them NARROWS the neighbours the clause selects, so dropping one and
/// answering 200 would report neighbours the query did not ask for.
const EVALUATED_KNN_MEMBERS: &[&str] = &["vector", "k"];

/// The members of a search body this plane evaluates: the query and the page it asks for.
const EVALUATED_SEARCH_MEMBERS: &[&str] = &["query", "size", "from"];

/// The members of a count body this plane evaluates.
const EVALUATED_COUNT_MEMBERS: &[&str] = &["query"];

/// The refusal for an object naming a member this plane does not evaluate, or `None` when
/// every member it names is one this plane applies.
///
/// What makes a refusal the right answer rather than a convenience: a member that is read
/// nowhere is a constraint the caller asked for and did not get, and answering 200 to it
/// reports hits that were never selected under it. The refusal is the engine's own
/// unknown-key wording.
fn only_evaluated_members(
    named: &Value,
    what: &str,
    evaluated: &[&str],
) -> Option<Box<RtResponse>> {
    let unevaluated = named
        .as_object()?
        .keys()
        .find(|member| !evaluated.contains(&member.as_str()))?;
    Some(Box::new(error(
        400,
        PARSING_EXCEPTION,
        &format!(
            "Unknown key for a VALUE_OBJECT in [{what}]: [{unevaluated}]: this collection \
             evaluates {evaluated:?} and refuses a request naming a member it would not apply"
        ),
    )))
}

/// The `knn` clause for one field: the query vector, the space the index's mapping
/// declares for that field, and the number of neighbours asked for.
fn knn_selector(
    index: &state::IndexRecord,
    field: String,
    clause: &Value,
) -> Result<Selector, Box<RtResponse>> {
    if let Some(refusal) = only_evaluated_members(clause, "knn", EVALUATED_KNN_MEMBERS) {
        return Err(refusal);
    }
    let vector = clause
        .get("vector")
        .and_then(numbers)
        .ok_or_else(|| unparsed("knn", "a [knn] query names its [vector]"))?;
    if vector.is_empty() || vector.len() > MAX_CLAUSE_VALUES {
        return Err(too_many_values("knn"));
    }
    let mapped = knn_field(index, &field);
    let mapped = mapped.as_ref();
    if let Some(dimension) = mapped
        .and_then(|mapped| mapped.get("dimension"))
        .and_then(Value::as_u64)
    {
        if usize::try_from(dimension).unwrap_or(usize::MAX) != vector.len() {
            return Err(Box::new(error(
                400,
                ILLEGAL_ARGUMENT_EXCEPTION,
                &format!(
                    "Query vector has invalid dimension: {}. Dimension should be: {dimension}",
                    vector.len()
                ),
            )));
        }
    }
    // The space type is the FIELD's, declared in the mapping the index create carried, as
    // the k-NN plugin reads it; a field that declares none is in the default space.
    let named = mapped
        .and_then(|mapped| mapped.get("method"))
        .and_then(|method| method.get("space_type"))
        .or_else(|| mapped.and_then(|mapped| mapped.get("space_type")))
        .and_then(Value::as_str);
    let space = match named {
        Some(named) => Space::named(named).ok_or_else(|| {
            Box::new(error(
                400,
                ILLEGAL_ARGUMENT_EXCEPTION,
                &format!("Unable to find space: {named}"),
            ))
        })?,
        None => Space::L2,
    };
    let k = clause
        .get("k")
        .and_then(Value::as_u64)
        .and_then(|k| usize::try_from(k).ok())
        .filter(|k| *k > 0)
        .ok_or_else(|| {
            // The plugin requires one of `k`, `min_score` or `max_distance`, and the other
            // two are refused above: a clause naming none of them would select every
            // document of the index, which is not what it asked for.
            Box::new(error(
                400,
                ILLEGAL_ARGUMENT_EXCEPTION,
                "[knn] requires exactly one of k, distance or score",
            ))
        })?;
    Ok(Selector::Knn {
        field,
        vector,
        space,
        k,
    })
}

/// The mapping the index declares for one field, when that field is mapped at all —
/// where a `knn_vector` field's dimension and space type are read from.
fn knn_field(index: &state::IndexRecord, field: &str) -> Option<Value> {
    let mappings = super::indexes::mappings(index);
    let mut held = mappings.get("properties")?.clone();
    let mut segments = field.split('.').peekable();
    while let Some(segment) = segments.next() {
        held = held.get(segment)?.clone();
        if segments.peek().is_some() {
            held = held.get("properties")?.clone();
        }
    }
    Some(held)
}

/// The clauses a `bool` member holds: a list of clauses, or the one clause written where
/// a list belongs.
fn clause_list(member: Option<&Value>) -> Vec<&Value> {
    match member {
        Some(Value::Array(clauses)) => clauses.iter().collect(),
        Some(single) => vec![single],
        Option::None => Vec::new(),
    }
}

/// The one field a field-named clause is about, and the value written for it.
fn single_field(clause: &Value, name: &str) -> Result<(String, Value), Box<RtResponse>> {
    let mut members = clause
        .as_object()
        .ok_or_else(|| unparsed(name, &format!("a [{name}] query names one field")))?
        .iter();
    let (field, value) = members
        .next()
        .ok_or_else(|| unparsed(name, &format!("a [{name}] query names one field")))?;
    if members.next().is_some() {
        return Err(Box::new(error(
            400,
            PARSING_EXCEPTION,
            &format!("[{name}] query doesn't support multiple fields"),
        )));
    }
    Ok((field.clone(), value.clone()))
}

/// The refusal for a clause this plane could not read.
fn unparsed(name: &str, reason: &str) -> Box<RtResponse> {
    Box::new(error(
        400,
        PARSING_EXCEPTION,
        &format!("[{name}] malformed query, {reason}"),
    ))
}

/// The refusal for a clause naming more values than one clause may carry.
fn too_many_values(name: &str) -> Box<RtResponse> {
    Box::new(error(
        400,
        ILLEGAL_ARGUMENT_EXCEPTION,
        &format!("a [{name}] query must name between 1 and {MAX_CLAUSE_VALUES} values"),
    ))
}

/// The values a document carries at `field`, where a dotted field name walks into nested
/// objects — each member AS STORED, so a list-valued member is one value (a vector, which
/// a `knn` clause reads whole).
///
/// The members are BORROWED from the document: a clause is evaluated against every
/// document of the index, so nothing here may copy one.
fn field_at<'v>(source: &'v Value, field: &str) -> Vec<&'v Value> {
    let mut held = vec![source];
    for segment in field.split('.') {
        let mut next: Vec<&'v Value> = Vec::new();
        for value in held {
            match value {
                Value::Object(members) => {
                    if let Some(found) = members.get(segment) {
                        next.push(found);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        if let Some(found) = item.get(segment) {
                            next.push(found);
                        }
                    }
                }
                _ => {}
            }
        }
        held = next;
        if held.is_empty() {
            return Vec::new();
        }
    }
    held
}

/// The values a document carries at `field`, with a list-valued member contributing each
/// of its ELEMENTS — which is what makes a `term` over a list of keywords match any one of
/// them, and what a `match` tokenises. Borrowed, for the reason [`field_at`] gives.
fn field_values<'v>(source: &'v Value, field: &str) -> Vec<&'v Value> {
    field_at(source, field)
        .into_iter()
        .flat_map(|value| match value {
            Value::Array(items) => items.iter().collect::<Vec<&'v Value>>(),
            single => vec![single],
        })
        .filter(|value| !value.is_null())
        .collect()
}

/// Whether a stored value is the one a `term` names. A term is compared as the engine
/// compares a keyword: by the text of the value, so `1` and `"1"` are the same term.
fn same_value(held: &Value, named: &Value) -> bool {
    held == named || scalar_text(held) == scalar_text(named)
}

/// The text of a scalar value; an object or a list has no term text of its own.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(held) => held.to_string(),
        _ => String::new(),
    }
}

/// The tokens a text carries, as the standard analyzer produces them: lowercased runs of
/// letters and digits, with everything else a separator.
fn text_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The numbers a value carries, when it is a list of them — a vector, stored or queried.
fn numbers(value: &Value) -> Option<Vec<f64>> {
    let items = value.as_array()?;
    items.iter().map(Value::as_f64).collect()
}
