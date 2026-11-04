// SPDX-License-Identifier: CC0-1.0

//! WHERE this service serves the Medical Scribe (AWS HealthScribe) operation family, and
//! how it answers a request for that family addressed anywhere else.
//!
//! Amazon Transcribe has one endpoint per Region, but not every feature behind that
//! endpoint is offered in every Region: AWS HealthScribe — the feature
//! `StartMedicalScribeJob`, `GetMedicalScribeJob`, `ListMedicalScribeJobs` and
//! `DeleteMedicalScribeJob` belong to — is published for a SUBSET of them
//! ([`MEDICAL_SCRIBE_REGIONS`]). An account therefore holds no permission to call that
//! API through a Region outside the subset, and the service answers the request with its
//! authorization refusal: `BadRequestException` — the only client-error shape all four of
//! those operations model (`smithy_query(query="operation",
//! operation="GetMedicalScribeJob,StartMedicalScribeJob,ListMedicalScribeJobs,\
//! DeleteMedicalScribeJob")`) — carrying [`ACCOUNT_NOT_AUTHORIZED`].
//!
//! The refusal comes FIRST, ahead of the operation's own input validation and of any
//! lookup of the job the request names, because a call the account may not make is never
//! examined: the same answer comes back whether the named job exists or not, whether the
//! request would otherwise validate or not, and for the listing that names no job at all.
//! Nothing is read from the store and nothing is written to it, so a Region outside the
//! subset holds no Medical Scribe record of any kind.
//!
//! The Regions the feature IS offered in serve the family exactly as before: the gate
//! decides only where it is served, never what a served call answers.
//!
//! The other families behind the same endpoint (transcription, medical transcription,
//! Call Analytics, the custom-resource families) are offered Region-wide and are left
//! alone — this gate names only the four operations of this one feature.

use crate::ops::types::BadRequestException;

/// The Regions AWS offers AWS HealthScribe — the Medical Scribe operation family — in.
///
/// The feature publishes its own Region availability, separately from Amazon Transcribe's
/// (the AWS HealthScribe section of the Amazon Transcribe Developer Guide; `smithy_query`
/// carries no Region data, so the model cannot state it), and that availability is a strict
/// subset of this service's Regions: the feature is published for US East (N. Virginia),
/// and the wire recordings of this family confirm the gate from the other side, a Region
/// outside this set answering [`ACCOUNT_NOT_AUTHORIZED`] for every one of the four
/// operations.
///
/// Kept SORTED and free of duplicates. This set is the ONE place to extend as the feature's
/// published availability grows: a Region the feature reaches being absent here would make
/// this emulator refuse a call AWS serves, so a Region is added here — never worked around
/// in a handler — when the guide lists it.
pub const MEDICAL_SCRIBE_REGIONS: &[&str] = &["us-east-1"];

/// The `Message` this service refuses a call the account may not make with.
///
/// Entirely FIXED text: it interpolates nothing from the request and nothing from the
/// store — not the job name, not the Region — because the refusal is about the caller's
/// permission to make the call, not about anything the call named. The apostrophe of
/// "isn't" is the ASCII `'`.
pub const ACCOUNT_NOT_AUTHORIZED: &str = "Your account isn't authorized to call this \
     operation. Check your account permissions and try your request again.";

/// Whether the Medical Scribe family is served in `region`: whether AWS offers AWS
/// HealthScribe there ([`MEDICAL_SCRIBE_REGIONS`]).
pub fn serves_medical_scribe(region: &str) -> bool {
    MEDICAL_SCRIBE_REGIONS.contains(&region)
}

/// The refusal every Medical Scribe operation answers in `region` when the feature is not
/// offered there, and `None` in a Region that serves it.
///
/// One function for all four operations: each models `BadRequestException` itself, so a
/// handler lifts this into its own error channel and returns it as its FIRST act.
pub fn account_not_authorized_in(region: &str) -> Option<BadRequestException> {
    if serves_medical_scribe(region) {
        return None;
    }
    Some(BadRequestException {
        message: Some(ACCOUNT_NOT_AUTHORIZED.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_region_set_is_sorted_and_free_of_duplicates() {
        let mut sorted = MEDICAL_SCRIBE_REGIONS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, MEDICAL_SCRIBE_REGIONS,
            "the Region set must be sorted and hold each Region once, got \
             {MEDICAL_SCRIBE_REGIONS:?}"
        );
    }

    #[test]
    fn a_region_outside_the_set_refuses_with_the_fixed_sentence() {
        // One row per form a Region string takes: a Region of this service that the
        // feature is not offered in, a three-part Region name, another partition, and the
        // empty Region — which `crate::state::request_region` never hands over (it falls
        // back to the default Region for a request carrying no credential scope), so that
        // row only holds the predicate total for a value it may one day be asked about.
        for region in ["us-west-2", "ap-southeast-3", "us-gov-west-1", ""] {
            let refusal = account_not_authorized_in(region);
            assert_eq!(
                refusal.and_then(|refusal| refusal.message).as_deref(),
                Some(ACCOUNT_NOT_AUTHORIZED),
                "case {region:?}: a Region outside {MEDICAL_SCRIBE_REGIONS:?} must refuse \
                 with the fixed sentence"
            );
        }
    }

    #[test]
    fn a_region_in_the_set_is_served() {
        for region in MEDICAL_SCRIBE_REGIONS {
            assert!(
                account_not_authorized_in(region).is_none(),
                "case {region:?}: a Region the feature is offered in must not be refused"
            );
        }
    }
}
