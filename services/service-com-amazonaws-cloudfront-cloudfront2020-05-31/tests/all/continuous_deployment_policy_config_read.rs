// SPDX-License-Identifier: CC0-1.0

//! `GetContinuousDeploymentPolicyConfig` end to end over HTTP.
//!
//! The rule under test: a config read REPORTS the stored record of the resource an id
//! names — the configuration exactly as the last mutation stored it, beside the version
//! token that mutation handed back — it writes nothing a later caller can see, and an id no
//! stored resource holds is that resource's "no such" error. For a continuous deployment
//! policy that means the `ContinuousDeploymentPolicyConfig` payload and the `ETag` header
//! are the same ones `GetContinuousDeploymentPolicy` reports around the policy's identity,
//! a `TrafficConfig` the create left out stays absent, and an unknown id is
//! `NoSuchContinuousDeploymentPolicy` (HTTP 404).
//!
//! The rows span what the rule generalises over: how many staging distribution domain
//! names a policy names (one, two, and the documented maximum), whether it is enabled,
//! which of the two traffic configurations it carries and whether it carries one at all,
//! whether a single-weight configuration names session stickiness, and the weight and the
//! session TTLs at their published bounds. The table, the XML writer and the configuration
//! assertion are the continuous deployment policy suite's, reused rather than copied; the
//! recorded case is one row.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_STAGING_DISTRIBUTION_DNS_NAMES;
use smithy_server_runtime::MultiServiceRouter;

use crate::continuous_deployment_policies::{create_policy, get_policy, policy_table, staged,
    Policy, Traffic};
use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::response_headers_policy_lifecycle::text;

/// `GET /2020-05-31/continuous-deployment-policy/{Id}/config`, from the operation's `http`
/// binding.
async fn get_policy_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/continuous-deployment-policy/{id}/config"),
        None,
        None,
    )
    .await
}

/// The configurations the rules below are asserted over: the shared table, plus a policy
/// that names the documented maximum many staging distribution domain names
/// (`MAX_STAGING_DISTRIBUTION_DNS_NAMES`, "a maximum of 10 domain names") and one carrying
/// a single-header configuration the shared table does not spell.
fn config_read_table() -> Vec<Policy> {
    let mut table = policy_table();
    table.push(Policy {
        case: "the documented maximum many staging domains, disabled",
        dns_names: (0..MAX_STAGING_DISTRIBUTION_DNS_NAMES)
            .map(|index| format!("dcfgread{index}abcdef8.cloudfront.net"))
            .collect(),
        enabled: false,
        traffic: Traffic::SingleWeight {
            weight: 0.05,
            stickiness: None,
        },
    });
    table.push(Policy {
        case: "a single-header configuration with a non-default value",
        dns_names: vec!["dcfgreadheader.cloudfront.net".to_string()],
        enabled: true,
        traffic: Traffic::SingleHeader {
            // The prefix the documentation requires of a single-header name.
            header: "aws-cf-cd-config-read".to_string(),
            value: "canary-7".to_string(),
        },
    });
    table
}

/// The config read answers the stored configuration and the stored version token: the same
/// ones the create handed back and `GetContinuousDeploymentPolicy` reports, over and over,
/// because a read changes nothing a later caller sees.
#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();

    for policy in staged(&router, "cfgread", config_read_table()).await {
        let case = policy.case;
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the create", &created.raw);
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token from the create, got {etag:?}"
        );

        let read = get_policy_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the config read, got {} with {}",
            read.status, read.raw
        );
        // `ContinuousDeploymentPolicyConfig` is this output's `@httpPayload`, so the
        // configuration IS the document's root element.
        policy.assert_matches(read.root(), "the config read");
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the config read to report the version the create handed \
             back ({etag:?}), got {:?}",
            read.header("etag")
        );

        // Parity with `GetContinuousDeploymentPolicy`: both reads answer from the ONE
        // record.
        let whole = get_policy(&router, &id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetContinuousDeploymentPolicy, got {} with {}",
            whole.status, whole.raw
        );
        let nested = whole
            .root()
            .at("ContinuousDeploymentPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: GetContinuousDeploymentPolicy carried no \
                     ContinuousDeploymentPolicyConfig: {}",
                    whole.raw
                )
            });
        policy.assert_matches(nested, "GetContinuousDeploymentPolicy parity");
        assert_eq!(
            whole.header("etag"),
            etag,
            "case {case}: expected one version token across both reads, got {:?} and {etag:?}",
            whole.header("etag")
        );

        // A read writes nothing: the same request answers the same document and version.
        let again = get_policy_config(&router, &id).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected HTTP 200 from the repeated config read, got {} with {}",
            again.status, again.raw
        );
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
        assert_eq!(
            again.header("etag"),
            etag,
            "case {case}: expected the version to survive a read, got {:?}",
            again.header("etag")
        );
    }
}

/// An id no policy holds is `NoSuchContinuousDeploymentPolicy` (HTTP 404), whatever shape
/// the id takes — and the refusals leave the policy that does exist readable at the version
/// it was created with.
#[tokio::test]
async fn the_config_read_refuses_an_id_no_policy_holds() {
    let router = emulator();
    let present = staged(&router, "cfgreadabsent", policy_table())
        .await
        .swap_remove(0);
    let case = present.case;
    let created = create_policy(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let id = text(created.root(), "Id", case, "the create", &created.raw);
    let etag = created.header("etag").to_string();

    // Rows: a UUID never minted, the existing id with one character changed, the existing
    // id in upper case, an id of CloudFront's other shape, and a token that is no id.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("an id of another shape", "E1111111111111".to_string()),
        ("not an id at all", "nope".to_string()),
    ];

    for (absent_case, candidate) in absent {
        let answer = get_policy_config(&router, &candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {absent_case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchContinuousDeploymentPolicy",
            "case {absent_case}: expected NoSuchContinuousDeploymentPolicy for {candidate:?}, \
             got {}",
            answer.raw
        );
    }

    let read = get_policy_config(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected HTTP 200 after the refusals, got {} with {}",
        read.status, read.raw
    );
    present.assert_matches(read.root(), "after the refusals");
    assert_eq!(
        read.header("etag"),
        etag,
        "case {case}: expected the version token to survive the refusals, got {:?}",
        read.header("etag")
    );
}
