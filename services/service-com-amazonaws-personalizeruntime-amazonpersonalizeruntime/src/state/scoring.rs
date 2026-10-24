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
pub fn relevance(base_score: f64, parts: &[&str]) -> f64 {
    let weight = FLOOR_WEIGHT + (1.0 - FLOOR_WEIGHT) * affinity(parts);
    base_score.max(0.0) * weight
}

/// The request's `context` map rendered as one stable seed part.
///
/// Sorted, so an equal map hashes equally however the wire order arrived — the map
/// is unordered on the wire, and an idempotent call may not depend on its order.
pub fn context_seed(context: Option<&HashMap<String, String>>) -> String {
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

/// Rescale `scores` in place so that they sum to 1.0, as Amazon Personalize does
/// across the candidate set it scored ("a numeric representation of the model's
/// certainty that the item will be the next user selection").
///
/// `scores` must be the WHOLE candidate set the request scored — every entry the
/// resolved resource's catalog holds that the request's filter kept — and never the
/// slice a caller's result count happens to ask back. The score of one entry is a
/// property of the resource, the request's personalization inputs and the entry, so
/// the denominator may not depend on how many entries the caller asked for; a
/// response that reports a subset of the candidates therefore sums to at most 1.0,
/// and reports the same score for an entry whatever its result count was.
///
/// Rescaling is strictly positive and uniform, so it cannot change the ordering the
/// caller is promised. When every score is 0 (a catalog whose entries carry no
/// popularity) the certainty is spread evenly over the candidate set rather than
/// left at 0.
pub fn normalize(scores: &mut [f64]) {
    normalize_by(scores, |score| score);
}

/// [`normalize`] over the score field of a scored candidate set, for a caller whose
/// candidates are tuples or structs: the candidates are normalized IN PLACE, so each
/// keeps carrying its own share of 1.0 through sorting and selection.
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
    if total > 0.0 {
        for candidate in candidates.iter_mut() {
            let score = score(candidate);
            *score /= total;
        }
    } else {
        // Every candidate scored 0 (a catalog whose entries carry no popularity), so
        // the certainty is spread evenly over the set rather than left at 0.
        let share = 1.0 / candidates.len() as f64;
        for candidate in candidates.iter_mut() {
            *score(candidate) = share;
        }
    }
}

/// Order two scored entries the way every one of these operations reports them:
/// by score descending, then by id ascending so that equal scores still produce ONE
/// stable order for an idempotent call.
pub fn by_score_desc(left: (&str, f64), right: (&str, f64)) -> std::cmp::Ordering {
    right
        .1
        .partial_cmp(&left.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.0.cmp(right.0))
}
