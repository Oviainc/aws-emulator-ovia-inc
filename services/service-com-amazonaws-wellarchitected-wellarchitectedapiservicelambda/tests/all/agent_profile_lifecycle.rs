// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteAgentProfile` — the removal half of the agent-profile
//! lifecycle whose create and read are pinned by `super::agent_goals`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `DeleteAgentProfile` removes the profile its `profileArn` resolves to, whatever
//!    that profile carries: afterwards the ARN resolves to nothing, so `GetAgentProfile`
//!    and a repeat `DeleteAgentProfile` both answer `404 ResourceNotFoundException`, the
//!    store no longer holds it, and every OTHER stored profile still reports exactly what
//!    it did before.
//! 2. "Deletes an optimization profile and its associated configuration": the goals and
//!    contexts recorded against the profile go with it — each answers 404 afterwards —
//!    while another profile's goals and contexts are untouched.
//! 3. A profile whose stored `deletionProtection` is enabled is NOT removed: the delete is
//!    refused `409 ConflictException` naming the ARN as sent and the member that has to
//!    change, and the profile stays exactly as it was. The flag is read from the STORE, so
//!    a create that omitted the member takes the modelled `@default(true)` and its profile
//!    is protected too.
//! 4. An `AgentProfileArn` is RESOLVED against the store: one naming no stored profile is
//!    a `404 ResourceNotFoundException` carrying the ARN as sent and the missing
//!    resource's type — including the ARN of a profile this suite deleted.
//! 5. A delete also drops the create tokens that resolved to the profile, so a
//!    `clientToken` whose profile is gone has no original result to replay and creates
//!    afresh.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{encode_path_segment, Answer, Client};

/// `CreateAgentProfile`: `POST /api/v1/agent-profiles`
/// (`smithy_query(query="operation", operation="CreateAgentProfile")`).
const PROFILES_PATH: &str = "/api/v1/agent-profiles";

/// Every value the `Pillar` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="Pillar")`). `Pillars` is `@length(1, 5)` with
/// `uniqueItems`, so this is also the longest list a profile may carry.
const PILLARS: [&str; 5] = [
    "COST_OPTIMIZATION",
    "SECURITY",
    "RESILIENCE",
    "PERFORMANCE",
    "OPERATIONAL_EXCELLENCE",
];

/// The Region the harness signs every request for, which is the Region a minted
/// `AgentProfileArn` carries.
const REQUEST_REGION: &str = "us-west-2";

/// A `RoleArn` the modelled pattern accepts
/// (`^arn:([a-z\-]+):iam::\d{12}:role/(service-role/)?[a-zA-Z0-9+=,.@\-_]+$`), in the
/// emulator's one account.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One `AggregationConfiguration` — "which Amazon Web Services accounts and Regions to
/// analyze" (`smithy_query(query="shape", shape="AggregationConfiguration")`).
fn aggregation(account_id: &str, regions: &[&str], role: &str) -> Value {
    json!({
        "accountId": account_id,
        "regions": regions,
        "accessRoleArn": role_arn(role),
    })
}

/// A minimal valid `CreateAgentProfile` body: the modelled `@required` members, with
/// `deletionProtection` cleared so the profile can be deleted. A caller overrides the
/// members its case varies.
fn minimal_profile(name: &str) -> Value {
    json!({
        "name": name,
        "pillars": ["SECURITY"],
        "executionRoleArn": role_arn("AgentProfileExecution"),
        "aggregationConfiguration": [
            aggregation(engine::emulator_account_id(), &["us-east-1"], "AgentProfileAccess")
        ],
        "deletionProtection": false,
    })
}

/// `request` with every member of `overrides` applied, so a row states only what it
/// varies.
fn with_overrides(mut request: Value, overrides: &Value) -> Value {
    for (name, value) in overrides.as_object().expect("overrides are an object") {
        request[name] = value.clone();
    }
    request
}

async fn create_profile(client: &Client, body: Value) -> Answer {
    client.send("POST", PROFILES_PATH, Some(body)).await
}

/// `GET /api/v1/agent-profiles/{profileArn}`: the ARN is a non-greedy `@httpLabel`, so it
/// travels percent-encoded in one path segment, exactly as an SDK sends it.
async fn get_profile(client: &Client, profile_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!("{PROFILES_PATH}/{}", encode_path_segment(profile_arn)),
            None,
        )
        .await
}

pub(crate) async fn delete_profile(client: &Client, profile_arn: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!("{PROFILES_PATH}/{}", encode_path_segment(profile_arn)),
            None,
        )
        .await
}

async fn create_goal(client: &Client, profile_arn: &str, title: &str) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "{PROFILES_PATH}/{}/goals",
                encode_path_segment(profile_arn)
            ),
            Some(json!({ "pillars": ["SECURITY"], "title": title })),
        )
        .await
}

async fn get_goal(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "{PROFILES_PATH}/{}/goals/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

/// `CreateAgentContext`: `POST /api/v1/agent-profiles/{profileArn}/contexts`. `title` and
/// `contextType` are the modelled `@required` members; `APPLICATION` is the one value
/// `ContextType` publishes (`smithy_query(query="shape", shape="ContextType")`).
async fn create_context(client: &Client, profile_arn: &str, title: &str) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "{PROFILES_PATH}/{}/contexts",
                encode_path_segment(profile_arn)
            ),
            Some(json!({ "title": title, "contextType": "APPLICATION", "content": {} })),
        )
        .await
}

async fn get_context(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "{PROFILES_PATH}/{}/contexts/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A well-formed `AgentProfileArn` (the modelled
/// `^arn:aws([a-z0-9-]+)?:wellarchitected:[a-z0-9-]{6,64}:\d{12}:agent-profile/([a-zA-Z0-9_-]+)$`)
/// for `resource` in `region`.
fn profile_arn(region: &str, resource: &str) -> String {
    format!(
        "arn:aws:wellarchitected:{region}:{}:agent-profile/{resource}",
        engine::emulator_account_id()
    )
}

/// The not-found `Message` this service builds for an agent-profile lookup: the resource
/// asked for plus the account it was looked for in. The account comes from the emulator's
/// one source of truth, never a literal.
fn not_found_message(arn: &str) -> String {
    format!(
        "[ResourceNotFound] No agent profile with ARN {arn} was found in account {}.",
        engine::emulator_account_id()
    )
}

/// The `arn` a successful `CreateAgentProfile` answered with, checked against the modelled
/// `AgentProfileArn` pattern and its `@length(max: 2048)`.
fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentProfile 200, got {answer:?}"
    );
    let arn = member(answer, "arn").to_string();
    let expected_prefix = profile_arn(REQUEST_REGION, "");
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

/// The goal or context id a create answered with, read from the member the operation
/// names it (`goal` / `context`), with the create itself asserted.
fn created_child_id(answer: &Answer, member: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the {member} create to answer 200, got {answer:?}"
    );
    answer
        .body
        .get(member)
        .and_then(|child| child.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert that `arn` resolves to nothing: both the read and the delete answer the
/// modelled 404 naming the ARN as sent and the missing resource's type.
async fn assert_resolves_to_nothing(client: &Client, arn: &str, case: &str) {
    for (operation, answer) in [
        ("GetAgentProfile", get_profile(client, arn).await),
        ("DeleteAgentProfile", delete_profile(client, arn).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}/{operation}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "AGENT_PROFILE",
            "case {case:?}/{operation}: expected ResourceType AGENT_PROFILE, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            arn,
            "case {case:?}/{operation}: ResourceId must be the ARN as sent, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            not_found_message(arn),
            "case {case:?}/{operation}: expected the ARN/account not-found message, got {answer:?}"
        );
    }
}

/// Every row of the profile table: the case name and the overrides it applies to
/// [`minimal_profile`]. The rows differ along what a profile CARRIES — how many pillars,
/// how many accounts and Regions are in scope, which optional text members are present,
/// whether it is tagged — because what the delete removes is the profile the ARN resolves
/// to, not a request of one particular shape.
fn profile_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("minimal", json!({})),
        (
            "every-optional-member",
            json!({
                "displayName": "Agent profile under test",
                "description": "a profile that carries every optional member",
                "businessOverview": "a business overview",
                "tags": [{ "key": "Env", "value": "test" }, { "key": "Team", "value": "core" }],
            }),
        ),
        // `Pillars` is `@length(1, 5)`: one pillar is the published minimum, the whole
        // `Pillar` value set the maximum.
        ("one-pillar", json!({ "pillars": ["OPERATIONAL_EXCELLENCE"] })),
        ("every-pillar", json!({ "pillars": PILLARS })),
        // `AggregationConfigurations` is `@length(0, 100)`, so a profile may name no
        // scope at all, one account, or several.
        (
            "no-aggregation-configuration",
            json!({ "aggregationConfiguration": [] }),
        ),
        (
            "two-aggregation-configurations",
            json!({
                "aggregationConfiguration": [
                    aggregation("123456789012", &["us-east-1", "eu-west-1"], "AccessOne"),
                    aggregation("210987654321", &["ap-southeast-2"], "AccessTwo"),
                ],
            }),
        ),
        // `name` is `@length(3, 128)`: each published bound.
        ("name-at-the-minimum-length", json!({ "name": "abc" })),
        ("name-at-the-maximum-length", json!({ "name": "n".repeat(128) })),
    ]
}

/// Rules 1 and 4 (the deleted profile's own ARN).
#[tokio::test]
async fn deleting_a_profile_removes_it_and_leaves_every_other_profile_alone() {
    let client = Client::new();

    // A bystander profile, created first and never deleted: every row must leave it
    // reporting exactly what it did, so a delete removes ONE profile rather than
    // disturbing the store.
    let bystander = created_arn(
        &create_profile(&client, minimal_profile("bystander-profile")).await,
        "bystander",
    );
    let bystander_body = get_profile(&client, &bystander).await.body;

    for (case, overrides) in profile_cases() {
        let request = with_overrides(minimal_profile(&format!("doomed-{case}")), &overrides);
        let arn = created_arn(&create_profile(&client, request).await, case);
        assert_eq!(
            get_profile(&client, &arn).await.status,
            200,
            "case {case:?}: the profile under test must be readable before the delete"
        );

        let deleted = delete_profile(&client, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgentProfile 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {case:?}: DeleteAgentProfile's modelled response has no members, got {deleted:?}"
        );

        // The ARN now resolves to nothing, on the read and on a repeat delete alike.
        assert_resolves_to_nothing(&client, &arn, case).await;

        // The store itself no longer holds the profile, so nothing survives the delete
        // behind the wire projection.
        assert!(
            !client.store().lock().agent_profiles.contains_key(&arn),
            "case {case:?}: the store must not hold the deleted profile {arn:?}"
        );

        let still_there = get_profile(&client, &bystander).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: deleting one profile must leave the others readable, got {still_there:?}"
        );
        assert_eq!(
            still_there.body, bystander_body,
            "case {case:?}: the bystander profile must report exactly what it did before"
        );
    }
}

/// Rule 2. The rows differ along HOW MANY goals and contexts the deleted profile holds,
/// because a child resource belongs to its profile however many of them there are.
#[tokio::test]
async fn deleting_a_profile_removes_the_goals_and_contexts_recorded_against_it() {
    let client = Client::new();

    // The profile that survives, with a goal and a context of its own: the deletes below
    // must not touch either.
    let keeper = created_arn(
        &create_profile(&client, minimal_profile("keeper-profile")).await,
        "keeper",
    );
    let keeper_goal = created_child_id(
        &create_goal(&client, &keeper, "Keep this goal").await,
        "goal",
        "keeper",
    );
    let keeper_context = created_child_id(
        &create_context(&client, &keeper, "Keep this context").await,
        "context",
        "keeper",
    );

    for (case, children) in [
        ("no-children", 0usize),
        ("one-of-each", 1),
        ("three-of-each", 3),
    ] {
        let arn = created_arn(
            &create_profile(&client, minimal_profile(&format!("child-host-{case}"))).await,
            case,
        );
        let mut goals: Vec<String> = Vec::new();
        let mut contexts: Vec<String> = Vec::new();
        for index in 0..children {
            let goal = created_child_id(
                &create_goal(&client, &arn, &format!("{case} goal {index}")).await,
                "goal",
                case,
            );
            let context = created_child_id(
                &create_context(&client, &arn, &format!("{case} context {index}")).await,
                "context",
                case,
            );
            assert_eq!(
                get_goal(&client, &arn, &goal).await.status,
                200,
                "case {case:?}: the goal under test must be readable before the delete"
            );
            assert_eq!(
                get_context(&client, &arn, &context).await.status,
                200,
                "case {case:?}: the context under test must be readable before the delete"
            );
            goals.push(goal);
            contexts.push(context);
        }

        assert_eq!(
            delete_profile(&client, &arn).await.status,
            200,
            "case {case:?}: expected DeleteAgentProfile 200"
        );

        // "Deletes an optimization profile and its associated configuration": a goal and a
        // context are reached only through their profile, so neither can outlive it.
        for id in &goals {
            let orphan = get_goal(&client, &arn, id).await;
            assert_eq!(
                orphan.status, 404,
                "case {case:?}: the goal {id:?} of a deleted profile must resolve to nothing, \
                 got {orphan:?}"
            );
        }
        for id in &contexts {
            let orphan = get_context(&client, &arn, id).await;
            assert_eq!(
                orphan.status, 404,
                "case {case:?}: the context {id:?} of a deleted profile must resolve to \
                 nothing, got {orphan:?}"
            );
        }
        {
            let state = client.store();
            let store = state.lock();
            assert!(
                !store.agent_goals.contains_key(&arn),
                "case {case:?}: the store must hold no goals for the deleted profile {arn:?}"
            );
            assert!(
                !store.agent_contexts.contains_key(&arn),
                "case {case:?}: the store must hold no contexts for the deleted profile {arn:?}"
            );
        }

        // The other profile's children are untouched.
        let goal = get_goal(&client, &keeper, &keeper_goal).await;
        assert_eq!(
            goal.status, 200,
            "case {case:?}: another profile's goal must survive the delete, got {goal:?}"
        );
        let context = get_context(&client, &keeper, &keeper_context).await;
        assert_eq!(
            context.status, 200,
            "case {case:?}: another profile's context must survive the delete, got {context:?}"
        );
    }
}

/// Rule 3. The rows differ along HOW the stored flag came to be set — explicitly, or by
/// the modelled `@default(true)` on a request that omitted the member — because the delete
/// reads the stored profile, not the request that created it.
#[tokio::test]
async fn deletion_protection_refuses_the_delete_and_leaves_the_profile_readable() {
    let client = Client::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "deletion-protection-sent-true",
            json!({ "deletionProtection": true }),
        ),
        // `deletionProtection` is `@default(true)`, so a request that omits it creates a
        // PROTECTED profile.
        ("deletion-protection-omitted", json!({})),
    ];

    for (case, overrides) in cases {
        let mut request = with_overrides(minimal_profile(&format!("protected-{case}")), &overrides);
        if case == "deletion-protection-omitted" {
            request
                .as_object_mut()
                .expect("the request is an object")
                .remove("deletionProtection");
        }

        let created = create_profile(&client, request).await;
        let arn = created_arn(&created, case);
        assert_eq!(
            created
                .body
                .get("deletionProtection")
                .and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the created profile must report deletion protection enabled, \
             got {created:?}"
        );
        let before = get_profile(&client, &arn).await.body;

        let refused = delete_profile(&client, &arn).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected the delete of a protected profile to be refused 409, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceType"),
            "AGENT_PROFILE",
            "case {case:?}: expected ResourceType AGENT_PROFILE, got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceId"),
            arn,
            "case {case:?}: ResourceId must be the ARN as sent, got {refused:?}"
        );
        assert!(
            member(&refused, "Message").contains("deletionProtection"),
            "case {case:?}: the refusal must name the member that has to change, got {refused:?}"
        );

        // The refused delete changed nothing.
        let after = get_profile(&client, &arn).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: a refused delete must leave the profile readable, got {after:?}"
        );
        assert_eq!(
            after.body, before,
            "case {case:?}: a refused delete must leave the stored profile unchanged"
        );
        assert!(
            client.store().lock().agent_profiles.contains_key(&arn),
            "case {case:?}: the store must still hold the protected profile {arn:?}"
        );
    }
}

/// Rule 4. The rows differ along the SHAPE of the ARN's resource part and the Region it
/// names — every one satisfying the modelled `AgentProfileArn` pattern, so each reaches
/// the lookup rather than the boundary.
#[tokio::test]
async fn an_unresolvable_profile_arn_is_a_not_found_naming_the_arn_and_the_account() {
    let client = Client::new();

    // One profile exists, so the 404s below are a real miss and not an empty store.
    let present = created_arn(
        &create_profile(&client, minimal_profile("present-profile")).await,
        "present",
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "hex-resource-part",
            profile_arn(REQUEST_REGION, "0123456789abcdef0123456789abcdef"),
        ),
        (
            "name-shaped-resource-part",
            profile_arn(REQUEST_REGION, "no_such-profile"),
        ),
        // The same resource part in another Region names a DIFFERENT profile.
        (
            "another-region",
            profile_arn("eu-central-1", "0123456789abcdef0123456789abcdef"),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, present,
            "case {case:?}: the row must not name the profile that exists"
        );
        assert_resolves_to_nothing(&client, &arn, case).await;
    }

    // The profile that DOES exist still resolves, so the rows above fail on resolution
    // and not on a broken route.
    let fetched = get_profile(&client, &present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored profile must still resolve, got {fetched:?}"
    );
}

/// Rule 5.
#[tokio::test]
async fn a_create_token_whose_profile_was_deleted_creates_afresh() {
    let client = Client::new();

    let original = minimal_profile("idempotent-profile");
    let token = "tok-idempotent";
    let request = with_overrides(original.clone(), &json!({ "clientToken": token }));

    let first = create_profile(&client, request.clone()).await;
    let arn = created_arn(&first, "first");

    // While the profile stands, the token replays it.
    let replay = create_profile(&client, request.clone()).await;
    assert_eq!(
        replay.body, first.body,
        "case \"replay\": a replayed create must answer with the original profile"
    );
    assert_eq!(
        client.store().lock().agent_profiles.len(),
        1,
        "case \"replay\": a replayed create must not store a second profile"
    );

    assert_eq!(
        delete_profile(&client, &arn).await.status,
        200,
        "case \"deleted\": the profile under test must delete cleanly"
    );
    assert!(
        client
            .store()
            .lock()
            .create_agent_profile_tokens
            .get(token)
            .is_none(),
        "case \"deleted\": the delete must drop the create token that resolved to the profile"
    );

    // With no original result left to answer with, the same token creates afresh.
    let recreated = create_profile(&client, request).await;
    let recreated_arn = created_arn(&recreated, "deleted-then-replayed");
    assert_ne!(
        recreated_arn, arn,
        "case \"deleted-then-replayed\": a token whose profile is gone must create a new one"
    );
    assert_eq!(
        get_profile(&client, &recreated_arn).await.status,
        200,
        "case \"deleted-then-replayed\": the recreated profile must be readable"
    );
    assert_resolves_to_nothing(&client, &arn, "deleted-then-replayed").await;
}
