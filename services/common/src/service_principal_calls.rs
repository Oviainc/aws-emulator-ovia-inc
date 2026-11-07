// SPDX-License-Identifier: CC0-1.0

//! The requests one emulated service is making to another AS ITS OWN SERVICE PRINCIPAL,
//! on behalf of one of its resources — in flight, for the service that has to authorize
//! them.
//!
//! Some cross-service calls AWS makes are not made as the customer's principal at all.
//! CloudFront fetching a private origin under an ORIGIN ACCESS CONTROL is the shape: the
//! fetch is signed by CloudFront itself and arrives at the origin's service as the
//! `cloudfront.amazonaws.com` SERVICE PRINCIPAL, with the global condition keys
//! `aws:SourceArn` and `aws:SourceAccount` naming the distribution the fetch was made for —
//! which is exactly how a bucket policy and a KMS key policy written for such an origin
//! scope their grant.
//!
//! WHICH credential such a call presents is what says so. The calling service creates one in
//! the emulator's own credential store (`OperationContext::credential_store`) for the one
//! call, signs that call with it (`crate::in_process_sigv4::sign_in_process_request_as`), and
//! records HERE, under that credential's access key id, the principal it is calling as and
//! the resource it is calling for. The service SERVING the call reads the record for the
//! access key id the call's SIGNATURE was verified against — the one thing about a request a
//! client cannot choose for itself — and decides it accordingly. Held once in the shared
//! engine, like every other fact one service writes and another reads, so the two sides
//! cannot hold two accounts of one call.
//!
//! Three things keep a record to the one call it was written for. Its key is a credential the
//! EMULATOR minted, so presenting it takes the secret the emulator minted with it. It is
//! ACCOUNT-SCOPED: a reader passes the account it is serving and a record of another account
//! answers nothing. And it lives only while the call is in flight — the caller forgets it,
//! and deletes the credential, as soon as its call returns, and the clock drops a record
//! whose deadline has passed. A call whose credential this directory holds no record for is
//! therefore decided the ordinary way, against the principal that credential speaks for.
//!
//! Bounded like every other shared store: [`MAX_IN_FLIGHT_CALLS`] records at most, and a
//! record whose deadline the clock has passed is dropped. The cap REFUSES a new record rather
//! than evicting a live one, so a caller that never forgets its own records cannot make this
//! grow without end and cannot take the identity off a call already in flight either; a
//! refused record leaves that call to be decided by its credential, which is the narrower
//! answer.

use std::collections::HashMap;
use std::time::SystemTime;

/// How many in-flight records the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: a record is written per in-flight cross-service
/// call, so the directory needs a declared cap.
pub const MAX_IN_FLIGHT_CALLS: usize = 1_024;

/// One call an emulated service is making as a service principal on behalf of one of its
/// resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrincipalCall {
    /// The service principal the call is made as (`cloudfront.amazonaws.com`), which is the
    /// principal a resource policy or key policy names to grant it.
    pub principal: String,
    /// The ARN of the resource the call is made ON BEHALF OF, which is the value of
    /// `aws:SourceArn` a grant to that principal is scoped by.
    pub source_arn: String,
    /// The account that resource belongs to: the value of `aws:SourceAccount`, the other key
    /// such a grant is scoped by, and the account a reader's own must equal for this record
    /// to answer at all ([`ServicePrincipalCalls::call`]).
    pub source_account: String,
    /// The instant past which the record decides nothing: the calling service's own
    /// deadline for the call, so a record outlives neither the call nor the clock.
    pub expires_at: SystemTime,
}

/// The calls in flight right now, keyed by the access key id of the credential each is
/// signed with.
#[derive(Debug, Default)]
pub struct ServicePrincipalCalls {
    /// At most [`MAX_IN_FLIGHT_CALLS`] records.
    calls: HashMap<String, ServicePrincipalCall>,
}

impl ServicePrincipalCalls {
    /// Remember the call signed with `access_key_id`, for as long as it is in flight.
    ///
    /// Prunes first: a record whose deadline the clock has passed can decide nothing, so it
    /// is dropped — owed at this instant whether or not anybody is calling. `false` when the
    /// directory is at [`MAX_IN_FLIGHT_CALLS`] live records and nothing was written: the cap
    /// never takes the identity off a call already in flight, so the new call is left to be
    /// decided by the credential it presents.
    pub fn remember(
        &mut self,
        access_key_id: impl Into<String>,
        call: ServicePrincipalCall,
        now: SystemTime,
    ) -> bool {
        self.calls.retain(|_, held| now < held.expires_at);
        let key = access_key_id.into();
        if self.calls.len() >= MAX_IN_FLIGHT_CALLS && !self.calls.contains_key(&key) {
            return false;
        }
        self.calls.insert(key, call);
        true
    }

    /// The call `access_key_id` signed, or `None` when this directory holds none that the
    /// clock has not already passed and that is a call of `account`.
    ///
    /// `account` is the account the READER is serving: a record naming another account is a
    /// call about another account's resource and says nothing about this request.
    #[must_use]
    pub fn call(
        &self,
        access_key_id: &str,
        account: &str,
        now: SystemTime,
    ) -> Option<&ServicePrincipalCall> {
        self.calls
            .get(access_key_id)
            .filter(|call| now < call.expires_at && call.source_account == account)
    }

    /// Forget the call signed with `access_key_id` — called by the service that remembered
    /// it as soon as that call returns, so a record never outlives the call it describes.
    pub fn forget(&mut self, access_key_id: &str) {
        self.calls.remove(access_key_id);
    }

    /// How many records the directory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Whether the directory holds no record at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The account every record below is a call of.
    const ACCOUNT: &str = "000000000000";

    /// One record, due `seconds` from `now`.
    fn call(source_arn: &str, account: &str, now: SystemTime, seconds: u64) -> ServicePrincipalCall {
        ServicePrincipalCall {
            principal: "cloudfront.amazonaws.com".to_string(),
            source_arn: source_arn.to_string(),
            source_account: account.to_string(),
            expires_at: now + Duration::from_secs(seconds),
        }
    }

    /// A record answers for the one credential it was written for, only while that call is in
    /// flight, and only for the account it is a call of: the rows differ along the access key
    /// id asked about, the account asked for, whether the record was forgotten, and whether
    /// the clock has passed its deadline.
    #[test]
    fn a_record_answers_for_its_own_credential_account_and_window() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut directory = ServicePrincipalCalls::default();
        let a = "arn:aws:cloudfront::000000000000:distribution/EAAAAAAAAAAAAA";
        let b = "arn:aws:cloudfront::000000000000:distribution/EBBBBBBBBBBBBB";
        assert!(
            directory.remember("EMULATORAKONE", call(a, ACCOUNT, now, 30), now),
            "case the first record: the directory must accept it"
        );
        directory.remember("EMULATORAKTWO", call(b, ACCOUNT, now, 30), now);

        for (case, key, account, expected) in [
            ("the first call's credential", "EMULATORAKONE", ACCOUNT, Some(a)),
            ("the second call's credential", "EMULATORAKTWO", ACCOUNT, Some(b)),
            ("a credential nothing remembered", "EMULATORAKNONE", ACCOUNT, None),
            ("another account's reader", "EMULATORAKONE", "999999999999", None),
        ] {
            let found = directory
                .call(key, account, now)
                .map(|call| call.source_arn.as_str());
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?}, got {found:?}"
            );
        }

        directory.forget("EMULATORAKONE");
        assert!(
            directory.call("EMULATORAKONE", ACCOUNT, now).is_none(),
            "case a forgotten call: expected no record, got {:?}",
            directory.call("EMULATORAKONE", ACCOUNT, now)
        );
        let past = now + Duration::from_secs(31);
        assert!(
            directory.call("EMULATORAKTWO", ACCOUNT, past).is_none(),
            "case a call the clock has passed: expected no record, got {:?}",
            directory.call("EMULATORAKTWO", ACCOUNT, past)
        );
    }

    /// The directory never grows past its cap, and the cap refuses the NEW record rather than
    /// evicting a live one — so a call already in flight keeps its identity.
    #[test]
    fn the_cap_refuses_a_new_record_and_keeps_every_live_one() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut directory = ServicePrincipalCalls::default();
        let first = "arn:aws:cloudfront::000000000000:distribution/EFIRSTFIRSTFI";
        directory.remember("EMULATORAKFIRST", call(first, ACCOUNT, now, 300), now);
        for index in 0..MAX_IN_FLIGHT_CALLS + 64 {
            let arn = format!("arn:aws:cloudfront::000000000000:distribution/E{index}");
            let remembered =
                directory.remember(format!("EMULATORAK{index}"), call(&arn, ACCOUNT, now, 300), now);
            assert_eq!(
                remembered,
                directory.len() <= MAX_IN_FLIGHT_CALLS && remembered,
                "case record {index}: a refused record must not be held"
            );
            assert!(
                directory.len() <= MAX_IN_FLIGHT_CALLS,
                "case record {index}: expected at most {MAX_IN_FLIGHT_CALLS} records, got {}",
                directory.len()
            );
        }
        let kept = directory
            .call("EMULATORAKFIRST", ACCOUNT, now)
            .map(|call| call.source_arn.as_str());
        assert_eq!(
            kept,
            Some(first),
            "case the record held before the cap was reached: expected {first:?}, got {kept:?}"
        );
    }
}
