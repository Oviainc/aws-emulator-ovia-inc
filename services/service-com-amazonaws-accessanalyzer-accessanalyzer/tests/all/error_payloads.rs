// SPDX-License-Identifier: CC0-1.0

//! The shape of this service's resource-scoped refusals, driven over the wire.
//!
//! RULE under test: where an error shape declares `resourceId`/`resourceType` beside
//! `message` as REQUIRED members — which `ResourceNotFoundException`,
//! `ConflictException` and `ServiceQuotaExceededException` all do
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,ConflictException,\
//! ServiceQuotaExceededException",model_path="services/\
//! service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/\
//! com-amazonaws-accessanalyzer-accessanalyzer.json")`) — the refusal's `message` is a
//! FIXED text for the (error, resource kind) pair and the identifier of the resource
//! the call named travels in those structured members, never interpolated into prose.
//!
//! So each table below spans the inputs the rule generalises over (the name asked for,
//! its form and length, the operation that refuses, the resource kind, and — for the
//! conflict — how many callers race for one name) and asserts, per row, that the
//! message is the one fixed string, that it contains no value from the request, and
//! that `resourceId`/`resourceType` carry the identifier. The analyzer rows pin AWS's
//! own recorded texts as literals; the archive-rule rows, for which AWS publishes no
//! text, pin the invariant (one constant message, identifier in `resourceId`).
//!
//! The harness (`Emulator`, `Reply`) is the one `analyzer_lifecycle.rs` already owns.

use serde_json::{json, Value};

use super::harness::{Emulator, Reply};

/// AWS's fixed `ResourceNotFoundException` message for an analyzer this account and
/// Region does not hold, as the recorded responses carry it.
const ANALYZER_NOT_FOUND: &str = "Analyzer not found";

/// AWS's fixed `ConflictException` message for a `CreateAnalyzer` whose `analyzerName`
/// is already in use, as the recorded responses carry it.
const ANALYZER_NAME_CONFLICT: &str = "Analyzer with the same name already exists";

/// The `resourceType` an analyzer-scoped refusal reports: the resource-type segment of
/// an analyzer ARN.
const ANALYZER_RESOURCE_TYPE: &str = "analyzer";

/// The `resourceType` an archive-rule-scoped refusal reports.
const ARCHIVE_RULE_RESOURCE_TYPE: &str = "archive-rule";

/// `analyzerName` forms the rule has to hold over: the shortest name the model allows,
/// a plain name, a name opening with the `_` its pattern permits, and one using every
/// character class the pattern allows (`^[A-Za-z_][A-Za-z0-9_.-]*$`, len 1..255 — from
/// `smithy_query(query="operation",operation="GetAnalyzer")`). The longest allowed name
/// is appended at runtime by [`missing_analyzer_names`].
const ANALYZER_NAME_FORMS: &[&str] = &["z", "missing", "_private_name", "Mixed.Case-9_name"];

/// Every `analyzerName` the not-found table asks for, including the 255-character name
/// that sits ON the published upper bound of `AnalyzerName`.
fn missing_analyzer_names() -> Vec<String> {
    let mut names: Vec<String> = ANALYZER_NAME_FORMS.iter().map(|n| n.to_string()).collect();
    names.push(format!("b{}", "x".repeat(254)));
    names
}

/// `ruleName` forms the rule has to hold over (`Name`: `^[A-Za-z][A-Za-z0-9_.-]*$`,
/// len 1..255, from `smithy_query(query="operation",operation="GetArchiveRule")`).
const RULE_NAME_FORMS: &[&str] = &["r", "nope", "Rule.Two-3_x"];

/// The operations that refuse with the analyzer-scoped 404, named by what they do.
#[derive(Debug, Clone, Copy)]
enum AnalyzerCall {
    Get,
    Update,
    Delete,
    GetArchiveRule,
    ListArchiveRules,
    CreateArchiveRule,
    UpdateArchiveRule,
}

/// Every analyzer-scoped call, so the table covers each operation that can report an
/// absent analyzer rather than only the one the issue cited.
const ANALYZER_CALLS: &[AnalyzerCall] = &[
    AnalyzerCall::Get,
    AnalyzerCall::Update,
    AnalyzerCall::Delete,
    AnalyzerCall::GetArchiveRule,
    AnalyzerCall::ListArchiveRules,
    AnalyzerCall::CreateArchiveRule,
    AnalyzerCall::UpdateArchiveRule,
];

impl AnalyzerCall {
    async fn send(self, emulator: &Emulator, analyzer: &str) -> Reply {
        match self {
            AnalyzerCall::Get => emulator.get_analyzer(analyzer).await,
            AnalyzerCall::Update => {
                emulator
                    .update_analyzer(
                        analyzer,
                        json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}}),
                    )
                    .await
            }
            AnalyzerCall::Delete => emulator.delete_analyzer(analyzer).await,
            AnalyzerCall::GetArchiveRule => emulator.get_archive_rule(analyzer, "rule1").await,
            AnalyzerCall::ListArchiveRules => emulator.list_archive_rules(analyzer, "").await,
            AnalyzerCall::CreateArchiveRule => {
                emulator
                    .create_archive_rule(
                        analyzer,
                        json!({"ruleName": "rule1", "filter": {"isPublic": {"eq": ["true"]}}}),
                    )
                    .await
            }
            AnalyzerCall::UpdateArchiveRule => {
                emulator
                    .update_archive_rule(
                        analyzer,
                        "rule1",
                        json!({"filter": {"isPublic": {"eq": ["true"]}}}),
                    )
                    .await
            }
        }
    }
}

/// The shortest request value an "is it echoed?" assertion can look for: shorter
/// values appear inside ordinary words of a fixed message by coincidence.
const MIN_ECHO_LEN: usize = 4;

/// A body member as a string, or `""` when the response does not carry it.
fn text(reply: &Reply, member: &str) -> String {
    reply
        .member(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What one refusal row must answer: the whole payload, in one value, so the
/// assertion helper stays a single argument wide.
#[derive(Debug)]
struct Refusal<'a> {
    /// The row, named in every assertion message.
    case: String,
    status: u16,
    code: &'a str,
    /// The FIXED message text the refusal must carry.
    message: &'a str,
    resource_id: &'a str,
    resource_type: &'a str,
    /// Request values the message may not carry — the point of the rule: a client
    /// reading `message` must see the same string whatever it asked for. Values
    /// shorter than [`MIN_ECHO_LEN`] are skipped, since a one- or two-character name
    /// can occur inside ordinary English words ("Analyzer" contains "z"); for those
    /// rows the equality against the fixed message is what pins the rule.
    forbidden: Vec<&'a str>,
}

/// Assert the full refusal payload of one row: status, wire code, the FIXED message,
/// and the identifier in the modelled `resourceId`/`resourceType` members.
fn assert_refusal(reply: &Reply, expected: &Refusal<'_>) {
    let Refusal {
        case,
        status,
        code,
        message,
        resource_id,
        resource_type,
        forbidden,
    } = expected;
    assert_eq!(
        reply.status, *status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        *code,
        "case {case:?}: expected error code {code:?}, got {:?}",
        reply.code()
    );
    assert_eq!(
        text(reply, "message"),
        *message,
        "case {case:?}: expected the fixed message {message:?}, got {:?}",
        text(reply, "message")
    );
    for value in forbidden {
        if value.len() < MIN_ECHO_LEN {
            continue;
        }
        assert!(
            !text(reply, "message").contains(value),
            "case {case:?}: the message must not carry the request value {value:?}, got {:?}",
            text(reply, "message")
        );
    }
    assert_eq!(
        text(reply, "resourceId"),
        *resource_id,
        "case {case:?}: expected resourceId {resource_id:?}, got {:?}",
        text(reply, "resourceId")
    );
    assert_eq!(
        text(reply, "resourceType"),
        *resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
        text(reply, "resourceType")
    );
}

/// Every operation that can report an absent analyzer answers the SAME fixed message,
/// whatever name was asked for, and reports the name in `resourceId`/`resourceType`.
#[tokio::test]
async fn an_absent_analyzer_is_refused_with_a_fixed_message_and_the_name_in_resource_id() {
    let emulator = Emulator::new();
    // One stored analyzer, so the table is not answering out of an empty store.
    let seeded = emulator
        .create_analyzer(json!({"analyzerName": "stored", "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        seeded.status, 200,
        "the fixture create must succeed, got {seeded:?}"
    );

    for name in &missing_analyzer_names() {
        for call in ANALYZER_CALLS {
            let case = format!("{call:?} of {} chars", name.len());
            let reply = call.send(&emulator, name).await;
            assert_refusal(
                &reply,
                &Refusal {
                    case,
                    status: 404,
                    code: "ResourceNotFoundException",
                    message: ANALYZER_NOT_FOUND,
                    resource_id: name,
                    resource_type: ANALYZER_RESOURCE_TYPE,
                    forbidden: vec![name.as_str()],
                },
            );
        }
    }
}

/// A name re-read after `DeleteAnalyzer` is the same refusal as a name that never
/// existed: same message, same `resourceType`, each reporting its own `resourceId`.
#[tokio::test]
async fn a_deleted_analyzer_is_refused_exactly_like_one_that_never_existed() {
    for name in &missing_analyzer_names() {
        let emulator = Emulator::new();
        let created = emulator
            .create_analyzer(json!({"analyzerName": name, "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the create must succeed, got {created:?}"
        );
        let deleted = emulator.delete_analyzer(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: the delete must succeed, got {deleted:?}"
        );

        let reread = emulator.get_analyzer(name).await;
        assert_refusal(
            &reread,
            &Refusal {
                case: format!("GetAnalyzer after delete of {} chars", name.len()),
                status: 404,
                code: "ResourceNotFoundException",
                message: ANALYZER_NOT_FOUND,
                resource_id: name,
                resource_type: ANALYZER_RESOURCE_TYPE,
                forbidden: vec![name.as_str()],
            },
        );

        // And a name this store never held answers with the identical text.
        let never = emulator.get_analyzer("neverCreated").await;
        assert_eq!(
            text(&never, "message"),
            text(&reread, "message"),
            "case {name:?}: a deleted name and an unknown name must carry one message, got {:?} and {:?}",
            text(&reread, "message"),
            text(&never, "message")
        );
    }
}

/// An absent archive rule on a stored analyzer is its own resource kind: one fixed
/// message, the RULE name in `resourceId` and `resourceType` `archive-rule`.
#[tokio::test]
async fn an_absent_archive_rule_is_refused_with_a_fixed_message_and_the_rule_in_resource_id() {
    let emulator = Emulator::new();
    for analyzer in ["stored", "Other.Analyzer-1"] {
        let created = emulator
            .create_analyzer(json!({
                "analyzerName": analyzer,
                "type": "ACCOUNT",
                "archiveRules": [{"ruleName": "kept", "filter": {"isPublic": {"eq": ["true"]}}}],
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {analyzer:?}: the fixture create must succeed, got {created:?}"
        );
    }

    let mut messages: Vec<String> = Vec::new();
    for analyzer in ["stored", "Other.Analyzer-1"] {
        for rule in RULE_NAME_FORMS {
            // Both operations that resolve an existing rule refuse it the same way.
            let replies = [
                (
                    format!("GetArchiveRule {analyzer}/{rule}"),
                    emulator.get_archive_rule(analyzer, rule).await,
                ),
                (
                    format!("UpdateArchiveRule {analyzer}/{rule}"),
                    emulator
                        .update_archive_rule(
                            analyzer,
                            rule,
                            json!({"filter": {"isPublic": {"eq": ["true"]}}}),
                        )
                        .await,
                ),
            ];
            for (case, reply) in &replies {
                assert_eq!(
                    reply.status, 404,
                    "case {case:?}: an absent rule must be a 404, got {reply:?}"
                );
                assert_eq!(
                    reply.code(),
                    "ResourceNotFoundException",
                    "case {case:?}: the 404 must be a ResourceNotFoundException, got {reply:?}"
                );
                for value in [analyzer, rule] {
                    if value.len() < MIN_ECHO_LEN {
                        continue;
                    }
                    assert!(
                        !text(reply, "message").contains(value),
                        "case {case:?}: the message must not carry the request value {value:?}, got {:?}",
                        text(reply, "message")
                    );
                }
                assert_eq!(
                    text(reply, "resourceId"),
                    *rule,
                    "case {case:?}: expected resourceId {rule:?}, got {:?}",
                    text(reply, "resourceId")
                );
                assert_eq!(
                    text(reply, "resourceType"),
                    ARCHIVE_RULE_RESOURCE_TYPE,
                    "case {case:?}: expected resourceType {ARCHIVE_RULE_RESOURCE_TYPE:?}, got {:?}",
                    text(reply, "resourceType")
                );
                messages.push(text(reply, "message"));
            }
        }
    }

    // The message is FIXED: one distinct value across every analyzer/rule/operation
    // triple above.
    messages.sort();
    messages.dedup();
    assert_eq!(
        messages.len(),
        1,
        "the archive-rule not-found message must be one fixed string, got {messages:?}"
    );
    assert!(
        !messages[0].is_empty(),
        "the archive-rule not-found message must not be empty, got {messages:?}"
    );
}

/// A duplicate `analyzerName` is refused with AWS's fixed conflict text, whatever the
/// name and whatever analyzer type the second call asks for, and the conflict is STATE:
/// once the stored analyzer is deleted the identical call succeeds.
#[tokio::test]
async fn a_duplicate_analyzer_name_conflicts_with_a_fixed_message() {
    // (case, analyzerName, type of the first create, type of the second)
    let names = missing_analyzer_names();
    let second_types = ["ACCOUNT", "ORGANIZATION", "ACCOUNT_UNUSED_ACCESS"];

    for (index, name) in names.iter().enumerate() {
        let emulator = Emulator::new();
        let first = emulator
            .create_analyzer(json!({"analyzerName": name, "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first create must succeed, got {first:?}"
        );

        // Every repeat conflicts — including a repeat asking for a different type, and
        // a repeat carrying a fresh `clientToken` (a new request, not a retry).
        let second_type = second_types[index % second_types.len()];
        let repeats: Vec<(String, Value)> = vec![
            (
                format!("same type, {} chars", name.len()),
                json!({"analyzerName": name, "type": "ACCOUNT"}),
            ),
            (
                format!("type {second_type}"),
                json!({"analyzerName": name, "type": second_type}),
            ),
            (
                "a fresh clientToken".to_string(),
                json!({"analyzerName": name, "type": "ACCOUNT", "clientToken": "token-for-a-new-request"}),
            ),
        ];
        for (case, body) in &repeats {
            let conflict = emulator.create_analyzer(body.clone()).await;
            assert_refusal(
                &conflict,
                &Refusal {
                    case: case.clone(),
                    status: 409,
                    code: "ConflictException",
                    message: ANALYZER_NAME_CONFLICT,
                    resource_id: name,
                    resource_type: ANALYZER_RESOURCE_TYPE,
                    forbidden: vec![name.as_str()],
                },
            );
        }

        // The conflict is state, not invalid input.
        let deleted = emulator.delete_analyzer(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: the delete must succeed, got {deleted:?}"
        );
        let again = emulator
            .create_analyzer(json!({"analyzerName": name, "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            again.status, 200,
            "case {name:?}: after the delete the identical create must succeed, got {again:?}"
        );
    }
}

/// A duplicate `ruleName` under one analyzer conflicts with a fixed message naming the
/// rule only in `resourceId`.
#[tokio::test]
async fn a_duplicate_archive_rule_name_conflicts_with_a_fixed_message() {
    let emulator = Emulator::new();
    let created = emulator
        .create_analyzer(json!({"analyzerName": "stored", "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture create must succeed, got {created:?}"
    );

    let mut messages: Vec<String> = Vec::new();
    for rule in RULE_NAME_FORMS {
        let filter = json!({"isPublic": {"eq": ["true"]}});
        let first = emulator
            .create_archive_rule("stored", json!({"ruleName": rule, "filter": filter}))
            .await;
        assert_eq!(
            first.status, 200,
            "case {rule:?}: the first rule create must succeed, got {first:?}"
        );

        // A second create of the same rule name, with a DIFFERENT filter, is still a
        // name conflict.
        let conflict = emulator
            .create_archive_rule(
                "stored",
                json!({"ruleName": rule, "filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}),
            )
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {rule:?}: a duplicate rule name must be a 409, got {conflict:?}"
        );
        assert_eq!(
            conflict.code(),
            "ConflictException",
            "case {rule:?}: the 409 must be a ConflictException, got {conflict:?}"
        );
        for value in ["stored", *rule] {
            if value.len() < MIN_ECHO_LEN {
                continue;
            }
            assert!(
                !text(&conflict, "message").contains(value),
                "case {rule:?}: the message must not carry the request value {value:?}, got {:?}",
                text(&conflict, "message")
            );
        }
        assert_eq!(
            text(&conflict, "resourceId"),
            *rule,
            "case {rule:?}: expected resourceId {rule:?}, got {:?}",
            text(&conflict, "resourceId")
        );
        assert_eq!(
            text(&conflict, "resourceType"),
            ARCHIVE_RULE_RESOURCE_TYPE,
            "case {rule:?}: expected resourceType {ARCHIVE_RULE_RESOURCE_TYPE:?}, got {:?}",
            text(&conflict, "resourceType")
        );
        messages.push(text(&conflict, "message"));
    }

    messages.sort();
    messages.dedup();
    assert_eq!(
        messages.len(),
        1,
        "the archive-rule conflict message must be one fixed string, got {messages:?}"
    );
    assert!(
        !messages[0].is_empty(),
        "the archive-rule conflict message must not be empty, got {messages:?}"
    );
}

/// Under N callers racing for one unused name, exactly one wins and every loser gets
/// the SAME fixed conflict payload — the message is one template, not per-caller prose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_creates_of_one_name_leave_every_loser_the_same_conflict() {
    let emulator = ::std::sync::Arc::new(Emulator::new());
    // Counts spanning two racers to several; the rule is about EVERY loser, so the
    // loser count is what varies.
    for racers in [2usize, 3, 5] {
        let name = format!("race{racers}");
        let body = json!({"analyzerName": name, "type": "ACCOUNT"});

        // All `racers` calls are in flight together on several worker threads, so the
        // store decides the winner, not the order the test issued them in.
        let mut tasks = Vec::new();
        for _ in 0..racers {
            let emulator = ::std::sync::Arc::clone(&emulator);
            let body = body.clone();
            tasks.push(tokio::spawn(
                async move { emulator.create_analyzer(body).await },
            ));
        }
        let mut replies = Vec::new();
        for task in tasks {
            replies.push(task.await.expect("the racing task completed"));
        }

        let winners = replies.iter().filter(|r| r.status == 200).count();
        assert_eq!(
            winners, 1,
            "case {racers} racers: exactly one create must win, got {replies:?}"
        );

        let losers: Vec<&Reply> = replies.iter().filter(|r| r.status != 200).collect();
        assert_eq!(
            losers.len(),
            racers - 1,
            "case {racers} racers: every other create must be refused, got {replies:?}"
        );
        for loser in &losers {
            assert_refusal(
                loser,
                &Refusal {
                    case: format!("{racers} racers, a loser"),
                    status: 409,
                    code: "ConflictException",
                    message: ANALYZER_NAME_CONFLICT,
                    resource_id: &name,
                    resource_type: ANALYZER_RESOURCE_TYPE,
                    forbidden: vec![name.as_str()],
                },
            );
        }

        // The winner is readable, and it is the only analyzer stored under that name.
        let read = emulator.get_analyzer(&name).await;
        assert_eq!(
            read.status, 200,
            "case {racers} racers: the winner must be readable, got {read:?}"
        );
    }
}
