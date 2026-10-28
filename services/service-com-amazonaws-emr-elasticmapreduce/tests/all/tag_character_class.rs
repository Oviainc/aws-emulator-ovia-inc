// SPDX-License-Identifier: CC0-1.0

//! What Amazon EMR SAYS when a tag carries a character a tag may not, end to end over
//! the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. A tag set is only ever read back through
//! `DescribeCluster` or `DescribeStudio`.
//!
//! THE RULE every table below asserts: a tag key or value carrying a character outside
//! the class Amazon EMR draws a tag from — Unicode letters, digits and whitespace plus
//! the symbols the refusal enumerates — is refused HTTP 400 `InvalidRequestException`
//! with the sentence that NAMES that class and quotes the rejected text, and every
//! character the enumerated class admits is stored and read back unchanged. The class the
//! sentence teaches and the class that is enforced are therefore one class.

use serde_json::json;
use service_com_amazonaws_emr_elasticmapreduce::state::TAG_PUNCTUATION;

use crate::job_flow_lifecycle::{assert_refused, kept_alive_request, Emr};
use crate::refusal_messages::message;
use crate::resource_tags::{
    add_tags_ok, assert_tags, create_studio, error_code, pair, tag_list, CLUSTER,
    INVALID_REQUEST_EXCEPTION, STUDIO,
};

/// The symbols the refusal's sentence enumerates, spelled out here rather than read from
/// the service: `_ . : / = + \ -` — the symbol half of the character class Amazon EMR
/// states for a tag, in the order the sentence lists them.
const ENUMERATED_SYMBOLS: [char; 8] = ['_', '.', ':', '/', '=', '+', '\\', '-'];

/// Characters OUTSIDE the class, spanning the Unicode categories it leaves out: symbols
/// (an emoji and a currency sign), punctuation the enumeration does not list (including
/// `@`, which the sentence does not name), a control character that is whitespace but no
/// separator, and a bracket.
///
/// SOURCE: the enumeration in the refusal's own sentence ([`ENUMERATED_SYMBOLS`]) plus
/// the letters/digits/whitespace half of the class — so every character here is one the
/// class admits nowhere. The rows test characters the issue did not cite beside the one
/// it did.
const OUT_OF_CLASS_CHARS: [char; 10] = ['🚀', '€', '@', '#', '!', '%', '*', ',', '\t', '{'];

/// The sentence Amazon EMR refuses an out-of-class tag key or value with: the member it
/// rejected, the text the request sent, and the class a tag is drawn from.
///
/// Spelled out here, not read from the service: `member` is the singular member name
/// (`key` or `value`) and the symbol enumeration is literal, so a drift in the service's
/// wording fails these cases.
pub(crate) fn tag_charset_refusal(member: &str, value: &str) -> String {
    format!(
        "Invalid tag {member}: '{value}'. Tag {member}s may only contain unicode letters, \
         digits, whitespace, or one of these symbols: '_ . : / = + \\ -'."
    )
}

/// Where in the text the offending character sits — the rule is about the character, not
/// its position, so each row is tried in every one of them.
fn placements(offender: char) -> Vec<(&'static str, String)> {
    vec![
        ("alone", offender.to_string()),
        ("leading", format!("{offender}trailing-text")),
        ("inside", format!("no-{offender}-go")),
        ("trailing", format!("leading-text{offender}")),
    ]
}

/// THE RULE: every character outside the class is refused, in either half of the tag, at
/// any position in the text, on either resource kind `AddTags` names — with the sentence
/// that states the class — and nothing of the refused request is stored.
#[tokio::test]
async fn an_out_of_class_tag_character_is_refused_with_the_character_class_sentence() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("charset")).await;
    let studio = create_studio(&emr, "charset-studio").await;
    // A tag the resources DO carry throughout, so every refusal below is a rejected tag
    // rather than a blanket refusal of the operation.
    let held = [pair("stack", Some("Prod"))];
    for resource_id in [&cluster, &studio] {
        add_tags_ok(&emr, "the held tag", resource_id, &held).await;
    }

    for offender in OUT_OF_CLASS_CHARS {
        for (placement, text) in placements(offender) {
            // Both halves of the pair: the sentence names the member it rejected.
            let rows = [
                ("value", tag_list(&[pair("charset", Some(&text))])),
                ("key", tag_list(&[pair(&text, Some("x"))])),
                // A bad tag BEHIND a good one is refused the same way.
                (
                    "value",
                    tag_list(&[pair("team", Some("data")), pair("charset", Some(&text))]),
                ),
            ];
            for (member, tags) in rows {
                for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
                    let case = format!(
                        "the character {offender:?} {placement} in a {member} on a {kind}"
                    );
                    let (status, payload) = emr
                        .call(
                            "AddTags",
                            json!({ "ResourceId": resource_id, "Tags": tags.clone() }),
                        )
                        .await;
                    assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
                    assert_eq!(
                        message(&case, &payload),
                        tag_charset_refusal(member, &text),
                        "case {case:?}: unexpected message in {payload}"
                    );
                    assert_eq!(
                        error_code(&payload),
                        None,
                        "case {case:?}: a rejected tag carries no ErrorCode, got {payload}"
                    );
                }
            }
        }
    }

    // Every refusal above left both resources carrying exactly the tag they were given.
    for (kind, resource_id) in [(CLUSTER, &cluster), (STUDIO, &studio)] {
        assert_tags(
            &emr,
            &format!("the held {kind} after the refusals"),
            kind,
            resource_id,
            &held,
        )
        .await;
    }
}

/// THE OTHER HALF of the rule: the class the sentence enumerates is the class that is
/// ENFORCED, so each symbol it names is accepted in both halves of a tag and reads back
/// unchanged, and the service's own list of them is that same enumeration.
#[tokio::test]
async fn every_symbol_the_sentence_enumerates_is_accepted_and_reads_back() {
    assert_eq!(
        TAG_PUNCTUATION, ENUMERATED_SYMBOLS,
        "case \"the enforced class\": the service must admit exactly the symbols its \
         refusal enumerates, got {TAG_PUNCTUATION:?}"
    );

    let emr = Emr::new();
    for symbol in ENUMERATED_SYMBOLS {
        let cluster = emr
            .run_job_flow(kept_alive_request(&format!("symbol-{}", symbol as u32)))
            .await;
        let case = format!("the symbol {symbol:?}");
        // Every placement of the symbol, in both halves of the pair, plus a letter and a
        // digit of another script beside it: the class is Unicode-wide.
        let tags: Vec<_> = placements(symbol)
            .into_iter()
            .map(|(placement, text)| {
                pair(
                    &format!("キー{placement}{symbol}9"),
                    Some(&format!("{text} значение 42")),
                )
            })
            .collect();
        add_tags_ok(&emr, &case, &cluster, &tags).await;
        assert_tags(&emr, &case, CLUSTER, &cluster, &tags).await;
    }
}

/// The sentence a caller reads is one sentence, whatever asked for it: the same refusal
/// text comes back for the same text however many times it is sent, and a value differing
/// only in the offending character differs only there.
#[tokio::test]
async fn the_character_class_sentence_is_stable_and_quotes_the_request_text() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("stable")).await;

    let mut said: Vec<String> = Vec::new();
    for offender in ['🚀', '@'] {
        let text = format!("no-{offender}-go");
        for attempt in 0..2 {
            let case = format!("the character {offender:?}, attempt {attempt}");
            let (status, payload) = emr
                .call(
                    "AddTags",
                    json!({
                        "ResourceId": &cluster,
                        "Tags": tag_list(&[pair("charset", Some(&text))]),
                    }),
                )
                .await;
            assert_refused(&case, status, &payload, INVALID_REQUEST_EXCEPTION);
            let message = message(&case, &payload);
            assert!(
                message.contains(&format!("'{text}'")),
                "case {case:?}: the sentence must quote the request's text, got {message:?}"
            );
            said.push(message);
        }
    }
    assert_eq!(
        said[0], said[1],
        "case \"repeated\": the same tag must be refused with the same sentence, got {said:?}"
    );
    assert_ne!(
        said[0], said[2],
        "case \"two offenders\": each refusal quotes its own text, got {said:?}"
    );
}

/// A tag the class admits goes on the resource even when an EARLIER call was refused: a
/// refusal of one request leaves the surface able to take the next.
#[tokio::test]
async fn a_refused_tag_does_not_stop_the_next_one() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("after-refusal")).await;

    let (status, payload) = emr
        .call(
            "AddTags",
            json!({
                "ResourceId": &cluster,
                "Tags": tag_list(&[pair("charset", Some("no-🚀-go"))]),
            }),
        )
        .await;
    assert_refused("the refused tag", status, &payload, INVALID_REQUEST_EXCEPTION);

    let accepted = [pair("charset", Some("ok+now/fine"))];
    add_tags_ok(&emr, "the tag after the refusal", &cluster, &accepted).await;
    assert_tags(
        &emr,
        "the tag after the refusal",
        CLUSTER,
        &cluster,
        &accepted,
    )
    .await;
}
