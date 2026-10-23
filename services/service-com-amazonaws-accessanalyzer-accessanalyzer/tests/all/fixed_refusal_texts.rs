// SPDX-License-Identifier: CC0-1.0

//! The CODE and the fixed TEXT this service's rejection paths answer with, driven over
//! the wire.
//!
//! Four rules are under test, each over a table that spans the class it generalises over
//! rather than over one recorded request:
//!
//! 1. An ARN-addressed TAGGING call (`POST`/`GET`/`DELETE /tags/{resourceArn}`) naming no
//!    resource this account and Region holds is refused with HTTP 404 and the error code
//!    `NotFoundException` — the tagging path's own code, which this model declares for no
//!    operation — while the analyzer-name-addressed operations keep the modelled
//!    `ResourceNotFoundException` for the same dead identifier.
//! 2. A refusal whose condition is one documented constraint, not one member's value,
//!    carries the service's FIXED text for that condition: `CheckAccessNotGranted`'s
//!    at-least-one-of rule on the `access` object,
//! 3. `CreateServiceLinkedAnalyzer`'s refusal of a direct customer invocation — which
//!    names no caller, so it is the same string for every credential — and
//! 4. `UpdateAnalyzer`'s and `StartResourceScan`'s refusal of an analyzer whose type the
//!    action does not act on.
//!
//! Each expected string is AWS's own constant, written out here as a literal (the crate's
//! copy is what it is asserted against, so a test row cannot be satisfied by reading the
//! implementation's text back). Every case goes through the real router
//! (`tests/all/harness.rs`), so restJson1 routing, the boundary constraint validation,
//! the handler and the response serialization all run exactly as they do in the emulator
//! binary.

use serde_json::{json, Value};

use super::analyzer_lifecycle::ANALYZER_TYPES;
use crate::harness::{expected_arn, Emulator, Reply, REGION};

/// AWS's fixed `ValidationException` message for an `access` object that names neither an
/// action nor a resource, as the recorded responses carry it. The constant states the
/// at-least-one-of rule about the access OBJECT, so it carries no list index and nothing
/// else from the request.
const ACCESS_OBJECT_EMPTY: &str = "The access object must have at least one action or one resource.";

/// AWS's fixed `AccessDeniedException` message for a direct customer invocation of
/// `CreateServiceLinkedAnalyzer`, as the recorded responses carry it. It names no caller
/// identity, account or Region.
const SERVICE_LINKED_DENIED: &str = "The caller does not have sufficient permissions, or the service principal is not authorized to create analyzers";

/// AWS's fixed `ValidationException` message for a well-formed request asking for
/// something the resource it names does not support, as the recorded responses carry it.
const UNSUPPORTED_OPERATION: &str = "Unsupported operation";

/// The error code the ARN-addressed tagging path answers a resource it cannot resolve
/// with. NOT a shape this service's model declares — the model gives the tagging
/// operations `ResourceNotFoundException` — which is exactly what rule 1 is about.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The error code every analyzer-name-addressed operation answers an identifier it
/// cannot resolve with: the modelled 404 of this service.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The analyzer `Type` values that have no `AnalyzerConfiguration` at all — the external
/// access analyzers, which is what `CreateAnalyzer`'s `configuration` member documents
/// ("Specifies the configuration of the analyzer. If the analyzer is an unused access
/// analyzer, the specified scope of unused access …"); both are rows of
/// [`ANALYZER_TYPES`].
const EXTERNAL_ACCESS_TYPES: &[&str] = &["ACCOUNT", "ORGANIZATION"];

/// A `message` member read off a reply, or the empty string when it carries none.
fn message(reply: &Reply) -> &str {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Create an `ACCOUNT` analyzer and answer the ARN it must carry.
async fn create_analyzer(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .create_analyzer(json!({"analyzerName": name, "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture create of {name:?} must succeed, got {created:?}"
    );
    expected_arn(name)
}

/// The three tagging operations, named by what they do, so one table row is asserted on
/// each of them.
#[derive(Debug, Clone, Copy)]
enum TaggingCall {
    Tag,
    ListTags,
    Untag,
}

const TAGGING_CALLS: &[TaggingCall] = &[
    TaggingCall::Tag,
    TaggingCall::ListTags,
    TaggingCall::Untag,
];

impl TaggingCall {
    async fn send(self, emulator: &Emulator, resource_arn: &str) -> Reply {
        match self {
            TaggingCall::Tag => {
                emulator
                    .tag_resource(resource_arn, &json!({"probe": "post-delete"}))
                    .await
            }
            TaggingCall::ListTags => emulator.list_tags_for_resource(resource_arn).await,
            TaggingCall::Untag => emulator.untag_resource(resource_arn, &["probe"]).await,
        }
    }
}

/// RULE 1: the ARN-addressed tagging path answers an ARN it cannot resolve with HTTP 404
/// `NotFoundException`, whatever makes the ARN unresolvable and whichever tagging
/// operation asked.
///
/// The rows differ along WHY nothing answers to the ARN — a name that never existed, a
/// name that was deleted (the recorded case), another account, another Region, another
/// service, a resource type this service does not tag — and the table is driven on each
/// of the three tagging operations, since all three resolve the resource the same way.
#[tokio::test]
async fn a_tagging_call_on_an_absent_resource_is_a_not_found_exception() {
    let emulator = Emulator::new();
    // One analyzer that DOES exist, so the table is not answering out of an empty store,
    // and one that existed and was deleted.
    let live = create_analyzer(&emulator, "live_target").await;
    let deleted = create_analyzer(&emulator, "deleted_target").await;
    let removed = emulator.delete_analyzer("deleted_target").await;
    assert_eq!(
        removed.status, 200,
        "the fixture delete must succeed, got {removed:?}"
    );

    let account = engine::emulator_account_id();
    let rows: Vec<(&str, String)> = vec![
        ("a name that never existed", expected_arn("never_created")),
        ("a name that was deleted", deleted.clone()),
        (
            "an analyzer of another account",
            format!("arn:aws:access-analyzer:{REGION}:111122223333:analyzer/live_target"),
        ),
        (
            "an analyzer of another Region",
            format!("arn:aws:access-analyzer:eu-west-1:{account}:analyzer/live_target"),
        ),
        ("another service's resource", "arn:aws:s3:::some-bucket".to_string()),
        (
            "a resource type this service does not tag",
            format!("arn:aws:access-analyzer:{REGION}:{account}:archive-rule/live_target"),
        ),
    ];

    let mut messages: Vec<String> = Vec::new();
    for (row, resource_arn) in &rows {
        for call in TAGGING_CALLS {
            let case = format!("{call:?} on {row}");
            let reply = call.send(&emulator, resource_arn).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an unresolvable ARN is HTTP 404, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                NOT_FOUND_EXCEPTION,
                "case {case:?}: expected the tagging path's code {NOT_FOUND_EXCEPTION:?}, got {:?}",
                reply.code()
            );
            assert!(
                !message(&reply).is_empty(),
                "case {case:?}: the refusal must carry a message, got {reply:?}"
            );
            // The shape carries no structured identifier, so the ARN must not be echoed
            // in the text either.
            assert!(
                !message(&reply).contains(resource_arn.as_str()),
                "case {case:?}: the message must not echo the ARN, got {:?}",
                message(&reply)
            );
            messages.push(message(&reply).to_string());
        }
    }

    // One constant for the refusal: every row and every operation answered the same text.
    let first = messages.first().cloned().unwrap_or_default();
    for (index, text) in messages.iter().enumerate() {
        assert_eq!(
            *text, first,
            "row {index}: every not-found refusal of the tagging path carries one text, \
             expected {first:?}, got {text:?}"
        );
    }

    // The boundary of the rule: the resource that IS there still tags, and a tagging call
    // whose ARN is not an ARN at all is still the modelled 400.
    let tagged = emulator.tag_resource(&live, &json!({"probe": "live"})).await;
    assert_eq!(
        tagged.status, 204,
        "a live resource must still accept tags, got {tagged:?}"
    );
    let malformed = emulator
        .tag_resource("live_target", &json!({"probe": "live"}))
        .await;
    assert_eq!(
        (malformed.status, malformed.code()),
        (400, "ValidationException"),
        "an ARN that is not an ARN is a 400 ValidationException, got {malformed:?}"
    );
}

/// The OTHER half of rule 1: the analyzer-name-addressed operations answer the SAME dead
/// identifier with the modelled `ResourceNotFoundException`, so the two planes are told
/// apart by their code.
#[tokio::test]
async fn an_analyzer_name_addressed_call_keeps_the_modelled_not_found_code() {
    let emulator = Emulator::new();
    create_analyzer(&emulator, "gone").await;
    let removed = emulator.delete_analyzer("gone").await;
    assert_eq!(
        removed.status, 200,
        "the fixture delete must succeed, got {removed:?}"
    );

    for (case, reply) in [
        ("GetAnalyzer", emulator.get_analyzer("gone").await),
        ("DeleteAnalyzer", emulator.delete_analyzer("gone").await),
        (
            "UpdateAnalyzer",
            emulator
                .update_analyzer(
                    "gone",
                    json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}}),
                )
                .await,
        ),
        (
            "ListArchiveRules",
            emulator.list_archive_rules("gone", "").await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent analyzer is HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {:?}",
            reply.code()
        );
    }

    // And the tagging call against that same analyzer's ARN answers the other code.
    let tagged = emulator
        .tag_resource(&expected_arn("gone"), &json!({"probe": "post-delete"}))
        .await;
    assert_eq!(
        (tagged.status, tagged.code()),
        (404, NOT_FOUND_EXCEPTION),
        "the tagging path must answer {NOT_FOUND_EXCEPTION:?} for the ARN whose analyzer \
         lookup answers {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {tagged:?}"
    );
}

/// A single-statement identity policy, as `AccessCheckPolicyDocument` is modelled: a JSON
/// string.
fn identity_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3:GetObject",
            "Resource": "arn:aws:s3:::reports/*",
        }],
    })
    .to_string()
}

/// A two-statement identity policy, so one row of the table probes a longer document.
fn wide_identity_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
            {"Effect": "Deny", "Action": "iam:*", "Resource": "*"},
        ],
    })
    .to_string()
}

/// A single-statement resource policy, for the other `policyType` the model accepts.
fn resource_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::111122223333:role/app"},
            "Action": "sqs:SendMessage",
            "Resource": "arn:aws:sqs:us-east-1:111122223333:jobs",
        }],
    })
    .to_string()
}

/// `POST /policy/check-access-not-granted`.
async fn check_access_not_granted(
    emulator: &Emulator,
    policy_document: &str,
    policy_type: &str,
    access: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            "/policy/check-access-not-granted",
            Some(json!({
                "policyDocument": policy_document,
                "policyType": policy_type,
                "access": access,
            })),
        )
        .await
}

/// RULE 2: an `access` object naming neither an action nor a resource is refused with the
/// service's fixed text for that documented constraint — the same string however the
/// emptiness is expressed, whatever the policy type, and whatever the document says.
#[tokio::test]
async fn an_access_object_with_neither_actions_nor_resources_carries_the_fixed_text() {
    let emulator = Emulator::new();

    // The rows differ along HOW the entry names neither dimension (both members present
    // and empty, one present and empty, neither present at all), along the policy type,
    // and along the document the check was given. The first row is the recorded case.
    let empty_entries: &[(&str, Value)] = &[
        ("both members present and empty", json!([{"actions": [], "resources": []}])),
        ("only actions, empty", json!([{"actions": []}])),
        ("only resources, empty", json!([{"resources": []}])),
        ("neither member present", json!([{}])),
    ];
    let documents: &[(&str, &str, String)] = &[
        ("IDENTITY_POLICY", "a one-statement identity policy", identity_policy()),
        ("IDENTITY_POLICY", "a two-statement identity policy", wide_identity_policy()),
        ("RESOURCE_POLICY", "a resource policy", resource_policy()),
    ];

    for (entry_case, access) in empty_entries {
        for (policy_type, document_case, document) in documents {
            let case = format!("{entry_case} with {document_case} ({policy_type})");
            let reply =
                check_access_not_granted(&emulator, document, policy_type, access.clone()).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: the refusal is HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {:?}",
                reply.code()
            );
            assert_eq!(
                message(&reply),
                ACCESS_OBJECT_EMPTY,
                "case {case:?}: expected the fixed text {ACCESS_OBJECT_EMPTY:?}, got {:?}",
                message(&reply)
            );
            // The constant is about the access object, so nothing of the request — the
            // document, the policy type, a list index — reaches the text.
            for leaked in ["s3:", "sqs:", "arn:", "Statement", policy_type, "[0]", "access["] {
                assert!(
                    !message(&reply).contains(leaked),
                    "case {case:?}: the text must carry no request content, but {:?} \
                     contains {leaked:?}",
                    message(&reply)
                );
            }
            // `fieldList` is where the member a client must correct travels.
            assert_eq!(
                reply
                    .member("fieldList")
                    .and_then(Value::as_array)
                    .map(|fields| fields
                        .iter()
                        .filter_map(|field| field.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()),
                Some(vec!["access"]),
                "case {case:?}: expected fieldList to name access, got {reply:?}"
            );
        }
    }

    // The boundary of the rule: an entry that names ONE dimension is not refused at all.
    for (case, access) in [
        ("one action only", json!([{"actions": ["s3:GetObject"]}])),
        (
            "one resource only",
            json!([{"resources": ["arn:aws:s3:::reports/*"]}]),
        ),
        (
            "both dimensions named",
            json!([{"actions": ["s3:GetObject"], "resources": ["arn:aws:s3:::reports/*"]}]),
        ),
    ] {
        let reply =
            check_access_not_granted(&emulator, &identity_policy(), "IDENTITY_POLICY", access)
                .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a well-formed access object is decided, got {reply:?}"
        );
    }
}

/// RULE 3: the refusal of a direct customer invocation of `CreateServiceLinkedAnalyzer`
/// carries the service's fixed text — the same string for every caller and every request
/// shape, naming no identity.
#[tokio::test]
async fn the_service_linked_denial_carries_the_fixed_text_for_every_caller() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // The rows differ along the CREDENTIAL the call was signed with (the value the old
    // text interpolated) and along the request's own members.
    let callers = ["AKIDEXAMPLE", "AKIAIOSFODNN7EXAMPLE", "AKIDOTHERCALLER9"];
    let mut bodies: Vec<(String, Value)> = ANALYZER_TYPES
        .iter()
        .map(|analyzer_type| {
            (
                format!("type only: {analyzer_type}"),
                json!({"type": analyzer_type}),
            )
        })
        .collect();
    bodies.push((
        "with an unused-access configuration".to_string(),
        json!({
            "type": "ACCOUNT_UNUSED_ACCESS",
            "configuration": {"unusedAccess": {"unusedAccessAge": 90}},
        }),
    ));
    bodies.push((
        "with a client token".to_string(),
        json!({"type": "ACCOUNT_UNUSED_ACCESS", "clientToken": "token-1"}),
    ));

    for caller in callers {
        for (body_case, body) in &bodies {
            let case = format!("{body_case} signed by {caller}");
            let reply = emulator
                .send_as(caller, "PUT", "/service-linked-analyzer", Some(body.clone()))
                .await;
            assert_eq!(
                reply.status, 403,
                "case {case:?}: a customer invocation is HTTP 403, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "AccessDeniedException",
                "case {case:?}: expected the modelled AccessDeniedException, got {:?}",
                reply.code()
            );
            assert_eq!(
                message(&reply),
                SERVICE_LINKED_DENIED,
                "case {case:?}: expected the fixed text {SERVICE_LINKED_DENIED:?}, got {:?}",
                message(&reply)
            );
            // Nothing environment-specific is in the text: not the caller, not an ARN,
            // not the account id, not the Region.
            for leaked in [caller, "arn:", "iam", account, REGION] {
                assert!(
                    !message(&reply).contains(leaked),
                    "case {case:?}: the text must name no environment value, but {:?} \
                     contains {leaked:?}",
                    message(&reply)
                );
            }
        }
    }

    // Nothing was created by any of those calls.
    let listed = emulator.list_analyzers("").await;
    let analyzers = listed
        .member("analyzers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert!(
        analyzers.is_empty(),
        "a refused create must store nothing, got {analyzers:?}"
    );
}

/// RULE 4: a request asking for something the resource it names does not support is
/// refused with the service's fixed text for an unsupported action — the same string for
/// `UpdateAnalyzer` against an analyzer whose type has no configuration, for
/// `StartResourceScan` against one that does, and whatever the request carried.
#[tokio::test]
async fn an_unsupported_action_carries_the_fixed_text() {
    let emulator = Emulator::new();

    let bodies: &[(&str, Value)] = &[
        (
            "an unused-access configuration",
            json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}}),
        ),
        (
            "an unused-access analysis rule",
            json!({"configuration": {"unusedAccess": {"analysisRule": {"exclusions": [
                {"resourceTags": [{"k": "v"}]}
            ]}}}}),
        ),
        (
            "an internal-access configuration",
            json!({"configuration": {"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::S3::Bucket"]}
            ]}}}}),
        ),
        ("no configuration at all", json!({})),
    ];

    for analyzer_type in EXTERNAL_ACCESS_TYPES {
        assert!(
            ANALYZER_TYPES.contains(analyzer_type),
            "{analyzer_type:?} must be one of the modelled types {ANALYZER_TYPES:?}"
        );
        let name = format!("external_{}", analyzer_type.to_lowercase());
        let created = emulator
            .create_analyzer(json!({"analyzerName": name, "type": analyzer_type}))
            .await;
        assert_eq!(
            created.status, 200,
            "the fixture create of {name:?} must succeed, got {created:?}"
        );

        for (body_case, body) in bodies {
            let case = format!("{analyzer_type} with {body_case}");
            let reply = emulator.update_analyzer(&name, body.clone()).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: the refusal is HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {:?}",
                reply.code()
            );
            assert_eq!(
                message(&reply),
                UNSUPPORTED_OPERATION,
                "case {case:?}: expected the fixed text {UNSUPPORTED_OPERATION:?}, got {:?}",
                message(&reply)
            );
            // The text is about the condition, not about this request: neither the
            // operation, nor the analyzer, nor its type is in it.
            for leaked in ["UpdateAnalyzer", name.as_str(), analyzer_type] {
                assert!(
                    !message(&reply).contains(leaked),
                    "case {case:?}: the text must carry no request content, but {:?} \
                     contains {leaked:?}",
                    message(&reply)
                );
            }
        }
    }

    // The same rule on the OTHER operation that raises an unsupported-action refusal:
    // `StartResourceScan` is supported only for the external-access types, so it refuses
    // the complementary family with the identical text.
    for analyzer_type in ["ACCOUNT_UNUSED_ACCESS", "ORGANIZATION_INTERNAL_ACCESS"] {
        let name = format!("scan_{}", analyzer_type.to_lowercase());
        let created = emulator
            .create_analyzer(json!({"analyzerName": name, "type": analyzer_type}))
            .await;
        assert_eq!(
            created.status, 200,
            "the fixture create of {name:?} must succeed, got {created:?}"
        );
        let case = format!("StartResourceScan against {analyzer_type}");
        let reply = emulator
            .start_resource_scan(json!({
                "analyzerArn": expected_arn(&name),
                "resourceArn": "arn:aws:s3:::scan-target",
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the refusal is HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?}",
            reply.code()
        );
        assert_eq!(
            message(&reply),
            UNSUPPORTED_OPERATION,
            "case {case:?}: expected the fixed text {UNSUPPORTED_OPERATION:?}, got {:?}",
            message(&reply)
        );
        for leaked in ["StartResourceScan", name.as_str(), analyzer_type] {
            assert!(
                !message(&reply).contains(leaked),
                "case {case:?}: the text must carry no request content, but {:?} contains \
                 {leaked:?}",
                message(&reply)
            );
        }
    }

    // The positive control: the same call against a CONFIGURED type succeeds, and what it
    // stored reads back through `GetAnalyzer`.
    let configured = json!({"unusedAccess": {"unusedAccessAge": 45}});
    let created = emulator
        .create_analyzer(json!({"analyzerName": "configured", "type": "ACCOUNT_UNUSED_ACCESS"}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture create must succeed, got {created:?}"
    );
    let updated = emulator
        .update_analyzer("configured", json!({"configuration": configured.clone()}))
        .await;
    assert_eq!(
        updated.status, 200,
        "a configured analyzer must accept the update, got {updated:?}"
    );
    let read_back = emulator.get_analyzer("configured").await;
    assert_eq!(
        read_back
            .member("analyzer")
            .and_then(|analyzer| analyzer.get("configuration")),
        Some(&configured),
        "the update must be readable back, got {read_back:?}"
    );
}
