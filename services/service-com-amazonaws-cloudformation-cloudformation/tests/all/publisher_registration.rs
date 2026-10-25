// SPDX-License-Identifier: CC0-1.0

//! The extension-publisher registration, end to end over the awsQuery wire:
//! `RegisterPublisher` and the `DescribePublisher` read of what it wrote.
//!
//! The rule every case here holds the service to: **`RegisterPublisher` registers the
//! CALLING ACCOUNT as a publisher of public extensions only when the request accepts the
//! terms and conditions — refusing one that does not with the registry's own modelled
//! `CFNRegistryException` (400) and storing nothing — and the registration it writes
//! carries one publisher id, minted once and the same in every Region, which
//! `DescribePublisher` reports back together with the publisher's verified status and the
//! identity the registration was verified by; a publisher the store holds no registration
//! for is answered with that same modelled refusal rather than with a record.**
//!
//! The table spans the class the rule generalises over: every form the acceptance member
//! can take (absent, false, true), every form the identity verification can take (none, a
//! connection in each of the two services a connection ARN names), the Region the call is
//! signed for, and each way a read can address a publisher (by the id the registration
//! minted, by another well-formed id, and by naming none at all).
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling these two operations sees.

use crate::stack_lifecycle::{call, call_in, member, router_with_engine, store, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::ops::types::{
    IDENTITY_PROVIDER_VALUES, PUBLISHER_STATUS_VALUES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::publisher::{
    identity_provider, publisher_status,
};
use smithy_server_runtime::MultiServiceRouter;

/// Another Region, for the rows that hold the registration to applying "to your account in
/// all Amazon Web Services Regions".
const OTHER_REGION: &str = "us-west-2";

/// A `PublisherId` nobody registered under, in the modelled form
/// (`@pattern("^[0-9a-zA-Z]{12,40}$")`).
const UNREGISTERED_PUBLISHER_ID: &str = "abcdef123456";

/// The connection ARNs a registration can be verified by, one per service a connection to a
/// source-control account lives in, and one of them in another partition — all inside the
/// modelled `ConnectionArn` pattern (`^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`).
const CODESTAR_CONNECTION_ARN: &str =
    "arn:aws:codestar-connections:us-east-1:000000000000:connection/\
     11111111-2222-3333-4444-555555555555";
const CODECONNECTIONS_ARN: &str =
    "arn:aws-us-gov:codeconnections:us-gov-west-1:000000000000:connection/\
     66666666-7777-8888-9999-aaaaaaaaaaaa";

async fn register_publisher(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "RegisterPublisher", members).await
}

async fn describe_publisher(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "DescribePublisher", members).await
}

/// Hold a reported id to the modelled `PublisherId` form: length 1..40 and
/// `^[0-9a-zA-Z]{12,40}$` (`smithy_query(query="shape", shape="PublisherId")`).
fn assert_publisher_id_form(case: &str, publisher_id: &str) {
    let length = publisher_id.chars().count();
    assert!(
        (12..=40).contains(&length) && publisher_id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a modelled PublisherId, got {publisher_id:?}"
    );
}

/// Hold a refused call to the registry's modelled refusal: HTTP 400 `CFNRegistryException`.
///
/// Shared with `publisher_reads.rs`, the suite for what a publisher READ answers, so both
/// hold a refusal to the same status and wire code.
pub(crate) fn assert_registry_refusal(case: &str, status_code: u16, body: &str) {
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("CFNRegistryException"),
        "case {case:?}: expected CFNRegistryException, got {body}"
    );
}

/// Every form of a registration that does not accept the terms and conditions, which is
/// what the operation refuses: "you must accept the terms and conditions in order to
/// register to publish public extensions to the CloudFormation registry. The default is
/// false."
fn unaccepted_registrations() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        ("no AcceptTermsAndConditions at all", Vec::new()),
        (
            "AcceptTermsAndConditions false",
            vec![member("AcceptTermsAndConditions", "false")],
        ),
        (
            "AcceptTermsAndConditions false beside a connection",
            vec![
                member("AcceptTermsAndConditions", "false"),
                member("ConnectionArn", CODESTAR_CONNECTION_ARN),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_registration_that_does_not_accept_the_terms_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();

    // Before any registration there is nothing to describe: a read is refused rather than
    // answered with a publisher nothing stands behind.
    let (status_code, body) = describe_publisher(&router, &[]).await;
    assert_registry_refusal("a read before any registration", status_code, &body);
    let (status_code, body) =
        describe_publisher(&router, &[member("PublisherId", UNREGISTERED_PUBLISHER_ID)]).await;
    assert_registry_refusal("a read of an unregistered PublisherId", status_code, &body);

    for (case, members) in unaccepted_registrations() {
        let (status_code, body) = register_publisher(&router, &members).await;
        assert_registry_refusal(case, status_code, &body);
        assert!(
            store(&engine).lock().publisher.is_none(),
            "case {case:?}: expected the refused registration to store nothing, got {:?}",
            store(&engine).lock().publisher
        );
        // And no id was handed out with the refusal.
        assert_eq!(
            xml_text(&body, "PublisherId"),
            None,
            "case {case:?}: expected no PublisherId in the refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn registering_mints_one_account_wide_publisher_id_a_read_reports_back() {
    let (router, engine) = router_with_engine();

    let (status_code, body) =
        register_publisher(&router, &[member("AcceptTermsAndConditions", "true")]).await;
    assert_eq!(
        status_code, 200,
        "case \"a registration accepting the terms\": expected 200, got {status_code} {body}"
    );
    let publisher_id = xml_text(&body, "PublisherId").unwrap_or_else(|| {
        panic!("case \"a registration accepting the terms\": expected a PublisherId, got {body}")
    });
    assert_publisher_id_form("a registration accepting the terms", &publisher_id);
    assert_eq!(
        store(&engine)
            .lock()
            .publisher
            .as_ref()
            .map(|registered| registered.publisher_id.clone()),
        Some(publisher_id.clone()),
        "case \"a registration accepting the terms\": expected the answered id persisted"
    );

    // Each way a read can address the registration, including from another Region: the
    // publisher id "applies to your account in all Amazon Web Services Regions".
    for (case, region, members) in [
        ("a read naming no PublisherId", None, Vec::new()),
        (
            "a read naming the registered PublisherId",
            None,
            vec![member("PublisherId", &publisher_id)],
        ),
        ("a read from another Region", Some(OTHER_REGION), Vec::new()),
        (
            "a read from another Region naming the id",
            Some(OTHER_REGION),
            vec![member("PublisherId", &publisher_id)],
        ),
    ] {
        let (status_code, body) = match region {
            Some(region) => call_in(&router, region, "DescribePublisher", &members).await,
            None => describe_publisher(&router, &members).await,
        };
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "PublisherId").as_deref(),
            Some(publisher_id.as_str()),
            "case {case:?}: expected the registered id, got {body}"
        );
        // "Currently, all registered publishers are verified", and the value is one the
        // modelled `PublisherStatus` enum admits.
        let status = xml_text(&body, "PublisherStatus");
        assert_eq!(
            status.as_deref(),
            Some(publisher_status::VERIFIED),
            "case {case:?}: expected a verified publisher, got {body}"
        );
        assert!(
            status
                .as_deref()
                .is_some_and(|status| PUBLISHER_STATUS_VALUES.contains(&status)),
            "case {case:?}: expected a modelled PublisherStatus, got {status:?}"
        );
    }

    // A well-formed id nobody registered under names no publisher, even once this account
    // has one.
    let (status_code, body) =
        describe_publisher(&router, &[member("PublisherId", UNREGISTERED_PUBLISHER_ID)]).await;
    assert_registry_refusal(
        "a read of another well-formed PublisherId",
        status_code,
        &body,
    );

    // `@idempotent`: registering again re-verifies the account and answers the id it
    // already has, and so does a registration that supplies a connection this time.
    for (case, members) in [
        (
            "registering again",
            vec![member("AcceptTermsAndConditions", "true")],
        ),
        (
            "registering again with a connection",
            vec![
                member("AcceptTermsAndConditions", "true"),
                member("ConnectionArn", CODESTAR_CONNECTION_ARN),
            ],
        ),
    ] {
        let (status_code, body) = register_publisher(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "PublisherId").as_deref(),
            Some(publisher_id.as_str()),
            "case {case:?}: expected the same publisher id, got {body}"
        );
    }

    // A refused registration leaves the record this account already has exactly as it was.
    for (case, members) in unaccepted_registrations() {
        let (status_code, body) = register_publisher(&router, &members).await;
        assert_registry_refusal(case, status_code, &body);
        let (_, body) = describe_publisher(&router, &[]).await;
        assert_eq!(
            xml_text(&body, "PublisherId").as_deref(),
            Some(publisher_id.as_str()),
            "case {case:?}: expected the stored registration untouched, got {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_identity_provider_is_the_one_the_registration_was_verified_by() {
    // One row per form the identity verification a registration supplies can take. The
    // reported `IdentityProvider` is "the type of account used as the identity provider
    // when registering this publisher": the account's own Amazon Web Services Marketplace
    // account where the registration supplied no connection, and — where it supplied one —
    // the Bitbucket or GitHub account that connection is to, which is a property of the
    // connection rather than of its ARN, so it is reported by nothing here.
    for (case, connection_arn, expected) in [
        (
            "a registration with no connection",
            None,
            Some(identity_provider::AWS_MARKETPLACE),
        ),
        (
            "a registration with a CodeStar Connections connection",
            Some(CODESTAR_CONNECTION_ARN),
            None,
        ),
        (
            "a registration with a CodeConnections connection",
            Some(CODECONNECTIONS_ARN),
            None,
        ),
    ] {
        // A fresh store per row: an account registers once, so each form of registration is
        // the FIRST one its own account made.
        let (router, engine) = router_with_engine();
        let mut members = vec![member("AcceptTermsAndConditions", "true")];
        if let Some(connection_arn) = connection_arn {
            members.push(member("ConnectionArn", connection_arn));
        }
        let (status_code, body) = register_publisher(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            store(&engine)
                .lock()
                .publisher
                .as_ref()
                .and_then(|registered| registered.connection_arn.clone()),
            connection_arn.map(str::to_string),
            "case {case:?}: expected the supplied verification persisted"
        );

        let (status_code, body) = describe_publisher(&router, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        let reported = xml_text(&body, "IdentityProvider");
        assert_eq!(
            reported.as_deref(),
            expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // Whatever is reported is a value the modelled `IdentityProvider` enum admits
        // (`smithy_query(query="shape", shape="IdentityProvider")`).
        assert!(
            reported
                .as_deref()
                .is_none_or(|provider| IDENTITY_PROVIDER_VALUES.contains(&provider)),
            "case {case:?}: expected a modelled IdentityProvider, got {reported:?}"
        );
        // "The URL to the publisher's profile with the identity provider" is a page held by
        // that provider, which no registration carries: nothing invents one.
        assert_eq!(
            xml_text(&body, "PublisherProfile"),
            None,
            "case {case:?}: expected no invented PublisherProfile, got {body}"
        );
    }
}

#[tokio::test]
async fn a_publisher_read_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let (status_code, body) =
        register_publisher(&router, &[member("AcceptTermsAndConditions", "true")]).await;
    assert_eq!(
        status_code, 200,
        "case \"a registration accepting the terms\": expected 200, got {status_code} {body}"
    );

    let before = format!("{:?}", store(&engine).lock().publisher);
    for (case, members) in [
        ("a read naming no PublisherId", Vec::new()),
        (
            "a read of an unregistered PublisherId",
            vec![member("PublisherId", UNREGISTERED_PUBLISHER_ID)],
        ),
    ] {
        let _ = describe_publisher(&router, &members).await;
        assert_eq!(
            format!("{:?}", store(&engine).lock().publisher),
            before,
            "case {case:?}: expected the read to leave the registration alone"
        );
    }
}
