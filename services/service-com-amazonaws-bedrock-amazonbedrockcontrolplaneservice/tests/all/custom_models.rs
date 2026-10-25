// SPDX-License-Identifier: CC0-1.0

//! The custom model resource end-to-end: `CreateCustomModel` mints it, `GetCustomModel`
//! reads it back and `ListCustomModels` reports it, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A listing reports the stored custom models of the signing region**, ordered by
//!    `creationTime` in `sortOrder` (`Ascending` by default), filtered by every filter the
//!    request carries (`nameContains`, `baseModelArnEquals`, `foundationModelArnEquals`,
//!    `modelStatus`, `isOwned`, `creationTimeAfter`, `creationTimeBefore`), paged at
//!    `maxResults` with a `nextToken` that partitions the listing, and each summary
//!    agreeing with what a read of that one model reports.
//! 2. **A created custom model round-trips**: every member a read reports comes out of the
//!    stored record, for rows differing in data source (an Amazon S3 URI and a SageMaker AI
//!    model package ARN), encryption key, tags and name length (both modelled bounds),
//!    addressed by either `ModelIdentifier` spelling (its ARN or its name).
//! 3. **The model artifacts are named exactly one way**: "You can specify either
//!    `customModelDataSource` or `modelSourceConfig`, but not both", and `roleArn` "is
//!    required when you use `modelSourceConfig` with an Amazon S3 data source. It is not
//!    required when you use `customModelDataSource` with a model package ARN".
//! 4. **`modelStatus` is non-terminal first and settles to `Active`**: `Creating` straight
//!    after a create, `Active` once `CUSTOM_MODEL_ACTIVE_AFTER` has passed — and the settle
//!    is owed by the clock, not by the read.
//! 5. **A model name is unique per account per region** while the model is live, and a
//!    replayed `clientRequestToken` mints nothing a second time.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateCustomModel,GetCustomModel,ListCustomModels")`
//! / `query="shape"` / `query="doc"`). The requests go through `crate::support` — the ONE
//! harness every suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// `ModelStatus` is `[Active, Creating, Failed]`
/// (`smithy_query(query="shape",shape="ModelStatus")`).
const CREATING: &str = "Creating";
const ACTIVE: &str = "Active";
const FAILED: &str = "Failed";

/// `CustomizationType` — the type a model created from artifacts carries: "The model
/// appears in ListCustomModels with a customizationType of imported"
/// (`smithy_query(query="doc",shape="CreateCustomModel")`).
const IMPORTED: &str = "IMPORTED";

/// The base-model name such a model reports, which is the base segment its own
/// `CustomModelArn` carries.
const IMPORTED_BASE: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::CUSTOM_MODEL_IMPORTED_BASE;

/// How long after a create the `Creating` status is owed its move to `Active`, as the
/// service states it.
const ACTIVE_AFTER: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::CUSTOM_MODEL_ACTIVE_AFTER;

/// A `RoleArn`: `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockCustomModelImport";

/// A `KmsKeyArn`: `^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/abcd1234-ab12-cd34-ef56-abcdef123456";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// `modelSourceConfig` — a `ModelDataSource` naming the Amazon S3 artifacts
/// (`smithy_query(query="shape",shape="ModelDataSource,S3DataSource")`).
fn s3_source(uri: &str) -> Value {
    json!({ "s3DataSource": { "s3Uri": uri } })
}

/// `customModelDataSource` — a `CustomModelDataSource` naming a SageMaker AI model package
/// (`smithy_query(query="shape",shape="CustomModelDataSource,ModelPackageArnDataSource")`).
fn package_source(package_arn: &str) -> Value {
    json!({ "modelPackageArnDataSource": { "modelPackageArn": package_arn } })
}

/// A create body naming the artifacts in Amazon S3, which is the form that also needs a
/// `roleArn`.
fn s3_body(name: &str) -> Value {
    json!({
        "modelName": name,
        "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/exports/model/"),
        "roleArn": ROLE_ARN,
    })
}

/// A create body naming the artifacts by model package ARN, which needs no role.
fn package_body(name: &str) -> Value {
    json!({
        "modelName": name,
        "customModelDataSource": package_source(
            "arn:aws:sagemaker:us-west-2:000000000000:model-package/nova-export-1",
        ),
    })
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    create_in_region(emu, REGION, body).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        "/custom-models/create-custom-model",
        Some(body),
    )
    .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call("GET", &format!("/custom-models/{}", label(identifier)), None)
        .await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/custom-models".to_string()
    } else {
        format!("/custom-models?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// Create one custom model and hand back its ARN, failing the case by name when the create
/// is refused.
async fn mint(emu: &Emulator, case: &str, body: Value) -> String {
    let reply = create(emu, body).await;
    assert_eq!(
        reply.status, 202,
        "case {case}: CreateCustomModel must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("modelArn")
}

// ---------------------------------------------------------------------------------------
// Response helpers.
// ---------------------------------------------------------------------------------------

/// The summaries a listing reported, in the order it reported them.
fn listed(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: modelSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The ARNs a listing reported, in order.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    listed(case, reply)
        .iter()
        .map(|row| text(case, row, "modelArn"))
        .collect()
}

fn member(case: &str, value: &Value, name: &str) -> Value {
    value
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the response has no member {name}: {value:?}"))
}

fn text(case: &str, value: &Value, name: &str) -> String {
    let member = member(case, value, name);
    member
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: member {name} must be a string, got {member:?}"))
        .to_string()
}

/// A `Timestamp` member in epoch seconds, read whichever wire form that member takes.
///
/// The restJson1 codec picks the form from the MEMBER schema the model generates
/// (`aws_smithy_schema::Schema::timestamp_format`): a member the model marks
/// `@timestampFormat("date-time")` goes onto the wire as an RFC 3339 string, and one it
/// leaves unmarked takes the protocol's epoch-seconds default — which is the form the
/// members of this surface take. The rules this suite is about are the INSTANTS themselves
/// — the creation instant a read reports, and the two bounds a listing filters on — so
/// this reads either form rather than pinning one.
fn instant(case: &str, value: &Value, name: &str) -> f64 {
    let member = member(case, value, name);
    if let Some(raw) = member.as_str() {
        return date_time_seconds(case, raw);
    }
    member.as_f64().unwrap_or_else(|| {
        panic!(
            "case {case}: member {name} is a Timestamp, so it is either an RFC 3339 string or \
             epoch seconds, got {member}"
        )
    })
}

/// `seconds` since the epoch as the RFC 3339 date-time the two `creationTime` filters
/// carry on the wire, rendered with the same codec the wire uses.
fn date_time_query(seconds: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant the date-time format can render")
}

/// Whether `arn` is a `CustomModelArn` of a model created from artifacts:
/// `arn:aws:bedrock:<region>:<account>:custom-model/imported/[a-z0-9]{12}`.
fn is_imported_custom_model_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:custom-model/{IMPORTED_BASE}/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    id.len() == 12
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The `FoundationModelArn` of `model_id` in `region`: a foundation model is published by
/// AWS rather than owned by the account, so it carries an EMPTY account field.
fn foundation_model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{region}::foundation-model/{model_id}")
}

// ---------------------------------------------------------------------------------------
// 1. A created custom model round-trips through GetCustomModel and the listing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_custom_model_round_trips() {
    let emu = emulator();

    // `modelName` is `@length(min: 1, max: 63)`, so the rows include both bounds; the
    // artifacts are named either way the operation admits; `modelKmsKeyArn` and `modelTags`
    // are optional, so rows carry them and rows do not.
    let longest_name = "a".repeat(63);
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an Amazon S3 data source with the role it requires",
            "cm-alpha-one",
            s3_body("cm-alpha-one"),
        ),
        (
            "a model package data source and no role",
            "cm-alpha-two",
            package_body("cm-alpha-two"),
        ),
        (
            "a customer managed key and tags",
            "cm_beta_three",
            json!({
                "modelName": "cm_beta_three",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/beta/"),
                "roleArn": ROLE_ARN,
                "modelKmsKeyArn": KMS_KEY_ARN,
                "modelTags": [{ "key": "team", "value": "research" }],
            }),
        ),
        (
            "a shortest-possible name",
            "c",
            package_body("c"),
        ),
        (
            "a longest-possible name",
            longest_name.as_str(),
            s3_body(longest_name.as_str()),
        ),
    ];

    for (case, name, body) in rows {
        let sent_key = body.get("modelKmsKeyArn").cloned();
        let arn = mint(&emu, case, body).await;
        assert!(
            is_imported_custom_model_arn(&arn, REGION),
            "case {case}: {arn:?} is not a CustomModelArn of an imported model in {REGION}"
        );

        // Both `ModelIdentifier` spellings address the same model, and report the same
        // record.
        let by_arn = get(&emu, &arn).await;
        assert_eq!(
            by_arn.status, 200,
            "case {case}: a read of {arn} must answer 200, got {} {:?}",
            by_arn.status, by_arn.body
        );
        let by_name = get(&emu, name).await;
        assert_eq!(
            by_name.body, by_arn.body,
            "case {case}: a read by name must report the same record as a read by ARN, got \
             {:?} and {:?}",
            by_name.body, by_arn.body
        );

        let read = &by_arn.body;
        assert_eq!(
            text(case, read, "modelName"),
            name,
            "case {case}: the read must report the name the create carried"
        );
        assert_eq!(
            text(case, read, "modelArn"),
            arn,
            "case {case}: the read must report the ARN the create minted"
        );
        assert_eq!(
            text(case, read, "customizationType"),
            IMPORTED,
            "case {case}: a model created from artifacts is customizationType {IMPORTED}"
        );
        assert_eq!(
            text(case, read, "modelStatus"),
            CREATING,
            "case {case}: a custom model is still being created this soon after the create"
        );
        assert_eq!(
            text(case, read, "baseModelArn"),
            arn,
            "case {case}: the create named no foundation model, so the model is its own base"
        );
        assert_eq!(
            read.get("modelKmsKeyArn").cloned(),
            sent_key,
            "case {case}: modelKmsKeyArn must be reported exactly as the create carried it"
        );
        // "For models that you create with the CreateCustomModel API operation, this is
        // NULL": no customization job produced this model, so none of its members is
        // reported.
        for absent in [
            "jobArn",
            "jobName",
            "hyperParameters",
            "trainingDataConfig",
            "validationDataConfig",
            "outputDataConfig",
            "trainingMetrics",
            "validationMetrics",
            "customizationConfig",
            "failureMessage",
        ] {
            assert!(
                read.get(absent).is_none(),
                "case {case}: {absent} belongs to a customization job, so a model created \
                 from artifacts must not report it, got {:?}",
                read.get(absent)
            );
        }
        let created = instant(case, read, "creationTime");
        assert!(
            created > 1_700_000_000.0,
            "case {case}: creationTime must be the instant the model was created, got {created}"
        );

        // The listing reports the same record, in a summary that agrees with the read.
        // `baseModelArnEquals` selects exactly this model: a model created from artifacts
        // is its own base, so no other row carries that ARN.
        let listing = list(&emu, &format!("baseModelArnEquals={}", label(&arn))).await;
        let rows = listed(case, &listing);
        assert_eq!(
            rows.len(),
            1,
            "case {case}: the listing must report exactly the model that ARN names, got {rows:?}"
        );
        let summary = &rows[0];
        for shared in [
            "modelArn",
            "modelName",
            "creationTime",
            "baseModelArn",
            "customizationType",
        ] {
            assert_eq!(
                member(case, summary, shared),
                member(case, read, shared),
                "case {case}: the summary's {shared} must be what a read of it reports"
            );
        }
        // `modelStatus` is the one member the CLOCK can move between the read and the
        // listing, so this row asserts the status is one of the two the model's own
        // lifecycle passes through rather than racing the settle span;
        // `the_status_settles_to_active_on_the_clock` is where the transition itself is
        // pinned.
        let listed_status = text(case, summary, "modelStatus");
        assert!(
            listed_status == CREATING || listed_status == ACTIVE,
            "case {case}: a created model is Creating or settled to Active, got \
             {listed_status:?}"
        );
        assert_eq!(
            text(case, summary, "baseModelName"),
            IMPORTED_BASE,
            "case {case}: the base model name is the base segment the model's own ARN carries"
        );
        assert_eq!(
            text(case, summary, "ownerAccountId"),
            ACCOUNT,
            "case {case}: this account created the model, so it owns it"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. A listing reports the stored custom models, filtered, sorted and paged.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_reports_the_stored_custom_models() {
    let emu = emulator();

    assert!(
        listed_arns("an account that has created none", &list(&emu, "").await).is_empty(),
        "an account holding no custom model must be reported an empty listing"
    );

    // Two groups, so a creation-time bound can separate them: names carrying `alpha` and
    // names carrying `beta`.
    let first_group = ["cm-alpha-one", "cm-alpha-two"];
    let second_group = ["cm-beta-one", "cm-beta-two"];
    let mut arns: Vec<String> = Vec::new();
    for name in first_group {
        arns.push(mint(&emu, name, s3_body(name)).await);
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    for name in second_group {
        arns.push(mint(&emu, name, package_body(name)).await);
    }

    let all = list(&emu, "").await;
    assert_eq!(
        listed_arns("the default listing", &all),
        arns,
        "the default listing must report every stored custom model in creation order"
    );
    assert_eq!(
        listed_arns("sortBy=CreationTime", &list(&emu, "sortBy=CreationTime").await),
        arns,
        "sortBy=CreationTime is the order an absent sortBy already takes"
    );
    assert_eq!(
        listed_arns(
            "sortOrder=Ascending",
            &list(&emu, "sortBy=CreationTime&sortOrder=Ascending").await
        ),
        arns,
        "sortOrder=Ascending is the order an absent sortOrder already takes"
    );
    let mut reversed = arns.clone();
    reversed.reverse();
    assert_eq!(
        listed_arns(
            "sortOrder=Descending",
            &list(&emu, "sortOrder=Descending").await
        ),
        reversed,
        "sortOrder=Descending must report the same listing newest first"
    );

    // `nameContains`: a substring of the name, not the whole name.
    let filters: Vec<(&str, String, Vec<String>)> = vec![
        (
            "nameContains the first group's fragment",
            "nameContains=alpha".to_string(),
            arns[..first_group.len()].to_vec(),
        ),
        (
            "nameContains the second group's fragment",
            "nameContains=beta".to_string(),
            arns[first_group.len()..].to_vec(),
        ),
        (
            "nameContains a fragment every name carries",
            "nameContains=cm".to_string(),
            arns.clone(),
        ),
        (
            "nameContains a fragment no name carries",
            "nameContains=zeta".to_string(),
            Vec::new(),
        ),
        (
            "isOwned the models this account owns",
            "isOwned=true".to_string(),
            arns.clone(),
        ),
        (
            "isOwned the models shared with this account",
            "isOwned=false".to_string(),
            Vec::new(),
        ),
        (
            "baseModelArnEquals one model's own base",
            format!("baseModelArnEquals={}", label(&arns[0])),
            vec![arns[0].clone()],
        ),
        (
            "baseModelArnEquals another model's own base",
            format!("baseModelArnEquals={}", label(&arns[3])),
            vec![arns[3].clone()],
        ),
        (
            "baseModelArnEquals a foundation model none descends from",
            format!(
                "baseModelArnEquals={}",
                label(&foundation_model_arn(
                    REGION,
                    "amazon.titan-text-lite-v1"
                ))
            ),
            Vec::new(),
        ),
        (
            "foundationModelArnEquals a foundation model none descends from",
            format!(
                "foundationModelArnEquals={}",
                label(&foundation_model_arn(REGION, "amazon.nova-micro-v1"))
            ),
            Vec::new(),
        ),
        (
            "a name fragment and a base model that disagree",
            format!("nameContains=alpha&baseModelArnEquals={}", label(&arns[3])),
            Vec::new(),
        ),
    ];
    for (case, query, expected) in filters {
        assert_eq!(
            listed_arns(case, &list(&emu, &query).await),
            expected,
            "case {case}: every filter a request carries must hold for a model to be listed"
        );
    }

    // `modelStatus` partitions the listing by the status each model holds NOW: every model
    // here is `Creating` or has settled to `Active`, so those two filters together report
    // the whole listing and neither reports a model the other does; and no call here leaves
    // a model `Failed`. Stated as the partition rather than as one expected status, because
    // which side a model is on depends on the clock, not on the listing.
    let creating = listed_arns(
        "modelStatus=Creating",
        &list(&emu, &format!("modelStatus={CREATING}")).await,
    );
    let settled = listed_arns(
        "modelStatus=Active",
        &list(&emu, &format!("modelStatus={ACTIVE}")).await,
    );
    let mut partition: Vec<String> = creating.iter().chain(settled.iter()).cloned().collect();
    partition.sort();
    let mut every = arns.clone();
    every.sort();
    assert_eq!(
        partition, every,
        "case modelStatus: the Creating and Active filters must partition the listing, got \
         {creating:?} and {settled:?}"
    );
    assert!(
        listed_arns(
            "modelStatus a state no call here produces",
            &list(&emu, &format!("modelStatus={FAILED}")).await
        )
        .is_empty(),
        "no call here leaves a custom model {FAILED}, so that filter reports none"
    );

    // Every summary agrees with what a read of that one model reports.
    for row in listed("the default listing", &all) {
        let arn = text("summary agreement", &row, "modelArn");
        let read = get(&emu, &arn).await;
        for shared in [
            "modelArn",
            "modelName",
            "creationTime",
            "baseModelArn",
            "customizationType",
        ] {
            assert_eq!(
                member("summary agreement", &row, shared),
                member("summary agreement", &read.body, shared),
                "case {arn}: the summary's {shared} must be what a read of it reports"
            );
        }
    }

    // The two creation-time bounds, which the members word strictly: "created after" and
    // "created before" the instant given.
    let rows = listed("the default listing", &all);
    let last_of_first = instant("the bound rows", &rows[first_group.len() - 1], "creationTime");
    let first_of_second = instant("the bound rows", &rows[first_group.len()], "creationTime");
    let between = (last_of_first + first_of_second) / 2.0;
    let far_past = date_time_query(0.0);
    let far_future = date_time_query(4_102_444_800.0);
    let bounds: Vec<(&str, String, Vec<String>)> = vec![
        (
            "creationTimeAfter the epoch",
            format!("creationTimeAfter={}", label(&far_past)),
            arns.clone(),
        ),
        (
            "creationTimeAfter the far future",
            format!("creationTimeAfter={}", label(&far_future)),
            Vec::new(),
        ),
        (
            "creationTimeBefore the far future",
            format!("creationTimeBefore={}", label(&far_future)),
            arns.clone(),
        ),
        (
            "creationTimeBefore the epoch",
            format!("creationTimeBefore={}", label(&far_past)),
            Vec::new(),
        ),
        (
            "creationTimeAfter a bound between the two groups",
            format!("creationTimeAfter={}", label(&date_time_query(between))),
            arns[first_group.len()..].to_vec(),
        ),
        (
            "creationTimeBefore a bound between the two groups",
            format!("creationTimeBefore={}", label(&date_time_query(between))),
            arns[..first_group.len()].to_vec(),
        ),
        (
            "both bounds around the second group",
            format!(
                "creationTimeAfter={}&creationTimeBefore={}",
                label(&date_time_query(between)),
                label(&far_future)
            ),
            arns[first_group.len()..].to_vec(),
        ),
    ];
    for (case, query, expected) in bounds {
        assert_eq!(
            listed_arns(case, &list(&emu, &query).await),
            expected,
            "case {case}: the creation-time bounds must select exactly those models"
        );
    }

    // Pages: `maxResults` bounds the page and `nextToken` resumes the listing, so the pages
    // partition it with neither a gap nor a repeat.
    for page_size in [1_usize, 2, 3, arns.len(), arns.len() + 1] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={}", label(token)),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&emu, &query).await;
            let case = format!("maxResults={page_size} page {pages}");
            let reported = listed_arns(&case, &page);
            assert!(
                reported.len() <= page_size,
                "case {case}: a page must hold at most {page_size} rows, got {}",
                reported.len()
            );
            walked.extend(reported);
            token = page.get("nextToken").as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= arns.len() + 2,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, arns,
            "case maxResults={page_size}: the pages must partition the listing in order"
        );
    }

    // A token this listing never issued is refused rather than read as some other page.
    for token in [
        "not-a-cursor",
        "0:short",
        "00000000000000000000",
        "x0000000000000000000:arn",
    ] {
        let reply = list(&emu, &format!("nextToken={}", label(token))).await;
        assert_eq!(
            reply.status, 400,
            "case {token}: a foreign nextToken must be refused with 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {token}: a foreign nextToken must be refused with ValidationException, got {:?}",
            reply.error
        );
    }

    // The listing is per region: a model of one region is not listed in another, and the
    // same name is free there.
    assert!(
        listed_arns(
            "another region",
            &list_in_region(&emu, "eu-central-1", "").await
        )
        .is_empty(),
        "a custom model of one region is not listed in another"
    );
    let elsewhere = create_in_region(&emu, "eu-central-1", s3_body(first_group[0])).await;
    assert_eq!(
        elsewhere.status, 202,
        "the same model name in another region names another model, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere_arn = elsewhere.text("modelArn");
    assert_eq!(
        listed_arns(
            "the other region once it holds one",
            &list_in_region(&emu, "eu-central-1", "").await
        ),
        vec![elsewhere_arn.clone()],
        "the other region's listing reports only its own model"
    );
    assert_eq!(
        listed_arns("the signing region after the other region's create", &list(&emu, "").await),
        arns,
        "a create in another region must not change this region's listing"
    );
    // And a model of another region is not this region's to read either: this service reads
    // a `custom-model` ARN of another region ITSELF and refuses it with its own 400
    // `ValidationException` sentence ahead of the handler (`crate::arn_inputs`), and the
    // store's own lookup is per region for the same reason the listing is.
    let foreign = get(&emu, &elsewhere_arn).await;
    assert_eq!(
        foreign.status, 400,
        "a custom-model ARN of another region must be refused here, got {} {:?}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.error.as_deref(),
        Some("ValidationException"),
        "that refusal is the service's own ARN sentence, got {:?}",
        foreign.error
    );
}

// ---------------------------------------------------------------------------------------
// 3. modelStatus is non-terminal first and settles to Active on the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_status_settles_to_active_on_the_clock() {
    let emu = emulator();

    let name = "cm-settling";
    let arn = mint(&emu, "the settling custom model", s3_body(name)).await;

    let creating = get(&emu, &arn).await;
    assert_eq!(
        text("straight after the create", &creating.body, "modelStatus"),
        CREATING,
        "a custom model is reported Creating straight after its create"
    );
    assert_eq!(
        listed_arns(
            "modelStatus=Creating straight after the create",
            &list(&emu, &format!("modelStatus={CREATING}")).await
        ),
        vec![arn.clone()],
        "the listing filters on the status the model holds now"
    );

    // The transition is owed by the CLOCK: it becomes due `ACTIVE_AFTER` after the create
    // whether or not anybody reads the model.
    tokio::time::sleep(ACTIVE_AFTER + std::time::Duration::from_millis(150)).await;

    let settled = get(&emu, &arn).await;
    assert_eq!(
        text("after the due instant", &settled.body, "modelStatus"),
        ACTIVE,
        "a custom model is Active once the create's span has passed"
    );
    assert_eq!(
        instant("after the due instant", &settled.body, "creationTime"),
        instant("straight after the create", &creating.body, "creationTime"),
        "the settle must not move the instant the model was created"
    );
    assert_eq!(
        listed_arns(
            "modelStatus=Active after the due instant",
            &list(&emu, &format!("modelStatus={ACTIVE}")).await
        ),
        vec![arn.clone()],
        "the listing reports and filters on the settled status"
    );
    assert!(
        listed_arns(
            "modelStatus=Creating after the due instant",
            &list(&emu, &format!("modelStatus={CREATING}")).await
        )
        .is_empty(),
        "a settled model is no longer reported as Creating"
    );
}

// ---------------------------------------------------------------------------------------
// 4. The artifacts are named exactly one way, and a name is unique per region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_create_naming_its_artifacts_wrongly_is_refused() {
    let emu = emulator();

    let refused: Vec<(&str, Value)> = vec![
        (
            "neither data source",
            json!({ "modelName": "cm-refused", "roleArn": ROLE_ARN }),
        ),
        (
            "both data sources",
            json!({
                "modelName": "cm-refused",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/both/"),
                "customModelDataSource": package_source(
                    "arn:aws:sagemaker:us-west-2:000000000000:model-package/nova-export-2",
                ),
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "an Amazon S3 data source without the role it requires",
            json!({
                "modelName": "cm-refused",
                "modelSourceConfig": s3_source("s3://bedrock-model-artifacts/no-role/"),
            }),
        ),
    ];
    for (case, body) in refused {
        let reply = create(&emu, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the create must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: the create must answer ValidationException, got {:?}",
            reply.error
        );
        assert_eq!(
            get(&emu, "cm-refused").await.status,
            404,
            "case {case}: a refused create must store nothing"
        );
    }
    assert!(
        listed_arns("after the refused creates", &list(&emu, "").await).is_empty(),
        "a refused create must leave the listing empty"
    );

    // A model package data source needs no role at all.
    let without_role = create(&emu, package_body("cm-no-role")).await;
    assert_eq!(
        without_role.status, 202,
        "a model package data source needs no roleArn, got {} {:?}",
        without_role.status, without_role.body
    );
}

#[tokio::test]
async fn a_model_name_is_unique_and_a_replayed_token_mints_nothing() {
    let emu = emulator();

    let name = "cm-unique";
    let first = mint(&emu, "the first create", s3_body(name)).await;

    // A second create of a live name is a conflict, whichever data source it names.
    for (case, body) in [
        ("the same data source", s3_body(name)),
        ("another data source", package_body(name)),
    ] {
        let reply = create(&emu, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: a duplicate modelName must answer 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ConflictException"),
            "case {case}: a duplicate modelName must answer ConflictException, got {:?}",
            reply.error
        );
    }
    assert_eq!(
        listed_arns("after the duplicate creates", &list(&emu, "").await),
        vec![first.clone()],
        "a refused duplicate must not add a second model"
    );

    // `@idempotencyToken`: a replayed token answers with what the first request minted.
    let mut body = s3_body("cm-token");
    body["clientRequestToken"] = json!("TokenOne1");
    let minted = mint(&emu, "the token's first create", body.clone()).await;
    let replay = create(&emu, body.clone()).await;
    assert_eq!(
        replay.status, 202,
        "a replayed token must be admitted, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("modelArn"),
        minted,
        "a replayed token must answer with the ARN the first request minted"
    );
    // Even when the replay names another model: the token, not the body, decides.
    let mut other = s3_body("cm-token-other");
    other["clientRequestToken"] = json!("TokenOne1");
    let second_replay = create(&emu, other).await;
    assert_eq!(
        second_replay.text("modelArn"),
        minted,
        "a replayed token mints nothing a second time"
    );
    assert_eq!(
        listed_arns("after the token replays", &list(&emu, "").await),
        vec![first, minted],
        "the replays must leave exactly the two models the two fresh creates minted"
    );
}

// ---------------------------------------------------------------------------------------
// 5. An identifier that names no custom model of this account is not found.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_of_an_unknown_custom_model_is_not_found() {
    let emu = emulator();

    let name = "cm-present";
    mint(&emu, "the live model", s3_body(name)).await;

    let unknown: Vec<(&str, String)> = vec![
        ("a name no create ever used", "cm-absent".to_string()),
        ("a prefix of a live name", "cm-presen".to_string()),
        (
            "an in-region ARN of this account that names nothing",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{IMPORTED_BASE}/aaaaaaaaaaaa"),
        ),
        (
            "an in-region ARN of another account",
            format!(
                "arn:aws:bedrock:{REGION}:123456789012:custom-model/{IMPORTED_BASE}/aaaaaaaaaaaa"
            ),
        ),
    ];
    for (case, identifier) in unknown {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: a read of {identifier} must answer 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a read of {identifier} must answer ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}
