// SPDX-License-Identifier: CC0-1.0

//! WHAT a `PutPermissionsBoundaryToPermissionSet` refusal of the documented
//! `PermissionsBoundary` choice SAYS, over the `awsJson1_1` wire.
//!
//! THE RULE under test: a refusal of a documented either/or choice is stated from the
//! members the request actually got wrong, so the two malformed forms the structure's pair
//! of optional members admits earn DIFFERENT detail text — a body that named both kinds is
//! told to drop one, and a body that named neither is told to add one, never that it must
//! "not" name both. Both texts name both halves of the choice
//! (`PermissionsBoundary$CustomerManagedPolicyReference` and
//! `PermissionsBoundary$ManagedPolicyArn`, as
//! `smithy_query(query="shape",shape="PermissionsBoundary",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`
//! spells them), because the caller has to fix the request from this text alone; the
//! refusal carries HTTP 400 `ValidationException` and stores nothing.
//!
//! The text depends on WHICH members the body named and on nothing else, so the table's
//! rows vary everything else: the policy kinds' values, the partition and path forms the
//! `ManagedPolicyArn` pattern admits, the presence of `CustomerManagedPolicyReference$Path`,
//! the published upper bound of `ManagedPolicyName` (1..128), and the ways a body can name
//! NEITHER member (an empty structure, a member the structure does not declare, and the
//! managed-policy member under another spelling). Two rows of the accepted single-kind
//! form bound the class from the other side: naming exactly one kind is not refused.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` error envelope are
//! exercised rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs`
//! owns, and the calls run against an ORGANIZATION instance because the permission-set
//! surface is withdrawn on an account instance.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, organization_router};

/// The detail text a body that named BOTH policy kinds earns: it has to DROP one.
const BOTH_KINDS_MESSAGE: &str = "Specify either CustomerManagedPolicyReference or \
                                  ManagedPolicyArn in PermissionsBoundary, but not both.";

/// The detail text a body that named NEITHER policy kind earns: it has to ADD one, so the
/// clause forbidding both is absent.
const NEITHER_KIND_MESSAGE: &str =
    "Specify either CustomerManagedPolicyReference or ManagedPolicyArn in \
     PermissionsBoundary.";

/// Which half of the choice a row's `PermissionsBoundary` got wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Both `CustomerManagedPolicyReference` and `ManagedPolicyArn` named.
    BothKinds,
    /// Neither named.
    NeitherKind,
}

impl Form {
    /// The detail text this form earns.
    fn message(self) -> &'static str {
        match self {
            Self::BothKinds => BOTH_KINDS_MESSAGE,
            Self::NeitherKind => NEITHER_KIND_MESSAGE,
        }
    }
}

/// One row: a `PermissionsBoundary` the documented choice forbids, and the form it is.
struct RefusedCase {
    label: &'static str,
    sent: Value,
    form: Form,
}

/// The rows. Each form appears several times with different member values, because the
/// refusal depends on which members are present and on nothing else.
fn refused_cases() -> Vec<RefusedCase> {
    // `ManagedPolicyName` is `length(min: 1, max: 128)`; a name AT the upper bound still
    // only makes the row's boundary a both-kinds one.
    let name_at_the_upper_bound = "N".repeat(128);
    vec![
        RefusedCase {
            label: "both kinds, a bare reference name",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
                "CustomerManagedPolicyReference": {"Name": "Both"},
            }),
            form: Form::BothKinds,
        },
        RefusedCase {
            label: "both kinds, a nested path on each",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
                "CustomerManagedPolicyReference": {"Name": "Both", "Path": "/team/"},
            }),
            form: Form::BothKinds,
        },
        RefusedCase {
            // `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy…` admits partition suffixes.
            label: "both kinds, a suffixed partition and a reference name at its bound",
            sent: json!({
                "ManagedPolicyArn": "arn:aws-us-gov:iam::aws:policy/PowerUserAccess",
                "CustomerManagedPolicyReference": {"Name": name_at_the_upper_bound},
            }),
            form: Form::BothKinds,
        },
        RefusedCase {
            label: "neither kind, an empty boundary",
            sent: json!({}),
            form: Form::NeitherKind,
        },
        RefusedCase {
            // A member the structure does not declare names neither half of the choice,
            // so this body is a neither-kind one too.
            label: "neither kind, a member the structure does not declare",
            sent: json!({"PermissionsBoundaryKind": "managed"}),
            form: Form::NeitherKind,
        },
        RefusedCase {
            // The wire spells `ManagedPolicyArn` with a leading capital; any other
            // spelling names no member of the structure.
            label: "neither kind, the managed policy member misspelled",
            sent: json!({"managedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
            form: Form::NeitherKind,
        },
    ]
}

/// The single-kind rows the choice ADMITS, which bound the refused class from the other
/// side: exactly one half named is stored, not refused.
fn accepted_boundaries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the managed policy half alone",
            json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
        ),
        (
            "the customer managed reference half alone",
            json!({"CustomerManagedPolicyReference": {"Name": "Alone", "Path": "/team/"}}),
        ),
    ]
}

/// Create a permission set and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no minted PermissionSetArn in {body}"))
        .to_string()
}

/// A put of `boundary`, as `(status, body)`.
async fn put_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    boundary: &Value,
) -> (u16, Value) {
    call(
        router,
        "PutPermissionsBoundaryToPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
            "PermissionsBoundary": boundary,
        }),
    )
    .await
}

/// `GetPermissionsBoundaryForPermissionSet`, as the reported boundary (or `Null`).
async fn get_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GetPermissionsBoundaryForPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetPermissionsBoundaryForPermissionSet expected 200, got {status} {body}"
    );
    body["PermissionsBoundary"].clone()
}

/// THE RULE, row by row: the refusal states the form the request actually sent, in the
/// `Message` member, and stores nothing.
#[tokio::test]
async fn the_refusal_states_the_members_the_request_got_wrong() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "ChoiceRefusal").await;

    for case in refused_cases() {
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected ValidationException, got {body}",
            case.label
        );
        let expected = case.form.message();
        assert_eq!(
            body["Message"].as_str(),
            Some(expected),
            "case {:?}: expected Message {expected:?}, got {body}",
            case.label
        );
        let stored = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored,
            Value::Null,
            "case {:?}: a refused put must store nothing, got {stored}",
            case.label
        );
    }
}

/// The two forms are TOLD APART by the text: every both-kinds row says so and no
/// neither-kind row does, and each text names both halves of the choice so the caller can
/// fix the request from it alone.
#[tokio::test]
async fn the_two_malformed_forms_earn_different_text() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "ChoiceForms").await;

    // The clause that is TRUE only of a body that named both kinds.
    const NOT_BOTH_CLAUSE: &str = "but not both";

    let mut seen_by_form: Vec<(Form, String)> = Vec::new();
    for case in refused_cases() {
        let (_, body) = put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        let message = body["Message"].as_str().unwrap_or_default().to_string();
        for member in ["CustomerManagedPolicyReference", "ManagedPolicyArn"] {
            assert!(
                message.contains(member),
                "case {:?}: the refusal must name {member}, got {message:?}",
                case.label
            );
        }
        assert_eq!(
            message.contains(NOT_BOTH_CLAUSE),
            case.form == Form::BothKinds,
            "case {:?}: only a both-kinds body may be told {NOT_BOTH_CLAUSE:?}, got \
             {message:?}",
            case.label
        );
        seen_by_form.push((case.form, message));
    }

    // One text per form, whatever member values the row carried.
    for form in [Form::BothKinds, Form::NeitherKind] {
        let texts: Vec<&String> = seen_by_form
            .iter()
            .filter(|(seen, _)| *seen == form)
            .map(|(_, message)| message)
            .collect();
        assert!(
            !texts.is_empty(),
            "case {form:?}: the table must carry at least one row of this form"
        );
        assert!(
            texts.iter().all(|message| *message == texts[0]),
            "case {form:?}: the text depends on the members named and nothing else, got \
             {texts:?}"
        );
    }
    let both = seen_by_form
        .iter()
        .find(|(form, _)| *form == Form::BothKinds)
        .map(|(_, message)| message.clone())
        .unwrap_or_default();
    let neither = seen_by_form
        .iter()
        .find(|(form, _)| *form == Form::NeitherKind)
        .map(|(_, message)| message.clone())
        .unwrap_or_default();
    assert_ne!(
        both, neither,
        "case {:?}: the two forms must be told apart from the body, got {both:?} for both \
         and {neither:?} for neither",
        "both kinds vs neither kind"
    );
}

/// The other side of the class: a boundary naming EXACTLY ONE half is accepted and read
/// back, so the refusal is about the choice and not about the operation.
#[tokio::test]
async fn naming_exactly_one_kind_is_not_refused() {
    let (router, instance_arn) = organization_router();

    for (index, (label, sent)) in accepted_boundaries().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Accepted{index}")).await;
        let (status, body) = put_boundary(&router, &instance_arn, &permission_set_arn, &sent).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} {body}"
        );
        let stored = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        let object = stored
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected a structure, got {stored}"));
        assert_eq!(
            object.len(),
            1,
            "case {label:?}: exactly one half of the choice is stored, got {stored}"
        );
    }
}
