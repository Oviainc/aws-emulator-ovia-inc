// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateAgentProfile` — the create that mints an agent
//! profile (`POST /api/v1/agent-profiles`, `smithy_query(query="operation",
//! operation="CreateAgentProfile")`).
//!
//! The round trip of a created profile and its derived members is pinned by
//! `super::agent_goals`, and the resolution of several stored profiles by
//! `super::get_agent_profile`. What this suite adds is the part of the create's own
//! modelled surface those do not reach, each rule over a TABLE of inputs rather than one
//! request:
//!
//! 1. IDEMPOTENCY. `clientToken` is modelled `@idempotencyToken`: the same token with the
//!    same parameters answers the profile the first request created and records no second
//!    one, the same token with ANY parameter changed is a `409 ConflictException` naming
//!    the profile that token already created and recording nothing, and a fresh token —
//!    or no token at all — mints a new profile even when every other member is identical.
//!    The rows differ along WHICH member changed, because what makes a second request a
//!    different request is any parameter of it, not one particular member.
//! 2. METERING. Agent profiles are metered: a create AT the ceiling is accepted and the
//!    one past it is refused `402 ServiceQuotaExceededException` naming the kind of
//!    resource the quota meters, the ceiling it reached and the `@required` `QuotaCode` /
//!    `ServiceCode`, and records nothing — while the profiles inside the ceiling still
//!    resolve.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the agent-profile
//! request builders and the profile projection check live in `super::agent_goals`.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    AgentProfileRecord, MAX_AGENT_PROFILES,
};

use super::agent_goals::{
    assert_profile_projection, create_profile, get_profile, member, profile_body, PILLARS,
};
use super::workload_harness::{Answer, Client, SIGNED_REGION};

/// `ExceptionResourceType` for an agent profile: a free-form string, and this service
/// words the kind of resource an agent-profile failure is about this way on every
/// operation that resolves or meters one (`super::agent_profile_lifecycle` pins the same
/// value on the lookup and the delete).
const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// The `arn` a successful create answered with, checked against the modelled
/// `AgentProfileArn` (`^arn:aws([a-z0-9-]+)?:wellarchitected:[a-z0-9-]{6,64}:\d{12}:agent-profile/([a-zA-Z0-9_-]+)$`,
/// `@length(max: 2048)`) in the Region the request was signed for.
fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentProfile 200, got {answer:?}"
    );
    let arn = member(answer, "arn").to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:{SIGNED_REGION}:{}:agent-profile/",
        engine::emulator_account_id()
    );
    let resource = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        !resource.is_empty()
            && resource
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "case {case:?}: arn must be {expected_prefix}<[a-zA-Z0-9_-]+>, got {arn:?}"
    );
    assert!(
        arn.len() <= 2048,
        "case {case:?}: arn must satisfy @length(max: 2048), got {} characters in {arn:?}",
        arn.len()
    );
    arn
}

/// `body` with every member of `overrides` applied, so a row states only what it varies.
/// A `null` override REMOVES the member, which is how a row drops an optional one.
fn with_overrides(mut body: Value, overrides: &Value) -> Value {
    for (name, value) in overrides.as_object().expect("overrides are an object") {
        if value.is_null() {
            body.as_object_mut()
                .expect("the body is an object")
                .remove(name);
        } else {
            body[name] = value.clone();
        }
    }
    body
}

/// The base `CreateAgentProfile` body rule 1 replays: every optional member present, one
/// aggregation scope (so the created profile is eligible for generation) and tags, so a
/// row can change ANY of them.
fn base_body() -> Value {
    with_overrides(
        profile_body("token-base-profile", &["SECURITY"], 1),
        &json!({
            "tags": [
                { "key": "team", "value": "core" },
                { "key": "cost-centre", "value": "4711" },
            ],
        }),
    )
}

/// How many profiles the store holds.
fn stored_profiles(client: &Client) -> usize {
    client.store().lock().agent_profiles.len()
}

/// Rule 1, the accepting half: a token replayed with the same parameters answers the
/// profile the first request created, while a fresh token and no token at all each mint a
/// new one. The rows differ along what makes the second request idempotent — the same
/// token, another token, no token — over parameters that are otherwise identical.
#[tokio::test]
async fn a_client_token_replays_the_created_profile_and_only_a_token_does() {
    let client = Client::new();
    let token = "agent-profile-token-1";

    let original = with_overrides(base_body(), &json!({ "clientToken": token }));
    let first = create_profile(&client, original.clone()).await;
    let arn = created_arn(&first, "original");

    // The created profile is the one the request described, and it reads back.
    assert_profile_projection(
        &first.body,
        &original,
        "original",
        "CreateAgentProfile",
        // `deletionProtection` is modelled `@default(true)` and the base body sends none.
        true,
        // One aggregation scope, so the profile is eligible for generation.
        true,
    );
    let read_back = get_profile(&client, &arn).await;
    assert_eq!(
        read_back.status, 200,
        "case \"original\": the created profile must read back, got {read_back:?}"
    );
    assert_profile_projection(
        &read_back.body,
        &original,
        "original",
        "GetAgentProfile",
        true,
        true,
    );

    // The same token with the same parameters: the original profile, byte for byte, and
    // no second profile recorded.
    for attempt in 1..=2 {
        let case = format!("replay-{attempt}");
        let replay = create_profile(&client, original.clone()).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: expected CreateAgentProfile 200, got {replay:?}"
        );
        assert_eq!(
            replay.body, first.body,
            "case {case:?}: a replay must answer the profile the original create made, \
             got {replay:?}"
        );
        assert_eq!(
            stored_profiles(&client),
            1,
            "case {case:?}: a replay must not record a second profile"
        );
    }

    // A token the ledger has not seen, and a request carrying NO token, each mint a new
    // profile from the very same parameters: only a repeated token makes a create a
    // replay.
    let mut minted = vec![arn];
    let fresh_rows: Vec<(&str, Value)> = vec![
        (
            "fresh-token",
            json!({ "clientToken": "agent-profile-token-2" }),
        ),
        ("no-token-first", json!({ "clientToken": null })),
        ("no-token-second", json!({ "clientToken": null })),
    ];
    for (case, overrides) in fresh_rows {
        let request = with_overrides(original.clone(), &overrides);
        let answer = create_profile(&client, request.clone()).await;
        let new_arn = created_arn(&answer, case);
        assert!(
            !minted.contains(&new_arn),
            "case {case:?}: every accepted create mints its own ARN, got a repeat of \
             {new_arn:?}"
        );
        assert_profile_projection(
            &answer.body,
            &request,
            case,
            "CreateAgentProfile",
            true,
            true,
        );
        minted.push(new_arn);
        assert_eq!(
            stored_profiles(&client),
            minted.len(),
            "case {case:?}: an accepted create records exactly one more profile"
        );
    }

    // Every minted profile still resolves to its own record.
    for arn in &minted {
        let fetched = get_profile(&client, arn).await;
        assert_eq!(
            fetched.status, 200,
            "case \"after-the-rows\": {arn:?} must still resolve, got {fetched:?}"
        );
        assert_eq!(
            fetched.body.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case \"after-the-rows\": {arn:?} must report itself, got {fetched:?}"
        );
    }
}

/// Rule 1, the refusing half. The rows differ along WHICH member of an otherwise
/// identical request changed — a required member, each optional text member, the list
/// members, the defaulted boolean and the tags — because any changed parameter makes the
/// second request a different request.
#[tokio::test]
async fn a_token_resubmitted_with_different_parameters_is_a_conflict_that_records_nothing() {
    let client = Client::new();
    let token = "agent-profile-token-conflict";

    let original = with_overrides(base_body(), &json!({ "clientToken": token }));
    let first = create_profile(&client, original.clone()).await;
    let arn = created_arn(&first, "original");

    let rows: Vec<(&str, Value)> = vec![
        ("other-name", json!({ "name": "another-profile-name" })),
        (
            "other-display-name",
            json!({ "displayName": "Another display name" }),
        ),
        (
            "other-description",
            json!({ "description": "another description" }),
        ),
        ("description-dropped", json!({ "description": null })),
        (
            "other-business-overview",
            json!({ "businessOverview": "another business overview" }),
        ),
        // `Pillars` is `@length(1, 5)` with `@uniqueItems`: another member of the set, and
        // the whole published set, are both different parameters.
        ("other-pillar", json!({ "pillars": ["RESILIENCE"] })),
        ("every-pillar", json!({ "pillars": PILLARS })),
        // `deletionProtection` is `@default(true)`, which the original took: sending
        // `false` is a different parameter.
        ("deletion-protection-off", json!({ "deletionProtection": false })),
        (
            "other-execution-role",
            json!({
                "executionRoleArn": format!(
                    "arn:aws:iam::{}:role/AnotherAgentProfileExecution",
                    engine::emulator_account_id()
                ),
            }),
        ),
        // `AggregationConfigurations` is `@length(0, 100)`: no scope at all, and a
        // different scope, are each a different parameter.
        (
            "no-aggregation-scope",
            json!({ "aggregationConfiguration": [] }),
        ),
        (
            "other-aggregation-scope",
            json!({
                "aggregationConfiguration": [{
                    "accountId": engine::emulator_account_id(),
                    "regions": ["ap-southeast-2"],
                    "accessRoleArn": format!(
                        "arn:aws:iam::{}:role/AnotherAgentProfileAccess",
                        engine::emulator_account_id()
                    ),
                }],
            }),
        ),
        (
            "other-tags",
            json!({ "tags": [{ "key": "team", "value": "platform" }] }),
        ),
        ("tags-dropped", json!({ "tags": null })),
    ];

    for (case, overrides) in rows {
        let request = with_overrides(original.clone(), &overrides);
        let refused = create_profile(&client, request).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceType"),
            AGENT_PROFILE_RESOURCE_TYPE,
            "case {case:?}: ResourceType must name the resource the token created, \
             got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceId"),
            arn,
            "case {case:?}: ResourceId must be the profile the token already created, \
             got {refused:?}"
        );
        assert!(
            member(&refused, "Message").contains(token),
            "case {case:?}: the refusal must name the token that was resubmitted, \
             got {refused:?}"
        );

        // A refused create records nothing: the store still holds exactly the one profile
        // the original create made, and it reports what it did.
        assert_eq!(
            stored_profiles(&client),
            1,
            "case {case:?}: a refused create must record no profile"
        );
        let unchanged = get_profile(&client, &arn).await;
        assert_eq!(
            unchanged.status, 200,
            "case {case:?}: the original profile must still resolve, got {unchanged:?}"
        );
        assert_profile_projection(
            &unchanged.body,
            &original,
            case,
            "GetAgentProfile",
            true,
            true,
        );
    }

    // And the token still replays the original profile afterwards: a refused resubmission
    // does not consume or re-point it.
    let replay = create_profile(&client, original).await;
    assert_eq!(
        replay.body, first.body,
        "case \"replay-after-the-conflicts\": the token must still answer the original \
         profile, got {replay:?}"
    );
}

/// Rule 2: the agent-profile quota. Checked AT the ceiling, which is accepted, and one
/// past it, which is the modelled 402.
#[tokio::test]
async fn the_agent_profile_quota_bounds_the_create_and_the_refusal_records_nothing() {
    // The ceiling the emulator meters agent profiles against, read from the constant the
    // handler reads: what this test pins is the BEHAVIOUR at and past the ceiling rather
    // than the number itself.
    const QUOTA: usize = MAX_AGENT_PROFILES;

    let client = Client::new();

    // The profiles below the ceiling go in through the shared state rather than a
    // thousand HTTP creates; everything the test asserts still crosses the boundary.
    let seeded: Vec<String> = {
        let state = client.store();
        let mut store = state.lock();
        (0..QUOTA - 1)
            .map(|index| {
                let arn = store.mint_agent_profile_arn(SIGNED_REGION);
                store.agent_profiles.insert(
                    arn.clone(),
                    AgentProfileRecord {
                        arn: arn.clone(),
                        name: format!("seeded-{index:06}"),
                        display_name: None,
                        description: None,
                        business_overview: None,
                        pillars: vec!["SECURITY".to_string()],
                        deletion_protection: false,
                        execution_role_arn: format!(
                            "arn:aws:iam::{}:role/SeededAgentProfileExecution",
                            engine::emulator_account_id()
                        ),
                        aggregation_configuration: Vec::new(),
                        tags: std::collections::BTreeMap::new(),
                        created_by: engine::emulator_account_id().to_string(),
                        created_at: SystemTime::now(),
                        last_modified_by: None,
                        last_modified_at: None,
                    },
                );
                arn
            })
            .collect()
    };
    assert_eq!(
        stored_profiles(&client),
        QUOTA - 1,
        "case \"below-the-quota\": the store must start one profile short of the ceiling"
    );

    // The create AT the ceiling is accepted.
    let at_the_ceiling = create_profile(
        &client,
        profile_body("profile-at-the-quota", &["SECURITY"], 1),
    )
    .await;
    let last_arn = created_arn(&at_the_ceiling, "at-the-quota");
    assert_eq!(
        stored_profiles(&client),
        QUOTA,
        "case \"at-the-quota\": the create at the ceiling must be recorded"
    );

    // The one past it is the modelled 402, over rows that differ along what the refused
    // request carries — the refusal is the ceiling, not the request's shape.
    let rows: Vec<(&str, Value)> = vec![
        (
            "past-the-quota-minimal",
            profile_body("profile-past-the-quota", &["SECURITY"], 0),
        ),
        (
            "past-the-quota-every-pillar",
            with_overrides(
                profile_body("profile-past-the-quota-again", &PILLARS, 2),
                &json!({ "clientToken": "agent-profile-token-past-the-quota" }),
            ),
        ),
    ];
    for (case, request) in rows {
        let past = create_profile(&client, request).await;
        assert_eq!(
            past.status, 402,
            "case {case:?}: expected HTTP 402, got {past:?}"
        );
        assert_eq!(
            past.error_code, "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {past:?}"
        );
        assert_eq!(
            member(&past, "ResourceType"),
            AGENT_PROFILE_RESOURCE_TYPE,
            "case {case:?}: ResourceType must name what the quota meters, got {past:?}"
        );
        assert!(
            member(&past, "Message").contains(&QUOTA.to_string()),
            "case {case:?}: the refusal must name the ceiling it reached, got {past:?}"
        );
        assert!(
            !member(&past, "QuotaCode").is_empty() && !member(&past, "ServiceCode").is_empty(),
            "case {case:?}: QuotaCode and ServiceCode are @required, got {past:?}"
        );
        assert_eq!(
            stored_profiles(&client),
            QUOTA,
            "case {case:?}: a refused create must record nothing"
        );
    }

    // The refusal is the quota and not a broken store: the create at the ceiling and a
    // profile from below it both still resolve.
    for (case, arn) in [
        ("at-the-quota", &last_arn),
        (
            "below-the-quota",
            seeded.first().expect("the quota is more than one profile"),
        ),
    ] {
        let fetched = get_profile(&client, arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: {arn:?} must still resolve, got {fetched:?}"
        );
    }
}
