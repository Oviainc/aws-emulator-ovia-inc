// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that a `NextToken` which names no position is no
//! token at all: the empty token the model admits (`NextToken` is len 0..4096, which
//! is what a bare `?NextToken=` binds to) is answered with the request's FIRST page,
//! by every read of this service that admits the member, while a token that does name
//! a position stays subject to that read's own recognition of it.
//!
//! The rule is checked over a TABLE that spans the reads the member is bound on
//! (`ListSpeechSynthesisTasks`, `ListLexicons`, `DescribeVoices` — every operation of
//! this service whose input carries `NextToken`) crossed with the other parameters
//! each read takes: every modelled `TaskStatus`, both published bounds of
//! `MaxResults`, a voice filter, and the empty token written both before and after
//! those parameters. The single `?NextToken=` call an issue quotes is one row.

use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::types::TASK_STATUS_VALUES;

use crate::lexicons_and_speech::{error_code, json, pls, Polly};
use crate::support::{fixture_task_id, seed_task, stored_task};

/// One GET of `path` with `query` appended.
async fn get(polly: &Polly, path: &str, query: &str) -> http::Response<Bytes> {
    polly.call("GET", &format!("{path}{query}"), "").await
}

/// The query string that carries `filters` and no `NextToken`, and the same one with
/// an EMPTY `NextToken` written first and written last. `filters` is a list of
/// `&`-prefixed parameters, so the two spellings differ only in where the empty token
/// sits.
fn spellings(filters: &str) -> (String, [String; 2]) {
    let without = if filters.is_empty() {
        String::new()
    } else {
        format!("?{}", filters.trim_start_matches('&'))
    };
    let with = [
        format!("?NextToken={filters}"),
        if filters.is_empty() {
            "?NextToken=".to_string()
        } else {
            format!("?{}&NextToken=", filters.trim_start_matches('&'))
        },
    ];
    (without, with)
}

/// Every read of this service whose input binds `NextToken`, each with the other
/// parameters it takes: one row per (path, the response member that carries the
/// result, filters) the rule has to hold for. The member is named so each case can
/// check its baseline really reports something, and is therefore not two empty lists
/// being compared.
fn read_table() -> Vec<(&'static str, &'static str, String)> {
    let mut rows: Vec<(&'static str, &'static str, String)> = Vec::new();

    // The paginated read: no bound, both published bounds of `MaxResults` (range
    // 1..100), a size in between, and every value of the model's `TaskStatus` enum
    // (`smithy_query(query="shape", shape="TaskStatus")`), alone and with a bound.
    for filters in ["", "&MaxResults=1", "&MaxResults=3", "&MaxResults=100"] {
        rows.push(("/v1/synthesisTasks", "SynthesisTasks", filters.to_string()));
    }
    for status in TASK_STATUS_VALUES {
        rows.push((
            "/v1/synthesisTasks",
            "SynthesisTasks",
            format!("&Status={status}"),
        ));
        rows.push((
            "/v1/synthesisTasks",
            "SynthesisTasks",
            format!("&MaxResults=2&Status={status}"),
        ));
    }

    // The unpaginated reads, which mint no token of their own and still take the
    // empty one as no token.
    rows.push(("/v1/lexicons", "Lexicons", String::new()));
    rows.push(("/v1/voices", "Voices", String::new()));
    rows.push(("/v1/voices", "Voices", "&Engine=standard".to_string()));
    rows.push((
        "/v1/voices",
        "Voices",
        "&LanguageCode=en-US&IncludeAdditionalLanguageCodes=true".to_string(),
    ));

    rows
}

/// Seed the state every row of the table reads: tasks in each modelled status, more
/// than the smallest page holds, and a couple of lexicons. Voices are the service's
/// own catalog.
async fn seed(polly: &Polly) {
    let now = SystemTime::now();
    for (index, status) in TASK_STATUS_VALUES.iter().enumerate() {
        for copy in 0..3u64 {
            let seconds = 100 - (index as u64) * 10 - copy;
            // A task id in the form the service mints one, which is the only form the
            // task reads accept.
            let mut task = stored_task(
                &fixture_task_id(&format!("empty-token-{status}-{copy}")),
                now - Duration::from_secs(seconds),
            );
            task.task_status = (*status).to_string();
            seed_task(polly, &task);
        }
    }
    for name in ["Alpha", "Beta"] {
        let response = polly.put_lexicon(name, &pls("ipa", "en-US", 1)).await;
        assert_eq!(
            response.status(),
            200,
            "seeding lexicon {name}: body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }
}

/// RULE: a `NextToken` that names no position is no token at all — every read that
/// binds the member answers the empty token with exactly the response it gives when
/// the member is absent, under any other parameters the read takes and wherever in
/// the query string the empty token is written.
#[tokio::test]
async fn an_empty_next_token_reads_as_no_token_on_every_read_that_binds_it() {
    let polly = Polly::new();
    seed(&polly).await;

    for (path, member, filters) in read_table() {
        let (without, with) = spellings(&filters);
        let expected = get(&polly, path, &without).await;
        assert_eq!(
            expected.status(),
            200,
            "case {path}{without:?}: the baseline read must succeed, body {:?}",
            String::from_utf8_lossy(expected.body())
        );
        let expected_body = json(&expected);
        // The comparison below is only a check if the baseline reports something, so
        // every row's result member has to carry at least one entry.
        assert!(
            expected_body[member]
                .as_array()
                .is_some_and(|reported| !reported.is_empty()),
            "case {path}{without:?}: the baseline must report at least one {member}, got \
             {expected_body:?}"
        );
        for query in with {
            let response = get(&polly, path, &query).await;
            assert_eq!(
                response.status(),
                200,
                "case {path}{query:?}: an empty NextToken must read as no token, got {} \
                 with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let body = json(&response);
            assert_eq!(
                body, expected_body,
                "case {path}{query:?}: an empty NextToken must answer the first page, \
                 expected {expected_body:?}, got {body:?}"
            );
        }
    }
}

/// RULE (the other side): the empty token names no position, so it does not resume one
/// — a read handed it restarts at the first page, while the token the read minted
/// still continues where it left off.
#[tokio::test]
async fn an_empty_next_token_restarts_rather_than_resumes() {
    let polly = Polly::new();
    seed(&polly).await;

    for filters in ["?MaxResults=1", "?MaxResults=2", "?MaxResults=2&Status=failed"] {
        let first = json(&get(&polly, "/v1/synthesisTasks", filters).await);
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {filters:?}: the seeded store needs a further page"))
            .to_string();

        let resumed = json(&get(&polly, "/v1/synthesisTasks", &format!("{filters}&NextToken={token}")).await);
        assert_ne!(
            resumed["SynthesisTasks"], first["SynthesisTasks"],
            "case {filters:?}: a minted token must continue the listing, got {resumed:?}"
        );

        let restarted = json(&get(&polly, "/v1/synthesisTasks", &format!("{filters}&NextToken=")).await);
        assert_eq!(
            restarted, first,
            "case {filters:?}: an empty token must restart at the first page, expected \
             {first:?}, got {restarted:?}"
        );
    }
}

/// RULE (the boundary): a token that DOES name a position is still judged by the read
/// it is presented to — one this service could not have minted, for that read and that
/// filter, is `InvalidNextTokenException` (HTTP 400), so reading the empty token as no
/// token does not make every token acceptable.
#[tokio::test]
async fn a_token_that_names_a_position_is_still_judged() {
    let polly = Polly::new();
    seed(&polly).await;

    // Tokens differing in form: prose, non-hex, hex too short to be a cursor, hex of
    // a cursor whose position does not parse, and one at the modelled maximum length
    // (`NextToken` is len 0..4096).
    let tokens = [
        "not-a-token".to_string(),
        "zzzz".to_string(),
        "ff".to_string(),
        hex::encode("\u{1f}task\u{1f}"),
        hex::encode("1\u{1f}x"),
        "a".repeat(4_096),
    ];
    for token in &tokens {
        for (path, _, filters) in read_table() {
            let query = format!("?NextToken={token}{filters}");
            let response = get(&polly, path, &query).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidNextTokenException".to_string()),
                "case {path}{query:?}: a token naming a position this read never minted \
                 must be refused, body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }
}
