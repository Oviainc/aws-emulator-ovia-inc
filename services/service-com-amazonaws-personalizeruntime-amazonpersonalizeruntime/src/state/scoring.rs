// SPDX-License-Identifier: CC0-1.0

//! The deterministic scoring the three runtime operations share.
//!
//! # Why a hash, and why no clock or randomness
//!
//! All three operations are `@idempotent` in the model, and their documented output
//! is an ordering ("sorted in descending order by prediction score"). So the score of
//! one stored catalog entry for one request must be a PURE FUNCTION of
//!
//! * the entry's stored `base_score` (its popularity in the stored catalog), and
//! * the request's personalization inputs — `userId` / `itemId` and `context`.
//!
//! Nothing else: no clock, no counter, no randomness. Two identical requests
//! therefore return the same ordering with the same scores, which is what a caller
//! replaying an idempotent call observes against the real service.
//!
//! Which of those inputs seed one request, and in which order, is part of that rule,
//! so it is composed in ONE place — [`Seed`] — and every operation scores each stored
//! entry through it rather than spelling the tuple out again.
//!
//! The user/item-specific half comes from SHA-256 over the seed parts (the `sha2`
//! crate, already a workspace dependency), read as a unit interval. A hash is used
//! purely as a stable, well-distributed mixing function: it gives a different — but
//! fixed — ordering per user over the same catalog, which is the observable property
//! of a personalization model. It is NOT a model, and nothing here claims to be one;
//! the scores are derived from stored state rather than canned.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

/// Byte written between seed parts so that `["ab", "c"]` and `["a", "bc"]` hash
/// differently. ASCII unit separator, which no modelled input may contain in a
/// meaningful position.
const SEED_SEPARATOR: u8 = 0x1f;

/// The smallest share of an entry's `base_score` any user-specific weight can keep.
///
/// The weight lands in `[FLOOR_WEIGHT, 1.0)`, so a strictly more popular entry can
/// still be out-ranked for a given user (that is personalization), while the stored
/// `base_score` remains the dominant term (that is popularity). CHOSEN — AWS
/// publishes no scoring function — and documented here so the one place that decides
/// it is visible.
const FLOOR_WEIGHT: f64 = 0.5;

/// SHA-256 over `parts`, each part separated by [`SEED_SEPARATOR`].
pub fn digest(parts: &[&str]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([SEED_SEPARATOR]);
    }
    hasher.finalize().into()
}

/// The first `bytes` bytes of [`digest`], hex encoded — an opaque, URL-safe id.
pub fn digest_hex(parts: &[&str], bytes: usize) -> String {
    let full = digest(parts);
    hex::encode(&full[..bytes.min(full.len())])
}

/// A stable value in `[0.0, 1.0)` derived from `parts`.
pub fn affinity(parts: &[&str]) -> f64 {
    let digest = digest(parts);
    let mut head = [0_u8; 8];
    head.copy_from_slice(&digest[..8]);
    // `u64::MAX + 1` as f64 is exact (2^64), so the quotient stays below 1.0.
    u64::from_be_bytes(head) as f64 / (u64::MAX as f64 + 1.0)
}

/// The prediction score of a catalog entry whose stored popularity is `base_score`
/// for the request described by `parts`.
///
/// Monotone in `base_score` for a fixed request, and never negative: a stored entry
/// with no popularity scores 0.
///
/// PRIVATE: which parts seed one request, and in which order, is the rule this module
/// states, so it is composed in exactly one place — [`Seed`], the entry point every
/// operation scores through.
fn relevance(base_score: f64, parts: &[&str]) -> f64 {
    let weight = FLOOR_WEIGHT + (1.0 - FLOOR_WEIGHT) * affinity(parts);
    base_score.max(0.0) * weight
}

/// Everything about ONE request a stored entry's prediction score is a function of,
/// composed once: the resolved resource the entry was read from, the personalization
/// input that resource uses, and the request's `context`.
///
/// Built once per request, then asked for the score of each entry of that resource's
/// stored catalog ([`Seed::relevance`]). The composition lives here and nowhere else
/// because it is the rule, not an implementation detail of one operation: the three
/// runtime operations serve DISJOINT recipe types, so a composition spelled out per
/// handler could differ per handler and no single request would ever score through two
/// of them to show it.
#[derive(Debug, Clone)]
pub struct Seed<'a> {
    /// The resolved campaign or recommender: the scope every score is derived under,
    /// so two resources never share an ordering over the same catalog.
    scope: &'a str,
    /// The ONE personalization input the resolved resource uses — `userId` or
    /// `itemId`; see [`crate::state::RequiredInput::personalization`].
    personalization: &'a str,
    /// The request's `context` rendered by [`context_seed`], or `None` for an
    /// operation whose model carries no `context` member at all: a member no request
    /// to that operation can send is not part of its seed, rather than an empty part
    /// of it.
    context: Option<String>,
}

impl<'a> Seed<'a> {
    /// The seed of a request to an operation that models `context`, which seeds the
    /// score whether the request carried a map or not (an absent map is an empty
    /// context, and still a context this operation has).
    pub fn with_context(
        scope: &'a str,
        personalization: &'a str,
        context: Option<&HashMap<String, String>>,
    ) -> Self {
        Self {
            scope,
            personalization,
            context: Some(context_seed(context)),
        }
    }

    /// The seed of a request to an operation whose model carries no `context` member.
    pub fn without_context(scope: &'a str, personalization: &'a str) -> Self {
        Self {
            scope,
            personalization,
            context: None,
        }
    }

    /// The prediction score of one stored catalog entry — whose popularity is
    /// `base_score` and whose id is `entry_id` — for this request.
    pub fn relevance(&self, base_score: f64, entry_id: &str) -> f64 {
        match self.context.as_deref() {
            Some(context) => relevance(
                base_score,
                &[self.scope, self.personalization, context, entry_id],
            ),
            None => relevance(base_score, &[self.scope, self.personalization, entry_id]),
        }
    }
}

/// The request's `context` map rendered as one stable seed part.
///
/// Sorted, so an equal map hashes equally however the wire order arrived — the map
/// is unordered on the wire, and an idempotent call may not depend on its order.
fn context_seed(context: Option<&HashMap<String, String>>) -> String {
    let Some(context) = context else {
        return String::new();
    };
    let mut pairs: Vec<String> = context
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    pairs.sort();
    pairs.join(",")
}

/// Rescale `scores` in place into each entry's share of the certainty Amazon
/// Personalize reports ("a numeric representation of the model's certainty that the
/// item will be the next user selection").
///
/// `scores` must be the relevance of EVERY entry the resolved resource's stored
/// catalog holds for this request — the whole scored catalog, settled BEFORE any
/// member of the request that merely selects which entries come back (which ids the
/// caller listed, which filter it named, how many results it asked for, which of them
/// a promotion takes). The reported score of one entry is the model's certainty for
/// that (resource, personalization inputs, entry) tuple, so the denominator may not
/// depend on a selection member: a response that reports a subset of the catalog
/// therefore sums to at most 1.0, and reports the same score for an entry whatever
/// the rest of the request asked for.
///
/// Rescaling is strictly positive and uniform, so it cannot change the ordering the
/// caller is promised. When every score is 0 (a catalog whose entries carry no
/// popularity) every share stays 0: there is no certainty to report, and spreading
/// a full 1.0 over such a catalog would invent one.
pub fn normalize(scores: &mut [f64]) {
    normalize_by(scores, |score| score);
}

/// [`normalize`] over the score field of a scored catalog, for a caller whose entries
/// are tuples or structs: the entries are normalized IN PLACE, so each keeps carrying
/// its own share of 1.0 through filtering, sorting and selection.
///
/// `score` names the field to rescale; the set is walked twice (the sum, then the
/// rescale) straight over the slice, so nothing is allocated.
pub fn normalize_by<T>(candidates: &mut [T], score: impl Fn(&mut T) -> &mut f64) {
    if candidates.is_empty() {
        return;
    }
    let mut total = 0.0_f64;
    for candidate in candidates.iter_mut() {
        total += *score(candidate);
    }
    // A total of 0 means every entry of the scored catalog carries no popularity, so
    // the model has no certainty to apportion and every share stays 0.
    if total <= 0.0 {
        return;
    }
    for candidate in candidates.iter_mut() {
        let score = score(candidate);
        *score /= total;
    }
}

/// Order two scored entries the way every one of these operations reports them: by
/// score descending, with a pair whose scores do not compare (a non-numeric score)
/// treated as equal.
///
/// Each entry is `(tie-break key, score)`. The score half is the rule all three
/// operations share and lives only here; the key is the caller's, because a tied pair
/// still has to produce ONE stable order for an idempotent call and each operation
/// states which key settles it — the entry's own id where the operation chooses the
/// list, the caller's list position where it re-ranks a list the caller sent. The key
/// is compared ascending.
pub fn by_score_desc<Key: Ord>(left: (Key, f64), right: (Key, f64)) -> std::cmp::Ordering {
    right
        .1
        .partial_cmp(&left.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.0.cmp(&right.0))
}
