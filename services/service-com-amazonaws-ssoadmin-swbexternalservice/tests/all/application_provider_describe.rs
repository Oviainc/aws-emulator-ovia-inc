// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationProvider` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an application provider is one of AWS's own — the model's
//! `ApplicationProviderArn` pattern pins the owner segment to `aws`
//! (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`) and no
//! operation of this service creates one — so `DescribeApplicationProvider` REPORTS the
//! built-in provider its ARN names, echoing the ARN asked for with that provider's
//! `FederationProtocol` and display data, and refuses every other ARN the pattern admits
//! with the modelled `ResourceNotFoundException`, at the HTTP 400 this awsJson1_1 service
//! answers every refusal of a request with. It writes nothing.
//!
//! The table is the service's own catalogue
//! ([`state::BUILT_IN_APPLICATION_PROVIDERS`], read from the crate so the test tracks it
//! rather than restating it) crossed with every partition the pattern admits, plus a row
//! for each way an ARN can name no provider. `custom` is the entry the recorded case
//! cited; `athena` and the rest are asserted the same way.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;

use crate::ssoadmin_permission_sets::{call, error_code, router};

/// The partitions `aws(-[a-z]{1,5}){0,3}` admits: a provider is AWS's own in each of
/// them, and the ARN reported back must be the one asked for.
const PARTITIONS: &[&str] = &["aws", "aws-us-gov", "aws-cn", "aws-iso", "aws-iso-b"];

/// The ARN of the built-in provider `id` in `partition`.
fn provider_arn(partition: &str, id: &str) -> String {
    format!("arn:{partition}:sso::aws:applicationProvider/{id}")
}

/// RULE — every entry of the catalogue describes, in every partition, from the catalogue
/// record and under the ARN the caller asked for.
#[tokio::test]
async fn every_built_in_provider_describes_in_every_partition() {
    let router = router();
    assert!(
        state::BUILT_IN_APPLICATION_PROVIDERS.len() > 1,
        "the catalogue must hold more than the one provider the recorded case cited"
    );

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        for partition in PARTITIONS {
            let arn = provider_arn(partition, provider.id);
            let (status, body) = call(
                &router,
                "DescribeApplicationProvider",
                &json!({"ApplicationProviderArn": arn}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {arn}: a built-in provider expected 200, got {status} {body}"
            );
            assert_eq!(
                body["ApplicationProviderArn"].as_str(),
                Some(arn.as_str()),
                "case {arn}: the ARN asked for must be the ARN reported, got {:?}",
                body["ApplicationProviderArn"]
            );
            assert_eq!(
                body["FederationProtocol"].as_str(),
                Some(provider.federation_protocol),
                "case {arn}: expected FederationProtocol {:?}, got {:?}",
                provider.federation_protocol,
                body["FederationProtocol"]
            );
            // The model's `FederationProtocol` enum is exactly `SAML | OAUTH`.
            assert!(
                ["SAML", "OAUTH"].contains(&provider.federation_protocol),
                "case {arn}: {:?} is not a modelled FederationProtocol",
                provider.federation_protocol
            );
            assert_eq!(
                body["DisplayData"]["DisplayName"].as_str(),
                Some(provider.display_name),
                "case {arn}: expected DisplayName {:?}, got {:?}",
                provider.display_name,
                body["DisplayData"]
            );
            // Nothing holds an icon, a blurb or a resource server for these providers,
            // so none is invented.
            for absent in ["IconUrl", "Description"] {
                assert_eq!(
                    body["DisplayData"][absent],
                    Value::Null,
                    "case {arn}: no {absent} is held, so none may be reported, got {:?}",
                    body["DisplayData"][absent]
                );
            }
            assert_eq!(
                body["ResourceServerConfig"],
                Value::Null,
                "case {arn}: no resource server is held, so none may be reported, got {:?}",
                body["ResourceServerConfig"]
            );
        }
    }
}

/// The two entries worth naming explicitly: `custom`, the provider the recorded case
/// describes, and `athena`, an entry it did not cite.
#[tokio::test]
async fn the_cited_and_an_uncited_entry_both_describe() {
    let router = router();

    for (id, federation_protocol) in [("custom", "SAML"), ("athena", "OAUTH")] {
        let arn = provider_arn("aws", id);
        let (status, body) = call(
            &router,
            "DescribeApplicationProvider",
            &json!({"ApplicationProviderArn": arn}),
        )
        .await;
        assert_eq!(status, 200, "case {id}: expected 200, got {status} {body}");
        assert_eq!(
            body["ApplicationProviderArn"].as_str(),
            Some(arn.as_str()),
            "case {id}: expected the ARN echoed, got {:?}",
            body["ApplicationProviderArn"]
        );
        assert_eq!(
            body["FederationProtocol"].as_str(),
            Some(federation_protocol),
            "case {id}: expected FederationProtocol {federation_protocol:?}, got {:?}",
            body["FederationProtocol"]
        );
    }
}

/// RULE — any other ARN the pattern admits names no provider:
/// `ResourceNotFoundException`, never a 200 and never a 501.
#[tokio::test]
async fn an_arn_outside_the_catalogue_is_not_found() {
    let router = router();

    // The rows differ along the ways an ARN can miss the catalogue: a name nothing
    // publishes, a near miss of a real id, the wrong case, a path under a real id, and a
    // real id with a suffix.
    let unknown: Vec<&str> = vec![
        "sim2real-behav-not-a-real-provider",
        "custom2",
        "CUSTOM",
        "custom/extra",
        "athena-x",
        "nothing/at/all",
    ];
    for id in unknown {
        for partition in PARTITIONS {
            let arn = provider_arn(partition, id);
            let (status, body) = call(
                &router,
                "DescribeApplicationProvider",
                &json!({"ApplicationProviderArn": arn}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {arn}: an unknown provider expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {arn}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// An `ApplicationProviderArn` outside the modelled pattern is a BAD REQUEST, refused at
/// the boundary before the handler runs: the rows span the ways the pattern and its
/// `length 10..1224` can be missed.
#[tokio::test]
async fn an_arn_outside_the_pattern_is_a_validation_error() {
    let router = router();

    let rows: Vec<(&str, &str)> = vec![
        ("empty", ""),
        ("too short", "arn:aws:s"),
        ("not an arn at all", "custom"),
        // The owner segment is `aws`: a customer account never owns a provider.
        (
            "an account-owned provider",
            "arn:aws:sso::123456789012:applicationProvider/custom",
        ),
        // Another service's ARN.
        ("another service", "arn:aws:iam::123456789012:role/demo"),
        // The id class is `[a-zA-Z0-9-/]`: an underscore is outside it.
        (
            "an id outside the character class",
            "arn:aws:sso::aws:applicationProvider/cus_tom",
        ),
        // No id at all.
        ("an empty id", "arn:aws:sso::aws:applicationProvider/"),
    ];
    for (label, arn) in rows {
        let (status, body) = call(
            &router,
            "DescribeApplicationProvider",
            &json!({"ApplicationProviderArn": arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for {arn:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException for {arn:?}, got {body}"
        );
    }
}

/// Describing a provider is a READ: two describes report the identical record, and it
/// changes nothing a later caller sees.
#[tokio::test]
async fn describing_a_provider_changes_nothing() {
    let router = router();
    let arn = provider_arn("aws", "custom");
    let request = json!({"ApplicationProviderArn": arn});

    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");

    let (_, first) = call(&router, "DescribeApplicationProvider", &request).await;
    let (_, second) = call(&router, "DescribeApplicationProvider", &request).await;
    assert_eq!(
        first, second,
        "two describes of one provider must report the identical record, got {first} \
         then {second}"
    );

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a read must not change what a later caller sees, got {before} then {after}"
    );
}
