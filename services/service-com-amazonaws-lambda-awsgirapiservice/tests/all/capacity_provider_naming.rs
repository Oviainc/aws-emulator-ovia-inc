// SPDX-License-Identifier: CC0-1.0

//! How a `CapacityProviderName` is read, over the wire: the two forms the model's pattern
//! admits, and every shape that is neither.
//!
//! `CreateCapacityProvider` and `GetCapacityProvider` bind the member as a path label and
//! the model's pattern admits EITHER a bare name OR the provider's full ARN
//! (`smithy_query(query="operation", operation="GetCapacityProvider")`:
//! `^(arn:aws[a-zA-Z-]*:lambda:…:capacity-provider:[a-zA-Z0-9-_]+)|[a-zA-Z0-9-_]+$`), so
//! each handler reads the provider out of the value a client sent.
//!
//! The rule this suite pins, asserted through real HTTP requests rather than against the
//! helper directly: both documented forms reach the SAME stored provider; a value that
//! begins like an ARN but names no capacity provider of this service is the modelled 400;
//! a well-formed name or ARN that no provider is stored under is the modelled 404; and no
//! input — however malformed — makes the service fail some other way, because the reading
//! never indexes, slices or does arithmetic on request text.

use std::sync::Arc;

use serde_json::json;

use crate::function_lifecycle::{json_call, router_on, ACCOUNT, REGION, ROLE};

/// `GetCapacityProvider` on whatever a client put in the path label.
async fn get_provider(
    router: &smithy_server_runtime::MultiServiceRouter,
    reference: &str,
) -> (u16, String) {
    let answer = json_call(
        router,
        "GET",
        &format!("/2025-11-30/capacity-providers/{reference}"),
        &serde_json::Value::Null,
    )
    .await;
    (answer.status, answer.error_code())
}

#[tokio::test]
async fn both_documented_forms_of_a_capacity_provider_name_reach_the_same_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "cp-1";
    let arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}");

    let created = json_call(
        &router,
        "POST",
        "/2025-11-30/capacity-providers",
        &json!({
            "CapacityProviderName": name,
            "VpcConfig": { "SubnetIds": ["subnet-0123456789abcdef0"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": ROLE },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case create: expected 202, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    assert_eq!(
        created.json()["CapacityProvider"]["CapacityProviderArn"],
        json!(arn),
        "case create: expected the ARN {arn}, got {}",
        created.json()
    );

    // (case, the value a client sent, the status it must answer)
    //
    // 200: a documented form naming the stored provider. 404: a documented form naming
    // no provider — it is well formed, so the refusal is "not found". 400: a value that
    // begins like an ARN and is not one of this service's capacity-provider ARNs, which
    // is the only way the pattern can be satisfied and the value still name nothing.
    let rows: &[(&str, String, u16)] = &[
        ("the bare name", name.to_string(), 200),
        ("the provider's own ARN", arn.clone(), 200),
        (
            "an ARN of the same provider in another partition",
            format!("arn:aws-us-gov:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}"),
            200,
        ),
        (
            "an ARN with no region and no account",
            format!("arn:aws:lambda:::capacity-provider:{name}"),
            200,
        ),
        ("a bare name no provider is stored under", "cp-absent".to_string(), 404),
        (
            "an ARN no provider is stored under",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:cp-absent"),
            404,
        ),
        (
            "an ARN of another vendor",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:capacity-provider:{name}"),
            400,
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            400,
        ),
        (
            "an ARN of the right type with no name",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:"),
            400,
        ),
        (
            "an ARN of the right type and nothing else",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider"),
            400,
        ),
        ("the ARN prefix alone", "arn:".to_string(), 400),
        ("only separators", "arn:::::".to_string(), 400),
        ("too few fields", "arn:aws:lambda".to_string(), 400),
        (
            "an ARN with an empty vendor",
            format!("arn:aws::{REGION}:{ACCOUNT}:capacity-provider:{name}"),
            400,
        ),
    ];

    for (case, reference, expected) in rows {
        let (status, code) = get_provider(&router, reference).await;
        assert_eq!(
            status, *expected,
            "case {case:?} ({reference:?}): expected HTTP {expected}, got {status} {code}"
        );
        // A 400 may come from either guard, and both are modelled refusals of the same
        // request: the framework's boundary validator answers `ValidationException` for a
        // value the member's own pattern rejects, and the handler answers
        // `InvalidParameterValueException` for one the pattern admits that still names no
        // provider. What the row pins is that such a value is refused rather than read.
        let expected_codes: &[&str] = match expected {
            400 => &["InvalidParameterValueException", "ValidationException"],
            404 => &["ResourceNotFoundException"],
            _ => &[],
        };
        if !expected_codes.is_empty() {
            assert!(
                expected_codes.iter().any(|expected| code.contains(expected)),
                "case {case:?} ({reference:?}): expected one of {expected_codes:?}, got {code}"
            );
        }
    }

    // A create addressed by the stored provider's own ARN names the SAME provider, so it
    // conflicts rather than minting a second one under a different key.
    let duplicate = json_call(
        &router,
        "POST",
        "/2025-11-30/capacity-providers",
        &json!({
            "CapacityProviderName": arn,
            "VpcConfig": { "SubnetIds": ["subnet-0123456789abcdef0"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": ROLE },
        }),
    )
    .await;
    assert_eq!(
        duplicate.status,
        409,
        "case create by ARN: the ARN names the provider that already exists, expected \
         409, got {} {}",
        duplicate.status,
        String::from_utf8_lossy(&duplicate.body)
    );
}
