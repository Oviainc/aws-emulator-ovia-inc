// SPDX-License-Identifier: CC0-1.0

//! The data plane's ANALYZE API: `GET`/`POST /_analyze` and `GET`/`POST /{index}/_analyze`,
//! which report "the tokens that would be produced" for a text.
//!
//! The chain the text goes through is the one [`crate::state::analysis`] builds — an
//! analyzer the index defines, one the engine ships, a field's own, or one the request
//! composes out of a tokenizer and filters — and the answer is the token stream that chain
//! produces: nothing here invents a token, and a request naming an analyzer, tokenizer,
//! filter or field nothing resolves is refused with what the engine could not find rather
//! than answered with the default chain's output.

use ::serde_json::{json, Value};
use aws_smithy_runtime_api::http::Response as RtResponse;

use crate::state::analysis::{IndexAnalysis, PackageFiles, Token, MAX_TOKEN_COUNT};
use crate::state::{fgac, ingest, Domain};

use super::{error, guarded, index_not_found, ok, wrong_method, Request};

/// The member, and the query parameter, a request names the text to analyze with.
const TEXT: &str = "text";

/// The member a request names the analyzer to use with.
const ANALYZER: &str = "analyzer";

/// The member a request names the field whose own analyzer to use with.
const FIELD: &str = "field";

/// The member a request composes a chain with: its tokenizer.
const TOKENIZER: &str = "tokenizer";

/// The member a request composes a chain with: its token filters.
const FILTER: &str = "filter";

/// The most texts one request analyzes.
///
/// The emulator's OWN bound on the work one request causes: every value it carries goes
/// through the whole chain, and the token count of each is bounded by the engine's own
/// ceiling ([`crate::state::analysis::MAX_TOKEN_COUNT`]).
const MAX_TEXTS: usize = 100;

/// `GET`/`POST /_analyze`, and the same addressed to one index.
///
/// `index_name` is the index the path named, where it named one: its settings are what an
/// analyzer name, a filter name and a field resolve against.
pub(super) fn analyze_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
    files: &PackageFiles,
) -> RtResponse {
    match request.method {
        "GET" | "POST" => guarded(domain, request, fgac::ACTION_ANALYZE, |domain| {
            analyze(domain, request, index_name, files)
        }),
        _ => wrong_method(request, "GET, POST"),
    }
}

/// The tokens the request's text produces, through the chain the request resolves.
fn analyze(
    domain: &Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
    files: &PackageFiles,
) -> RtResponse {
    let body = match request.json() {
        Ok(body) => body.unwrap_or_else(|| json!({})),
        Err(refusal) => return *refusal,
    };
    // An index-scoped request is answered out of THAT index's analysis settings, so an
    // index the domain does not hold is the engine's own not-found rather than an answer
    // from the cluster's defaults.
    let analysis = match index_name {
        Some(name) => match domain.indexes.get(name) {
            Some(index) => IndexAnalysis::of(&index.schema),
            None => return index_not_found(name),
        },
        None => IndexAnalysis::default(),
    };
    let texts = match requested_texts(&body, request) {
        Ok(texts) => texts,
        Err(named) => {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &format!(
                    "a request may analyze at most {MAX_TEXTS} texts, and this one names \
                     {named}"
                ),
            )
        }
    };
    if texts.is_empty() {
        return error(400, ingest::ILLEGAL_ARGUMENT_EXCEPTION, "text is missing");
    }
    let named = |member: &str| -> Option<String> {
        body.get(member)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| request.parameter(member))
    };
    let resolved = match (named(ANALYZER), named(FIELD)) {
        // "The analyzer that should be applied to the provided text."
        (Some(analyzer), _) => analysis.analyzer(&analyzer, files),
        // "Field that should be used to derive the analyzer", which only an index has.
        (None, Some(field)) => {
            if index_name.is_none() {
                return error(
                    400,
                    ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                    "analysis based on a specific field requires an index",
                );
            }
            analysis.field_analyzer(&field, true, files)
        }
        // A chain the request composes itself, or — where it composes none — the index's
        // own default analyzer. The tokenizer is taken in EITHER form the request may
        // write it in — a name or an inline configuration — so neither is dropped and
        // answered with the default chain's tokens.
        (None, None) => {
            let tokenizer = requested_tokenizer(&body, request);
            let filters = requested_filters(&body, request);
            if tokenizer.is_some() || filters.is_some() {
                analysis.composed(tokenizer.as_ref(), filters.as_ref(), files)
            } else {
                Ok(analysis.default_analyzer(files))
            }
        }
    };
    let analyzer = match resolved {
        Ok(analyzer) => analyzer,
        Err(unresolved) => {
            // A request the path scoped to no index resolves names against the CLUSTER's
            // own registry, which is what the engine says it could not find in.
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &unresolved.message(index_name.is_none()),
            )
        }
    };
    // Several values are analyzed as ONE stream, as a multi-valued field's are: each value
    // continues the positions and the character offsets the previous one left off at, so a
    // token's position says where in the whole input it stands.
    let mut tokens: Vec<Value> = Vec::new();
    let mut position = 0_usize;
    let mut offset = 0_usize;
    for text in &texts {
        let analyzed = analyzer.analyze(text);
        let last = analyzed.iter().map(|token| token.position).max();
        tokens.extend(
            analyzed
                .iter()
                .map(|token| rendered(token, position, offset)),
        );
        // The engine refuses a stream past its own ceiling rather than reporting the part
        // of it that fits: a token list that is not the whole analysis is one a client
        // cannot tell from a complete one. The ceiling is over the WHOLE request, so
        // neither the number of values nor what a filter expands them into gets past it.
        if tokens.len() > MAX_TOKEN_COUNT {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &format!(
                    "The number of tokens produced by analyzer must not exceed \
                     [{MAX_TOKEN_COUNT}]. This limit can be set by changing the \
                     [index.analyze.max_token_count] index level setting."
                ),
            );
        }
        position = position.saturating_add(last.map_or(0, |last| last.saturating_add(1)));
        offset = offset.saturating_add(text.chars().count());
    }
    ok(&json!({ "tokens": tokens }))
}

/// One token as the analyze API reports it, shifted by where the value it came from stands
/// in the whole input.
fn rendered(token: &Token, position: usize, offset: usize) -> Value {
    json!({
        "token": token.token,
        "start_offset": token.start_offset.saturating_add(offset),
        "end_offset": token.end_offset.saturating_add(offset),
        "type": token.kind,
        "position": token.position.saturating_add(position),
    })
}

/// The texts the request names: "the text on which the analysis should be performed", as
/// one string or an array of them, in the body or in the query string.
///
/// `Err` carries the count of a request naming more values than [`MAX_TEXTS`]: past that
/// bound the request is refused rather than analyzed from part of what it sent.
fn requested_texts(body: &Value, request: &Request<'_>) -> Result<Vec<String>, usize> {
    match body.get(TEXT) {
        Some(Value::String(text)) => return Ok(vec![text.clone()]),
        Some(Value::Array(texts)) => {
            if texts.len() > MAX_TEXTS {
                return Err(texts.len());
            }
            return Ok(texts
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect());
        }
        _ => {}
    }
    Ok(request
        .parameter(TEXT)
        .map(|text| vec![text])
        .unwrap_or_default())
}

/// The tokenizer the request composes its chain with, in either form it may write it in —
/// a name or an inline configuration — or as the query string names it, or `None` where it
/// names none.
fn requested_tokenizer(body: &Value, request: &Request<'_>) -> Option<Value> {
    if let Some(named) = body.get(TOKENIZER) {
        return Some(named.clone());
    }
    Some(Value::String(request.parameter(TOKENIZER)?))
}

/// The token filters the request composes its chain with, as the body lists them or as the
/// query string names them (comma-separated), or `None` where it names none.
fn requested_filters(body: &Value, request: &Request<'_>) -> Option<Value> {
    if let Some(listed) = body.get(FILTER) {
        return Some(listed.clone());
    }
    let named = request.parameter(FILTER)?;
    Some(Value::Array(
        named
            .split(',')
            .filter(|name| !name.is_empty())
            .map(|name| Value::String(name.to_string()))
            .collect(),
    ))
}
