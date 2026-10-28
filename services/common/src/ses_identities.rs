// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT'S EMAIL IDENTITY REGISTRY: the set of identities one account holds,
//! and the verification state of each.
//!
//! Amazon SES publishes TWO APIs over ONE set of account identities: the classic
//! `awsQuery` surface (`VerifyEmailIdentity`, `VerifyDomainIdentity`,
//! `ListIdentities`, `GetIdentityVerificationAttributes`, `DeleteIdentity`) and the
//! v2 REST surface (`CreateEmailIdentity`, `ListEmailIdentities`,
//! `GetEmailIdentity`, `DeleteEmailIdentity`). An identity recorded through either
//! one is the SAME account identity: the other surface lists it, reports its
//! verification state, refuses a send from it while that state is not successful,
//! and loses it when it is deleted.
//!
//! So the set lives HERE, once in the shared engine, rather than in each surface's
//! own store: a surface PUBLISHES the identities it records and ADOPTS the ones it
//! does not hold yet, and each keeps beside that only what its own API adds (the
//! classic surface's sending-authorization policies and notification topics; the v2
//! surface's tags and certificate associations). What makes an identity the one the
//! account holds is carried here too — its verification, and the Easy DKIM setup
//! ([`SesDkimSetup`]) whose tokens its owner publishes in DNS.
//!
//! Nothing here names a wire shape. The registry holds the identity as its creator
//! spelled it, which class of identity it is, the verification state and the instant
//! the state's next move is due, and the TXT value a domain identity's owner
//! publishes; each surface spells those onto the shapes ITS model declares — the
//! classic surface's `Pending`/`Success` for the v2 surface's `PENDING`/`SUCCESS`.
//!
//! The registry is BOUNDED at [`MAX_REGISTERED_IDENTITIES`], the identity quota SES
//! documents per account per Region, so a caller looping on a verify cannot grow it
//! without end.

use std::collections::BTreeMap;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// The identities one account may hold, and so the ceiling on this registry: SES
/// documents a limit of 10,000 verified identities per account per Region, and a
/// publication past it is refused rather than letting caller input grow the registry
/// without end.
pub const MAX_REGISTERED_IDENTITIES: usize = 10_000;

/// How many bytes a domain identity's verification token carries before it is
/// base64-encoded.
///
/// The token is the TXT value the owner publishes at `_amazonses.<domain>`, and its
/// DECODED length is the one structural property a caller can check before it puts
/// the record in DNS: Amazon SES issues base64 that decodes to 32 bytes. The encoded
/// string is therefore 44 characters, the last of them base64 padding.
const VERIFICATION_TOKEN_BYTES: usize = 32;

/// Whether an identity names a single mailbox or a whole domain.
///
/// A registry record carries the class its creator classified it as, so the two
/// surfaces cannot disagree about whether an identity is an address or a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesIdentityKind {
    /// A single mailbox, e.g. `user@example.com`.
    EmailAddress,
    /// A domain, which also covers the mailboxes under it and its subdomains.
    Domain,
}

impl SesIdentityKind {
    /// The class `identity` belongs to: a value carrying a local part (`@`) is an
    /// address, anything else is a domain. The same reading both SES surfaces apply
    /// to the identity they are handed.
    pub fn of(identity: &str) -> Self {
        if identity.contains('@') {
            Self::EmailAddress
        } else {
            Self::Domain
        }
    }
}

/// The verification state of an identity, as the SES value set names it.
///
/// Spelled `Pending`/`Success`/… by the classic surface and `PENDING`/`SUCCESS`/… by
/// the v2 surface; each surface maps this enum onto its own model's value, so one
/// state cannot be reported as two different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesVerificationState {
    /// Verification has started and has not completed: SES has sent the
    /// confirmation message, or is looking for the records the owner was told to
    /// publish.
    Pending,
    /// The identity is verified; SES accepts it as a sender.
    Success,
    /// Verification failed.
    Failed,
    /// Verification failed for a reason that may clear on its own.
    TemporaryFailure,
    /// Verification has not been attempted.
    NotStarted,
}

/// The Easy DKIM setup one identity of the account holds.
///
/// DKIM keys are published for an IDENTITY, not for an API: the tokens the owner puts
/// in the domain's DNS, whether the service has found them there, and whether messages
/// from the identity are signed are facts about the account's identity. So they are
/// carried HERE beside it, and a surface that adopts the identity reports the setup the
/// account holds rather than one of its own making — a client that published the records
/// one API handed it reads those same records back from the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SesDkimSetup {
    /// Whether messages sent from the identity are DKIM-signed.
    pub signing_enabled: bool,
    /// Where the search for the published records stands, in the same value set the
    /// identity's own verification uses ([`SesVerificationState`]); each surface spells
    /// it onto the shape its own model declares.
    pub status: SesVerificationState,
    /// The tokens the identity's owner publishes, in the order they were generated.
    pub tokens: Vec<String>,
}

/// One identity the account holds, as the registry records it.
#[derive(Debug, Clone)]
pub struct SesIdentityRecord {
    /// The identity as the request that recorded it spelled it, which is the value
    /// both surfaces report in a listing.
    pub name: String,
    /// Address or domain.
    pub kind: SesIdentityKind,
    /// The verification state the surface that holds the identity last published.
    pub verification: SesVerificationState,
    /// The INSTANT the move out of [`SesVerificationState::Pending`] is due, and
    /// `None` when no move is owed — either because there is none left to make, or
    /// because the identity's verification can never complete (a domain no DNS zone
    /// can cover). An instant rather than a timer: nothing sleeps for it, and a
    /// reader materialises the move only once the clock has already made it due
    /// ([`Self::settle`]).
    pub verification_due: Option<SystemTime>,
    /// The TXT value a DOMAIN identity's owner publishes at `_amazonses.<domain>`,
    /// which the classic surface reports as its `VerificationToken`.
    ///
    /// Minted ONCE, when the domain identity is first recorded ([`mint_verification_token`]),
    /// and then stored: every later read of the identity — through either surface —
    /// answers the value the owner was told to publish. `None` for an address
    /// identity, which has no DNS record to publish.
    pub verification_token: Option<String>,
    /// The Easy DKIM setup the account holds for this identity ([`SesDkimSetup`]), and
    /// `None` while no setup has been asked for.
    ///
    /// Recorded once, by whichever surface set Easy DKIM up, and reported by all of
    /// them: the tokens an owner has already published stay the tokens every API
    /// answers with.
    pub dkim: Option<SesDkimSetup>,
}

impl SesIdentityRecord {
    /// A record of `name` whose verification has just been REQUESTED: the state the
    /// surface recording it starts the identity in, and the instant that state's
    /// move is due.
    pub fn requested(
        name: &str,
        verification: SesVerificationState,
        verification_due: Option<SystemTime>,
    ) -> Self {
        Self {
            name: name.to_string(),
            kind: SesIdentityKind::of(name),
            verification,
            verification_due,
            verification_token: None,
            // Easy DKIM is set up by a call of its own, so an identity whose
            // verification has only just been requested holds no setup yet.
            dkim: None,
        }
    }

    /// Materialise the verification move the clock already owes this record at
    /// `now`, and answer the state to report.
    ///
    /// The move was owed at `now` whether or not anybody called, so applying it
    /// changes nothing a later caller would otherwise have seen. The same rule both
    /// surfaces apply to their own copy of the identity, so a reader of either one
    /// sees the same state.
    pub fn settle(&mut self, now: SystemTime) -> SesVerificationState {
        if self.verification == SesVerificationState::Pending
            && self.verification_due.is_some_and(|due| now >= due)
        {
            self.verification = SesVerificationState::Success;
            self.verification_due = None;
        }
        self.verification
    }
}

/// The registry itself: identity key -> the identity the account holds under it.
///
/// Registered in the shared engine under THIS type, so the two SES surfaces reach
/// one set of identities instead of each holding its own.
///
/// Keyed by [`identity_key`] and ordered by it, so a listing either surface pages is
/// in one stable order for every caller — which is what makes its `NextToken`
/// resumable.
#[derive(Debug, Default)]
pub struct SesIdentityDirectory {
    /// The records, by identity key.
    identities: BTreeMap<String, SesIdentityRecord>,
    /// How many tokens this registry has minted, which is what makes the next one
    /// UNIQUE rather than merely unlikely to repeat: every token digests this
    /// counter, and the counter is never reused.
    minted_tokens: u64,
}

impl SesIdentityDirectory {
    /// Publish `record` as an identity of this account, and hand back the stored
    /// record.
    ///
    /// Idempotent in everything a caller can observe: an identity already registered
    /// keeps the spelling, the class and the verification TOKEN it was recorded with
    /// — so the TXT value an owner has already published stays the value both
    /// surfaces report — while its verification STATE and the instant that state's
    /// move is due are taken from `record`, which is the publishing surface's own
    /// current reading of them.
    ///
    /// A new DOMAIN identity is given its verification token here, so an identity
    /// recorded through the surface whose model has no TXT token of its own still
    /// reports one through the surface whose model does.
    ///
    /// `None` when the registry already holds [`MAX_REGISTERED_IDENTITIES`]
    /// identities and this is a new one: the caller refuses the request rather than
    /// growing the registry past the account's quota.
    pub fn publish(&mut self, record: SesIdentityRecord) -> Option<&SesIdentityRecord> {
        let key = identity_key(&record.name);
        match self.identities.get_mut(&key) {
            Some(held) => {
                held.verification = record.verification;
                held.verification_due = record.verification_due;
                // The Easy DKIM setup is the account identity's, so a publisher that
                // holds one is reporting the account's and the registry takes it; one
                // that holds none — a surface whose API never set DKIM up for the
                // identity — must not erase the setup another surface recorded.
                if let Some(dkim) = record.dkim {
                    held.dkim = Some(dkim);
                }
            }
            None => {
                if self.identities.len() >= MAX_REGISTERED_IDENTITIES {
                    return None;
                }
                let token = match (record.kind, record.verification_token) {
                    (SesIdentityKind::Domain, Some(token)) => Some(token),
                    (SesIdentityKind::Domain, None) => Some(self.mint_verification_token()),
                    (SesIdentityKind::EmailAddress, _) => None,
                };
                self.identities.insert(
                    key.clone(),
                    SesIdentityRecord {
                        verification_token: token,
                        ..record
                    },
                );
            }
        }
        self.identities.get(&key)
    }

    /// Drop the identity `name` names: it was deleted, so the account no longer
    /// holds it and neither surface reports it.
    ///
    /// Deleting an identity the registry does not hold changes nothing, which is what
    /// makes a delete served on one surface and then replayed on the other harmless.
    pub fn forget(&mut self, name: &str) {
        self.identities.remove(&identity_key(name));
    }

    /// The identity `name` names, or `None` when the account holds no such identity.
    pub fn get(&self, name: &str) -> Option<&SesIdentityRecord> {
        self.identities.get(&identity_key(name))
    }

    /// Every identity the account holds, ordered by [`identity_key`].
    pub fn records(&self) -> impl Iterator<Item = &SesIdentityRecord> {
        self.identities.values()
    }

    /// Materialise the verification move the clock already owes every identity at
    /// `now` ([`SesIdentityRecord::settle`]).
    pub fn settle(&mut self, now: SystemTime) {
        for record in self.identities.values_mut() {
            record.settle(now);
        }
    }

    /// How many identities the account holds.
    pub fn len(&self) -> usize {
        self.identities.len()
    }

    /// Whether the account holds no identity at all.
    pub fn is_empty(&self) -> bool {
        self.identities.is_empty()
    }

    /// Bring one surface's own identity `table` and this registry back into agreement
    /// at `now`, so every API over the account's identities answers for ONE set of
    /// them.
    ///
    /// The whole reconciliation, written ONCE here rather than per surface, so no two
    /// of them can resolve a hand-off differently. Four steps, in this order:
    ///
    /// 1. every registry record carries the verification move the clock already owed
    ///    it ([`Self::settle`]);
    /// 2. an identity the table holds that the registry does not know about yet is
    ///    PUBLISHED — which is how a record written straight into the table (a test
    ///    fixture) reaches the other surfaces;
    /// 3. an identity the table holds that the registry no longer holds was DELETED
    ///    through another surface, so it goes from the table too;
    /// 4. an identity the registry holds that the table does not is ADOPTED
    ///    ([`AccountIdentityRecord::adopt`]).
    ///
    /// The verification state is never copied back ONTO a record the table already
    /// holds: every surface applies the same rule to the same stored due instant, so
    /// they reach the same state without one overwriting another's reading of it.
    ///
    /// The work is proportional to the account's identity set, which
    /// [`MAX_REGISTERED_IDENTITIES`] bounds, and allocates only when there is a
    /// publication or an adoption to make.
    pub fn reconcile<R: AccountIdentityRecord>(
        &mut self,
        table: &mut BTreeMap<String, R>,
        now: SystemTime,
    ) {
        self.settle(now);

        if table.values().any(|held| !held.is_registered()) {
            let unpublished: Vec<(String, SesIdentityRecord)> = table
                .values()
                .filter(|held| !held.is_registered())
                .map(|held| (R::table_key(held.identity_name()), held.published()))
                .collect();
            for (key, record) in unpublished {
                // A publication the registry refuses (the account is at its identity
                // quota) leaves the record unpublished rather than marking it
                // registered: it is this surface's own identity, not one another
                // surface deleted.
                let Some(published) = self.publish(record).cloned() else {
                    continue;
                };
                if let Some(held) = table.get_mut(&key) {
                    held.mark_registered(&published);
                }
            }
        }

        table.retain(|_, held| !held.is_registered() || self.get(held.identity_name()).is_some());

        if self
            .records()
            .any(|record| !table.contains_key(&R::table_key(&record.name)))
        {
            let adopted: Vec<(String, R)> = self
                .records()
                .filter(|record| !table.contains_key(&R::table_key(&record.name)))
                .map(|record| (R::table_key(&record.name), R::adopt(record, now)))
                .collect();
            for (key, record) in adopted {
                // A surface whose own API gives every identity of this class an Easy
                // DKIM setup has just built one for an identity the account holds no
                // setup for. The setup belongs to the identity, so the registry takes
                // it and every other surface answers with those same tokens.
                if let Some(setup) = record.published().dkim {
                    self.adopt_dkim(record.identity_name(), setup);
                }
                table.insert(key, record);
            }
        }
    }

    /// Record `setup` as the identity `name`'s Easy DKIM setup, unless the account
    /// already holds one for it.
    ///
    /// The first setup stands: the tokens an owner has been handed are the records it
    /// publishes, so nothing replaces them behind its back.
    fn adopt_dkim(&mut self, name: &str, setup: SesDkimSetup) {
        let Some(held) = self.identities.get_mut(&identity_key(name)) else {
            return;
        };
        if held.dkim.is_none() {
            held.dkim = Some(setup);
        }
    }

    /// A fresh verification token for a domain identity of this registry.
    ///
    /// Unique rather than merely improbable: the digest covers
    /// [`Self::minted_tokens`], which this method advances and never reuses, so no
    /// two tokens from one registry can collide. Opaque because it also covers
    /// entropy the system seeded and the minting instant, so a caller cannot derive
    /// the next token from one it holds — and NOTHING from the request goes in: the
    /// token identifies what the registry stored, it is never computed from what the
    /// caller sent.
    fn mint_verification_token(&mut self) -> String {
        self.minted_tokens = self.minted_tokens.wrapping_add(1);
        mint_verification_token(self.minted_tokens)
    }
}

/// One surface's OWN record of an account identity, as
/// [`SesIdentityDirectory::reconcile`] reaches it.
///
/// Each API over the account's identities keeps, beside the registry, what IT adds to
/// an identity — the classic SES surface's sending-authorization policies and
/// notification topics; the v2 surface's tags and certificate associations. This trait
/// is the little each of those records has to answer for the reconciliation to be
/// written once: how its table is keyed, which identity it is for, whether the registry
/// carries it yet, how the registry carries it, and how to make one from a registry
/// record.
pub trait AccountIdentityRecord: Sized {
    /// The key this surface's own table holds the identity `name` under.
    ///
    /// Not necessarily [`identity_key`]: a surface keys its table the way its own
    /// operations look an identity up, and only the registry's keying decides which
    /// records are ONE identity of the account.
    fn table_key(name: &str) -> String;

    /// The identity this record is for, as the request that recorded it spelled it.
    fn identity_name(&self) -> &str;

    /// Whether the registry already carries this identity.
    ///
    /// This is what tells an identity DELETED through another surface — registered
    /// here, gone from the registry — from one this surface recorded but has not
    /// published yet.
    fn is_registered(&self) -> bool;

    /// Note that the registry now carries this identity, as `published` holds it.
    fn mark_registered(&mut self, published: &SesIdentityRecord);

    /// How the registry carries this record: the identity as it was recorded, its
    /// class, the verification this surface holds for it, and the Easy DKIM setup it
    /// holds for it ([`SesDkimSetup`]).
    fn published(&self) -> SesIdentityRecord;

    /// This surface's record of an identity the registry holds and it does not.
    ///
    /// The account holds the identity in the state the registry carries, so the record
    /// is adopted in THAT state rather than started afresh; everything this surface's
    /// own API adds starts where its create starts it, since none of it has been asked
    /// for.
    fn adopt(published: &SesIdentityRecord, now: SystemTime) -> Self;
}

/// A verification token in the form Amazon SES issues one: base64 over
/// [`VERIFICATION_TOKEN_BYTES`] bytes, so the value a caller base64-decodes before
/// publishing it is exactly that many bytes long.
///
/// `sequence` is the registry's own never-reused counter, which is what makes the
/// token unique; the digest also covers entropy the system seeded and the minting
/// instant, which is what makes it opaque. Hashed with the `sha2` crate and encoded
/// with the workspace's own base64 codec rather than a hand-written one — SHA-256's
/// digest is itself 32 bytes, which is the length the token carries.
fn mint_verification_token(sequence: u64) -> String {
    let minted_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(system_entropy().to_be_bytes());
    digest.update(minted_at.to_be_bytes());
    let bytes = digest.finalize();
    ::aws_smithy_types::base64::encode(&bytes[..VERIFICATION_TOKEN_BYTES])
}

/// 64 bits of entropy this process did not choose, salting a minted token so it is
/// opaque as well as unique.
///
/// `std::collections::hash_map::RandomState::new` "is initialized with random keys"
/// (its own documentation) — the system seeds them — and a fresh one per call carries
/// fresh keys, so finishing its hasher yields a value a caller cannot derive from the
/// request or from an earlier token. It salts a token and nothing else: it is never a
/// key, a secret or a signature, and the UNIQUENESS of a token comes from the stored
/// counter it digests, not from here.
fn system_entropy() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// The key an identity is registered and looked up under.
///
/// The DOMAIN part of an identity is a DNS name, which is compared without regard to
/// case, while the local part of an address is not — so `Example.COM` and
/// `example.com` name one identity, and `User@example.com` and `user@example.com` name
/// two. This is the comparison that decides which records are ONE identity of the
/// account; a surface whose own table is keyed some other way therefore sees two of
/// its records collapse onto one registry record, whose spelling is the one it was
/// first recorded with.
pub fn identity_key(identity: &str) -> String {
    let identity = addr_spec(identity);
    match identity.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => identity.to_lowercase(),
    }
}

/// The addr-spec of an identity value, which may arrive either bare
/// (`user@example.com`) or with a display name (`"Me" <user@example.com>`).
///
/// A value with no angle-addr is returned trimmed and unchanged, so a malformed value
/// stays malformed for the caller's own check.
pub fn addr_spec(value: &str) -> &str {
    let trimmed = value.trim();
    match (trimmed.rfind('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if open + 1 < close => trimmed[open + 1..close].trim(),
        _ => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A requested identity, named.
    fn requested(name: &str, due: Option<SystemTime>) -> SesIdentityRecord {
        SesIdentityRecord::requested(name, SesVerificationState::Pending, due)
    }

    #[test]
    fn a_domain_identity_is_given_a_token_that_decodes_to_thirty_two_bytes() {
        let mut registry = SesIdentityDirectory::default();
        // (case, the identity recorded, whether it carries a verification token)
        let rows = [
            ("a domain", "example.com", true),
            ("a subdomain", "mail.example.com", true),
            ("a long domain", "a-very-long-label.example.co.uk", true),
            ("an address", "user@example.com", false),
        ];
        for (case, name, expects_token) in rows {
            let published = registry
                .publish(requested(name, None))
                .expect("the registry holds the published identity");
            let token = published.verification_token.clone();
            assert_eq!(
                token.is_some(),
                expects_token,
                "case {case:?} ({name}): expected a verification token present={expects_token}, \
                 got {token:?}"
            );
            if let Some(token) = token {
                let decoded = ::aws_smithy_types::base64::decode(&token)
                    .unwrap_or_else(|error| panic!("case {case:?}: token {token:?} is not base64: {error}"));
                assert_eq!(
                    decoded.len(),
                    VERIFICATION_TOKEN_BYTES,
                    "case {case:?} ({name}): expected the token to decode to \
                     {VERIFICATION_TOKEN_BYTES} bytes, got {} from {token:?}",
                    decoded.len()
                );
            }
        }
    }

    #[test]
    fn re_publishing_keeps_the_token_and_takes_the_state() {
        let mut registry = SesIdentityDirectory::default();
        let first = registry
            .publish(requested("example.com", None))
            .expect("the first publication")
            .verification_token
            .clone();
        let mut again = requested("EXAMPLE.com", None);
        again.verification = SesVerificationState::Success;
        let second = registry.publish(again).expect("the second publication");
        assert_eq!(
            second.verification_token, first,
            "case a re-publication: expected the stored token to stand, got {:?} after {first:?}",
            second.verification_token
        );
        assert_eq!(
            second.verification,
            SesVerificationState::Success,
            "case a re-publication: expected the publishing surface's state, got {:?}",
            second.verification
        );
        assert_eq!(
            registry.len(),
            1,
            "case a re-publication: expected one identity, got {}",
            registry.len()
        );
    }

    #[test]
    fn a_pending_identity_settles_only_once_its_move_is_due() {
        let now = SystemTime::now();
        let span = Duration::from_secs(3);
        let mut registry = SesIdentityDirectory::default();
        registry.publish(requested("user@example.com", Some(now + span)));
        // No transition is owed to a record whose verification can never complete.
        registry.publish(requested("example.invalid", None));

        registry.settle(now + span / 2);
        for name in ["user@example.com", "example.invalid"] {
            let state = registry.get(name).expect(name).verification;
            assert_eq!(
                state,
                SesVerificationState::Pending,
                "case {name:?} before the move is due: expected Pending, got {state:?}"
            );
        }

        registry.settle(now + span);
        let settled = registry.get("user@example.com").expect("the address").verification;
        assert_eq!(
            settled,
            SesVerificationState::Success,
            "case an identity whose move fell due: expected Success, got {settled:?}"
        );
        let never = registry.get("example.invalid").expect("the domain").verification;
        assert_eq!(
            never,
            SesVerificationState::Pending,
            "case an identity owed no move: expected Pending to stand, got {never:?}"
        );
    }

    #[test]
    fn an_identity_is_found_however_its_domain_is_cased_and_is_dropped_when_forgotten() {
        let mut registry = SesIdentityDirectory::default();
        registry.publish(requested("user@Example.COM", None));
        registry.publish(requested("Example.com", None));
        // (case, the spelling a request carries, whether it resolves)
        let rows = [
            ("the spelling it was recorded with", "user@Example.COM", true),
            ("a lower-cased domain", "user@example.com", true),
            ("an angle-addr", "\"Me\" <user@example.com>", true),
            ("a domain identity", "EXAMPLE.COM", true),
            ("another local part", "other@example.com", false),
            ("an unrelated domain", "example.net", false),
        ];
        for (case, spelling, expected) in rows {
            let actual = registry.get(spelling).is_some();
            assert_eq!(
                actual, expected,
                "case {case:?} ({spelling}): expected resolves={expected}, got {actual}"
            );
        }
        assert_eq!(
            registry.get("user@example.com").map(|held| held.name.as_str()),
            Some("user@Example.COM"),
            "case the recorded spelling: expected the identity as its creator spelled it"
        );

        // The domain part compares without regard to case, the local part does not.
        registry.forget("user@EXAMPLE.com");
        assert!(
            registry.get("user@Example.COM").is_none() && registry.get("example.com").is_some(),
            "case one identity forgotten: expected only it to go, got {} record(s)",
            registry.len()
        );
        registry.forget("example.com");
        assert!(
            registry.is_empty(),
            "case every identity forgotten: expected an empty registry, got {} record(s)",
            registry.len()
        );
    }

    #[test]
    fn reconciling_publishes_adopts_and_drops_in_step_with_the_registry() {
        /// One surface's own record, carrying just what the reconciliation reaches.
        #[derive(Debug, Clone, PartialEq, Eq)]
        struct SurfaceRecord {
            name: String,
            registered: bool,
            /// The token the registry handed this record when it was published, which
            /// a surface whose model reports one keeps.
            token: Option<String>,
            /// What the surface's own API added to the identity, which an adoption
            /// starts empty and a reconciliation must never clear.
            label: String,
        }

        impl AccountIdentityRecord for SurfaceRecord {
            fn table_key(name: &str) -> String {
                identity_key(name)
            }
            fn identity_name(&self) -> &str {
                &self.name
            }
            fn is_registered(&self) -> bool {
                self.registered
            }
            fn mark_registered(&mut self, published: &SesIdentityRecord) {
                self.registered = true;
                self.token = published.verification_token.clone();
            }
            fn published(&self) -> SesIdentityRecord {
                SesIdentityRecord::requested(&self.name, SesVerificationState::Pending, None)
            }
            fn adopt(published: &SesIdentityRecord, _now: SystemTime) -> Self {
                Self {
                    name: published.name.clone(),
                    registered: true,
                    token: published.verification_token.clone(),
                    label: String::new(),
                }
            }
        }

        let now = SystemTime::now();
        let mut registry = SesIdentityDirectory::default();
        // An identity another surface recorded, which this table does not hold yet.
        registry.publish(requested("adopted.example.com", None));
        // One this surface recorded and published before, which the other surface has
        // since deleted — the registry does not hold it.
        let mut table: BTreeMap<String, SurfaceRecord> = BTreeMap::new();
        table.insert(
            identity_key("deleted-elsewhere.example.com"),
            SurfaceRecord {
                name: "deleted-elsewhere.example.com".to_owned(),
                registered: true,
                token: None,
                label: "ours".to_owned(),
            },
        );
        // And one written straight into the table, which the registry has never seen.
        table.insert(
            identity_key("unpublished.example.com"),
            SurfaceRecord {
                name: "unpublished.example.com".to_owned(),
                registered: false,
                token: None,
                label: "ours".to_owned(),
            },
        );

        registry.reconcile(&mut table, now);

        let held: Vec<&str> = table.values().map(|held| held.name.as_str()).collect();
        assert_eq!(
            held,
            vec!["adopted.example.com", "unpublished.example.com"],
            "case one of each: expected the registry's identity adopted, the unpublished one \
             kept and the one deleted elsewhere dropped, got {held:?}"
        );
        let adopted = table
            .get(&identity_key("adopted.example.com"))
            .expect("the adopted identity");
        assert_eq!(
            (adopted.registered, adopted.label.as_str()),
            (true, ""),
            "case an adopted identity: expected it registered and carrying none of this \
             surface's own attributes, got {adopted:?}"
        );
        assert!(
            adopted.token.is_some(),
            "case an adopted domain identity: expected the registry's verification token, got \
             {adopted:?}"
        );
        let published = table
            .get(&identity_key("unpublished.example.com"))
            .expect("the published identity");
        assert_eq!(
            (published.registered, published.label.as_str()),
            (true, "ours"),
            "case a record written straight into the table: expected it published and its own \
             attributes untouched, got {published:?}"
        );
        assert!(
            published.token.is_some() && registry.get("unpublished.example.com").is_some(),
            "case a record written straight into the table: expected the registry to carry it \
             and hand back its token, got {published:?}"
        );
        assert!(
            registry.get("deleted-elsewhere.example.com").is_none(),
            "case an identity deleted elsewhere: expected the registry still not to hold it"
        );

        // Reconciling again changes nothing: the two sides already agree.
        let before = table.clone();
        registry.reconcile(&mut table, now);
        assert_eq!(
            table, before,
            "case a second reconciliation: expected nothing to change, got {table:?}"
        );
    }

    #[test]
    fn the_registry_is_bounded_at_the_account_quota() {
        let mut registry = SesIdentityDirectory::default();
        for index in 0..MAX_REGISTERED_IDENTITIES {
            registry.publish(requested(&format!("id{index}.example.com"), None));
        }
        assert_eq!(
            registry.len(),
            MAX_REGISTERED_IDENTITIES,
            "case a registry filled to the ceiling: expected {MAX_REGISTERED_IDENTITIES} \
             identities, got {}",
            registry.len()
        );
        // A re-publication is not a new identity, so it is served at the ceiling.
        assert!(
            registry
                .publish(requested("id0.example.com", None))
                .is_some(),
            "case a re-publication at the ceiling: expected it to be served"
        );
        assert!(
            registry
                .publish(requested("one-too-many.example.com", None))
                .is_none(),
            "case one past the ceiling: expected the publication to be refused"
        );
        assert_eq!(
            registry.len(),
            MAX_REGISTERED_IDENTITIES,
            "case one past the ceiling: expected the ceiling held, got {}",
            registry.len()
        );
    }
}
