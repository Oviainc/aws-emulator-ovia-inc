// SPDX-License-Identifier: CC0-1.0

//! @op Rerank
//!
//! ## What this handler does
//!
//! `Rerank` is a stateless data-plane computation: it mints no resource, and no other
//! operation of this service reports a rerank, so it persists nothing (the only store
//! it reads is the one the AUTHORIZATION decision is made from — the caller's STS
//! identity and the IAM policies in force on it, through
//! [`crate::state::authorize`], exactly as every other operation here decides it).
//!
//! It answers with ONE permutation of the supplied `sources`, ordered by the relevance
//! of each source document to the single query, highest first:
//!
//! * every source contributes exactly ONE result, carrying that source's POSITION in
//!   the request's `sources` list as `index` — the list is permuted once for the query,
//!   not once per document;
//! * `relevanceScore` is a number in `[0, 1]` computed from the query text and that
//!   source document's own text, and from nothing else: the same (query, document) pair
//!   scores the same in every call, whatever the rest of the candidate list holds;
//! * `numberOfResults` keeps the leading `n` of that ordering.
//!
//! ## How the relevance score is computed
//!
//! NO neural reranker model runs inside this emulator, and nothing here invents a score
//! and presents it as one a model produced (the same position
//! `service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice`'s `state` module
//! takes on inference). The score is instead the classic LEXICAL relevance of the pair,
//! computed from the real text of the query and the document: the cosine similarity of
//! their sublinear term-frequency vectors — `lnc.lnc` weighting in the SMART notation of
//! Manning, Raghavan & Schütze, *Introduction to Information Retrieval* §6.4, where a
//! term's weight in a bag is `1 + ln(tf)` and the vectors are length-normalised.
//!
//! That choice is what makes the properties above hold, and each is a property of the
//! real service too:
//!
//! * the value is in `[0, 1]`, `1.0` for a document whose terms are the query's in the
//!   same proportions and `0.0` for one sharing no term;
//! * it is a pure function of the ONE pair — a cross-encoder reranker scores each
//!   (query, document) pair independently, so a corpus-relative weighting (BM25's
//!   inverse document frequency, say) would be the wrong shape: a document's score
//!   would move when an unrelated candidate joined the list.
//!
//! A `JSON` source document is scored over the text its JSON carries ([`json_text`]),
//! since that serialisation is what a reranker model is shown.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `Rerank::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `Rerank`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json")
//! ```
//!
//! Reranks the relevance of sources based on queries. For more information, see Improve the
//! relevance of query responses with a reranker model.
//!
//! ## Input
//!
//! * `nextToken` (NextToken, len 1..2048, pattern ^\S*$) — If the total number of results was
//!   greater than could fit in a response, a token is returned in the nextToken field. You
//!   can enter that token in this field to return the next batch of results.
//! * `queries` (RerankQueriesList, len 1..1) **required** — An array of objects, each of
//!   which contains information about a query to submit to the reranker model.
//! * `rerankingConfiguration` (RerankingConfiguration) **required** — Contains configurations
//!   for reranking.
//! * `sources` (RerankSourcesList, len 1..1000) **required** — An array of objects, each of
//!   which contains information about the sources to rerank.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..2048, pattern ^\S*$) — If the total number of results is
//!   greater than can fit in the response, use this token in the nextToken field when making
//!   another request to return the next batch of results.
//! * `results` (RerankResultsList) **required** — An array of objects, each of which contains
//!   information about the results of reranking.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions. Check your permissions and retry your request.
//! * `BadGatewayException` (HTTP 502) — There was an issue with a dependency due to a server
//!   issue. Retry your request.
//! * `ConflictException` (HTTP 409) — There was a conflict performing an operation. Resolve
//!   the conflict and retry your request.
//! * `DependencyFailedException` (HTTP 424) — There was an issue with a dependency. Check the
//!   resource configurations and retry the request.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

use std::collections::BTreeMap;

use ::aws_smithy_types::Document as JsonValue;
use ::aws_smithy_types::Number as JsonNumber;

#[allow(unused_imports)]
use super::types::*;

/// The `RerankingConfigurationType` this operation serves, and the only one the model
/// declares (`RERANKING_CONFIGURATION_TYPE_VALUES`).
const CONFIGURATION_TYPE_BEDROCK: &str = "BEDROCK_RERANKING_MODEL";

/// The `RerankQueryContentType` whose text lives in `textQuery`, and the only one the
/// model declares (`RERANK_QUERY_CONTENT_TYPE_VALUES`).
const QUERY_TYPE_TEXT: &str = "TEXT";

/// The `RerankSourceType` whose document travels in the request, and the only one the
/// model declares (`RERANK_SOURCE_TYPE_VALUES`).
const SOURCE_TYPE_INLINE: &str = "INLINE";

/// The two `RerankDocumentType` values (`RERANK_DOCUMENT_TYPE_VALUES`): `TEXT` carries
/// the document in `textDocument`, `JSON` in `jsonDocument`.
const DOCUMENT_TYPE_TEXT: &str = "TEXT";

/// See [`DOCUMENT_TYPE_TEXT`].
const DOCUMENT_TYPE_JSON: &str = "JSON";

/// How deep a caller-supplied `jsonDocument` is walked before the walk stops, keeping
/// the recursion bound LOCAL to this crate rather than inherited from the
/// deserializer's own limit (the same bound, for the same reason, as
/// `service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice`'s
/// `MAX_DOCUMENT_DEPTH`).
const MAX_DOCUMENT_DEPTH: usize = 64;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RerankError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RerankError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#Rerank")]
#[derive(Debug)]
pub enum RerankErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::rerank::RerankError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RerankError = crate::scaffolded::Scaffolded<RerankErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RerankErrorModeled> for RerankError {
    fn from(modeled: RerankErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Rerank` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Rerank<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Rerank<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Rerank<Eng>
{
    type Input = RerankInput;
    type Output = RerankOutput;
    type Error = RerankError;

    const NAME: &'static str = "Rerank";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RERANK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RERANK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The request is read in full BEFORE anything is decided or scored: a request
        // this service cannot interpret is refused whatever the caller may do, and the
        // authorization decision below needs the model the call names.
        let configuration = &input.reranking_configuration;
        if configuration.r#type != CONFIGURATION_TYPE_BEDROCK {
            return Err(refuse(format!(
                "rerankingConfiguration.type: {:?} is not a reranking configuration type \
                 this service offers. Supported types: {}",
                configuration.r#type,
                RERANKING_CONFIGURATION_TYPE_VALUES.join(", ")
            )));
        }

        // `nextToken` names a page of a response this service never splits: a request
        // carries at most 1000 sources (`RerankSourcesList`'s own length bound), so the
        // complete ranking always fits one response and NO call issues a token. A token
        // reaching here therefore cannot have come from this service.
        if input.next_token.is_some() {
            return Err(refuse(
                "nextToken is not a pagination token this service issued: a Rerank response \
                 carries the complete ranking of the sources it was given"
                    .to_string(),
            ));
        }

        // `RerankQueriesList` is modelled `length 1..1`, so the boundary validator has
        // already refused any other count and the one query is the whole query.
        let Some(query) = input.queries.first() else {
            return Err(refuse(
                "queries must carry exactly one query to rerank the sources against"
                    .to_string(),
            ));
        };
        if query.r#type != QUERY_TYPE_TEXT {
            return Err(refuse(format!(
                "queries.0.type: {:?} is not a query content type this service offers. \
                 Supported types: {}",
                query.r#type,
                RERANK_QUERY_CONTENT_TYPE_VALUES.join(", ")
            )));
        }
        let Some(query_text) = query.text_query.text.as_deref() else {
            return Err(refuse(format!(
                "queries.0.textQuery.text is required for a {QUERY_TYPE_TEXT} query"
            )));
        };

        // One text per source, in the order the sources were given, so a source that
        // cannot be read is refused before any of them is scored.
        let mut documents: Vec<String> = Vec::with_capacity(input.sources.len());
        for (position, source) in input.sources.iter().enumerate() {
            if source.r#type != SOURCE_TYPE_INLINE {
                return Err(refuse(format!(
                    "sources.{position}.type: {:?} is not a source type this service offers. \
                     Supported types: {}",
                    source.r#type,
                    RERANK_SOURCE_TYPE_VALUES.join(", ")
                )));
            }
            documents.push(
                document_text(
                    &source.inline_document_source,
                    &format!("sources.{position}.inlineDocumentSource"),
                )
                .map_err(refuse)?,
            );
        }

        // Authorization is decided against the reranker model this call runs against —
        // the resource an identity-based policy for `bedrock:Rerank` names — and before
        // the ranking is computed.
        let resource = model_resource_arn(
            ctx,
            &configuration
                .bedrock_reranking_configuration
                .model_configuration
                .model_arn,
        );
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, Self::NAME, &resource) {
            return Err(RerankErrorModeled::AccessDeniedException(denied).into());
        }

        // ONE permutation of the source list for the query: every source is scored
        // against it exactly once, and the score of a pair depends on that pair alone.
        let query_terms = TermVector::of(query_text);
        let mut ranked: Vec<(usize, f64)> = documents
            .iter()
            .enumerate()
            .map(|(position, text)| (position, query_terms.similarity(&TermVector::of(text))))
            .collect();

        // Most relevant first. Sources of equal relevance keep the order they were
        // given in, so the permutation is total and one request always answers the same
        // way.
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(::std::cmp::Ordering::Equal)
                .then(left.0.cmp(&right.0))
        });

        // `numberOfResults` keeps the leading n of that ranking. Absent, every source is
        // reported: dropping candidates the caller did not ask to have dropped would
        // lose results the caller sent, and the model states no smaller default.
        let kept = configuration
            .bedrock_reranking_configuration
            .number_of_results
            .and_then(|requested| usize::try_from(requested).ok())
            .filter(|requested| *requested > 0)
            .unwrap_or(ranked.len())
            .min(ranked.len());
        ranked.truncate(kept);

        Ok(RerankOutput {
            results: ranked
                .into_iter()
                .map(|(position, score)| RerankResult {
                    // The source's POSITION in the request's `sources` list, which is
                    // what identifies the document that was ranked.
                    index: i32::try_from(position).unwrap_or(i32::MAX),
                    relevance_score: score as f32,
                    // `document` is optional, and the caller supplied every document in
                    // this very request: the RANKING is what the call computes, so
                    // nothing is echoed back.
                    document: None,
                })
                .collect(),
            // See the `nextToken` refusal above: the whole ranking is in this response.
            next_token: None,
        })
    }
}

/// This operation's modelled `ValidationException` (HTTP 400), naming the member that
/// was wrong and what it requires — the caller has to fix the request from this text
/// alone.
fn refuse(message: String) -> RerankError {
    RerankErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}

/// The resource an identity-based policy for `bedrock:Rerank` names: the reranker model
/// the call runs against.
///
/// A `modelArn` that is already an ARN IS that resource. A bare model id — which the
/// modelled `BedrockModelArn` pattern also admits — names the foundation model of that
/// name in the region the caller signed for, the same ARN
/// `service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice`'s
/// `foundation_model_arn` builds for a base model, so the two planes make the decision
/// about one resource name.
fn model_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    model_arn: &str,
) -> String {
    if model_arn.starts_with("arn:") {
        return model_arn.to_string();
    }
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx),
        &format!("foundation-model/{model_arn}"),
    )
}

/// The text one source document carries, or the `ValidationException` message owed for
/// a document whose `type` and payload disagree.
///
/// The model makes both payload members optional while `type` is required, so the
/// boundary validator cannot refuse a `TEXT` document that carries no `textDocument` (or
/// a `JSON` one that carries no `jsonDocument`): that cross-member rule is this
/// handler's to enforce, and `path` is the member path of the document being read.
fn document_text(document: &RerankDocument, path: &str) -> Result<String, String> {
    match document.r#type.as_str() {
        DOCUMENT_TYPE_TEXT => document
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.text.clone())
            .ok_or_else(|| {
                format!("{path}.textDocument.text is required when type is {DOCUMENT_TYPE_TEXT}")
            }),
        DOCUMENT_TYPE_JSON => document
            .json_document
            .as_ref()
            .map(json_text)
            .ok_or_else(|| {
                format!("{path}.jsonDocument is required when type is {DOCUMENT_TYPE_JSON}")
            }),
        other => Err(format!(
            "{path}.type: {other:?} is not a document type this service offers. Supported \
             types: {}",
            RERANK_DOCUMENT_TYPE_VALUES.join(", ")
        )),
    }
}

/// The text a `JSON` source document carries: every object MEMBER NAME and every scalar
/// in it, joined by single spaces — the serialisation a reranker model is shown for a
/// JSON document, so the member names are part of its content.
///
/// Object members are walked in sorted key order, because the deserialized map is
/// unordered and one document must always yield one text; the walk stops at
/// [`MAX_DOCUMENT_DEPTH`], so a deeply nested document cannot drive unbounded recursion.
fn json_text(value: &JsonValue) -> String {
    fn walk(value: &JsonValue, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DOCUMENT_DEPTH {
            return;
        }
        match value {
            JsonValue::String(text) => out.push(text.clone()),
            JsonValue::Number(number) => out.push(match number {
                JsonNumber::PosInt(unsigned) => unsigned.to_string(),
                JsonNumber::NegInt(signed) => signed.to_string(),
                JsonNumber::Float(float) => float.to_string(),
            }),
            JsonValue::Bool(flag) => out.push(flag.to_string()),
            // A null member carries no text at all.
            JsonValue::Null => {}
            JsonValue::Array(items) => {
                for item in items {
                    walk(item, depth + 1, out);
                }
            }
            JsonValue::Object(members) => {
                let ordered: BTreeMap<&String, &JsonValue> = members.iter().collect();
                for (name, member) in ordered {
                    out.push(name.clone());
                    walk(member, depth + 1, out);
                }
            }
        }
    }

    let mut parts: Vec<String> = Vec::new();
    walk(value, 0, &mut parts);
    parts.join(" ")
}

/// One text as the weighted bag of terms relevance is computed over: each term's
/// `1 + ln(tf)` weight (sublinear term frequency: the tenth occurrence of a term says
/// far less than the second), and the vector's Euclidean norm.
#[derive(Debug)]
struct TermVector {
    /// Term -> weight, keyed so two vectors can be intersected term by term.
    weights: BTreeMap<String, f64>,
    /// `sqrt(sum of squared weights)`, the length the cosine divides by. Zero exactly
    /// when the text carries no term at all.
    norm: f64,
}

impl TermVector {
    /// The vector of `text`.
    fn of(text: &str) -> Self {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for term in terms(text) {
            *counts.entry(term).or_default() += 1;
        }
        let weights: BTreeMap<String, f64> = counts
            .into_iter()
            .map(|(term, count)| (term, 1.0 + (count as f64).ln()))
            .collect();
        let norm = weights.values().map(|weight| weight * weight).sum::<f64>().sqrt();
        Self { weights, norm }
    }

    /// The relevance of `document` to this (query) vector: the cosine of the angle
    /// between them, in `[0, 1]` — `1.0` when the two carry the same terms in the same
    /// proportions, `0.0` when they share no term (and when either carries no term at
    /// all, which has no direction to compare).
    fn similarity(&self, document: &Self) -> f64 {
        if self.norm == 0.0 || document.norm == 0.0 {
            return 0.0;
        }
        // Walk the shorter side, so the cost is in the query's terms for the long
        // documents a reranker is given.
        let (shorter, longer) = if self.weights.len() <= document.weights.len() {
            (&self.weights, &document.weights)
        } else {
            (&document.weights, &self.weights)
        };
        let dot: f64 = shorter
            .iter()
            .filter_map(|(term, weight)| longer.get(term).map(|other| weight * other))
            .sum();
        // Clamped only against floating-point drift at the extremes: the Cauchy-Schwarz
        // inequality already bounds the quotient by 1, and no weight is negative.
        (dot / (self.norm * document.norm)).clamp(0.0, 1.0)
    }
}

/// The terms of `text`: its maximal runs of alphanumeric characters, lowercased, so
/// matching is case- and punctuation-insensitive. Unicode-aware, because a document and
/// a query are free text in any script.
fn terms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
}
