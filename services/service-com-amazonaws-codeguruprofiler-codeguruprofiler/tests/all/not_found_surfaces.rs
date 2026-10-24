// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a request gets when the resource it names is not there, end to end over
//! HTTP through the router this service mounts in production.
//!
//! Two rules are asserted here, each over a table whose rows vary what the rule
//! generalises over rather than reproducing one recorded request:
//!
//! 1. An operation that resolves a profiling group BY NAME words its not-found in its
//!    OWN terms, so a caller can tell WHICH lookup failed from the message alone: the
//!    operations whose refusal is account-qualified state the account the lookup was
//!    scoped to together with the member they looked the group up by, and no two of these
//!    operations state the same sentence for the same name.
//! 2. The ARN-addressed TAGGING surface is a different surface: an ARN it holds no live
//!    resource for is refused with that surface's own error CODE (`NotFoundException`)
//!    and its own fixed sentence, not with the `ResourceNotFoundException` the
//!    profiling-group operations answer.
//!
//! The status and the error code are asserted in every case too, so a message can never
//! be right on its own.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds and the
//! `@httpLabel` encoding is the one [`crate::untag_resource`] already applies.

use crate::profiling_group_lifecycle::{group_arn, Answer, Harness, ACCOUNT, REGION};
use crate::untag_resource::encode;

/// The fixed sentence the tagging surface reports a resource that is not there with
/// (`state::TAGGING_RESOURCE_NOT_FOUND`), restated here so this suite pins the wire text
/// rather than importing the value it compares against.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// Profiling-group names the store does not hold, varying in length and character class
/// and all inside the modelled `ProfilingGroupName` constraints (len 1..255, pattern
/// `^[\w-]+$`), so every request reaches the handler rather than the boundary validator.
fn missing_names() -> Vec<String> {
    vec![
        "never-created".to_string(),
        "z".to_string(),
        "Absent_1".to_string(),
        "a-b-c".to_string(),
        "g".repeat(255),
    ]
}

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

/// One group-scoped operation, as a request over a name and the sentence its refusal must
/// state for that name.
struct NameLookup {
    operation: &'static str,
    method: &'static str,
    /// The URI, built from the name the request names.
    uri: fn(&str) -> String,
    /// The request body, for the operations that carry one.
    body: fn() -> Vec<u8>,
    /// The sentence this operation's refusal must carry for that name.
    message: fn(&str) -> String,
}

/// RULE 1 over every implemented operation that resolves a group by name and has a
/// recorded sentence of its own.
fn name_lookups() -> Vec<NameLookup> {
    vec![
        NameLookup {
            operation: "DescribeProfilingGroup",
            method: "GET",
            uri: |name| format!("/profilingGroups/{name}"),
            body: Vec::new,
            // The sentence every operation with no recorded form of its own states.
            message: |name| {
                format!(
                    "CodeGuru Profiler can\u{2019}t complete the operation because {name} was \
                     not found."
                )
            },
        },
        NameLookup {
            operation: "UpdateProfilingGroup",
            method: "PUT",
            uri: |name| format!("/profilingGroups/{name}"),
            body: || {
                serde_json::to_vec(&serde_json::json!({
                    "agentOrchestrationConfig": { "profilingEnabled": true }
                }))
                .expect("a serializable body")
            },
            // The lookup it performed (the account it was scoped to, the member it looked
            // the group up by) AND the operation that could not be carried out.
            message: |name| {
                format!(
                    "Unable to find profiling group with accountId: {ACCOUNT}; \
                     profilingGroupName: {name} during update"
                )
            },
        },
        NameLookup {
            operation: "GetPolicy",
            method: "GET",
            uri: |name| format!("/profilingGroups/{name}/policy"),
            body: Vec::new,
            // The lookup it performed, naming the account it was scoped to.
            message: |name| {
                format!("No profiling group with accountId: {ACCOUNT}; name: {name} was found.")
            },
        },
        NameLookup {
            operation: "ConfigureAgent",
            method: "POST",
            uri: |name| format!("/profilingGroups/{name}/configureAgent"),
            body: || b"{}".to_vec(),
            // The AGENT plane's own account-qualified sentence.
            message: |name| format!("Profiling group for {ACCOUNT}/{name} does not exist!"),
        },
        NameLookup {
            operation: "DeleteProfilingGroup",
            method: "DELETE",
            uri: |name| format!("/profilingGroups/{name}"),
            body: Vec::new,
            // The refusal about the group RECORD itself.
            message: |name| format!("ProfilingGroup (name: {name}, accountId: {ACCOUNT}) not found."),
        },
    ]
}

#[tokio::test]
async fn each_group_lookup_states_its_own_not_found_sentence() {
    let harness = Harness::new();
    // One group that DOES exist, so no row can be passing because the store is empty.
    assert_eq!(
        create_group(&harness, "present").await.status,
        201,
        "the one existing group must be created"
    );

    for name in missing_names() {
        let mut seen: Vec<(&str, String)> = Vec::new();
        for lookup in name_lookups() {
            let case = format!("{} {name:?}", lookup.operation);
            let answer = harness
                .send(
                    lookup.method,
                    &(lookup.uri)(&name),
                    "application/json",
                    (lookup.body)(),
                )
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected HTTP 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected = (lookup.message)(&name);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
            seen.push((lookup.operation, answer.error_message()));
        }

        // No two of these operations say the same thing about the same name: that is what
        // lets a client tell which lookup failed.
        for (index, (operation, message)) in seen.iter().enumerate() {
            for (other_operation, other_message) in seen.iter().skip(index + 1) {
                assert_ne!(
                    message, other_message,
                    "name {name:?}: {operation} and {other_operation} must not state the \
                     same sentence, both said {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_account_qualified_lookups_name_the_account_and_the_member_they_looked_up_by() {
    let harness = Harness::new();

    // The operations whose recorded refusal states the lookup it performed, with the
    // member name that lookup used. Rows differ in the operation and in that member.
    let rows: [(&str, &str, &str, &str, Vec<u8>); 2] = [
        (
            "UpdateProfilingGroup",
            "PUT",
            "profilingGroupName",
            "/profilingGroups/{}",
            serde_json::to_vec(&serde_json::json!({
                "agentOrchestrationConfig": { "profilingEnabled": false }
            }))
            .expect("a serializable body"),
        ),
        (
            "GetPolicy",
            "GET",
            "name",
            "/profilingGroups/{}/policy",
            Vec::new(),
        ),
    ];

    for name in missing_names() {
        for (operation, method, member, uri, body) in &rows {
            let case = format!("{operation} {name:?}");
            let answer = harness
                .send(
                    method,
                    &uri.replace("{}", &name),
                    "application/json",
                    body.clone(),
                )
                .await;
            let message = answer.error_message();
            assert_eq!(
                answer.status, 404,
                "case {case}: expected HTTP 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                message.contains(&format!("accountId: {ACCOUNT}")),
                "case {case}: the refusal must name the account the lookup was scoped to, \
                 got {message:?}"
            );
            assert!(
                message.contains(&format!("{member}: {name}")),
                "case {case}: the refusal must name {member} and the group looked for, got \
                 {message:?}"
            );
        }
    }
}

/// One operation of the ARN-addressed tagging surface, as a request over a resource ARN.
struct TaggingCall {
    operation: &'static str,
    method: &'static str,
    /// The URI, built from the ARN the request addresses.
    uri: fn(&str) -> String,
    body: fn() -> Vec<u8>,
}

fn tagging_calls() -> Vec<TaggingCall> {
    vec![
        TaggingCall {
            operation: "TagResource",
            method: "POST",
            uri: |arn| format!("/tags/{}", encode(arn)),
            body: || {
                serde_json::to_vec(&serde_json::json!({ "tags": { "env": "dev" } }))
                    .expect("a serializable body")
            },
        },
        TaggingCall {
            operation: "UntagResource",
            method: "DELETE",
            uri: |arn| format!("/tags/{}?tagKeys=env", encode(arn)),
            body: Vec::new,
        },
        TaggingCall {
            operation: "ListTagsForResource",
            method: "GET",
            uri: |arn| format!("/tags/{}", encode(arn)),
            body: Vec::new,
        },
    ]
}

#[tokio::test]
async fn the_tagging_surface_reports_a_missing_resource_in_its_own_terms() {
    let harness = Harness::new();
    assert_eq!(
        create_group(&harness, "present").await.status,
        201,
        "the one existing group must be created"
    );
    assert_eq!(
        create_group(&harness, "doomed").await.status,
        201,
        "the group to delete must be created"
    );
    assert_eq!(
        harness
            .send("DELETE", "/profilingGroups/doomed", "application/json", Vec::new())
            .await
            .status,
        204,
        "the delete must succeed"
    );

    // Rows: every way a WELL-FORMED profiling-group ARN of this service can address no
    // live resource — a name never created, a name whose group was deleted, the longest
    // name the model allows, and an existing name in another account, Region or partition
    // (an ARN identifies ONE resource).
    let arns: Vec<(&str, String)> = vec![
        ("a group that was never created", group_arn("never-created")),
        ("a group that was deleted", group_arn("doomed")),
        (
            "the longest name the model allows",
            group_arn(&"z".repeat(255)),
        ),
        (
            "an existing name in another account",
            format!("arn:aws:codeguru-profiler:{REGION}:210987654321:profilingGroup/present"),
        ),
        (
            "an existing name in another Region",
            format!("arn:aws:codeguru-profiler:eu-west-1:{ACCOUNT}:profilingGroup/present"),
        ),
        (
            "an existing name in another partition",
            format!("arn:aws-cn:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/present"),
        ),
    ];

    for (label, arn) in &arns {
        for call in tagging_calls() {
            let case = format!("{} {label}", call.operation);
            let answer = harness
                .send(
                    call.method,
                    &(call.uri)(arn),
                    "application/json",
                    (call.body)(),
                )
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected HTTP 404, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case}: expected the tagging surface's code NotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_message(),
                RESOURCE_NOT_FOUND,
                "case {case}: expected {RESOURCE_NOT_FOUND:?}, got {:?}",
                answer.error_message()
            );
        }
    }

    // The SAME absent name, asked about through a profiling-group operation, is the
    // modelled 404 instead: the two surfaces report the same absence with different codes,
    // and that is what a client branches on.
    let described = harness.describe("never-created").await;
    assert_eq!(
        described.status, 404,
        "describing an absent group must answer 404, got {} {}",
        described.status,
        String::from_utf8_lossy(&described.body)
    );
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "the group surface must keep its modelled code, got {}",
        String::from_utf8_lossy(&described.body)
    );

    // Every refusal above stored nothing: the one live resource still carries no tags.
    let listed = harness
        .send(
            "GET",
            &format!("/tags/{}", encode(&group_arn("present"))),
            "application/json",
            Vec::new(),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "the live resource must still read back, got {} {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
    assert_eq!(
        listed.json().get("tags"),
        Some(&serde_json::json!({})),
        "a refused tagging call must store nothing, got {}",
        String::from_utf8_lossy(&listed.body)
    );
}

#[tokio::test]
async fn a_resource_arn_that_names_nothing_is_still_a_bad_input_not_a_missing_resource() {
    let harness = Harness::new();

    // The split this fix must not blur: a string that is not a profiling-group ARN of this
    // service names no resource of ANY kind, which is an INPUT error (400
    // `ValidationException`), while a well-formed ARN with nothing behind it is the 404
    // above. Rows: not an ARN, too few fields, another vendor, another resource type, no
    // name, a name with a path separator.
    for arn in [
        "present",
        "arn:aws:codeguru-profiler:us-east-1:000000000000",
        "arn:aws:codeguru-reviewer:us-east-1:000000000000:profilingGroup/present",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroups/present",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroup/",
        "arn:aws:codeguru-profiler:us-east-1:000000000000:profilingGroup/a/b",
    ] {
        for call in tagging_calls() {
            let case = format!("{} {arn:?}", call.operation);
            let answer = harness
                .send(
                    call.method,
                    &(call.uri)(arn),
                    "application/json",
                    (call.body)(),
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}
