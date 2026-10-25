// SPDX-License-Identifier: CC0-1.0

//! The PERMISSIONS BOUNDARY of a permission set, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: a permission set carries AT MOST ONE permissions boundary;
//! `PutPermissionsBoundaryToPermissionSet` stores exactly one of the two kinds
//! `PermissionsBoundary` offers ("Specify either `CustomerManagedPolicyReference` to use
//! the name and path of a customer managed policy, or `ManagedPolicyArn` to use the ARN of
//! an Amazon Web Services managed policy"), normalising a customer managed reference's
//! absent `Path` to its documented default `/`; a later put REPLACES the stored boundary;
//! `GetPermissionsBoundaryForPermissionSet` answers from the stored one, and reports
//! `PermissionsBoundary` ABSENT until a put stores one. A request that names both kinds or
//! neither is refused with the modelled `ValidationException` (HTTP 400) and stores
//! nothing, and an `InstanceArn` or `PermissionSetArn` nothing is held under is a
//! `ResourceNotFoundException`.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the KIND of boundary, the partition and path forms the `ManagedPolicyArn` pattern
//! admits, the published length bounds of `ManagedPolicyArn` (20..2048) and of
//! `ManagedPolicyName` (1..128) and `ManagedPolicyPath` (1..512), the presence of `Path`,
//! and the order the puts are made in.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_router, UNKNOWN_INSTANCE_ARN,
};

/// A pattern-valid `PermissionSetArn` that was never minted, for the not-found rows.
/// Shaped by the model's `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000";

/// The fixed part of an AWS managed policy ARN, as the model's `ManagedPolicyArn` pattern
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`)
/// requires: 24 characters, so the shortest value the pattern admits is longer than the
/// member's lower length bound of 20 and that bound is unreachable.
const MANAGED_POLICY_ARN_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// `ManagedPolicyArn`'s published upper length bound (`length(min: 20, max: 2048)`).
const MANAGED_POLICY_ARN_MAX: usize = 2048;

/// One row of the boundary table: the request body for `PermissionsBoundary` and the
/// value a later `GetPermissionsBoundaryForPermissionSet` must report.
struct BoundaryCase {
    label: &'static str,
    /// What the put sends as `PermissionsBoundary`.
    sent: Value,
    /// What the get must answer with — the stored choice, with `Path` materialised.
    expected: Value,
}

/// The table. Rows span both kinds, the partition suffixes and nested paths the
/// `ManagedPolicyArn` pattern admits, its upper length bound, the presence and the forms of
/// `CustomerManagedPolicyReference$Path`, and the length bounds of `Name` (1..128) and
/// `Path` (1..512). No row is the one a recording happened to carry.
fn boundary_cases() -> Vec<BoundaryCase> {
    vec![
        BoundaryCase {
            label: "an AWS managed policy in the aws partition",
            sent: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
            expected: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
        },
        BoundaryCase {
            label: "an AWS managed policy under a nested path",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
            }),
            expected: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
            }),
        },
        BoundaryCase {
            // `^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy…` admits partition suffixes.
            label: "an AWS managed policy in a suffixed partition",
            sent: json!({
                "ManagedPolicyArn": "arn:aws-us-gov:iam::aws:policy/PowerUserAccess",
            }),
            expected: json!({
                "ManagedPolicyArn": "arn:aws-us-gov:iam::aws:policy/PowerUserAccess",
            }),
        },
        BoundaryCase {
            // The full `[A-Za-z0-9\.,\+=@_-]` class the policy name admits.
            label: "an AWS managed policy whose name uses the full charset",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/a.b,c+d=e@f_g-H9",
            }),
            expected: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/a.b,c+d=e@f_g-H9",
            }),
        },
        BoundaryCase {
            label: "a customer managed policy with no Path (the documented default)",
            sent: json!({"CustomerManagedPolicyReference": {"Name": "AppBoundary"}}),
            // `Path`'s own documentation: "The default is `/`".
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "AppBoundary", "Path": "/"},
            }),
        },
        BoundaryCase {
            label: "a customer managed policy with the root Path spelled out",
            sent: json!({
                "CustomerManagedPolicyReference": {"Name": "RootPathBoundary", "Path": "/"},
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "RootPathBoundary", "Path": "/"},
            }),
        },
        BoundaryCase {
            label: "a customer managed policy under a nested Path",
            sent: json!({
                "CustomerManagedPolicyReference": {
                    "Name": "Nested",
                    "Path": "/engineering/platform/",
                },
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {
                    "Name": "Nested",
                    "Path": "/engineering/platform/",
                },
            }),
        },
        BoundaryCase {
            // `ManagedPolicyName` at its lower length bound (1..128).
            label: "a customer managed policy whose Name is one character",
            sent: json!({"CustomerManagedPolicyReference": {"Name": "a"}}),
            expected: json!({
                "CustomerManagedPolicyReference": {"Name": "a", "Path": "/"},
            }),
        },
    ]
}

/// The rows whose values are built rather than written out: the published upper length
/// bounds of `ManagedPolicyArn` (2048), `ManagedPolicyName` (128) and `ManagedPolicyPath`
/// (512), each exactly AT the bound.
fn boundary_cases_at_upper_bounds() -> Vec<BoundaryCase> {
    // 2048 characters in all: the 24-character prefix plus a policy name.
    let longest_arn = format!(
        "{MANAGED_POLICY_ARN_PREFIX}{}",
        "P".repeat(MANAGED_POLICY_ARN_MAX - MANAGED_POLICY_ARN_PREFIX.len())
    );
    // `ManagedPolicyName` at 128, over the full `^[\w+=,.@-]+$` class.
    let longest_name = format!("{}+=,.@-_", "n".repeat(121));
    // `ManagedPolicyPath` at 512: `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`, so a leading `/`, a
    // 510-character segment and the trailing `/`.
    let longest_path = format!("/{}/", "p".repeat(510));
    vec![
        BoundaryCase {
            label: "an AWS managed policy ARN at its upper length bound",
            sent: json!({"ManagedPolicyArn": longest_arn}),
            expected: json!({"ManagedPolicyArn": longest_arn}),
        },
        BoundaryCase {
            label: "a customer managed policy at the upper bounds of Name and Path",
            sent: json!({
                "CustomerManagedPolicyReference": {
                    "Name": longest_name,
                    "Path": longest_path,
                },
            }),
            expected: json!({
                "CustomerManagedPolicyReference": {
                    "Name": longest_name,
                    "Path": longest_path,
                },
            }),
        },
    ]
}

/// Every row the round-trip rule is asserted over.
fn every_boundary_case() -> Vec<BoundaryCase> {
    let mut cases = boundary_cases();
    cases.extend(boundary_cases_at_upper_bounds());
    cases
}

/// Create a permission set named `name` on `instance_arn` and return its minted ARN.
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

/// `GetPermissionsBoundaryForPermissionSet`, as `(status, PermissionsBoundary or Null)`.
async fn get_boundary(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetPermissionsBoundaryForPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        }),
    )
    .await;
    if status != 200 {
        return (status, body);
    }
    (status, body["PermissionsBoundary"].clone())
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

/// THE RULE: a put stores the boundary it names and the get answers from the stored one —
/// over every row of the table, each on its own permission set. Before the put, the same
/// get reports no boundary at all.
#[tokio::test]
async fn a_put_stores_the_boundary_and_the_get_answers_from_it() {
    let (router, instance_arn) = organization_router();

    for (index, case) in every_boundary_case().into_iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Boundary{index}")).await;

        // A permission set with nothing attached: `PermissionsBoundary` is an OPTIONAL
        // output member, so it is absent rather than an error or an empty structure.
        let (status, before) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: a get before any put expected 200, got {status} {before}",
            case.label
        );
        assert_eq!(
            before,
            Value::Null,
            "case {:?}: a permission set with no boundary must report none, got {before}",
            case.label
        );

        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: PutPermissionsBoundaryToPermissionSet expected 200, got {status} {body}",
            case.label
        );

        let (status, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get after the put expected 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored, case.expected,
            "case {:?}: expected the stored boundary {}, got {stored}",
            case.label, case.expected
        );

        // Exactly ONE kind is reported, whatever the put sent: the stored boundary is a
        // choice, not a pair.
        let object = stored
            .as_object()
            .unwrap_or_else(|| panic!("case {:?}: expected a structure, got {stored}", case.label));
        assert_eq!(
            object.len(),
            1,
            "case {:?}: expected exactly one of ManagedPolicyArn / \
             CustomerManagedPolicyReference, got {stored}",
            case.label
        );
    }
}

/// A read must not change what a later caller sees: two gets of the same permission set
/// answer identically, and a get never brings a boundary into being.
#[tokio::test]
async fn a_get_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Repeatable").await;

    for round in 0..3 {
        let (status, boundary) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case round {round}: expected 200, got {status} {boundary}"
        );
        assert_eq!(
            boundary,
            Value::Null,
            "case round {round}: a get must not attach a boundary, got {boundary}"
        );
    }

    let case = &boundary_cases()[0];
    let (status, body) = put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
    assert_eq!(status, 200, "the put expected 200, got {status} {body}");

    let (_, first) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
    let (_, second) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        first, second,
        "case {:?}: repeated gets must agree, got {first} then {second}",
        case.label
    );
    assert_eq!(
        first, case.expected,
        "case {:?}: expected {}, got {first}",
        case.label, case.expected
    );
}

/// A permission set carries AT MOST ONE boundary, so a later put REPLACES the stored one:
/// walking the whole table over ONE permission set, each get reports only the newest row.
#[tokio::test]
async fn a_later_put_replaces_the_stored_boundary() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Replaced").await;

    for case in every_boundary_case() {
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the replacing put expected 200, got {status} {body}",
            case.label
        );

        let (status, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get expected 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored, case.expected,
            "case {:?}: a later put replaces the boundary, expected {}, got {stored}",
            case.label, case.expected
        );
    }

    // Putting the SAME boundary twice is the same stored boundary: a put sets, it does
    // not accumulate.
    let case = &boundary_cases()[0];
    for round in 0..2 {
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case round {round}: a repeated put expected 200, got {status} {body}"
        );
    }
    let (_, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
    assert_eq!(
        stored, case.expected,
        "case {:?}: a repeated put stores one boundary, expected {}, got {stored}",
        case.label, case.expected
    );
}

/// A boundary of one permission set is NOT a boundary of another: the attachment belongs to
/// the permission set the put named.
#[tokio::test]
async fn a_boundary_belongs_to_the_permission_set_it_was_put_on() {
    let (router, instance_arn) = organization_router();
    let cases = boundary_cases();
    let mut attached: Vec<(String, Value)> = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Owned{index}")).await;
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put expected 200, got {status} {body}",
            case.label
        );
        attached.push((permission_set_arn, case.expected.clone()));
    }

    for ((permission_set_arn, expected), case) in attached.iter().zip(cases.iter()) {
        let (status, stored) = get_boundary(&router, &instance_arn, permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get expected 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            &stored, expected,
            "case {:?}: each permission set reports its own boundary, expected {expected}, \
             got {stored}",
            case.label
        );
    }
}

/// One row of the refusal table: a `PermissionsBoundary` the documented choice forbids.
struct RefusedBoundary {
    label: &'static str,
    sent: Value,
}

/// The rows: both kinds named, and neither — the two shapes the model's pair of optional
/// members admits but "Specify either … or …" forbids. The rows vary the values of the
/// members as well, because the refusal depends on WHICH members are present and on
/// nothing else.
fn refused_boundaries() -> Vec<RefusedBoundary> {
    vec![
        RefusedBoundary {
            label: "both kinds, a bare policy name",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
                "CustomerManagedPolicyReference": {"Name": "Both"},
            }),
        },
        RefusedBoundary {
            label: "both kinds, a nested path on each",
            sent: json!({
                "ManagedPolicyArn": "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
                "CustomerManagedPolicyReference": {"Name": "Both", "Path": "/team/"},
            }),
        },
        RefusedBoundary {
            label: "neither kind",
            sent: json!({}),
        },
    ]
}

/// A boundary that names BOTH kinds or NEITHER is refused with `ValidationException`, and
/// the refusal writes nothing: the boundary stored before it is still the stored one.
#[tokio::test]
async fn a_boundary_naming_both_kinds_or_neither_is_refused_and_stores_nothing() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Refused").await;

    // The refusal on a permission set that carries NO boundary leaves it carrying none.
    for case in refused_boundaries() {
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
        let (status, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get expected 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored,
            Value::Null,
            "case {:?}: a refused put must store nothing, got {stored}",
            case.label
        );
    }

    // And on one that already carries a boundary, it leaves THAT boundary in place.
    let accepted = &boundary_cases()[0];
    let (status, body) =
        put_boundary(&router, &instance_arn, &permission_set_arn, &accepted.sent).await;
    assert_eq!(status, 200, "the accepted put expected 200, got {status} {body}");
    for case in refused_boundaries() {
        let (status, body) =
            put_boundary(&router, &instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            case.label
        );
        let (_, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored, accepted.expected,
            "case {:?}: a refused put must leave the stored boundary, expected {}, got \
             {stored}",
            case.label, accepted.expected
        );
    }
}

/// The modelled constraints of the boundary's own members are enforced: `ManagedPolicyArn`
/// is accepted AT its published upper bound (2048, asserted in the round-trip table above)
/// and refused one past it, and a value the pattern does not admit is refused too — with
/// the boundary validator's 400 `ValidationException`, never a stored boundary.
#[tokio::test]
async fn a_boundary_outside_the_modeled_constraints_is_refused() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Constrained").await;

    let one_past_the_bound = format!(
        "{MANAGED_POLICY_ARN_PREFIX}{}",
        "P".repeat(MANAGED_POLICY_ARN_MAX + 1 - MANAGED_POLICY_ARN_PREFIX.len())
    );
    // `ManagedPolicyName` is 1..128, `ManagedPolicyPath` 1..512 and ends in `/`.
    let name_one_past_the_bound = "n".repeat(129);
    let path_one_past_the_bound = format!("/{}/", "p".repeat(511));
    let cases: Vec<(String, Value)> = vec![
        (
            "ManagedPolicyArn one character past its upper bound".to_string(),
            json!({"ManagedPolicyArn": one_past_the_bound}),
        ),
        (
            "a ManagedPolicyArn the pattern does not admit".to_string(),
            json!({"ManagedPolicyArn": "arn:aws:iam::123456789012:policy/Customer"}),
        ),
        (
            "Name one character past its upper bound".to_string(),
            json!({"CustomerManagedPolicyReference": {"Name": name_one_past_the_bound}}),
        ),
        (
            "Path one character past its upper bound".to_string(),
            json!({
                "CustomerManagedPolicyReference": {
                    "Name": "Bounded",
                    "Path": path_one_past_the_bound,
                },
            }),
        ),
        (
            "a Path the pattern does not admit (no trailing slash)".to_string(),
            json!({"CustomerManagedPolicyReference": {"Name": "Bounded", "Path": "/team"}}),
        ),
    ];

    for (label, sent) in cases {
        let (status, body) = put_boundary(&router, &instance_arn, &permission_set_arn, &sent).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        let (_, stored) = get_boundary(&router, &instance_arn, &permission_set_arn).await;
        assert_eq!(
            stored,
            Value::Null,
            "case {label:?}: a refused put must store nothing, got {stored}"
        );
    }
}

/// An `InstanceArn` or `PermissionSetArn` nothing is held under is a missing resource for
/// BOTH operations — never a 200 — and a deleted permission set is as absent as one that
/// never existed.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn) = organization_router();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Present").await;
    let boundary = boundary_cases()[0].sent.clone();

    let mut rows: Vec<(String, String, String)> = vec![
        (
            "an InstanceArn no instance is held under".to_string(),
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
        (
            "a PermissionSetArn never minted".to_string(),
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
    ];

    // A permission set that was created and then deleted.
    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": deleted}),
    )
    .await;
    assert_eq!(status, 200, "DeletePermissionSet expected 200, got {status} {body}");
    rows.push((
        "a deleted PermissionSetArn".to_string(),
        instance_arn.clone(),
        deleted,
    ));

    for (label, instance, permission_set) in rows {
        let (status, body) = put_boundary(&router, &instance, &permission_set, &boundary).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the put expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: the put expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = get_boundary(&router, &instance, &permission_set).await;
        assert_eq!(
            status, 400,
            "case {label:?}: the get expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: the get expected ResourceNotFoundException, got {body}"
        );
    }
}

/// Both operations belong to the permission-set surface, which is WITHDRAWN on an account
/// instance: there they answer the modelled `ValidationException` naming account instances,
/// before any resource lookup.
#[tokio::test]
async fn both_operations_are_refused_on_an_account_instance() {
    let (router, account_instance_arn) = account_instance_router().await;
    let permission_set_arn = format!(
        "arn:aws:sso:::permissionSet/{}/ps-0123456789abcdef",
        account_instance_arn.rsplit('/').next().unwrap_or_default()
    );

    for case in every_boundary_case() {
        let (status, body) =
            put_boundary(&router, &account_instance_arn, &permission_set_arn, &case.sent).await;
        assert_eq!(
            status, 400,
            "case {:?}: the put expected HTTP 400, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: the put expected ValidationException, got {body}",
            case.label
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("account instances"),
            "case {:?}: the refusal must say the operation is unsupported for account \
             instances, got {message:?}",
            case.label
        );
    }

    let (status, body) = get_boundary(&router, &account_instance_arn, &permission_set_arn).await;
    assert_eq!(status, 400, "the get expected HTTP 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "the get expected ValidationException, got {body}"
    );
}
