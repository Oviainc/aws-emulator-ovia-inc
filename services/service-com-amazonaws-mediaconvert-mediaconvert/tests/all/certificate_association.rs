// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the account's ACM certificate associations:
//! `AssociateCertificate` and `DisassociateCertificate`.
//!
//! The rule these cases pin: the ARNs of the ACM certificates an account has associated
//! are a SET in the account's persisted state. `AssociateCertificate` adds the ARN it is
//! given to that set, `DisassociateCertificate` removes exactly the ARN it addresses and
//! leaves every other member in place, and an `Arn` the set does not hold addresses no
//! association at all — so it is the modelled `NotFoundException` (404), not a success for
//! a removal that removed nothing. A value that is not an ACM certificate ARN never
//! reaches the set: both doors answer it with the modelled `BadRequestException` (400).
//!
//! The API has no operation that READS the set, so every case here observes it the only
//! way a client can: through what `DisassociateCertificate` answers. That is also what
//! makes these cases fail against a handler that stores nothing — a disassociation of a
//! certificate that was just associated answers 202 only if the association was persisted.
//!
//! The wire harness is the one `job_lifecycle` builds: a single shared engine behind the
//! real multi-service router, so every case asserts on the bytes a client would see.

use serde_json::json;
use service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_CERTIFICATES;

use super::job_lifecycle::{Emulator, Reply};
// The one path-label encoder this test binary has: the tagging suite addresses its
// resource by ARN exactly as `DisassociateCertificate` does, so the encoding rule is
// shared rather than written again here.
use super::resource_tagging::encode_label;

/// The certificate collection's URI — the `http` binding of both operations
/// (`smithy_query(query="operation",operation="AssociateCertificate,DisassociateCertificate")`:
/// `POST /2017-08-29/certificates` and `DELETE /2017-08-29/certificates/{Arn}`).
const CERTIFICATES_URI: &str = "/2017-08-29/certificates";

async fn associate(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .send("POST", CERTIFICATES_URI, Some(json!({ "arn": arn })))
        .await
}

async fn disassociate(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!("{CERTIFICATES_URI}/{}", encode_label(arn)),
            None,
        )
        .await
}

/// How this service words the refusal of a member that carries no characters.
fn blank_refusal(member: &str) -> String {
    format!("{member} must not be empty.")
}

/// How this service words the refusal of a value that is not an ACM certificate ARN.
fn arn_refusal(arn: &str) -> String {
    format!("Arn {arn} is not the ARN of an ACM certificate.")
}

/// How this service words a request that addresses something the store does not hold.
fn not_found_refusal(member: &str) -> String {
    format!("The resource you specified in {member} was not found.")
}

/// Assert `reply` is the modelled bad request carrying `message`.
fn assert_bad_request(case: &str, message: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        message,
        "case {case:?}: expected the refusal to read {message:?}, got body {}",
        reply.body
    );
}

/// Assert `reply` is the modelled 404 for an association the account does not hold.
fn assert_not_associated(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404 for an association the account does not hold, \
         got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected NotFoundException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        not_found_refusal("arn"),
        "case {case:?}: expected the member-naming lookup refusal, got body {}",
        reply.body
    );
}

/// Assert `reply` is the empty 202 a removed association answers with.
fn assert_disassociated(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 202,
        "case {case:?}: expected HTTP 202, got {} with body {}",
        reply.status, reply.body
    );
    assert!(
        reply.at("/message").is_null(),
        "case {case:?}: a removed association answers an empty body, got {}",
        reply.body
    );
}

/// Assert `reply` is the 201 an association answers with.
fn assert_associated(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {}",
        reply.status, reply.body
    );
}

/// The ACM certificate ARNs the table below is driven through.
///
/// Both members are documented as "the ARN of the ACM certificate" that is associated with
/// (or disassociated from) the MediaConvert resource, and target the model's bare
/// `__string` — no pattern and no length
/// (`smithy_query(query="operation",operation="AssociateCertificate,DisassociateCertificate")`),
/// so what makes a value acceptable is that it is an ACM certificate ARN,
/// `arn:<partition>:acm:<region>:<account>:certificate/<id>`. The rows differ along every
/// field of that form: the partition, the region, the account and the shape of the
/// certificate id — including an id that is not a UUID, which the model's `__string`
/// admits.
fn certificates() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "the standard partition",
            "arn:aws:acm:us-west-2:123456789012:certificate/12345678-1234-1234-1234-123456789012",
        ),
        (
            "another region",
            "arn:aws:acm:eu-central-1:123456789012:certificate/0b1d7e8a-5c3f-4a2e-9d6b-7f8e9a0b1c2d",
        ),
        (
            "another account",
            "arn:aws:acm:us-west-2:210987654321:certificate/aaaabbbb-cccc-dddd-eeee-ffff00001111",
        ),
        (
            "the GovCloud partition",
            "arn:aws-us-gov:acm:us-gov-west-1:123456789012:certificate/4f0b2c1d-8e7a-4b3c-9d2e-1a0b9c8d7e6f",
        ),
        (
            "the China partition",
            "arn:aws-cn:acm:cn-north-1:123456789012:certificate/9c8b7a65-4321-4fed-cba9-876543210fed",
        ),
        (
            "a certificate id that is not a UUID",
            "arn:aws:acm:us-west-2:123456789012:certificate/legacy-media-cert-01",
        ),
    ]
}

/// A certificate is associated with the account until it is disassociated, and the
/// association is per certificate: removing one leaves every other one in place.
///
/// Every row runs against ONE account in sequence, so each certificate is associated while
/// the others already are. What proves the set is persisted is the order of answers for a
/// single ARN: 404 before it is associated (nothing is pre-seeded), 202 once it is, and 404
/// again after it has been removed.
#[tokio::test]
async fn a_certificate_is_associated_with_the_account_until_it_is_disassociated() {
    let emulator = Emulator::new();
    let rows = certificates();

    // Nothing is associated on a fresh account: every row addresses no association yet.
    for (case, arn) in &rows {
        assert_not_associated(
            &format!("{case}, before any association"),
            &disassociate(&emulator, arn).await,
        );
    }

    // Associate them all, then remove them one at a time: after each removal the one just
    // removed is gone and every one still associated is still there.
    for (case, arn) in &rows {
        assert_associated(case, &associate(&emulator, arn).await);
    }

    for (index, (case, arn)) in rows.iter().enumerate() {
        assert_disassociated(case, &disassociate(&emulator, arn).await);
        // The same ARN a second time: the association is gone, so there is nothing left to
        // remove under it.
        assert_not_associated(
            &format!("{case}, disassociated twice"),
            &disassociate(&emulator, arn).await,
        );

        // Every row already addressed stays removed: a disassociation removes exactly the
        // ARN it names, so no earlier removal comes back. The rows NOT yet addressed are
        // still associated, which the next iterations observe when their own 202 arrives.
        for (other_case, other_arn) in rows.iter().take(index) {
            assert_not_associated(
                &format!("{other_case}, after {case} was disassociated"),
                &disassociate(&emulator, other_arn).await,
            );
        }
    }
}

/// Associating a certificate the account already has associated asks for the state the
/// account is in: the repeat answers the same 201 and the account still holds that ONE
/// association, so a single disassociation removes it.
///
/// The rows differ in how many times the association is repeated, so the set cannot be
/// storing one entry per call.
#[tokio::test]
async fn associating_the_same_certificate_again_leaves_one_association() {
    for (case, arn) in certificates() {
        for repeats in [1_usize, 2, 5] {
            let emulator = Emulator::new();
            for attempt in 1..=repeats {
                assert_associated(
                    &format!("{case}, association #{attempt} of {repeats}"),
                    &associate(&emulator, arn).await,
                );
            }

            assert_disassociated(
                &format!("{case}, after {repeats} association(s)"),
                &disassociate(&emulator, arn).await,
            );
            // One removal is enough however many times it was associated: the account
            // holds a SET, not one entry per call.
            assert_not_associated(
                &format!("{case}, after one removal of {repeats} association(s)"),
                &disassociate(&emulator, arn).await,
            );
        }
    }
}

/// The values that are not ACM certificate ARNs, which neither door accepts.
///
/// The rows span the ways a value can fail to be one: not an ARN at all, an ARN of another
/// service, an ACM ARN naming another kind of resource, one with no resource id, one whose
/// certificate id is empty, and one missing an ARN field altogether.
fn not_certificate_arns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not an ARN at all", "certificate/12345678-1234-1234-1234-123456789012"),
        ("a bare certificate id", "12345678-1234-1234-1234-123456789012"),
        (
            "an ARN of another service",
            "arn:aws:mediaconvert:us-west-2:123456789012:queues/Default",
        ),
        (
            "an ACM ARN naming another kind of resource",
            "arn:aws:acm:us-west-2:123456789012:certificate-authority/12345678-1234-1234-1234-123456789012",
        ),
        (
            "an ACM ARN with no resource id",
            "arn:aws:acm:us-west-2:123456789012:certificate",
        ),
        (
            "an ACM ARN whose certificate id is empty",
            "arn:aws:acm:us-west-2:123456789012:certificate/",
        ),
        (
            "an ARN missing the account field",
            "arn:aws:acm:us-west-2:certificate/12345678-1234-1234-1234-123456789012",
        ),
    ]
}

/// A value that is not an ACM certificate ARN is refused by BOTH doors with the modelled
/// bad request, and changes nothing: the certificate the account does hold is still
/// associated afterwards, and the refused value never becomes an association of its own.
#[tokio::test]
async fn a_value_that_is_not_an_acm_certificate_arn_is_refused_by_both_doors() {
    let emulator = Emulator::new();
    // One association the refusals must leave exactly as it is.
    let (held_case, held) = certificates()[0];
    assert_associated(held_case, &associate(&emulator, held).await);

    for (case, value) in not_certificate_arns() {
        assert_bad_request(
            &format!("AssociateCertificate with {case}"),
            &arn_refusal(value),
            &associate(&emulator, value).await,
        );
        assert_bad_request(
            &format!("DisassociateCertificate with {case}"),
            &arn_refusal(value),
            &disassociate(&emulator, value).await,
        );
    }

    // An `Arn` that carries no characters is refused naming the member instead: it is not
    // a mis-shaped ARN, it is an absent value. The zero-length form is sent to the
    // associate door only — it is no path segment at all, so it addresses no operation.
    assert_bad_request(
        "AssociateCertificate with no characters at all",
        &blank_refusal("Arn"),
        &associate(&emulator, "").await,
    );
    for blank in [" ", "   ", "\t", "\n"] {
        assert_bad_request(
            &format!("AssociateCertificate with the blank value {blank:?}"),
            &blank_refusal("Arn"),
            &associate(&emulator, blank).await,
        );
        assert_bad_request(
            &format!("DisassociateCertificate with the blank value {blank:?}"),
            &blank_refusal("Arn"),
            &disassociate(&emulator, blank).await,
        );
    }

    // The one real association survived every refusal above, and nothing else was stored.
    assert_disassociated(held_case, &disassociate(&emulator, held).await);
    assert_not_associated(held_case, &disassociate(&emulator, held).await);
}

/// Caller-driven growth is bounded: the account associates certificates up to its capacity
/// and then answers the modelled quota error instead of growing further. A certificate
/// already associated is still a no-op success at capacity, and disassociating one makes
/// room for exactly one more.
#[tokio::test]
async fn associating_past_the_account_capacity_is_a_quota_refusal() {
    let emulator = Emulator::new();
    let arn_at = |index: usize| {
        format!("arn:aws:acm:us-west-2:123456789012:certificate/bulk-{index:04}")
    };

    for index in 0..MAX_CERTIFICATES {
        assert_associated(
            &format!("certificate #{index} of {MAX_CERTIFICATES}"),
            &associate(&emulator, &arn_at(index)).await,
        );
    }

    // Past the cap: the modelled quota error, for every further certificate.
    for index in [MAX_CERTIFICATES, MAX_CERTIFICATES + 1] {
        let arn = arn_at(index);
        let refused = associate(&emulator, &arn).await;
        assert_eq!(
            refused.status, 402,
            "case certificate #{index} at capacity: expected HTTP 402, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case certificate #{index} at capacity: expected ServiceQuotaExceededException, \
             got {:?} with body {}",
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.string("/message"),
            format!("An account associates at most {MAX_CERTIFICATES} ACM certificates."),
            "case certificate #{index} at capacity: expected the refusal to name the bound, \
             got body {}",
            refused.body
        );
        // Nothing was stored for it: it is still an association the account does not hold.
        assert_not_associated(&format!("certificate #{index} after the refusal"), &{
            disassociate(&emulator, &arn).await
        });
    }

    // A certificate the account already holds adds nothing, so it is accepted at capacity.
    assert_associated(
        "a certificate already associated, at capacity",
        &associate(&emulator, &arn_at(0)).await,
    );

    // Room for exactly one more once one is removed.
    assert_disassociated("the first certificate", &disassociate(&emulator, &arn_at(0)).await);
    assert_associated(
        "one more after a removal",
        &associate(&emulator, &arn_at(MAX_CERTIFICATES)).await,
    );
    let refused = associate(&emulator, &arn_at(MAX_CERTIFICATES + 1)).await;
    assert_eq!(
        refused.status, 402,
        "case back at capacity: expected HTTP 402, got {} with body {}",
        refused.status, refused.body
    );
}
