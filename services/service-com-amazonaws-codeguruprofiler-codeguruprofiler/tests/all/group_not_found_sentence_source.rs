// SPDX-License-Identifier: CC0-1.0

//! WHICH not-found sentence the group-scoped WRITES state, end to end over HTTP through
//! the router this service mounts in production.
//!
//! The rule asserted here is the one `crate::state` holds in a single place: an operation
//! ON a profiling group that cannot resolve the name it was given answers 404
//! `ResourceNotFoundException` with the service's shared not-found sentence, which names
//! the group alone, UNLESS that operation has a recorded form of its own — and the
//! notification surface is exactly a surface where the two halves differ, its READ
//! stating the lookup it performed (account and name) while its WRITES state the shared
//! sentence. So "the shared sentence" is the sentence of the operations with no recorded
//! form of their own, never a uniform sentence for a whole surface.
//!
//! The rows drive the four writes that take their sentence from that one shared helper —
//! the notification writes and the two policy writes — over names that differ in length
//! and character class, and over a name that was created and then deleted, so the rule is
//! asserted across operations, name shapes and both ways a name can be absent. The
//! sibling suites pin the operations with a sentence of their own
//! ([`crate::refusal_message_rules`], [`crate::not_found_surfaces`]) and the reads with no
//! sentence of their own ([`crate::refusal_message_formats`]).
//!
//! The harness and the senders are the ones [`crate::profiling_group_lifecycle`],
//! [`crate::notification_channels`] and [`crate::permission_writes`] already build.

use crate::notification_channels::{add, channel_body, create_group};
use crate::permission_writes::{put_permission, remove_permission};
use crate::profiling_group_lifecycle::{ACCOUNT, Answer, Harness};

/// The only value the modelled `ActionGroup` enum declares
/// (`smithy_query(query="shape",shape="ActionGroup")`).
const ACTION_GROUP: &str = "agentPermissions";

/// An SNS topic ARN used as a channel uri: `ChannelUri` is an `@arnReference` documented
/// as "Currently we only support SNS channels and thus required to be an ARN"
/// (`smithy_query(query="shape",shape="Channel")`), so a well-formed one is what reaches
/// the group lookup rather than the uri check.
const TOPIC: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-shared-sentence";

/// A well-formed `ChannelId` and `RevisionId` — both modelled with the pattern
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`
/// (`smithy_query(query="shape",shape="ChannelId")`) — so the boundary validator admits
/// them and the handler's group lookup is what answers.
const CHANNEL_ID: &str = "dddddddd-1111-2222-3333-444444444444";
const REVISION_ID: &str = "eeeeeeee-1111-2222-3333-444444444444";

/// The shared sentence, restated here so this suite pins the wire text rather than
/// importing the value it compares against. The apostrophe is the typographic one
/// (U+2019) the service's own text uses.
fn shared_sentence(name: &str) -> String {
    format!("CodeGuru Profiler can\u{2019}t complete the operation because {name} was not found.")
}

/// The sentence the notification READ states instead: the lookup it performed, naming the
/// account it was scoped to together with the name it looked for.
fn notification_read_sentence(name: &str) -> String {
    format!("Profiling group not found with accountId: {ACCOUNT}, name: {name}.")
}

/// The group-scoped writes whose refusal takes its sentence from the one shared helper.
#[derive(Debug, Clone, Copy)]
enum GroupWrite {
    AddNotificationChannels,
    RemoveNotificationChannel,
    PutPermission,
    RemovePermission,
}

impl GroupWrite {
    fn all() -> [GroupWrite; 4] {
        [
            GroupWrite::AddNotificationChannels,
            GroupWrite::RemoveNotificationChannel,
            GroupWrite::PutPermission,
            GroupWrite::RemovePermission,
        ]
    }
}

/// One of those writes, addressed to `name`, with every member it carries well formed so
/// the group lookup is the only thing that can refuse it.
async fn attempt(harness: &Harness, write: GroupWrite, name: &str) -> Answer {
    match write {
        GroupWrite::AddNotificationChannels => {
            add(harness, name, &[channel_body(None, TOPIC)]).await
        }
        GroupWrite::RemoveNotificationChannel => {
            harness
                .send(
                    "DELETE",
                    &format!("/profilingGroups/{name}/notificationConfiguration/{CHANNEL_ID}"),
                    "application/json",
                    Vec::new(),
                )
                .await
        }
        GroupWrite::PutPermission => {
            let principals = vec![format!("arn:aws:iam::{ACCOUNT}:role/AgentRole")];
            put_permission(harness, name, ACTION_GROUP, &principals, None).await
        }
        GroupWrite::RemovePermission => {
            remove_permission(harness, name, ACTION_GROUP, REVISION_ID).await
        }
    }
}

/// Names no stored group answers to: never created, of the shortest and the longest length
/// the modelled `ProfilingGroupName` allows (len 1..255), and spanning the characters its
/// pattern `^[\w-]+$` allows — plus one name whose group existed and was deleted, added by
/// the test itself.
fn absent_names() -> Vec<String> {
    vec![
        "never-created".to_string(),
        "q".to_string(),
        "Absent_7-name".to_string(),
        "n".repeat(255),
    ]
}

#[tokio::test]
async fn the_group_scoped_writes_state_the_shared_not_found_sentence() {
    let harness = Harness::new();
    // A group that DOES exist, so no row can pass merely because the store is empty, and
    // one that existed and was deleted, so "absent" covers both ways a name gets there.
    create_group(&harness, "present").await;
    create_group(&harness, "doomed").await;
    let deleted = harness
        .send(
            "DELETE",
            "/profilingGroups/doomed",
            "application/json",
            Vec::new(),
        )
        .await;
    assert_eq!(
        deleted.status,
        204,
        "the group to delete must delete, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    let mut names = absent_names();
    names.push("doomed".to_string());

    for name in &names {
        for write in GroupWrite::all() {
            let case = format!("{write:?} {name:?}");
            let answer = attempt(&harness, write, name).await;
            assert_eq!(
                answer.status,
                404,
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
            let expected = shared_sentence(name);
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.error_message()
            );
            // The shared sentence names the group alone: naming the account belongs to
            // the forms the account-qualified operations state.
            assert!(
                !answer.error_message().contains("accountId"),
                "case {case}: the shared sentence must not name an account, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn the_notification_read_states_a_different_sentence_from_the_notification_writes() {
    let harness = Harness::new();
    create_group(&harness, "present").await;

    for name in absent_names() {
        let read = harness
            .send(
                "GET",
                &format!("/profilingGroups/{name}/notificationConfiguration"),
                "application/json",
                Vec::new(),
            )
            .await;
        let expected_read = notification_read_sentence(&name);
        assert_eq!(
            read.status,
            404,
            "case GetNotificationConfiguration {name:?}: expected HTTP 404, got {} {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_message(),
            expected_read,
            "case GetNotificationConfiguration {name:?}: expected {expected_read:?}, got {:?}",
            read.error_message()
        );

        // The two writes of the SAME surface, on the SAME name, state the shared sentence
        // instead — so no surface-wide sentence exists to unify them onto.
        for write in [
            GroupWrite::AddNotificationChannels,
            GroupWrite::RemoveNotificationChannel,
        ] {
            let answer = attempt(&harness, write, &name).await;
            let expected_write = shared_sentence(&name);
            assert_eq!(
                answer.error_message(),
                expected_write,
                "case {write:?} {name:?}: expected {expected_write:?}, got {:?}",
                answer.error_message()
            );
            assert_ne!(
                answer.error_message(),
                read.error_message(),
                "case {write:?} {name:?}: the write and the read of the notification \
                 surface must not state the same sentence, both said {:?}",
                answer.error_message()
            );
        }
    }
}
