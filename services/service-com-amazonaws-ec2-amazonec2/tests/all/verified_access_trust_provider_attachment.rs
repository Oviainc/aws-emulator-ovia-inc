// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the attachment between a Verified Access INSTANCE and a Verified
//! Access TRUST PROVIDER: `AttachVerifiedAccessTrustProvider` and
//! `DetachVerifiedAccessTrustProvider`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. an ATTACH adds the trust provider to the instance's own
//!    `VerifiedAccessTrustProviders` set and a DETACH takes it away: both calls answer the
//!    instance as they leave it beside the trust provider, `DescribeVerifiedAccessInstances`
//!    reports the same set afterwards, and the entry is the condensed form of the STORED
//!    trust provider — its id, its description and its kinds;
//! 2. a detached trust provider OUTLIVES the attachment: the detach removes the attachment,
//!    not the resource, so the trust provider still reads back through
//!    `DescribeVerifiedAccessTrustProviders`;
//! 3. an instance names each trust provider AT MOST ONCE and in the order they were
//!    attached, so detaching one of several leaves the rest where they were;
//! 4. an id that names no resource of the account, a detach of a pair that is not attached
//!    and an attach of a pair that already is are all REFUSED, and none of them changes the
//!    set the instance reports;
//! 5. one `ClientToken` names one change: a repeat of the call it was given with changes
//!    nothing further and still answers, while the same token carrying a different request —
//!    another pair, or the opposite direction — is refused;
//! 6. a `DryRun` reports the outcome the request would have had and WRITES NOTHING;
//! 7. an attachment cannot outlive the trust provider it names: deleting an attached trust
//!    provider ends its attachments and leaves the instance's other attachments alone.
//!
//! Each table row is a distinct input along what the rule generalises over — which kind of
//! trust provider (every pair the model's `TRUST_PROVIDER_TYPE_VALUES`,
//! `USER_TRUST_PROVIDER_TYPE_VALUES` and `DEVICE_TRUST_PROVIDER_TYPE_VALUES` catalogs
//! publish), which side of the pair is unresolvable and in which form, how many trust
//! providers an instance holds and which of them is removed, which direction a token was
//! first spent on, which call carries the dry run — so no row is the specification.

use ec2::ops::types::{
    DEVICE_TRUST_PROVIDER_TYPE_VALUES, TRUST_PROVIDER_TYPE_VALUES, USER_TRUST_PROVIDER_TYPE_VALUES,
};
use ec2::state::{TRUST_PROVIDER_TYPE_USER, USER_TRUST_PROVIDER_TYPE_OIDC};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for. Neither resource's modelled shape names a
/// Region, so the Region only selects the envelope.
const REGION: &str = "us-east-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn attach(router: &MultiServiceRouter, params: &Params) -> (u16, String) {
    call(
        router,
        REGION,
        "AttachVerifiedAccessTrustProvider",
        params,
    )
    .await
}

async fn detach(router: &MultiServiceRouter, params: &Params) -> (u16, String) {
    call(
        router,
        REGION,
        "DetachVerifiedAccessTrustProvider",
        params,
    )
    .await
}

/// The parameters that name one pair.
fn pair(instance_id: &str, trust_provider_id: &str) -> Params {
    vec![
        p("VerifiedAccessInstanceId", instance_id),
        p("VerifiedAccessTrustProviderId", trust_provider_id),
    ]
}

/// The parameters one KIND of trust provider needs: its sub-type, plus the options the
/// modelled docs call required for that kind. The catalogs are the model's own
/// (`USER_TRUST_PROVIDER_TYPE_VALUES` / `DEVICE_TRUST_PROVIDER_TYPE_VALUES`), so every
/// published kind can be attached.
fn provider_params(trust_provider_type: &str, sub_type: &str, description: &str) -> Params {
    let mut params = vec![
        p("TrustProviderType", trust_provider_type),
        p("Description", description),
        p(
            "PolicyReferenceName",
            format!("ref{}", sub_type.replace('-', "")),
        ),
    ];
    if trust_provider_type == TRUST_PROVIDER_TYPE_USER {
        params.push(p("UserTrustProviderType", sub_type));
        if sub_type == USER_TRUST_PROVIDER_TYPE_OIDC {
            params.extend([
                p("OidcOptions.Issuer", "https://login.example.test/"),
                p(
                    "OidcOptions.AuthorizationEndpoint",
                    "https://login.example.test/authorize",
                ),
                p("OidcOptions.TokenEndpoint", "https://login.example.test/token"),
                p(
                    "OidcOptions.UserInfoEndpoint",
                    "https://login.example.test/userinfo",
                ),
                p("OidcOptions.ClientId", "client-42"),
                p("OidcOptions.ClientSecret", "secret-42"),
                p("OidcOptions.Scope", "openid"),
            ]);
        }
    } else {
        params.push(p("DeviceTrustProviderType", sub_type));
        params.extend([
            p("DeviceOptions.TenantId", format!("tenant-{sub_type}")),
            p(
                "DeviceOptions.PublicSigningKeyUrl",
                "https://device.example.test/keys",
            ),
        ]);
    }
    params
}

/// Every `(trust provider type, sub-type)` pair the model publishes, as the kind table every
/// rule here is read over.
fn published_kinds() -> Vec<(&'static str, &'static str)> {
    TRUST_PROVIDER_TYPE_VALUES
        .iter()
        .flat_map(|trust_provider_type| {
            let sub_types = if *trust_provider_type == TRUST_PROVIDER_TYPE_USER {
                USER_TRUST_PROVIDER_TYPE_VALUES
            } else {
                DEVICE_TRUST_PROVIDER_TYPE_VALUES
            };
            sub_types
                .iter()
                .map(move |sub_type| (*trust_provider_type, *sub_type))
        })
        .collect()
}

/// Create one Verified Access instance that must succeed, and answer the id it was minted.
async fn seed_instance(router: &MultiServiceRouter, case: &str, description: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateVerifiedAccessInstance",
        &[p("Description", description)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessInstance, got {status} \
         with body {body}"
    );
    let instance_id = element(&body, "verifiedAccessInstanceId").unwrap_or_default();
    assert!(
        instance_id.starts_with("vai-"),
        "case {case:?}: expected a minted instance id, got {instance_id:?}"
    );
    instance_id
}

/// Create one Verified Access trust provider of that kind that must succeed, and answer the
/// id it was minted.
async fn seed_provider(
    router: &MultiServiceRouter,
    case: &str,
    trust_provider_type: &str,
    sub_type: &str,
    description: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateVerifiedAccessTrustProvider",
        &provider_params(trust_provider_type, sub_type, description),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessTrustProvider, got \
         {status} with body {body}"
    );
    let trust_provider_id = element(&body, "verifiedAccessTrustProviderId").unwrap_or_default();
    assert!(
        trust_provider_id.starts_with("vatp-"),
        "case {case:?}: expected a minted trust provider id, got {trust_provider_id:?}"
    );
    trust_provider_id
}

/// The `verifiedAccessTrustProviderSet` an answer carries for the instance it reports, as the
/// ids in it in document order. A set with no entry at all reports no id.
fn attached_in(body: &str) -> Vec<String> {
    element(body, "verifiedAccessTrustProviderSet")
        .map(|set| elements(&set, "verifiedAccessTrustProviderId"))
        .unwrap_or_default()
}

/// The trust provider ids `DescribeVerifiedAccessInstances` reports as attached to
/// `instance_id`, in the order it reports them.
async fn attached_ids(router: &MultiServiceRouter, case: &str, instance_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeVerifiedAccessInstances",
        &[p("VerifiedAccessInstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVerifiedAccessInstances, got {status} \
         with body {body}"
    );
    attached_in(&body)
}

/// RULES 1 and 2 over the published kind catalog: an attach of any kind of trust provider
/// puts the condensed entry on the instance — reported by the attach, by the describe of the
/// instances and by the detach that takes it off again — and the detach leaves the trust
/// provider itself where it was.
#[tokio::test]
async fn every_published_kind_attaches_and_detaches() {
    for (trust_provider_type, sub_type) in published_kinds() {
        let case = format!("{trust_provider_type}/{sub_type}");
        let router = emulator();
        let instance_id = seed_instance(&router, &case, &format!("instance for {case}")).await;
        let description = format!("trust provider for {case}");
        let trust_provider_id =
            seed_provider(&router, &case, trust_provider_type, sub_type, &description).await;

        assert!(
            attached_ids(&router, &case, &instance_id).await.is_empty(),
            "case {case:?}: a created instance has nothing attached yet"
        );

        let (status, body) = attach(&router, &pair(&instance_id, &trust_provider_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the attach, got {status} with body {body}"
        );
        assert_eq!(
            attached_in(&body),
            vec![trust_provider_id.clone()],
            "case {case:?}: the attach reports the instance carrying the trust provider, got \
             {body}"
        );
        // The condensed entry is the STORED trust provider's own facts: its description and
        // its kinds, as `VerifiedAccessTrustProviderCondensed` carries them.
        let set = element(&body, "verifiedAccessTrustProviderSet").unwrap_or_default();
        assert_eq!(
            element(&set, "description"),
            Some(description.clone()),
            "case {case:?}: the entry reports the trust provider's description, got {set}"
        );
        assert_eq!(
            element(&set, "trustProviderType"),
            Some(trust_provider_type.to_string()),
            "case {case:?}: the entry reports the trust provider's type, got {set}"
        );
        let sub_type_member = if trust_provider_type == TRUST_PROVIDER_TYPE_USER {
            "userTrustProviderType"
        } else {
            "deviceTrustProviderType"
        };
        assert_eq!(
            element(&set, sub_type_member),
            Some(sub_type.to_string()),
            "case {case:?}: the entry reports the trust provider's {sub_type_member}, got {set}"
        );
        // The attach answers the trust provider beside the instance.
        assert!(
            elements(&body, "verifiedAccessTrustProviderId").contains(&trust_provider_id),
            "case {case:?}: the attach reports the trust provider it attached, got {body}"
        );

        assert_eq!(
            attached_ids(&router, &case, &instance_id).await,
            vec![trust_provider_id.clone()],
            "case {case:?}: the describe reports the attachment the attach made"
        );

        let (status, body) = detach(&router, &pair(&instance_id, &trust_provider_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the detach, got {status} with body {body}"
        );
        assert!(
            attached_in(&body).is_empty(),
            "case {case:?}: the detach reports the instance with nothing attached, got {body}"
        );
        assert!(
            attached_ids(&router, &case, &instance_id).await.is_empty(),
            "case {case:?}: the describe reports the attachment as gone"
        );

        // RULE 2: the attachment ended, the trust provider did not.
        let (status, body) = call(
            &router,
            REGION,
            "DescribeVerifiedAccessTrustProviders",
            &[p("VerifiedAccessTrustProviderId.1", &trust_provider_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a detached trust provider is still the account's, got {status} \
             with body {body}"
        );
        assert_eq!(
            elements(&body, "verifiedAccessTrustProviderId"),
            vec![trust_provider_id.clone()],
            "case {case:?}: the detached trust provider still reads back, got {body}"
        );
    }
}

/// RULE 3: an instance holds several trust providers in the order they were attached, each
/// once, and a detach removes exactly the one it names.
#[tokio::test]
async fn an_instance_holds_its_trust_providers_once_and_in_order() {
    let case = "three kinds on one instance";
    let router = emulator();
    let instance_id = seed_instance(&router, case, "instance holding several").await;

    let kinds = published_kinds();
    let mut attached: Vec<String> = Vec::new();
    for (index, (trust_provider_type, sub_type)) in kinds.iter().enumerate() {
        let trust_provider_id = seed_provider(
            &router,
            case,
            trust_provider_type,
            sub_type,
            &format!("provider {index}"),
        )
        .await;
        let (status, body) = attach(&router, &pair(&instance_id, &trust_provider_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 attaching {trust_provider_id}, got {status} \
             with body {body}"
        );
        attached.push(trust_provider_id);
        assert_eq!(
            attached_in(&body),
            attached,
            "case {case:?}: the instance reports its trust providers in attachment order, got \
             {body}"
        );
    }
    assert!(
        attached.len() >= 3,
        "case {case:?}: the kind catalog must give at least three distinct trust providers, \
         got {}",
        attached.len()
    );

    // Detach one from the MIDDLE: the ones around it keep their places.
    let removed = attached.remove(1);
    let (status, body) = detach(&router, &pair(&instance_id, &removed)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 detaching {removed}, got {status} with body {body}"
    );
    assert_eq!(
        attached_in(&body),
        attached,
        "case {case:?}: the detach removes exactly the trust provider it names, got {body}"
    );
    assert_eq!(
        attached_ids(&router, case, &instance_id).await,
        attached,
        "case {case:?}: the describe agrees with what the detach answered"
    );
}

/// RULE 4: every request that names a resource the account does not hold, a pair that is not
/// attached, or a pair that already is, is refused with its own code — and the set the
/// instance reports is exactly the one it had before the table ran.
#[tokio::test]
async fn unresolvable_and_impossible_changes_are_refused() {
    let case = "refusals";
    let router = emulator();
    let instance_id = seed_instance(&router, case, "instance under the refusals").await;
    let attached_provider =
        seed_provider(&router, case, "user", USER_TRUST_PROVIDER_TYPE_OIDC, "attached").await;
    let loose_provider = seed_provider(
        &router,
        case,
        "device",
        DEVICE_TRUST_PROVIDER_TYPE_VALUES[0],
        "never attached",
    )
    .await;
    let other_instance = seed_instance(&router, case, "another instance").await;
    let (status, _body) = attach(&router, &pair(&instance_id, &attached_provider)).await;
    assert_eq!(status, 200, "case {case:?}: the seeding attach must succeed");

    // (case, whether the call is the attach, the instance id, the trust provider id, the
    // status, the error code).
    let rows: Vec<(&str, bool, String, String, u16, &str)> = vec![
        (
            "detach: an instance id nothing minted",
            false,
            "vai-00000000000000001".to_string(),
            attached_provider.clone(),
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "detach: an instance id of no EC2 form at all",
            false,
            "not-an-instance".to_string(),
            attached_provider.clone(),
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "detach: a trust provider id nothing minted",
            false,
            instance_id.clone(),
            "vatp-0000000000000beef".to_string(),
            400,
            "InvalidVerifiedAccessTrustProviderId.NotFound",
        ),
        (
            "detach: a pair that is not attached",
            false,
            instance_id.clone(),
            loose_provider.clone(),
            400,
            "InvalidParameterValue",
        ),
        (
            "detach: attached, but to another instance",
            false,
            other_instance.clone(),
            attached_provider.clone(),
            400,
            "InvalidParameterValue",
        ),
        (
            "attach: a pair that is already attached",
            true,
            instance_id.clone(),
            attached_provider.clone(),
            400,
            "Resource.AlreadyAssociated",
        ),
        (
            "attach: an instance id nothing minted",
            true,
            "vai-0000000000000dead".to_string(),
            loose_provider.clone(),
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "attach: a trust provider id nothing minted",
            true,
            instance_id.clone(),
            "vatp-00000000".to_string(),
            400,
            "InvalidVerifiedAccessTrustProviderId.NotFound",
        ),
    ];

    for (row, is_attach, instance, provider, expected_status, expected_code) in rows {
        let params = pair(&instance, &provider);
        let (status, body) = if is_attach {
            attach(&router, &params).await
        } else {
            detach(&router, &params).await
        };
        assert_eq!(
            status, expected_status,
            "case {row:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {row:?}: expected the code {expected_code}, got body {body}"
        );
    }

    assert_eq!(
        attached_ids(&router, case, &instance_id).await,
        vec![attached_provider.clone()],
        "case {case:?}: a refused change leaves the attachment it named alone"
    );
    assert!(
        attached_ids(&router, case, &other_instance).await.is_empty(),
        "case {case:?}: a refused change attaches nothing to the instance it named"
    );
}

/// RULE 5: one `ClientToken` names ONE change. A repeat of the call it was given with still
/// answers and changes nothing further; the same token carrying another pair, or the opposite
/// direction, is the mismatch EC2 refuses.
#[tokio::test]
async fn one_client_token_names_one_change() {
    let case = "tokens";
    let router = emulator();
    let instance_id = seed_instance(&router, case, "instance under the tokens").await;
    let other_instance = seed_instance(&router, case, "another instance").await;
    let trust_provider_id = seed_provider(
        &router,
        case,
        "user",
        USER_TRUST_PROVIDER_TYPE_OIDC,
        "tokened",
    )
    .await;

    // The attach's token: the first call attaches, the repeat answers the same attachment
    // rather than the duplicate-attachment refusal a tokenless repeat gets.
    let attach_token = "attach-token-1";
    let mut params = pair(&instance_id, &trust_provider_id);
    params.push(p("ClientToken", attach_token));
    for round in ["first attach", "repeat of that attach"] {
        let (status, body) = attach(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {round:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            attached_in(&body),
            vec![trust_provider_id.clone()],
            "case {round:?}: the attachment is reported once, got {body}"
        );
    }
    assert_eq!(
        attached_ids(&router, case, &instance_id).await,
        vec![trust_provider_id.clone()],
        "case {case:?}: a replayed attach adds no second entry"
    );

    // The same token on the DETACH of that pair is a different request.
    let mut mismatched = pair(&instance_id, &trust_provider_id);
    mismatched.push(p("ClientToken", attach_token));
    let (status, body) = detach(&router, &mismatched).await;
    assert_eq!(
        status, 400,
        "case \"the attach's token on a detach\": expected HTTP 400, got {status} with body \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatch",
        "case \"the attach's token on a detach\": got body {body}"
    );

    // The detach's own token: the first call detaches, the repeat answers it again instead of
    // refusing a pair this very token unattached.
    let detach_token = "detach-token-1";
    let mut params = pair(&instance_id, &trust_provider_id);
    params.push(p("ClientToken", detach_token));
    for round in ["first detach", "repeat of that detach"] {
        let (status, body) = detach(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {round:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            attached_in(&body).is_empty(),
            "case {round:?}: the instance reports nothing attached, got {body}"
        );
    }

    // The detach's token carrying ANOTHER pair is a different request too.
    let mut other_pair = pair(&other_instance, &trust_provider_id);
    other_pair.push(p("ClientToken", detach_token));
    let (status, body) = detach(&router, &other_pair).await;
    assert_eq!(
        status, 400,
        "case \"the detach's token on another pair\": expected HTTP 400, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatch",
        "case \"the detach's token on another pair\": got body {body}"
    );

    // A token the store has not seen is a change of its own.
    let mut fresh = pair(&instance_id, &trust_provider_id);
    fresh.push(p("ClientToken", "attach-token-2"));
    let (status, body) = attach(&router, &fresh).await;
    assert_eq!(
        status, 200,
        "case \"a token nothing has spent\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        attached_ids(&router, case, &instance_id).await,
        vec![trust_provider_id],
        "case \"a token nothing has spent\": the attach it carried was performed"
    );
}

/// RULE 6: a `DryRun` reports the outcome the request would have had — the refusal when it
/// would have been refused — and changes no attachment either way.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let case = "dry runs";
    let router = emulator();
    let instance_id = seed_instance(&router, case, "instance under the dry runs").await;
    let attached_provider = seed_provider(
        &router,
        case,
        "user",
        USER_TRUST_PROVIDER_TYPE_OIDC,
        "attached",
    )
    .await;
    let loose_provider = seed_provider(
        &router,
        case,
        "device",
        DEVICE_TRUST_PROVIDER_TYPE_VALUES[1],
        "never attached",
    )
    .await;
    let (status, _body) = attach(&router, &pair(&instance_id, &attached_provider)).await;
    assert_eq!(status, 200, "case {case:?}: the seeding attach must succeed");

    // (case, whether the call is the attach, the trust provider id, the status, the code).
    let rows: Vec<(&str, bool, String, u16, &str)> = vec![
        (
            "an attach that would have succeeded",
            true,
            loose_provider.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "a detach that would have succeeded",
            false,
            attached_provider.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "an attach that would have been refused",
            true,
            attached_provider.clone(),
            400,
            "Resource.AlreadyAssociated",
        ),
        (
            "a detach that would have been refused",
            false,
            loose_provider.clone(),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (row, is_attach, provider, expected_status, expected_code) in rows {
        let mut params = pair(&instance_id, &provider);
        params.push(p("DryRun", "true"));
        let (status, body) = if is_attach {
            attach(&router, &params).await
        } else {
            detach(&router, &params).await
        };
        assert_eq!(
            status, expected_status,
            "case {row:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {row:?}: expected the code {expected_code}, got body {body}"
        );
        assert_eq!(
            attached_ids(&router, row, &instance_id).await,
            vec![attached_provider.clone()],
            "case {row:?}: a dry run changes no attachment"
        );
    }
}

/// RULE 7: an attachment names a trust provider, so it cannot outlive one. Deleting an
/// attached trust provider ends its attachment on every instance that had it, and leaves the
/// instance's other attachments where they were.
#[tokio::test]
async fn deleting_a_trust_provider_ends_its_attachments() {
    let case = "delete under an attachment";
    let router = emulator();
    let first_instance = seed_instance(&router, case, "first instance").await;
    let second_instance = seed_instance(&router, case, "second instance").await;
    let deleted_provider = seed_provider(
        &router,
        case,
        "user",
        USER_TRUST_PROVIDER_TYPE_OIDC,
        "about to be deleted",
    )
    .await;
    let kept_provider = seed_provider(
        &router,
        case,
        "device",
        DEVICE_TRUST_PROVIDER_TYPE_VALUES[2],
        "kept",
    )
    .await;

    for (instance, provider) in [
        (&first_instance, &deleted_provider),
        (&first_instance, &kept_provider),
        (&second_instance, &deleted_provider),
    ] {
        let (status, body) = attach(&router, &pair(instance, provider)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 attaching {provider} to {instance}, got {status} \
             with body {body}"
        );
    }

    let (status, body) = call(
        &router,
        REGION,
        "DeleteVerifiedAccessTrustProvider",
        &[p("VerifiedAccessTrustProviderId", &deleted_provider)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
    );

    assert_eq!(
        attached_ids(&router, case, &first_instance).await,
        vec![kept_provider.clone()],
        "case {case:?}: the deleted trust provider's attachment is gone and the other stays"
    );
    assert!(
        attached_ids(&router, case, &second_instance)
            .await
            .is_empty(),
        "case {case:?}: every instance that had the deleted trust provider loses it"
    );
}
