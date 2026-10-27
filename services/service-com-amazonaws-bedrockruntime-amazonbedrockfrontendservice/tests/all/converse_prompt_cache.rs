// SPDX-License-Identifier: CC0-1.0

//! Prompt caching on the Converse API, end to end over HTTP: what a
//! `cachePoint` marker does to the prompt it divides and to the token
//! accounting the answer reports.
//!
//! Every case drives the real router through the harness the other suites of
//! this target use, so what is asserted is the wire behaviour a client sees.
//!
//! Each rule is asserted over a SET of inputs: the tables differ along what the
//! rule generalises over (where the marker sits, how long the marked prefix is,
//! how many markers the request carries, which time to live they ask for, how a
//! later prefix differs from the cached one, which model it is prompted to), so
//! no row is the specification on its own.

use serde_json::{json, Value};

use crate::bedrock_runtime_inference::{emulator, post_json, tokens_of};
use crate::bedrock_runtime_streaming::decode_frames;

/// A model the registry serves and the Converse API supports.
const MODEL: &str = "anthropic.claude-3-5-sonnet-20241022-v2:0";

/// A second served model, for the rule that a cached prefix belongs to the
/// model it was prompted to.
const OTHER_MODEL: &str = "amazon.nova-lite-v1:0";

/// The question a turn asks after its marked prefix.
const ASK: &str = "Reply with the single word ALPHA.";

/// A prefix of `sections` sentences, each one distinct, so a prefix of any size
/// can be built and two prefixes of different sizes never share a prefix text.
fn prefix_of(sections: usize) -> String {
    (0..sections)
        .map(|section| {
            format!("Section {section}: the emulator persists every value it reports.")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A turn whose content is a cacheable prefix, a `cachePoint` marker and the
/// question after it.
///
/// Member names from `smithy_query(query="shape",
/// shape="ContentBlock,CachePointBlock,CachePointType,CacheTTL")` on this
/// service's model: the marker's `type` is required and its `ttl` optional.
fn marked_request(prefix: &str, suffix: &str, ttl: Option<&str>) -> Value {
    json!({
        "messages": [{"role": "user", "content": [
            {"text": prefix},
            {"cachePoint": cache_point(ttl)},
            {"text": suffix},
        ]}],
        "inferenceConfig": {"maxTokens": 64},
    })
}

/// One `cachePoint` block, with the time to live it asks for when it asks for
/// one.
fn cache_point(ttl: Option<&str>) -> Value {
    match ttl {
        Some(ttl) => json!({"type": "default", "ttl": ttl}),
        None => json!({"type": "default"}),
    }
}

/// The token accounting one answer reports: the four counters, and the
/// breakdown of what it wrote.
#[derive(Debug)]
struct Usage {
    input: i64,
    output: i64,
    total: i64,
    /// `None` when the answer reports no cache counter at all.
    read: Option<i64>,
    write: Option<i64>,
    details: Value,
}

/// The `usage` an answer reports, read off the wire.
fn usage_of(case: &str, usage: &Value) -> Usage {
    let number = |name: &str| {
        usage[name]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: {name} is a modelled integer in {usage}"))
    };
    Usage {
        input: number("inputTokens"),
        output: number("outputTokens"),
        total: number("totalTokens"),
        read: usage["cacheReadInputTokens"].as_i64(),
        write: usage["cacheWriteInputTokens"].as_i64(),
        details: usage["cacheDetails"].clone(),
    }
}

/// `totalTokens` closes over every counter the answer reports.
fn assert_total_closed(case: &str, usage: &Usage) {
    let expected =
        usage.input + usage.output + usage.read.unwrap_or_default() + usage.write.unwrap_or_default();
    assert_eq!(
        usage.total, expected,
        "case {case:?}: expected totalTokens {expected}, the sum of the counters reported, \
         got {usage:?}"
    );
}

/// `Converse` for one model, with the body as given.
async fn converse(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    model_id: &str,
    request: &Value,
) -> Usage {
    let call = post_json(router, &format!("/model/{model_id}/converse"), request).await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected Converse to answer 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    usage_of(case, &call.json()["usage"])
}

/// One request that marks a cacheable prefix, with the prompt pieces before and
/// after its last marker and the time to live the write is attributed to.
struct CacheRow {
    case: &'static str,
    model_id: &'static str,
    request: Value,
    /// The prompt pieces before the last marker, in prompt order.
    prefix: Vec<String>,
    /// The prompt pieces after it.
    tail: Vec<String>,
    /// The modelled `CacheTTL` the write is reported under.
    ttl: &'static str,
}

/// The places a marker can sit and the forms it can take: a message's content,
/// the system prompt, the tool list; a long prefix and a short one; one marker
/// and two; the default time to live and the explicit one-hour one; a marker
/// with nothing after it.
fn cache_rows() -> Vec<CacheRow> {
    let long = prefix_of(60);
    let short = prefix_of(1);
    let first = prefix_of(2);
    let second = "And one more fact the model should remember.".to_string();
    vec![
        CacheRow {
            case: "a marker in the message content, after a long prefix",
            model_id: MODEL,
            request: marked_request(&long, ASK, None),
            prefix: vec![long.clone()],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "a marker in the message content, after a one-sentence prefix",
            model_id: MODEL,
            request: marked_request(&short, ASK, None),
            prefix: vec![short.clone()],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "a marker asking for the one-hour time to live",
            model_id: OTHER_MODEL,
            request: marked_request(&long, ASK, Some("1h")),
            prefix: vec![long.clone()],
            tail: vec![ASK.to_string()],
            ttl: "1h",
        },
        CacheRow {
            case: "a marker asking for the five-minute time to live",
            model_id: MODEL,
            request: marked_request(&long, ASK, Some("5m")),
            prefix: vec![long.clone()],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "a marker at the end of the content, with nothing after it",
            model_id: MODEL,
            request: json!({
                "messages": [{"role": "user", "content": [
                    {"text": long},
                    {"cachePoint": cache_point(None)},
                ]}],
            }),
            prefix: vec![long.clone()],
            tail: Vec::new(),
            ttl: "5m",
        },
        CacheRow {
            case: "a marker in the system prompt",
            model_id: MODEL,
            request: json!({
                "system": [{"text": long}, {"cachePoint": cache_point(None)}],
                "messages": [{"role": "user", "content": [{"text": ASK}]}],
            }),
            prefix: vec![long.clone()],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "a marker in the tool list, after one tool definition",
            model_id: MODEL,
            request: json!({
                "toolConfig": {"tools": [
                    {"toolSpec": {
                        "name": "get_weather",
                        "description": "Look up the weather.",
                        "inputSchema": {"json": {"type": "object"}},
                    }},
                    {"cachePoint": cache_point(None)},
                ]},
                "messages": [{"role": "user", "content": [{"text": ASK}]}],
            }),
            // A tool reaches the model verbatim: its name, its description and
            // its input schema are each a piece of the prompt before the
            // marker.
            prefix: vec![
                "get_weather".to_string(),
                "Look up the weather.".to_string(),
                r#"{"type":"object"}"#.to_string(),
            ],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "two markers, the longer prefix ending at the second",
            model_id: MODEL,
            request: json!({
                "messages": [{"role": "user", "content": [
                    {"text": first},
                    {"cachePoint": cache_point(None)},
                    {"text": second},
                    {"cachePoint": cache_point(None)},
                    {"text": ASK},
                ]}],
            }),
            prefix: vec![first.clone(), second.clone()],
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
        CacheRow {
            case: "six markers, one after every piece of the prefix",
            model_id: MODEL,
            request: json!({
                "messages": [{"role": "user", "content": sections(6)
                    .into_iter()
                    .flat_map(|text| [json!({"text": text}), json!({"cachePoint": cache_point(None)})])
                    .chain([json!({"text": ASK})])
                    .collect::<Vec<Value>>()}],
            }),
            prefix: sections(6),
            tail: vec![ASK.to_string()],
            ttl: "5m",
        },
    ]
}

/// `count` distinct sentences, one per content block.
fn sections(count: usize) -> Vec<String> {
    (0..count)
        .map(|section| format!("Part {section}: every counter is computed from state."))
        .collect()
}

/// The tokens a set of prompt pieces is.
fn tokens(pieces: &[String]) -> i64 {
    tokens_of(&pieces.iter().map(String::as_str).collect::<Vec<_>>())
}

/// RULE: a `cachePoint` block ends a cacheable section of the prompt, so the
/// first call that carries it reports the tokens of everything before the
/// marker as `usage.cacheWriteInputTokens`, a later call whose marked prefix is
/// identical reports exactly those tokens as `usage.cacheReadInputTokens` and
/// writes nothing, `usage.inputTokens` counts only the prompt after the last
/// marker on both, and `usage.totalTokens` closes over all four counters.
#[tokio::test]
async fn a_marked_prefix_is_written_on_the_first_call_and_read_back_on_a_replay() {
    for row in cache_rows() {
        let case = row.case;
        // A fresh emulator per row, so each row's first call is a cold one.
        let router = emulator();
        let prefix_tokens = tokens(&row.prefix);
        let tail_tokens = tokens(&row.tail);
        assert!(
            prefix_tokens > 0,
            "case {case:?}: the row must mark a prefix that carries text, got {prefix_tokens}"
        );

        let cold = converse(&router, case, row.model_id, &row.request).await;
        assert_eq!(
            cold.write,
            Some(prefix_tokens),
            "case {case:?}: expected the marked prefix ({prefix_tokens} tokens) to be \
             written to the cache, got {cold:?}"
        );
        assert_eq!(
            cold.read,
            Some(0),
            "case {case:?}: expected nothing to be read from the cache on the first call, \
             got {cold:?}"
        );
        assert_eq!(
            cold.input, tail_tokens,
            "case {case:?}: expected only the {tail_tokens} tokens after the marker to be \
             charged as input, got {cold:?}"
        );
        assert_eq!(
            cold.details,
            json!([{"ttl": row.ttl, "inputTokens": prefix_tokens}]),
            "case {case:?}: expected the write to be broken down under the time to live \
             {:?}, got {cold:?}",
            row.ttl
        );
        assert_total_closed(case, &cold);

        let warm = converse(&router, case, row.model_id, &row.request).await;
        assert_eq!(
            warm.read,
            Some(prefix_tokens),
            "case {case:?}: expected the replay to read the {prefix_tokens} cached tokens, \
             got {warm:?}"
        );
        assert_eq!(
            warm.write,
            Some(0),
            "case {case:?}: expected the replay to write nothing, got {warm:?}"
        );
        assert_eq!(
            warm.input, cold.input,
            "case {case:?}: expected the replay to be charged the same {} input tokens, \
             got {warm:?}",
            cold.input
        );
        assert_eq!(
            warm.details,
            json!([]),
            "case {case:?}: expected no cache-creation breakdown on a replay, got {warm:?}"
        );
        assert_total_closed(case, &warm);
    }
}

/// RULE: a cache-point marker's time to live is one of the values the model
/// declares for it, so a request naming any other is refused with the
/// operation's `ValidationException` instead of being cached under a duration
/// the model does not carry.
///
/// The value set is `CacheTTL`'s (`smithy_query(query="shape",
/// shape="CacheTTL")`): `5m` and `1h`.
#[tokio::test]
async fn a_marker_naming_a_time_to_live_outside_the_modelled_set_is_refused() {
    let router = emulator();
    let prefix = prefix_of(2);

    for ttl in ["7d", "5M", "300s", "1 h"] {
        let call = post_json(
            &router,
            &format!("/model/{MODEL}/converse"),
            &marked_request(&prefix, ASK, Some(ttl)),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {ttl:?}: expected the unmodelled time to live to be refused with 400, \
             got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {ttl:?}: expected a ValidationException, got {}",
            String::from_utf8_lossy(&call.body)
        );
    }

    for ttl in ["5m", "1h"] {
        // A distinct prefix per row, so each one is cached by its own call
        // rather than read back from the previous row's.
        let marked = format!("{prefix} Asked for {ttl}.");
        let usage = converse(&router, ttl, MODEL, &marked_request(&marked, ASK, Some(ttl))).await;
        assert_eq!(
            usage.details,
            json!([{"ttl": ttl, "inputTokens": tokens_of(&[marked.as_str()])}]),
            "case {ttl:?}: expected the write to be reported under the time to live asked \
             for, got {usage:?}"
        );
    }
}

/// One later call against a cache holding one prefix, and what it reads and
/// writes.
struct ScopeRow {
    case: &'static str,
    model_id: &'static str,
    request: Value,
    /// The prefix tokens the call reads from the cache.
    read: i64,
    /// The prefix tokens it writes to it.
    write: i64,
    /// The input tokens it is charged.
    input: i64,
}

/// RULE: a cached section is matched on the whole of the marked prefix and the
/// model it was prompted to, so a prefix that diverges anywhere — its first
/// token included — or the same prefix sent to another model reads nothing and
/// caches itself instead, while a changed prompt AFTER the marker still reads
/// the cached prefix.
#[tokio::test]
async fn a_cache_hit_needs_the_whole_marked_prefix_and_the_same_model() {
    let router = emulator();
    let base = prefix_of(30);
    let cold = converse(
        &router,
        "the first call",
        MODEL,
        &marked_request(&base, ASK, None),
    )
    .await;
    let base_tokens = tokens_of(&[base.as_str()]);
    assert_eq!(
        cold.write,
        Some(base_tokens),
        "case \"the first call\": expected the prefix to be cached, got {cold:?}"
    );

    let leading = format!("PREAMBLE {base}");
    let trailing = format!("{base} And a closing remark.");
    let other_ask = "Reply with the single word BETA.";
    let rows = vec![
        ScopeRow {
            case: "the identical request",
            model_id: MODEL,
            request: marked_request(&base, ASK, None),
            read: base_tokens,
            write: 0,
            input: tokens_of(&[ASK]),
        },
        ScopeRow {
            case: "an extra token before the prefix",
            model_id: MODEL,
            request: marked_request(&leading, ASK, None),
            read: 0,
            write: tokens_of(&[leading.as_str()]),
            input: tokens_of(&[ASK]),
        },
        ScopeRow {
            case: "an extra token at the end of the prefix",
            model_id: MODEL,
            request: marked_request(&trailing, ASK, None),
            read: 0,
            write: tokens_of(&[trailing.as_str()]),
            input: tokens_of(&[ASK]),
        },
        ScopeRow {
            case: "the same prefix with a different question after the marker",
            model_id: MODEL,
            request: marked_request(&base, other_ask, None),
            read: base_tokens,
            write: 0,
            input: tokens_of(&[other_ask]),
        },
        ScopeRow {
            case: "the same prefix prompted to another model",
            model_id: OTHER_MODEL,
            request: marked_request(&base, ASK, None),
            read: 0,
            write: base_tokens,
            input: tokens_of(&[ASK]),
        },
    ];

    for row in rows {
        let case = row.case;
        let usage = converse(&router, case, row.model_id, &row.request).await;
        assert_eq!(
            usage.read,
            Some(row.read),
            "case {case:?}: expected {} tokens read from the cache, got {usage:?}",
            row.read
        );
        assert_eq!(
            usage.write,
            Some(row.write),
            "case {case:?}: expected {} tokens written to the cache, got {usage:?}",
            row.write
        );
        assert_eq!(
            usage.input, row.input,
            "case {case:?}: expected {} input tokens, got {usage:?}",
            row.input
        );
        assert_total_closed(case, &usage);
    }
}

/// RULE: only the part of the prompt that is not cached yet is written, so a
/// call whose earlier marker matches a cached prefix reads that prefix and
/// writes just the section between it and its last marker.
#[tokio::test]
async fn a_call_extending_a_cached_prefix_writes_only_the_new_section() {
    let router = emulator();
    let first = prefix_of(20);
    let extension = prefix_of(5).replace("Section", "Appendix");

    let cold = converse(
        &router,
        "the cached prefix",
        MODEL,
        &marked_request(&first, ASK, None),
    )
    .await;
    let first_tokens = tokens_of(&[first.as_str()]);
    assert_eq!(
        cold.write,
        Some(first_tokens),
        "case \"the cached prefix\": expected the prefix to be cached, got {cold:?}"
    );

    let case = "a second marker after the cached prefix";
    let extended = json!({
        "messages": [{"role": "user", "content": [
            {"text": first},
            {"cachePoint": cache_point(None)},
            {"text": extension},
            {"cachePoint": cache_point(None)},
            {"text": ASK},
        ]}],
    });
    let usage = converse(&router, case, MODEL, &extended).await;
    assert_eq!(
        usage.read,
        Some(first_tokens),
        "case {case:?}: expected the {first_tokens} already-cached tokens to be read, \
         got {usage:?}"
    );
    let extension_tokens = tokens_of(&[extension.as_str()]);
    assert_eq!(
        usage.write,
        Some(extension_tokens),
        "case {case:?}: expected only the {extension_tokens} tokens of the new section to \
         be written, got {usage:?}"
    );
    assert_eq!(
        usage.input,
        tokens_of(&[ASK]),
        "case {case:?}: expected only the prompt after the last marker to be charged as \
         input, got {usage:?}"
    );
    assert_total_closed(case, &usage);
}

/// RULE: a request that marks no cacheable prefix engages no cache, so it
/// reports no cache counters at all and `usage.totalTokens` is its input and
/// output counts.
#[tokio::test]
async fn a_turn_that_marks_no_prefix_reports_no_cache_counters() {
    let router = emulator();
    let rows = vec![
        (
            "one plain user turn",
            json!({"messages": [{"role": "user", "content": [{"text": ASK}]}]}),
        ),
        (
            "a system prompt and a user turn",
            json!({
                "system": [{"text": "You are terse."}],
                "messages": [{"role": "user", "content": [{"text": ASK}]}],
            }),
        ),
        (
            "a tool configuration with no marker in it",
            json!({
                "toolConfig": {"tools": [{"toolSpec": {
                    "name": "get_weather",
                    "description": "Look up the weather.",
                    "inputSchema": {"json": {"type": "object"}},
                }}]},
                "messages": [{"role": "user", "content": [{"text": ASK}]}],
            }),
        ),
        (
            "a marker that delimits no prompt text at all",
            json!({
                "messages": [{"role": "user", "content": [
                    {"cachePoint": cache_point(None)},
                    {"text": ASK},
                ]}],
            }),
        ),
    ];

    for (case, request) in rows {
        let usage = converse(&router, case, MODEL, &request).await;
        assert_eq!(
            usage.read, None,
            "case {case:?}: expected no cacheReadInputTokens, got {usage:?}"
        );
        assert_eq!(
            usage.write, None,
            "case {case:?}: expected no cacheWriteInputTokens, got {usage:?}"
        );
        assert!(
            usage.details.is_null(),
            "case {case:?}: expected no cacheDetails, got {usage:?}"
        );
        assert!(
            usage.input > 0,
            "case {case:?}: expected the whole prompt to be charged as input, got {usage:?}"
        );
        assert_total_closed(case, &usage);
    }
}

/// RULE: `ConverseStream` serves the turn `Converse` serves, so its `metadata`
/// event reports the same prompt-cache accounting: the marked prefix written on
/// the first call and read back on a replay.
#[tokio::test]
async fn converse_stream_reports_the_prompt_cache_accounting_in_its_metadata_event() {
    let router = emulator();
    let prefix = prefix_of(40);
    let request = marked_request(&prefix, ASK, None);
    let prefix_tokens = tokens_of(&[prefix.as_str()]);

    for (case, expected_read, expected_write) in [
        ("a cold stream", 0, prefix_tokens),
        ("a replayed stream", prefix_tokens, 0),
    ] {
        let call = post_json(
            &router,
            &format!("/model/{MODEL}/converse-stream"),
            &request,
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected ConverseStream to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let frames = decode_frames(case, &call.body);
        let metadata = frames
            .iter()
            .find(|(name, _)| name == "metadata")
            .map(|(_, payload)| payload.clone())
            .unwrap_or_else(|| panic!("case {case:?}: no metadata frame in the stream"));
        let usage = usage_of(case, &metadata["usage"]);
        assert_eq!(
            usage.read,
            Some(expected_read),
            "case {case:?}: expected {expected_read} tokens read from the cache, got {usage:?}"
        );
        assert_eq!(
            usage.write,
            Some(expected_write),
            "case {case:?}: expected {expected_write} tokens written to the cache, \
             got {usage:?}"
        );
        assert_eq!(
            usage.input,
            tokens_of(&[ASK]),
            "case {case:?}: expected only the prompt after the marker to be charged as \
             input, got {usage:?}"
        );
        assert_total_closed(case, &usage);
    }
}
