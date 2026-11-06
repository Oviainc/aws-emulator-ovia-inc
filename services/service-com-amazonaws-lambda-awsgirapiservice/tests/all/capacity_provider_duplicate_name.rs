// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentence a DUPLICATE capacity-provider name is refused with.
//!
//! The rule under test: a capacity-provider name identifies at most one provider in an
//! account, so a create naming one the account already holds is refused with the modelled
//! `ResourceConflictException` (409) whose message states the conflict and the rule that
//! makes it one — two fixed sentences whose only variable part is the ARN of the provider
//! that already exists, built from the account and the Region the caller signed for and the
//! name that provider was created under. Nothing of the refused REQUEST is echoed: a create
//! addressed by the provider ARN earns the same sentence as one addressed by the bare name,
//! because both name the same record.
//!
//! Every row drives real HTTP requests through the router and reads the provider back
//! afterwards, so a refusal that replaced or changed the provider it refused for fails
//! here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{json_call, router_on, Answer, ACCOUNT, REGION};

/// Where the capacity-provider operations are bound
/// (`smithy_query(query="operation", operation="CreateCapacityProvider")`:
/// `POST /2025-11-30/capacity-providers`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// A well-formed capacity-provider operator role ARN, satisfying the model's `RoleArn`
/// pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it (`smithy_query(query="shape", shape="CapacityProviderArn")`).
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// The `message` of a wire error, under either casing the JSON envelope uses.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// `CreateCapacityProvider` naming `reference`, stating the required configuration alone.
async fn create_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": reference,
            "VpcConfig": { "SubnetIds": ["subnet-10000000"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    )
    .await
}

/// `GetCapacityProvider` on the provider `reference` names.
async fn get_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

#[tokio::test]
async fn a_second_create_of_one_name_is_refused_with_the_existing_providers_arn() {
    // The rows differ along what the rule generalises over: the NAME the provider was
    // created under (a plain word, one carrying the punctuation the name pattern admits,
    // one that is only digits, and a long one), and the form the second create addresses
    // it by — the bare name or the ARN of the provider that already exists, which name the
    // same record and so earn the same sentence.
    let names = [
        "duplicate-create",
        "team_payments.capacity-2",
        "20261104",
        &"c".repeat(64),
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in names {
        let created = create_provider(&router, name).await;
        assert_eq!(
            created.status,
            202,
            "case {name:?}: the first create must be accepted, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let minted = created.json()["CapacityProvider"]["CapacityProviderArn"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            minted.as_deref(),
            Some(provider_arn(name).as_str()),
            "case {name:?}: the created provider must carry the ARN its name mints"
        );

        for (form, reference) in [
            ("the bare name", name.to_string()),
            ("the provider ARN", provider_arn(name)),
        ] {
            let refused = create_provider(&router, &reference).await;
            assert_eq!(
                refused.status,
                409,
                "case {name:?} by {form}: a duplicate name must be refused 409, got {} {}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert!(
                refused.error_code().contains("ResourceConflictException"),
                "case {name:?} by {form}: expected the modelled ResourceConflictException, got \
                 {:?}",
                refused.error_code()
            );
            assert_eq!(
                message(&refused),
                format!(
                    "A capacity provider with ARN {} already exists. Capacity provider names \
                     must be unique within your account.",
                    provider_arn(name)
                ),
                "case {name:?} by {form}: expected the two sentences naming the existing \
                 provider's ARN, got {:?}",
                message(&refused)
            );
        }

        // The refusals replaced nothing: the provider the first create stored is still the
        // one the account holds, under the ARN its name mints.
        let fetched = get_provider(&router, name).await;
        assert_eq!(
            (
                fetched.status,
                fetched.json()["CapacityProvider"]["CapacityProviderArn"]
                    .as_str()
                    .map(str::to_string)
            ),
            (200, Some(provider_arn(name))),
            "case {name:?}: a refused create must leave the stored provider as it was, got {} {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
    }
}

#[tokio::test]
async fn a_name_no_provider_holds_is_created_rather_than_refused() {
    // The admissible side of the same rule: the conflict is about the NAME, so a second
    // create under a different name is accepted, and so is a create under a name whose
    // provider has already been taken away.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for name in ["first-provider", "second-provider"] {
        let created = create_provider(&router, name).await;
        assert_eq!(
            created.status,
            202,
            "case {name:?}: a name no provider holds must be created, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    let refused = create_provider(&router, "first-provider").await;
    assert_eq!(
        refused.status,
        409,
        "the taken name must still conflict, got {} {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        message(&refused),
        format!(
            "A capacity provider with ARN {} already exists. Capacity provider names must be \
             unique within your account.",
            provider_arn("first-provider")
        ),
        "the conflict must name the provider that already exists, got {:?}",
        message(&refused)
    );
}
