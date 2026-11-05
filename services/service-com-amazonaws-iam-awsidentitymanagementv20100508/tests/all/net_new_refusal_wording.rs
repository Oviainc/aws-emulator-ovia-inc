// SPDX-License-Identifier: CC0-1.0

//! The refusals and the reported boundary type this service words AWS's way rather than its
//! own, end to end over the awsQuery wire.
//!
//! Six rules, each over a FAMILY of operations rather than over one call:
//!
//! 1. a value outside the PATTERN its member's shape publishes is refused HTTP 400
//!    `ValidationError` with AWS's one sentence for that FORM, naming the member the value
//!    was supplied for and nothing of the value — one sentence for the entity-name character
//!    class and one for an entity path — while the member's LENGTH is a separate requirement
//!    and keeps the model-level walk's own clause;
//! 2. a `Marker` the service did not issue is refused with AWS's one fixed sentence,
//!    `Invalid Marker.`, whichever collection was being paged, under the code IAM answers a
//!    value it cannot accept with (`ValidationError`) where the operation models no
//!    client-input error and under its modelled `InvalidInput` where it does;
//! 3. a permissions-boundary DELETE on an entity that carries none refuses the missing SCOPE,
//!    naming the principal by the unique id its own read reports (`Principal <id> does not
//!    have scope`), while a name the account holds no USER under is `User <name> was not
//!    found.`;
//! 4. an entity that carries a boundary reports `PermissionsBoundaryType` `Policy` — the
//!    value AWS puts on the wire, not the name the model gives the enum's single entry;
//! 5. `RemoveRoleFromInstanceProfile` for a role the profile does not hold refuses the
//!    BINDING: `Role <role> in Instance Profile <profile> cannot be found.`;
//! 6. `GetPolicyVersion` is addressed by a `PolicyArn` AND a `VersionId`, so an ARN it
//!    resolves nothing under is refused in the sentence naming both, and a key outside the
//!    published form of an account property is refused under `ValidationError` with the
//!    constraint envelope AWS states for a map whose KEYS failed.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: which
//! operation and which member carry the value, which published form it breaks and HOW
//! (character class, missing delimiter, length), which collection is paged, which entity kind
//! the boundary is set on, and the form a name misses a held entity by. No single request is
//! the specification, and every assertion compares the WHOLE message.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which a fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The wire code the query protocol names a refused input value with, and the one IAM
/// answers a value it cannot accept with. Written out rather than read from the
/// implementation: what these suites pin is the code a client branches on.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// AWS's sentence for a value outside the character class published for an entity NAME,
/// with the member it was supplied for in place of `{member}`.
///
/// Source: the recorded refusal of a `userName` carrying a character outside the class. The
/// class is the `@pattern` every name shape in the model declares — `^[\w+=,.@-]+$` on
/// `userNameType`, `roleNameType`, `groupNameType`, `instanceProfileNameType`,
/// `policyNameType` (`smithy_query(query="shape", shape="userNameType,roleNameType")`) — so
/// the sentence is the same for every member targeting one of them, which is what the rows
/// below read back on members the recording never named.
fn invalid_name(member: &str) -> String {
    format!(
        "The specified value for {member} is invalid. It must contain only alphanumeric \
         characters and/or the following: +=,.@_-"
    )
}

/// AWS's sentence for a value outside the pattern published for an entity PATH
/// (`pathType`): a path is delimited by `/` at BOTH ends, where a path PREFIX only has to
/// open with one.
fn invalid_path(member: &str) -> String {
    format!(
        "The specified value for {member} is invalid. It must begin and end with / and \
         contain only alphanumeric characters and/or / characters."
    )
}

/// AWS's one fixed sentence for a pagination `Marker` the service did not issue: it names
/// neither the token nor the collection.
const INVALID_MARKER: &str = "Invalid Marker.";

/// The `PermissionsBoundaryType` AWS puts on the wire for an entity carrying a boundary.
const BOUNDARY_TYPE: &str = "Policy";

/// An AWS-managed policy ARN, used as a boundary and as an attachment: an entry of the AWS
/// Managed Policy Reference.
const AWS_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// Assert `action` with `params` is refused HTTP 400 under `code` with exactly `expected`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
    code: &str,
    expected: &str,
) {
    let (status, body) = call(router, action, &borrowed(params)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    let answered = error_code(&body);
    assert_eq!(
        answered, code,
        "case {case:?}: expected the code {code:?}, got {answered:?}: {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// One row of the name-form table: the call to make, the member carrying the malformed
/// value, and the value itself.
struct NameCase {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    value: &'static str,
    /// The other members the request must carry for the name to be the only thing that can
    /// refuse it.
    extra: &'static [(&'static str, &'static str)],
}

/// RULE 1a: a value outside the entity-name character class is refused 400
/// `ValidationError` with AWS's one sentence for that class, naming the member it was
/// supplied for — on every operation and every member whose shape publishes the class, and
/// for every way a value can be outside it.
#[tokio::test]
async fn a_name_outside_its_character_class_is_refused_with_awss_sentence() {
    let router = router();
    // Rows span the member (five different name members, across a read and four creates),
    // the operation that carries it, and HOW the value is outside the class: a slash, a
    // space, a colon and an exclamation mark. The class is the regex engine's `\w` plus the
    // punctuation it names, so a letter outside ASCII is INSIDE it and is no row here.
    let rows = &[
        NameCase {
            case: "a user name carrying a slash, on the read",
            action: "GetUser",
            member: "UserName",
            value: "has/slash",
            extra: &[],
        },
        NameCase {
            case: "a user name carrying a space, on the create",
            action: "CreateUser",
            member: "UserName",
            value: "has space",
            extra: &[],
        },
        NameCase {
            case: "a role name carrying a colon",
            action: "CreateRole",
            member: "RoleName",
            value: "has:colon",
            extra: &[("AssumeRolePolicyDocument", TRUST_POLICY)],
        },
        NameCase {
            case: "a group name carrying an exclamation mark",
            action: "CreateGroup",
            member: "GroupName",
            value: "group!",
            extra: &[],
        },
        NameCase {
            case: "an instance profile name carrying a space",
            action: "CreateInstanceProfile",
            member: "InstanceProfileName",
            value: "has space",
            extra: &[],
        },
        NameCase {
            case: "a policy name carrying a slash",
            action: "CreatePolicy",
            member: "PolicyName",
            value: "team/policy",
            extra: &[("PolicyDocument", POLICY_DOCUMENT)],
        },
        NameCase {
            case: "a group name carrying a space, on the membership write",
            action: "AddUserToGroup",
            member: "GroupName",
            value: "has space",
            extra: &[("UserName", "a-user")],
        },
    ];

    for row in rows {
        let mut params = vec![param(row.member, row.value)];
        for (member, value) in row.extra {
            params.push(param(member, value));
        }
        let expected = invalid_name(&lower_camel(row.member));
        assert_refused(
            &router,
            row.case,
            row.action,
            &params,
            VALUE_REFUSAL_CODE,
            &expected,
        )
        .await;
        // Nothing of the value, however it was spelled.
        let (_, body) = call(&router, row.action, &borrowed(&params)).await;
        assert!(
            !body.contains(row.value),
            "case {:?}: the refusal must not echo {:?}, got {body}",
            row.case,
            row.value
        );
    }
}

/// The member name a refusal states, from the member name a request spells: the refusal
/// names it in lowerCamel, as the framework builds a member path.
fn lower_camel(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// RULE 1b: the character-class sentence states no LENGTH, so a name past the length its
/// member declares is a different requirement and keeps the model-level walk's own clause —
/// which is what shows the fixed sentence is not a blanket refusal of the member.
#[tokio::test]
async fn a_name_past_its_declared_length_keeps_the_model_level_clause() {
    let router = router();
    // `userNameType` is length 1..64 and `groupNameType` 1..128
    // (`smithy_query(query="shape", shape="userNameType,groupNameType")`): one character
    // past each published bound, spelled inside the character class so the length is the
    // only requirement broken.
    let rows: &[(&str, &str, &str, usize, i64)] = &[
        ("a user name one past its bound", "CreateUser", "UserName", 65, 64),
        ("a group name one past its bound", "CreateGroup", "GroupName", 129, 128),
    ];
    for (case, action, member, length, bound) in rows {
        let value = "a".repeat(*length);
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: \
             Member must have length less than or equal to {bound}",
            lower_camel(member)
        );
        assert_refused(
            &router,
            case,
            action,
            &[param(member, &value)],
            VALUE_REFUSAL_CODE,
            &expected,
        )
        .await;
    }

    // And a name AT each bound is served, so the bound itself is not what refuses.
    for (action, member, length) in [("CreateUser", "UserName", 64), ("CreateGroup", "GroupName", 128)] {
        let value = "a".repeat(length);
        let (status, body) = call(&router, action, &[(member, value.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {action:?} at the bound: expected HTTP 200, got {status}: {body}"
        );
    }
}

/// RULE 1c: a `Path` outside the published form of `pathType` is refused with AWS's one
/// sentence for that member, on every operation that takes one — whether the value is
/// outside the form in a way the model's own pattern catches or in one only the published
/// form does (a path that opens with the delimiter but does not close with it).
#[tokio::test]
async fn a_path_outside_its_published_form_is_refused_with_awss_sentence() {
    let router = router();
    // Rows: the five creates whose `Path` targets `pathType`
    // (`smithy_query(query="operation", operation="CreateUser,CreateRole,CreateGroup")`;
    // `CreatePolicy`'s targets `policyPathType`, a different published form), and the ways a
    // value misses that form — no leading delimiter, no trailing one, neither, an empty
    // inner segment, and an inner character outside the published ASCII range.
    let malformed = [
        "eng/",
        "/eng",
        "eng",
        "//",
        "/has space/",
        "/caf\u{e9}/",
    ];
    let takers: &[(&str, &[(&str, &str)])] = &[
        ("CreateUser", &[("UserName", "path-user")]),        (
            "CreateRole",
            &[
                ("RoleName", "path-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        ),
        ("CreateGroup", &[("GroupName", "path-group")]),
        (
            "CreateInstanceProfile",
            &[("InstanceProfileName", "path-profile")],
        ),
        (
            "CreateVirtualMFADevice",
            &[("VirtualMFADeviceName", "path-device")],
        ),
    ];
    let expected = invalid_path("path");
    for (action, extra) in takers {
        for path in malformed {
            let mut params = vec![param("Path", path)];
            for (member, value) in *extra {
                params.push(param(member, value));
            }
            let case = format!("{action} with Path={path:?}");
            assert_refused(
                &router,
                &case,
                action,
                &params,
                VALUE_REFUSAL_CODE,
                &expected,
            )
            .await;
        }
    }

    // A path INSIDE the form is served on each of them, so the sentence is not a blanket
    // refusal of the member: the account root, a one-segment path and a nested one.
    for (index, path) in ["/", "/eng/", "/eng/tools/"].into_iter().enumerate() {
        for (action, extra) in takers {
            let mut params = vec![param("Path", path)];
            for (member, value) in *extra {
                // One entity per row: a create that repeats a held name is a different
                // refusal, which this suite is not about. Only the NAME member varies —
                // a document member is passed through as it is.
                if member.ends_with("Name") {
                    params.push(param(member, &format!("{value}-{index}")));
                } else {
                    params.push(param(member, value));
                }
            }
            let (status, body) = call_action(action, &router, &params).await;
            assert_eq!(
                status, 200,
                "case {action:?} with Path={path:?}: expected HTTP 200, got {status}: {body}"
            );
        }
    }
}

/// `call` with the router and the action in the order this suite's well-formed pass reads
/// best, answering the status and body.
async fn call_action(
    action: &str,
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, action, &borrowed(params)).await
}

/// One row of the marker table: the paginated call, the members it needs, and the wire code
/// its model gives a refused input value.
struct MarkerCase {
    action: &'static str,
    extra: Vec<(String, String)>,
    code: &'static str,
}

/// RULE 2: a `Marker` this service did not issue is refused with AWS's one fixed sentence,
/// whichever collection the call pages and whichever code its own model gives the refusal.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_is_refused_with_awss_fixed_sentence() {
    let router = router();
    // One held user, group and policy, so each paginated call below is refused by the
    // MARKER rather than by a missing entity.
    for (action, params) in [
        ("CreateUser", vec![param("UserName", "marker-user")]),
        ("CreateGroup", vec![param("GroupName", "marker-group")]),
        (
            "CreatePolicy",
            vec![
                param("PolicyName", "marker-policy"),
                param("PolicyDocument", POLICY_DOCUMENT),
            ],
        ),
    ] {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "the {action} fixture must succeed, got {status}: {body}"
        );
    }
    let policy_arn = format!("arn:aws:iam::{}:policy/marker-policy", account_id());

    // Rows span the collection being paged — an account-wide listing, a listing scoped to
    // one entity, a tag listing and a policy's versions — and so both codes a refused
    // marker is answered under: `ValidationError` where the operation models no
    // client-input error, its modelled `InvalidInput` where it does.
    let rows = vec![
        MarkerCase {
            action: "ListUsers",
            extra: Vec::new(),
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListRoles",
            extra: Vec::new(),
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListGroups",
            extra: Vec::new(),
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListPolicies",
            extra: Vec::new(),
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListAccountAliases",
            extra: Vec::new(),
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListUserTags",
            extra: vec![param("UserName", "marker-user")],
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListGroupsForUser",
            extra: vec![param("UserName", "marker-user")],
            code: VALUE_REFUSAL_CODE,
        },
        MarkerCase {
            action: "ListAttachedUserPolicies",
            extra: vec![param("UserName", "marker-user")],
            code: "InvalidInput",
        },
        MarkerCase {
            action: "ListPolicyVersions",
            extra: vec![param("PolicyArn", &policy_arn)],
            code: "InvalidInput",
        },
        MarkerCase {
            action: "ListEntitiesForPolicy",
            extra: vec![param("PolicyArn", &policy_arn)],
            code: "InvalidInput",
        },
    ];

    // The tokens: text that is no encoded token at all, text that decodes to foreign bytes,
    // and base64 that decodes to text this service never wrote.
    let tokens = ["not-a-marker", "zz", "Zm9yZWlnbi10b2tlbg=="];

    for row in &rows {
        for token in tokens {
            let mut params = row.extra.clone();
            params.push(param("Marker", token));
            let case = format!("{} with Marker={token:?}", row.action);
            assert_refused(
                &router,
                &case,
                row.action,
                &params,
                row.code,
                INVALID_MARKER,
            )
            .await;
        }
    }
}

/// Set the boundary of `entity_name` and read the type back through the entity's own read.
async fn put_boundary(
    router: &MultiServiceRouter,
    action: &str,
    name_member: &str,
    entity_name: &str,
    boundary: &str,
) {
    let (status, body) = call(
        router,
        action,
        &[
            (name_member, entity_name),
            ("PermissionsBoundary", boundary),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {entity_name:?}: the boundary put must succeed, got {status}: {body}"
    );
}

/// RULE 4: an entity carrying a permissions boundary reports the type AWS puts on the wire
/// (`Policy`) beside the boundary's ARN — on a role and on a user, whichever policy the
/// boundary names and whether a create or a put set it.
#[tokio::test]
async fn a_reported_permissions_boundary_carries_awss_wire_type() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "boundary-policy"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(status, 200, "the policy fixture must succeed, got {body}");
    let customer_policy = element(&body, "Arn");

    // Rows: the entity kind, how the boundary came to be set (the create itself, or a put),
    // and whether the boundary names an AWS-managed or a customer-managed policy.
    let boundaries = [AWS_MANAGED_POLICY.to_string(), customer_policy];
    for (index, boundary) in boundaries.iter().enumerate() {
        // Set by the create.
        let user = format!("created-bounded-user-{index}");
        let (status, body) = call(
            &router,
            "CreateUser",
            &[
                ("UserName", user.as_str()),
                ("PermissionsBoundary", boundary.as_str()),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {user:?}: expected 200, got {body}");
        assert_eq!(
            element(&body, "PermissionsBoundaryType"),
            BOUNDARY_TYPE,
            "case {user:?}: the create must report AWS's wire type, got {body}"
        );
        let (status, body) = call(&router, "GetUser", &[("UserName", user.as_str())]).await;
        assert_eq!(status, 200, "case {user:?}: expected 200, got {body}");
        assert_eq!(
            element(&body, "PermissionsBoundaryType"),
            BOUNDARY_TYPE,
            "case {user:?}: GetUser must report AWS's wire type, got {body}"
        );
        assert_eq!(
            element(&body, "PermissionsBoundaryArn"),
            *boundary,
            "case {user:?}: GetUser must report the boundary that was set, got {body}"
        );

        // Set by a put, on a role.
        let role = format!("put-bounded-role-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {role:?}: expected 200, got {body}");
        put_boundary(
            &router,
            "PutRolePermissionsBoundary",
            "RoleName",
            &role,
            boundary,
        )
        .await;
        let (status, body) = call(&router, "GetRole", &[("RoleName", role.as_str())]).await;
        assert_eq!(status, 200, "case {role:?}: expected 200, got {body}");
        assert_eq!(
            element(&body, "PermissionsBoundaryType"),
            BOUNDARY_TYPE,
            "case {role:?}: GetRole must report AWS's wire type, got {body}"
        );
    }
}

/// RULE 3: a permissions-boundary delete on an entity that carries none refuses the missing
/// SCOPE, naming the principal by the unique id the entity's own read reports — on a role
/// and on a user, however the entity came to carry no boundary.
#[tokio::test]
async fn clearing_a_boundary_an_entity_does_not_carry_names_the_principal_id() {
    let router = router();

    // Rows: the entity kind (so which unique-id prefix the message carries), the path depth,
    // the case the delete spells the name in, and how the entity came to carry no
    // boundary — never bounded, or bounded and cleared already.
    let rows: &[(&str, &str, &str, &str, bool)] = &[
        ("a user never bounded", "never-bounded-user", "/", "never-bounded-user", false),
        ("a user bounded and cleared", "cleared-user", "/eng/tools/", "CLEARED-USER", true),
        ("a role never bounded", "never-bounded-role", "/", "NEVER-BOUNDED-ROLE", false),
        ("a role bounded and cleared", "cleared-role", "/sales/", "cleared-role", true),
    ];

    for (case, name, path, spelled, was_bounded) in rows {
        let is_role = name.ends_with("role");
        let (create, read, delete, name_member, put) = if is_role {
            (
                "CreateRole",
                "GetRole",
                "DeleteRolePermissionsBoundary",
                "RoleName",
                "PutRolePermissionsBoundary",
            )
        } else {
            (
                "CreateUser",
                "GetUser",
                "DeleteUserPermissionsBoundary",
                "UserName",
                "PutUserPermissionsBoundary",
            )
        };
        let mut params = vec![param(name_member, name), param("Path", path)];
        if is_role {
            params.push(param("AssumeRolePolicyDocument", TRUST_POLICY));
        }
        let (status, body) = call(&router, create, &borrowed(&params)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body}");
        let id_tag = if is_role { "RoleId" } else { "UserId" };
        let unique_id = element(&body, id_tag);
        assert!(
            unique_id.starts_with(if is_role { "AROA" } else { "AIDA" }),
            "case {case:?}: expected the entity's unique id, got {unique_id:?}"
        );

        if *was_bounded {
            put_boundary(&router, put, name_member, name, AWS_MANAGED_POLICY).await;
            let (status, body) = call(&router, delete, &[(name_member, name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first clear must be served, got {status}: {body}"
            );
        }

        let (status, body) = call(&router, delete, &[(name_member, spelled)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Principal {unique_id} does not have scope"),
            "case {case:?}: expected the principal id in the refusal, got {message:?}"
        );
        // The refusal describes the scope, so it carries nothing of the name the request
        // spelled, and the entity itself is still reported.
        assert!(
            !message.contains(*spelled),
            "case {case:?}: the refusal must not name the entity, got {message:?}"
        );
        let (status, body) = call(&router, read, &[(name_member, name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the entity must survive the refusal, got {status}: {body}"
        );
        assert_eq!(
            element(&body, id_tag),
            unique_id,
            "case {case:?}: the entity must read back unchanged, got {body}"
        );
    }
}

/// RULE 3 (the other half): `DeleteUserPermissionsBoundary` words a name the account holds
/// no user under in its own sentence, `User <name> was not found.`
#[tokio::test]
async fn clearing_the_boundary_of_an_absent_user_is_worded_for_the_missing_user() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "HeldUser")]).await;
    assert_eq!(status, 200, "the fixture user must be created, got {body}");

    // Rows spanning the forms a name can miss a held user by: never created, a near miss of
    // a held name, a held name with a separator the folded name does not have, and the whole
    // punctuation the name alphabet admits.
    for name in [
        "NoSuchUser",
        "HeldUser2",
        "held_user",
        "another+name=entirely.x@-",
    ] {
        let (status, body) =
            call(&router, "DeleteUserPermissionsBoundary", &[("UserName", name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("User {name} was not found."),
            "case {name:?}: expected the absent-user sentence, got {message:?}"
        );
    }
}

/// RULE 5: `RemoveRoleFromInstanceProfile` for a role the profile does not hold refuses the
/// BINDING, naming the role and the profile the request named.
#[tokio::test]
async fn removing_a_role_a_profile_does_not_hold_names_the_binding() {
    let router = router();
    // Two roles and two profiles, one binding: enough for a row where the role is held by
    // ANOTHER profile and a row where it is in none.
    for role in ["bound-role", "unbound-role"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(status, 200, "the {role} fixture must succeed, got {body}");
    }
    for profile in ["holding-profile", "empty-profile"] {
        let (status, body) = call(
            &router,
            "CreateInstanceProfile",
            &[("InstanceProfileName", profile)],
        )
        .await;
        assert_eq!(status, 200, "the {profile} fixture must succeed, got {body}");
    }
    let (status, body) = call(
        &router,
        "AddRoleToInstanceProfile",
        &[
            ("InstanceProfileName", "holding-profile"),
            ("RoleName", "bound-role"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the binding fixture must succeed, got {body}");

    // Rows: a role in no profile at all, a role held by another profile, the held binding
    // after it has been removed, and the held pair spelled in another case — IAM entity
    // names are not distinguished by case, so the refusal quotes the names the REQUEST
    // spelled.
    let (status, body) = call(
        &router,
        "RemoveRoleFromInstanceProfile",
        &[
            ("InstanceProfileName", "holding-profile"),
            ("RoleName", "bound-role"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the first removal must succeed, got {body}");

    let rows: &[(&str, &str, &str)] = &[
        ("a role in no profile", "empty-profile", "unbound-role"),
        ("a role held by another profile", "empty-profile", "bound-role"),
        (
            "the binding that was already removed",
            "holding-profile",
            "bound-role",
        ),
        (
            "the removed binding spelled in another case",
            "HOLDING-PROFILE",
            "BOUND-ROLE",
        ),
    ];
    for (case, profile, role) in rows {
        let (status, body) = call(
            &router,
            "RemoveRoleFromInstanceProfile",
            &[("InstanceProfileName", profile), ("RoleName", role)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {body}"
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Role {role} in Instance Profile {profile} cannot be found."),
            "case {case:?}: expected the binding sentence, got {message:?}"
        );
    }
}

/// RULE 6a: `GetPolicyVersion` is addressed by a `PolicyArn` AND a `VersionId`, so an ARN it
/// resolves no policy under is refused in the sentence that names both — the same sentence a
/// version the resolved policy does not carry answers with, because the call cannot tell the
/// two apart.
#[tokio::test]
async fn a_policy_version_read_names_the_version_it_was_addressed_by() {
    let router = router();
    let account = account_id();
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "HeldVersionedPolicy"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(status, 200, "the policy fixture must succeed, got {body}");
    let held = element(&body, "Arn");

    // Rows: an ARN the account holds nothing under (in three forms) against a version id in
    // three forms, plus the HELD policy with a version it does not carry — one sentence for
    // both absences.
    let absent = [
        format!("arn:aws:iam::{account}:policy/NeverCreated"),
        format!("arn:aws:iam::{account}:policy/elsewhere/HeldVersionedPolicy"),
        "arn:aws:iam::210987654321:policy/HeldVersionedPolicy".to_string(),
    ];
    for arn in absent.iter().chain([&held]) {
        for version in ["v1", "v4", "v2.1"] {
            if arn == &held && version == "v1" {
                // The held policy DOES carry v1: that row is the control below.
                continue;
            }
            let case = format!("{arn} at {version}");
            let expected =
                format!("Policy {arn} version {version} does not exist or is not attachable.");
            let (status, body) = call(
                &router,
                "GetPolicyVersion",
                &[("PolicyArn", arn.as_str()), ("VersionId", version)],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The control: the version the held policy does carry is served, so the sentence is
    // about the pair the request named.
    let (status, body) = call(
        &router,
        "GetPolicyVersion",
        &[("PolicyArn", held.as_str()), ("VersionId", "v1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the held version must be served, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "VersionId"),
        "v1",
        "expected the held version to be reported, got {body}"
    );
}

/// RULE 6b: an account-property KEY outside its published form is refused under the code IAM
/// answers a value it cannot accept with, carrying the constraint envelope AWS states for a
/// map whose KEYS failed — one clause naming the map member, the key shape's whole declared
/// set of constraints in one bracket, and nothing of the key.
#[tokio::test]
async fn an_account_property_key_outside_its_published_form_is_refused() {
    let router = router();
    // `accountPropertyKeyType` is length 1..50 with pattern `^[A-Za-z][A-Za-z0-9/_-]*$`
    // (`smithy_query(query="shape", shape="accountPropertiesMapType")`).
    let expected = "1 validation error detected: Value at 'properties' failed to satisfy \
                    constraint: Map keys must satisfy constraint: [Member must have length \
                    less than or equal to 50, Member must have length greater than or equal \
                    to 1, Member must satisfy regular expression pattern: \
                    ^[A-Za-z][A-Za-z0-9/_-]*$]";
    // Rows: a key opening with a digit, one carrying a character outside the alphabet, one
    // carrying a space, and one a character past the published maximum length.
    let malformed = [
        "9RoleManager/Enabled".to_string(),
        "Role!Manager/Enabled".to_string(),
        "Role Manager/Enabled".to_string(),
        format!("R{}", "B".repeat(50)),
    ];
    for key in malformed {
        let params = vec![
            param("Properties.entry.1.key", &key),
            param("Properties.entry.1.value", "true"),
        ];
        let case = format!("a property key {key:?}");
        assert_refused(
            &router,
            &case,
            "PutAccountProperties",
            &params,
            VALUE_REFUSAL_CODE,
            expected,
        )
        .await;
    }

    // A key under a namespace the service does not serve is a different rule, and is
    // answered under the same code — what a client branches on is one code for a value this
    // service cannot accept.
    let params = vec![
        param("Properties.entry.1.key", "NotAServedNamespace/Enabled"),
        param("Properties.entry.1.value", "true"),
    ];
    assert_refused(
        &router,
        "a key under an unserved namespace",
        "PutAccountProperties",
        &params,
        VALUE_REFUSAL_CODE,
        "Unsupported namespace: NotAServedNamespace",
    )
    .await;
}
