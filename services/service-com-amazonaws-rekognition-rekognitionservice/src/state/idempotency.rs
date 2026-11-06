// SPDX-License-Identifier: CC0-1.0

//! The idempotency ledger: how this service tells a RETRY of a request from a fresh
//! one, and both from a token reused for a different request.
//!
//! Several of this API's operations carry a `ClientRequestToken` the model marks
//! `@idempotencyToken` (`CreateUser`, `DeleteUser`, `AssociateFaces`,
//! `DisassociateFaces`, …), each documented as "an idempotency token that ensures a
//! call … completes only once", and each modelling
//! `IdempotentParameterMismatchException` for "a `ClientRequestToken` input parameter
//! … reused with an operation, but at least one of the other input parameters is
//! different from the previous call to the operation". That is ONE rule over all of
//! them, so it lives here once: a handler names its operation and the request
//! parameters the token committed, and [`TokenLedger`] answers how the token relates
//! to what it already holds.
//!
//! The ledger remembers the parameters a token committed, not the response, because a
//! token is only ever compared against them; what a retry answers is the operation's
//! own business.
//!
//! The ledger lives in this process's memory, so it is BOUNDED: see
//! [`MAX_REMEMBERED_TOKENS`].

use std::collections::{BTreeMap, VecDeque};

/// How many (operation, token) pairs the ledger remembers before the oldest is
/// forgotten.
///
/// The ledger is this process's memory, so a caller sending a fresh token per request
/// is bounded rather than allowed to grow the store without limit. A token older than
/// this many committed requests is classified as a fresh one, which is the same answer
/// the ledger gives for a token it never saw.
pub const MAX_REMEMBERED_TOKENS: usize = 1_024;

/// How a `ClientRequestToken` relates to what the ledger holds for the operation it
/// was sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenUse {
    /// The ledger holds no committed request under this operation and token: a fresh
    /// request, which the handler carries out.
    First,
    /// This operation already committed a request under this token with exactly these
    /// parameters: the same request again, which must complete only once.
    Retry,
    /// This operation committed a request under this token with at least one different
    /// parameter: the refusal `IdempotentParameterMismatchException` is modelled for.
    Mismatch,
}

/// The committed requests this service remembers by their idempotency token.
///
/// Keyed by the operation AND the token, so one token sent to two operations commits
/// two independent requests: the modelled refusal is about reusing a token "with an
/// operation", not across the API.
#[derive(Debug, Default)]
pub struct TokenLedger {
    /// The parameters each committed (operation, token) was carried out with.
    committed: BTreeMap<(String, String), Vec<(String, String)>>,
    /// The commit order of the keys above, so the OLDEST is the one forgotten when the
    /// ledger is full.
    order: VecDeque<(String, String)>,
}

impl TokenLedger {
    /// How `token` relates to what `operation` already committed under it, comparing
    /// `parameters` — the request members that identify the request — member by member.
    pub fn classify(&self, operation: &str, token: &str, parameters: &[(&str, &str)]) -> TokenUse {
        match self.committed.get(&key(operation, token)) {
            None => TokenUse::First,
            Some(committed) if names_same_request(committed, parameters) => TokenUse::Retry,
            Some(_) => TokenUse::Mismatch,
        }
    }

    /// Remember that `operation` carried out a request under `token` with
    /// `parameters`, evicting the oldest remembered request once the ledger is full.
    ///
    /// Called only once the request has actually been carried out, so a REFUSED
    /// request commits nothing and the caller may send the same token again.
    pub fn commit(&mut self, operation: &str, token: &str, parameters: &[(&str, &str)]) {
        while self.order.len() >= MAX_REMEMBERED_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.committed.remove(&oldest);
                }
                None => break,
            }
        }
        let key = key(operation, token);
        let parameters = parameters
            .iter()
            .map(|(member, value)| ((*member).to_string(), (*value).to_string()))
            .collect();
        if self.committed.insert(key.clone(), parameters).is_none() {
            self.order.push_back(key);
        }
    }
}

/// The ledger's key for one token of one operation.
fn key(operation: &str, token: &str) -> (String, String) {
    (operation.to_string(), token.to_string())
}

/// Whether `requested` is the same request as the `committed` one: the same members
/// carrying the same values, in the order the handler lists them.
fn names_same_request(committed: &[(String, String)], requested: &[(&str, &str)]) -> bool {
    committed.len() == requested.len()
        && committed
            .iter()
            .zip(requested)
            .all(|((member, value), (other_member, other_value))| {
                member == other_member && value == other_value
            })
}
