// SPDX-License-Identifier: CC0-1.0

//! The ACTION-GROUP rule, as the ONE place this service states it
//! ([`state::validate_action_group`]) and as both policy writes answer from it.
//!
//! The rule every case here asserts: a resource-based policy holds a statement only for
//! an action group whose permissions this service can name, so `PutPermission` and
//! `RemovePermission` both accept exactly the grantable set and both refuse anything
//! else with the SAME sentence — the one the shared helper words, naming the allowed set
//! and the value the request carried.
//!
//! Rows vary what the rule generalises over: which operation the value reaches (both
//! policy writes), and the FORM of the value — every value the model's `ActionGroup`
//! enum declares, and non-grantable values that differ in case, in surrounding
//! whitespace, in being empty, in being a prefix or a suffix of a modelled value, and in
//! naming an API action instead of a group. The grantable set itself is read from the
//! model's own enum rather than written out here, so a value the issue never cited is
//! covered as soon as the model declares it.
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness and the policy senders [`crate::permission_writes`] already builds, not a fork
//! of either.

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::ops::types::ACTION_GROUP_VALUES;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state;

use crate::interval_validation::create_group;
use crate::permission_writes::{get_policy, put_permission, remove_permission, string_member};
use crate::profiling_group_lifecycle::{ACCOUNT, Harness};

/// The group every row writes its policy on, created through the API like any other.
const GROUP: &str = "action-group-rule";

/// A well-formed `RevisionId` the store never minted, for the removals that are refused
/// before any revision is looked at.
const FOREIGN_REVISION: &str = "00000000-0000-4000-8000-000000000000";

/// The principals one row grants to: the ARNs of an IAM role and an IAM user, the two
/// forms the `principals` doc names ("A list ARNs for the roles and users you want to
/// grant access to the profiling group").
fn principals(label: &str) -> Vec<String> {
    vec![
        format!("arn:aws:iam::{ACCOUNT}:role/{label}-agent"),
        format!("arn:aws:iam::{ACCOUNT}:user/{label}-operator"),
    ]
}

/// The `Principal.AWS` block a statement granting exactly `granted` carries: an IAM
/// policy document writes ONE principal as the ARN itself and several as a list.
fn expected_principal(granted: &[String]) -> serde_json::Value {
    match granted {
        [one] => serde_json::Value::from(one.clone()),
        several => serde_json::Value::from(several.to_vec()),
    }
}

/// Whether `value` is a URI path segment a request can carry `actionGroup` in at all:
/// `actionGroup` is bound to the path, so an empty or whitespace-carrying value names no
/// route for a handler to refuse.
fn is_path_segment(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character.is_ascii_alphanumeric())
}

/// One `actionGroup` value a request can carry, and whether the rule admits it.
#[derive(Debug)]
struct Case {
    /// What makes this row its own case, carried into every assertion's message.
    #[allow(dead_code)]
    label: &'static str,
    value: String,
    grantable: bool,
}

/// The table. The accepted rows are the model's own enum values (so the catalogue is
/// never a hand-written copy of it); the refused rows are values outside it, differing in
/// the form that makes them not a group this service can grant.
fn cases() -> Vec<Case> {
    // Source: the model's `ActionGroup` enum, as the generated `ACTION_GROUP_VALUES`
    // lists it (`smithy_query(query="shape",shape="ActionGroup")`).
    let mut cases: Vec<Case> = ACTION_GROUP_VALUES
        .iter()
        .map(|value| Case {
            label: "a value the model's ActionGroup enum declares",
            value: (*value).to_string(),
            grantable: true,
        })
        .collect();
    let modelled = ACTION_GROUP_VALUES
        .first()
        .expect("the model declares at least one action group");
    for (label, value) in [
        ("an action group this service does not know", "bogusPermissions".to_string()),
        ("the empty value", String::new()),
        ("a modelled value upper-cased", modelled.to_uppercase()),
        ("a modelled value with trailing whitespace", format!("{modelled} ")),
        ("a modelled value with leading whitespace", format!(" {modelled}")),
        ("a prefix of a modelled value", modelled[..modelled.len() - 1].to_string()),
        ("a modelled value with a suffix", format!("{modelled}2")),
        ("an API action instead of a group", "ConfigureAgent".to_string()),
    ] {
        cases.push(Case {
            label,
            value,
            grantable: false,
        });
    }
    cases
}

#[test]
fn the_action_group_rule_admits_the_modelled_set_and_states_one_sentence_for_the_rest() {
    let grantable = state::grantable_action_groups();
    // The predicate and the sentence read the ONE list: a modelled value is grantable
    // exactly when this service can name the permissions it grants.
    for value in ACTION_GROUP_VALUES {
        let has_actions = !state::action_group_actions(value).is_empty();
        assert_eq!(
            grantable.contains(value),
            has_actions,
            "case {value:?}: a modelled action group is grantable exactly when the \
             service names its actions, got grantable={} actions={:?}",
            grantable.contains(value),
            state::action_group_actions(value)
        );
    }

    let expected_refusal = |value: &str| {
        format!(
            "actionGroup must be one of {}, got {value}",
            grantable.join(", ")
        )
    };

    for case in cases() {
        let answer = state::validate_action_group(&case.value);
        if case.grantable {
            assert_eq!(
                answer,
                Ok(()),
                "case {case:?}: a grantable action group must be admitted, got {answer:?}"
            );
            continue;
        }
        let expected = expected_refusal(&case.value);
        assert_eq!(
            answer,
            Err(expected.clone()),
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn both_policy_writes_answer_from_the_shared_action_group_rule() {
    let harness = Harness::new();
    create_group(&harness, GROUP).await;

    // The ACCEPT side of the shared path, driven through BOTH operations that take an
    // `actionGroup`: every grantable group is written, read back and removed again. Each
    // write names the revision the stored policy carries, which is none at the start of
    // every row — the previous row's removal took its policy's last statement and so
    // deleted the policy.
    let mut current: Option<String> = None;
    for action_group in state::grantable_action_groups() {
        let granted = principals(action_group);
        let written =
            put_permission(&harness, GROUP, action_group, &granted, current.as_deref()).await;
        assert_eq!(
            written.status,
            200,
            "case {action_group:?}: a grantable action group must be written, got {} {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        let revision = string_member(&written, "revisionId");

        let read_back = get_policy(&harness, GROUP).await;
        assert_eq!(
            string_member(&read_back, "policy"),
            string_member(&written, "policy"),
            "case {action_group:?}: GetPolicy must report the document the write stored"
        );
        let document: serde_json::Value = serde_json::from_str(&string_member(&read_back, "policy"))
            .unwrap_or_else(|error| panic!("case {action_group:?}: policy must be JSON: {error}"));
        let sid = state::action_group_statement_id(action_group);
        let statement = document["Statement"]
            .as_array()
            .and_then(|statements| {
                statements
                    .iter()
                    .find(|statement| statement["Sid"].as_str() == Some(sid.as_str()))
            })
            .unwrap_or_else(|| {
                panic!("case {action_group:?}: no statement for the action group in {document}")
            })
            .clone();
        assert_eq!(
            statement["Action"],
            serde_json::Value::from(state::action_group_actions(action_group)),
            "case {action_group:?}: the statement must grant the action group's actions, got {}",
            statement["Action"]
        );
        assert_eq!(
            statement["Principal"]["AWS"],
            expected_principal(&granted),
            "case {action_group:?}: the statement must grant to the submitted principals, got {}",
            statement["Principal"]["AWS"]
        );

        let removed = remove_permission(&harness, GROUP, action_group, &revision).await;
        assert_eq!(
            removed.status,
            200,
            "case {action_group:?}: the same action group must be removable, got {} {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        // The statement is gone; it was the policy's only one, so the policy went with
        // it and the group is policy-less again — so the removal itself reports neither
        // member, which is what the reader reports too, and why the next grantable
        // action group is written as a FIRST write.
        assert_eq!(
            removed.json(),
            serde_json::json!({}),
            "case {action_group:?}: the removal took the policy's last statement, so it \
             must report neither policy nor revisionId, got {}",
            String::from_utf8_lossy(&removed.body)
        );
        let after = get_policy(&harness, GROUP).await;
        assert_eq!(
            after.json(),
            serde_json::json!({}),
            "case {action_group:?}: the removal took the policy's last statement, so \
             GetPolicy must report neither policy nor revisionId, got {}",
            String::from_utf8_lossy(&after.body)
        );
        assert!(
            !String::from_utf8_lossy(&removed.body).contains(sid.as_str()),
            "case {action_group:?}: the removed action group must hold no statement, got {}",
            String::from_utf8_lossy(&removed.body)
        );
        current = None;
    }

    // Whatever the accept side left stored, no refusal below may change it.
    let before_refusals = get_policy(&harness, GROUP).await;

    // The REFUSE side, over HTTP: a value the rule does not admit is refused by both
    // writes with HTTP 400 `ValidationException`, and nothing is stored for it — whether
    // the sentence is the shared helper's or the boundary walk's own for the closed enum.
    // Only the rows that are a path segment at all are sent: `actionGroup` is bound to
    // the URI, so an empty or whitespace-carrying value names no route to refuse.
    for case in cases()
        .into_iter()
        .filter(|case| !case.grantable && is_path_segment(&case.value))
    {
        for (operation, answer) in [
            (
                "PutPermission",
                put_permission(
                    &harness,
                    GROUP,
                    &case.value,
                    &principals("refused"),
                    None,
                )
                .await,
            ),
            (
                "RemovePermission",
                remove_permission(&harness, GROUP, &case.value, FOREIGN_REVISION).await,
            ),
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {operation} {case:?}: expected HTTP 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {operation} {case:?}: expected ValidationException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
        assert_eq!(
            get_policy(&harness, GROUP).await.body,
            before_refusals.body,
            "case {case:?}: a refused write must store nothing"
        );
    }
}
