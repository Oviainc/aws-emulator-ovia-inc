// SPDX-License-Identifier: CC0-1.0

//! The Bedrock Marketplace model endpoint resource end-to-end:
//! `CreateMarketplaceModelEndpoint` writes it, `UpdateMarketplaceModelEndpoint`
//! reconfigures it and `GetMarketplaceModelEndpoint` reads it back, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **An `endpointArn` no stored endpoint carries is refused by every operation that
//!    addresses one** — `UpdateMarketplaceModelEndpoint` with 404
//!    `ResourceNotFoundException` and `GetMarketplaceModelEndpoint` with 400
//!    `ValidationException`, the per-operation codes the corpus records — and an update
//!    creates nothing. Rows span a never-created name, a name that exists in another
//!    region, another account, another resource type, a prefix of a live name and a value
//!    that is no ARN at all. The update's sentence is the same for every row and echoes no
//!    part of the request; the read's names the endpoint the ARN names, so it is built from
//!    the request and differs row by row.
//! 2. **A created endpoint round-trips**: every member a read reports comes back out of
//!    the stored record, for configurations differing in instance count, instance type,
//!    execution role, KMS key, VPC, tag count and endpoint-name length (both modelled
//!    bounds).
//! 3. **An update REPLACES `endpointConfig` whole and moves `updatedAt`**, leaving
//!    `endpointArn`, `modelSourceIdentifier`, `status` and `createdAt` where they were, for
//!    a table of replacement configurations.
//! 4. **`endpointStatus` is non-terminal first and settles to `InService`**: `Creating`
//!    straight after a create, `Updating` straight after an update, `InService` once
//!    `ENDPOINT_SETTLE_AFTER` has passed — and the settle is owed by the clock, not by the
//!    read.
//! 5. **A replayed `clientRequestToken` mints and applies nothing** (create and update),
//!    while a fresh one does.
//! 6. **`endpointName` is unique per account per region**, so a second create of a name in
//!    one region conflicts while the same name in another region is minted.
//! 7. **A listing reports the stored endpoints of the signing region in ARN order**,
//!    filtered by `modelSourceEquals`, paged at `maxResults` with a cursor that partitions
//!    the listing, and each summary agreeing with what a read of that endpoint reports.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateMarketplaceModelEndpoint,GetMarketplaceModelEndpoint,UpdateMarketplaceModelEndpoint")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

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

/// `Status` is `[REGISTERED, INCOMPATIBLE_ENDPOINT]`
/// (`smithy_query(query="shape",shape="Status")`); an endpoint Bedrock created from a
/// Marketplace model is registered with Bedrock.
const REGISTERED: &str = "REGISTERED";

/// The three `endpointStatus` spellings this emulator reports, as the member's own doc
/// lists them ("e.g., Creating, InService, Updating, Failed").
const CREATING: &str = "Creating";
const UPDATING: &str = "Updating";
const IN_SERVICE: &str = "InService";

/// The sentence this service words for a marketplace model endpoint ARN it holds no
/// endpoint for, as the handlers state it.
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MARKETPLACE_ENDPOINT_NOT_FOUND_MESSAGE;

/// The sentence `GetMarketplaceModelEndpoint` words for the same ARN, which it refuses as a
/// `ValidationException` rather than a not-found — the per-operation code the corpus
/// records. It NAMES the endpoint the ARN names (the resource id of a `.../endpoint/<name>`
/// ARN, or the whole value when it is not one), so it differs row by row.
fn get_invalid_arn_message(endpoint_arn: &str) -> String {
    let name = endpoint_arn
        .rsplit_once(":endpoint/")
        .map_or(endpoint_arn, |(_, name)| name);
    format!("Could not find endpoint \"{name}\".")
}

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

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

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/marketplace-model/endpoints", Some(body))
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

/// `ListMarketplaceModelEndpoints`, with the query string `query` (empty for none).
async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/marketplace-model/endpoints".to_string()
    } else {
        format!("/marketplace-model/endpoints?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// The summaries a listing reported, in the order it reported them.
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

/// The ARNs a listing reported, in order.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    listed(case, reply)
        .iter()
        .map(|row| {
            row.get("endpointArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The names the listing suite deployed from `MODEL_SOURCES[source]`, sorted — the same
/// `index % MODEL_SOURCES.len()` assignment the suite creates them with.
fn names_of_source(names: &[&str], source: usize) -> Vec<String> {
    let mut picked: Vec<String> = names
        .iter()
        .enumerate()
        .filter(|(index, _)| index % MODEL_SOURCES.len() == source)
        .map(|(_, name)| (*name).to_string())
        .collect();
    picked.sort();
    picked
}

/// The ARN every endpoint named `name` in `region` carries: a Bedrock Marketplace endpoint
/// IS a SageMaker endpoint, so the `endpointArn` is that endpoint's ARN and its resource id
/// is the endpoint name.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:endpoint/{name}")
}

/// The `marketplaceModelEndpoint` member of a reply, which every one of these three
/// operations reports.
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

fn text(case: &str, endpoint: &Value, name: &str) -> String {
    let value = member(case, endpoint, name);
    value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: member {name} must be a string, got {value:?}"))
        .to_string()
}

/// An endpoint's `createdAt` / `updatedAt` in epoch seconds. Both are
/// `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING.
fn instant(case: &str, endpoint: &Value, name: &str) -> f64 {
    let value = member(case, endpoint, name);
    let raw = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case}: member {name} is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string, got {value}"
        )
    });
    date_time_seconds(case, raw)
}

/// Every member of an endpoint except `endpointStatus`, which the clock moves on its own
/// and which each test asserts separately.
fn without_endpoint_status(endpoint: &Value) -> Value {
    let mut copy = endpoint.clone();
    copy.as_object_mut()
        .expect("an endpoint object")
        .remove("endpointStatus");
    copy
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------
// 1. An ARN no endpoint carries: the refusal each operation words for it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_endpoint_arn_is_refused_by_every_operation_that_addresses_one() {
    let emu = emulator();

    // One live endpoint, so the refusals below are about the ARN rather than about an empty
    // store.
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
        let reply = update(
            &emu,
            &arn,
            json!({ "endpointConfig": sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[1]) }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case}: UpdateMarketplaceModelEndpoint of {arn} must answer 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: UpdateMarketplaceModelEndpoint of {arn} must answer \
             ResourceNotFoundException, got {:?}",
            reply.error
        );

        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 400,
            "case {case}: GetMarketplaceModelEndpoint of {arn} must answer 400, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ValidationException"),
            "case {case}: GetMarketplaceModelEndpoint of {arn} must answer \
             ValidationException, got {:?}",
            read.error
        );

        // Each refusal's sentence is the one its operation words: the update's is this
        // service's constant for a resource it does not hold, which echoes no part of the
        // request; the read's names the endpoint the ARN names, so it differs per row.
        let update_message = refusal_message(&reply);
        assert_eq!(
            update_message, NOT_FOUND_MESSAGE,
            "case {case}: the update refusal must state {NOT_FOUND_MESSAGE:?}, got \
             {update_message:?}"
        );
        assert!(
            !update_message.contains(&arn),
            "case {case}: the update refusal must not echo the ARN it was asked for, got \
             {update_message:?}"
        );
        let read_message = refusal_message(&read);
        let expected = get_invalid_arn_message(&arn);
        assert_eq!(
            read_message, expected,
            "case {case}: the read refusal must state {expected:?}, got {read_message:?}"
        );
    }

    // A refused update created nothing and changed nothing: the live endpoint still reads
    // back with the configuration it was created with.
    let read = get(&emu, &expected_arn(REGION, live)).await;
    let endpoint = endpoint_of("live endpoint after the refusals", &read);
    assert_eq!(
        member("live endpoint after the refusals", &endpoint, "endpointConfig"),
        sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        "the refused updates must leave the live endpoint's configuration alone, got \
         {endpoint:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 2. A created endpoint round-trips through GetMarketplaceModelEndpoint.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_endpoint_round_trips() {
    let emu = emulator();

    // `endpointName` is `@length(min: 1, max: 30)`
    // (`smithy_query(query="shape",shape="EndpointName")`), so the rows include both bounds.
    let rows: Vec<(&str, Value)> = vec![
        (
            "only the required members",
            create_body(
                "minimal",
                MODEL_SOURCES[0],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
        ),
        (
            "several instances of another type and another role",
            create_body(
                "four-instances",
                MODEL_SOURCES[1],
                sage_maker_config(4, "ml.p4d.24xlarge", EXECUTION_ROLES[1]),
            ),
        ),
        (
            "a KMS-encrypted endpoint",
            create_body(
                "kms-encrypted",
                MODEL_SOURCES[0],
                json!({
                    "sageMaker": {
                        "initialInstanceCount": 2,
                        "instanceType": "ml.g5.12xlarge",
                        "executionRole": EXECUTION_ROLES[0],
                        "kmsEncryptionKey": KMS_KEY,
                    }
                }),
            ),
        ),
        (
            "a VPC-attached endpoint",
            create_body(
                "vpc-attached",
                MODEL_SOURCES[1],
                json!({
                    "sageMaker": {
                        "initialInstanceCount": 3,
                        "instanceType": "ml.m5.xlarge",
                        "executionRole": EXECUTION_ROLES[1],
                        // `subnetIds` is @length(1..16), `securityGroupIds` @length(1..5).
                        "vpc": {
                            "subnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                            "securityGroupIds": ["sg-0123456789abcdef"],
                        },
                    }
                }),
            ),
        ),
        (
            "tags on the underlying endpoint",
            json!({
                "endpointName": "tagged",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": sage_maker_config(1, "ml.g5.xlarge", EXECUTION_ROLES[0]),
                "acceptEula": true,
                "tags": [
                    { "key": "team", "value": "search" },
                    { "key": "env", "value": "" },
                ],
            }),
        ),
        (
            "acceptEula left at its modelled default",
            json!({
                "endpointName": "eula-default",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": sage_maker_config(1, "ml.g5.xlarge", EXECUTION_ROLES[0]),
            }),
        ),
        (
            "a name at the modelled minimum length",
            create_body(
                "a",
                MODEL_SOURCES[0],
                sage_maker_config(1, "ml.g5.xlarge", EXECUTION_ROLES[0]),
            ),
        ),
        (
            "a name at the modelled maximum length",
            create_body(
                "abcdefghij-klmnopqrst-uvwxyz12",
                MODEL_SOURCES[1],
                sage_maker_config(8, "ml.g6.48xlarge", EXECUTION_ROLES[1]),
            ),
        ),
    ];

    for (case, body) in rows {
        let name = body["endpointName"].as_str().expect("a row names its endpoint");
        let reply = create(&emu, body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: CreateMarketplaceModelEndpoint must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let created = endpoint_of(case, &reply);

        assert_eq!(
            text(case, &created, "endpointArn"),
            expected_arn(REGION, name),
            "case {case}: the minted endpointArn must name this endpoint in the signing \
             region, got {created:?}"
        );
        assert_eq!(
            text(case, &created, "modelSourceIdentifier"),
            body["modelSourceIdentifier"].as_str().unwrap_or_default(),
            "case {case}: modelSourceIdentifier must be the model the create named, got \
             {created:?}"
        );
        assert_eq!(
            member(case, &created, "endpointConfig"),
            body["endpointConfig"],
            "case {case}: endpointConfig must be the configuration the create carried, got \
             {created:?}"
        );
        assert_eq!(
            text(case, &created, "status"),
            REGISTERED,
            "case {case}: an endpoint Bedrock created from a Marketplace model is registered \
             with Bedrock, got {created:?}"
        );
        assert_eq!(
            text(case, &created, "endpointStatus"),
            CREATING,
            "case {case}: the endpoint is still coming up when the create answers, got \
             {created:?}"
        );
        assert!(
            (instant(case, &created, "createdAt") - instant(case, &created, "updatedAt")).abs()
                < f64::EPSILON,
            "case {case}: nothing has updated the endpoint yet, so updatedAt must equal \
             createdAt, got {created:?}"
        );

        // The read reports the STORED record: every member except the status the clock
        // moves comes back exactly as the create answered.
        let read = get(&emu, &text(case, &created, "endpointArn")).await;
        assert_eq!(
            read.status, 200,
            "case {case}: GetMarketplaceModelEndpoint must answer 200, got {} {:?}",
            read.status, read.body
        );
        let fetched = endpoint_of(case, &read);
        assert_eq!(
            without_endpoint_status(&fetched),
            without_endpoint_status(&created),
            "case {case}: the read must report the stored record, got {fetched:?}"
        );
        let status = text(case, &fetched, "endpointStatus");
        assert!(
            status == CREATING || status == IN_SERVICE,
            "case {case}: a created endpoint is coming up or serving, got {status:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. An update replaces the configuration whole and moves updatedAt.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_update_replaces_the_configuration_and_moves_updated_at() {
    let emu = emulator();

    let replacements: Vec<(&str, Value)> = vec![
        (
            "a larger instance count",
            sage_maker_config(6, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        ),
        (
            "another instance type",
            sage_maker_config(1, "ml.p5.48xlarge", EXECUTION_ROLES[0]),
        ),
        (
            "another execution role",
            sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[1]),
        ),
        (
            "a KMS key added",
            json!({
                "sageMaker": {
                    "initialInstanceCount": 2,
                    "instanceType": "ml.g5.2xlarge",
                    "executionRole": EXECUTION_ROLES[0],
                    "kmsEncryptionKey": KMS_KEY,
                }
            }),
        ),
        (
            "a VPC added",
            json!({
                "sageMaker": {
                    "initialInstanceCount": 1,
                    "instanceType": "ml.g5.2xlarge",
                    "executionRole": EXECUTION_ROLES[0],
                    "vpc": {
                        "subnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b", "subnet-8c9d0e1f"],
                        "securityGroupIds": ["sg-0123456789abcdef", "sg-fedcba9876543210"],
                    },
                }
            }),
        ),
        (
            "back to the minimum configuration",
            sage_maker_config(1, "ml.m5.large", EXECUTION_ROLES[1]),
        ),
    ];

    for (index, (case, config)) in replacements.into_iter().enumerate() {
        // One endpoint per row, so each row's update is applied to a freshly created
        // record rather than to the previous row's.
        let name = format!("updated-{index}");
        let created = endpoint_of(
            case,
            &create(
                &emu,
                create_body(
                    &name,
                    MODEL_SOURCES[0],
                    sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
                ),
            )
            .await,
        );
        let arn = text(case, &created, "endpointArn");

        let reply = update(&emu, &arn, json!({ "endpointConfig": config.clone() })).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: UpdateMarketplaceModelEndpoint must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let updated = endpoint_of(case, &reply);

        assert_eq!(
            member(case, &updated, "endpointConfig"),
            config,
            "case {case}: the stored configuration must be the one this update carried, got \
             {updated:?}"
        );
        assert_eq!(
            text(case, &updated, "endpointArn"),
            arn,
            "case {case}: an update never re-mints the ARN, got {updated:?}"
        );
        assert_eq!(
            text(case, &updated, "modelSourceIdentifier"),
            MODEL_SOURCES[0],
            "case {case}: an update carries no model source, so it must leave it alone, got \
             {updated:?}"
        );
        assert_eq!(
            text(case, &updated, "status"),
            REGISTERED,
            "case {case}: an update does not change the Marketplace registration, got \
             {updated:?}"
        );
        assert_eq!(
            text(case, &updated, "endpointStatus"),
            UPDATING,
            "case {case}: the endpoint is rolling the new configuration out when the update \
             answers, got {updated:?}"
        );
        assert!(
            (instant(case, &updated, "createdAt") - instant(case, &created, "createdAt")).abs()
                < f64::EPSILON,
            "case {case}: createdAt never moves, got {updated:?}"
        );
        assert!(
            instant(case, &updated, "updatedAt") >= instant(case, &created, "updatedAt"),
            "case {case}: updatedAt must move to the instant of the update, got {updated:?}"
        );

        let fetched = endpoint_of(case, &get(&emu, &arn).await);
        assert_eq!(
            without_endpoint_status(&fetched),
            without_endpoint_status(&updated),
            "case {case}: the read must report what the update stored, got {fetched:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. The endpoint status the clock owes.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_non_terminal_endpoint_status_settles_to_in_service() {
    // The emulator's span for bringing an endpoint up, which the handlers store as a due
    // instant rather than wait on.
    let settle = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ENDPOINT_SETTLE_AFTER;
    assert!(
        settle >= std::time::Duration::from_millis(250)
            && settle <= std::time::Duration::from_millis(750),
        "the settle span must be observable but short, got {settle:?}"
    );

    let emu = emulator();
    let name = "settling";
    let created = endpoint_of(
        "create",
        &create(
            &emu,
            create_body(
                name,
                MODEL_SOURCES[0],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
        )
        .await,
    );
    let arn = text("create", &created, "endpointArn");

    // A caller acting straight after the create still sees the transient status.
    assert_eq!(
        text("create", &created, "endpointStatus"),
        CREATING,
        "case create: a just-created endpoint is coming up, got {created:?}"
    );
    let immediate = endpoint_of("read while creating", &get(&emu, &arn).await);
    assert_eq!(
        text("read while creating", &immediate, "endpointStatus"),
        CREATING,
        "case read while creating: the endpoint is still coming up, got {immediate:?}"
    );

    tokio::time::sleep(settle + std::time::Duration::from_millis(150)).await;
    let settled = endpoint_of("read once the span has passed", &get(&emu, &arn).await);
    assert_eq!(
        text("read once the span has passed", &settled, "endpointStatus"),
        IN_SERVICE,
        "case read once the span has passed: the endpoint must be serving, got {settled:?}"
    );

    // An update starts the same owed transition over again.
    let updated = endpoint_of(
        "update",
        &update(
            &emu,
            &arn,
            json!({ "endpointConfig": sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[1]) }),
        )
        .await,
    );
    assert_eq!(
        text("update", &updated, "endpointStatus"),
        UPDATING,
        "case update: the endpoint is rolling out the new configuration, got {updated:?}"
    );
    let immediate = endpoint_of("read while updating", &get(&emu, &arn).await);
    assert_eq!(
        text("read while updating", &immediate, "endpointStatus"),
        UPDATING,
        "case read while updating: the endpoint is still rolling out, got {immediate:?}"
    );

    tokio::time::sleep(settle + std::time::Duration::from_millis(150)).await;
    let settled = endpoint_of("read after the update settled", &get(&emu, &arn).await);
    assert_eq!(
        text("read after the update settled", &settled, "endpointStatus"),
        IN_SERVICE,
        "case read after the update settled: the endpoint must be serving again, got \
         {settled:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 5. Idempotency tokens.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_and_applies_nothing() {
    let emu = emulator();

    // A create replayed under the same token answers with what the first one minted, even
    // when the replay names another endpoint — and nothing is minted for that other name.
    let first = endpoint_of(
        "first create",
        &create(
            &emu,
            json!({
                "endpointName": "token-minted",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
                "clientRequestToken": "create-token-1",
            }),
        )
        .await,
    );
    let arn = text("first create", &first, "endpointArn");

    let rows: Vec<(&str, Value)> = vec![
        (
            "the same create replayed",
            json!({
                "endpointName": "token-minted",
                "modelSourceIdentifier": MODEL_SOURCES[0],
                "endpointConfig": sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
                "clientRequestToken": "create-token-1",
            }),
        ),
        (
            "the same token on another endpoint name",
            json!({
                "endpointName": "token-replay-other",
                "modelSourceIdentifier": MODEL_SOURCES[1],
                "endpointConfig": sage_maker_config(4, "ml.p4d.24xlarge", EXECUTION_ROLES[1]),
                "clientRequestToken": "create-token-1",
            }),
        ),
    ];
    for (case, body) in rows {
        let replay = endpoint_of(case, &create(&emu, body).await);
        assert_eq!(
            text(case, &replay, "endpointArn"),
            arn,
            "case {case}: a replayed token mints nothing new, got {replay:?}"
        );
        assert!(
            (instant(case, &replay, "createdAt") - instant("first create", &first, "createdAt"))
                .abs()
                < f64::EPSILON,
            "case {case}: a replayed token answers with the endpoint the first call minted, \
             got {replay:?}"
        );
    }
    let other = get(&emu, &expected_arn(REGION, "token-replay-other")).await;
    assert_eq!(
        other.status, 400,
        "a replayed token must not mint the endpoint the replay named, got {} {:?}",
        other.status, other.body
    );
    assert_eq!(
        other.error.as_deref(),
        Some("ValidationException"),
        "a read of an ARN this account holds no endpoint for is refused, got {:?}",
        other.error
    );

    // A fresh token mints a second endpoint.
    let second = endpoint_of(
        "a fresh create token",
        &create(
            &emu,
            json!({
                "endpointName": "token-minted-2",
                "modelSourceIdentifier": MODEL_SOURCES[1],
                "endpointConfig": sage_maker_config(2, "ml.g5.4xlarge", EXECUTION_ROLES[1]),
                "clientRequestToken": "create-token-2",
            }),
        )
        .await,
    );
    assert_eq!(
        text("a fresh create token", &second, "endpointArn"),
        expected_arn(REGION, "token-minted-2"),
        "case a fresh create token: a distinct token mints a distinct endpoint, got {second:?}"
    );

    // The same for an update: the retry of an applied update changes nothing, a fresh token
    // applies.
    let applied_config = sage_maker_config(3, "ml.g5.12xlarge", EXECUTION_ROLES[1]);
    let applied = endpoint_of(
        "first update",
        &update(
            &emu,
            &arn,
            json!({
                "endpointConfig": applied_config.clone(),
                "clientRequestToken": "update-token-1",
            }),
        )
        .await,
    );
    assert_eq!(
        member("first update", &applied, "endpointConfig"),
        applied_config,
        "case first update: the update must apply, got {applied:?}"
    );

    let retried = endpoint_of(
        "the same update token with another configuration",
        &update(
            &emu,
            &arn,
            json!({
                "endpointConfig": sage_maker_config(9, "ml.m5.24xlarge", EXECUTION_ROLES[0]),
                "clientRequestToken": "update-token-1",
            }),
        )
        .await,
    );
    assert_eq!(
        member(
            "the same update token with another configuration",
            &retried,
            "endpointConfig"
        ),
        applied_config,
        "case the same update token with another configuration: a retry applies nothing a \
         second time, got {retried:?}"
    );

    let fresh_config = sage_maker_config(5, "ml.g6.12xlarge", EXECUTION_ROLES[0]);
    let fresh = endpoint_of(
        "a fresh update token",
        &update(
            &emu,
            &arn,
            json!({
                "endpointConfig": fresh_config.clone(),
                "clientRequestToken": "update-token-2",
            }),
        )
        .await,
    );
    assert_eq!(
        member("a fresh update token", &fresh, "endpointConfig"),
        fresh_config,
        "case a fresh update token: a distinct token applies, got {fresh:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 6. The endpoint name is unique per account per region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_endpoint_name_is_unique_within_the_account_and_region() {
    let emu = emulator();
    let name = "shared-name";
    let body = create_body(
        name,
        MODEL_SOURCES[0],
        sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
    );

    let first = create(&emu, body.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first create of {name} must answer 200, got {} {:?}",
        first.status, first.body
    );

    let again = create(&emu, body.clone()).await;
    assert_eq!(
        again.status, 400,
        "case same region: a second create of {name} must answer 400, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error.as_deref(),
        Some("ConflictException"),
        "case same region: a duplicate endpoint name is a ConflictException, got {:?}",
        again.error
    );

    // The same name in another region names another endpoint: the ARN carries the region.
    let elsewhere = create_in_region(&emu, "eu-central-1", body).await;
    assert_eq!(
        elsewhere.status, 200,
        "case another region: {name} in another region must answer 200, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    let minted = endpoint_of("another region", &elsewhere);
    assert_eq!(
        text("another region", &minted, "endpointArn"),
        expected_arn("eu-central-1", name),
        "case another region: the minted ARN must name that region, got {minted:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 7. The listing: region scope, model-source filter, ARN order, pagination.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_reports_the_stored_endpoints_of_this_region_in_arn_order() {
    let emu = emulator();

    // Names deliberately NOT in creation order, so "ARN order" is not creation order.
    let names = ["delta", "alpha", "charlie", "bravo", "echo"];
    for (index, name) in names.iter().enumerate() {
        let reply = create(
            &emu,
            create_body(
                name,
                MODEL_SOURCES[index % MODEL_SOURCES.len()],
                sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "the listing's endpoint {name} must be created, got {} {:?}",
            reply.status, reply.body
        );
    }
    // One endpoint in another region, which this region's listing must not report.
    let elsewhere = create_in_region(
        &emu,
        "eu-central-1",
        create_body(
            "foreign",
            MODEL_SOURCES[0],
            sage_maker_config(1, "ml.g5.2xlarge", EXECUTION_ROLES[0]),
        ),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "the other region's endpoint must be created, got {} {:?}",
        elsewhere.status, elsewhere.body
    );

    let mut all: Vec<String> = names
        .iter()
        .map(|name| expected_arn(REGION, name))
        .collect();
    all.sort();

    // (case, query, the ARNs the page must report)
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), all.clone()),
        (
            "maxResults at the whole span the model admits",
            "maxResults=1000".to_string(),
            all.clone(),
        ),
        (
            "the first model source",
            format!("modelSourceIdentifier={}", label(MODEL_SOURCES[0])),
            names_of_source(&names, 0)
                .iter()
                .map(|name| expected_arn(REGION, name))
                .collect(),
        ),
        (
            "the second model source",
            format!("modelSourceIdentifier={}", label(MODEL_SOURCES[1])),
            names_of_source(&names, 1)
                .iter()
                .map(|name| expected_arn(REGION, name))
                .collect(),
        ),
        (
            "a model source no endpoint was deployed from",
            format!(
                "modelSourceIdentifier={}",
                label("arn:aws:sagemaker:us-west-2:aws:hub-content/SageMakerPublicHub/Model/absent/1.0.0")
            ),
            Vec::new(),
        ),
        (
            "one row at a time, the first page",
            "maxResults=1".to_string(),
            all[..1].to_vec(),
        ),
    ];

    for (case, query, expected) in rows {
        let reply = list(&emu, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: ListMarketplaceModelEndpoints must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            listed_arns(case, &reply),
            expected,
            "case {case}: the listing must report exactly these endpoints, in ARN order"
        );
    }

    // The other region reports only its own endpoint, which is the same rule from the
    // other side.
    let foreign = list_in_region(&emu, "eu-central-1", "").await;
    assert_eq!(
        listed_arns("the other region", &foreign),
        vec![expected_arn("eu-central-1", "foreign")],
        "case the other region: a listing is scoped to the region the request addresses"
    );

    // Paging at every page size from 1 to one past the number of endpoints: the pages
    // concatenate to the whole listing, with no gap and no repeat, and the last page
    // issues no token.
    for page_size in 1..=names.len() + 1 {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(cursor) = token.as_deref() {
                query.push_str(&format!("&nextToken={}", label(cursor)));
            }
            let reply = list(&emu, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: page {pages} must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = listed_arns(&case, &reply);
            assert!(
                page.len() <= page_size,
                "case {case}: a page carries at most maxResults rows, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {pages} pages for {seen:?}"
            );
            token = reply
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case}: the pages must partition the listing with no gap and no repeat"
        );
    }

    // A summary agrees with what a read of that endpoint reports: same members, same
    // values.
    for arn in &all {
        let read = endpoint_of("summary against read", &get(&emu, arn).await);
        let reply = list(&emu, "").await;
        let summary = listed("summary against read", &reply)
            .into_iter()
            .find(|row| row.get("endpointArn").and_then(Value::as_str) == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("the listing must carry endpoint {arn}"));
        for name in [
            "endpointArn",
            "modelSourceIdentifier",
            "status",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                summary.get(name).cloned().unwrap_or(Value::Null),
                member("summary against read", &read, name),
                "case summary against read: {arn} member {name} must agree with the read, \
                 got {summary:?}"
            );
        }
        // The summary shape carries NO endpointConfig or endpointStatus
        // (`smithy_query(query="shape",shape="MarketplaceModelEndpointSummary")`).
        for absent in ["endpointConfig", "endpointStatus"] {
            assert!(
                summary.get(absent).is_none(),
                "case summary against read: MarketplaceModelEndpointSummary models no \
                 {absent}, got {summary:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_refuses_a_page_size_outside_the_model_and_a_token_it_never_issued() {
    let emu = emulator();
    let created = create(
        &emu,
        create_body(
            "listed",
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

    // `maxResults` is `@range(min: 1, max: 1000)`
    // (`smithy_query(query="operation",operation="ListMarketplaceModelEndpoints")`): each
    // published bound is accepted and one past it is refused.
    let rows: Vec<(&str, &str, u16)> = vec![
        ("the minimum the range admits", "maxResults=1", 200),
        ("the maximum the range admits", "maxResults=1000", 200),
        ("one below the minimum", "maxResults=0", 400),
        ("one past the maximum", "maxResults=1001", 400),
        // The cursor this service issues is an endpoint ARN, so anything else is not a
        // token it handed out.
        ("a token this service never issued", "nextToken=nope", 400),
    ];

    for (case, query, status) in rows {
        let reply = list(&emu, query).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected {status}, got {} {:?}",
            reply.status, reply.body
        );
        if status == 400 {
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case}: a refused listing is a ValidationException, got {:?}",
                reply.error
            );
        }
    }
}
