// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteAgentContext`: the operation that removes one context
//! of an agent profile.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `DeleteAgentContext` removes EXACTLY the context its `(profileArn, id)` pair
//!    resolves to: it answers an empty HTTP 200, that context then answers `404
//!    ResourceNotFoundException` to `GetAgentContext` and to a repeat
//!    `DeleteAgentContext`, `ListAgentContexts` stops reporting it, and every context not
//!    yet deleted still reports the summary its create answered.
//! 2. A context is reached only THROUGH the profile it belongs to: the same context id
//!    under another profile's ARN resolves to nothing and removes nothing, so the context
//!    still resolves under its own profile.
//! 3. A well-formed `profileArn` that names no stored profile is reported as the missing
//!    PROFILE, and a well-formed `id` that names no context of a stored profile as the
//!    missing CONTEXT — each `404 ResourceNotFoundException` carrying the identifier as
//!    sent and the missing resource's modelled type. A value the modelled
//!    `AgentProfileArn` / `UUID` pattern refuses never reaches the lookup: the boundary
//!    refuses it as a `400 ValidationException` naming the member, and nothing is removed.
//! 4. Everything keyed to the deleted context goes with it: the `clientToken` that
//!    `CreateAgentContext` spent on it names no earlier request afterwards, so that same
//!    token carrying DIFFERENT parameters is served as a new create rather than refused as
//!    a conflicting retry.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the contexts are
//! recorded and read back through `super::agent_contexts`, and the profile they hang off is
//! minted by `super::agent_goals::profile_arn_of`.

use serde_json::Value;

use super::agent_contexts::{
    assert_summary_matches, context_rows, create_context, get_context, id_of, items, list_contexts,
    summary,
};
use super::agent_goals::{absent_profile_arn, profile_arn_of};
use super::workload_harness::{assert_not_found, encode_path_segment, member, Answer, Client};

/// `DELETE /api/v1/agent-profiles/{profileArn}/contexts/{id}`: both labels are non-greedy
/// `@httpLabel`s, so each travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn delete_context(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/api/v1/agent-profiles/{}/contexts/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

/// How a context of an agent profile is named in a refusal's `ResourceId`: the profile it
/// belongs to and its own id, which together identify it.
fn context_resource_id(profile_arn: &str, id: &str) -> String {
    format!("{profile_arn}.{id}")
}

/// The subject a missing context's `Message` names.
fn context_subject(profile_arn: &str, id: &str) -> String {
    format!("context with ID {id} in an agent profile with ARN {profile_arn}")
}

/// Create one context of `body` under `profile_arn`, asserting the create and answering
/// the id it minted together with the summary it reported.
async fn created_context(
    client: &Client,
    profile_arn: &str,
    body: &Value,
    case: &str,
) -> (String, Value) {
    let answer = create_context(client, profile_arn, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
    );
    let context = summary(&answer).clone();
    assert_summary_matches(&context, body, profile_arn, case);
    (id_of(&context), context)
}

/// The ids `ListAgentContexts` reports for one profile, in the order the page carries
/// them, asserting the whole set arrived on one page (the member's `@default(100)` page is
/// larger than any row count here).
async fn listed_ids(client: &Client, profile_arn: &str, case: &str) -> Vec<String> {
    let answer = list_contexts(client, profile_arn, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListAgentContexts 200, got {answer:?}"
    );
    assert!(
        answer.body.get("nextToken").is_none_or(Value::is_null),
        "case {case:?}: the whole set must arrive on one page, got {answer:?}"
    );
    items(&answer).iter().map(id_of).collect()
}

/// Rule 1: every row round-trips through `CreateAgentContext`, and deleting one row
/// removes exactly that row — it answers 404 afterwards (to the read and to a repeat
/// delete) and leaves the list while every context not yet deleted still reports its
/// original summary.
#[tokio::test]
async fn delete_removes_exactly_the_context_its_pair_resolves_to() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-delete-host", 1).await;

    // Create every row under one profile, remembering the summary each create answered.
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (case, body) in context_rows() {
        let (id, context) = created_context(&client, &profile_arn, &body, case).await;
        created.push((case, id, context));
    }

    let listed = listed_ids(&client, &profile_arn, "every-row-created").await;
    assert_eq!(
        listed.len(),
        created.len(),
        "case \"every-row-created\": the list must report every created context, got {listed:?}"
    );

    // Delete the rows one at a time: each delete takes exactly its own context with it.
    for index in 0..created.len() {
        let (case, id, _) = created[index].clone();
        let deleted = delete_context(&client, &profile_arn, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgentContext 200, got {deleted:?}"
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteAgentContext answers no members, got {deleted:?}"
        );

        let read_back = get_context(&client, &profile_arn, &id).await;
        assert_not_found(
            case,
            &read_back,
            "CONTEXT",
            &context_resource_id(&profile_arn, &id),
            &context_subject(&profile_arn, &id),
        );

        let again = delete_context(&client, &profile_arn, &id).await;
        assert_not_found(
            case,
            &again,
            "CONTEXT",
            &context_resource_id(&profile_arn, &id),
            &context_subject(&profile_arn, &id),
        );

        // The list reports exactly the contexts not yet deleted, and each of them still
        // reads back as the summary its create answered.
        let remaining: Vec<String> = created[index + 1..]
            .iter()
            .map(|(_, other_id, _)| other_id.clone())
            .collect();
        let mut listed = listed_ids(&client, &profile_arn, case).await;
        listed.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: deleting it must leave exactly the other contexts listed, \
             got {listed:?}"
        );
        assert_eq!(
            client.store().lock().agent_context_count(&profile_arn),
            remaining.len(),
            "case {case:?}: only the deleted context is gone from the store"
        );

        for (other_case, other_id, other_context) in &created[index + 1..] {
            let still_there = get_context(&client, &profile_arn, other_id).await;
            assert_eq!(
                still_there.status, 200,
                "case {case:?}: deleting it must leave {other_case:?} in place, \
                 got {still_there:?}"
            );
            assert_eq!(
                summary(&still_there),
                other_context,
                "case {case:?}: {other_case:?} must still report its original summary, \
                 got {still_there:?}"
            );
        }
    }

    let listed = listed_ids(&client, &profile_arn, "all-rows-deleted").await;
    assert!(
        listed.is_empty(),
        "case \"all-rows-deleted\": a profile whose contexts are all deleted lists none, \
         got {listed:?}"
    );
}

/// Rule 2: a context belongs to ONE profile, so the same id under another profile's ARN
/// resolves to nothing and removes nothing.
#[tokio::test]
async fn a_context_is_reached_only_through_its_own_profile() {
    let client = Client::new();
    let first = profile_arn_of(&client, "context-owner-one", 1).await;
    let second = profile_arn_of(&client, "context-owner-two", 1).await;
    assert_ne!(
        first, second,
        "case \"two-profiles\": each CreateAgentProfile mints its own ARN"
    );

    // One context per profile, over rows that differ in which content members they carry.
    let rows = context_rows();
    let mut contexts: Vec<(&str, String, String, Value)> = Vec::new();
    for (row, arn) in [(0usize, &first), (1usize, &second)] {
        let (case, body) = &rows[row];
        let (id, context) = created_context(&client, arn, body, case).await;
        contexts.push((case, arn.clone(), id, context));
    }

    for (case, owner, id, context) in &contexts {
        let other = if owner == &first { &second } else { &first };
        let answer = delete_context(&client, other, id).await;
        assert_not_found(
            case,
            &answer,
            "CONTEXT",
            &context_resource_id(other, id),
            &context_subject(other, id),
        );

        let still_there = get_context(&client, owner, id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: a delete against another profile removes nothing, \
             got {still_there:?}"
        );
        assert_eq!(
            summary(&still_there),
            context,
            "case {case:?}: the context must still report its original summary, \
             got {still_there:?}"
        );
    }
}

/// Rule 3: what an identifier that resolves to nothing is answered, and what the boundary
/// refuses before the lookup runs — over a table of each form.
#[tokio::test]
async fn an_unresolvable_or_malformed_identifier_is_refused_and_removes_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-refusal-host", 1).await;
    let (present_case, present_body) = context_rows()[1].clone();
    let (present, present_context) =
        created_context(&client, &profile_arn, &present_body, present_case).await;

    // Well-formed `AgentProfileArn`s that name no stored profile: the resource segment is
    // what the modelled pattern lets differ, so the rows differ along it.
    let absent_profiles: Vec<(&str, String)> = vec![
        ("plain-name", absent_profile_arn("no-such-profile")),
        ("underscored", absent_profile_arn("no_such_profile")),
        ("digits-only", absent_profile_arn("00000000")),
        ("single-character", absent_profile_arn("p")),
    ];
    for (case, arn) in absent_profiles {
        assert_ne!(
            arn, profile_arn,
            "case {case:?}: the row must not name the profile that exists"
        );
        let answer = delete_context(&client, &arn, &present).await;
        assert_not_found(case, &answer, "AGENT_PROFILE", &arn, &format!("agent profile with ARN {arn}"));
    }

    // Well-formed `UUID`s that name no context of this profile.
    let absent_ids: Vec<(&str, String)> = vec![
        ("all-zero", "00000000-0000-0000-0000-000000000000".to_string()),
        ("all-f", "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string()),
        (
            "mixed-hex",
            "0123abcd-4567-89ef-0123-456789abcdef".to_string(),
        ),
        (
            "version-4-looking",
            "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_string(),
        ),
    ];
    for (case, id) in absent_ids {
        assert_ne!(
            id, present,
            "case {case:?}: the row must not name the context that exists"
        );
        let answer = delete_context(&client, &profile_arn, &id).await;
        assert_not_found(
            case,
            &answer,
            "CONTEXT",
            &context_resource_id(&profile_arn, &id),
            &context_subject(&profile_arn, &id),
        );
    }

    // Values the modelled `AgentProfileArn` pattern refuses: the boundary answers 400
    // naming the member, and the lookup never runs.
    let bad_arns: Vec<(&str, String)> = vec![
        ("not-an-arn", "no-such-profile".to_string()),
        (
            "another-service",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:workload/0123456789abcdef0123456789abcdef",
                engine::emulator_account_id()
            ),
        ),
        (
            "no-region",
            format!(
                "arn:aws:wellarchitected::{}:agent-profile/p",
                engine::emulator_account_id()
            ),
        ),
        (
            "short-account",
            "arn:aws:wellarchitected:us-west-2:1234:agent-profile/p".to_string(),
        ),
    ];
    for (case, arn) in bad_arns {
        let answer = delete_context(&client, &arn, &present).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains("profileArn"),
            "case {case:?}: the refusal must name the member, got {answer:?}"
        );
    }

    // Values the modelled `UUID` pattern refuses.
    let bad_ids: Vec<(&str, String)> = vec![
        ("no-dashes", "0".repeat(32)),
        (
            "uppercase-hex",
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string(),
        ),
        (
            "one-character-short",
            "00000000-0000-0000-0000-00000000000".to_string(),
        ),
        (
            "one-character-long",
            "00000000-0000-0000-0000-0000000000000".to_string(),
        ),
        ("not-hex-at-all", "not-a-context-id".to_string()),
    ];
    for (case, id) in bad_ids {
        let answer = delete_context(&client, &profile_arn, &id).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains("id"),
            "case {case:?}: the refusal must name the member, got {answer:?}"
        );
    }

    // Nothing any of those rows asked for was removed: the stored context is untouched.
    let still_there = get_context(&client, &profile_arn, &present).await;
    assert_eq!(
        summary(&still_there),
        &present_context,
        "case \"refused-rows\": a refused delete removes nothing, got {still_there:?}"
    );
    assert_eq!(
        client.store().lock().agent_context_count(&profile_arn),
        1,
        "case \"refused-rows\": exactly the one created context is stored"
    );
}

/// Rule 4: the create token that resolved to the deleted context is forgotten with it, so
/// the same token carrying different parameters is a NEW request rather than a conflicting
/// retry of one whose result is gone.
#[tokio::test]
async fn the_create_token_of_a_deleted_context_names_no_earlier_request() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-token-host", 1).await;

    // One row per token, so the rule is asserted over more than one set of parameters.
    for (case, body) in context_rows().into_iter().take(3) {
        let token = format!("context-delete-token-{case}");
        let mut original = body.clone();
        original["clientToken"] = Value::String(token.clone());
        let (id, _) = created_context(&client, &profile_arn, &original, case).await;

        let deleted = delete_context(&client, &profile_arn, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgentContext 200, got {deleted:?}"
        );

        // The SAME token with DIFFERENT parameters: a token the service still remembered
        // would make this a conflicting retry (409), and a forgotten one makes it a new
        // create.
        let mut different = original.clone();
        different["title"] = Value::String(format!("{case} after the delete"));
        let answer = create_context(&client, &profile_arn, different.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the token of a deleted context must not refuse a new create, \
             got {answer:?}"
        );
        let replacement = summary(&answer).clone();
        assert_summary_matches(&replacement, &different, &profile_arn, case);
        let replacement_id = id_of(&replacement);
        assert_ne!(
            replacement_id, id,
            "case {case:?}: the new create mints its own id"
        );

        let gone = get_context(&client, &profile_arn, &id).await;
        assert_not_found(
            case,
            &gone,
            "CONTEXT",
            &context_resource_id(&profile_arn, &id),
            &context_subject(&profile_arn, &id),
        );

        // Leave the profile empty for the next row.
        let cleaned = delete_context(&client, &profile_arn, &replacement_id).await;
        assert_eq!(
            cleaned.status, 200,
            "case {case:?}: expected DeleteAgentContext 200 for the replacement, \
             got {cleaned:?}"
        );
        assert_eq!(
            client.store().lock().agent_context_count(&profile_arn),
            0,
            "case {case:?}: both the original and the replacement are gone"
        );
    }
}
