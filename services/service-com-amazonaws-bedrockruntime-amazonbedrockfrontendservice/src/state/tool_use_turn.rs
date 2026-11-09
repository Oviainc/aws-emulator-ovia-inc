// SPDX-License-Identifier: CC0-1.0

//! Which tool request a Converse-API turn makes, for a request that OFFERS tools.
//!
//! `toolConfig.toolChoice` says how much of the decision is the model's (the modelled
//! `ToolChoice` union of this service): `any` obliges the turn to request
//! some tool, `tool` obliges it to request the one named, and `auto` — the default, and
//! what a request carrying no `toolChoice` at all gets — leaves the decision to the model.
//! Whichever way the choice falls, a turn that requests a tool ends on the `tool_use` stop
//! reason with a `toolUse` content block, which the caller answers with a `toolResult`
//! block in the next user turn; the turn that is handed that result answers in text.
//!
//! `auto` therefore cannot mean "never request a tool": the tool-use protocol is a ROUND
//! TRIP, and a turn that never asks for a tool leaves a client waiting for a `toolUse`
//! block that never arrives, with the result-bearing second turn unreachable. What decides
//! it here is the state of the CONVERSATION, which is the one thing about the request that
//! says whether the offered tool has already been called:
//!
//! * the latest turn is a user prompt that answers no tool request -> the tool the request
//!   offers has not been called for this prompt yet, so the turn requests it;
//! * the latest turn carries a `toolResult` -> the tool's result is in hand, so the turn
//!   answers in text and the conversation ends on `end_turn`.
//!
//! No language model runs in this emulator, so nothing here is a judgement about whether a
//! prompt NEEDS a tool: the decision is a function of the conversation and of the tools the
//! request offers, and the arguments the request carries are the instantiation of the
//! schema that tool was DECLARED with
//! ([`super::tool_input_arguments`]) rather than values a model inferred.

use std::collections::HashMap;

use crate::ops::types::{ContentBlock, Message, ToolResultContentBlock};

/// The most tool-result clauses one turn states.
///
/// The EMULATOR's bound, so the number of `toolResult` blocks a request carries cannot
/// decide how much text one turn composes: a clause past the generation budget
/// ([`super::emulated_completion_with`]) is discarded anyway, so nothing a caller could
/// read is lost by stopping here.
const MAX_TOOL_RESULT_CLAUSES: usize = 8;

/// The most characters of ONE tool result a clause quotes, so a large result cannot grow
/// the answer without limit. The EMULATOR's bound, for the same reason as above.
const MAX_QUOTED_RESULT_CHARS: usize = 512;

/// The most tool requests the id index below holds: one entry per `toolUse` block, so the
/// index cannot grow with the conversation without limit. The EMULATOR's bound; a clause
/// whose id is not in the index names no tool rather than scanning for it.
const MAX_INDEXED_TOOL_REQUESTS: usize = 256;

/// Whether the conversation's latest turn is one that has already answered a tool request:
/// a message carrying at least one `toolResult` content block.
///
/// Only the LATEST turn counts. An earlier tool exchange in the same conversation is
/// finished business — the client went on to ask something else — while the turn in front
/// of the model is the one this answer is for.
#[must_use]
pub fn latest_turn_answers_a_tool_request(messages: Option<&Vec<Message>>) -> bool {
    messages
        .and_then(|messages| messages.last())
        .is_some_and(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::toolResult(_)))
        })
}

/// What the turn handed a tool's result STATES about it: one clause per `toolResult` the
/// latest turn carries, naming the tool that was requested and the result it came back
/// with.
///
/// The point of the round trip is that the answer USES the tool's result: a client calls
/// the tool the turn requested and hands the outcome back so the next turn can act on it,
/// so an answer that says nothing about it leaves the exchange pointless. No model runs
/// here, so the clause is not prose about the result: it REPORTS the result the
/// conversation carries, beside the notice that no model ran
/// ([`super::generation`]) — a fact computed from the conversation, which is why it is
/// never mistaken for inference.
///
/// The tool a result answers is the one the `toolUse` block it is addressed to requested
/// ([`super::validate_tool_results`] has already refused a result addressed to no request
/// the conversation carries), so the clause names that tool. The conversation is walked
/// ONCE for that, into the id index below, so the work is linear in the request rather than
/// one scan per result; and both the number of clauses and the characters each one quotes
/// are bounded above, so neither the number of results a request carries nor their size
/// decides how much work this does.
#[must_use]
pub fn tool_result_clauses(messages: Option<&Vec<Message>>) -> Vec<String> {
    let Some(messages) = messages else {
        return Vec::new();
    };
    let Some(latest) = messages.last() else {
        return Vec::new();
    };
    let requested = requested_tool_names(messages);
    latest
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::toolResult(result) => Some(result),
            _ => None,
        })
        .take(MAX_TOOL_RESULT_CLAUSES)
        .map(|result| {
            let rendered = rendered_result(&result.content);
            match requested.get(result.tool_use_id.as_str()) {
                Some(name) => format!(" The tool {name} returned {rendered}."),
                None => format!(" The tool returned {rendered}."),
            }
        })
        .collect()
}

/// The tool each request of the conversation was issued for, keyed by the `toolUseId` it
/// was issued under: ONE pass over the messages, holding at most
/// [`MAX_INDEXED_TOOL_REQUESTS`] entries.
fn requested_tool_names(messages: &[Message]) -> HashMap<&str, &str> {
    let mut requested: HashMap<&str, &str> = HashMap::new();
    for block in messages.iter().flat_map(|message| message.content.iter()) {
        if requested.len() >= MAX_INDEXED_TOOL_REQUESTS {
            break;
        }
        if let ContentBlock::toolUse(tool_use) = block {
            requested.insert(
                tool_use.tool_use_id.as_str(),
                tool_use.name.as_str(),
            );
        }
    }
    requested
}

/// One tool result's content, as the clause above quotes it: the text of a text block and
/// the compact JSON of a JSON one, in the order the result lists them. A block of another
/// kind (an image, a document, a video, a search result) carries no text to quote, so it is
/// reported by its KIND rather than by inventing a rendering for its bytes.
///
/// Quoted up to [`MAX_QUOTED_RESULT_CHARS`] characters, cut on a character boundary: what
/// the clause states about a result is bounded whatever size the result is.
fn rendered_result(content: &[ToolResultContentBlock]) -> String {
    let mut rendered = String::new();
    for block in content {
        if rendered.chars().count() >= MAX_QUOTED_RESULT_CHARS {
            break;
        }
        let piece = match block {
            ToolResultContentBlock::Text(text) => text.clone(),
            ToolResultContentBlock::Json(document) => {
                super::document_to_json(document).to_string()
            }
            ToolResultContentBlock::Image(_) => "an image".to_string(),
            ToolResultContentBlock::Document(_) => "a document".to_string(),
            ToolResultContentBlock::Video(_) => "a video".to_string(),
            ToolResultContentBlock::searchResult(_) => "a search result".to_string(),
        };
        if piece.is_empty() {
            continue;
        }
        if !rendered.is_empty() {
            rendered.push(' ');
        }
        let room = MAX_QUOTED_RESULT_CHARS.saturating_sub(rendered.chars().count());
        rendered.extend(piece.chars().take(room));
    }
    if rendered.is_empty() {
        return "no content".to_string();
    }
    rendered
}
