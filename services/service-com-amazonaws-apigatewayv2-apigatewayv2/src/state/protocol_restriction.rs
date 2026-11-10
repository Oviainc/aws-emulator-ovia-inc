// SPDX-License-Identifier: CC0-1.0

//! The `ProtocolType` gate: which request members and child resources an api of each
//! protocol supports, and the sentence the service answers a request that asks for one
//! of the other protocol's features with.
//!
//! A module of its own under [`crate::state`], because this is one concern several
//! operations share: every writer of a protocol-restricted member (`CreateRoute` /
//! `UpdateRoute`, `CreateIntegration` / `UpdateIntegration`, `CreateStage` /
//! `UpdateStage`, `ReimportApi`) refuses through the one gate here, so two operations
//! that share a member cannot disagree about whether it is refused, or about the words
//! the refusal carries.
//!
//! API Gateway words the refusal per restricted thing rather than from one template:
//!
//! * a whole child resource is named in the plural and "not permitted"
//!   ("Currently, Models are not permitted for APIs with a protocol type of HTTP");
//! * a member that configures behaviour the other protocol does not have is named as the
//!   request spells it and is likewise "not permitted";
//! * a member that only SELECTS something — a selection expression — is named as the
//!   service's prose names it ("the route response selection expression for the route",
//!   `CreateRoute$RouteResponseSelectionExpression`) and is "not necessary", because such
//!   an expression would simply never be evaluated under the other protocol.
//!
//! Which sentence a member takes is therefore a property of the MEMBER, carried on
//! [`ProtocolRestrictedMember`], not of the operation that happened to receive it.

use super::*;

/// How API Gateway words the refusal of one [`ProtocolRestrictedMember`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRefusal {
    /// The member configures behaviour the other protocol has no implementation of, so
    /// the api MAY NOT carry it: the sentence names the member as the request spells it
    /// and reads "<Member> is not permitted".
    NotPermitted,
    /// The member is a selection expression: it only selects, so under the other protocol
    /// there is nothing for it to select and no behaviour to forbid. The sentence names
    /// the feature in prose, as the member's own documentation words it, and reads
    /// "a <feature> is not necessary".
    NotNecessary,
}

/// One request member the model restricts to APIs of a single `ProtocolType`.
///
/// API Gateway v2 serves two protocols from one control plane, and a member's own
/// documentation is what says which of them supports it ("Supported only for WebSocket
/// APIs", "Supported only for HTTP APIs"). A request that configures such a member on an
/// api of the OTHER protocol asks for a resource that protocol has no behaviour for, so
/// the service refuses it rather than storing a setting nothing could ever apply.
#[derive(Debug, Clone, Copy)]
pub struct ProtocolRestrictedMember<'a> {
    /// The member's name as a request spells it, for the refusal message.
    pub member: &'a str,
    /// The one `ProtocolType` whose APIs support the member, from
    /// `PROTOCOL_TYPE_VALUES`.
    pub supported_by: &'a str,
    /// Does THIS request configure the member? A member the request left out asks for
    /// nothing, and neither does one set to the value it would carry anyway — an empty
    /// map or list, or a `false` flag whose documented default is `false` — so a caller
    /// may send those to an api of either protocol.
    pub configured: bool,
    /// Which of the service's two sentences this member's refusal carries.
    pub refusal: ProtocolRefusal,
}

impl<'a> ProtocolRestrictedMember<'a> {
    /// A member whose documentation reads "Supported only for WebSocket APIs".
    pub fn websocket_only(member: &'a str, configured: bool) -> Self {
        Self {
            member,
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            configured,
            refusal: ProtocolRefusal::NotPermitted,
        }
    }

    /// A member whose documentation reads "Supported only for HTTP APIs".
    pub fn http_only(member: &'a str, configured: bool) -> Self {
        Self {
            member,
            supported_by: HTTP_PROTOCOL_TYPE,
            configured,
            refusal: ProtocolRefusal::NotPermitted,
        }
    }

    /// A SELECTION EXPRESSION whose documentation reads "Supported only for WebSocket
    /// APIs" — a member whose value selects a WebSocket resource (a model, a response, a
    /// request template), so an api of the other protocol has nothing for it to select
    /// and the refusal reads "is not necessary" (see [`ProtocolRefusal`]).
    pub fn websocket_only_selection_expression(member: &'a str, configured: bool) -> Self {
        Self {
            refusal: ProtocolRefusal::NotNecessary,
            ..Self::websocket_only(member, configured)
        }
    }

    /// The sentence this member's refusal carries, for an api of `protocol_type`.
    fn refusal_message(&self, protocol_type: &str) -> String {
        match self.refusal {
            ProtocolRefusal::NotPermitted => format!(
                "Currently, {} is not permitted for APIs with a protocol type of \
                 {protocol_type}",
                self.member
            ),
            // The clause this form carries ("for APIs with  a protocol type of") is the
            // service's own, doubled space and all; it is fixed text, so it is
            // reproduced rather than normalised.
            ProtocolRefusal::NotNecessary => format!(
                "Currently, {} is not necessary for APIs with  a protocol type of \
                 {protocol_type}",
                feature_phrase(self.member)
            ),
        }
    }
}

/// A member named as the service's prose names it, with the article the sentence carries:
/// `RouteResponseSelectionExpression` reads "a route response selection expression", the
/// phrase the member's own documentation uses ("The route response selection expression
/// for the route").
///
/// Derived from the member name rather than listed per member, so the phrase cannot drift
/// from the member it names, and so a member added to the gate later needs no second
/// table.
fn feature_phrase(member: &str) -> String {
    let words = member_words(member);
    // English takes "an" before a vowel sound; every member of this gate so far begins
    // with a consonant, and spelling the choice out keeps the sentence grammatical for
    // one that does not.
    let article = if words.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {words}")
}

/// The camel-case `member` as lower-case words: `ModelSelectionExpression` becomes
/// "model selection expression".
fn member_words(member: &str) -> String {
    let mut words = String::with_capacity(member.len() + 4);
    for (index, character) in member.char_indices() {
        if index != 0 && character.is_ascii_uppercase() {
            words.push(' ');
        }
        words.push(character.to_ascii_lowercase());
    }
    words
}

/// Refuse a request that configures a [`ProtocolRestrictedMember`] on an api of another
/// protocol.
///
/// Every writer of a protocol-restricted member routes through here, so the gate cannot
/// differ between the operations that share a member, and an api of the supporting
/// protocol is never refused. The FIRST offending member is the one reported — a request
/// fixing it is checked again.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: the member's own
/// sentence (see [`ProtocolRefusal`]), carrying the protocol type of the api the request
/// was aimed at.
pub fn validate_protocol_restricted_members(
    protocol_type: &str,
    members: &[ProtocolRestrictedMember<'_>],
) -> Result<(), String> {
    for restricted in members {
        if restricted.configured && protocol_type != restricted.supported_by {
            return Err(restricted.refusal_message(protocol_type));
        }
    }
    Ok(())
}

/// Refuse a child resource one protocol's APIs have no behaviour for, on an api of
/// another protocol — the whole-resource form of
/// [`validate_protocol_restricted_members`], for a create whose every member is moot
/// once the parent's protocol is wrong.
///
/// `resource_plural` names the resource as the service's own messages do (`Models`,
/// `Route responses`), and the message is the sentence API Gateway answers such a
/// request with, built from the parent api's protocol type: "Currently, Models are not
/// permitted for APIs with a protocol type of HTTP". `Ok(())` or the message for the
/// operation's `BadRequestException`.
pub fn validate_protocol_restricted_resource(
    protocol_type: &str,
    resource_plural: &str,
    supported_by: &str,
) -> Result<(), String> {
    if protocol_type == supported_by {
        return Ok(());
    }
    Err(format!(
        "Currently, {resource_plural} are not permitted for APIs with a protocol type \
         of {protocol_type}"
    ))
}
