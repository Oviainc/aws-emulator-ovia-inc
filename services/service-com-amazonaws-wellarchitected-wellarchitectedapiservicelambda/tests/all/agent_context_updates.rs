// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAgentContext`: the operation that changes a stored
//! context of an agent profile — "Updates an existing context associated with a profile"
//! (`smithy_query(query="operation", operation="UpdateAgentContext")`) — read back
//! through the `GetAgentContext` beside it.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. An update MERGES at the level of its input members: every member the request
//!    carries replaces the stored one, every member it leaves out keeps the value the
//!    context already had, and the context's identity (`id`, `profileArn`, `contextType`,
//!    `createdBy`, `createdAt`) is untouched. The answer IS the stored record, so
//!    `GetAgentContext` reports the identical summary.
//! 2. `content` is one typed structure, so a request that carries it replaces the WHOLE
//!    content: a member the stored content had and the new one omits is gone, and the
//!    derived `applicationType` / `criticality` follow the content member for member.
//! 3. A successful update records WHEN and BY WHOM: `lastModifiedAt` is the instant of the
//!    call and `lastModifiedBy` the account the request is attributed to, and successive
//!    updates compose on the STORED context rather than on the created one.
//! 4. The `(profileArn, id)` pair is RESOLVED: a well-formed `profileArn` naming no stored
//!    profile, a well-formed `id` naming no context of it, and the id of a context
//!    belonging to ANOTHER profile are each `404 ResourceNotFoundException` naming the
//!    identifier as sent — and change nothing.
//! 5. A `clientToken` makes the update idempotent: the same token with the same parameters
//!    answers the context as the original update left it, without stamping the
//!    modification members again; a fresh token applies the next update.
//! 6. Every clause the model puts on an updatable member bounds the update exactly as it
//!    bounds the create: `title`'s `@length(1, 1000)` is served on each bound and refused
//!    one past it, and `content.applicationType` / `content.criticality` outside their
//!    published enum sets are refused — each `400 ValidationException` naming the member
//!    and leaving the stored context as it was.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The contexts these updates change are recorded through the
//! `CreateAgentContext` helper of `super::agent_contexts` — a context is created by exactly
//! one operation, so the request builders live there and this suite imports them; the
//! profile they hang off is minted by `super::agent_goals::profile_arn_of`.

use serde_json::{json, Value};

use super::agent_contexts::{assert_create_instant, create_context, get_context, id_of, summary};
use super::agent_goals::absent_profile_arn;
use super::agent_goals::profile_arn_of;
use super::workload_harness::{assert_not_found, encode_path_segment, member, Answer, Client};

/// `ResourceNotFoundException$ResourceType` for a context and for the agent profile it
/// hangs off, as this service words each (`ExceptionResourceType` is a free-form string —
/// `smithy_query(query="shape", shape="ExceptionResourceType")`).
const CONTEXT_RESOURCE_TYPE: &str = "CONTEXT";
const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// Every value the `ContextType` enum publishes
/// (`smithy_query(query="shape", shape="ContextType")`). Spelled out here so the suite
/// pins the catalog rather than reading it back out of the code under test.
const CONTEXT_TYPES: [&str; 1] = ["APPLICATION"];

/// Every value the `ApplicationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="ApplicationType")`).
const APPLICATION_TYPES: [&str; 3] = ["SAS", "DESKTOP_APPLICATION", "OTHER"];

/// Every value the `Criticality` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="Criticality")`).
const CRITICALITIES: [&str; 4] = [
    "MISSION_CRITICAL",
    "BUSINESS_CRITICAL",
    "NON_CRITICAL",
    "TEST_DEVELOPMENT",
];

/// The published bounds of `UpdateAgentContextRequest$title`'s `SensitiveString`
/// (`@length(min: 1, max: 1000)` —
/// `smithy_query(query="operation", operation="UpdateAgentContext")`).
const TITLE_MIN: usize = 1;
const TITLE_MAX: usize = 1000;

/// `PUT /api/v1/agent-profiles/{profileArn}/contexts/{id}` — the modelled route of
/// `UpdateAgentContext`. The ARN is a non-greedy `@httpLabel`, so it travels
/// percent-encoded in one path segment, exactly as an SDK sends it.
///
/// `pub(crate)` because a context is changed by exactly ONE operation, so the suite about
/// the update that supplies no member (`super::agent_no_op_updates`) sends its requests
/// through this builder rather than building the same request again.
pub(crate) async fn update_context(
    client: &Client,
    profile_arn: &str,
    id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/api/v1/agent-profiles/{}/contexts/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            Some(body),
        )
        .await
}

/// The `context` an accepted answer carries, with the status asserted first so a refusal
/// is reported as itself rather than as a missing member.
fn accepted_context(answer: &Answer, case: &str, operation: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{operation}: expected 200, got {answer:?}"
    );
    summary(answer).clone()
}

/// The body every row of this suite starts its context from: a content carrying members of
/// every kind `ContextContent` holds — lists, free-form strings and both enum-typed
/// members — so a row that omits one is distinguishable from a row that sends it.
fn base_body() -> Value {
    json!({
        "title": "the stored title",
        "contextType": CONTEXT_TYPES[0],
        "content": {
            "accountIds": [engine::emulator_account_id()],
            "regions": ["us-east-1", "eu-west-1"],
            "awsServices": ["lambda", "s3"],
            "resourceTypes": ["AWS::S3::Bucket"],
            "resourceTags": [{ "key": "env", "value": "prod" }],
            "applicationOverview": "the stored overview",
            "industry": "Retail",
            "architectureOverview": "the stored architecture",
            "additionalContext": "the stored notes",
            "applicationType": APPLICATION_TYPES[0],
            "criticality": CRITICALITIES[0],
        },
    })
}

/// A context of `profile_arn` carrying [`base_body`], with the summary its create answered.
async fn stored_context(client: &Client, profile_arn: &str, case: &str) -> (String, Value) {
    let created = create_context(client, profile_arn, base_body()).await;
    let context = accepted_context(&created, case, "CreateAgentContext");
    let id = id_of(&context);
    assert!(
        !id.is_empty(),
        "case {case:?}: the created context must carry its @required id, got {context:?}"
    );
    (id, context)
}

/// Every stored context, grouped under the profile that holds it: its id and its whole
/// stored record, read straight off the shared state rather than through an operation.
/// Rule 4 and rule 6 compare this before and after a refused update.
fn stored_contexts_by_profile(client: &Client) -> Vec<(String, Vec<(String, String)>)> {
    let state = client.store();
    let store = state.lock();
    let mut stored: Vec<(String, Vec<(String, String)>)> = store
        .agent_contexts
        .iter()
        .map(|(profile_arn, contexts)| {
            (
                profile_arn.clone(),
                contexts
                    .iter()
                    .map(|(id, record)| (id.clone(), format!("{record:?}")))
                    .collect(),
            )
        })
        .collect();
    stored.sort();
    stored
}

/// Rules 1, 2 and 3: each row sends a different SUBSET of the updatable members, and the
/// context that comes back carries the sent members as sent, the omitted members as
/// stored, its identity unchanged, and the modification members of this call.
#[tokio::test]
async fn an_update_merges_the_members_it_carries_and_leaves_the_rest() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-host", 1).await;

    // The rows differ along WHICH members the request carries (one, the other, both),
    // along the title's length at each published `@length(1, 1000)` bound, and along the
    // shape of the content: empty, one member, every list at several entries, and enum
    // values the issue did not cite. A request that carries NEITHER writable member asks
    // for no change, so it stamps no modification and is pinned by
    // `super::agent_no_op_updates` instead.
    let rows: Vec<(&str, Value)> = vec![
        ("title-only", json!({ "title": "a retitled context" })),
        ("shortest-title", json!({ "title": "x".repeat(TITLE_MIN) })),
        ("longest-title", json!({ "title": "t".repeat(TITLE_MAX) })),
        ("empty-content", json!({ "content": {} })),
        (
            "content-one-member",
            json!({ "content": { "industry": "Healthcare" } }),
        ),
        (
            "content-every-list-several-entries",
            json!({
                "content": {
                    "accountIds": [engine::emulator_account_id(), "210987654321"],
                    "regions": ["ap-southeast-2", "sa-east-1", "us-west-2"],
                    "awsServices": ["dynamodb", "ec2", "rds"],
                    "resourceTypes": ["AWS::EC2::Instance", "AWS::RDS::DBInstance"],
                    "resourceTags": [
                        { "key": "env", "value": "staging" },
                        { "key": "team", "value": "" },
                    ],
                },
            }),
        ),
        (
            "content-enum-values-the-issue-did-not-cite",
            json!({
                "content": {
                    "applicationType": APPLICATION_TYPES[2],
                    "criticality": CRITICALITIES[2],
                },
            }),
        ),
        (
            "title-and-content",
            json!({
                "title": "both members at once",
                "content": {
                    "applicationOverview": "o".repeat(10_000),
                    "applicationType": APPLICATION_TYPES[1],
                    "criticality": CRITICALITIES[3],
                },
            }),
        ),
        (
            "the-stored-values-again",
            json!({
                "title": "the stored title",
                "content": base_body()["content"].clone(),
            }),
        ),
    ];

    for (case, patch) in rows {
        let (id, created) = stored_context(&client, &profile_arn, case).await;

        let answer = update_context(&client, &profile_arn, &id, patch.clone()).await;
        let updated = accepted_context(&answer, case, "UpdateAgentContext");

        // A member the request carried reads back as sent; one it omitted reads back as
        // the create stored it. `content` is replaced WHOLE, so a stored content member
        // the patch's content omits is gone rather than merged.
        for name in ["title", "content"] {
            let expected = patch.get(name).or_else(|| created.get(name));
            assert_eq!(
                updated.get(name),
                expected,
                "case {case:?}: expected {name} {expected:?}, got {updated:?}"
            );
        }
        // `applicationType` and `criticality` describe the application the CONTENT
        // describes, so they follow whichever content now stands — including by being
        // absent when that content names neither.
        let effective_content = patch.get("content").or_else(|| created.get("content"));
        for name in ["applicationType", "criticality"] {
            let expected = effective_content.and_then(|content| content.get(name));
            assert_eq!(
                updated.get(name).filter(|value| !value.is_null()),
                expected,
                "case {case:?}: {name} must agree with the content that now stands \
                 ({expected:?}), got {updated:?}"
            );
        }
        // The context's identity is not updatable: the input models none of these members.
        for name in ["id", "profileArn", "contextType", "createdBy", "createdAt"] {
            assert_eq!(
                updated.get(name),
                created.get(name),
                "case {case:?}: {name} must be untouched by an update, got {updated:?}"
            );
        }
        assert_eq!(
            updated.get("lastModifiedBy").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: lastModifiedBy must be the account the request is attributed \
             to, got {updated:?}"
        );
        assert_create_instant(updated.get("lastModifiedAt"), case, "lastModifiedAt");

        // The answer is the STORED record, so the reader reports the identical summary.
        let fetched = get_context(&client, &profile_arn, &id).await;
        let read_back = accepted_context(&fetched, case, "GetAgentContext");
        assert_eq!(
            read_back, updated,
            "case {case:?}: GetAgentContext must report the updated context, got {fetched:?}"
        );
    }
}

/// Rule 2, stated as the invariant it is: whatever content a request carries, the stored
/// content afterwards is EXACTLY that content — no member of the content it replaced
/// survives. The rows walk a content down from every member to none and back up, so each
/// step both drops and adds members.
#[tokio::test]
async fn a_content_an_update_carries_replaces_the_stored_content_whole() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-content", 1).await;
    let (id, _created) = stored_context(&client, &profile_arn, "content-replacement").await;

    let contents: Vec<(&str, Value)> = vec![
        (
            "drop-to-two-members",
            json!({ "regions": ["us-west-1"], "criticality": CRITICALITIES[1] }),
        ),
        ("drop-to-none", json!({})),
        (
            "add-one-member-back",
            json!({ "applicationType": APPLICATION_TYPES[1] }),
        ),
        ("the-whole-stored-content", base_body()["content"].clone()),
        (
            "swap-every-member-for-another",
            json!({
                "accountIds": ["210987654321"],
                "awsServices": ["sqs"],
                "industry": "Manufacturing",
                "applicationType": APPLICATION_TYPES[2],
                "criticality": CRITICALITIES[3],
            }),
        ),
    ];

    for (case, content) in contents {
        let answer =
            update_context(&client, &profile_arn, &id, json!({ "content": content })).await;
        let updated = accepted_context(&answer, case, "UpdateAgentContext");
        assert_eq!(
            updated.get("content"),
            Some(&content),
            "case {case:?}: the stored content must be exactly the content sent, \
             got {updated:?}"
        );
        let fetched = get_context(&client, &profile_arn, &id).await;
        let read_back = accepted_context(&fetched, case, "GetAgentContext");
        assert_eq!(
            read_back.get("content"),
            Some(&content),
            "case {case:?}: the read must report the content the update left, \
             got {fetched:?}"
        );
        // A context carries ONE title, and this request named none, so the title the
        // earlier rows left stands rather than being cleared with the content.
        assert!(
            read_back
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| !title.is_empty()),
            "case {case:?}: a content-only update must leave the title in place, \
             got {fetched:?}"
        );
    }
}

/// Rule 3, for composition: a second update merges onto what the FIRST one left, not onto
/// what the create stored, and never moves the modification instant backwards. The rows
/// are the order the two members are updated in, so neither order is privileged.
#[tokio::test]
async fn successive_updates_compose_on_the_stored_context() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-sequence", 1).await;

    let orders: [(&str, [(&str, Value); 2]); 2] = [
        (
            "title-then-content",
            [
                ("title", json!({ "title": "retitled before the content" })),
                (
                    "content",
                    json!({ "content": { "criticality": CRITICALITIES[2] } }),
                ),
            ],
        ),
        (
            "content-then-title",
            [
                (
                    "content",
                    json!({ "content": { "regions": ["ca-central-1"] } }),
                ),
                ("title", json!({ "title": "retitled after the content" })),
            ],
        ),
    ];

    for (case, steps) in orders {
        let (id, created) = stored_context(&client, &profile_arn, case).await;
        let mut expected = created.clone();
        let mut previous_instant = 0.0_f64;

        for (step, patch) in steps {
            let answer = update_context(&client, &profile_arn, &id, patch.clone()).await;
            let updated = accepted_context(&answer, case, "UpdateAgentContext");
            for name in ["title", "content"] {
                if let Some(value) = patch.get(name) {
                    expected[name] = value.clone();
                }
                assert_eq!(
                    updated.get(name),
                    expected.get(name),
                    "case {case:?}/{step}: expected {name} {:?}, got {updated:?}",
                    expected.get(name)
                );
            }
            let instant = updated
                .get("lastModifiedAt")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            assert!(
                instant >= previous_instant,
                "case {case:?}/{step}: lastModifiedAt must not move backwards \
                 (was {previous_instant}), got {instant}"
            );
            previous_instant = instant;
        }

        let fetched = get_context(&client, &profile_arn, &id).await;
        let read_back = accepted_context(&fetched, case, "GetAgentContext");
        for name in ["title", "content"] {
            assert_eq!(
                read_back.get(name),
                expected.get(name),
                "case {case:?}: the stored context must hold both updates, got {fetched:?}"
            );
        }
    }
}

/// Rule 4: the pair the update resolves. The rows are the three ways it can fail to
/// resolve — an absent profile, an absent context id (each extreme of the modelled `UUID`
/// value space), and a context of another profile — and each leaves every stored context
/// exactly as it was.
#[tokio::test]
async fn an_update_of_an_unresolvable_context_is_a_not_found_and_writes_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-lookup", 1).await;
    let other_arn = profile_arn_of(&client, "context-update-other", 0).await;
    let (id, created) = stored_context(&client, &profile_arn, "present").await;
    let (other_id, other_created) = stored_context(&client, &other_arn, "other-profile").await;

    let patch = json!({ "title": "an update that must not land" });
    let before = stored_contexts_by_profile(&client);

    // An absent PROFILE is reported as the missing profile, naming the ARN as sent: the
    // rows differ along the ARN's resource part, Region and partition, so the lookup is
    // exact rather than a match on one field.
    let absent_arns: Vec<(&str, String)> = vec![
        ("absent-hex-resource", absent_profile_arn(&"0".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-7")),
        (
            "another-region",
            format!(
                "arn:aws:wellarchitected:eu-central-1:{}:agent-profile/elsewhere",
                engine::emulator_account_id()
            ),
        ),
        (
            "another-partition",
            format!(
                "arn:aws-us-gov:wellarchitected:us-gov-west-1:{}:agent-profile/elsewhere",
                engine::emulator_account_id()
            ),
        ),
    ];
    for (case, arn) in &absent_arns {
        assert_ne!(
            arn, &profile_arn,
            "case {case:?}: the row must not name the profile that exists"
        );
        let answer = update_context(&client, arn, &id, patch.clone()).await;
        assert_not_found(
            case,
            &answer,
            AGENT_PROFILE_RESOURCE_TYPE,
            arn,
            &format!("agent profile with ARN {arn}"),
        );
    }

    // An absent CONTEXT of a stored profile is reported as the missing context, naming the
    // composite `<profileArn>.<id>` as sent.
    let absent_ids = [
        ("all-zeros", "00000000-0000-0000-0000-000000000000"),
        ("all-f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("mixed-hex", "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d"),
    ];
    for (case, absent_id) in absent_ids {
        assert_ne!(
            absent_id, id,
            "case {case:?}: the row must not name the context that exists"
        );
        let answer = update_context(&client, &profile_arn, absent_id, patch.clone()).await;
        assert_not_found(
            case,
            &answer,
            CONTEXT_RESOURCE_TYPE,
            &format!("{profile_arn}.{absent_id}"),
            &format!(
                "context with ID {absent_id} in an agent profile with ARN {profile_arn}"
            ),
        );
    }

    // A context is reached only THROUGH the profile that holds it, in either direction.
    for (case, arn, context_id) in [
        ("other-profiles-context", &profile_arn, &other_id),
        ("this-profiles-context", &other_arn, &id),
    ] {
        let answer = update_context(&client, arn, context_id, patch.clone()).await;
        assert_not_found(
            case,
            &answer,
            CONTEXT_RESOURCE_TYPE,
            &format!("{arn}.{context_id}"),
            &format!("context with ID {context_id} in an agent profile with ARN {arn}"),
        );
    }

    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"unresolvable-pairs\": a refused update records, changes and removes nothing"
    );
    // Both stored contexts still report the summary their create answered, modification
    // members and all.
    for (case, arn, context_id, expected) in [
        ("present", &profile_arn, &id, &created),
        ("other-profile", &other_arn, &other_id, &other_created),
    ] {
        let fetched = get_context(&client, arn, context_id).await;
        assert_eq!(
            fetched.body.get("context"),
            Some(expected),
            "case {case:?}: a refused update must leave the context untouched, \
             got {fetched:?}"
        );
    }
}

/// Rule 5: `clientToken` replay. The rows are what a token can be — replayed with the same
/// parameters, sent without a token, replayed with different ones, and fresh — each checked
/// against what the store then reports.
#[tokio::test]
async fn a_client_token_makes_the_context_update_idempotent() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-token", 1).await;
    let (id, _created) = stored_context(&client, &profile_arn, "token").await;

    let original = json!({
        "title": "the first update",
        "content": { "criticality": CRITICALITIES[0] },
        "clientToken": "context-update-token-1",
    });
    let first = update_context(&client, &profile_arn, &id, original.clone()).await;
    let after_first = accepted_context(&first, "same-token-same-parameters", "UpdateAgentContext");

    // A retry of the SAME request acts a second time on nothing: the modification members
    // are the first update's, not this call's.
    let replay = update_context(&client, &profile_arn, &id, original.clone()).await;
    let replayed = accepted_context(&replay, "same-token-same-parameters", "UpdateAgentContext");
    assert_eq!(
        replayed, after_first,
        "case \"same-token-same-parameters\": the replay must answer the context the \
         original update left, got {replay:?}"
    );
    assert_eq!(
        client.store().lock().update_agent_context_tokens.len(),
        1,
        "case \"same-token-same-parameters\": a replay remembers no second token"
    );

    // The SAME body WITHOUT the token is not a replay, so it is applied: the modification
    // instant moves, which is exactly what the replay above must not do.
    let mut untokened = original.clone();
    untokened
        .as_object_mut()
        .expect("the request body is an object")
        .remove("clientToken");
    let repeated = update_context(&client, &profile_arn, &id, untokened).await;
    let after_repeat = accepted_context(&repeated, "no-token-same-parameters", "UpdateAgentContext");
    let replay_instant = replayed
        .get("lastModifiedAt")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let repeat_instant = after_repeat
        .get("lastModifiedAt")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        repeat_instant >= replay_instant,
        "case \"no-token-same-parameters\": an untokened repeat is applied, so its \
         lastModifiedAt is not before the replay's ({replay_instant}), got {repeat_instant}"
    );
    assert_eq!(
        client.store().lock().update_agent_context_tokens.len(),
        1,
        "case \"no-token-same-parameters\": a request carrying no token remembers none"
    );

    // The same token carrying DIFFERENT parameters is a different request, and this
    // operation models no conflict for one, so it is served on its own merits.
    let mut changed = original.clone();
    changed["title"] = json!("the second update");
    let second = update_context(&client, &profile_arn, &id, changed).await;
    let after_second = accepted_context(&second, "same-token-other-parameters", "UpdateAgentContext");
    assert_eq!(
        after_second.get("title").and_then(Value::as_str),
        Some("the second update"),
        "case \"same-token-other-parameters\": the update must land, got {second:?}"
    );

    // A fresh token applies the next update, and the one before it is what it merges onto.
    let third = update_context(
        &client,
        &profile_arn,
        &id,
        json!({
            "content": { "industry": "Education" },
            "clientToken": "context-update-token-2",
        }),
    )
    .await;
    let after_third = accepted_context(&third, "fresh-token", "UpdateAgentContext");
    assert_eq!(
        after_third.get("title").and_then(Value::as_str),
        Some("the second update"),
        "case \"fresh-token\": the merge is onto the stored context, got {third:?}"
    );
    assert_eq!(
        after_third.get("content"),
        Some(&json!({ "industry": "Education" })),
        "case \"fresh-token\": the update must land, got {third:?}"
    );
    assert_eq!(
        client.store().lock().update_agent_context_tokens.len(),
        2,
        "case \"token-rows\": exactly the two tokens the rows spent are remembered"
    );
}

/// Rule 6: every clause the model puts on an updatable member bounds the update too. The
/// rows break one clause each — one PAST a published bound where the bound is a range, and
/// a value outside the published set where the member is an enum — and each leaves the
/// stored context as it was.
#[tokio::test]
async fn a_member_outside_its_modelled_clause_is_refused_and_the_context_is_untouched() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-update-validation", 1).await;
    let (id, created) = stored_context(&client, &profile_arn, "validation").await;

    // The second column is the text the refusal must state: the member the value was sent
    // for, except for a value outside an enumerated type's set, which is reported as the
    // schema finding that quotes the value and the set and names no member.
    let rows: Vec<(&str, String, Value)> = vec![
        ("empty-title", "title".to_string(), json!({ "title": "" })),
        (
            "title-past-the-bound",
            "title".to_string(),
            json!({ "title": "t".repeat(TITLE_MAX + 1) }),
        ),
        (
            "application-type-outside-the-enum",
            r#"instance value ("MAINFRAME") not found in enum"#.to_string(),
            json!({ "content": { "applicationType": "MAINFRAME" } }),
        ),
        (
            "criticality-outside-the-enum",
            r#"instance value ("SOMEWHAT_CRITICAL") not found in enum"#.to_string(),
            json!({ "content": { "criticality": "SOMEWHAT_CRITICAL" } }),
        ),
        (
            "lowercased-published-application-type",
            format!(
                "instance value (\"{}\") not found in enum",
                APPLICATION_TYPES[0].to_lowercase()
            ),
            json!({ "content": { "applicationType": APPLICATION_TYPES[0].to_lowercase() } }),
        ),
    ];

    let before = stored_contexts_by_profile(&client);
    for (case, expected, patch) in rows {
        let answer = update_context(&client, &profile_arn, &id, patch).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains(&expected),
            "case {case:?}: the refusal must state {expected:?}, got {answer:?}"
        );

        let fetched = get_context(&client, &profile_arn, &id).await;
        assert_eq!(
            fetched.body.get("context"),
            Some(&created),
            "case {case:?}: a refused update must leave the context untouched, \
             got {fetched:?}"
        );
    }
    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"refused-rows\": a refused update records, changes and removes nothing"
    );
}
