// SPDX-License-Identifier: CC0-1.0

//! `DescribePublisher`, end to end over the awsQuery wire: what a read of an extension
//! publisher reports, and how it refuses one no registration stands behind.
//!
//! The rule every case here holds the service to: **a publisher read answers the stored
//! registration the request ADDRESSES — the one its `PublisherId` names, or the calling
//! account's own where the request names none — reporting that record's id, its verified
//! status and the identity it was verified by; a publisher the store holds no registration
//! for is refused with the registry's modelled `CFNRegistryException` (400), and the refusal
//! of a request that named no id is the registry's own account wording, carrying the
//! 12-digit account id the request was signed under; a read writes nothing either way.**
//!
//! The table spans the class the rule generalises over: each way a read can address a
//! publisher (naming no id, naming the registered one, naming another well-formed one),
//! every form a well-formed `PublisherId` can take (the shortest the modelled pattern
//! admits, the longest, and a mixed-case one between them), the Region the read is signed
//! for — "this publisher ID applies to your account in all Amazon Web Services Regions" —
//! and whether this account has registered at all.
//!
//! `publisher_registration.rs` is the suite for what `RegisterPublisher` WRITES; this one is
//! about the read of it.

use crate::publisher_registration::assert_registry_refusal;
use crate::stack_lifecycle::{
    call, call_in, member, router_with_engine, store, xml_text, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::publisher::{
    identity_provider, publisher_status,
};
use smithy_server_runtime::MultiServiceRouter;

/// A second Region every row is repeated in: the registration is the ACCOUNT's, so the
/// Region a read is signed for selects nothing.
const OTHER_REGION: &str = "us-west-2";

/// The Regions every row is driven from.
const REGIONS: &[&str] = &[REGION, OTHER_REGION];

/// `PublisherId`s nobody registered under, spanning the modelled form (length 1..40,
/// `@pattern("^[0-9a-zA-Z]{12,40}$")` — `smithy_query(query="shape", shape="PublisherId")`):
/// the shortest the pattern admits, a mixed-case one, and the longest it admits.
const UNREGISTERED_IDS: &[(&str, &str)] = &[
    ("the shortest well-formed id", "abcdefghijkl"),
    ("a mixed-case well-formed id", "AbCdEf123456GhIjKl"),
    (
        "the longest well-formed id",
        "0123456789abcdefghijABCDEFGHIJ0123456789",
    ),
];

async fn describe_publisher(
    router: &MultiServiceRouter,
    region: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, region, "DescribePublisher", members).await
}

/// Hold a refused read of this ACCOUNT's own registration to the registry's modelled
/// refusal — HTTP 400 `CFNRegistryException` ([`assert_registry_refusal`]) — and to the
/// wording that refusal carries.
fn assert_account_refusal(case: &str, status_code: u16, body: &str, expected_message: &str) {
    assert_registry_refusal(case, status_code, body);
    let message = xml_text(body, "Message");
    assert_eq!(
        message.as_deref(),
        Some(expected_message),
        "case {case:?}: expected the message {expected_message:?}, got {message:?}"
    );
}

/// Hold a refused read that NAMED a publisher to the same modelled refusal, and to a
/// message about the publisher the request asked about.
///
/// What is asserted here is the part the request derives — the id it named — rather than the
/// sentence around it: this error class has no recorded wording to hold the prose to, unlike
/// the account refusal above.
fn assert_refusal_naming(case: &str, status_code: u16, body: &str, publisher_id: &str) {
    assert_registry_refusal(case, status_code, body);
    let message = xml_text(body, "Message");
    assert!(
        message
            .as_deref()
            .is_some_and(|message| message.contains(publisher_id)),
        "case {case:?}: expected a message about {publisher_id:?}, got {message:?}"
    );
}

/// The refusal a read that named no `PublisherId` gets while this account holds no
/// registration: fixed wording, with the account the request was signed under as its only
/// variable part.
fn account_not_registered(account_id: &str) -> String {
    format!(
        "The account [{account_id}] is not registered as a publisher, if you want to \
         describe a specific publisher, please specify the publisherID"
    )
}

#[tokio::test]
async fn a_read_of_a_publisher_no_registration_stands_behind_is_refused() {
    let (router, engine) = router_with_engine();

    for region in REGIONS {
        // A read that names no id means this account's own registration, which it has not
        // made: the refusal is about the ACCOUNT and names it.
        let case = format!("a read naming no PublisherId, signed for {region}");
        let (status_code, body) = describe_publisher(&router, region, &[]).await;
        assert_account_refusal(
            &case,
            status_code,
            &body,
            &account_not_registered(ACCOUNT_ID),
        );

        // A read that names one addresses THAT publisher, so the refusal names it instead.
        for (form, publisher_id) in UNREGISTERED_IDS {
            let case = format!("a read naming {form} ({publisher_id}), signed for {region}");
            let (status_code, body) =
                describe_publisher(&router, region, &[member("PublisherId", publisher_id)]).await;
            assert_refusal_naming(&case, status_code, &body, publisher_id);
        }
    }

    // Nothing was written by any of it: a read is a report.
    assert!(
        store(&engine).lock().publisher.is_none(),
        "case \"refused reads\": expected no registration written, got {:?}",
        store(&engine).lock().publisher
    );
}

#[tokio::test]
async fn a_read_reports_the_registration_the_request_addresses() {
    let (router, engine) = router_with_engine();

    // The registration every row below reads back, written through the wire that writes it.
    let (status_code, body) = call(
        &router,
        "RegisterPublisher",
        &[member("AcceptTermsAndConditions", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"registering as a publisher\": expected 200, got {status_code} {body}"
    );
    let publisher_id = xml_text(&body, "PublisherId").unwrap_or_else(|| {
        panic!("case \"registering as a publisher\": expected a PublisherId, got {body}")
    });

    let before = format!("{:?}", store(&engine).lock().publisher);
    for region in REGIONS {
        for (addressing, members) in [
            ("naming no PublisherId", Vec::new()),
            (
                "naming the registered PublisherId",
                vec![member("PublisherId", &publisher_id)],
            ),
        ] {
            let case = format!("a read {addressing}, signed for {region}");
            let (status_code, body) = describe_publisher(&router, region, &members).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected 200, got {status_code} {body}"
            );
            let reported = xml_text(&body, "PublisherId");
            assert_eq!(
                reported.as_deref(),
                Some(publisher_id.as_str()),
                "case {case:?}: expected the registered id, got {reported:?}"
            );
            // "Currently, all registered publishers are verified."
            let status = xml_text(&body, "PublisherStatus");
            assert_eq!(
                status.as_deref(),
                Some(publisher_status::VERIFIED),
                "case {case:?}: expected a verified publisher, got {status:?}"
            );
            // The registration supplied no connection, so it was verified through the
            // account's own Amazon Web Services Marketplace account.
            let provider = xml_text(&body, "IdentityProvider");
            assert_eq!(
                provider.as_deref(),
                Some(identity_provider::AWS_MARKETPLACE),
                "case {case:?}: expected the identity the registration was verified by, got \
                 {provider:?}"
            );
        }

        // A well-formed id nobody registered under still names no publisher, even now that
        // this account has a registration of its own.
        for (form, other_id) in UNREGISTERED_IDS {
            let case = format!("a registered account reading {form} ({other_id}) in {region}");
            let (status_code, body) =
                describe_publisher(&router, region, &[member("PublisherId", other_id)]).await;
            assert_refusal_naming(&case, status_code, &body, other_id);
        }
    }

    // None of those reads moved the registration they reported.
    assert_eq!(
        format!("{:?}", store(&engine).lock().publisher),
        before,
        "case \"reads of a registered publisher\": expected the registration left alone"
    );
}
