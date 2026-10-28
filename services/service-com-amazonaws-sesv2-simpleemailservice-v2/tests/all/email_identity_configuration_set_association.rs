// SPDX-License-Identifier: CC0-1.0

//! `PutEmailIdentityConfigurationSetAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization are
//! all exercised, and what the call persisted is read back through `GetEmailIdentity`.
//!
//! The rule these cases pin: the call stores, on the identity the path names, the
//! association the request carries — the whole association, so afterwards the identity
//! references exactly the configuration set named, whichever set it referenced before,
//! and a request naming none leaves it referencing nothing — and every later read of
//! that identity reports it; both halves of the association are resolved against the
//! account's state first, so an identity the account does not hold and a configuration
//! set it does not hold are each refused with a terminal 404 `NotFoundException` naming
//! the missing one, a name outside `ConfigurationSetName`'s documented character and
//! length rule with a 400 `BadRequestException`, and nothing is persisted by a refused
//! call; and because what is stored is the name itself and not a key kept pointing at a
//! live set, this call is the ONLY one that changes it — a name left behind by a deleted
//! set is cleared by a request of this call carrying no name, and by nothing else.
//!
//! The tables below span what the rule generalises over: each identity kind and
//! spelling, the forms a `ConfigurationSetName` may take (one character, the longest
//! allowed, hyphens, underscores, digits), the orderings, repetitions and removals in a
//! sequence of writes, the member left out of the request, and each way either lookup
//! can miss.

use serde_json::{json, Value};

use crate::configuration_set_event_destinations::EventDestinations;
use crate::configuration_sets::{Reply, Ses};

/// The documented length bound of `ConfigurationSetName`, from the member's own
/// documentation: "The name can contain up to 64 alphanumeric characters, including
/// letters, numbers, hyphens (-) and underscores (_) only."
const MAX_CONFIGURATION_SET_NAME_LEN: usize = 64;

/// Convenience calls for the identity-to-configuration-set association, over the
/// harness the sibling suites share.
///
/// The configuration-set side of a case is driven through the sibling
/// [`EventDestinations`] trait, which already carries those calls.
pub(crate) trait IdentityConfigurationSet {
    async fn create_identity(&self, identity: &str) -> Reply;
    async fn get_identity(&self, identity: &str) -> Reply;
    async fn delete_identity(&self, identity: &str) -> Reply;
    /// `PutEmailIdentityConfigurationSetAttributes` with whatever body the case
    /// supplies, so a case can leave the optional member out entirely.
    async fn put_association_body(&self, identity: &str, body: Value) -> Reply;
    /// `PutEmailIdentityConfigurationSetAttributes` naming `set`, or naming no set at
    /// all when `set` is `None`.
    async fn put_association(&self, identity: &str, set: Option<&str>) -> Reply;
}

impl IdentityConfigurationSet for Ses {
    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete_identity(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn put_association_body(&self, identity: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/configuration-set"),
            Some(body),
        )
        .await
    }

    async fn put_association(&self, identity: &str, set: Option<&str>) -> Reply {
        let body = match set {
            Some(name) => json!({"ConfigurationSetName": name}),
            None => json!({}),
        };
        self.put_association_body(identity, body).await
    }
}

/// The identity spellings the rule has to hold for: an address, a domain, a subdomain,
/// and the mixed-case forms of each, so both identity kinds are covered.
pub(crate) const IDENTITIES: &[&str] = &[
    "sender@example.com",
    "no-reply@mail.example.net",
    "example.com",
    "mail.example.org",
    "Mixed.Case@Example.COM",
    "SUB.Example.ORG",
];

/// The configuration-set names every case creates up front, spanning the forms
/// `ConfigurationSetName` allows: one character, letters with a hyphen, letters with
/// underscores and a digit, and the longest name the documented rule permits.
pub(crate) fn set_names() -> Vec<String> {
    vec![
        "A".to_string(),
        "primary-set".to_string(),
        "second_set_9".to_string(),
        "c".repeat(MAX_CONFIGURATION_SET_NAME_LEN),
    ]
}

/// Create every name in `sets`, which must succeed: a case associating an identity
/// with a set needs the set to exist first.
pub(crate) async fn create_sets(ses: &Ses, sets: &[String]) {
    for set in sets {
        let created = ses.create_set(json!({"ConfigurationSetName": set})).await;
        assert_eq!(
            created.status, 200,
            "case {set:?}: expected the configuration set to be created, got {} {}",
            created.status,
            created.message()
        );
    }
}

/// The `ConfigurationSetName` a read of `identity` reports, `Value::Null` where it
/// reports none. The read must be a 200.
pub(crate) async fn reported_set(ses: &Ses, identity: &str, case: &str) -> Value {
    let read = ses.get_identity(identity).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected {identity:?} to be readable, got {} {}",
        read.status,
        read.message()
    );
    read.member("ConfigurationSetName")
}

/// What a read must report after a write that named `set`.
fn expected(set: Option<&String>) -> Value {
    match set {
        Some(name) => Value::String(name.clone()),
        None => Value::Null,
    }
}

/// One write sequence: the sets named in order, whose LAST entry is what every later
/// read must report. `None` is a request that names no set at all.
///
/// The rows span a single association, a removal with nothing to remove, a replacement
/// by another set, a removal of a live association, an association after a removal, the
/// same request repeated (which changes nothing), and a longer walk through all of
/// them.
fn write_sequences(sets: &[String]) -> Vec<Vec<Option<String>>> {
    let (one_char, hyphened, underscored, longest) = (&sets[0], &sets[1], &sets[2], &sets[3]);
    vec![
        vec![Some(hyphened.clone())],
        vec![None],
        vec![Some(hyphened.clone()), Some(underscored.clone())],
        vec![Some(underscored.clone()), None],
        vec![None, Some(one_char.clone())],
        vec![Some(longest.clone()), Some(longest.clone())],
        vec![
            Some(one_char.clone()),
            Some(longest.clone()),
            None,
            Some(hyphened.clone()),
        ],
    ]
}

#[tokio::test]
async fn the_association_becomes_the_set_the_request_names() {
    let sets = set_names();
    for identity in IDENTITIES {
        for sequence in write_sequences(&sets) {
            let case = format!("({identity:?}, {sequence:?})");
            let ses = Ses::new();
            create_sets(&ses, &sets).await;
            let created = ses.create_identity(identity).await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected the identity to be created, got {} {}",
                created.status,
                created.message()
            );
            // A new identity references no configuration set, so every row below is an
            // observable change rather than a value that was already there.
            assert_eq!(
                reported_set(&ses, identity, &case).await,
                Value::Null,
                "case {case}: expected a new identity to report no ConfigurationSetName, got {:?}",
                reported_set(&ses, identity, &case).await
            );

            for (step, set) in sequence.iter().enumerate() {
                let put = ses.put_association(identity, set.as_deref()).await;
                assert_eq!(
                    put.status, 200,
                    "case {case} step {step}: expected 200, got {} {}",
                    put.status,
                    put.message()
                );
                // The response shape models no members, so the body carries none.
                assert!(
                    put.is_empty_body(),
                    "case {case} step {step}: the response models no members, got {:?}",
                    put.member("ConfigurationSetName")
                );
                // Every write is visible immediately, not only the last one.
                let seen = reported_set(&ses, identity, &case).await;
                assert_eq!(
                    seen,
                    expected(set.as_ref()),
                    "case {case} step {step}: expected the read to report {:?}, got {seen:?}",
                    expected(set.as_ref())
                );
            }

            let last = sequence.last().expect("a sequence writes at least once");
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                expected(last.as_ref()),
                "case {case}: expected the last write to be what is read back, {:?}, got {seen:?}",
                expected(last.as_ref())
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_set_removes_the_association() {
    let sets = set_names();
    // `ConfigurationSetName` is optional, so a body without it carries no association:
    // it removes whichever one the identity had rather than leaving it alone.
    for identity in IDENTITIES {
        for set in &sets {
            let case = format!("({identity:?}, {set:?})");
            let ses = Ses::new();
            create_sets(&ses, &sets).await;
            ses.create_identity(identity).await;
            let associated = ses.put_association(identity, Some(set)).await;
            assert_eq!(
                associated.status, 200,
                "case {case}: expected the association to be stored, got {} {}",
                associated.status,
                associated.message()
            );

            // Twice: the removal of a live association, and the same request repeated
            // against an identity that already references nothing.
            for round in 0..2 {
                let removed = ses.put_association_body(identity, json!({})).await;
                assert_eq!(
                    removed.status, 200,
                    "case {case} round {round}: expected 200, got {} {}",
                    removed.status,
                    removed.message()
                );
                let seen = reported_set(&ses, identity, &case).await;
                assert_eq!(
                    seen,
                    Value::Null,
                    "case {case} round {round}: expected no ConfigurationSetName, got {seen:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_domain_is_matched_without_regard_to_case() {
    // DNS compares domain labels case-insensitively, so a write that spells the domain
    // differently reaches the same identity; the local part belongs to the receiving
    // mailbox and is compared as supplied.
    let cases: &[(&str, &str)] = &[
        ("sender@example.com", "sender@EXAMPLE.COM"),
        ("Mixed.Case@Example.COM", "Mixed.Case@example.com"),
        ("example.com", "EXAMPLE.COM"),
        ("SUB.Example.ORG", "sub.example.org"),
    ];
    let sets = set_names();

    for (created_as, written_as) in cases {
        let case = format!("({created_as:?}, {written_as:?})");
        let ses = Ses::new();
        create_sets(&ses, &sets).await;
        ses.create_identity(created_as).await;

        let put = ses.put_association(written_as, Some(&sets[1])).await;
        assert_eq!(
            put.status, 200,
            "case {case}: expected the write to reach the identity, got {} {}",
            put.status,
            put.message()
        );
        let seen = reported_set(&ses, created_as, &case).await;
        assert_eq!(
            seen,
            Value::String(sets[1].clone()),
            "case {case}: expected the read to report {:?}, got {seen:?}",
            sets[1]
        );
    }
}

#[tokio::test]
async fn a_configuration_set_the_account_does_not_hold_is_refused() {
    let sets = set_names();
    let ses = Ses::new();
    create_sets(&ses, &sets).await;
    let deleted_set = "deleted-set";
    create_sets(&ses, &[deleted_set.to_string()]).await;
    let dropped = ses.delete_set(deleted_set).await;
    assert_eq!(
        dropped.status, 200,
        "expected the configuration set to be deleted, got {} {}",
        dropped.status,
        dropped.message()
    );
    for identity in IDENTITIES {
        ses.create_identity(identity).await;
        ses.put_association(identity, Some(&sets[1])).await;
    }

    // Each way the lookup misses: never created, created and then deleted, a name
    // differing only in case (a `ConfigurationSetName` is compared exactly), a name
    // that merely contains a held one, and a prefix of one.
    let missing: &[&str] = &[
        "absent-set",
        deleted_set,
        "PRIMARY-SET",
        "primary-set-2",
        "primary",
    ];

    for identity in IDENTITIES {
        for set in missing {
            let case = format!("({identity:?}, {set:?})");
            let put = ses.put_association(identity, Some(set)).await;
            assert_eq!(
                put.status, 404,
                "case {case}: expected 404, got {} {}",
                put.status,
                put.message()
            );
            assert_eq!(
                put.error_code(),
                "NotFoundException",
                "case {case}: expected NotFoundException, got {:?}",
                put.error_code()
            );
            assert_eq!(
                put.message(),
                format!("Configuration set <{set}> does not exist."),
                "case {case}: expected the message to name the configuration set, got {:?}",
                put.message()
            );
            // A refused write persists nothing: the association the identity had
            // survives it.
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                Value::String(sets[1].clone()),
                "case {case}: expected the stored association to be unchanged, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_identity_the_account_does_not_hold_is_refused() {
    let sets = set_names();
    let ses = Ses::new();
    create_sets(&ses, &sets).await;
    ses.create_identity("sender@example.com").await;
    ses.create_identity("example.com").await;
    ses.create_identity("deleted@example.com").await;
    ses.put_association("sender@example.com", Some(&sets[1]))
        .await;
    ses.put_association("example.com", Some(&sets[2])).await;
    let dropped = ses.delete_identity("deleted@example.com").await;
    assert_eq!(
        dropped.status, 200,
        "expected the identity to be deleted, got {} {}",
        dropped.status,
        dropped.message()
    );

    // Each way the lookup misses: never created, created and then deleted, a local part
    // differing only in case (the local part is compared as supplied), and a domain
    // that merely contains a held one.
    let missing: &[&str] = &[
        "absent@example.com",
        "absent.example.com",
        "deleted@example.com",
        "SENDER@example.com",
        "sub.example.com",
    ];

    for identity in missing {
        // Both the association leg and the removal leg: neither resolves the identity.
        for set in [Some(sets[1].clone()), None] {
            let case = format!("({identity:?}, {set:?})");
            let put = ses.put_association(identity, set.as_deref()).await;
            assert_eq!(
                put.status, 404,
                "case {case}: expected 404, got {} {}",
                put.status,
                put.message()
            );
            assert_eq!(
                put.error_code(),
                "NotFoundException",
                "case {case}: expected NotFoundException, got {:?}",
                put.error_code()
            );
            assert_eq!(
                put.message(),
                format!("Email identity {identity} does not exist."),
                "case {case}: expected the message to name the identity, got {:?}",
                put.message()
            );
        }
    }

    // A refused write persists nothing: the identities the account does hold keep the
    // association they were given.
    for (identity, set) in [("sender@example.com", &sets[1]), ("example.com", &sets[2])] {
        let seen = reported_set(&ses, identity, identity).await;
        assert_eq!(
            seen,
            Value::String(set.clone()),
            "case {identity:?}: expected the stored association to be unchanged, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_documented_rule_is_refused() {
    let sets = set_names();
    let ses = Ses::new();
    create_sets(&ses, &sets).await;
    for identity in IDENTITIES {
        ses.create_identity(identity).await;
        ses.put_association(identity, Some(&sets[1])).await;
    }

    // `ConfigurationSetName`'s documented rule: "The name can contain up to 64
    // alphanumeric characters, including letters, numbers, hyphens (-) and underscores
    // (_) only" — so the empty name, one character past the bound, and each kind of
    // character outside the allowed set are all refused. The bound itself is exercised
    // from the other side by `set_names`, whose longest row is a 64-character name that
    // is created and associated successfully above.
    let one_past_the_bound = "d".repeat(MAX_CONFIGURATION_SET_NAME_LEN + 1);
    let invalid: Vec<String> = vec![
        String::new(),
        one_past_the_bound,
        "has space".to_string(),
        "dots.not.allowed".to_string(),
        "plus+sign".to_string(),
        "slash/set".to_string(),
        "unicode-\u{e9}".to_string(),
    ];

    for identity in IDENTITIES {
        for set in &invalid {
            let case = format!("({identity:?}, {set:?})");
            let put = ses.put_association(identity, Some(set)).await;
            assert_eq!(
                put.status, 400,
                "case {case}: expected 400, got {} {}",
                put.status,
                put.message()
            );
            assert_eq!(
                put.error_code(),
                "BadRequestException",
                "case {case}: expected BadRequestException, got {:?}",
                put.error_code()
            );
            assert_eq!(
                put.message(),
                format!(
                    "Configuration set name {set} is invalid. The name can contain up to \
                     {MAX_CONFIGURATION_SET_NAME_LEN} alphanumeric characters, including letters, \
                     numbers, hyphens (-) and underscores (_) only."
                ),
                "case {case}: expected the documented refusal, got {:?}",
                put.message()
            );
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                Value::String(sets[1].clone()),
                "case {case}: expected the stored association to be unchanged, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_left_behind_by_a_deleted_set_is_cleared_only_by_this_call() {
    let sets = set_names();
    for (deleted, kept) in [(&sets[1], &sets[2]), (&sets[3], &sets[0])] {
        let case = format!("({deleted:?}, {kept:?})");
        let ses = Ses::new();
        create_sets(&ses, &sets).await;
        // Several identities on the set that goes, one on the set that stays, and one
        // on no set at all, so what the delete leaves behind is seen on each kind of
        // identity record.
        let (on_deleted, on_kept, unassociated) = (&IDENTITIES[..3], &IDENTITIES[3], &IDENTITIES[4]);
        for identity in on_deleted {
            ses.create_identity(identity).await;
            ses.put_association(identity, Some(deleted)).await;
        }
        ses.create_identity(on_kept).await;
        ses.put_association(on_kept, Some(kept)).await;
        ses.create_identity(unassociated).await;

        let dropped = ses.delete_set(deleted).await;
        assert_eq!(
            dropped.status, 200,
            "case {case}: expected the configuration set to be deleted, got {} {}",
            dropped.status,
            dropped.message()
        );

        for identity in on_deleted {
            // The stored value is the name the client wrote, not a key kept pointing at
            // a live set, so the delete leaves it reported verbatim.
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                Value::String(deleted.clone()),
                "case {case}: expected {identity:?} to still report {deleted:?} after the delete, got {seen:?}"
            );
            // Naming the dead set again IS refused, because this call resolves the name
            // it is given against the account's sets.
            let put = ses.put_association(identity, Some(deleted)).await;
            assert_eq!(
                put.status, 404,
                "case {case}: expected naming the deleted set again to be refused for {identity:?}, got {} {}",
                put.status,
                put.message()
            );
            // And the refusal left the name where it was.
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                Value::String(deleted.clone()),
                "case {case}: expected the refused write to leave {deleted:?} on {identity:?}, got {seen:?}"
            );
            // This call carrying no name is what clears it.
            let cleared = ses.put_association_body(identity, json!({})).await;
            assert_eq!(
                cleared.status, 200,
                "case {case}: expected the clearing write on {identity:?} to be accepted, got {} {}",
                cleared.status,
                cleared.message()
            );
            let seen = reported_set(&ses, identity, &case).await;
            assert_eq!(
                seen,
                Value::Null,
                "case {case}: expected {identity:?} to report no configuration set once cleared, got {seen:?}"
            );
        }
        // The other identities were not touched by any of it: the one on the set that
        // stays still reports it, and the one on no set still reports none.
        let seen = reported_set(&ses, on_kept, &case).await;
        assert_eq!(
            seen,
            Value::String(kept.clone()),
            "case {case}: expected {on_kept:?} to still reference {kept:?}, got {seen:?}"
        );
        let seen = reported_set(&ses, unassociated, &case).await;
        assert_eq!(
            seen,
            Value::Null,
            "case {case}: expected {unassociated:?} to reference no configuration set, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_identitys_other_attributes_are_untouched() {
    let sets = set_names();
    for identity in IDENTITIES {
        for set in [Some(sets[1].clone()), None] {
            let case = format!("({identity:?}, {set:?})");
            let ses = Ses::new();
            create_sets(&ses, &sets).await;
            ses.create_identity(identity).await;
            let before = ses.get_identity(identity).await;

            let put = ses.put_association(identity, set.as_deref()).await;
            assert_eq!(
                put.status, 200,
                "case {case}: expected 200, got {} {}",
                put.status,
                put.message()
            );

            let after = ses.get_identity(identity).await;
            // Everything except the association, less the two members the clock owns:
            // the verification transition falls due on its own span, so a comparison
            // of two reads cannot pin `VerificationStatus`, `VerifiedForSendingStatus`
            // or the DKIM status that moves with them.
            for member in [
                "IdentityType",
                "FeedbackForwardingStatus",
                "Policies",
                "Tags",
                "MailFromAttributes",
                "VerificationInfo",
            ] {
                assert_eq!(
                    after.member(member),
                    before.member(member),
                    "case {case}: expected {member} to be unchanged, got {:?} where it was {:?}",
                    after.member(member),
                    before.member(member)
                );
            }
        }
    }
}
