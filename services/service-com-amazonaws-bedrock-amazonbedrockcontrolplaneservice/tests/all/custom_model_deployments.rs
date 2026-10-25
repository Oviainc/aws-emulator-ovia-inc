// SPDX-License-Identifier: CC0-1.0

//! The custom model deployment resource end-to-end: `CreateCustomModelDeployment` mints it,
//! `GetCustomModelDeployment` reads it back and `ListCustomModelDeployments` reports it,
//! over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created deployment round-trips**: every member a read reports comes out of the
//!    stored record, for rows differing in the base model the `modelArn` embeds, the
//!    deployment name's length (both modelled bounds), whether a `description` and `tags`
//!    are carried, and which `CustomModelDeploymentIdentifier` spelling addresses the read
//!    (its ARN or its name).
//! 2. **`modelArn` must embed a base model the account's published catalog carries**: a
//!    `custom-model` ARN whose base-model segment names no model in the catalog is refused
//!    synchronously with 400 `ValidationException` naming THAT segment, while a segment the
//!    catalog carries — with or without the `:`-separated version qualifiers AWS appends to
//!    a base model id in an ARN — is accepted, as is the `imported` segment, which names a
//!    model customized from no base model at all.
//! 3. **`status` is non-terminal first and settles to `Active`**: `Creating` straight after
//!    a create, `Active` once `CUSTOM_MODEL_DEPLOYMENT_SETTLE_AFTER` has passed — and the
//!    settle is owed by the clock, not by the read, so a read reports it without changing
//!    anything else.
//! 4. **A name is unique per account per region** while it is live, and a replayed
//!    `clientRequestToken` deploys nothing a second time.
//! 5. **An identifier that names no deployment is refused by its SPELLING**: an ARN is a
//!    resource that is not there (404 `ResourceNotFoundException`), while a bare name has
//!    to be resolved to an ARN before anything can be looked up, so one that resolves to
//!    none is not a usable identifier (400 `ValidationException`).
//! 6. **A listing reports the stored deployments of the signing region**, ordered by
//!    `createdAt` in `sortOrder` (`Descending` by default), filtered by every filter the
//!    request carries (`statusEquals`, `modelArnEquals`, `nameContains`, `createdAfter`,
//!    `createdBefore`), paged at `maxResults` with a `nextToken` that partitions the
//!    listing, and each summary agreeing with what a read of that one deployment reports.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateCustomModelDeployment,GetCustomModelDeployment,ListCustomModelDeployments")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model and the published catalog admit for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// Base model ids this account's published catalog carries, written into a
/// `CustomModelArn` the way AWS writes the base model of a custom model
/// (`service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS`,
/// which `ListFoundationModels` reports). The last two rows carry the `:`-separated version
/// qualifiers `CustomModelArn`'s pattern admits after the model id
/// (`([:][a-z0-9-]{1,63}){0,2}`), which name a variant of the same published model.
const CATALOG_BASE_MODELS: &[&str] = &[
    "amazon.titan-text-lite-v1",
    "cohere.command-text-v14",
    "amazon.nova-lite-v1:0",
    "amazon.titan-text-lite-v1:0:4k",
];

/// The base-model segment `CustomModelArn` admits for a model that was IMPORTED rather than
/// customized from a base model: the literal alternative in its pattern,
/// `custom-model/(imported|<base model id>)/<id>`.
const IMPORTED: &str = "imported";

/// `CustomModelDeploymentStatus` is `[Creating, Active, Failed]`
/// (`smithy_query(query="shape",shape="CustomModelDeploymentStatus")`).
const CREATING: &str = "Creating";
const ACTIVE: &str = "Active";

/// How long after a create the `Creating` status is owed its move to `Active`, as the
/// service states it.
const SETTLE_AFTER: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::CUSTOM_MODEL_DEPLOYMENT_SETTLE_AFTER;

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// A `CustomModelArn` of this account and region embedding `base_model`: the form
/// `^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|<base model id>)/[a-z0-9]{12}$`
/// spells (`smithy_query(query="operation",operation="CreateCustomModelDeployment")`).
fn custom_model_arn(base_model: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{base_model}/{model_id}")
}

/// A create body: `modelDeploymentName` and `modelArn` are required, and the optional
/// members are carried only when the row asks for them.
fn create_body(
    name: &str,
    model_arn: &str,
    description: Option<&str>,
    tags: &[(&str, &str)],
) -> Value {
    let mut body = json!({ "modelDeploymentName": name, "modelArn": model_arn });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if !tags.is_empty() {
        body["tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect(),
        );
    }
    body
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call(
        "POST",
        "/model-customization/custom-model-deployments",
        Some(body),
    )
    .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/model-customization/custom-model-deployments/{}",
            label(identifier)
        ),
        None,
    )
    .await
}

/// Create one deployment and hand back its ARN, failing the case by name when the create is
/// refused.
async fn deploy(emu: &Emulator, case: &str, body: Value) -> String {
    let reply = create(emu, body).await;
    assert_eq!(
        reply.status, 202,
        "case {case:?}: CreateCustomModelDeployment must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    let arn = reply.text("customModelDeploymentArn");
    let expected_prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model-deployment/");
    assert!(
        arn.starts_with(&expected_prefix),
        "case {case:?}: the minted ARN must be this account's custom-model-deployment ARN in \
         the signing region ({expected_prefix}<id>), got {arn}"
    );
    arn
}

/// The resource id of a minted `customModelDeploymentArn`, which `CustomModelDeploymentArn`
/// declares as `[a-z0-9]{12}`.
fn deployment_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or_default()
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/model-customization/custom-model-deployments".to_string()
    } else {
        format!("/model-customization/custom-model-deployments?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// The `customModelDeploymentArn` of each summary a listing reported, in the order it
/// reported them.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListCustomModelDeployments must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("modelDeploymentSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the listing must carry modelDeploymentSummaries: {:?}",
                reply.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("customModelDeploymentArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries its ARN: {summary:?}")
                })
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// 1. A created deployment round-trips through GetCustomModelDeployment.
// ---------------------------------------------------------------------------------------

/// One round-trip row: what the create carries, and which identifier spelling reads it
/// back.
struct Row {
    case: &'static str,
    name: String,
    model_arn: String,
    description: Option<String>,
    tags: Vec<(&'static str, &'static str)>,
    read_by_arn: bool,
}

#[tokio::test]
async fn a_created_deployment_round_trips() {
    let emu = emulator();

    // `modelDeploymentName` is `@length(min: 1, max: 63)` with pattern
    // `^([0-9a-zA-Z][_-]?){1,63}$`, so the rows include both bounds; `description` is
    // `@length(min: 1, max: 2048)`; `tags` is `@length(min: 0, max: 200)`. The base model
    // spans the catalog rows above, the qualified spellings and the imported segment.
    let longest_name = "a".repeat(63);
    let longest_description = "d".repeat(2048);
    let rows: Vec<Row> = vec![
        Row {
            case: "the shortest name, no description and no tags",
            name: "a".to_string(),
            model_arn: custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456"),
            description: None,
            tags: Vec::new(),
            read_by_arn: true,
        },
        Row {
            case: "the longest name, read back by its name",
            name: longest_name.clone(),
            model_arn: custom_model_arn(CATALOG_BASE_MODELS[1], "0a1b2c3d4e5f"),
            description: Some("the longest name".to_string()),
            tags: vec![("team", "inference")],
            read_by_arn: false,
        },
        Row {
            case: "a base model id the catalog spells with a version",
            name: "deployment_with_underscore".to_string(),
            model_arn: custom_model_arn(CATALOG_BASE_MODELS[2], "aaaaaaaaaaaa"),
            description: Some("nova".to_string()),
            tags: vec![("env", "test"), ("owner", "sim")],
            read_by_arn: true,
        },
        Row {
            case: "a base model id carrying both version qualifiers",
            name: "deployment-with-hyphen".to_string(),
            model_arn: custom_model_arn(CATALOG_BASE_MODELS[3], "999999999999"),
            description: Some(longest_description.clone()),
            tags: Vec::new(),
            read_by_arn: false,
        },
        Row {
            case: "an imported custom model, which has no base model",
            name: "importedDeployment".to_string(),
            model_arn: custom_model_arn(IMPORTED, "bbbbbb111111"),
            description: None,
            tags: Vec::new(),
            read_by_arn: true,
        },
    ];

    for Row {
        case,
        name,
        model_arn,
        description,
        tags,
        read_by_arn,
    } in rows
    {
        let body = create_body(&name, &model_arn, description.as_deref(), &tags);
        let arn = deploy(&emu, case, body).await;
        assert_eq!(
            deployment_id(&arn).len(),
            12,
            "case {case:?}: the minted ARN's resource id is `[a-z0-9]{{12}}`, got {arn}"
        );

        let identifier = if read_by_arn { arn.clone() } else { name.clone() };
        let read = get(&emu, &identifier).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a read of {identifier} must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("customModelDeploymentArn"),
            arn,
            "case {case:?}: the read must report the ARN the create minted"
        );
        assert_eq!(
            read.text("modelDeploymentName"),
            name,
            "case {case:?}: the read must report the name the create carried"
        );
        assert_eq!(
            read.text("modelArn"),
            model_arn,
            "case {case:?}: the read must report the custom model the create carried"
        );
        assert_eq!(
            read.get("description"),
            description.clone().map_or(Value::Null, Value::from),
            "case {case:?}: the read must report the description the create carried, and none \
             when it carried none"
        );
        assert_eq!(
            read.text("status"),
            CREATING,
            "case {case:?}: a deployment read straight after its create is still being \
             brought up"
        );
        assert_eq!(
            read.get("lastUpdatedAt"),
            Value::Null,
            "case {case:?}: nothing has changed the deployment since the create, so there is \
             no last update to report"
        );
        assert_eq!(
            read.get("failureMessage"),
            Value::Null,
            "case {case:?}: an accepted deployment has no failure to report"
        );
        assert_eq!(
            read.get("updateDetails"),
            Value::Null,
            "case {case:?}: no update is in flight on a freshly created deployment"
        );
        // `createdAt` is modelled `@timestampFormat("date-time")`, so the wire form is an
        // RFC 3339 string; `Reply::instant` panics on anything else.
        let created = read.instant("createdAt");
        assert!(
            created > 0.0,
            "case {case:?}: the creation instant must be the instant the create accepted \
             the deployment, got {created}"
        );

        // A read is a REPORT: a second read of either spelling answers the same record.
        let again = get(&emu, &arn).await;
        assert_eq!(
            again.instant("createdAt"),
            created,
            "case {case:?}: reading a deployment must not change what a later caller sees"
        );
        assert_eq!(
            again.text("modelDeploymentName"),
            name,
            "case {case:?}: both identifier spellings address the same deployment"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. modelArn must embed a base model the published catalog carries.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_base_model_the_catalog_does_not_carry_is_refused() {
    let emu = emulator();

    // Each row's segment satisfies `CustomModelArn`'s own pattern — so the request reaches
    // the handler — and names no model the catalog carries. They differ along the provider
    // half, the model half, a near miss on a catalog id, and a catalog id truncated at a
    // position that is NOT a `:` qualifier boundary.
    let rows: Vec<(&str, &str)> = vec![
        ("a provider the catalog does not publish", "sim2real-noexist.v1"),
        ("a model that provider does not publish", "amazon.titan-not-a-model"),
        ("a near miss on a catalog id", "amazon.titan-text-lite-v1x"),
        (
            "a catalog id cut at a non-qualifier boundary",
            "amazon.titan-text-lite-v",
        ),
        ("a qualifier on a model that does not exist", "cohere.command-nope-v1:0"),
    ];

    for (case, segment) in rows {
        let model_arn = custom_model_arn(segment, "abc123def456");
        let reply = create(
            &emu,
            create_body("rejected-deployment", &model_arn, None, &[]),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a base model the catalog does not carry must be refused 400, got \
             {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal is a ValidationException, got {:?}",
            reply.error
        );
        assert_eq!(
            reply.text("message"),
            format!(
                "The foundation model '{segment}' for custom model is not a valid foundation \
                 model"
            ),
            "case {case:?}: the refusal must name the base model the submitted ARN embeds"
        );
    }

    // And the same name is still free, because none of those creates stored anything.
    let accepted = create(
        &emu,
        create_body(
            "rejected-deployment",
            &custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456"),
            None,
            &[],
        ),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "a refused create must persist nothing, so the name it asked for is still free: got \
         {} {:?}",
        accepted.status, accepted.body
    );
}

// ---------------------------------------------------------------------------------------
// 3. The status is non-terminal first and settles to Active on the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_status_settles_to_active_on_the_clock() {
    let emu = emulator();

    let name = "settling-deployment";
    let model_arn = custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456");
    let arn = deploy(
        &emu,
        "the settling deployment",
        create_body(name, &model_arn, Some("settles"), &[]),
    )
    .await;

    let fresh = get(&emu, &arn).await;
    assert_eq!(
        fresh.text("status"),
        CREATING,
        "a caller reading straight after the create still observes the deployment coming up"
    );
    let created = fresh.instant("createdAt");

    // The move is owed by the CLOCK: once the span has passed the next call that touches
    // the record reports it active, and reports the due instant as its last update.
    tokio::time::sleep(SETTLE_AFTER + std::time::Duration::from_millis(150)).await;
    let settled = get(&emu, name).await;
    assert_eq!(
        settled.text("status"),
        ACTIVE,
        "the read must report the active status once the span has passed, got {:?}",
        settled.body
    );
    let updated = settled.instant("lastUpdatedAt");
    assert!(
        updated >= created + SETTLE_AFTER.as_secs_f64() - 0.001,
        "the settle must be stamped at the instant it was owed ({}), got {updated} against a \
         creation at {created}",
        SETTLE_AFTER.as_secs_f64()
    );
    assert_eq!(
        settled.instant("createdAt"),
        created,
        "a settle must leave the creation instant alone"
    );
    assert_eq!(
        settled.text("modelArn"),
        model_arn,
        "a settle must leave the custom model the deployment serves alone"
    );
}

// ---------------------------------------------------------------------------------------
// 4. A live name is unique per region, and a replayed token deploys once.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_live_name_is_unique_per_region_and_a_replayed_token_deploys_once() {
    let emu = emulator();

    let name = "unique-deployment";
    let first = custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456");
    let arn = deploy(
        &emu,
        "the first deployment of the name",
        create_body(name, &first, None, &[]),
    )
    .await;

    // The name is the other spelling of a `CustomModelDeploymentIdentifier`, so a second
    // live deployment of it could not be addressed: the create is refused.
    let duplicate = create(
        &emu,
        create_body(
            name,
            &custom_model_arn(CATALOG_BASE_MODELS[1], "0a1b2c3d4e5f"),
            None,
            &[],
        ),
    )
    .await;
    assert_eq!(
        duplicate.status, 400,
        "a second create of a live name must be refused, got {} {:?}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error.as_deref(),
        Some("ValidationException"),
        "the refusal of a duplicate name is a ValidationException, got {:?}",
        duplicate.error
    );
    assert_eq!(
        get(&emu, name).await.text("modelArn"),
        first,
        "the refused create must not have replaced the deployment the name already names"
    );

    // The same name in ANOTHER region names another deployment, because the name is unique
    // per account and region.
    let other_region = "eu-west-1";
    let elsewhere = emu
        .call_in_region(
            other_region,
            "POST",
            "/model-customization/custom-model-deployments",
            Some(create_body(
                name,
                &format!(
                    "arn:aws:bedrock:{other_region}:{ACCOUNT}:custom-model/{}/abc123def456",
                    CATALOG_BASE_MODELS[0]
                ),
                None,
                &[],
            )),
        )
        .await;
    assert_eq!(
        elsewhere.status, 202,
        "the same name in another region is another deployment, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        elsewhere.text("customModelDeploymentArn"),
        arn,
        "two regions' deployments of one name are two resources"
    );

    // `@idempotencyToken`: a replayed token answers with the ARN the first request minted
    // and deploys nothing a second time.
    let token = "replayedtoken1";
    let mut body = create_body(
        "token-deployment",
        &custom_model_arn(CATALOG_BASE_MODELS[1], "cccccc222222"),
        None,
        &[],
    );
    body["clientRequestToken"] = json!(token);
    let minted = deploy(&emu, "the first create with a token", body.clone()).await;
    let replay = create(&emu, body).await;
    assert_eq!(
        replay.status, 202,
        "a replayed token is not an error, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("customModelDeploymentArn"),
        minted,
        "a replayed token must answer with the deployment the first request minted"
    );
    // And a DIFFERENT name under the same token still answers the first deployment, because
    // the token — not the body — decides what a replay returns.
    let mut other = create_body(
        "token-deployment-two",
        &custom_model_arn(CATALOG_BASE_MODELS[1], "dddddd333333"),
        None,
        &[],
    );
    other["clientRequestToken"] = json!(token);
    assert_eq!(
        create(&emu, other).await.text("customModelDeploymentArn"),
        minted,
        "a replayed token deploys nothing a second time"
    );
}

// ---------------------------------------------------------------------------------------
// 5. An identifier that names no deployment is refused by its spelling.
// ---------------------------------------------------------------------------------------

/// The sentence this service words for an identifier that is no deployment's ARN and
/// resolves to none, as the service states it.
const IDENTIFIER_INVALID: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::CUSTOM_MODEL_DEPLOYMENT_IDENTIFIER_INVALID;

#[tokio::test]
async fn an_identifier_that_names_nothing_is_refused_by_its_spelling() {
    let emu = emulator();

    let live = "live-deployment";
    let arn = deploy(
        &emu,
        "the live deployment",
        create_body(
            live,
            &custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456"),
            None,
            &[],
        ),
    )
    .await;

    // (case, identifier, status, error code). An ARN names a resource that is not there; a
    // bare NAME has to be resolved to an ARN first, so one that resolves to none is not a
    // usable identifier. The rows span both spellings, several name forms the member's
    // pattern admits, and an ARN of this account whose resource id no deployment carries.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a name no deployment carries",
            "no-such-deployment".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a one-character name no deployment carries",
            "z".to_string(),
            400,
            "ValidationException",
        ),
        (
            "the longest name no deployment carries",
            "z".repeat(63),
            400,
            "ValidationException",
        ),
        (
            "an ARN no deployment carries",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model-deployment/zzzzzzzzzzzz"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, identifier, status, code) in rows {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: {identifier} names no deployment, so the read is {status}, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some(code),
            "case {case:?}: the refusal is a {code}, got {:?}",
            reply.error
        );
        if code == "ValidationException" {
            assert_eq!(
                reply.text("message"),
                IDENTIFIER_INVALID,
                "case {case:?}: an unresolvable name is refused with the service's own \
                 sentence"
            );
        }
    }

    // The live deployment is read in the region it was created in, and nowhere else: a
    // deployment belongs to the account-region it was created in, so NEITHER spelling
    // addresses it from another region — the name resolves to nothing there (so it is not a
    // usable identifier), and the ARN names a resource that region does not hold.
    let elsewhere_rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "its name, from another region",
            live.to_string(),
            400,
            "ValidationException",
        ),
        (
            "its ARN, from another region",
            arn.clone(),
            404,
            "ResourceNotFoundException",
        ),
    ];
    for (case, identifier, status, code) in elsewhere_rows {
        let elsewhere = emu
            .call_in_region(
                "eu-west-1",
                "GET",
                &format!(
                    "/model-customization/custom-model-deployments/{}",
                    label(&identifier)
                ),
                None,
            )
            .await;
        assert_eq!(
            elsewhere.status, status,
            "case {case:?}: a deployment is scoped to the region it was created in, got {} \
             {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.error.as_deref(),
            Some(code),
            "case {case:?}: the refusal is a {code}, got {:?}",
            elsewhere.error
        );
    }
    assert_eq!(
        get(&emu, &arn).await.status,
        200,
        "the live deployment is still readable by its ARN in its own region"
    );
}

// ---------------------------------------------------------------------------------------
// 6. The listing: order, filters, pages and agreement with a read.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_orders_filters_and_pages_the_stored_deployments() {
    let emu = emulator();

    assert!(
        listed_arns("an empty account", &list(&emu, "").await).is_empty(),
        "an account that has deployed nothing lists nothing"
    );

    // Two groups, created in two bursts with a clear gap between them, so a creation-time
    // bound between the groups is unambiguous.
    let first_group = ["alpha-one", "alpha-two", "alpha-three"];
    let second_group = ["beta-one", "beta-two"];
    let mut arns: Vec<String> = Vec::new();
    for (index, name) in first_group.iter().enumerate() {
        arns.push(
            deploy(
                &emu,
                name,
                create_body(
                    name,
                    &custom_model_arn(CATALOG_BASE_MODELS[0], &format!("aaaaaaaaaa{index:02}")),
                    None,
                    &[],
                ),
            )
            .await,
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let boundary = std::time::SystemTime::now();
    for (index, name) in second_group.iter().enumerate() {
        arns.push(
            deploy(
                &emu,
                name,
                create_body(
                    name,
                    &custom_model_arn(CATALOG_BASE_MODELS[1], &format!("bbbbbbbbbb{index:02}")),
                    None,
                    &[],
                ),
            )
            .await,
        );
    }

    // Order: `sortBy` admits only `CreationTime`, and `sortOrder` defaults to `Descending`,
    // so an unfiltered listing is newest first and `Ascending` reverses it.
    let newest_first: Vec<String> = arns.iter().rev().cloned().collect();
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("the default order", "", newest_first.clone()),
        ("sortOrder=Descending", "sortOrder=Descending", newest_first),
        ("sortOrder=Ascending", "sortOrder=Ascending", arns.clone()),
        (
            "sortBy=CreationTime, the only value the enum carries",
            "sortBy=CreationTime&sortOrder=Ascending",
            arns.clone(),
        ),
    ];
    for (case, query, expected) in rows {
        let actual = listed_arns(case, &list(&emu, query).await);
        assert_eq!(
            actual, expected,
            "case {case:?}: the listing must report every stored deployment in creation \
             order, got {actual:?}"
        );
    }

    // Filters: each one narrows the listing on its own member, over the whole set.
    let boundary_text = ::aws_smithy_types::DateTime::from(boundary)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the boundary instant renders as an RFC 3339 date-time");
    let first_three: Vec<String> = arns[..3].to_vec();
    let last_two: Vec<String> = arns[3..].to_vec();
    let filters: Vec<(&str, String, Vec<String>)> = vec![
        (
            "nameContains on the first group's prefix",
            "nameContains=alpha&sortOrder=Ascending".to_string(),
            first_three.clone(),
        ),
        (
            "nameContains on one deployment's whole name",
            "nameContains=beta-two&sortOrder=Ascending".to_string(),
            vec![arns[4].clone()],
        ),
        (
            "nameContains matching nothing",
            "nameContains=gamma&sortOrder=Ascending".to_string(),
            Vec::new(),
        ),
        (
            "statusEquals Failed, which no accepted deployment is",
            "statusEquals=Failed&sortOrder=Ascending".to_string(),
            Vec::new(),
        ),
        (
            "modelArnEquals the first group's custom model",
            format!(
                "modelArnEquals={}&sortOrder=Ascending",
                label(&custom_model_arn(CATALOG_BASE_MODELS[0], "aaaaaaaaaa00"))
            ),
            vec![arns[0].clone()],
        ),
        (
            "createdAfter the boundary",
            format!(
                "createdAfter={}&sortOrder=Ascending",
                label(&boundary_text)
            ),
            last_two.clone(),
        ),
        (
            "createdBefore the boundary",
            format!(
                "createdBefore={}&sortOrder=Ascending",
                label(&boundary_text)
            ),
            first_three.clone(),
        ),
    ];
    for (case, query, expected) in filters {
        let actual = listed_arns(case, &list(&emu, &query).await);
        assert_eq!(
            actual, expected,
            "case {case:?}: the filter must narrow the listing to the deployments it \
             names, got {actual:?}"
        );
    }

    // `statusEquals` is the one filter whose member moves on the clock, so it is asserted
    // as a PARTITION rather than against a fixed expectation: the two statuses an accepted
    // deployment can hold split the whole listing between them, with every row on the side
    // it asks for and none on both.
    let creating = listed_arns(
        "statusEquals Creating",
        &list(&emu, "statusEquals=Creating&sortOrder=Ascending").await,
    );
    let active = listed_arns(
        "statusEquals Active",
        &list(&emu, "statusEquals=Active&sortOrder=Ascending").await,
    );
    let mut partitioned: Vec<String> = creating.iter().chain(active.iter()).cloned().collect();
    partitioned.sort();
    let mut all_arns = arns.clone();
    all_arns.sort();
    assert_eq!(
        partitioned, all_arns,
        "the two statuses an accepted deployment can hold must partition the listing, got \
         Creating {creating:?} and Active {active:?}"
    );
    for (status, filtered) in [(CREATING, &creating), (ACTIVE, &active)] {
        for listed in filtered {
            let read = get(&emu, listed).await;
            let reported = read.text("status");
            assert!(
                reported == status || reported == ACTIVE,
                "case statusEquals={status}: {listed} was reported under that filter, so a \
                 read must report that status (or the one the clock has since owed it), got \
                 {reported}"
            );
        }
    }

    // Pages: `maxResults` bounds one page and the token resumes the next, so the pages
    // partition the listing — every row once, in order, with no token on the last page.
    for page_size in [1_usize, 2, 5] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!(
                    "maxResults={page_size}&sortOrder=Ascending&nextToken={}",
                    label(token)
                ),
                None => format!("maxResults={page_size}&sortOrder=Ascending"),
            };
            let page = list(&emu, &query).await;
            let page_arns = listed_arns(&case, &page);
            assert!(
                page_arns.len() <= page_size,
                "case {case}: a page carries at most maxResults rows, got {}",
                page_arns.len()
            );
            seen.extend(page_arns);
            token = page.get("nextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, arns,
            "case {case}: the pages must partition the listing in order"
        );
    }

    // A token this listing did not issue is refused rather than read as some other page.
    // The cursor is the one `state::listing_cursor` every creation-time listing of this
    // service pages with, so the rows are values that cursor cannot render.
    let foreign: Vec<(&str, String)> = vec![
        ("a token that is not a cursor at all", "not-a-cursor".to_string()),
        (
            "a cursor whose instant half is the wrong width",
            format!("{}:{}", "0".repeat(10), arns[0]),
        ),
        (
            "a cursor whose instant half is not decimal",
            format!("{}:{}", "a".repeat(20), arns[0]),
        ),
        (
            "a cursor with no ARN half at all",
            format!("{}:", "0".repeat(20)),
        ),
    ];
    for (case, token) in foreign {
        let bogus = list(&emu, &format!("nextToken={}", label(&token))).await;
        assert_eq!(
            bogus.status, 400,
            "case {case:?}: a token this listing never issued is refused, got {} {:?}",
            bogus.status, bogus.body
        );
        assert_eq!(
            bogus.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal of a foreign token is a ValidationException, got {:?}",
            bogus.error
        );
    }

    // A summary agrees with what a read of that one deployment reports, member for member.
    let summaries = list(&emu, "sortOrder=Ascending").await;
    let summaries = summaries
        .get("modelDeploymentSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    for summary in summaries {
        let arn = summary
            .get("customModelDeploymentArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let read = get(&emu, &arn).await;
        assert_eq!(
            summary.get("customModelDeploymentName").and_then(Value::as_str),
            Some(read.text("modelDeploymentName").as_str()),
            "the summary must report the name the read reports for {arn}"
        );
        assert_eq!(
            summary.get("modelArn").and_then(Value::as_str),
            Some(read.text("modelArn").as_str()),
            "the summary must report the custom model the read reports for {arn}"
        );
        assert_eq!(
            summary.get("createdAt").and_then(Value::as_str),
            read.get("createdAt").as_str(),
            "the summary must report the creation instant the read reports for {arn}"
        );
        // The status is the one member the clock moves, so the read may legitimately report
        // the status the summary did or the one it was owed since — never some third value.
        let summary_status = summary
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let read_status = read.text("status");
        assert!(
            read_status == summary_status || read_status == ACTIVE,
            "the summary reported {summary_status} for {arn}, so a read must report that \
             status or the one the clock has since owed it, got {read_status}"
        );
    }

    // The listing is scoped to the signing region: another region lists none of these.
    assert!(
        listed_arns("another region", &list_in_region(&emu, "eu-west-1", "").await).is_empty(),
        "a deployment belongs to the region it was created in, so another region lists none"
    );
}

// ---------------------------------------------------------------------------------------
// 7. The create's tags are the deployment's ONE tag list.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deployments_tags_are_the_list_the_create_filled() {
    let emu = emulator();

    // A deployment is a resource of this service, so its `customModelDeploymentArn` is a
    // `TaggableResourcesArn` the tagging operations resolve — to the very list the create
    // filled, reached the way every other family's is.
    let tagged = "tagged-deployment";
    let arn = deploy(
        &emu,
        "the tagged deployment",
        create_body(
            tagged,
            &custom_model_arn(CATALOG_BASE_MODELS[0], "abc123def456"),
            None,
            &[("team", "inference"), ("env", "test")],
        ),
    )
    .await;

    let reported = |reply: &Reply| -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = reply
            .get("tags")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| {
                (
                    row.get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    row.get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    };

    let listed = emu
        .call(
            "POST",
            "/listTagsForResource",
            Some(json!({ "resourceARN": arn })),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "a deployment's tags are listable by its ARN, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        reported(&listed),
        vec![
            ("env".to_string(), "test".to_string()),
            ("team".to_string(), "inference".to_string()),
        ],
        "the listing must report exactly the tags the create attached"
    );

    // A later association rewrites that same list: a key it already carries takes the new
    // value and a fresh key is added.
    let tagged_again = emu
        .call(
            "POST",
            "/tagResource",
            Some(json!({
                "resourceARN": arn,
                "tags": [
                    { "key": "env", "value": "prod" },
                    { "key": "owner", "value": "sim" },
                ],
            })),
        )
        .await;
    assert_eq!(
        tagged_again.status, 200,
        "TagResource on a deployment must succeed, got {} {:?}",
        tagged_again.status, tagged_again.body
    );
    let listed = emu
        .call(
            "POST",
            "/listTagsForResource",
            Some(json!({ "resourceARN": arn })),
        )
        .await;
    assert_eq!(
        reported(&listed),
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "sim".to_string()),
            ("team".to_string(), "inference".to_string()),
        ],
        "the association must rewrite the one list, one value per key"
    );

    // A deployment ARN this account holds no deployment for names nothing to tag.
    let unknown = emu
        .call(
            "POST",
            "/listTagsForResource",
            Some(json!({
                "resourceARN": format!(
                    "arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model-deployment/zzzzzzzzzzzz"
                ),
            })),
        )
        .await;
    assert_eq!(
        unknown.status, 404,
        "a deployment ARN naming nothing is refused, got {} {:?}",
        unknown.status, unknown.body
    );
}
