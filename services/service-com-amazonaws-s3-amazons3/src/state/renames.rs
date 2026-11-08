// SPDX-License-Identifier: CC0-1.0

//! The renames this store has already performed, remembered by the CLIENT TOKEN that asked
//! for them.
//!
//! "`RenameObject` supports idempotency using a client token. To make an idempotent API
//! request using `RenameObject`, specify a client token in the request. … If you retry a
//! request that completed successfully using the same client token and the same parameters,
//! the retry succeeds without performing any further actions. If you retry a successful
//! request using the same client token, but one or more of the parameters are different, the
//! retry fails and an `IdempotentParameterMismatch` error is returned"
//! (`RenameObjectRequest$ClientToken`).
//!
//! So a completed rename has to be remembered together with the parameters it was asked
//! with: that is the only way a retry can be told from a different call wearing the same
//! token.

use super::*;

/// How many completed renames this process remembers at once.
///
/// A client token is the CALLER's string, so the record of completed renames would grow
/// without bound as a caller keeps renaming objects with fresh tokens. The bound is this
/// emulator's own memory bound rather than an AWS quota: at the ceiling the OLDEST record
/// makes room for the newest, which is what keeps a retry of a recent call idempotent
/// whatever a caller does.
pub const MAX_COMPLETED_RENAMES: usize = 256;

/// One rename that completed, as the parameters it was asked with.
///
/// The three values that make a rename the same call: the bucket it was performed in, the
/// destination key it stored the object under, and the source key it moved it from. A retry
/// naming the same token and the same three is the same request; one naming the same token
/// and anything else is a different request wearing that token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedRename {
    /// The directory bucket the rename was performed in.
    pub bucket: String,
    /// The destination key, as `RenameObjectRequest$Key` named it.
    pub key: String,
    /// The source object's key, as this service read it out of
    /// `RenameObjectRequest$RenameSource`.
    pub source_key: String,
}

/// What a client token says about the rename a request is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameIdempotency {
    /// No completed rename carries this token: the request is a fresh call.
    Fresh,
    /// A completed rename carries it, for these same parameters: "the retry succeeds without
    /// performing any further actions".
    Repeated,
    /// A completed rename carries it, for other parameters: the retry is refused with
    /// `IdempotencyParameterMismatch`.
    ParametersDiffer,
}

impl ComAmazonawsS3Amazons3State {
    /// What the client token `token` says about the rename `asked` — see
    /// [`RenameIdempotency`]. Read-only.
    pub fn rename_idempotency(&self, token: &str, asked: &CompletedRename) -> RenameIdempotency {
        match self.completed_renames.get(token) {
            None => RenameIdempotency::Fresh,
            Some((_, completed)) if completed == asked => RenameIdempotency::Repeated,
            Some(_) => RenameIdempotency::ParametersDiffer,
        }
    }

    /// Remember `completed` as the rename the client token `token` asked for, at `now`.
    ///
    /// Called only for a rename this store really performed, so what is remembered is a
    /// completed call and never an attempt. At [`MAX_COMPLETED_RENAMES`] the oldest record
    /// makes room for this one.
    pub fn record_completed_rename(
        &mut self,
        token: &str,
        completed: CompletedRename,
        now: SystemTime,
    ) {
        if self.completed_renames.len() >= MAX_COMPLETED_RENAMES
            && !self.completed_renames.contains_key(token)
        {
            let oldest = self
                .completed_renames
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(remembered, _)| remembered.clone());
            if let Some(oldest) = oldest {
                self.completed_renames.remove(&oldest);
            }
        }
        self.completed_renames
            .insert(token.to_string(), (now, completed));
    }
}
