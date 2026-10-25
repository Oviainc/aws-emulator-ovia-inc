// SPDX-License-Identifier: CC0-1.0

//! The application inference profile resource end-to-end: `CreateInferenceProfile` writes
//! it, `GetInferenceProfile` reads it back and `DeleteInferenceProfile` forgets it, over
//! the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created profile round-trips**: every member `GetInferenceProfile` reports comes
//!    back out of the stored record, for profiles differing in description, tag count,
//!    name form and model source, and under BOTH spellings of an
//!    `InferenceProfileIdentifier` (the ARN and the bare `inferenceProfileId`).
//! 2. **The profile tracks the model its `modelSource.copyFrom` names**, as a
//!    `FoundationModelArn` in the region the source carries: a `foundation-model` source
//!    names the model directly, and a system-defined `inference-profile` source names the
//!    model behind its geography prefix.
//! 3. **A replayed `clientRequestToken` mints nothing new**, and two distinct tokens mint
//!    two profiles.
//! 4. **`inferenceProfileName` is not unique** — the member's doc states no uniqueness, so
//!    a second create of one name mints a second profile.
//! 5. **A delete forgets the profile**, whatever else refers to it (a prompt router naming
//!    it as a target model), and an identifier naming no profile of this account and
//!    region is 404 for both the read and the delete.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateInferenceProfile,GetInferenceProfile,DeleteInferenceProfile")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// Base model ids this emulator's published catalog carries
/// (`crate::foundation_models` reports them; the ids are the `modelId`s
/// `ListFoundationModels` answers with).
const CATALOG_MODEL: &str = "anthropic.claude-3-haiku-20240307-v1:0";
const OTHER_CATALOG_MODEL: &str = "amazon.nova-pro-v1:0";

/// A base model id the published catalog does NOT carry, which the real service does: the
/// rule reads the source ARN, so a model outside this emulator's subset is tracked the
/// same way.
const MODEL_OUTSIDE_THE_CATALOG: &str = "anthropic.claude-3-5-sonnet-20240620-v1:0";

/// A `foundation-model` source ARN, as `InferenceProfileModelSourceArn`'s pattern spells
/// one (`smithy_query(query="shape",shape="InferenceProfileModelSource")`).
fn foundation_model_source(region: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{region}::foundation-model/{model_id}")
}

/// A system-defined `inference-profile` source ARN — the other resource type that same
/// pattern admits — whose id is a base model id behind a geography prefix.
fn system_profile_source(region: &str, profile_id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:inference-profile/{profile_id}")
}

/// The `FoundationModelArn` a tracked model is reported as
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/…$`: an EMPTY account
/// field, since a foundation model is published by AWS).
fn model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:bedrock:{region}::foundation-model/{model_id}")
}

/// The ARN prefix every profile minted for `region` carries (`InferenceProfileArn`'s
/// pattern: `…:bedrock:<region>:<account>:application-inference-profile/…`).
fn arn_prefix(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:application-inference-profile/")
}

/// A create body with only the required members.
fn create_body(name: &str, source: &str) -> Value {
    json!({
        "inferenceProfileName": name,
        "modelSource": { "copyFrom": source },
    })
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/inference-profiles", Some(body)).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/inference-profiles", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    get_in_region(emu, REGION, identifier).await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/inference-profiles/{}", label(identifier)),
        None,
    )
    .await
}

async fn delete(emu: &Emulator, identifier: &str) -> Reply {
    delete_in_region(emu, REGION, identifier).await
}

async fn delete_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "DELETE",
        &format!("/inference-profiles/{}", label(identifier)),
        None,
    )
    .await
}

/// Create a profile with only the required members and hand back its ARN.
async fn given_profile(emu: &Emulator, name: &str, source: &str) -> String {
    let reply = create(emu, create_body(name, source)).await;
    assert_eq!(
        reply.status, 201,
        "case {name:?}: CreateInferenceProfile must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("inferenceProfileArn")
}

/// The `modelArn`s a read reported, in the order it reported them.
fn reported_models(case: &str, reply: &Reply) -> Vec<String> {
    reply
        .get("models")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: models must be a present array, got {:?}", reply.body))
        .iter()
        .map(|row| {
            row.get("modelArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the round-trip table: the case name, the profile name, the optional
/// description, and the tags the create carries.
type ProfileRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static [(&'static str, &'static str)],
);

// ---------------------------------------------------------------------------------------
// 1. A created profile round-trips, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_profile_is_read_back_under_both_identifier_spellings() {
    // (case, name, description, tags) — the rows differ along what the create admits:
    // the optional description, the tag count (none, one, several), and the name forms
    // `InferenceProfileName`'s pattern (`^([0-9a-zA-Z][ _-]?)+$`) admits.
    let rows: &[ProfileRow] = &[
        ("only the required members", "profile-one", None, &[]),
        (
            "a description and one tag",
            "profile two",
            Some("tracks the haiku model"),
            &[("Environment", "production")],
        ),
        (
            "several tags",
            "Profile_Three",
            Some("costs by team"),
            &[("Team", "search"), ("CostCenter", "4711"), ("Owner", "ada")],
        ),
    ];

    let emu = emulator();
    for (case, name, description, tags) in rows {
        let mut body = create_body(name, &foundation_model_source(REGION, CATALOG_MODEL));
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

        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateInferenceProfile must answer 201, got {} {:?}",
            created.status, created.body
        );
        let arn = created.text("inferenceProfileArn");
        assert!(
            arn.starts_with(&arn_prefix(REGION)),
            "case {case:?}: the minted ARN must be an application-inference-profile ARN of \
             {REGION}, got {arn:?}"
        );
        assert_eq!(
            created.get("status"),
            json!("ACTIVE"),
            "case {case:?}: the only InferenceProfileStatus is ACTIVE, got {:?}",
            created.get("status")
        );

        // Both spellings `InferenceProfileIdentifier` admits name the same record.
        let id = arn
            .rsplit_once('/')
            .map(|(_, id)| id.to_string())
            .unwrap_or_default();
        for (spelling, identifier) in [("the ARN", arn.clone()), ("the bare id", id.clone())] {
            let read = get(&emu, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} ({spelling}): GetInferenceProfile must answer 200, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("inferenceProfileArn"),
                arn,
                "case {case:?} ({spelling}): the read must report the stored ARN"
            );
            assert_eq!(
                read.text("inferenceProfileId"),
                id,
                "case {case:?} ({spelling}): the read must report the ARN's own id"
            );
            assert_eq!(
                read.text("inferenceProfileName"),
                *name,
                "case {case:?} ({spelling}): the read must report the submitted name"
            );
            assert_eq!(
                read.get("description"),
                description.map_or(Value::Null, |text| json!(text)),
                "case {case:?} ({spelling}): the read must report the submitted description, \
                 got {:?}",
                read.get("description")
            );
            assert_eq!(
                read.text("status"),
                "ACTIVE",
                "case {case:?} ({spelling}): a created profile is ACTIVE"
            );
            assert_eq!(
                read.text("type"),
                "APPLICATION",
                "case {case:?} ({spelling}): a profile a caller created is APPLICATION"
            );
            assert_eq!(
                reported_models(case, &read),
                vec![model_arn(REGION, CATALOG_MODEL)],
                "case {case:?} ({spelling}): the read must report the tracked model"
            );
            let created_at = read.instant("createdAt");
            let updated_at = read.instant("updatedAt");
            assert!(
                (created_at - updated_at).abs() < f64::EPSILON,
                "case {case:?} ({spelling}): nothing has changed the profile, so updatedAt \
                 ({updated_at}) must equal createdAt ({created_at})"
            );
        }

        // The tags the create carried are the tags the tagging path reports for the
        // profile: `TaggableResourcesArn` admits an `application-inference-profile` ARN
        // (`smithy_query(query="shape",shape="TaggableResourcesArn")`).
        let listed = emu
            .call(
                "POST",
                "/listTagsForResource",
                Some(json!({ "resourceARN": arn })),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource must answer 200 for a live profile, got {} {:?}",
            listed.status, listed.body
        );
        let reported: Vec<(String, String)> = listed
            .get("tags")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tag| {
                (
                    tag.get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    tag.get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the profile's tags are the ones the create carried, got {:?}",
            listed.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// The wire form of the two instants a read reports.
// ---------------------------------------------------------------------------------------

/// The two instants `GetInferenceProfileResponse` carries target `Timestamp`, which the
/// model marks `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="Timestamp")`), so each member schema must report
/// that format — it is what makes the codec write the RFC 3339 string the round-trip test
/// reads rather than the protocol's default epoch number.
#[test]
fn the_date_time_members_of_a_profile_read_carry_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::GetInferenceProfileOutput;

    let rows = [
        (
            "GetInferenceProfileResponse$createdAt",
            &GetInferenceProfileOutput::M_CREATED_AT,
        ),
        (
            "GetInferenceProfileResponse$updatedAt",
            &GetInferenceProfileOutput::M_UPDATED_AT,
        ),
    ];
    for (case, schema) in rows {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), \
             got {format:?}"
        );
    }

    // The control row: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let status = GetInferenceProfileOutput::M_STATUS
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        status.is_none(),
        "case GetInferenceProfileResponse$status: the model gives this member no \
         @timestampFormat, got {status:?}"
    );
}


#[tokio::test]
async fn the_tracked_model_is_the_one_the_model_source_names() {
    // (case, copyFrom, the FoundationModelArn the profile must track). The rows differ
    // along what the rule generalises over: the source's resource type
    // (`foundation-model` / system-defined `inference-profile`), the region field the
    // source carries (this one, another, none at all), the geography prefix a
    // system-defined profile id carries, and whether the model is one the published
    // catalog happens to carry.
    let rows: &[(&str, String, String)] = &[
        (
            "a foundation model of this region",
            foundation_model_source(REGION, CATALOG_MODEL),
            model_arn(REGION, CATALOG_MODEL),
        ),
        (
            "a foundation model of another region",
            foundation_model_source("us-east-1", OTHER_CATALOG_MODEL),
            model_arn("us-east-1", OTHER_CATALOG_MODEL),
        ),
        (
            "a foundation model whose ARN carries no region",
            format!("arn:aws:bedrock::{ACCOUNT}:foundation-model/{CATALOG_MODEL}"),
            model_arn(REGION, CATALOG_MODEL),
        ),
        (
            "a foundation model the published catalog does not carry",
            foundation_model_source(REGION, MODEL_OUTSIDE_THE_CATALOG),
            model_arn(REGION, MODEL_OUTSIDE_THE_CATALOG),
        ),
        (
            "a system-defined profile with the us geography prefix",
            system_profile_source(REGION, &format!("us.{CATALOG_MODEL}")),
            model_arn(REGION, CATALOG_MODEL),
        ),
        (
            "a system-defined profile with the eu geography prefix",
            system_profile_source("eu-west-1", &format!("eu.{OTHER_CATALOG_MODEL}")),
            model_arn("eu-west-1", OTHER_CATALOG_MODEL),
        ),
        (
            "a system-defined profile over a model outside the catalog",
            system_profile_source(REGION, &format!("us.{MODEL_OUTSIDE_THE_CATALOG}")),
            model_arn(REGION, MODEL_OUTSIDE_THE_CATALOG),
        ),
        (
            "a system-defined profile id with no geography prefix to drop",
            system_profile_source(REGION, CATALOG_MODEL),
            model_arn(REGION, CATALOG_MODEL),
        ),
    ];

    let emu = emulator();
    for (index, (case, source, expected)) in rows.iter().enumerate() {
        let name = format!("source-{index}");
        let arn = given_profile(&emu, &name, source).await;
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetInferenceProfile must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            reported_models(case, &read),
            vec![expected.clone()],
            "case {case:?}: the profile created from {source} must track {expected}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. Idempotency: a replayed `clientRequestToken` mints nothing new.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing_new() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);

    // The rows differ in what the REPLAY carries beside the token: the same body, and a
    // body whose other members changed — `@idempotencyToken` means the token decides,
    // so both answer with the profile the token already minted.
    let first = create(&emu, {
        let mut body = create_body("token-one", &source);
        body["clientRequestToken"] = json!("Token0001");
        body
    })
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must answer 201, got {} {:?}",
        first.status, first.body
    );
    let minted = first.text("inferenceProfileArn");

    for (case, name) in [
        ("the same body", "token-one"),
        ("a body with another name", "token-one-again"),
    ] {
        let replay = create(&emu, {
            let mut body = create_body(name, &source);
            body["clientRequestToken"] = json!("Token0001");
            body
        })
        .await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a replayed token must be accepted, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("inferenceProfileArn"),
            minted,
            "case {case:?}: a replayed token must answer with the profile it already minted"
        );
        let read = get(&emu, &minted).await;
        assert_eq!(
            read.text("inferenceProfileName"),
            "token-one",
            "case {case:?}: a replay must change nothing about the stored profile, got {:?}",
            read.body
        );
    }

    // A DIFFERENT token mints a second profile, and the same token in another region is
    // an independent request: the token index is scoped to the region, the way every
    // other create of this service scopes it.
    let second = create(&emu, {
        let mut body = create_body("token-two", &source);
        body["clientRequestToken"] = json!("Token0002");
        body
    })
    .await;
    assert_ne!(
        second.text("inferenceProfileArn"),
        minted,
        "case \"a second token\": a distinct token must mint a distinct profile, got {:?}",
        second.body
    );

    let elsewhere = create_in_region(&emu, "eu-central-1", {
        let mut body = create_body("token-one", &source);
        body["clientRequestToken"] = json!("Token0001");
        body
    })
    .await;
    let elsewhere_arn = elsewhere.text("inferenceProfileArn");
    assert!(
        elsewhere_arn.starts_with(&arn_prefix("eu-central-1")),
        "case \"the same token in another region\": the profile must be minted in that \
         region, got {elsewhere_arn:?}"
    );
    assert_ne!(
        elsewhere_arn, minted,
        "case \"the same token in another region\": that token minted nothing in this region"
    );
}

// ---------------------------------------------------------------------------------------
// 4. `inferenceProfileName` is not unique.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn one_name_may_be_held_by_several_profiles() {
    // `inferenceProfileName` is documented as "a name for the inference profile", with no
    // uniqueness clause of the kind `promptRouterName` carries
    // (`smithy_query(query="doc",shape="CreateInferenceProfileRequest",member="inferenceProfileName")`),
    // so a second create of one name mints a second profile rather than conflicting.
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);

    let mut minted: Vec<String> = Vec::new();
    for attempt in 0..3 {
        let case = format!("create {attempt} of one name");
        let reply = create(&emu, create_body("shared name", &source)).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: the create must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply.text("inferenceProfileArn");
        assert!(
            !minted.contains(&arn),
            "case {case:?}: each create must mint its own profile, got {arn:?} again"
        );
        minted.push(arn);
    }

    for (index, arn) in minted.iter().enumerate() {
        let read = get(&emu, arn).await;
        assert_eq!(
            read.status, 200,
            "case \"profile {index}\": every minted profile must read back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("inferenceProfileName"),
            "shared name",
            "case \"profile {index}\": each holds the name it was created with"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. A delete forgets the profile; an identifier naming none is 404.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_forgets_the_profile_whatever_refers_to_it() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    let arn = given_profile(&emu, "referenced", &source).await;

    // A prompt router may name the profile as a target model —
    // `PromptRouterTargetModelArn` admits an `application-inference-profile` ARN
    // (`smithy_query(query="shape",shape="PromptRouterTargetModel")`) — and that
    // reference does not hold the delete back.
    let router = emu
        .call(
            "POST",
            "/prompt-routers",
            Some(json!({
                "promptRouterName": "router over the profile",
                "models": [{ "modelArn": arn }],
                "fallbackModel": { "modelArn": foundation_model_source(REGION, OTHER_CATALOG_MODEL) },
                "routingCriteria": { "responseQualityDifference": 5.0 },
            })),
        )
        .await;
    assert_eq!(
        router.status, 200,
        "the prompt router that references the profile must be created, got {} {:?}",
        router.status, router.body
    );

    // Both spellings of the identifier address the same profile, so the delete is driven
    // through each in turn: the first removes it, the second finds nothing.
    let removed = delete(&emu, &arn).await;
    assert_eq!(
        removed.status, 200,
        "case \"a referenced profile\": the delete must be accepted, got {} {:?}",
        removed.status, removed.body
    );

    let read = get(&emu, &arn).await;
    assert_eq!(
        read.status, 404,
        "case \"after the delete\": the read must answer 404, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"after the delete\": the read must answer ResourceNotFoundException, got {:?}",
        read.error
    );

    let again = delete(&emu, &arn).await;
    assert_eq!(
        again.status, 404,
        "case \"a second delete\": the delete must answer 404, got {} {:?}",
        again.status, again.body
    );

    // The router the profile was referenced by is untouched by the delete.
    let router_arn = router.text("promptRouterArn");
    let router_read = emu
        .call(
            "GET",
            &format!("/prompt-routers/{}", label(&router_arn)),
            None,
        )
        .await;
    assert_eq!(
        router_read.status, 200,
        "case \"the referring router\": it must still read back, got {} {:?}",
        router_read.status, router_read.body
    );
}

#[tokio::test]
async fn an_identifier_naming_no_profile_is_refused_by_the_read_and_the_delete() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);

    // One live profile in another region, so the row that names it is refused for being
    // another account-region's profile rather than for an empty store.
    let elsewhere = create_in_region(&emu, "eu-central-1", create_body("far away", &source)).await;
    let elsewhere_arn = elsewhere.text("inferenceProfileArn");
    // One live profile in this region, so the refusals cannot be an empty store either.
    let live = given_profile(&emu, "the live one", &source).await;

    // The rows differ along what the rule generalises over: the spelling of the
    // identifier (bare id, this region's ARN, another region's ARN) and the resource type
    // the ARN names (the system-defined `inference-profile` type, which no operation of
    // this service mints).
    let rows: Vec<(&str, String)> = vec![
        ("a bare id nothing minted", "zzzzzzzzzzzz".to_string()),
        (
            "an application profile ARN nothing minted",
            format!("{}zzzzzzzzzzzz", arn_prefix(REGION)),
        ),
        (
            "a system-defined profile ARN",
            system_profile_source(REGION, &format!("us.{CATALOG_MODEL}")),
        ),
        ("a live profile of another region", elsewhere_arn.clone()),
    ];

    for (case, identifier) in rows {
        let read = get(&emu, &identifier).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: GetInferenceProfile must answer 404 for {identifier}, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: GetInferenceProfile must answer ResourceNotFoundException, got {:?}",
            read.error
        );

        let removed = delete(&emu, &identifier).await;
        assert_eq!(
            removed.status, 404,
            "case {case:?}: DeleteInferenceProfile must answer 404 for {identifier}, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: DeleteInferenceProfile must answer ResourceNotFoundException, got \
             {:?}",
            removed.error
        );
    }

    // Neither live profile was removed by any of those refusals.
    for (case, region, arn) in [
        ("this region's profile", REGION, &live),
        ("the other region's profile", "eu-central-1", &elsewhere_arn),
    ] {
        let read = get_in_region(&emu, region, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused call must remove nothing, got {} {:?}",
            read.status, read.body
        );
    }

    // And the other region's profile is deleted in its OWN region, which is where it
    // lives: the same ARN, addressed from the region it carries.
    let removed = delete_in_region(&emu, "eu-central-1", &elsewhere_arn).await;
    assert_eq!(
        removed.status, 200,
        "case \"the other region's profile in its own region\": the delete must be accepted, \
         got {} {:?}",
        removed.status, removed.body
    );
}
