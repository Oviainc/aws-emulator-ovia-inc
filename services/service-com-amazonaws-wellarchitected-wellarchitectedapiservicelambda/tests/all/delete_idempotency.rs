// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `@idempotencyToken` rule on the DELETES this service
//! implements.
//!
//! The rule, from the member's own documentation (`smithy_query(query="shape",
//! shape="ClientRequestToken")`): "A unique case-sensitive string used to ensure that this
//! request is idempotent (executes only once) … If you retry a request with the same
//! client request token and the same parameters after the original request has completed
//! successfully, the result of the original request is returned." For a delete that means
//! the retry CANNOT be served by resolving the target — the original request removed it —
//! so what the rule pins here is:
//!
//! 1. A retry carrying the token of a delete that already completed, naming the same
//!    target, replays that request's result: an empty HTTP 200, however many times it is
//!    repeated, and without resurrecting what was deleted.
//! 2. A delete carrying a token this service has not spent is a NEW request, so it
//!    resolves its target and is a 404 `ResourceNotFoundException` when nothing carries
//!    the identifier.
//! 3. A refused delete completes nothing, so the token it carried is still unspent: the
//!    same token later deletes a target that does exist.
//! 4. A token already spent on a delete of a DIFFERENT target cannot be a retry of this
//!    one, so it is a 409 `ConflictException` and the target it named is untouched.
//! 5. A replay answers the ORIGINAL request's result and changes NOTHING: it does not
//!    resurrect what that request removed, it does not touch a resource created after it —
//!    including a replacement carrying the deleted resource's own name — and it keeps
//!    replaying however many later deletes have spent tokens of their own in between.
//!
//! Every rule is asserted over a TABLE: one row per implemented delete that binds the
//! member, crossed with the token forms the model allows. Requests go through the real
//! `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run as they do in production.

use serde_json::Value;

use super::workload_harness::{
    created_arn, created_id, encode_path_segment, minimal_create, minimal_template, Answer, Client,
};

/// The deletes this service IMPLEMENTS that bind a `ClientRequestToken`, so the rule is
/// asserted over the class rather than over one operation. `DeleteLens`, `DeleteProfile`,
/// `DeleteWorkloadShare`, `DeleteLensShare`, `DeleteProfileShare` and
/// `DeleteTemplateShare` bind the member too and are still stubs, each with its own issue;
/// `DeleteAgentGoal`, `DeleteAgentContext` and `DeleteAgentProfile` bind no token at all
/// (`smithy_query(query="operation", operation="DeleteAgentGoal")`), so the rule has
/// nothing to say about them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deletable {
    Workload,
    ReviewTemplate,
}

/// Both rows, for the tables below to walk.
const DELETABLES: [Deletable; 2] = [Deletable::Workload, Deletable::ReviewTemplate];

impl Deletable {
    /// The operation a row exercises, for the assertion messages.
    fn operation(self) -> &'static str {
        match self {
            Self::Workload => "DeleteWorkload",
            Self::ReviewTemplate => "DeleteReviewTemplate",
        }
    }

    /// A short, path-safe tag naming the kind, so the resource names a row mints are
    /// unique within the shared store.
    fn tag(self) -> &'static str {
        match self {
            Self::Workload => "wl",
            Self::ReviewTemplate => "rt",
        }
    }
}

/// Create one resource of `kind` and answer the identifier its delete takes — the
/// `WorkloadId` or the `TemplateArn`.
async fn create(client: &Client, kind: Deletable, name: &str) -> String {
    let token = format!("tok-create-{name}");
    match kind {
        Deletable::Workload => created_id(
            &client.create(minimal_create(name, &token)).await,
            &format!("{name}/create"),
        ),
        Deletable::ReviewTemplate => {
            let answer = client
                .create_review_template(minimal_template(name, &token))
                .await;
            created_arn(&answer, &format!("{name}/create"))
        }
    }
}

/// Delete `id` of `kind` under `token`. The token is percent-encoded into the query
/// string, exactly as an SDK sends an `@httpQuery` member, so a row may carry any ASCII
/// the modelled `^[\x00-\x7F]*$` pattern allows.
async fn delete(client: &Client, kind: Deletable, id: &str, token: &str) -> Answer {
    let encoded = encode_path_segment(token);
    match kind {
        Deletable::Workload => {
            client
                .send(
                    "DELETE",
                    &format!("/workloads/{id}?ClientRequestToken={encoded}"),
                    None,
                )
                .await
        }
        Deletable::ReviewTemplate => {
            client
                .send(
                    "DELETE",
                    &format!(
                        "/reviewTemplates/{}?ClientRequestToken={encoded}",
                        encode_path_segment(id)
                    ),
                    None,
                )
                .await
        }
    }
}

/// Read `id` of `kind` back through the operation that resolves that identifier, so a row
/// can tell whether a delete or a replay changed what the store holds.
async fn read(client: &Client, kind: Deletable, id: &str) -> Answer {
    match kind {
        Deletable::Workload => client.get(id).await,
        Deletable::ReviewTemplate => client.get_review_template(id).await,
    }
}

/// Every assertion the result of a delete has to satisfy: the modelled empty 200 that both
/// operations answer (`http {method: DELETE, code: 200}` over a `Unit` output).
fn assert_deleted(answer: &Answer, case: &str, operation: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} 200, got {answer:?}"
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: {operation} models no output members, so the body is empty, got {answer:?}"
    );
    assert!(
        answer.error_code.is_empty(),
        "case {case:?}: a successful {operation} carries no error code, got {answer:?}"
    );
}

/// One row of rule 1: the token the original delete carries and how many retries follow
/// it.
///
/// `ClientRequestToken` is `@length(min: 1, max: 2048)` over `^[\x00-\x7F]*$`
/// (`smithy_query(query="operation", operation="DeleteWorkload")`, whose
/// `ClientRequestToken` member reports both), so the rows span that range: each published
/// bound, a typical SDK-generated token, and an ASCII form the pattern allows but no SDK
/// would mint. One character past each bound is refused at the boundary, which
/// [`a_token_outside_the_modelled_length_is_refused`] pins. Every token is distinct,
/// because a token names ONE request.
fn replay_rows() -> Vec<(&'static str, String, usize)> {
    vec![
        ("shortest-allowed", "a".to_string(), 1),
        ("shortest-allowed-repeated", "b".to_string(), 5),
        (
            "sdk-generated",
            "3f1c9e02-5b7a-4d16-8e44-0c2f9a7b1d53".to_string(),
            1,
        ),
        ("ascii-punctuation", "retry.token~1_A-B".to_string(), 3),
        ("longest-allowed", "t".repeat(2048), 1),
        (
            "longest-allowed-repeated",
            format!("{}u", "t".repeat(2047)),
            5,
        ),
    ]
}

/// Rule 1. The rows differ along the operation, the form and length of the token (each
/// published bound included) and how many times the retry is repeated, because a replay is
/// the token's doing and not the shape of it.
#[tokio::test]
async fn a_retry_under_the_original_token_replays_the_delete() {
    let client = Client::new();

    for kind in DELETABLES {
        for (form, token, retries) in replay_rows() {
            let case = format!("{}/{form}", kind.operation());
            let name = format!("{}-{form}", kind.tag());
            let id = create(&client, kind, &name).await;

            let first = delete(&client, kind, &id, &token).await;
            assert_deleted(&first, &case, kind.operation());

            // The target is gone: the delete really deleted rather than being swallowed
            // by the ledger.
            let gone = read(&client, kind, &id).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: the deleted target must not resolve, got {gone:?}"
            );

            for attempt in 1..=retries {
                let retry = delete(&client, kind, &id, &token).await;
                assert_deleted(&retry, &format!("{case}/retry-{attempt}"), kind.operation());
                // A replay returns a result; it does not bring the resource back.
                let still_gone = read(&client, kind, &id).await;
                assert_eq!(
                    still_gone.status, 404,
                    "case {case:?}/retry-{attempt}: a replay must not resurrect the \
                     target, got {still_gone:?}"
                );
            }
        }
    }
}

/// Rule 2. The rows differ along the operation and along the token the second delete
/// carries, because only the token of the request that completed names that request.
#[tokio::test]
async fn a_delete_under_an_unspent_token_resolves_its_target_afresh() {
    let client = Client::new();

    for kind in DELETABLES {
        // Rows differ along how the fresh token relates to the spent one: a different
        // token entirely, and one differing from it only in case — the member is
        // documented "case-sensitive", so the two are different tokens.
        for (form, spent, fresh) in [
            ("unrelated-token", "tok-spent-delete", "tok-other-delete"),
            ("differs-only-in-case", "tok-Case-Delete", "tok-case-delete"),
        ] {
            let case = format!("{}/{form}", kind.operation());
            let name = format!("{}-unspent-{form}", kind.tag());
            let id = create(&client, kind, &name).await;

            assert_deleted(
                &delete(&client, kind, &id, spent).await,
                &case,
                kind.operation(),
            );

            let answer = delete(&client, kind, &id, fresh).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: a delete under an unspent token must resolve the target, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
        }
    }
}

/// Rule 3. The rows differ along the operation: a delete that was REFUSED completed
/// nothing, so the token it carried is still free to name a request that does complete.
#[tokio::test]
async fn a_refused_delete_does_not_spend_its_token() {
    let client = Client::new();

    for kind in DELETABLES {
        let case = kind.operation().to_string();
        let name = format!("{}-refused-then-used", kind.tag());
        let token = format!("tok-refused-{}", kind.tag());

        // A target that does not exist, built from one that does and then deleted, so the
        // identifier is well formed under the modelled pattern.
        let absent = create(&client, kind, &format!("{name}-absent")).await;
        assert_deleted(
            &delete(&client, kind, &absent, "tok-clear-the-way").await,
            &format!("{case}/setup"),
            kind.operation(),
        );

        let refused = delete(&client, kind, &absent, &token).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: the setup delete must be refused, got {refused:?}"
        );

        // The same token now names a request that CAN complete, and completes it.
        let live = create(&client, kind, &name).await;
        assert_deleted(
            &delete(&client, kind, &live, &token).await,
            &format!("{case}/reused-after-refusal"),
            kind.operation(),
        );
        let gone = read(&client, kind, &live).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the reused token must have deleted the live target, got {gone:?}"
        );
    }
}

/// Rule 4. The rows differ along the operation: one token names ONE request, so a token
/// already spent on another target is not a retry of this one.
#[tokio::test]
async fn a_token_spent_on_another_target_is_a_conflict() {
    let client = Client::new();

    for kind in DELETABLES {
        let case = kind.operation().to_string();
        let token = format!("tok-shared-{}", kind.tag());
        let first = create(&client, kind, &format!("{}-conflict-first", kind.tag())).await;
        let second = create(&client, kind, &format!("{}-conflict-second", kind.tag())).await;

        assert_deleted(
            &delete(&client, kind, &first, &token).await,
            &format!("{case}/first"),
            kind.operation(),
        );

        let answer = delete(&client, kind, &second, &token).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: a token spent on another target must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(first.as_str()),
            "case {case:?}: the conflict must name what the token already deleted, got {answer:?}"
        );

        // The refused request changed nothing: the second target is still stored, and
        // still deletable under a token of its own.
        let survivor = read(&client, kind, &second).await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: a refused delete must leave its target stored, got {survivor:?}"
        );
        assert_deleted(
            &delete(&client, kind, &second, &format!("{token}-own")).await,
            &format!("{case}/second"),
            kind.operation(),
        );
    }
}

/// One row of rule 5: how much happens between the original delete and its retry, because
/// a replay is served from the record of the original request and not from whatever the
/// store holds when the retry arrives.
const INTERLEAVED_DELETES: [(&str, usize); 3] = [
    ("retried-straight-away", 0),
    ("one-delete-in-between", 1),
    ("several-deletes-in-between", 5),
];

/// Rule 5. The rows differ along the operation and along how many OTHER deletes completed
/// between the original request and its retry, and every row puts a replacement resource —
/// one carrying the deleted resource's own name, which the delete freed — in the store
/// before the retry runs.
#[tokio::test]
async fn a_replay_leaves_the_store_exactly_as_the_original_delete_left_it() {
    let client = Client::new();

    for kind in DELETABLES {
        for (form, in_between) in INTERLEAVED_DELETES {
            let case = format!("{}/{form}", kind.operation());
            let name = format!("{}-replay-{form}", kind.tag());
            let token = format!("tok-replay-{}-{form}", kind.tag());

            let original = create(&client, kind, &name).await;
            assert_deleted(
                &delete(&client, kind, &original, &token).await,
                &case,
                kind.operation(),
            );

            // Deletes of other targets, each under a token of its own, spend ledger
            // entries between the original request and its retry.
            let mut interleaved: Vec<String> = Vec::new();
            for step in 0..in_between {
                let other = create(&client, kind, &format!("{name}-between-{step}")).await;
                assert_deleted(
                    &delete(&client, kind, &other, &format!("{token}-between-{step}")).await,
                    &format!("{case}/between-{step}"),
                    kind.operation(),
                );
                interleaved.push(other);
            }

            // The replacement is a DIFFERENT resource that happens to carry the deleted
            // one's name, so a replay keyed to anything but the original request would
            // reach it.
            let replacement = create(&client, kind, &name).await;
            assert_ne!(
                replacement, original,
                "case {case:?}: the replacement must be its own resource, got the deleted \
                 identifier back"
            );

            assert_deleted(
                &delete(&client, kind, &original, &token).await,
                &format!("{case}/retry"),
                kind.operation(),
            );

            // What the original request removed is still gone, and so is every target the
            // interleaved deletes removed.
            let gone = read(&client, kind, &original).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: a replay must not resurrect the original target, got {gone:?}"
            );
            for (step, other) in interleaved.iter().enumerate() {
                let still_gone = read(&client, kind, other).await;
                assert_eq!(
                    still_gone.status, 404,
                    "case {case:?}/between-{step}: a replay must not resurrect another \
                     deleted target, got {still_gone:?}"
                );
            }

            // And the replacement is untouched: still stored, and still deletable under a
            // token of its own.
            let survivor = read(&client, kind, &replacement).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: a replay must leave a resource created after the original \
                 delete stored, got {survivor:?}"
            );
            assert_deleted(
                &delete(&client, kind, &replacement, &format!("{token}-replacement")).await,
                &format!("{case}/replacement"),
                kind.operation(),
            );
        }
    }
}

/// A token one character past either published bound of the modelled
/// `@length(min: 1, max: 2048)` is refused at the boundary before the handler runs, so it
/// never reaches a ledger — the bounds themselves are pinned by the `shortest-allowed` and
/// `longest-allowed` rows of rule 1.
#[tokio::test]
async fn a_token_outside_the_modelled_length_is_refused() {
    let client = Client::new();

    for kind in DELETABLES {
        // Rows differ along WHICH bound the token breaks: one character short of the
        // minimum, and one past the maximum.
        for (form, token) in [
            ("one-short-of-the-minimum", String::new()),
            ("one-past-the-maximum", "t".repeat(2049)),
        ] {
            let case = format!("{}/{form}", kind.operation());
            let id = create(&client, kind, &format!("{}-{form}", kind.tag())).await;

            let answer = delete(&client, kind, &id, &token).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a {}-character ClientRequestToken breaks \
                 @length(1, 2048), got {answer:?}",
                token.len()
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );

            // The refused request deleted nothing.
            let survivor = read(&client, kind, &id).await;
            assert_eq!(
                survivor.status, 200,
                "case {case:?}: a refused delete must leave its target stored, got \
                 {survivor:?}"
            );
        }
    }
}
