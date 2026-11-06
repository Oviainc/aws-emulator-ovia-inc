// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationProvider` / `ListApplicationProviders` display data and the
//! whole-provider envelope, end to end over the `awsJson1_1` wire.
//!
//! Two rules under test, both about the ONE application-provider catalog the two reads
//! project ([`state::BUILT_IN_APPLICATION_PROVIDERS`]):
//!
//! 1. a provider's `DisplayData$Description` is its `DisplayName` as an application —
//!    the display name followed by ` Application` — for EVERY entry of the catalog,
//!    whichever of the two id forms it carries (`catalog/<Name>` for a pre-integrated
//!    application, a service slug for an Amazon Web Services managed one), and its
//!    `DisplayData$IconUrl` is that provider's static portal icon path
//!    (`https://<host>/app-<id>/icons/default.png`). `DescribeApplicationProvider` and
//!    `ListApplicationProviders` compose both through the same builder, so they agree
//!    member for member.
//! 2. `DescribeApplicationProvider` reports the entry it resolved as a whole
//!    `ApplicationProvider` structure BESIDE the flat `ApplicationProviderArn` /
//!    `FederationProtocol` / `DisplayData` / `ResourceServerConfig` members — recorded
//!    behaviour the on-disk model does not declare (the modelled output is the four flat
//!    members: `smithy_query(query="operation",
//!    operation="DescribeApplicationProvider")`) — carrying exactly what those members
//!    carry, under the ARN the caller asked about, and a REFUSED read carries no
//!    provider at all.
//!
//! The tables span what the rules generalise over: the catalog ENTRY (every row of the
//! catalog, read from the crate so the test tracks it rather than restating it — `custom`
//! and `catalog/Statuspage` are the entries the recorded cases cite, the rest are entries
//! they do not), the two id forms, display names of one word and of several, entries with
//! and without published access scopes, every partition the modelled
//! `ApplicationProviderArn` pattern admits, and the ways an ARN can name no provider.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_providers::assert_display_data;
use crate::ssoadmin_permission_sets::{call, error_code, router};

/// The word a provider's portal description ends in: its `DisplayName` as an
/// application. RECORDED behaviour — the entry displayed as `Statuspage` is described
/// `Statuspage Application`, and the custom provider's description is its own display
/// name followed by the same word.
const DESCRIPTION_SUFFIX: &str = "Application";

/// The partitions `aws(-[a-z]{1,5}){0,3}` admits: a provider is AWS's own in each of
/// them, so each is a row the described ARN must come back under.
const PARTITIONS: &[&str] = &["aws", "aws-us-gov", "aws-cn", "aws-iso", "aws-iso-b"];

/// The members `DescribeApplicationProvider` reports flat AND inside the whole-provider
/// structure. Every one of them must agree between the two.
const PROVIDER_MEMBERS: &[&str] = &[
    "ApplicationProviderArn",
    "FederationProtocol",
    "DisplayData",
    "ResourceServerConfig",
];

/// The ARN of the built-in provider `id` in `partition`.
fn provider_arn(partition: &str, id: &str) -> String {
    format!("arn:{partition}:sso::aws:applicationProvider/{id}")
}

/// One `DescribeApplicationProvider` call: `(status, body)`.
async fn describe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeApplicationProvider",
        &json!({"ApplicationProviderArn": arn}),
    )
    .await
}

/// Every entry `ListApplicationProviders` reports, read by paging the enumeration to its
/// end: a response carries at most one page, so the whole catalog is reached by following
/// the `NextToken` until it is absent.
async fn list_all(router: &MultiServiceRouter) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListApplicationProviders", &request).await;
        assert_eq!(
            status, 200,
            "ListApplicationProviders expected 200, got {status} {body}"
        );
        entries.extend(
            body["ApplicationProviders"]
                .as_array()
                .unwrap_or_else(|| panic!("no ApplicationProviders in {body}"))
                .iter()
                .cloned(),
        );
        assert!(
            entries.len() <= state::BUILT_IN_APPLICATION_PROVIDERS.len(),
            "paging must not report more than the catalog, got {} entries",
            entries.len()
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    entries
}

/// RULE 1 — a provider's `Description` is its `DisplayName` as an application, and its
/// `IconUrl` is that provider's static portal icon path. The table is every entry of the
/// catalog, asserted on BOTH reads of it.
#[tokio::test]
async fn every_provider_is_described_as_its_display_name_as_an_application() {
    let router = router();
    let listed = list_all(&router).await;

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        let case = provider.id;
        let arn = provider_arn("aws", case);
        let expected = format!("{} {DESCRIPTION_SUFFIX}", provider.display_name);
        // The composed description must fit the modelled `Description` (1..128) for the
        // row to be a legal one at all; the modelled `Name` admits 1..100.
        assert!(
            (1..=128).contains(&expected.chars().count()),
            "case {case:?}: expected a Description inside the modelled 1..128, got \
             {expected:?}"
        );

        let (status, body) = describe(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["DisplayData"]["Description"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected Description {expected:?}, got {:?}",
            body["DisplayData"]["Description"]
        );
        // The rest of the complete `DisplayData` — the display name, the modelled
        // lengths and the static portal icon path — through the one assertion
        // `application_providers.rs` owns for it.
        assert_display_data(&body["DisplayData"], provider);

        // The enumeration composes the same display data for the same entry.
        let entry = listed
            .iter()
            .find(|entry| entry["ApplicationProviderArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the enumeration must list {arn}"));
        assert_eq!(
            entry["DisplayData"], body["DisplayData"],
            "case {case:?}: the listed DisplayData must be the described one, got {} vs {}",
            entry["DisplayData"], body["DisplayData"]
        );
    }

    // The table really did span both id forms and both shapes of display name, so the
    // rule was asserted over the class and not over one row of it.
    let catalog_forms = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .filter(|provider| provider.id.starts_with("catalog/"))
        .count();
    let slug_forms = state::BUILT_IN_APPLICATION_PROVIDERS.len() - catalog_forms;
    let multi_word = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .filter(|provider| provider.display_name.contains(' '))
        .count();
    for (case, covered) in [
        ("catalog/<Name> entries", catalog_forms),
        ("service-slug entries", slug_forms),
        ("display names of several words", multi_word),
    ] {
        assert!(
            covered > 1,
            "case {case:?}: the catalog must hold more than one of these for the rule to \
             be asserted over the class, got {covered}"
        );
    }
}

/// RULE 2 — a described provider comes back as a whole `ApplicationProvider` structure
/// beside the flat members, carrying exactly what they carry. The table is every entry of
/// the catalog: entries of both id forms, with and without published access scopes.
#[tokio::test]
async fn a_described_provider_is_reported_as_a_whole_provider_too() {
    let router = router();
    let mut with_scopes = 0usize;

    for provider in state::BUILT_IN_APPLICATION_PROVIDERS {
        let case = provider.id;
        let arn = provider_arn("aws", case);
        let (status, body) = describe(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let envelope = body["ApplicationProvider"].as_object().unwrap_or_else(|| {
            panic!("case {case:?}: expected an ApplicationProvider structure, got {body}")
        });
        for member in PROVIDER_MEMBERS {
            assert_eq!(
                envelope.get(*member).unwrap_or(&Value::Null),
                &body[*member],
                "case {case:?}: member {member} must agree with the flat member, got {:?} vs \
                 {:?}",
                envelope.get(*member),
                body[*member]
            );
        }
        // Nothing else: the structure is the modelled `ApplicationProvider` and no more.
        for member in envelope.keys() {
            assert!(
                PROVIDER_MEMBERS.contains(&member.as_str()),
                "case {case:?}: the structure must carry only the modelled \
                 ApplicationProvider members, got {member:?} in {body}"
            );
        }
        assert_eq!(
            envelope["ApplicationProviderArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the ARN asked for, got {:?}",
            envelope["ApplicationProviderArn"]
        );
        if !provider.scopes.is_empty() {
            with_scopes += 1;
            assert!(
                envelope["ResourceServerConfig"]["Scopes"].is_object(),
                "case {case:?}: a provider that publishes access scopes must report them \
                 in the structure too, got {body}"
            );
        }

        // The structure reports the catalog record's own federation protocol.
        assert_eq!(
            envelope.get("FederationProtocol").and_then(Value::as_str),
            Some(provider.federation_protocol),
            "case {case:?}: expected FederationProtocol {:?}, got {:?}",
            provider.federation_protocol,
            envelope.get("FederationProtocol")
        );
    }

    assert!(
        with_scopes > 0,
        "the catalog must hold at least one provider that publishes access scopes for the \
         ResourceServerConfig row to be covered"
    );
}

/// RULE 2 — the structure comes back under the ARN the caller ASKED about, in every
/// partition the pattern admits. The rows cross the partitions with entries of both id
/// forms: the ones the recorded cases cite and ones they do not.
#[tokio::test]
async fn the_reported_provider_carries_the_arn_asked_about() {
    let router = router();
    let last = state::BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .next_back()
        .expect("the catalog must not be empty")
        .id;

    for id in ["custom", "catalog/Statuspage", "glue", "catalog/Zoom", last] {
        for partition in PARTITIONS {
            let arn = provider_arn(partition, id);
            let case = format!("{id} in {partition}");
            let (status, body) = describe(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            assert_eq!(
                body["ApplicationProvider"]["ApplicationProviderArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the ARN asked about, got {:?}",
                body["ApplicationProvider"]["ApplicationProviderArn"]
            );
            assert_eq!(
                body["ApplicationProviderArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the ARN asked about echoed flat, got {:?}",
                body["ApplicationProviderArn"]
            );
            let record = state::built_in_application_provider(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the catalog must hold {id}"));
            assert_eq!(
                body["ApplicationProvider"]["DisplayData"]["Description"].as_str(),
                Some(format!("{} {DESCRIPTION_SUFFIX}", record.display_name).as_str()),
                "case {case:?}: expected the entry's own Description, got {:?}",
                body["ApplicationProvider"]["DisplayData"]
            );
        }
    }
}

/// RULE 2 — an ARN that names no provider is REFUSED, and a refusal reports no provider:
/// neither the structure nor a flat member. The rows differ along the ways an ARN the
/// pattern admits can miss the catalog.
#[tokio::test]
async fn a_refused_describe_reports_no_provider() {
    let router = router();

    for id in [
        "no-such-provider",
        "custom2",
        "CUSTOM",
        "catalog/NoSuchApplication",
        "catalog/statuspage",
    ] {
        let arn = provider_arn("aws", id);
        let (status, body) = describe(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {id:?}: an ARN naming no provider expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body["ApplicationProvider"].is_null(),
            "case {id:?}: a refusal must report no provider, got {body}"
        );
        assert!(
            body["ApplicationProviderArn"].is_null(),
            "case {id:?}: a refusal must report no provider ARN, got {body}"
        );
    }
}

/// Reporting a provider is a READ: two describes report the identical structure, and the
/// catalog a later caller enumerates is unchanged.
#[tokio::test]
async fn reporting_a_provider_changes_nothing() {
    let router = router();
    let arn = provider_arn("aws", "catalog/Statuspage");

    let before = list_all(&router).await;
    let (status, first) = describe(&router, &arn).await;
    assert_eq!(status, 200, "expected 200, got {status} {first}");
    let (status, second) = describe(&router, &arn).await;
    assert_eq!(status, 200, "expected 200, got {status} {second}");
    assert_eq!(
        first, second,
        "two describes of one provider must report the identical record, got {first} then \
         {second}"
    );
    let after = list_all(&router).await;
    assert_eq!(
        before, after,
        "a read must not change what a later caller sees"
    );
}
