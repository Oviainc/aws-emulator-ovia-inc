// SPDX-License-Identifier: CC0-1.0

//! `PutUserPermissionsBoundary` and its reporting half `GetUser`, end to end over
//! the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of a
//! boundary that was PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over:
//! the user's path and the case the request spells its name in, the kind of policy
//! the boundary names (AWS-managed, customer-managed) and the path it is filed
//! under, how many puts the user has seen, and the form of a malformed ARN.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s
/// own documentation describes, as `tests/all/managed_policies.rs` uses it.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The `PermissionsBoundaryType` AWS reports for every entity carrying a boundary,
/// written out here rather than read from the implementation's constant: what this suite
/// pins is the value AWS puts on the WIRE, which is `Policy` — not the name the model gives
/// the single `PermissionsBoundaryAttachmentType` entry
/// (`smithy_query(query="shape", shape="PermissionsBoundaryAttachmentType")`).
pub(crate) const PERMISSIONS_BOUNDARY_TYPE: &str = "Policy";

/// ARNs of AWS-managed policies, spanning the paths the managed-policy ARN grammar
/// admits: the account root, the `/service-role/` path and the `/job-function/`
/// path. The names and paths are entries of the AWS Managed Policy Reference; what
/// the rule generalises over is the ARN's SHAPE (path depth, name alphabet), which
/// is why the rows differ in path rather than only in name.
pub(crate) const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/PowerUserAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// A managed policy ARN is `arn:aws:iam::<account>:policy<path><name>` — IAM is
/// global, so the region segment is EMPTY.
fn customer_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// Create a user at `path`, which every boundary case needs first.
pub(crate) async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?} at {path:?}: CreateUser must succeed, got {status}: {body}"
    );
}

/// Create a customer-managed policy at `path`, answering the ARN it reports — the
/// ARN a boundary then names.
pub(crate) async fn create_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    path: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");
    assert_eq!(
        arn,
        customer_policy_arn(path, policy_name),
        "case {policy_name:?} at {path:?}: CreatePolicy must answer the policy's own ARN"
    );
    arn
}

/// One `PutUserPermissionsBoundary` call.
pub(crate) async fn put_boundary(
    router: &MultiServiceRouter,
    user_name: &str,
    permissions_boundary: &str,
) -> (u16, String) {
    call(
        router,
        "PutUserPermissionsBoundary",
        &[
            ("UserName", user_name),
            ("PermissionsBoundary", permissions_boundary),
        ],
    )
    .await
}

/// The boundary `GetUser` reports for `user_name`: its type and ARN, or `None` when
/// the user carries none.
pub(crate) async fn reported_boundary(
    router: &MultiServiceRouter,
    user_name: &str,
) -> Option<(String, String)> {
    let (status, body) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: GetUser must succeed, got {status}: {body}"
    );
    let types = elements(&body, "PermissionsBoundaryType");
    let arns = elements(&body, "PermissionsBoundaryArn");
    assert_eq!(
        types.len(),
        arns.len(),
        "case {user_name:?}: a reported boundary must carry both members: {body}"
    );
    assert!(
        arns.len() <= 1,
        "case {user_name:?}: a user carries at most one permissions boundary, got {arns:?}"
    );
    types.into_iter().zip(arns).next()
}

/// RULE: `PutUserPermissionsBoundary` persists the managed policy the request names
/// as the user's permissions boundary, so a later `GetUser` reports that ARN under
/// the one attachment type the model declares — whatever the user's path, whatever
/// case the request spells the user name in, and whether the policy is AWS-managed
/// or one this account created.
#[tokio::test]
async fn put_persists_the_boundary_and_get_user_reads_it_back() {
    let router = router();

    // The customer-managed policies the rows below bound a user with, created first
    // so the ARNs name policies this account actually holds.
    let customer_root = create_policy(&router, "user-boundary-at-root", "/").await;
    let customer_nested = create_policy(&router, "Boundary_2", "/boundaries/eng/").await;

    // Rows: (user name as created, user path, the name the PUT spells, boundary ARN).
    // They differ along the user's path depth, the case the put spells the name in
    // (IAM entity names "are not distinguished by case"), the name alphabet, and the
    // policy's owner and path depth.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("alice", "/", "alice", AWS_MANAGED_POLICIES[0]),
        ("Bob", "/eng/", "BOB", AWS_MANAGED_POLICIES[2]),
        (
            "user_with+every=allowed.char@-",
            "/eng/tools/",
            "user_with+every=allowed.char@-",
            AWS_MANAGED_POLICIES[3],
        ),
        ("CustomerBounded", "/", "customerbounded", &customer_root),
        (
            "NestedPolicyBounded",
            "/sales/west/",
            "NestedPolicyBounded",
            &customer_nested,
        ),
    ];

    for (created_name, path, put_name, boundary) in cases {
        let case = format!("{created_name:?} at {path:?} <- {boundary:?}");
        create_user(&router, created_name, path).await;

        let (status, body) = put_boundary(&router, put_name, boundary).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");

        let reported = reported_boundary(&router, created_name).await;
        assert_eq!(
            reported,
            Some((
                PERMISSIONS_BOUNDARY_TYPE.to_string(),
                (*boundary).to_string()
            )),
            "case {case}: expected the persisted boundary, got {reported:?}"
        );
    }
}

/// RULE: the operation "adds or UPDATES" the one boundary a user carries, so each
/// put replaces whatever boundary the user held — including one `CreateUser` itself
/// set — and a repeat of the ARN already set leaves the user exactly as it is, never
/// a second attachment.
#[tokio::test]
async fn a_later_put_replaces_the_boundary_the_user_held() {
    let router = router();
    let user = "ReplacedBoundaryUser";
    create_user(&router, user, "/").await;
    let customer = create_policy(&router, "user-replacement", "/boundaries/").await;

    assert_eq!(
        reported_boundary(&router, user).await,
        None,
        "a user created without a boundary reports none"
    );

    // Rows: the sequence of ARNs put, including a repeat of the one already set, so
    // what is asserted is the LAST put's ARN after each step whatever came before.
    let sequence = [
        AWS_MANAGED_POLICIES[0],
        AWS_MANAGED_POLICIES[1],
        AWS_MANAGED_POLICIES[1],
        customer.as_str(),
        AWS_MANAGED_POLICIES[2],
    ];
    for boundary in sequence {
        let (status, body) = put_boundary(&router, user, boundary).await;
        assert_eq!(
            status, 200,
            "case {boundary:?}: expected 200, got {status}: {body}"
        );
        let reported = reported_boundary(&router, user).await;
        assert_eq!(
            reported,
            Some((PERMISSIONS_BOUNDARY_TYPE.to_string(), boundary.to_string())),
            "case {boundary:?}: expected the last put's boundary, got {reported:?}"
        );
    }

    // A user whose boundary `CreateUser` set is bounded by the same one record, so a
    // put replaces that boundary just as it replaces one a put set.
    let created_bounded = "CreatedWithBoundary";
    let (status, body) = call(
        &router,
        "CreateUser",
        &[
            ("UserName", created_bounded),
            ("PermissionsBoundary", AWS_MANAGED_POLICIES[3]),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    assert_eq!(
        reported_boundary(&router, created_bounded).await,
        Some((
            PERMISSIONS_BOUNDARY_TYPE.to_string(),
            AWS_MANAGED_POLICIES[3].to_string()
        )),
        "CreateUser's own boundary must be reported"
    );
    let (status, body) = put_boundary(&router, created_bounded, AWS_MANAGED_POLICIES[0]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported_boundary(&router, created_bounded).await,
        Some((
            PERMISSIONS_BOUNDARY_TYPE.to_string(),
            AWS_MANAGED_POLICIES[0].to_string()
        )),
        "the put must replace the boundary CreateUser set"
    );
}

/// RULE: `UserName` names a user the account holds, so a name no user is held under
/// refers to no entity at all — 404 `NoSuchEntity`, persisting nothing.
#[tokio::test]
async fn bounding_a_user_that_does_not_exist_is_refused() {
    let router = router();
    create_user(&router, "HeldUser", "/").await;

    // Rows spanning the forms a name can miss by: never created, a near miss of a
    // held name, and the whole allowed alphabet.
    let unknown = [
        "NoSuchUser",
        "HeldUser2",
        "held_user",
        "another+name=entirely.x@-",
    ];
    for user_name in unknown {
        let (status, body) = put_boundary(&router, user_name, AWS_MANAGED_POLICIES[0]).await;
        assert_eq!(
            status, 404,
            "case {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    // The held user is untouched by every refusal.
    assert_eq!(
        reported_boundary(&router, "HeldUser").await,
        None,
        "a refused put must persist nothing"
    );
}

/// RULE: `PermissionsBoundary` must BE a managed policy's ARN —
/// `arn:<partition>:iam::<account>:policy/<path><name>`, with an empty region
/// segment because IAM is global — and anything else is an invalid value for an
/// input parameter (400 `InvalidInput`), persisting nothing.
#[tokio::test]
async fn a_user_boundary_that_is_not_a_managed_policy_arn_is_refused() {
    let router = router();
    let user = "ArnValidationUser";
    create_user(&router, user, "/").await;
    assert_eq!(
        put_boundary(&router, user, AWS_MANAGED_POLICIES[0]).await.0,
        200,
        "the fixture boundary must be set"
    );

    // Each row is at least 20 characters, `arnType`'s minimum length, so the handler
    // — not the modelled length constraint — is what refuses it. The rows break one
    // part of the grammar each.
    let malformed = [
        "this-is-not-an-arn-at-all",
        "arn:aws:s3:::a-bucket-named-like-this",
        "arn:aws:iam::aws:role/NotAPolicyAtAll",
        "arn:aws:iam::aws:user/SomeUserNamedHere",
        "arn:aws:iam:us-east-1:aws:policy/RegionIsSet",
        "arn::iam::aws:policy/PartitionIsEmpty",
        "arn:aws:iam:::policy/AccountIsEmptyHere",
        "arn:aws:iam::aws:policy/job-function/",
        "arn:aws:iam::aws:policy//EmptySegment",
        "aws:iam::aws:policy/MissingArnPrefixHere",
    ];
    for boundary in malformed {
        assert!(
            boundary.len() >= 20,
            "case {boundary:?}: a row must satisfy arnType's 20-character minimum"
        );
        let (status, body) = put_boundary(&router, user, boundary).await;
        assert_eq!(
            status, 400,
            "case {boundary:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {boundary:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }

    // A refused put writes nothing: the user still carries the fixture boundary.
    let reported = reported_boundary(&router, user).await;
    assert_eq!(
        reported,
        Some((
            PERMISSIONS_BOUNDARY_TYPE.to_string(),
            AWS_MANAGED_POLICIES[0].to_string()
        )),
        "a refused put must leave the boundary at the fixture, got {reported:?}"
    );
}

/// RULE: a customer-managed policy sets a boundary only if this account holds it, so
/// an ARN in this account naming a policy that was never created refers to no entity
/// — 404 `NoSuchEntity` — while the SAME name once created is accepted.
#[tokio::test]
async fn a_user_boundary_naming_a_policy_the_account_does_not_hold_is_refused() {
    let router = router();
    let user = "PolicyExistenceUser";
    create_user(&router, user, "/").await;
    create_policy(&router, "held-user-boundary", "/").await;

    // Rows: a name never created, a held name under another path, a held name in
    // another case (policy names are not distinguished by case, but the ARN carries
    // the minted spelling), and a nested path nothing was created under.
    let absent = [
        customer_policy_arn("/", "never-created-at-all"),
        customer_policy_arn("/elsewhere/", "held-user-boundary"),
        customer_policy_arn("/", "HELD-USER-BOUNDARY"),
        customer_policy_arn("/a/b/c/", "absent"),
    ];
    for boundary in &absent {
        let (status, body) = put_boundary(&router, user, boundary).await;
        assert_eq!(
            status, 404,
            "case {boundary:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {boundary:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            reported_boundary(&router, user).await,
            None,
            "case {boundary:?}: a refused put must persist nothing"
        );
    }

    // The policy this account DOES hold sets the boundary, which is what shows the
    // refusals above turn on existence rather than on the ARN's shape.
    let held = customer_policy_arn("/", "held-user-boundary");
    let (status, body) = put_boundary(&router, user, &held).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported_boundary(&router, user).await,
        Some((PERMISSIONS_BOUNDARY_TYPE.to_string(), held)),
        "the held policy must set the boundary"
    );
}

/// RULE: a policy AWS files under the reserved service-linked role path is a service
/// role policy, which "can only be attached to the service-linked role for that
/// service" — and no user is a service-linked role, so bounding one with such a
/// policy is refused with 400 `PolicyNotAttachable`.
#[tokio::test]
async fn a_service_role_policy_cannot_bound_a_user() {
    let router = router();
    let user = "OrdinaryBoundedUser";
    create_user(&router, user, "/").await;

    // Rows spanning the service the policy belongs to and the depth below the
    // reserved path.
    let service_role_policies = [
        "arn:aws:iam::aws:policy/aws-service-role/AWSLambdaRole",
        "arn:aws:iam::aws:policy/aws-service-role/AmazonEKSServiceRolePolicy",
        "arn:aws:iam::aws:policy/aws-service-role/elasticbeanstalk/ServicePolicy",
    ];
    for boundary in service_role_policies {
        let (status, body) = put_boundary(&router, user, boundary).await;
        assert_eq!(
            status, 400,
            "case {boundary:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "PolicyNotAttachable",
            "case {boundary:?}: expected PolicyNotAttachable, got {:?}",
            error_code(&body)
        );
    }

    assert_eq!(
        reported_boundary(&router, user).await,
        None,
        "a refused put must persist nothing"
    );
}
