// SPDX-License-Identifier: CC0-1.0

//! `Intent`'s membership, over the whole class of values rather than one example.
//!
//! RULE: a member whose target is a modelled enum admits exactly the values that enum
//! declares — `CreateAgreementRequest$intent` targets `Intent`, whose values are
//! `NEW | AMEND | REPLACE` (`smithy_query(query="shape",shape="Intent")`) — so each of
//! those three is SERVED (with the members that intent makes required) and any other value
//! is refused at the boundary, before the handler's own cross-member rules are reached,
//! with the modelled `ValidationException` (HTTP 400) naming `intent`.
//!
//! The refused rows differ along what the rule generalises over: the case of a modelled
//! word, a word the enum does not declare at all, a modelled word with a modelled word's
//! prefix, one carrying surrounding space, a list of two modelled words, and the empty
//! string. Each carries BOTH conditional members (`agreementProposalIdentifier` and
//! `sourceAgreementIdentifier`), so a row that was NOT refused on `intent` would be
//! refused on one of those instead and the assertion on the reason would say so.
//!
//! `validation_envelope` and `validation_messages` each pin ONE such value inside the
//! whole-service envelope rule; what this suite adds is the SERVED half (every declared
//! value is accepted) and the breadth of the refused half.

use serde_json::{json, Value};

use crate::agreement_lifecycle::{agreement_over, call, emulator, term};

/// The operation whose `intent` is under test.
const OPERATION: &str = "CreateAgreementRequest";

/// The proposal every row names, inside `AgreementProposalId`'s
/// `^(at-|ap-)[A-Za-z0-9]+$`, so no row is refused on its form.
const PROPOSAL: &str = "ap-intentvalues";

/// RULE: every value `Intent` declares is served, with the members that intent makes
/// required ("`agreementProposalIdentifier` … is required if the intent is not `AMEND`";
/// "`sourceAgreementIdentifier` … is required for all non-`NEW` intents … Don't provide
/// this parameter if the intent is `NEW`").
#[tokio::test]
async fn every_value_the_enum_declares_is_served() {
    let router = emulator();
    let source = agreement_over(&router, "ap-intentsource", vec![term("eula.v1")]).await;

    // (case, the request body, which is the one that intent makes well formed)
    let rows: Vec<(&str, Value)> = vec![
        (
            "NEW, which names a proposal and no source",
            json!({
                "intent": "NEW",
                "agreementProposalIdentifier": PROPOSAL,
                "requestedTerms": [term("eula.v1")],
            }),
        ),
        (
            "AMEND, which names the source it modifies",
            json!({
                "intent": "AMEND",
                "sourceAgreementIdentifier": source,
                "requestedTerms": [term("eula.v1")],
            }),
        ),
        (
            "REPLACE, which names both",
            json!({
                "intent": "REPLACE",
                "agreementProposalIdentifier": PROPOSAL,
                "sourceAgreementIdentifier": source,
                "requestedTerms": [term("eula.v1")],
            }),
        ),
    ];

    for (case, request) in rows {
        let answer = call(&router, OPERATION, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a declared intent must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.str_at("/agreementRequestId").is_empty(),
            "case {case:?}: the served create answers an agreementRequestId, got {:?}",
            answer.body
        );
    }
}

/// RULE: a value outside the enum is refused 400 `ValidationException`, with the reason
/// naming `intent`, the sentence that reason states, and a `fields` entry at `intent` —
/// and nothing is minted for it.
#[tokio::test]
async fn a_value_outside_the_enum_is_refused_on_intent() {
    let router = emulator();
    let source = agreement_over(&router, "ap-intentrefused", vec![term("eula.v1")]).await;

    // (case, the value `intent` carries)
    let values: &[(&str, Value)] = &[
        ("a declared word in the wrong case", json!("new")),
        ("another declared word in mixed case", json!("Replace")),
        ("a word the enum does not declare", json!("SUPERSEDE_EVERYTHING")),
        ("a word a declared one is a prefix of", json!("NEWER")),
        ("a declared word with surrounding space", json!(" AMEND ")),
        ("two declared words at once", json!("NEW,REPLACE")),
        ("the empty string", json!("")),
    ];

    for (case, value) in values {
        let answer = call(
            &router,
            OPERATION,
            json!({
                "intent": value,
                // Both conditional members, so nothing the handler checks about them can
                // be what refuses the row: the intent is the only thing left.
                "agreementProposalIdentifier": PROPOSAL,
                "sourceAgreementIdentifier": source,
                "requestedTerms": [term("eula.v1")],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_INTENT",
            "case {case:?}: expected the reason naming intent, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str_at("/message"),
            "Provided intent is invalid.",
            "case {case:?}: expected the sentence that reason states, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/name"),
            "intent",
            "case {case:?}: expected the fields entry to name intent, got {:?}",
            answer.body
        );
        assert!(
            answer.str_at("/agreementRequestId").is_empty(),
            "case {case:?}: a refused create mints no quote, got {:?}",
            answer.body
        );
    }
}
