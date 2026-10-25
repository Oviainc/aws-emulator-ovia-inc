// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAgentContext`, the read that resolves ONE context of an
//! agent profile: "Retrieves detailed information about a specific context associated
//! with a profile" (`smithy_query(query="operation", operation="GetAgentContext")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. The read RESOLVES the path's `(profileArn, id)` pair against what the store holds
//!    and reports that context's `ContextSummary` — the identical summary the create
//!    answered, for every shape of content a context can carry.
//! 2. A pair that resolves nothing is this operation's modelled `404
//!    ResourceNotFoundException`: a well-formed `id` no context of the profile carries, a
//!    context id that belongs to ANOTHER profile, and a well-formed `profileArn` no
//!    stored profile carries are all the same refusal, carrying the identifier AS SENT
//!    and the missing resource's type.
//! 3. The read REPORTS: reading a context repeatedly, and reading pairs that resolve
//!    nothing, leave every stored context exactly as it was — same ids, same stored
//!    members, none created and none removed.
//! 4. A path identifier the MODEL refuses never reaches the lookup: a value outside the
//!    `UUID` or the `AgentProfileArn` pattern is refused `400 ValidationException` naming
//!    the member, with the `Message` alone.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The contexts these reads resolve are recorded through the
//! `CreateAgentContext` helper and the row table of `super::agent_contexts` — a context is
//! created by exactly one operation, so the request builders live there and this suite
//! imports them, as `super::get_agent_profile` does for profiles; the 404 is worded by the
//! one `assert_not_found` of `super::workload_harness`, and the profile the contexts hang
//! off is minted by `super::agent_goals::profile_arn_of`.

use serde_json::Value;

use super::agent_contexts::{
    assert_summary_matches, context_rows, create_context, get_context, id_of, summary,
};
use super::agent_goals::{absent_profile_arn, profile_arn_of};
use super::workload_harness::{assert_not_found, member, Answer, Client};

/// `ResourceNotFoundException$ResourceType` for a context of an agent profile, as this
/// service names it. `ExceptionResourceType` is a free-form string
/// (`smithy_query(query="shape", shape="ExceptionResourceType")`), so the type names the
/// kind of resource the lookup missed.
const CONTEXT_RESOURCE_TYPE: &str = "CONTEXT";

/// Every stored context, grouped under the profile that holds it: its id and its whole
/// stored record, read straight off the shared state rather than through the operation
/// under test. Rule 3 compares this before and after the reads: a reporting operation
/// cannot change any member of any stored context.
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

/// Assert one answer is the not-found a context pair resolving nothing earns, through the
/// one not-found assertion this crate's suites share: the 404, the error code, the
/// modelled `ResourceType`, the identifier AS SENT and the message the service builds
/// from the request and the owning account.
fn assert_context_not_found(answer: &Answer, profile_arn: &str, id: &str, case: &str) {
    assert_not_found(
        case,
        answer,
        CONTEXT_RESOURCE_TYPE,
        &format!("{profile_arn}.{id}"),
        &format!("context with ID {id} in an agent profile with ARN {profile_arn}"),
    );
}

/// Whether `message` names `member_name` as a MEMBER rather than as a fragment of another
/// word: the short member names these labels carry (`id`) occur inside words a refusal
/// may well use ("invalid"), so the name has to stand on its own.
fn names_member(message: &str, member_name: &str) -> bool {
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    message.match_indices(member_name).any(|(at, _)| {
        let before = message[..at].chars().next_back();
        let after = message[at + member_name.len()..].chars().next();
        before.is_none_or(|c| !is_word_char(c)) && after.is_none_or(|c| !is_word_char(c))
    })
}

/// The same `UUID` with its last hex digit moved on, so the value differs from the stored
/// id by one character and still matches the modelled pattern.
fn one_digit_off(id: &str) -> String {
    let mut digits: Vec<char> = id.chars().collect();
    let last = digits
        .iter()
        .rposition(char::is_ascii_hexdigit)
        .expect("a UUID carries hex digits");
    digits[last] = if digits[last] == '0' { '1' } else { '0' };
    digits.into_iter().collect()
}

/// Rules 1 and 3: every row of the context table is reported by the pair it was created
/// under — the same summary the create answered, the same summary again on a repeat read
/// — and the reads leave every stored record as it was.
#[tokio::test]
async fn a_stored_context_is_reported_by_the_pair_it_was_created_under() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-read-host", 1).await;

    let mut created: Vec<(&str, Value, Value)> = Vec::new();
    for (case, request) in context_rows() {
        let answer = create_context(&client, &profile_arn, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
        );
        created.push((case, request, summary(&answer).clone()));
    }

    let before = stored_contexts_by_profile(&client);
    for (case, request, created_summary) in &created {
        // Read the same pair twice: a reporting operation answers the identical summary
        // every time, so neither read can have consumed or moved anything.
        for attempt in ["first-read", "second-read"] {
            let read = get_context(&client, &profile_arn, &id_of(created_summary)).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}/{attempt}: expected GetAgentContext 200, got {read:?}"
            );
            assert_eq!(
                summary(&read),
                created_summary,
                "case {case:?}/{attempt}: the read must report the stored context, \
                 got {read:?}"
            );
            assert_summary_matches(summary(&read), request, &profile_arn, case);
        }
    }
    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"repeated-reads\": reading contexts must leave the store as it was"
    );
}

/// Rule 2, across profiles: a context is reached only THROUGH the profile that holds it,
/// so the same id under another profile's ARN resolves nothing — and the context stays
/// where it is. The rows are each profile of a pair, so neither direction is privileged.
#[tokio::test]
async fn a_context_is_read_only_through_the_profile_that_holds_it() {
    let client = Client::new();
    let first = profile_arn_of(&client, "context-read-first", 1).await;
    let second = profile_arn_of(&client, "context-read-second", 0).await;
    assert_ne!(
        first, second,
        "case \"two-profiles\": each CreateAgentProfile mints its own ARN"
    );

    // One context under each profile, so the cross-profile read misses a pair whose id
    // DOES name a stored context — just not one of the profile asked about.
    let mut contexts: Vec<(&str, String, String, Value)> = Vec::new();
    for (case, owner) in [("first", &first), ("second", &second)] {
        let (_, request) = context_rows()
            .into_iter()
            .next()
            .expect("the context table has rows");
        let answer = create_context(&client, owner, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
        );
        let created_summary = summary(&answer).clone();
        contexts.push((case, owner.clone(), id_of(&created_summary), created_summary));
    }

    let before = stored_contexts_by_profile(&client);
    for (case, owner, id, created_summary) in &contexts {
        let other = if owner == &first { &second } else { &first };
        let answer = get_context(&client, other, id).await;
        assert_context_not_found(&answer, other, id, case);

        let still_there = get_context(&client, owner, id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: the context must still resolve under its own profile, \
             got {still_there:?}"
        );
        assert_eq!(
            summary(&still_there),
            created_summary,
            "case {case:?}: the cross-profile read must not have changed it, \
             got {still_there:?}"
        );
    }
    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"cross-profile-reads\": a refused read records and removes nothing"
    );
}

/// Rule 2, for identifiers that name nothing: the `id` is resolved within its profile and
/// the `profileArn` is resolved too, so every well-formed value naming no stored resource
/// is the same 404. The id rows differ along their hex digits, including both extremes of
/// the modelled `UUID` value space; the ARN rows differ along the ARN's resource part,
/// Region and partition, so the lookup is exact rather than a match on one field.
#[tokio::test]
async fn an_unresolvable_context_pair_is_a_not_found_naming_the_pair_as_sent() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-lookup-host", 1).await;
    let (_, request) = context_rows()
        .into_iter()
        .next()
        .expect("the context table has rows");
    let stored = summary(&create_context(&client, &profile_arn, request).await).clone();
    let present_id = id_of(&stored);

    // Well-formed `UUID`s the stored profile holds no context for.
    let absent_ids: Vec<(&str, String)> = vec![
        (
            "all-zeros",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "all-f",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "mixed-hex",
            "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d".to_string(),
        ),
        ("one-digit-off-the-stored-id", one_digit_off(&present_id)),
    ];

    // Well-formed `AgentProfileArn`s no stored profile carries.
    let absent_arns: Vec<(&str, String)> = vec![
        ("absent-hex-resource", absent_profile_arn(&"0".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-2")),
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

    let before = stored_contexts_by_profile(&client);
    for (case, id) in &absent_ids {
        assert_ne!(
            id, &present_id,
            "case {case:?}: the row must not name the context that exists"
        );
        let answer = get_context(&client, &profile_arn, id).await;
        assert_context_not_found(&answer, &profile_arn, id, case);
    }
    for (case, arn) in &absent_arns {
        assert_ne!(
            arn, &profile_arn,
            "case {case:?}: the row must not name the profile that exists"
        );
        // The id names a STORED context — of another profile — so only the unresolvable
        // ARN can be what makes this a miss.
        let answer = get_context(&client, arn, &present_id).await;
        assert_context_not_found(&answer, arn, &present_id, case);
    }

    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"unresolvable-pairs\": a refused read records and removes nothing"
    );
    let fetched = get_context(&client, &profile_arn, &present_id).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored context must still resolve, got {fetched:?}"
    );
    assert_eq!(
        summary(&fetched),
        &stored,
        "case \"present\": the stored context must be unchanged, got {fetched:?}"
    );
}

/// Rule 4: `profileArn` and `id` carry modelled patterns, so a value outside either is
/// refused by the boundary walk as a 400 `ValidationException` naming the member — never
/// answered as a lookup miss. The rows differ along how each value breaks its pattern.
#[tokio::test]
async fn a_malformed_context_path_identifier_is_refused_as_a_validation_exception() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-refusal-host", 1).await;
    let (_, request) = context_rows()
        .into_iter()
        .next()
        .expect("the context table has rows");
    let present_id = id_of(summary(&create_context(&client, &profile_arn, request).await));

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

    // Values the modelled `AgentProfileArn` pattern refuses.
    let bad_arns: Vec<(&str, String)> = vec![
        ("not-an-arn-at-all", "not-an-agent-profile".to_string()),
        (
            "another-service",
            format!(
                "arn:aws:iam::{}:role/NotAProfile",
                engine::emulator_account_id()
            ),
        ),
        (
            "wrong-resource-kind",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:profile/{}",
                engine::emulator_account_id(),
                "0".repeat(32)
            ),
        ),
        (
            "account-too-short",
            "arn:aws:wellarchitected:us-west-2:12345:agent-profile/p".to_string(),
        ),
    ];

    let before = stored_contexts_by_profile(&client);
    let mut refused: Vec<(&str, &str, Answer)> = Vec::new();
    for (case, id) in &bad_ids {
        refused.push((case, "id", get_context(&client, &profile_arn, id).await));
    }
    for (case, arn) in &bad_arns {
        refused.push((
            case,
            "profileArn",
            get_context(&client, arn, &present_id).await,
        ));
    }

    for (case, member_name, answer) in refused {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value the modelled pattern refuses is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            names_member(member(&answer, "Message"), member_name),
            "case {case:?}: the refusal must name the member {member_name:?} on its own, \
             got {answer:?}"
        );
        // A label value refused while the request is still being read as a shape is not
        // classified: `Reason` and `Fields` describe a rule the service ran over a
        // request it could read, and no rule ran here — the envelope
        // `super::review_template_lens_review_reads` pins for the same class.
        assert_eq!(
            (answer.body.get("Reason"), answer.body.get("Fields")),
            (None, None),
            "case {case:?}: a label outside its shape's constraints carries the Message \
             alone, got {answer:?}"
        );
    }

    assert_eq!(
        stored_contexts_by_profile(&client),
        before,
        "case \"refused-rows\": a refused read records and removes nothing"
    );
    let fetched = get_context(&client, &profile_arn, &present_id).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored context must still resolve, got {fetched:?}"
    );
}
