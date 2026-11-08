// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level gate on registering as an extension publisher, end to end over the
//! awsQuery wire.
//!
//! The rule every case here holds the service to: **`RegisterPublisher` registers a publisher
//! only for an ACCOUNT that is already enrolled as an Amazon Web Services Marketplace seller
//! — an enrolment no API call performs — refusing one that is not with the registry's own
//! modelled `CFNRegistryException` (400) carrying the Marketplace-seller sentence and writing
//! nothing, so every operation that asks whether this account is a publisher goes on
//! answering the not-a-publisher refusal.**
//!
//! The table spans what the rule generalises over: every form a registration that accepts the
//! terms can take (no identity verification, a connection, a connection in another
//! partition), the same rows on an enrolled and on an unenrolled account so the gate is the
//! only difference between them, each operation downstream of the gate, and the ways a
//! request might try to assert the enrolment itself.
//!
//! `publisher_registration.rs` is the suite for what a registration WRITES once the gate is
//! open; this one is the gate.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness, so what it asserts is what a client sees.

use service_com_amazonaws_cloudformation_cloudformation::state::publisher::{
    MarketplaceSellerEnrollment, NOT_A_MARKETPLACE_SELLER,
};
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registry_type, StoredExtension,
};

use crate::stack_lifecycle::{
    call, enroll_as_a_marketplace_seller, member, router_with_engine, store, xml_text, ACCOUNT_ID,
    REGION,
};

/// A private extension this account's registry DOES hold, for the row whose operation
/// resolves the extension it was asked about before it reads the caller.
const REGISTERED: &str = "My::Registered::Resource";

/// The refusal AWS answers an account that has not registered as a publisher, which is what
/// `TestType` requires of its caller. Entirely fixed text.
const TEST_TYPE_NOT_A_PUBLISHER: &str =
    "You must register as a publisher with CloudFormation to call test-type.";

/// The refusal AWS answers a `PublishType` call from an account that is not a verified
/// publisher: fixed wording whose one variable part is the calling account's 12-digit id.
fn publish_type_not_a_publisher() -> String {
    format!(
        "The account {ACCOUNT_ID} is not verified as a publisher. Please register as \
         publisher first"
    )
}

/// Hold a refused call to the registry's modelled refusal — HTTP 400 `CFNRegistryException` —
/// carrying exactly `message`.
fn assert_registry_refusal(case: &str, status_code: u16, body: &str, message: &str) {
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("CFNRegistryException"),
        "case {case:?}: expected CFNRegistryException, got {body}"
    );
    let reported = xml_text(body, "Message");
    assert_eq!(
        reported.as_deref(),
        Some(message),
        "case {case:?}: expected {message:?}, got {reported:?}"
    );
}

/// Every form a registration that DOES accept the terms and conditions can take: the gate is
/// about the ACCOUNT, so none of them changes what an unenrolled account is answered, and
/// each of them registers once the account is enrolled.
fn accepting_registrations() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "accepting the terms, verified by the account's own Marketplace account",
            vec![member("AcceptTermsAndConditions", "true")],
        ),
        (
            "accepting the terms, verified by a source-control connection",
            vec![
                member("AcceptTermsAndConditions", "true"),
                member(
                    "ConnectionArn",
                    "arn:aws:codestar-connections:us-east-1:000000000000:connection/\
                     11111111-2222-3333-4444-555555555555",
                ),
            ],
        ),
        (
            "accepting the terms, verified by a connection in another partition",
            vec![
                member("AcceptTermsAndConditions", "true"),
                member(
                    "ConnectionArn",
                    "arn:aws-us-gov:codeconnections:us-gov-west-1:000000000000:connection/\
                     66666666-7777-8888-9999-aaaaaaaaaaaa",
                ),
            ],
        ),
    ]
}

#[tokio::test]
async fn an_account_that_is_not_a_marketplace_seller_cannot_register_as_a_publisher() {
    let (router, engine) = router_with_engine();
    // A fresh account is an account nobody enrolled, which is what the gate is about.
    assert_eq!(
        store(&engine).lock().marketplace_seller,
        MarketplaceSellerEnrollment::not_enrolled(),
        "case \"a fresh account\": expected an account nobody enrolled"
    );

    for (case, members) in accepting_registrations() {
        let (status_code, body) = call(&router, "RegisterPublisher", &members).await;
        assert_registry_refusal(case, status_code, &body, NOT_A_MARKETPLACE_SELLER);
        // The refusal hands out no publisher id, and writes no registration.
        assert_eq!(
            xml_text(&body, "PublisherId"),
            None,
            "case {case:?}: expected no PublisherId in the refusal, got {body}"
        );
        assert!(
            store(&engine).lock().publisher.is_none(),
            "case {case:?}: expected the refused registration to store nothing, got {:?}",
            store(&engine).lock().publisher
        );
    }

    // And because nothing registered, every operation downstream of "is this account a
    // publisher" goes on answering its own refusal. Both `TestType` and `PublishType` read the
    // caller before the extension the request names, so the row below names a record the
    // registry does hold: what each call is refused for is the caller.
    {
        let state = store(&engine);
        state.lock().extensions.push(StoredExtension::new(
            REGION,
            registry_type::RESOURCE,
            REGISTERED,
        ));
    }
    let (status_code, body) = call(&router, "DescribePublisher", &[]).await;
    assert_registry_refusal(
        "DescribePublisher after a refused registration",
        status_code,
        &body,
        &format!(
            "The account [{ACCOUNT_ID}] is not registered as a publisher, if you want to \
             describe a specific publisher, please specify the publisherID"
        ),
    );
    let publish_type_refusal = publish_type_not_a_publisher();
    for (case, action, type_name, expected) in [
        (
            "TestType after a refused registration",
            "TestType",
            REGISTERED,
            TEST_TYPE_NOT_A_PUBLISHER,
        ),
        (
            "PublishType after a refused registration",
            "PublishType",
            REGISTERED,
            publish_type_refusal.as_str(),
        ),
    ] {
        let (status_code, body) = call(
            &router,
            action,
            &[
                member("Type", registry_type::RESOURCE),
                member("TypeName", type_name),
            ],
        )
        .await;
        assert_registry_refusal(case, status_code, &body, expected);
    }
}

#[tokio::test]
async fn an_enrolled_seller_registers_and_the_gate_is_the_only_difference() {
    // The same requests the rows above were refused, on an account the operator has said is
    // an enrolled seller: each one registers, and the registration is what makes the read of
    // it answer.
    for (case, members) in accepting_registrations() {
        let (router, engine) = router_with_engine();
        enroll_as_a_marketplace_seller(&engine);

        let (status_code, body) = call(&router, "RegisterPublisher", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        let publisher_id = xml_text(&body, "PublisherId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a PublisherId, got {body}"));
        // The modelled `PublisherId`: length 1..40, `^[0-9a-zA-Z]{12,40}$`
        // (`smithy_query(query="shape", shape="PublisherId")`).
        let length = publisher_id.chars().count();
        assert!(
            (12..=40).contains(&length)
                && publisher_id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected a modelled PublisherId, got {publisher_id:?}"
        );
        assert_eq!(
            store(&engine)
                .lock()
                .publisher
                .as_ref()
                .map(|registered| registered.publisher_id.clone()),
            Some(publisher_id.clone()),
            "case {case:?}: expected the answered id persisted"
        );

        let (status_code, body) = call(&router, "DescribePublisher", &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the registration readable, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "PublisherId").as_deref(),
            Some(publisher_id.as_str()),
            "case {case:?}: expected the registered id read back, got {body}"
        );
    }
}

/// Nothing a request can send enrols the account: the enrolment is made outside this API, so
/// repeating the call, or spelling the acceptance differently, changes nothing.
#[tokio::test]
async fn no_request_member_enrols_the_account() {
    let (router, engine) = router_with_engine();
    for (case, members) in [
        (
            "a second attempt after the first was refused",
            vec![member("AcceptTermsAndConditions", "true")],
        ),
        (
            "AcceptTermsAndConditions spelled in upper case",
            vec![member("AcceptTermsAndConditions", "TRUE")],
        ),
        (
            "a registration naming a publisher id of its own",
            vec![
                member("AcceptTermsAndConditions", "true"),
                member("PublisherId", "abcdef123456"),
            ],
        ),
    ] {
        let (status_code, body) = call(&router, "RegisterPublisher", &members).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the registration refused, got {status_code} {body}"
        );
        assert!(
            store(&engine).lock().publisher.is_none(),
            "case {case:?}: expected no registration written, got {:?}",
            store(&engine).lock().publisher
        );
        assert_eq!(
            store(&engine).lock().marketplace_seller,
            MarketplaceSellerEnrollment::not_enrolled(),
            "case {case:?}: expected the account still not enrolled"
        );
    }
}
