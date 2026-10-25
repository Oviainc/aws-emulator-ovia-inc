// SPDX-License-Identifier: CC0-1.0

//! `DeregisterMarketplaceModelEndpoint` end-to-end, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A deregistration removes the endpoint's registration with Amazon Bedrock**: the
//!    call answers 200, and afterwards every operation that addresses that `endpointArn`
//!    refuses it as absent (`GetMarketplaceModelEndpoint` 400 `ValidationException`,
//!    `UpdateMarketplaceModelEndpoint` and a second deregistration 404
//!    `ResourceNotFoundException`), a listing omits it, and every OTHER registered endpoint
//!    is left exactly as it stood. Rows span both `endpointName` bounds, instance count,
//!    instance type, execution role, KMS key, VPC, tag count, model source, an endpoint
//!    reconfigured by an update first and one minted under a `clientRequestToken`.
//! 2. **An `endpointArn` this account holds no registration for is refused 404
//!    `ResourceNotFoundException`** with this service's constant not-found sentence, which
//!    echoes no part of the request, and the refusal removes nothing. Rows span a name no
//!    create ever used, a live name in another region, in another account, under another
//!    resource type, a prefix and a suffix of a live name, and a value that is no ARN at
//!    all.
//! 3. **The registration, not a tombstone, is what a deregistration drops**: the same
//!    `endpointName` is registrable again afterwards and reads back as the SECOND
//!    endpoint's record, and the first endpoint's `clientRequestToken` replays nothing the
//!    deregistration removed.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="DeregisterMarketplaceModelEndpoint,CreateMarketplaceModelEndpoint,GetMarketplaceModelEndpoint,UpdateMarketplaceModelEndpoint")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// Marketplace model sources `modelSourceIdentifier` admits
/// (`smithy_query(query="shape",shape="ModelSourceIdentifier")`: pattern
/// `arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*`).
const MODEL_SOURCES: &[&str] = &[
    "arn:aws:sagemaker:us-west-2:aws:hub-content/SageMakerPublicHub/Model/huggingface-llm-mistral-7b-instruct/1.0.0",
    "arn:aws:sagemaker:us-west-2:aws:hub-content/SageMakerPublicHub/Model/deepseek-llm-r1-distill-qwen-7b/2.1.0",
];

/// Execution roles `SageMakerEndpoint$executionRole` admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const EXECUTION_ROLES: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-marketplace-endpoint",
    "arn:aws:iam::000000000000:role/service-role/SageMakerExecution",
];

/// A KMS key ARN `SageMakerEndpoint$kmsEncryptionKey` admits (its pattern's key-ARN
/// alternative: `arn:aws:kms:<region>:<account>:key/<36 chars>`).
const KMS_KEY: &str = "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555";

/// The sentence this service words for a marketplace model endpoint ARN it holds no
/// registration for, as the handlers that refuse one with `ResourceNotFoundException`
/// state it.
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE;

/// One `endpointConfig` union value: its only member is `sageMaker`
/// (`smithy_query(query="shape",shape="EndpointConfig")`).
fn sage_maker_config(instance_count: u32, instance_type: &str, role: &str) -> Value {
    json!({
        "sageMaker": {
            "initialInstanceCount": instance_count,
            "instanceType": instance_type,
            "executionRole": role,
        }
    })
}

fn create_body(name: &str, model_source: &str, config: Value) -> Value {
    json!({
        "endpointName": name,
        "modelSourceIdentifier": model_source,
        "endpointConfig": config,
        "acceptEula": true,
    })
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/marketplace-model/endpoints", Some(body))
        .await
}

async fn get(emu: &Emulator, arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/marketplace-model/endpoints/{}", label(arn)),
        None,
    )
    .await
}

async fn update(emu: &Emulator, arn: &str, body: Value) -> Reply {
    emu.call(
        "PATCH",
        &format!("/marketplace-model/endpoints/{}", label(arn)),
        Some(body),
    )
    .await
}

/// `DeregisterMarketplaceModelEndpoint`: `DELETE
/// /marketplace-model/endpoints/{endpointArn}/registration`, the ARN being one path label
/// (`smithy_query(query="operation",operation="DeregisterMarketplaceModelEndpoint")`).
async fn deregister(emu: &Emulator, arn: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/marketplace-model/endpoints/{}/registration", label(arn)),
        None,
    )
    .await
}

async fn list(emu: &Emulator) -> Reply {
    emu.call("GET", "/marketplace-model/endpoints", None).await
}

/// The ARNs a listing reported, in order.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    reply
        .get("marketplaceModelEndpoints")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: marketplaceModelEndpoints must be a present array, got {:?}",
                reply.body
            )
        })
        .iter()
        .map(|row| {
            row.get("endpointArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The ARN every endpoint named `name` in `region` carries: a Bedrock Marketplace endpoint
/// IS a SageMaker endpoint, so the `endpointArn` is that endpoint's ARN and its resource id
/// is the endpoint name.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{name}")
}

/// The `marketplaceModelEndpoint` a create, update or read reports.
fn endpoint_of(case: &str, reply: &Reply) -> Value {
    let value = reply.get("marketplaceModelEndpoint");
    assert!(
        value.is_object(),
        "case {case}: the reply must carry a marketplaceModelEndpoint object, got {:?}",
        reply.body
    );
    value
}

fn member(case: &str, endpoint: &Value, name: &str) -> Value {
    endpoint
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the endpoint has no member {name}: {endpoint:?}"))
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `arn` names no registration this service knows: every operation that
/// addresses an endpoint by ARN refuses it, each with the code and sentence it words.
async fn assert_unregistered(emu: &Emulator, case: &str, arn: &str) {
    let read = get(emu, arn).await;
    assert_eq!(
        read.status, 400,
        "case {case}: GetMarketplaceModelEndpoint of a deregistered {arn} must answer 400, \
         got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.error.as_deref(),
        Some("ValidationException"),
        "case {case}: GetMarketplaceModelEndpoint of a deregistered {arn} must answer \
         ValidationException, got {:?}",
        read.error
    );

    let reconfigure = update(
        emu,
        arn,
        json!({ "endpointConfig": sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[1]) }),
    )
    .await;
    assert_eq!(
        reconfigure.status, 404,
        "case {case}: UpdateMarketplaceModelEndpoint of a deregistered {arn} must answer 404, \
         got {} {:?}",
        reconfigure.status, reconfigure.body
    );
    assert_eq!(
        reconfigure.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case}: UpdateMarketplaceModelEndpoint of a deregistered {arn} must answer \
         ResourceNotFoundException, got {:?}",
        reconfigure.error
    );

    let again = deregister(emu, arn).await;
    assert_eq!(
        again.status, 404,
        "case {case}: a second DeregisterMarketplaceModelEndpoint of {arn} must answer 404, \
         got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case}: a second DeregisterMarketplaceModelEndpoint of {arn} must answer \
         ResourceNotFoundException, got {:?}",
        again.error
    );
    let message = refusal_message(&again);
    assert_eq!(
        message, NOT_FOUND_MESSAGE,
        "case {case}: the deregistration refusal must state {NOT_FOUND_MESSAGE:?}, got \
         {message:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 1. A deregistration removes the registration, and only that endpoint's.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn deregistering_removes_the_endpoints_registration_with_bedrock() {
    // `endpointName` is `@length(min: 1, max: 30)`
    // (`smithy_query(query="shape",shape="EndpointName")`), so the rows include both
    // bounds; `initialInstanceCount` is `@range(min: 1)` and the optional members
    // (`kmsEncryptionKey`, `vpc`, `tags`, `clientRequestToken`) are present on some rows
    // and absent on others.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "the shortest name the model admits",
            "a",
            create_body("a", MODEL_SOURCES[0], sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0])),
            false,
        ),
        (
            "the longest name the model admits",
            "endpoint-name-of-thirty-chars-",
            create_body(
                "endpoint-name-of-thirty-chars-",
                MODEL_SOURCES[1],
                sage_maker_config(3, "ml.p4d.24xlarge", EXECUTION_ROLES[1]),
            ),
            false,
        ),
        (
            "a KMS key and a VPC on the configuration",
            "encrypted-in-a-vpc",
            json!({
                "endpointName": "encrypted-in-a-vpc",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": {
                    "sageMaker": {
                        "initialInstanceCount": 2,
                        "instanceType": "ml.g5.12xlarge",
                        "executionRole": EXECUTION_ROLES[0],
                        "kmsEncryptionKey": KMS_KEY,
                        "vpc": {
                            "subnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                            "securityGroupIds": ["sg-00000000000000001"],
                        },
                    }
                },
                "acceptEula": false,
            }),
            false,
        ),
        (
            "tags on the endpoint",
            "tagged-endpoint",
            json!({
                "endpointName": "tagged-endpoint",
                "modelSourceIdentifier": MODEL_SOURCES[1],
                "endpointConfig": sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[1]),
                "acceptEula": true,
                "tags": [
                    { "key": "team", "value": "search" },
                    { "key": "cost-center", "value": "4711" },
                ],
            }),
            false,
        ),
        (
            "an endpoint minted under a clientRequestToken",
            "token-minted-endpoint",
            json!({
                "endpointName": "token-minted-endpoint",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": sage_maker_config(8, "ml.g5.48xlarge", EXECUTION_ROLES[0]),
                "acceptEula": true,
                "clientRequestToken": "deregistered-under-this-token",
            }),
            false,
        ),
        (
            "an endpoint reconfigured by an update first",
            "updated-endpoint",
            create_body(
                "updated-endpoint",
                MODEL_SOURCES[1],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
            true,
        ),
    ];

    for (case, name, body, reconfigure_first) in rows {
        // Each row runs against its own store, so what is left standing afterwards is this
        // row's doing.
        let emu = emulator();

        // A bystander registration, so what the deregistration removes is one endpoint
        // rather than the registry.
        let bystander = "bystander-endpoint";
        let bystander_created = create(
            &emu,
            create_body(
                bystander,
                MODEL_SOURCES[0],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[1]),
            ),
        )
        .await;
        assert_eq!(
            bystander_created.status, 200,
            "case {case}: the bystander endpoint must be created, got {} {:?}",
            bystander_created.status, bystander_created.body
        );
        let bystander_before = endpoint_of(case, &get(&emu, &expected_arn(REGION, bystander)).await);

        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateMarketplaceModelEndpoint must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = expected_arn(REGION, name);
        assert_eq!(
            member(case, &endpoint_of(case, &created), "endpointArn"),
            Value::String(arn.clone()),
            "case {case}: the created endpoint must carry the ARN of its name"
        );

        if reconfigure_first {
            let reconfigured = update(
                &emu,
                &arn,
                json!({
                    "endpointConfig": sage_maker_config(4, "ml.g5.24xlarge", EXECUTION_ROLES[1]),
                }),
            )
            .await;
            assert_eq!(
                reconfigured.status, 200,
                "case {case}: the update before the deregistration must answer 200, got {} {:?}",
                reconfigured.status, reconfigured.body
            );
        }

        // The endpoint is registered right up to the deregistration.
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the endpoint must read back before the deregistration, got {} {:?}",
            read.status, read.body
        );

        let dropped = deregister(&emu, &arn).await;
        assert_eq!(
            dropped.status, 200,
            "case {case}: DeregisterMarketplaceModelEndpoint must answer 200, got {} {:?}",
            dropped.status, dropped.body
        );
        assert_eq!(
            dropped.error, None,
            "case {case}: DeregisterMarketplaceModelEndpoint must not answer an error, got {:?}",
            dropped.error
        );

        assert_unregistered(&emu, case, &arn).await;

        // The listing reports what the registry now holds: the bystander, not the endpoint
        // whose registration is gone.
        let listing = list(&emu).await;
        let arns = listed_arns(case, &listing);
        assert!(
            !arns.contains(&arn),
            "case {case}: the listing must omit the deregistered {arn}, got {arns:?}"
        );
        assert!(
            arns.contains(&expected_arn(REGION, bystander)),
            "case {case}: the listing must still report the bystander endpoint, got {arns:?}"
        );

        // And the bystander reads back exactly as it stood before the deregistration.
        let bystander_after = endpoint_of(case, &get(&emu, &expected_arn(REGION, bystander)).await);
        assert_eq!(
            bystander_after, bystander_before,
            "case {case}: the deregistration must leave every other endpoint alone, got \
             {bystander_after:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. An ARN this account holds no registration for.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn deregistering_an_arn_with_no_registration_is_refused_and_removes_nothing() {
    let emu = emulator();

    // One live endpoint, so the refusals below are about the ARN rather than about an empty
    // registry.
    let live = "live-endpoint";
    let created = create(
        &emu,
        create_body(
            live,
            MODEL_SOURCES[0],
            sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the live endpoint must be created, got {} {:?}",
        created.status, created.body
    );
    let live_arn = expected_arn(REGION, live);
    let live_before = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);

    let rows: Vec<(&str, String)> = vec![
        ("a name no create ever used", expected_arn(REGION, "absent")),
        (
            "the live name in another region",
            expected_arn("eu-central-1", live),
        ),
        (
            "the live name in another account",
            format!("arn:aws:sagemaker:{REGION}:123456789012:endpoint/{live}"),
        ),
        (
            "the live name under another resource type",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model/{live}"),
        ),
        (
            "a prefix of the live name",
            expected_arn(REGION, "live-endpoin"),
        ),
        (
            "the live name with a suffix",
            expected_arn(REGION, "live-endpoint-2"),
        ),
        ("no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in rows {
        let reply = deregister(&emu, &arn).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: DeregisterMarketplaceModelEndpoint of {arn} must answer 404, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: DeregisterMarketplaceModelEndpoint of {arn} must answer \
             ResourceNotFoundException, got {:?}",
            reply.error
        );
        let message = refusal_message(&reply);
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {case}: the refusal must state {NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&arn),
            "case {case}: the refusal must not echo the ARN it was asked for, got {message:?}"
        );
    }

    // Every refused deregistration removed nothing: the live endpoint still reads back as
    // it stood, and the listing still reports it.
    let live_after = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);
    assert_eq!(
        live_after, live_before,
        "the refused deregistrations must leave the live endpoint alone, got {live_after:?}"
    );
    let arns = listed_arns("the live endpoint", &list(&emu).await);
    assert_eq!(
        arns,
        vec![live_arn],
        "the refused deregistrations must leave the registry as it was, got {arns:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 3. What a deregistration drops is the registration itself.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deregistered_endpoint_name_is_registrable_again() {
    let emu = emulator();

    let name = "reusable-endpoint";
    let arn = expected_arn(REGION, name);
    let first = create(
        &emu,
        json!({
            "endpointName": name,
            "modelSourceIdentifier": MODEL_SOURCES[0],
            "endpointConfig": sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            "acceptEula": true,
            "clientRequestToken": "first-registration-token",
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must answer 200, got {} {:?}",
        first.status, first.body
    );

    let dropped = deregister(&emu, &arn).await;
    assert_eq!(
        dropped.status, 200,
        "the deregistration must answer 200, got {} {:?}",
        dropped.status, dropped.body
    );

    // The name is free again, because what was dropped is the registration rather than a
    // tombstone under the ARN: a second create of the same name is minted, not refused as a
    // conflict.
    let second_config = sage_maker_config(5, "ml.p4d.24xlarge", EXECUTION_ROLES[1]);
    let second = create(
        &emu,
        create_body(name, MODEL_SOURCES[1], second_config.clone()),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "re-registering the deregistered name must answer 200, got {} {:?}",
        second.status, second.body
    );

    // And the read reports the SECOND endpoint's record, not the removed one's.
    let read = endpoint_of("the re-registered endpoint", &get(&emu, &arn).await);
    assert_eq!(
        member("the re-registered endpoint", &read, "endpointConfig"),
        second_config,
        "the re-registered endpoint must read back with the second create's configuration, \
         got {read:?}"
    );
    assert_eq!(
        member("the re-registered endpoint", &read, "modelSourceIdentifier"),
        Value::String(MODEL_SOURCES[1].to_string()),
        "the re-registered endpoint must read back with the second create's model source, \
         got {read:?}"
    );

    // The token that minted the endpoint the deregistration removed replays nothing: it
    // cannot answer with a record this registry no longer holds.
    let replayed = create(
        &emu,
        json!({
            "endpointName": "replayed-after-deregistration",
            "modelSourceIdentifier": MODEL_SOURCES[0],
            "endpointConfig": sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[0]),
            "acceptEula": true,
            "clientRequestToken": "first-registration-token",
        }),
    )
    .await;
    assert_eq!(
        replayed.status, 200,
        "a create under the removed endpoint's token must answer 200, got {} {:?}",
        replayed.status, replayed.body
    );
    let minted = endpoint_of("the replayed token", &replayed);
    assert_eq!(
        member("the replayed token", &minted, "endpointArn"),
        Value::String(expected_arn(REGION, "replayed-after-deregistration")),
        "a create under the removed endpoint's token must mint the endpoint it names, got \
         {minted:?}"
    );
}
