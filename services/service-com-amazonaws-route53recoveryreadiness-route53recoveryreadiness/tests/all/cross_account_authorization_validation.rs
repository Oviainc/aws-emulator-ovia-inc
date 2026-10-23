// SPDX-License-Identifier: CC0-1.0

//! Which values `CreateCrossAccountAuthorization` accepts as a grant, and what it
//! answers for the ones it refuses.
//!
//! The rule under test: a cross-account readiness grant names the ACCOUNT being
//! authorized, so the create accepts exactly a value whose LITERAL bytes are an IAM
//! account-root ARN — `arn:<partition>:iam::<12 digits>:root`, with an empty region
//! segment — and refuses anything else with HTTP 400, the error code `Validation` and no
//! change to the store. Nothing is trimmed, lower-cased or otherwise normalised on the
//! way in, so a value differing from a well-formed one only by whitespace is refused.
//!
//! The tables range over what the rule generalises over: on the accepting side the
//! partition and the account id's digits (including both ends of the 12-digit range), on
//! the refusing side every way a value can fail to be that ARN — surrounding and embedded
//! whitespace (the issue's recorded case is the trailing-space row), a wrong segment
//! count, a wrong service, a non-empty region, an account id of the wrong width or with a
//! non-digit, a non-root resource, the wrong case, and a partition outside the published
//! catalog. [`inserting_a_space_anywhere_in_a_valid_grant_is_refused`] covers the
//! whitespace half as an invariant over every position in the value rather than as a
//! handful of rows.
//!
//! Everything goes over the wire through the harness in
//! [`super::cross_account_authorizations`], so no assertion reaches into the service's
//! state struct: "nothing persisted" is checked by listing.

use super::cross_account_authorizations::Emulator;

/// Grants a create must accept.
///
/// The partition values are the ones the ARN patterns in this tree publish (`aws`,
/// `aws-cn`, `aws-us-gov`, `aws-iso`, `aws-iso-<letter>`), mirrored by
/// `service_..._route53recoveryreadiness::state::is_arn_partition`; the issue cited only
/// `aws`, so the others are the rows it did not cite. The account ids walk the
/// published 12-digit width at both ends.
const WELL_FORMED: &[&str] = &[
    "arn:aws:iam::123456789012:root",
    "arn:aws:iam::000000000000:root",
    "arn:aws:iam::999999999999:root",
    "arn:aws-cn:iam::111122223333:root",
    "arn:aws-us-gov:iam::444455556666:root",
    "arn:aws-iso:iam::555566667777:root",
    "arn:aws-iso-b:iam::777788889999:root",
];

/// Values a create must refuse, each with the reason it is not an account-root ARN.
///
/// Rows 1-5 differ from a well-formed value only by whitespace — the issue's recorded
/// case is the first of them. The account-id rows sit one digit either side of the
/// published 12-digit width, and `aws-fake` is one partition past the catalog above.
const MALFORMED: &[(&str, &str)] = &[
    ("arn:aws:iam::123456789012:root ", "trailing space"),
    (" arn:aws:iam::123456789012:root", "leading space"),
    ("arn:aws:iam::123456789012:root\n", "trailing newline"),
    ("arn:aws:iam::123456789012:root\t", "trailing tab"),
    ("arn:aws:iam:: 123456789012:root", "space inside the account id"),
    ("", "empty"),
    ("123456789012", "a bare account id, not an ARN"),
    ("arn:aws:iam::123456789012:user/Alice", "a user, not the root"),
    ("arn:aws:iam::123456789012:role/Reader", "a role, not the root"),
    ("arn:aws:iam::123456789012:ROOT", "the resource in the wrong case"),
    ("ARN:aws:iam::123456789012:root", "the arn prefix in the wrong case"),
    ("arn:aws:sts::123456789012:root", "another service's ARN"),
    ("arn:aws:iam:us-east-1:123456789012:root", "a non-empty region"),
    ("arn:aws-fake:iam::123456789012:root", "a partition no AWS partition names"),
    (
        "arn:aws-iso-bb:iam::123456789012:root",
        "two letters where the isolated-partition family admits one",
    ),
    ("arn:aws:iam::12345678901:root", "an 11-digit account id"),
    ("arn:aws:iam::1234567890123:root", "a 13-digit account id"),
    ("arn:aws:iam::12345678901a:root", "a non-digit in the account id"),
    ("arn:aws:iam::123456789012:root:x", "a seventh segment"),
    ("arn:aws:iam::123456789012", "no resource segment"),
    ("авторизация", "not an ARN at all"),
];

/// The HTTP status and error code a refused grant carries. The status and the code are
/// what a client branches on; the sentence is the service's own and is not asserted
/// here beyond being present.
const REFUSAL_STATUS: u16 = 400;
const REFUSAL_CODE: &str = "Validation";

#[tokio::test]
async fn a_well_formed_grant_round_trips_byte_for_byte_and_can_be_withdrawn() {
    for value in WELL_FORMED {
        let emulator = Emulator::new();
        let created = emulator.create(value).await;
        assert_eq!(
            created.status, 200,
            "case {value:?}: expected HTTP 200 for a well-formed grant, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("crossAccountAuthorization").as_deref(),
            Some(*value),
            "case {value:?}: expected the submitted bytes back, got {:?}",
            created.string("crossAccountAuthorization")
        );
        assert_eq!(
            emulator.authorizations(value).await,
            vec![value.to_string()],
            "case {value:?}: expected the list to report exactly the stored grant"
        );

        assert_eq!(
            emulator.delete(value).await.status,
            200,
            "case {value:?}: expected HTTP 200 from the withdrawal"
        );
        assert_eq!(
            emulator.authorizations(value).await,
            Vec::<String>::new(),
            "case {value:?}: expected the grant gone after the withdrawal"
        );
    }
}

#[tokio::test]
async fn a_grant_that_is_not_an_account_root_arn_is_refused_and_stores_nothing() {
    for (value, reason) in MALFORMED {
        let emulator = Emulator::new();
        // A neighbouring, well-formed grant, so "nothing persisted" is checked against a
        // store that is not merely empty.
        let neighbour = WELL_FORMED[0];
        assert_eq!(
            emulator.create(neighbour).await.status,
            200,
            "case {value:?} ({reason}): expected the setup grant to be accepted"
        );

        let refused = emulator.create(value).await;
        assert_eq!(
            refused.status, REFUSAL_STATUS,
            "case {value:?} ({reason}): expected HTTP {REFUSAL_STATUS}, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            REFUSAL_CODE,
            "case {value:?} ({reason}): expected error code {REFUSAL_CODE:?}, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
        let message = refused.string("message").unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {value:?} ({reason}): expected the refusal to carry a message, got {:?}",
            refused.body
        );
        // Skipped for a row shorter than two characters (the empty one), where
        // "the message does not contain the value" is vacuous.
        if value.len() >= 2 {
            assert!(
                !message.contains(value),
                "case {value:?} ({reason}): the refusal must not echo the rejected value, got {message:?}"
            );
        }

        assert_eq!(
            emulator.authorizations(value).await,
            vec![neighbour.to_string()],
            "case {value:?} ({reason}): a refused grant must persist nothing"
        );
    }
}

#[tokio::test]
async fn inserting_a_space_anywhere_in_a_valid_grant_is_refused() {
    // The invariant behind the recorded case: the submitted bytes are judged as sent, so
    // a space at ANY position — not just the end — makes the value a different one, which
    // is not an account-root ARN. Bounded by the value's length, over every well-formed
    // row above.
    for value in WELL_FORMED {
        let emulator = Emulator::new();
        for position in 0..=value.len() {
            if !value.is_char_boundary(position) {
                continue;
            }
            let mut padded = String::with_capacity(value.len() + 1);
            padded.push_str(&value[..position]);
            padded.push(' ');
            padded.push_str(&value[position..]);
            let label = format!("{padded:?} (space at {position})");

            let refused = emulator.create(&padded).await;
            assert_eq!(
                refused.status, REFUSAL_STATUS,
                "case {label}: expected HTTP {REFUSAL_STATUS}, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                REFUSAL_CODE,
                "case {label}: expected error code {REFUSAL_CODE:?}, got {:?} / {}",
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                emulator.authorizations(&label).await,
                Vec::<String>::new(),
                "case {label}: a refused grant must persist nothing"
            );
        }
    }
}

#[tokio::test]
async fn withdrawing_a_grant_the_store_could_never_hold_is_still_an_empty_success() {
    // The delete models no not-found error and no grant rule applies to it: a malformed
    // value can never be in the store, so the withdrawal is the same empty HTTP 200 as
    // one for a value that was never created, and it leaves the store alone.
    let emulator = Emulator::new();
    for value in WELL_FORMED {
        assert_eq!(
            emulator.create(value).await.status,
            200,
            "case {value:?}: expected the setup grant to be accepted"
        );
    }
    let before = emulator.authorizations("setup").await;

    for (value, reason) in MALFORMED {
        if value.is_empty() {
            // An empty value cannot be addressed by the delete's `@httpLabel` at all.
            continue;
        }
        let reply = emulator.delete(value).await;
        assert_eq!(
            reply.status, 200,
            "case {value:?} ({reason}): expected HTTP 200 from the withdrawal, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            emulator.authorizations(value).await,
            before,
            "case {value:?} ({reason}): the withdrawal must leave every held grant alone"
        );
    }
}
