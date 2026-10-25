// SPDX-License-Identifier: CC0-1.0

//! The DUPLICATE ATTACH of a policy on a permission set, end to end over the `awsJson1_1`
//! wire — for both attach operations of the permission-set policy surface.
//!
//! THE RULE under test: an attach the model does not mark `@idempotent` refuses, with
//! `ConflictException`, a request whose attachment the permission set already carries (a
//! conflict with the previous successful write), and leaves the stored set exactly as it
//! was; the same attachment is accepted on a permission set that does not carry it, and
//! again after a detach has removed it. Which attach that is follows from the model:
//! `smithy_query(query="operation", operation="AttachManagedPolicyToPermissionSet,
//! AttachCustomerManagedPolicyReferenceToPermissionSet")` reports `ConflictException` and
//! NO `idempotent` flag for either, while the same query for `PutApplicationAccessScope`
//! reports `"idempotent": true`.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: the
//! ATTACH OPERATION (an AWS managed policy ARN, a customer managed policy reference), the
//! form of the attachment's identity (a plain policy name, a nested path, a suffixed
//! partition, the character classes the members admit), the SPELLING the repeat is sent in
//! (an omitted `Path` names the same reference as the default spelled out), and the NUMBER
//! of repeats.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance.
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever
//! `@httpError` its shape declares, so the assertions below pin the status AND the wire
//! error code, which is what tells one refusal from another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, organization_router};

/// Which attachment kind a row is about: the pair of attach/detach/list operations that
/// serve one SET on a permission set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The AWS managed policy ARNs of a permission set.
    ManagedPolicy,
    /// The customer managed policy references of a permission set.
    CustomerReference,
}

impl Kind {
    fn attach(self) -> &'static str {
        match self {
            Self::ManagedPolicy => "AttachManagedPolicyToPermissionSet",
            Self::CustomerReference => "AttachCustomerManagedPolicyReferenceToPermissionSet",
        }
    }

    fn detach(self) -> &'static str {
        match self {
            Self::ManagedPolicy => "DetachManagedPolicyFromPermissionSet",
            Self::CustomerReference => "DetachCustomerManagedPolicyReferenceFromPermissionSet",
        }
    }

    fn list(self) -> &'static str {
        match self {
            Self::ManagedPolicy => "ListManagedPoliciesInPermissionSet",
            Self::CustomerReference => "ListCustomerManagedPolicyReferencesInPermissionSet",
        }
    }

    /// The list output member the attachments are reported in.
    fn items_member(self) -> &'static str {
        match self {
            Self::ManagedPolicy => "AttachedManagedPolicies",
            Self::CustomerReference => "CustomerManagedPolicyReferences",
        }
    }

    /// One reported attachment as its identity: the ARN of an AWS managed policy, the
    /// (`Name`, `Path`) pair of a customer managed policy reference.
    fn identity_of(self, item: &Value) -> String {
        match self {
            Self::ManagedPolicy => item["Arn"].as_str().unwrap_or_default().to_string(),
            Self::CustomerReference => format!(
                "{}|{}",
                item["Name"].as_str().unwrap_or_default(),
                item["Path"].as_str().unwrap_or_default()
            ),
        }
    }
}

/// One row: an attachment, the way a FIRST attach sends it, the way a REPEAT sends the very
/// same attachment, and the identity a later list must report for it.
struct Row {
    label: &'static str,
    kind: Kind,
    /// The members the first attach carries beside `InstanceArn` and `PermissionSetArn`.
    first: Value,
    /// The members the repeat carries — the same attachment, possibly spelled differently.
    repeat: Value,
    /// What [`Kind::identity_of`] must report for the stored attachment.
    identity: String,
}

/// The `ManagedPolicyArn` prefix of the `aws` partition, per the member's pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`.
const POLICY_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// `CustomerManagedPolicyReference$Path`'s documented default: "The default is `/`."
const DEFAULT_PATH: &str = "/";

/// An AWS managed policy row: the ARN is the attachment's identity, so the repeat is the
/// same ARN.
fn managed(label: &'static str, arn: String) -> Row {
    Row {
        label,
        kind: Kind::ManagedPolicy,
        first: json!({"ManagedPolicyArn": arn}),
        repeat: json!({"ManagedPolicyArn": arn}),
        identity: arn,
    }
}

/// A customer managed policy reference row: `first` and `repeat` are the two shapes sent,
/// which may spell `Path` differently while naming the same (`Name`, `Path`) identity.
fn reference(label: &'static str, first: Value, repeat: Value, name: &str, path: &str) -> Row {
    Row {
        label,
        kind: Kind::CustomerReference,
        first: json!({"CustomerManagedPolicyReference": first}),
        repeat: json!({"CustomerManagedPolicyReference": repeat}),
        identity: format!("{name}|{path}"),
    }
}

/// The table. Rows span both attach operations, the forms each attachment's identity takes,
/// and the two spellings that name one reference.
fn rows() -> Vec<Row> {
    vec![
        managed(
            "an AWS managed policy with no path",
            format!("{POLICY_PREFIX}ReadOnlyAccess"),
        ),
        managed(
            "an AWS managed policy under a nested path",
            format!("{POLICY_PREFIX}job-function/ViewOnlyAccess"),
        ),
        managed(
            "an AWS managed policy of a suffixed partition",
            "arn:aws-us-gov:iam::aws:policy/PowerUserAccess".to_string(),
        ),
        managed(
            "an AWS managed policy using the full name charset",
            format!("{POLICY_PREFIX}a.b,c+d=e@f_g-H9"),
        ),
        reference(
            "a customer managed policy reference under the default path",
            json!({"Name": "ReadOnlyPolicy", "Path": "/"}),
            json!({"Name": "ReadOnlyPolicy", "Path": "/"}),
            "ReadOnlyPolicy",
            "/",
        ),
        reference(
            "a reference attached with Path, repeated without it",
            json!({"Name": "OmittedOnRepeat", "Path": "/"}),
            json!({"Name": "OmittedOnRepeat"}),
            "OmittedOnRepeat",
            DEFAULT_PATH,
        ),
        reference(
            "a reference attached without Path, repeated with it",
            json!({"Name": "OmittedOnFirst"}),
            json!({"Name": "OmittedOnFirst", "Path": "/"}),
            "OmittedOnFirst",
            DEFAULT_PATH,
        ),
        reference(
            "a customer managed policy reference under a nested path",
            json!({"Name": "NestedPolicy", "Path": "/engineering/platform/"}),
            json!({"Name": "NestedPolicy", "Path": "/engineering/platform/"}),
            "NestedPolicy",
            "/engineering/platform/",
        ),
        reference(
            "a reference using the full character classes of both members",
            json!({"Name": "a.b,c+d=e@f_g-H9", "Path": "/a.b,c+d@e=f_g-H9/"}),
            json!({"Name": "a.b,c+d=e@f_g-H9", "Path": "/a.b,c+d@e=f_g-H9/"}),
            "a.b,c+d=e@f_g-H9",
            "/a.b,c+d@e=f_g-H9/",
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

/// Send one attach or detach of `row`'s attachment, as `(status, body)`.
async fn send(
    router: &MultiServiceRouter,
    operation: &str,
    instance_arn: &str,
    permission_set_arn: &str,
    members: &Value,
) -> (u16, Value) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    for (member, value) in members.as_object().expect("the row carries members") {
        request[member] = value.clone();
    }
    call(router, operation, &request).await
}

/// The identities the permission set's set holds, read over the wire, sorted so the
/// comparison is about the SET rather than about a page order.
async fn attached(
    router: &MultiServiceRouter,
    kind: Kind,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        kind.list(),
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} expected 200, got {status} {body}",
        kind.list()
    );
    let mut found: Vec<String> = body[kind.items_member()]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must report a list, got {body}",
                kind.items_member()
            )
        })
        .iter()
        .map(|item| kind.identity_of(item))
        .collect();
    found.sort();
    found
}

/// THE RULE: the attachment the permission set already carries is refused
/// `ConflictException`, however often the request is repeated and however it is spelled,
/// and the stored set keeps exactly the one entry the first attach made.
#[tokio::test]
async fn a_repeated_attach_of_an_attachment_the_permission_set_holds_is_refused() {
    let (router, instance_arn) = organization_router();
    for (index, row) in rows().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Conflict{index}")).await;

        let (status, body) = send(
            &router,
            row.kind.attach(),
            &instance_arn,
            &permission_set_arn,
            &row.first,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first attach expected 200, got {status} {body}",
            row.label
        );

        // Repeats, alternating the two spellings of the same attachment: each one is the
        // conflict, and none of them changes the stored set.
        for attempt in 2..=4 {
            let members = if attempt % 2 == 0 {
                &row.repeat
            } else {
                &row.first
            };
            let (status, body) = send(
                &router,
                row.kind.attach(),
                &instance_arn,
                &permission_set_arn,
                members,
            )
            .await;
            assert_refused(
                row.label,
                &format!("attach #{attempt}"),
                "ConflictException",
                status,
                &body,
            );
            let found = attached(
                &router,
                row.kind,
                &instance_arn,
                &permission_set_arn,
                row.label,
            )
            .await;
            assert_eq!(
                found,
                vec![row.identity.clone()],
                "case {:?}: after attach #{attempt} the set must still hold exactly \
                 {:?}, got {found:?}",
                row.label,
                row.identity
            );
        }
    }
}

/// The conflict is about the attachment THIS permission set carries: the same attachment is
/// accepted on a permission set that does not carry it, so the refusal never leaks across
/// resources.
#[tokio::test]
async fn the_same_attachment_is_accepted_on_a_permission_set_that_does_not_carry_it() {
    let (router, instance_arn) = organization_router();
    for (index, row) in rows().into_iter().enumerate() {
        let carrier =
            create_permission_set(&router, &instance_arn, &format!("Carrier{index}")).await;
        let other = create_permission_set(&router, &instance_arn, &format!("Other{index}")).await;

        for (case, permission_set_arn) in [("the carrier", &carrier), ("another set", &other)] {
            let (status, body) = send(
                &router,
                row.kind.attach(),
                &instance_arn,
                permission_set_arn,
                &row.first,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the attach on {case} expected 200, got {status} {body}",
                row.label
            );
            let found = attached(
                &router,
                row.kind,
                &instance_arn,
                permission_set_arn,
                row.label,
            )
            .await;
            assert_eq!(
                found,
                vec![row.identity.clone()],
                "case {:?}: {case} must hold {:?}, got {found:?}",
                row.label,
                row.identity
            );
        }
    }
}

/// A detach removes the attachment, so the write the conflict refers to is gone: the same
/// attach is accepted again, and refused again once it has been made.
#[tokio::test]
async fn an_attachment_a_detach_removed_can_be_attached_again() {
    let (router, instance_arn) = organization_router();
    for (index, row) in rows().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Cycle{index}")).await;

        // Two full attach / refused-repeat / detach cycles, so the rule holds whatever the
        // permission set's history is.
        for cycle in 1..=2 {
            let (status, body) = send(
                &router,
                row.kind.attach(),
                &instance_arn,
                &permission_set_arn,
                &row.first,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the attach of cycle {cycle} expected 200, got {status} {body}",
                row.label
            );

            let (status, body) = send(
                &router,
                row.kind.attach(),
                &instance_arn,
                &permission_set_arn,
                &row.repeat,
            )
            .await;
            assert_refused(
                row.label,
                &format!("the repeat of cycle {cycle}"),
                "ConflictException",
                status,
                &body,
            );

            let (status, body) = send(
                &router,
                row.kind.detach(),
                &instance_arn,
                &permission_set_arn,
                &row.repeat,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the detach of cycle {cycle} expected 200, got {status} {body}",
                row.label
            );
            let found = attached(
                &router,
                row.kind,
                &instance_arn,
                &permission_set_arn,
                row.label,
            )
            .await;
            assert!(
                found.is_empty(),
                "case {:?}: after the detach of cycle {cycle} the set must be empty, got \
                 {found:?}",
                row.label
            );
        }
    }
}
