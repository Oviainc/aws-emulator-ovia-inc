// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationProvider` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: an application provider is one of AWS's own — the model's
//! `ApplicationProviderArn` pattern pins the owner segment to `aws`
//! (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`) and no
//! operation of this service creates one — so `DescribeApplicationProvider` REPORTS the
//! catalogue entry THE REQUESTED ARN names, echoing the ARN asked for with that entry's
//! `FederationProtocol`, its COMPLETE `DisplayData` (`DisplayName`, `Description` and an
//! `https` `IconUrl`) and a `ResourceServerConfig` exactly when the entry publishes
//! access scopes, and refuses every other ARN the pattern admits with the modelled
//! `ResourceNotFoundException`, at the HTTP 400 this awsJson1_1 service answers every
//! refusal of a request with. It writes nothing.
//!
//! The table is the service's own catalogue
//! ([`state::BUILT_IN_APPLICATION_PROVIDERS`], read from the crate so the test tracks it
//! rather than restating it), plus every partition the pattern admits on a sample of it
//! and a row for each way an ARN can name no provider. `custom` and `catalog/Statuspage`
//! are the entries the recorded cases cited; the rest are entries they did not.

use serde_json::json;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;

use crate::application_providers::{assert_display_data, assert_resource_server_config};
use crate::ssoadmin_permission_sets::{call, error_code, router};

/// The partitions `aws(-[a-z]{1,5}){0,3}` admits: a provider is AWS's own in each of
/// them, and the ARN reported back must be the one asked for.
const PARTITIONS: &[&str] = &["aws", "aws-us-gov", "aws-cn", "aws-iso", "aws-iso-b"];

/// The ARN of the built-in provider `id` in `partition`.
fn provider_arn(partition: &str, id: &str) -> String {
    format!("arn:{partition}:sso::aws:applicationProvider/{id}")
}

/// RULE — every entry of the catalogue describes from its own record, under the ARN the
/// caller asked for, with a complete `DisplayData` and the scopes it publishes.
#[tokio::test]
async fn every_built_in_provider_describes_from_its_own_record() {
    let router = router();
    assert!(
        state::BUILT_IN_APPLICATION_PROVIDERS.len() > 1,
        "the catalogue must hold more than the one provider the recorded case cited"
    );

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        let arn = provider_arn("aws", provider.id);
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
        assert_display_data(&body["DisplayData"], provider);
        assert_resource_server_config(&body["ResourceServerConfig"], provider);
    }
}

/// RULE — a provider exists in every partition the pattern admits, and the ARN reported
/// is the one asked for. The rows cross the partitions with entries of both id forms:
/// the ones the recorded cases cited and ones they did not.
#[tokio::test]
async fn a_provider_describes_in_every_partition_under_the_arn_asked_for() {
    let router = router();
    let first = state::BUILT_IN_APPLICATION_PROVIDERS
        .first()
        .expect("the catalogue must not be empty")
        .id;
    let last = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .next_back()
        .expect("the catalogue must not be empty")
        .id;

    for id in ["custom", "catalog/Statuspage", "glue", first, last] {
        for partition in PARTITIONS {
            let arn = provider_arn(partition, id);
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
            let described = state::built_in_application_provider(&arn)
                .unwrap_or_else(|| panic!("case {arn}: the catalogue must hold {id}"));
            assert_eq!(
                described.id, id,
                "case {arn}: the ARN must resolve to the provider it names, got {:?}",
                described.id
            );
            assert_display_data(&body["DisplayData"], described);
        }
    }
}

/// RULE — the answer is keyed on the REQUESTED ARN: describing each entry in turn
/// reports that entry, never the first or a neighbouring one. The rows are the two
/// entries the recorded cases cite plus uncited ones, including one that publishes
/// scopes.
#[tokio::test]
async fn the_requested_provider_is_the_one_reported() {
    let router = router();

    // Grounded in the catalogue record, which the crate owns: the row states the id and
    // the test reads the expected values from that record, so a described entry can
    // never be another entry's.
    for id in ["catalog/Statuspage", "custom", "glue", "catalog/Zoom"] {
        let arn = provider_arn("aws", id);
        let record = state::built_in_application_provider(&arn)
            .unwrap_or_else(|| panic!("case {id}: the catalogue must hold {arn}"));
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
            "case {id}: expected the ARN asked for echoed, got {:?}",
            body["ApplicationProviderArn"]
        );
        assert_eq!(
            body["FederationProtocol"].as_str(),
            Some(record.federation_protocol),
            "case {id}: expected FederationProtocol {:?}, got {:?}",
            record.federation_protocol,
            body["FederationProtocol"]
        );
        assert_eq!(
            body["DisplayData"]["DisplayName"].as_str(),
            Some(record.display_name),
            "case {id}: expected DisplayName {:?}, got {:?}",
            record.display_name,
            body["DisplayData"]
        );
        assert_display_data(&body["DisplayData"], record);
        assert_resource_server_config(&body["ResourceServerConfig"], record);
    }
}

/// RULE — any other ARN the pattern admits names no provider:
/// `ResourceNotFoundException`, never a 200 and never a 501.
#[tokio::test]
async fn an_arn_outside_the_catalogue_is_not_found() {
    let router = router();

    // The rows differ along the ways an ARN can miss the catalogue: a name nothing
    // publishes, a near miss of a real id, the wrong case, a path under a real id, a real
    // id with a suffix, and a catalogue application nothing publishes.
    let unknown: Vec<&str> = vec![
        "sim2real-behav-not-a-real-provider",
        "custom2",
        "CUSTOM",
        "custom/extra",
        "athena-x",
        "nothing/at/all",
        "catalog/NoSuchApplication",
        "catalog/statuspage",
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
    assert_eq!(
        status, 200,
        "ListInstances expected 200, got {status} {before}"
    );

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
