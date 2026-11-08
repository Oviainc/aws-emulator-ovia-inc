// SPDX-License-Identifier: CC0-1.0

//! The ANALYSIS a domain's cluster runs, driven over the real wire path: the analyze API on
//! the endpoint a domain publishes, and the search that is evaluated over the terms the same
//! chain produces.
//!
//! Three rules, each over a SET of inputs rather than one recorded call:
//!
//! * the analyze API reports the tokens the chain the request RESOLVES produces — over a
//!   table spanning how a chain is named (a built-in analyzer, a language analyzer, one the
//!   index defines, a field's own, one the request composes from a tokenizer and filters) and
//!   what each does to the text, plus the refusals for a name nothing resolves;
//! * the content of an associated TXT-DICTIONARY package IS what a filter naming its mount
//!   point reads — over a table of terms, a listed one and an unlisted one, asserted through
//!   the analyze API and through a `match` query on a field that uses the chain, against a
//!   control field analyzed by the engine's own `standard`;
//! * a synonym rule the package's file holds makes a query term find the document that
//!   carries the term it stands for.
//!
//! The package's content is the Amazon S3 OBJECT the create imports: it is put in the S3
//! store of the SAME shared engine — the store `s3:PutObject` writes — so what drives the
//! analyzer is the content that service holds, not a value this suite handed the analyzer.
//! Every call path comes from the sibling suites ([`super::domain_lifecycle`],
//! [`super::package_associations`], [`super::package_dependency_lifecycle`],
//! [`super::index_registry`] and [`super::domain_rest_data_plane`]), so this suite holds no
//! second copy of one.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::PACKAGE_TYPE_TXT_DICTIONARY;
use service_com_amazonaws_s3_amazons3::state as s3;

use super::domain_lifecycle::{Answer, Emulator, REGION, SETTLE_WAIT};
use super::domain_rest_data_plane::{error_type, rest_prefixed};
use super::index_registry::created_index;
use super::package_associations::{create_package, package_key, PACKAGE_BUCKET};
use super::package_dependency_lifecycle::associated;

/// The analyzer the index of the package cases defines, and the filters it chains.
const PACKAGED_ANALYZER: &str = "packaged";

/// The field that analyzer analyzes, and the control field beside it that the engine's own
/// `standard` analyzer analyzes — so one document's text is judged by two chains.
const PACKAGED_FIELD: &str = "pkg_text";
const CONTROL_FIELD: &str = "plain_text";

/// Put `content` at the S3 location the create of the package `name` imports from, as
/// `s3:PutObject` records an object.
fn given_dictionary(emulator: &Emulator, name: &str, content: &str) {
    let store = s3::service_state(&**emulator.engine()).expect("the S3 store");
    let mut guard = store.lock();
    let _ = guard.create_bucket(PACKAGE_BUCKET, REGION);
    let written = guard.put_object(
        PACKAGE_BUCKET,
        &package_key(name),
        s3::Object::stored(content.as_bytes().to_vec(), Some("text/plain".to_string())),
    );
    assert!(
        written.is_some(),
        "case \"the package's own content\": expected S3 to hold the object at \
         {PACKAGE_BUCKET}/{}, got {written:?}",
        package_key(name)
    );
}

/// The domain's own access policy: every data-plane action of this service allowed on
/// every path of this one domain, so what a case is decided by is the ANALYSIS chain and
/// not authorization — which the sibling suites
/// ([`super::published_domain_address`], [`super::domain_rest_data_plane`]) own.
fn open_policy(domain: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "es:ESHttp*",
            "Resource": format!(
                "arn:aws:es:{REGION}:{}:domain/{domain}/*",
                ::engine::emulator_account_id()
            ),
        }],
    })
    .to_string()
}

/// A domain whose data plane this suite's own credential may reach, and one AVAILABLE
/// `TXT-DICTIONARY` package whose content is `content`, with no association between them
/// yet. Answers the `PackageID` the create minted.
///
/// The content is put where the create IMPORTS it from before the create runs, so the
/// package this suite associates carries the object Amazon S3 holds.
async fn given_domain_and_package(
    emulator: &Emulator,
    domain: &str,
    package: &str,
    content: &str,
) -> String {
    given_dictionary(emulator, package, content);
    let package_id = create_package(emulator, package, PACKAGE_TYPE_TXT_DICTIONARY).await;
    let answer = emulator
        .create(json!({ "DomainName": domain, "AccessPolicies": open_policy(domain) }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    // The copy span `CreatePackage` leaves the package in `COPYING` for, and the domain's
    // own provisioning span, both fall due inside this one wait.
    tokio::time::sleep(SETTLE_WAIT).await;
    package_id
}

/// `POST /{index}/_analyze`, or the cluster-wide `/_analyze` where `index` is `None`.
async fn analyze(emulator: &Emulator, domain: &str, index: Option<&str>, body: Value) -> Answer {
    let path = match index {
        Some(index) => format!("/{index}/_analyze"),
        None => "/_analyze".to_string(),
    };
    rest_prefixed(emulator, domain, "POST", &path, Some(body)).await
}

/// The token texts an answer reports, in the order it reports them.
fn tokens(answer: &Answer) -> Vec<String> {
    answer.body["tokens"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|token| token["token"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The schema the package cases create their index with: a filter reading the package's
/// file by path, a custom analyzer chaining it, and two text fields — one analyzed by that
/// chain and one by the engine's own default.
fn packaged_schema(kind: &str, setting: &str, path: &str) -> Value {
    json!({
        "settings": {
            "analysis": {
                "filter": {
                    "from_package": { "type": kind, setting: path },
                },
                "analyzer": {
                    PACKAGED_ANALYZER: {
                        "type": "custom",
                        "tokenizer": "standard",
                        "filter": ["lowercase", "from_package"],
                    },
                },
            },
        },
        "mappings": {
            "properties": {
                PACKAGED_FIELD: { "type": "text", "analyzer": PACKAGED_ANALYZER },
                CONTROL_FIELD: { "type": "text" },
            },
        },
    })
}

/// How many hits a `match` on `field` for `term` answers with.
async fn hits(emulator: &Emulator, domain: &str, index: &str, field: &str, term: &str) -> i64 {
    let answer = rest_prefixed(
        emulator,
        domain,
        "POST",
        &format!("/{index}/_search"),
        Some(json!({ "query": { "match": { field: term } } })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"a match on {field} for {term:?}\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["hits"]["total"]["value"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!(
                "case \"a match on {field} for {term:?}\": expected a hit total, got {:?}",
                answer.body
            )
        })
}

/// RULE: the analyze API reports the tokens the chain the request resolves produces — an
/// analyzer the engine ships, one the index defines, a field's own, or one the request
/// composes from a tokenizer and filters.
#[tokio::test]
async fn the_analyze_api_reports_the_tokens_the_resolved_chain_produces() {
    let emulator = Emulator::new();
    let domain = "analyze-surface";
    let package = "analyze-surface-dictionary";
    let package_id = given_domain_and_package(&emulator, domain, package, "fluffy\n").await;
    let path = associated(&emulator, domain, &package_id, "the analyze surface").await;
    let index = "analyzed";
    created_index(
        &emulator,
        domain,
        index,
        &packaged_schema("stop", "stopwords_path", &path),
        "the index the analyze cases read",
    )
    .await;

    // (the case, the index the path names, the request, the tokens the chain produces)
    struct Row {
        case: &'static str,
        index: Option<&'static str>,
        request: Value,
        expected: Vec<&'static str>,
    }
    let text = "The Quick brown foxes 42";
    let rows: Vec<Row> = vec![
        Row {
            case: "the engine's standard analyzer",
            index: None,
            request: json!({ "analyzer": "standard", "text": text }),
            expected: vec!["the", "quick", "brown", "foxes", "42"],
        },
        Row {
            case: "no analyzer named at all, which is the default chain",
            index: None,
            request: json!({ "text": text }),
            expected: vec!["the", "quick", "brown", "foxes", "42"],
        },
        Row {
            case: "the whitespace analyzer, which folds no case",
            index: None,
            request: json!({ "analyzer": "whitespace", "text": text }),
            expected: vec!["The", "Quick", "brown", "foxes", "42"],
        },
        Row {
            case: "the keyword analyzer, which splits nothing",
            index: None,
            request: json!({ "analyzer": "keyword", "text": text }),
            expected: vec!["The Quick brown foxes 42"],
        },
        Row {
            case: "the simple analyzer, which keeps only letters",
            index: None,
            request: json!({ "analyzer": "simple", "text": text }),
            expected: vec!["the", "quick", "brown", "foxes"],
        },
        Row {
            case: "the stop analyzer, which drops the published function words",
            index: None,
            request: json!({ "analyzer": "stop", "text": text }),
            expected: vec!["quick", "brown", "foxes"],
        },
        Row {
            case: "a chain the request composes itself",
            index: None,
            request: json!({
                "tokenizer": "whitespace",
                "filter": ["lowercase"],
                "text": text,
            }),
            expected: vec!["the", "quick", "brown", "foxes", "42"],
        },
        Row {
            case: "a tokenizer the request configures inline",
            index: None,
            request: json!({
                "tokenizer": { "type": "whitespace" },
                "text": text,
            }),
            expected: vec!["The", "Quick", "brown", "foxes", "42"],
        },
        Row {
            case: "a pattern tokenizer configured with the setting's own default",
            index: None,
            request: json!({
                "tokenizer": { "type": "pattern", "pattern": r"\W+" },
                "text": "quick-brown foxes",
            }),
            expected: vec!["quick", "brown", "foxes"],
        },
        Row {
            case: "a filter the request configures inline",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": [{ "type": "stop", "stopwords": ["the", "42"] }],
                "text": "the quick 42",
            }),
            expected: vec!["quick"],
        },
        Row {
            case: "a stop filter naming the predefined list of no words at all",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": [{ "type": "stop", "stopwords": "_none_" }],
                "text": "the quick 42",
            }),
            expected: vec!["the", "quick", "42"],
        },
        Row {
            case: "a stop filter naming a predefined language list",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": [{ "type": "stop", "stopwords": "_english_" }],
                "text": "the quick 42",
            }),
            expected: vec!["quick", "42"],
        },
        Row {
            case: "a stemmer filter naming a language this engine stems",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": ["lowercase", { "type": "stemmer", "language": "english" }],
                "text": "running foxes",
            }),
            expected: vec!["run", "fox"],
        },
        Row {
            case: "several texts, analyzed as one stream",
            index: None,
            request: json!({ "analyzer": "whitespace", "text": ["one two", "three"] }),
            expected: vec!["one", "two", "three"],
        },
        Row {
            case: "an analyzer the index defines, reading the package's file",
            index: Some("analyzed"),
            request: json!({ "analyzer": PACKAGED_ANALYZER, "text": "Fluffy clouds drift" }),
            expected: vec!["clouds", "drift"],
        },
        Row {
            case: "the analyzer a mapped field names",
            index: Some("analyzed"),
            request: json!({ "field": PACKAGED_FIELD, "text": "Fluffy clouds drift" }),
            expected: vec!["clouds", "drift"],
        },
        Row {
            case: "a field the mappings analyze with the engine's own default",
            index: Some("analyzed"),
            request: json!({ "field": CONTROL_FIELD, "text": "Fluffy clouds drift" }),
            expected: vec!["fluffy", "clouds", "drift"],
        },
        Row {
            case: "an index-scoped request naming one of the engine's analyzers",
            index: Some("analyzed"),
            request: json!({ "analyzer": "whitespace", "text": "Fluffy clouds" }),
            expected: vec!["Fluffy", "clouds"],
        },
    ];

    for row in rows {
        let answer = analyze(&emulator, domain, row.index, row.request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200 from _analyze, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let reported = tokens(&answer);
        assert_eq!(
            reported, row.expected,
            "case {:?}: expected {:?}, got {reported:?}",
            row.case, row.expected
        );
    }

    // One row's whole token record, so what the answer carries about a token — where it
    // came from, its type and its position — is pinned and not only its text.
    let answered = analyze(
        &emulator,
        domain,
        None,
        json!({ "analyzer": "standard", "text": "quick 42" }),
    )
    .await;
    assert_eq!(
        answered.body["tokens"],
        json!([
            {
                "token": "quick",
                "start_offset": 0,
                "end_offset": 5,
                "type": "<ALPHANUM>",
                "position": 0,
            },
            {
                "token": "42",
                "start_offset": 6,
                "end_offset": 8,
                "type": "<NUM>",
                "position": 1,
            },
        ]),
        "case \"the token record\": expected each token's offsets, type and position, got {:?}",
        answered.body
    );

    // The same request in the form a client sends with no body at all: the text and the
    // analyzer in the query string, where the text arrives percent-encoded.
    let parameters = rest_prefixed(
        &emulator,
        domain,
        "GET",
        "/_analyze?analyzer=whitespace&text=Quick%20brown",
        None,
    )
    .await;
    assert_eq!(
        (parameters.status, tokens(&parameters)),
        (200, vec!["Quick".to_string(), "brown".to_string()]),
        "case \"the text in the query string\": expected the parameter's decoded text to be \
         analyzed, got {} with {:?}",
        parameters.status,
        parameters.body
    );
}

/// RULE: a request naming an analyzer, a field or a filter nothing resolves is refused with
/// what the engine could not find, and an index the domain does not hold is its own
/// not-found — no request is answered with the default chain's output instead.
#[tokio::test]
async fn an_analyze_request_naming_what_nothing_resolves_is_refused() {
    let emulator = Emulator::new();
    let domain = "analyze-refusals";
    let package = "analyze-refusals-dictionary";
    let package_id = given_domain_and_package(&emulator, domain, package, "fluffy\n").await;
    let path = associated(&emulator, domain, &package_id, "the analyze refusals").await;
    created_index(
        &emulator,
        domain,
        "analyzed",
        &packaged_schema("stop", "stopwords_path", &path),
        "the index the refusal cases address",
    )
    .await;

    // One request the engine refuses while BUILDING the chain, before any text is
    // analyzed: what the request named, what the refusal is, and the sentence it states —
    // a request the path scoped to no index resolves names against the cluster's own
    // registry, which is what its sentence says.
    struct Refusal {
        case: &'static str,
        index: Option<&'static str>,
        request: Value,
        status: u16,
        error: &'static str,
        reason: &'static str,
    }
    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "an analyzer nothing registers",
            index: None,
            request: json!({ "analyzer": "nonesuch", "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find global analyzer [nonesuch]",
        },
        Refusal {
            case: "an analyzer the index does not define either",
            index: Some("analyzed"),
            request: json!({ "analyzer": "nonesuch", "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find analyzer [nonesuch]",
        },
        Refusal {
            case: "a tokenizer nothing registers",
            index: None,
            request: json!({ "tokenizer": "nonesuch", "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find global tokenizer under [nonesuch]",
        },
        Refusal {
            case: "a token filter nothing registers",
            index: None,
            request: json!({ "tokenizer": "standard", "filter": ["nonesuch"], "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find global token filter under [nonesuch]",
        },
        Refusal {
            case: "a tokenizer configured inline with no type at all",
            index: None,
            request: json!({ "tokenizer": { "pattern": "[-_]" }, "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find global tokenizer under [{\"pattern\":\"[-_]\"}]",
        },
        Refusal {
            case: "a pattern tokenizer configured with an expression this cluster does not \
                   split on",
            index: None,
            request: json!({
                "tokenizer": { "type": "pattern", "pattern": "[-_]" },
                "text": "a",
            }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "[pattern] does not support pattern [[-_]]",
        },
        Refusal {
            case: "a stop filter naming a predefined list this engine does not carry",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": [{ "type": "stop", "stopwords": "_klingon_" }],
                "text": "a",
            }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "[stop] does not support stopwords [_klingon_]",
        },
        Refusal {
            case: "a stemmer filter naming a language this engine ships no stemmer for",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": [{ "type": "stemmer", "language": "basque" }],
                "text": "a",
            }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "[stemmer] does not support language [basque]",
        },
        Refusal {
            case: "a token filter the index does not define either",
            index: Some("analyzed"),
            request: json!({ "tokenizer": "standard", "filter": ["nonesuch"], "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find token filter under [nonesuch]",
        },
        Refusal {
            case: "a field the mappings do not map",
            index: Some("analyzed"),
            request: json!({ "field": "absent", "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "failed to find field [absent]",
        },
        Refusal {
            case: "a field named with no index to resolve it against",
            index: None,
            request: json!({ "field": PACKAGED_FIELD, "text": "a" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "analysis based on a specific field requires an index",
        },
        Refusal {
            case: "no text at all",
            index: None,
            request: json!({ "analyzer": "standard" }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "text is missing",
        },
        Refusal {
            case: "more texts than one request may analyze",
            index: None,
            request: json!({
                "analyzer": "standard",
                "text": (0..101).map(|value| value.to_string()).collect::<Vec<_>>(),
            }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "a request may analyze at most 100 texts, and this one names 101",
        },
        Refusal {
            case: "more token filters than one chain may name",
            index: None,
            request: json!({
                "tokenizer": "standard",
                "filter": (0..33).map(|_| "lowercase").collect::<Vec<_>>(),
                "text": "a",
            }),
            status: 400,
            error: "illegal_argument_exception",
            reason: "a chain may name at most 32 token filters, and this one names 33",
        },
        Refusal {
            case: "an index the domain does not hold",
            index: Some("absent"),
            request: json!({ "analyzer": "standard", "text": "a" }),
            status: 404,
            error: "index_not_found_exception",
            reason: "no such index [absent]",
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = analyze(&emulator, domain, row.index, row.request).await;
        assert_eq!(
            (answer.status, error_type(&answer)),
            (row.status, row.error.to_string()),
            "case {case:?}: expected {} {}, got {} with {:?}",
            row.status,
            row.error,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["error"]["reason"].as_str(),
            Some(row.reason),
            "case {case:?}: expected the engine's own reason, got {:?}",
            answer.body
        );
    }

    // A method the route does not serve is the engine's own refusal, not an answer.
    let refused = rest_prefixed(&emulator, domain, "DELETE", "/analyzed/_analyze", None).await;
    assert_eq!(
        refused.status, 405,
        "case \"a method the analyze route does not serve\": expected 405, got {} with {:?}",
        refused.status, refused.body
    );
}

/// RULE: a query's BREADTH is bounded as its depth is — past the engine's clause ceiling a
/// search is refused rather than resolving an analysis chain for every field it named.
#[tokio::test]
async fn a_query_past_the_clause_ceiling_is_refused_rather_than_analyzed() {
    let emulator = Emulator::new();
    let domain = "analyze-clauses";
    let package = "analyze-clauses-dictionary";
    let package_id = given_domain_and_package(&emulator, domain, package, "fluffy\n").await;
    let path = associated(&emulator, domain, &package_id, "the clause ceiling").await;
    let index = "wide";
    created_index(
        &emulator,
        domain,
        index,
        &packaged_schema("stop", "stopwords_path", &path),
        "the index the clause cases search",
    )
    .await;

    // (the case, the clause form the query repeats, how many of them it carries, the status)
    // Both forms, because EVERY leaf clause is asked of every document the scan reaches:
    // the analyzed one a `match` is and the unanalyzed one a `term` is.
    let rows: &[(&str, &str, usize, u16)] = &[
        ("a handful of analyzed clauses", "match", 8, 200),
        ("as many analyzed clauses as the ceiling admits", "match", 1_024, 200),
        ("one analyzed clause past the ceiling", "match", 1_025, 400),
        ("far past the ceiling", "match", 4_096, 400),
        ("unanalyzed clauses past the ceiling", "term", 1_025, 400),
    ];
    for (case, form, clauses, status) in rows {
        let should: Vec<Value> = (0..*clauses)
            .map(|field| json!({ *form: { format!("field_{field}"): "clouds" } }))
            .collect();
        let answer = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/{index}/_search"),
            Some(json!({ "query": { "bool": { "should": should } } })),
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status} for {clauses} {form} clauses, got {} with {:?}",
            answer.status, answer.body
        );
        if *status != 200 {
            assert_eq!(
                error_type(&answer),
                "too_many_clauses",
                "case {case:?}: expected the engine's own clause-ceiling refusal, got {:?}",
                answer.body
            );
        }
    }

    // And ONE clause's own breadth is bounded the same way: past the engine's
    // `index.max_terms_count` the query is refused rather than comparing every term it
    // names against every document.
    let values: Vec<Value> = (0..65_537).map(|value| json!(value)).collect();
    let refused = rest_prefixed(
        &emulator,
        domain,
        "POST",
        &format!("/{index}/_search"),
        Some(json!({ "query": { "terms": { CONTROL_FIELD: values } } })),
    )
    .await;
    assert_eq!(
        (refused.status, error_type(&refused)),
        (400, "illegal_argument_exception".to_string()),
        "case \"a terms clause past the engine's term ceiling\": expected the refusal, got {} \
         with {:?}",
        refused.status,
        refused.body
    );
    assert!(
        refused.body["error"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("index.max_terms_count"),
        "case \"a terms clause past the engine's term ceiling\": expected the engine's own \
         sentence, got {:?}",
        refused.body
    );
}

/// RULE: resolving a query's chains is bounded too — a search asking about fields mapped to
/// more distinct analyzers than one index builds chains for is refused rather than
/// materialising one dictionary copy per analyzer.
#[tokio::test]
async fn a_query_naming_more_analyzers_than_one_index_builds_is_refused() {
    let emulator = Emulator::new();
    let domain = "analyze-chains";
    let package = "analyze-chains-dictionary";
    let package_id = given_domain_and_package(&emulator, domain, package, "fluffy\n").await;
    let path = associated(&emulator, domain, &package_id, "the chain ceiling").await;

    // One analyzer per field, each a chain of its own reading the SAME mounted file: what
    // the bound counts is the chains a scan builds, not the dictionaries behind them.
    let analyzers = 40;
    let mut analyzer_section = serde_json::Map::new();
    let mut properties = serde_json::Map::new();
    for index in 0..analyzers {
        analyzer_section.insert(
            format!("chain_{index}"),
            json!({ "type": "custom", "tokenizer": "standard", "filter": ["from_package"] }),
        );
        properties.insert(
            format!("field_{index}"),
            json!({ "type": "text", "analyzer": format!("chain_{index}") }),
        );
    }
    let index = "chained";
    created_index(
        &emulator,
        domain,
        index,
        &json!({
            "settings": {
                "analysis": {
                    "filter": { "from_package": { "type": "stop", "stopwords_path": path } },
                    "analyzer": Value::Object(analyzer_section),
                },
            },
            "mappings": { "properties": Value::Object(properties) },
        }),
        "the index whose fields name an analyzer each",
    )
    .await;

    // (the case, how many distinctly-analyzed fields the query asks about, the status)
    let rows: &[(&str, usize, u16)] = &[
        ("as many analyzers as one index builds chains for", 32, 200),
        ("one analyzer past that bound", 33, 400),
        ("far past that bound", 40, 400),
    ];
    for (case, fields, status) in rows {
        let should: Vec<Value> = (0..*fields)
            .map(|field| json!({ "match": { format!("field_{field}"): "clouds" } }))
            .collect();
        let answer = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/{index}/_search"),
            Some(json!({ "query": { "bool": { "should": should } } })),
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status} for {fields} analyzed fields, got {} with {:?}",
            answer.status, answer.body
        );
        if *status != 200 {
            assert_eq!(
                error_type(&answer),
                "illegal_argument_exception",
                "case {case:?}: expected the engine's own illegal-argument refusal, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a chain this engine cannot BUILD is refused, on the search path as on the analyze
/// path — never replaced by another chain, whose terms would answer a question the caller
/// did not ask.
#[tokio::test]
async fn a_chain_the_engine_cannot_build_is_refused_by_both_the_analyze_api_and_a_search() {
    let emulator = Emulator::new();
    let domain = "analyze-unbuildable";
    let package = "analyze-unbuildable-dictionary";
    let package_id = given_domain_and_package(&emulator, domain, package, "fluffy\n").await;
    associated(&emulator, domain, &package_id, "the unbuildable chains").await;

    // (the case, the analyzer the index defines, the sentence the engine states)
    // Each row's analyzer NAME resolves — the index defines it — so the create is served;
    // what cannot be built is the chain the definition states.
    let rows: &[(&str, &str, Value, &str)] = &[
        (
            "a tokenizer nothing registers",
            "broken-tokenizer",
            json!({ "type": "custom", "tokenizer": "nonesuch" }),
            "failed to find tokenizer under [nonesuch]",
        ),
        (
            "a token filter nothing registers",
            "broken-filter",
            json!({ "type": "custom", "tokenizer": "standard", "filter": ["nonesuch"] }),
            "failed to find token filter under [nonesuch]",
        ),
        (
            "a pattern tokenizer configured with an expression this cluster does not split on",
            "broken-pattern",
            json!({ "type": "custom", "tokenizer": "odd_pattern" }),
            "[pattern] does not support pattern [[-_]]",
        ),
    ];
    for (case, analyzer, definition, reason) in rows {
        let index = format!("index-{analyzer}");
        created_index(
            &emulator,
            domain,
            &index,
            &json!({
                "settings": {
                    "analysis": {
                        "tokenizer": {
                            "odd_pattern": { "type": "pattern", "pattern": "[-_]" },
                        },
                        "analyzer": { *analyzer: definition },
                    },
                },
                "mappings": {
                    "properties": { PACKAGED_FIELD: { "type": "text", "analyzer": analyzer } },
                },
            }),
            case,
        )
        .await;

        // The analyze API refuses it...
        let analyzed = analyze(
            &emulator,
            domain,
            Some(&index),
            json!({ "analyzer": analyzer, "text": "clouds" }),
        )
        .await;
        assert_eq!(
            (analyzed.status, error_type(&analyzed)),
            (400, "illegal_argument_exception".to_string()),
            "case {case:?}: expected _analyze to refuse the chain, got {} with {:?}",
            analyzed.status,
            analyzed.body
        );
        assert_eq!(
            analyzed.body["error"]["reason"].as_str(),
            Some(*reason),
            "case {case:?}: expected the engine's own reason from _analyze, got {:?}",
            analyzed.body
        );

        // ...and so does a search whose clause asks an analyzed question of that field.
        let searched = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/{index}/_search"),
            Some(json!({ "query": { "match": { PACKAGED_FIELD: "clouds" } } })),
        )
        .await;
        assert_eq!(
            (searched.status, error_type(&searched)),
            (400, "illegal_argument_exception".to_string()),
            "case {case:?}: expected the search to refuse the chain rather than answering \
             from another, got {} with {:?}",
            searched.status,
            searched.body
        );
        assert_eq!(
            searched.body["error"]["reason"].as_str(),
            Some(*reason),
            "case {case:?}: expected the search to state the same reason, got {:?}",
            searched.body
        );
    }
}

/// RULE: a dictionary file is read for the entries it holds — a line that states no rule
/// contributes none, and a file past what one package imports is read as far as it goes —
/// so malformed, truncated and oversized content is answered from what could be read rather
/// than failing the analysis or inventing entries.
#[tokio::test]
async fn a_malformed_or_oversized_dictionary_is_read_for_the_entries_it_holds() {
    let emulator = Emulator::new();
    let domain = "analyze-malformed";
    let package = "analyze-malformed-dictionary";
    // A comment, a blank line, a line of separators only, a rule with no input, a rule with
    // no output, a group of one term, one well-formed group — and then more lines than one
    // package imports, past the byte bound as well, so the file is both oversized and cut.
    let mut content = String::from(
        "# the rules this file states\n\n,,,\n=> nothing\nalpha =>\nsingleton,\n  beta , gamma  \n",
    );
    content.push_str(&"zzz\n".repeat(300_000));
    let package_id = given_domain_and_package(&emulator, domain, package, &content).await;
    let path = associated(&emulator, domain, &package_id, "the malformed dictionary").await;
    let index = "resilient";
    created_index(
        &emulator,
        domain,
        index,
        &packaged_schema("synonym", "synonyms_path", &path),
        "the index the malformed dictionary drives",
    )
    .await;

    // (the case, the text, the tokens the chain produces for it)
    let rows: &[(&str, &str, &[&str])] = &[
        ("a term the one well-formed group names", "beta", &["beta", "gamma"]),
        ("the other term of that group", "gamma", &["gamma", "beta"]),
        ("a term only a malformed line named", "alpha", &["alpha"]),
        ("a term only a one-term group named", "singleton", &["singleton"]),
        ("a term the file's own padding named", "zzz", &["zzz"]),
        ("a term the file does not name at all", "clouds", &["clouds"]),
    ];
    for (case, text, expected) in rows {
        let answered = analyze(
            &emulator,
            domain,
            Some(index),
            json!({ "analyzer": PACKAGED_ANALYZER, "text": text }),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected the analysis to be served, got {} with {:?}",
            answered.status, answered.body
        );
        let reported = tokens(&answered);
        assert_eq!(
            reported,
            expected.iter().map(|term| (*term).to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected {expected:?} for {text:?}, got {reported:?}"
        );
    }
}

/// RULE: the CONTENT of an associated TXT-DICTIONARY package is what a filter naming its
/// mount point reads, so that content decides both what the analyze API reports and what a
/// `match` query on a field using the chain finds — while a field the engine's own
/// `standard` analyzer analyzes is untouched by it.
#[tokio::test]
async fn an_associated_packages_content_drives_the_analyzer_and_the_search() {
    let emulator = Emulator::new();
    let domain = "analyze-stopwords";
    let package = "analyze-stopwords-dictionary";
    // One word per line, with a comment and a blank line the engine's word-list loader
    // drops — the form a dictionary file takes.
    let package_id = given_domain_and_package(
        &emulator,
        domain,
        package,
        "# the words this domain drops\nfluffy\ndrift\n\n",
    )
    .await;
    let path = associated(&emulator, domain, &package_id, "the package-driven chain").await;
    let index = "weather";
    created_index(
        &emulator,
        domain,
        index,
        &packaged_schema("stop", "stopwords_path", &path),
        "the index the package drives",
    )
    .await;

    // One document, whose text carries both the words the package lists and words it does
    // not, in BOTH fields — so the only difference between the two fields is the chain.
    let written = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        &format!("/{index}/_doc/1"),
        Some(json!({
            PACKAGED_FIELD: "Fluffy clouds drift over Seattle",
            CONTROL_FIELD: "Fluffy clouds drift over Seattle",
        })),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"the document the search finds\": expected 201, got {} with {:?}",
        written.status, written.body
    );

    // (the case, the term, the tokens the packaged chain leaves of it, the hits through the
    // packaged field, the hits through the control field)
    let rows: Vec<(&str, &str, Vec<&str>, i64, i64)> = vec![
        (
            "a word the package lists",
            "fluffy",
            vec![],
            0,
            1,
        ),
        (
            "another word the package lists",
            "drift",
            vec![],
            0,
            1,
        ),
        (
            "a word the package does not list",
            "clouds",
            vec!["clouds"],
            1,
            1,
        ),
        (
            "a word in another case, which the chain folds",
            "SEATTLE",
            vec!["seattle"],
            1,
            1,
        ),
        (
            "a word the document does not carry",
            "rain",
            vec!["rain"],
            0,
            0,
        ),
        (
            "a listed and an unlisted word together",
            "fluffy clouds",
            vec!["clouds"],
            1,
            1,
        ),
    ];

    for (case, term, expected, packaged_hits, control_hits) in rows {
        let analyzed = analyze(
            &emulator,
            domain,
            Some(index),
            json!({ "field": PACKAGED_FIELD, "text": term }),
        )
        .await;
        assert_eq!(
            (analyzed.status, tokens(&analyzed)),
            (
                200,
                expected.iter().map(|term| (*term).to_string()).collect()
            ),
            "case {case:?}: expected the packaged chain to leave {expected:?} of {term:?}, got \
             {} with {:?}",
            analyzed.status,
            analyzed.body
        );
        let through_package = hits(&emulator, domain, index, PACKAGED_FIELD, term).await;
        assert_eq!(
            through_package, packaged_hits,
            "case {case:?}: expected {packaged_hits} hits for {term:?} through the \
             package-driven field, got {through_package}"
        );
        let through_control = hits(&emulator, domain, index, CONTROL_FIELD, term).await;
        assert_eq!(
            through_control, control_hits,
            "case {case:?}: expected {control_hits} hits for {term:?} through the control \
             field, got {through_control}"
        );
    }

    // And the reads above changed nothing: the document is still there, found by the term
    // the first row found it by.
    let again = hits(&emulator, domain, index, PACKAGED_FIELD, "clouds").await;
    assert_eq!(
        again, 1,
        "case \"a search after the reads\": expected the reads to have changed nothing, got \
         {again}"
    );
}

/// RULE: a synonym rule the package's file holds makes a query term find the document
/// carrying the term it stands for, and the analyze API reports the synonym the rule
/// produced.
#[tokio::test]
async fn a_packaged_synonym_rule_finds_the_term_it_stands_for() {
    let emulator = Emulator::new();
    let domain = "analyze-synonyms";
    let package = "analyze-synonyms-dictionary";
    // Both forms the engine's synonym file takes: a group whose terms stand for each other,
    // and a rule that REPLACES its input with what follows the arrow.
    let package_id = given_domain_and_package(
        &emulator,
        domain,
        package,
        "clouds, cumulus\nseattle => emerald city\n",
    )
    .await;
    let path = associated(&emulator, domain, &package_id, "the packaged synonyms").await;
    let index = "forecast";
    created_index(
        &emulator,
        domain,
        index,
        &packaged_schema("synonym", "synonyms_path", &path),
        "the index the synonyms drive",
    )
    .await;
    let written = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        &format!("/{index}/_doc/1"),
        Some(json!({
            PACKAGED_FIELD: "Clouds over Seattle",
            CONTROL_FIELD: "Clouds over Seattle",
        })),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"the document the synonym finds\": expected 201, got {} with {:?}",
        written.status, written.body
    );

    // (the case, the term, the tokens the chain produces for it, the hits through the
    // packaged field, the hits through the control field)
    let rows: Vec<(&str, &str, Vec<&str>, i64, i64)> = vec![
        (
            "the term the document carries",
            "clouds",
            vec!["clouds", "cumulus"],
            1,
            1,
        ),
        (
            "the synonym of that term",
            "cumulus",
            vec!["cumulus", "clouds"],
            1,
            0,
        ),
        (
            "a term a rule replaces, which the document's own text is replaced by too",
            "seattle",
            vec!["emerald", "city"],
            1,
            1,
        ),
        (
            "a term no rule names and no document carries",
            "rain",
            vec!["rain"],
            0,
            0,
        ),
    ];

    for (case, term, expected, packaged_hits, control_hits) in rows {
        let analyzed = analyze(
            &emulator,
            domain,
            Some(index),
            json!({ "analyzer": PACKAGED_ANALYZER, "text": term }),
        )
        .await;
        let reported = tokens(&analyzed);
        assert_eq!(
            reported,
            expected.iter().map(|term| (*term).to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected the chain to produce {expected:?} for {term:?}, got \
             {reported:?}"
        );
        let through_package = hits(&emulator, domain, index, PACKAGED_FIELD, term).await;
        assert_eq!(
            through_package, packaged_hits,
            "case {case:?}: expected {packaged_hits} hits for {term:?} through the \
             synonym-driven field, got {through_package}"
        );
        let through_control = hits(&emulator, domain, index, CONTROL_FIELD, term).await;
        assert_eq!(
            through_control, control_hits,
            "case {case:?}: expected {control_hits} hits for {term:?} through the control \
             field, got {through_control}"
        );
    }
}
