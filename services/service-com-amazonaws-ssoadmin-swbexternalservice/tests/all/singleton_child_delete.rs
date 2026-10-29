// SPDX-License-Identifier: CC0-1.0

//! The SINGLETON-CHILD deletes of this service, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: a delete of a singleton child of a parent resource answers a child
//! that is not there with whatever the child's OWN reader answers for it. Where the reader
//! reports the absence as a plain 200 with its optional output member left out, carrying no
//! child is a STATE of the parent rather than a missing resource, so the delete is
//! IDEMPOTENT — the same empty 200 whether or not a child was there. Where the reader
//! refuses the absence with the modelled `ResourceNotFoundException`, the child is a
//! resource of its own, so the delete refuses the same way. Either way the modelled
//! `ResourceNotFoundException` of the delete is never withheld from a parent the request
//! names and nothing is held under.
//!
//! The rule is asserted over a TABLE spanning the whole class it generalises over: every
//! singleton child this service gives a reader and a deleter of its own, both of the parents
//! that carry one (a permission set and an application), both kinds of reader answer for an
//! absent child, children keyed by a member of the request and children the parent holds at
//! most one of outright, and the orders a caller can call them in (a delete with no write
//! before it, a delete after a write, a repeated delete). The expectation of each row is
//! DERIVED from the reader of the same child in the same test, so the two cannot drift
//! apart.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness and the parent-creating helpers are the ones the
//! suites of each slice already own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_authentication_methods::{create_application, IAM};
use crate::inline_policy::create_permission_set;
use crate::ssoadmin_permission_sets::{
    assert_refused, call, organization_router, router, seeded_instance_arn,
};

/// The parent resources this service gives singleton children to. A permission set is a
/// feature of an ORGANIZATION instance, an application of the account's own instance, so
/// the two rows take different fixtures.
#[derive(Clone, Copy, Debug)]
enum Parent {
    PermissionSet,
    Application,
}

/// What a child's own READER answers for a child that is not there.
#[derive(Clone, Copy, Debug)]
enum Absent {
    /// A plain 200 with the reader's optional output member left out.
    ReportedByA200,
    /// The modelled `ResourceNotFoundException`.
    RefusedNotFound,
}

/// One row: a singleton child, the three operations that write, read and delete it, and the
/// answer its reader gives for it when the parent carries none.
struct ChildCase {
    label: &'static str,
    parent: Parent,
    reader: &'static str,
    deleter: &'static str,
    writer: &'static str,
    /// The members that NAME the child inside its parent, merged into every request of the
    /// row — empty for a child the parent holds at most one of outright.
    key: Value,
    /// The member the writer carries the child in, with a value for it; `None` for a write
    /// that names the child and nothing else.
    written: Option<(&'static str, Value)>,
    /// The member the reader reports the child in.
    reported: &'static str,
    absent: Absent,
    /// The model fact `absent` follows from.
    source: &'static str,
}

/// The table: every singleton child of this service that has a reader and a deleter of its
/// own.
///
/// SOURCE of each `absent` and `source`:
/// `smithy_query(query="operation",operation="GetInlinePolicyForPermissionSet,GetPermissionsBoundaryForPermissionSet,GetApplicationAccessScope,GetApplicationAuthenticationMethod,GetApplicationGrant",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
fn child_cases() -> Vec<ChildCase> {
    vec![
        ChildCase {
            label: "the inline policy of a permission set",
            parent: Parent::PermissionSet,
            reader: "GetInlinePolicyForPermissionSet",
            deleter: "DeleteInlinePolicyFromPermissionSet",
            writer: "PutInlinePolicyToPermissionSet",
            key: json!({}),
            written: Some((
                "InlinePolicy",
                json!("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
            )),
            reported: "InlinePolicy",
            absent: Absent::ReportedByA200,
            source: "the reader models InlinePolicy as an optional output member",
        },
        ChildCase {
            label: "the permissions boundary of a permission set",
            parent: Parent::PermissionSet,
            reader: "GetPermissionsBoundaryForPermissionSet",
            deleter: "DeletePermissionsBoundaryFromPermissionSet",
            writer: "PutPermissionsBoundaryToPermissionSet",
            key: json!({}),
            written: Some((
                "PermissionsBoundary",
                json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess"}),
            )),
            reported: "PermissionsBoundary",
            absent: Absent::ReportedByA200,
            source: "the reader models PermissionsBoundary as an optional output member",
        },
        ChildCase {
            label: "an access scope of an application",
            parent: Parent::Application,
            reader: "GetApplicationAccessScope",
            deleter: "DeleteApplicationAccessScope",
            writer: "PutApplicationAccessScope",
            // A scope name the issue did not cite, of the form the model's `Scope` pattern
            // admits (`^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}…`).
            key: json!({"Scope": "user_impersonation"}),
            written: None,
            reported: "Scope",
            absent: Absent::RefusedNotFound,
            source: "the reader models Scope as a REQUIRED output member",
        },
        ChildCase {
            label: "an authentication method of an application",
            parent: Parent::Application,
            reader: "GetApplicationAuthenticationMethod",
            deleter: "DeleteApplicationAuthenticationMethod",
            writer: "PutApplicationAuthenticationMethod",
            key: json!({"AuthenticationMethodType": IAM}),
            written: Some((
                "AuthenticationMethod",
                json!({"Iam": {"ActorPolicy": {"Version": "2012-10-17", "Statement": []}}}),
            )),
            reported: "AuthenticationMethod",
            absent: Absent::RefusedNotFound,
            source: "the reader refuses a type the application carries no method for",
        },
        ChildCase {
            label: "a grant of an application",
            parent: Parent::Application,
            reader: "GetApplicationGrant",
            deleter: "DeleteApplicationGrant",
            writer: "PutApplicationGrant",
            // A `GrantType` the model's enum carries, whose `Grant` union variant takes no
            // configuration member.
            key: json!({"GrantType": "refresh_token"}),
            written: Some(("Grant", json!({"RefreshToken": {}}))),
            reported: "Grant",
            absent: Absent::RefusedNotFound,
            source: "the reader models Grant as a REQUIRED output member",
        },
    ]
}

/// One request body: the members naming the parent, the members naming the child, and — for
/// a write — the member carrying the child.
fn request(parent: &Value, key: &Value, written: Option<&(&'static str, Value)>) -> Value {
    let mut members = parent.as_object().cloned().unwrap_or_default();
    if let Some(key) = key.as_object() {
        for (member, value) in key {
            members.insert(member.clone(), value.clone());
        }
    }
    if let Some((member, value)) = written {
        members.insert((*member).to_string(), value.clone());
    }
    Value::Object(members)
}

/// A router and the members naming a freshly created parent of the row's kind.
async fn parent_fixture(case: &ChildCase, index: usize) -> (MultiServiceRouter, Value) {
    match case.parent {
        Parent::PermissionSet => {
            let (router, instance_arn) = organization_router();
            let permission_set_arn =
                create_permission_set(&router, &instance_arn, &format!("Singleton{index}")).await;
            (
                router,
                json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
            )
        }
        Parent::Application => {
            let router = router();
            let instance_arn = seeded_instance_arn(&router).await;
            let application_arn =
                create_application(&router, &instance_arn, &format!("singleton-{index}")).await;
            (router, json!({"ApplicationArn": application_arn}))
        }
    }
}

/// Assert one answer for a child that is NOT there is the one the row's reader gives.
fn assert_absent_answer(case: &ChildCase, what: &str, status: u16, body: &Value) {
    match case.absent {
        Absent::ReportedByA200 => {
            assert_eq!(
                status, 200,
                "case {:?}: {what} must answer 200 because {}, got {status} {body}",
                case.label, case.source
            );
        }
        Absent::RefusedNotFound => assert_refused(
            case.label,
            what,
            "ResourceNotFoundException",
            status,
            body,
        ),
    }
}

/// THE RULE: for every singleton child, the delete of a child that is not there answers
/// exactly what the child's own reader answers for it — an empty 200 where the reader
/// reports the absence with a 200, the modelled `ResourceNotFoundException` where the reader
/// refuses it.
#[tokio::test]
async fn a_delete_answers_an_absent_child_the_way_its_reader_does() {
    for (index, case) in child_cases().into_iter().enumerate() {
        let (router, parent) = parent_fixture(&case, index).await;
        let naming_the_child = request(&parent, &case.key, None);

        // The reader's answer for a child that was never written, which the row's `absent`
        // declares and every assertion below is taken from.
        let (status, read) = call(&router, case.reader, &naming_the_child).await;
        assert_absent_answer(&case, "the reader of a child never written", status, &read);
        if matches!(case.absent, Absent::ReportedByA200) {
            assert_eq!(
                read[case.reported],
                Value::Null,
                "case {:?}: the reader must report no {} for a parent carrying none, got {read}",
                case.label,
                case.reported
            );
        }

        // THE RULE, on a child that was never written.
        let (status, deleted) = call(&router, case.deleter, &naming_the_child).await;
        assert_absent_answer(&case, "the delete of a child never written", status, &deleted);
        if matches!(case.absent, Absent::ReportedByA200) {
            assert_eq!(
                deleted,
                json!({}),
                "case {:?}: the delete models no output members, got {deleted}",
                case.label
            );
        }

        // Write the child, so the delete has one to remove.
        let (status, body) = call(
            &router,
            case.writer,
            &request(&parent, &case.key, case.written.as_ref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: {} expected 200, got {status} {body}",
            case.label, case.writer
        );
        let (status, stored) = call(&router, case.reader, &naming_the_child).await;
        assert_eq!(
            status, 200,
            "case {:?}: the reader of a written child expected 200, got {status} {stored}",
            case.label
        );
        assert!(
            !stored[case.reported].is_null(),
            "case {:?}: the reader must report the written {}, got {stored}",
            case.label,
            case.reported
        );

        // A delete that DOES remove a child is a success for every row.
        let (status, body) = call(&router, case.deleter, &naming_the_child).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete of a written child expected 200, got {status} {body}",
            case.label
        );

        // And afterwards the parent carries none again: the reader says so the way it did
        // before the write, and so does a repeated delete.
        let (status, after) = call(&router, case.reader, &naming_the_child).await;
        assert_absent_answer(&case, "the reader after the delete", status, &after);
        let (status, again) = call(&router, case.deleter, &naming_the_child).await;
        assert_absent_answer(&case, "a repeated delete", status, &again);
        assert_eq!(
            again, deleted,
            "case {:?}: a repeated delete must answer what the delete of a child never \
             written answered ({deleted}), got {again}",
            case.label
        );
    }
}

/// The consequence of the rule a caller depends on: an unconditional CLEANUP over a parent —
/// drop every singleton child it may carry, without asking first — succeeds on every child
/// whose reader reports absence with a 200, however many times it is run, and the parent
/// survives it.
#[tokio::test]
async fn an_unconditional_cleanup_of_the_reported_children_succeeds_however_often_it_runs() {
    for (index, case) in child_cases()
        .into_iter()
        .filter(|case| matches!(case.absent, Absent::ReportedByA200))
        .enumerate()
    {
        let (router, parent) = parent_fixture(&case, 100 + index).await;
        let naming_the_child = request(&parent, &case.key, None);

        // Rows differ along the ORDER of the calls: how many deletes run in the sweep, and
        // whether the child was ever written before it.
        for (written, sweeps) in [(false, 1), (false, 3), (true, 2), (true, 4)] {
            if written {
                let (status, body) = call(
                    &router,
                    case.writer,
                    &request(&parent, &case.key, case.written.as_ref()),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?} (written {written}, sweeps {sweeps}): {} expected 200, got \
                     {status} {body}",
                    case.label, case.writer
                );
            }
            for sweep in 0..sweeps {
                let (status, body) = call(&router, case.deleter, &naming_the_child).await;
                assert_eq!(
                    status, 200,
                    "case {:?} (written {written}, sweep {sweep}): the delete expected 200 \
                     because {}, got {status} {body}",
                    case.label, case.source
                );
                let (status, after) = call(&router, case.reader, &naming_the_child).await;
                assert_eq!(
                    status, 200,
                    "case {:?} (written {written}, sweep {sweep}): the reader expected 200, \
                     got {status} {after}",
                    case.label
                );
                assert_eq!(
                    after[case.reported],
                    Value::Null,
                    "case {:?} (written {written}, sweep {sweep}): the parent must carry no \
                     {} after the delete, got {after}",
                    case.label,
                    case.reported
                );
            }
        }

        // The parent itself is not touched by the sweep: it still takes a write of the
        // child afterwards.
        let (status, body) = call(
            &router,
            case.writer,
            &request(&parent, &case.key, case.written.as_ref()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: a write after the sweep expected 200, got {status} {body}",
            case.label
        );
    }
}
