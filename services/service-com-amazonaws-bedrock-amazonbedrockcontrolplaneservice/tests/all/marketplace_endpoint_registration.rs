// SPDX-License-Identifier: CC0-1.0

//! `RegisterMarketplaceModelEndpoint` end-to-end, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A registration records the Marketplace model deployed on the endpoint it names**:
//!    the call answers 200 with the STORED endpoint — `status` `REGISTERED` and the
//!    `modelSourceIdentifier` the request carried — a later `GetMarketplaceModelEndpoint`
//!    reports exactly that record, and a listing's summary of it agrees. Rows span both
//!    `endpointName` bounds, instance count, instance type, execution role, KMS key, VPC,
//!    tag count, an endpoint minted under a `clientRequestToken`, one reconfigured by an
//!    update first, and a model source that differs from the one the create named as well
//!    as one that does not. Nothing the registration was not asked about moves:
//!    `endpointArn`, `endpointConfig` and `createdAt` stay, every other endpoint is left
//!    as it stood, and `updatedAt` moves only when the registration changed the model
//!    source.
//! 2. **An `endpointIdentifier` this account holds no endpoint under is refused 404
//!    `ResourceNotFoundException`** with this service's constant not-found sentence, which
//!    echoes no part of the request, and the refusal registers nothing. Rows span a name no
//!    create ever used, a live name in another region, in another account, under another
//!    resource type, a prefix and a suffix of a live name, an endpoint whose registration
//!    was deregistered, and a value that is no ARN at all — `Arn` declares no pattern
//!    (`@length(max: 2048)` only), so a malformed identifier names no endpoint rather than
//!    breaking a constraint.
//! 3. **A registration is idempotent in the model source it records**: re-registering the
//!    model source the endpoint already carries leaves the whole record where it stood,
//!    while naming another one replaces it.
//! 4. **The Marketplace model is resolved against the public hub FIRST**: a
//!    `modelSourceIdentifier` whose hub content name the AWS-managed Amazon SageMaker
//!    public hub publishes nothing under is refused 404 `ResourceNotFoundException` naming
//!    that content — whether the endpoint half of the request names a live endpoint or none
//!    — and registers nothing, while every name the hub publishes registers and reads back.
//!    Rows span a name no hub content carries, with and without the version segment the
//!    identifier may carry, a prefix of a published name, a published name under another
//!    case, a published name with a suffix, and another region's hub ARN. The published
//!    catalog is a SUBSET of the hub's contents, so a model source an endpoint of this
//!    account was DEPLOYED from stays registrable on that endpoint while any other unlisted
//!    one is still refused.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="RegisterMarketplaceModelEndpoint,CreateMarketplaceModelEndpoint,GetMarketplaceModelEndpoint")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// Marketplace model sources `modelSourceIdentifier` admits
/// (`smithy_query(query="shape",shape="ModelSourceIdentifier")`: pattern
/// `arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*`). Three of them, so a row
/// can register a model source the create did not name.
const MODEL_SOURCES: &[&str] = &[
    "arn:aws:sagemaker:us-west-2:aws:hub-content/SageMakerPublicHub/Model/huggingface-llm-mistral-7b-instruct/1.0.0",
    "arn:aws:sagemaker:us-west-2:aws:hub-content/SageMakerPublicHub/Model/deepseek-llm-r1-distill-qwen-7b/2.1.0",
    "arn:aws:sagemaker:eu-central-1:aws:hub-content/SageMakerPublicHub/Model/meta-textgeneration-llama-3-8b-instruct/3.0.2",
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

/// Every Bedrock Marketplace model the emulator's public hub publishes, which is the set a
/// `modelSourceIdentifier` can name: the rows below register EVERY one of them, so the
/// catalog is exercised past the entries this suite's own rows cite.
const PUBLISHED_MODELS: &[&str] = bedrock::state::PUBLISHED_MARKETPLACE_MODELS;

/// A `ModelSourceIdentifier` naming the hub content `name` of the public hub, in the form
/// the member's pattern admits (`arn:aws:sagemaker:<region>:aws:hub-content/\
/// SageMakerPublicHub/Model/<name>[/<version>]`).
fn model_source(region: &str, name: &str, version: Option<&str>) -> String {
    let content = match version {
        Some(version) => format!("{name}/{version}"),
        None => name.to_string(),
    };
    format!("arn:aws:sagemaker:{region}:aws:hub-content/SageMakerPublicHub/Model/{content}")
}

/// `Status` is `[REGISTERED, INCOMPATIBLE_ENDPOINT]`
/// (`smithy_query(query="shape",shape="Status")`); an endpoint registered with Amazon
/// Bedrock Marketplace is one Bedrock knows.
const REGISTERED: &str = "REGISTERED";

/// The terminal `endpointStatus` an endpoint reports once SageMaker has it serving, as the
/// member's own doc spells it ("e.g., Creating, InService, Updating, Failed").
const IN_SERVICE: &str = "InService";

/// The sentence this service words for a marketplace model endpoint ARN it holds no
/// endpoint for, as the handlers that refuse one with `ResourceNotFoundException` state it.
const NOT_FOUND_MESSAGE: &str = bedrock::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE;

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

async fn deregister(emu: &Emulator, arn: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/marketplace-model/endpoints/{}/registration", label(arn)),
        None,
    )
    .await
}

/// `RegisterMarketplaceModelEndpoint`: `POST
/// /marketplace-model/endpoints/{endpointIdentifier}/registration`, the identifier being
/// one path label and `modelSourceIdentifier` the one body member
/// (`smithy_query(query="operation",operation="RegisterMarketplaceModelEndpoint")`).
async fn register(emu: &Emulator, identifier: &str, model_source: &str) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/marketplace-model/endpoints/{}/registration",
            label(identifier)
        ),
        Some(json!({ "modelSourceIdentifier": model_source })),
    )
    .await
}

async fn list(emu: &Emulator) -> Reply {
    emu.call("GET", "/marketplace-model/endpoints", None).await
}

/// The summaries a listing reported, in order.
fn listed(case: &str, reply: &Reply) -> Vec<Value> {
    reply
        .get("marketplaceModelEndpoints")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: marketplaceModelEndpoints must be a present array, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The ARN every endpoint named `name` in `region` carries: a Bedrock Marketplace endpoint
/// IS a SageMaker endpoint, so the `endpointArn` is that endpoint's ARN and its resource id
/// is the endpoint name.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{name}")
}

/// The `marketplaceModelEndpoint` a create, register or read reports.
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

/// A `Timestamp` member an endpoint reports, in epoch seconds: every one is modelled
/// `@timestampFormat("date-time")`, so it travels as an RFC 3339 string.
fn instant(case: &str, endpoint: &Value, name: &str) -> f64 {
    let value = member(case, endpoint, name);
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: {name} must be an RFC 3339 string, got {value:?}"));
    date_time_seconds(case, text)
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Shift the stored endpoint `arn`'s own timeline `by` into the past, as though it had been
/// created that much earlier.
///
/// How a rule that needs the endpoint PAST the span SageMaker takes to bring it up
/// ([`bedrock::state::ENDPOINT_SETTLE_AFTER`]) is asserted without sleeping through that
/// span: the transition is owed at a stored instant, so moving that instant into the past
/// is the same as waiting for it, and the next call that touches the endpoint materialises
/// it. The requests themselves still travel through the router.
fn age_endpoint(emu: &Emulator, arn: &str, by: Duration) {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let endpoint = store
        .marketplace_model_endpoints
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the endpoint {arn}"));
    endpoint.created_at -= by;
    endpoint.updated_at -= by;
    if let Some(due) = endpoint.in_service_due_at.as_mut() {
        *due -= by;
    }
}

/// One row of the registration table.
struct Row {
    case: &'static str,
    /// The `endpointName` the create mints the endpoint under.
    name: &'static str,
    /// The create request, so each row's endpoint differs in its configuration too.
    create: Value,
    /// The `modelSourceIdentifier` the registration names.
    register_source: &'static str,
    /// Whether the row reconfigures the endpoint with an update before registering it.
    update_first: bool,
}

fn rows() -> Vec<Row> {
    // `endpointName` is `@length(min: 1, max: 30)`
    // (`smithy_query(query="shape",shape="EndpointName")`), so the rows include both
    // bounds; `initialInstanceCount` is `@range(min: 1)` and the optional members
    // (`kmsEncryptionKey`, `vpc`, `tags`, `clientRequestToken`) are present on some rows and
    // absent on others.
    vec![
        Row {
            case: "the shortest name the model admits, registered to another model source",
            name: "a",
            create: create_body(
                "a",
                MODEL_SOURCES[0],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
            register_source: MODEL_SOURCES[1],
            update_first: false,
        },
        Row {
            case: "the longest name the model admits",
            name: "endpoint-name-of-thirty-chars-",
            create: create_body(
                "endpoint-name-of-thirty-chars-",
                MODEL_SOURCES[1],
                sage_maker_config(3, "ml.p4d.24xlarge", EXECUTION_ROLES[1]),
            ),
            register_source: MODEL_SOURCES[0],
            update_first: false,
        },
        Row {
            case: "a KMS key and a VPC on the configuration, re-registered to the model \
                   source it already carries",
            name: "encrypted-in-a-vpc",
            create: json!({
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
            register_source: MODEL_SOURCES[0],
            update_first: false,
        },
        Row {
            case: "tags on the endpoint, registered to a model source of another region's \
                   public hub",
            name: "tagged-endpoint",
            create: json!({
                "endpointName": "tagged-endpoint",
                "modelSourceIdentifier": MODEL_SOURCES[1],
                "endpointConfig": sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[1]),
                "acceptEula": true,
                "tags": [
                    { "key": "team", "value": "search" },
                    { "key": "cost-center", "value": "4711" },
                ],
            }),
            register_source: MODEL_SOURCES[2],
            update_first: false,
        },
        Row {
            case: "an endpoint minted under a clientRequestToken",
            name: "token-minted-endpoint",
            create: json!({
                "endpointName": "token-minted-endpoint",
                "modelSourceIdentifier": MODEL_SOURCES[2],
                "endpointConfig": sage_maker_config(8, "ml.g5.48xlarge", EXECUTION_ROLES[0]),
                "acceptEula": true,
                "clientRequestToken": "registered-under-this-token",
            }),
            register_source: MODEL_SOURCES[1],
            update_first: false,
        },
        Row {
            case: "an endpoint reconfigured by an update first",
            name: "updated-endpoint",
            create: create_body(
                "updated-endpoint",
                MODEL_SOURCES[1],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
            register_source: MODEL_SOURCES[2],
            update_first: true,
        },
    ]
}

// ---------------------------------------------------------------------------------------
// 1. A registration records the model source deployed on the endpoint it names.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn registering_records_the_model_source_on_the_endpoint_it_names() {
    let settle = bedrock::state::ENDPOINT_SETTLE_AFTER;

    for row in rows() {
        let case = row.case;
        // Each row runs against its own store, so what stands afterwards is this row's
        // doing.
        let emu = emulator();

        // A bystander endpoint, so what a registration touches is one endpoint rather than
        // the registry.
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
        let bystander_arn = expected_arn(REGION, bystander);
        age_endpoint(&emu, &bystander_arn, settle);
        let bystander_before = endpoint_of(case, &get(&emu, &bystander_arn).await);

        let created = create(&emu, row.create).await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateMarketplaceModelEndpoint must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = expected_arn(REGION, row.name);

        if row.update_first {
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
                "case {case}: the update before the registration must answer 200, got {} {:?}",
                reconfigured.status, reconfigured.body
            );
        }

        // Past the span SageMaker takes to bring the endpoint up, so the row's assertions
        // are about the registration rather than about a status the clock moves underneath
        // them.
        age_endpoint(&emu, &arn, settle);
        let before = endpoint_of(case, &get(&emu, &arn).await);

        let registered = register(&emu, &arn, row.register_source).await;
        assert_eq!(
            registered.status, 200,
            "case {case}: RegisterMarketplaceModelEndpoint must answer 200, got {} {:?}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.error, None,
            "case {case}: RegisterMarketplaceModelEndpoint must not answer an error, got {:?}",
            registered.error
        );
        let reported = endpoint_of(case, &registered);

        assert_eq!(
            member(case, &reported, "modelSourceIdentifier"),
            Value::String(row.register_source.to_string()),
            "case {case}: the registration must report the model source it was asked to \
             register, got {reported:?}"
        );
        assert_eq!(
            member(case, &reported, "status"),
            Value::String(REGISTERED.to_string()),
            "case {case}: a registered endpoint's status must be {REGISTERED}, got {reported:?}"
        );
        assert_eq!(
            member(case, &reported, "endpointArn"),
            Value::String(arn.clone()),
            "case {case}: the registration must report the endpoint it was asked for, got \
             {reported:?}"
        );
        assert_eq!(
            member(case, &reported, "endpointConfig"),
            member(case, &before, "endpointConfig"),
            "case {case}: a registration must leave the endpoint's configuration alone, got \
             {reported:?}"
        );
        assert_eq!(
            member(case, &reported, "endpointStatus"),
            Value::String(IN_SERVICE.to_string()),
            "case {case}: an endpoint past its settle span must report {IN_SERVICE}, got \
             {reported:?}"
        );
        assert_eq!(
            instant(case, &reported, "createdAt"),
            instant(case, &before, "createdAt"),
            "case {case}: a registration must leave createdAt where it stood, got {reported:?}"
        );

        // `updatedAt` is the endpoint's own last change: a registration that replaced the
        // model source moved it, and one that recorded the model source already stored
        // changed nothing to move it for.
        let changed_the_source =
            member(case, &before, "modelSourceIdentifier") != Value::String(row.register_source.to_string());
        let updated_before = instant(case, &before, "updatedAt");
        let updated_after = instant(case, &reported, "updatedAt");
        if changed_the_source {
            assert!(
                updated_after > updated_before,
                "case {case}: a registration that replaced the model source must move \
                 updatedAt past {updated_before}, got {updated_after}"
            );
        } else {
            assert_eq!(
                updated_after, updated_before,
                "case {case}: a registration that recorded the model source already stored \
                 must leave updatedAt at {updated_before}, got {updated_after}"
            );
        }

        // The round trip: a read reports exactly the record the registration wrote.
        let read = endpoint_of(case, &get(&emu, &arn).await);
        assert_eq!(
            read, reported,
            "case {case}: GetMarketplaceModelEndpoint must report the registered record, got \
             {read:?}"
        );

        // And so does the listing's summary of that endpoint.
        let summaries = listed(case, &list(&emu).await);
        let summary = summaries
            .iter()
            .find(|listed| listed.get("endpointArn") == Some(&Value::String(arn.clone())))
            .unwrap_or_else(|| panic!("case {case}: the listing must report {arn}: {summaries:?}"));
        assert_eq!(
            summary.get("modelSourceIdentifier"),
            Some(&Value::String(row.register_source.to_string())),
            "case {case}: the listed summary must carry the registered model source, got \
             {summary:?}"
        );
        assert_eq!(
            summary.get("status"),
            Some(&Value::String(REGISTERED.to_string())),
            "case {case}: the listed summary must carry the registered status, got {summary:?}"
        );

        // The bystander is untouched: a registration is about the endpoint its identifier
        // names.
        let bystander_after = endpoint_of(case, &get(&emu, &bystander_arn).await);
        assert_eq!(
            bystander_after, bystander_before,
            "case {case}: the registration must leave every other endpoint alone, got \
             {bystander_after:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. An identifier this account holds no endpoint under.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn registering_an_identifier_with_no_endpoint_is_refused_and_registers_nothing() {
    let emu = emulator();

    // One live endpoint, so the refusals below are about the identifier rather than about an
    // empty registry.
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
    age_endpoint(&emu, &live_arn, bedrock::state::ENDPOINT_SETTLE_AFTER);
    let live_before = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);

    // An endpoint whose registration is gone: what a deregistration dropped is the
    // registration, so Amazon Bedrock knows no endpoint under that ARN any more.
    let deregistered = "deregistered-endpoint";
    let minted = create(
        &emu,
        create_body(
            deregistered,
            MODEL_SOURCES[1],
            sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[1]),
        ),
    )
    .await;
    assert_eq!(
        minted.status, 200,
        "the endpoint to deregister must be created, got {} {:?}",
        minted.status, minted.body
    );
    let dropped = deregister(&emu, &expected_arn(REGION, deregistered)).await;
    assert_eq!(
        dropped.status, 200,
        "the deregistration must answer 200, got {} {:?}",
        dropped.status, dropped.body
    );

    let listing_before = listed("the live registry", &list(&emu).await);

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
        (
            "an endpoint whose registration was deregistered",
            expected_arn(REGION, deregistered),
        ),
        ("no ARN at all", "not-an-arn".to_string()),
        (
            "a value that is no ARN and carries no endpoint segment",
            "endpoint".to_string(),
        ),
    ];

    for (case, identifier) in rows {
        let reply = register(&emu, &identifier, MODEL_SOURCES[0]).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: RegisterMarketplaceModelEndpoint of {identifier} must answer 404, \
             got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: RegisterMarketplaceModelEndpoint of {identifier} must answer \
             ResourceNotFoundException, got {:?}",
            reply.error
        );
        let message = refusal_message(&reply);
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {case}: the refusal must state {NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&identifier),
            "case {case}: the refusal must not echo the identifier it was asked for, got \
             {message:?}"
        );

        // A refused registration mints nothing: the identifier still names no endpoint.
        let read = get(&emu, &identifier).await;
        assert_eq!(
            read.status, 400,
            "case {case}: a refused registration must leave {identifier} naming no endpoint, \
             got {} {:?}",
            read.status, read.body
        );
    }

    // Every refused registration left the registry as it was.
    let live_after = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);
    assert_eq!(
        live_after, live_before,
        "the refused registrations must leave the live endpoint alone, got {live_after:?}"
    );
    let listing_after = listed("the live registry", &list(&emu).await);
    assert_eq!(
        listing_after, listing_before,
        "the refused registrations must leave the registry as it was, got {listing_after:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 3. The model source a registration records, and re-records.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn re_registering_the_stored_model_source_changes_nothing() {
    let emu = emulator();

    let name = "re-registered-endpoint";
    let created = create(
        &emu,
        create_body(
            name,
            MODEL_SOURCES[0],
            sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the endpoint must be created, got {} {:?}",
        created.status, created.body
    );
    let arn = expected_arn(REGION, name);
    age_endpoint(&emu, &arn, bedrock::state::ENDPOINT_SETTLE_AFTER);

    // Each model source in turn: the first registration of a source the endpoint does not
    // carry replaces the stored one, and a second registration of that same source is a
    // no-op the record cannot tell apart from not having been called.
    for source in MODEL_SOURCES {
        let case = format!("registering {source}");
        let first = register(&emu, &arn, source).await;
        assert_eq!(
            first.status, 200,
            "case {case}: the registration must answer 200, got {} {:?}",
            first.status, first.body
        );
        let after_first = endpoint_of(&case, &first);
        assert_eq!(
            member(&case, &after_first, "modelSourceIdentifier"),
            Value::String((*source).to_string()),
            "case {case}: the registration must record the model source it named, got \
             {after_first:?}"
        );

        let again = register(&emu, &arn, source).await;
        assert_eq!(
            again.status, 200,
            "case {case}: the second registration must answer 200, got {} {:?}",
            again.status, again.body
        );
        let after_second = endpoint_of(&case, &again);
        assert_eq!(
            after_second, after_first,
            "case {case}: re-registering the stored model source must change nothing, got \
             {after_second:?}"
        );

        let read = endpoint_of(&case, &get(&emu, &arn).await);
        assert_eq!(
            read, after_second,
            "case {case}: the read must report the registered record, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. The Marketplace model the identifier names, resolved against the public hub.
// ---------------------------------------------------------------------------------------

/// One endpoint this account holds, past the span SageMaker takes to bring it up, and its
/// ARN — what the rows below register against.
async fn live_endpoint(emu: &Emulator, name: &str) -> String {
    let created = create(
        emu,
        create_body(
            name,
            MODEL_SOURCES[0],
            sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the endpoint {name} must be created, got {} {:?}",
        created.status, created.body
    );
    let arn = expected_arn(REGION, name);
    age_endpoint(emu, &arn, bedrock::state::ENDPOINT_SETTLE_AFTER);
    arn
}

#[tokio::test]
async fn a_model_source_the_public_hub_does_not_publish_is_refused_and_registers_nothing() {
    let emu = emulator();
    let live_arn = live_endpoint(&emu, "hub-content-endpoint").await;
    let before = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);

    // Each row is a `modelSourceIdentifier` the member's pattern admits whose hub content
    // name the public hub publishes nothing under, and the content name the refusal must
    // name — the segment after the ARN's `Model/`, which is the name whether or not a
    // version follows it.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a name no hub content carries, with the version segment",
            model_source(REGION, "absent-marketplace-model", Some("1.0.0")),
            "absent-marketplace-model",
        ),
        (
            "a name no hub content carries, with no version segment",
            model_source(REGION, "absent-model-without-version", None),
            "absent-model-without-version",
        ),
        (
            "a prefix of a published name",
            model_source(REGION, "huggingface-llm-mistral-7b", Some("1.0.0")),
            "huggingface-llm-mistral-7b",
        ),
        (
            "a published name under another case",
            model_source(REGION, "HuggingFace-LLM-Mistral-7B-Instruct", Some("1.0.0")),
            "HuggingFace-LLM-Mistral-7B-Instruct",
        ),
        (
            "a published name with a suffix",
            model_source(
                REGION,
                "meta-textgeneration-llama-3-8b-instruct-v2",
                Some("2.0.0"),
            ),
            "meta-textgeneration-llama-3-8b-instruct-v2",
        ),
        (
            "another region's hub ARN",
            model_source("ap-southeast-2", "absent-in-every-region", Some("4.1.0")),
            "absent-in-every-region",
        ),
    ];

    for (case, source, content) in rows {
        // The sentence stated here rather than taken from the service, so the row asserts
        // the wording rather than re-asserting the helper that builds it: the fixed text,
        // and the hub content name the identifier carried.
        let expected = format!("Hub content with name {content} does not exist.");

        // The model source is resolved FIRST, so the refusal is the same whether the
        // endpoint half of the request names a live endpoint, an endpoint this account
        // holds none of, or no ARN at all.
        for (endpoint_case, identifier) in [
            ("a live endpoint", live_arn.clone()),
            (
                "an identifier naming no endpoint",
                expected_arn(REGION, "absent-endpoint"),
            ),
            ("an identifier that is no ARN", "not-an-arn".to_string()),
        ] {
            let reply = register(&emu, &identifier, &source).await;
            assert_eq!(
                reply.status, 404,
                "case {case} with {endpoint_case}: must answer 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case} with {endpoint_case}: must answer ResourceNotFoundException, \
                 got {:?}",
                reply.error
            );
            let message = refusal_message(&reply);
            assert_eq!(
                message, expected,
                "case {case} with {endpoint_case}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // None of those refusals registered anything: the live endpoint still carries the model
    // source it carried before them.
    let after = endpoint_of("the live endpoint", &get(&emu, &live_arn).await);
    assert_eq!(
        after, before,
        "a refused model source must leave the endpoint alone, got {after:?}"
    );
}

#[tokio::test]
async fn a_model_source_an_endpoint_was_deployed_from_stays_registrable() {
    // The published catalog is a SUBSET of the public hub's contents, and
    // `CreateMarketplaceModelEndpoint` deploys from any model source the member's pattern
    // admits — so an endpoint of this account can carry a model source the catalog does not
    // list, and registering exactly that model source on that endpoint must not be refused
    // as a content that does not exist. Any OTHER unlisted model source still is.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a model source outside the published catalog, with a version segment",
            "deployed-outside-the-catalog",
            model_source(REGION, "privately-published-model", Some("1.0.0")),
        ),
        (
            "a model source outside the published catalog, with no version segment",
            "deployed-without-a-version",
            model_source(REGION, "privately-published-model-2", None),
        ),
    ];

    for (case, name, source) in rows {
        let emu = emulator();
        let created = create(
            &emu,
            create_body(
                name,
                &source,
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: the endpoint must be created from {source}, got {} {:?}",
            created.status, created.body
        );
        let arn = expected_arn(REGION, name);
        age_endpoint(&emu, &arn, bedrock::state::ENDPOINT_SETTLE_AFTER);

        let registered = register(&emu, &arn, &source).await;
        assert_eq!(
            registered.status, 200,
            "case {case}: registering the model source the endpoint was deployed from must \
             answer 200, got {} {:?}",
            registered.status, registered.body
        );
        let reported = endpoint_of(case, &registered);
        assert_eq!(
            member(case, &reported, "modelSourceIdentifier"),
            Value::String(source.clone()),
            "case {case}: the registration must report the deployed model source, got \
             {reported:?}"
        );
        assert_eq!(
            member(case, &reported, "status"),
            Value::String(REGISTERED.to_string()),
            "case {case}: the endpoint must read back registered, got {reported:?}"
        );

        // Another model source the catalog does not list is still refused on that catalog,
        // because no endpoint of this account was deployed from it.
        let other = model_source(REGION, "never-deployed-model", Some("1.0.0"));
        let refused = register(&emu, &arn, &other).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case}: an unlisted model source nothing was deployed from must be \
             refused 404 ResourceNotFoundException, got {} {:?}",
            refused.status,
            refused.body
        );
        let message = refusal_message(&refused);
        let expected = "Hub content with name never-deployed-model does not exist.";
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn every_model_the_public_hub_publishes_registers_and_reads_back() {
    let emu = emulator();
    let live_arn = live_endpoint(&emu, "published-models-endpoint").await;
    assert!(
        PUBLISHED_MODELS.len() > 1,
        "case the published catalog: it must hold more than one model, got {PUBLISHED_MODELS:?}"
    );

    for name in PUBLISHED_MODELS {
        // The version segment an identifier may carry is not part of the content name, so
        // each published model registers with one and without one.
        for version in [Some("1.0.0"), None] {
            let source = model_source(REGION, name, version);
            let case = format!("registering {source}");
            let reply = register(&emu, &live_arn, &source).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let reported = endpoint_of(&case, &reply);
            assert_eq!(
                member(&case, &reported, "modelSourceIdentifier"),
                Value::String(source.clone()),
                "case {case}: the registration must record the identifier it was given, got \
                 {reported:?}"
            );
            let read = endpoint_of(&case, &get(&emu, &live_arn).await);
            assert_eq!(
                read, reported,
                "case {case}: the read must report the registered record, got {read:?}"
            );
        }
    }
}
