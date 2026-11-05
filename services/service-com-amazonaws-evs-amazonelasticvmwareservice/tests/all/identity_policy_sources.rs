// SPDX-License-Identifier: CC0-1.0

//! WHICH identity-based documents this service's authorization gate reads for a caller.
//!
//! The rule under test: the identity-based documents in force on a calling principal are
//! its INLINE policies plus the DEFAULT version of every MANAGED policy attached to it,
//! read out of the IAM service's own store — the same two sources for every kind of
//! principal the gate resolves (an assumed-role session and an IAM user alike). So an
//! explicit `Deny` of this service's actions refuses the call wherever of those places it
//! is held, and one document attached to two kinds of principal decides both the same
//! way.
//!
//! What is NOT in force is as much of the rule as what is: a version of an attached
//! policy that is not its default is not the document the attachment puts in force, and
//! an attachment this account holds no document for carries no statement the gate can
//! read — and since only an explicit deny refuses here (see
//! `tests/all/authorization.rs`, which owns that part of the rule), neither refuses
//! anything.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    Policy, PolicyVersion, User,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use crate::environment_lifecycle::{
    call_as, create, emulator, error_type, Emulator, ACCOUNT, REGION,
};

/// The wire error code the AWS authorization layer refuses with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// How long the sessions these tests register last — long enough that none expires while
/// the test runs.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// A document denying every action of this service, on every resource.
const DENY_EVS: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"evs:*","Resource":"*"}]}"#;

/// A document allowing every action of this service, denying nothing.
const ALLOW_EVS: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"evs:*","Resource":"*"}]}"#;

/// A document denying another service's actions: it says nothing about this one.
const DENY_EC2: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"ec2:*","Resource":"*"}]}"#;

/// A document denying this service, but only on ONE environment — never the environment
/// the requests below address, which the test creates and whose id this service mints.
const DENY_OTHER_ENVIRONMENT: &str = r#"{"Version":"2012-10-17","Statement":[
    {"Effect":"Deny","Action":"evs:*",
     "Resource":"arn:aws:evs:us-west-2:000000000000:environment/env-0000000000"}
]}"#;

/// An ARN in AWS's OWN managed-policy namespace (`arn:aws:iam::aws:policy/…`) that the
/// IAM emulator carries no document for, so an attachment of it is a grant whose
/// statements nothing here can read.
const UNREADABLE_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AWSDenyAll";

/// Which kind of IAM principal a request speaks for: the two forms this service's gate
/// resolves out of a caller ARN.
#[derive(Clone, Copy, Debug)]
enum Principal {
    /// An `sts:AssumeRole` session of a role, reported as
    /// `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
    AssumedRole,
    /// An IAM user, as an `sts:GetSessionToken` credential minted for that user's own
    /// long-term access key reports it: the user's own ARN. `path` is the user's IAM
    /// path, which its ARN carries and its name does not.
    IamUser { path: &'static str },
}

/// Every principal kind a row below is run for.
const PRINCIPALS: [(&str, Principal); 3] = [
    ("an assumed-role session", Principal::AssumedRole),
    ("an IAM user", Principal::IamUser { path: "/" }),
    (
        "an IAM user under a path",
        Principal::IamUser { path: "/team/evs/" },
    ),
];

/// Where a row holds the documents it puts in the account.
#[derive(Clone, Copy, Debug)]
enum Document {
    /// An inline policy embedded on the principal itself, where `iam:PutRolePolicy` and
    /// `iam:PutUserPolicy` write one.
    Inline(&'static str),
    /// A managed policy the account holds, attached to the principal — what
    /// `iam:CreatePolicy` plus `iam:AttachRolePolicy` / `iam:AttachUserPolicy` leave
    /// behind. Its versions, as `(version id, document, is the default version)`.
    Attached(&'static [(&'static str, &'static str, bool)]),
    /// An attachment of a policy whose document this account does not hold.
    AttachedUnreadable(&'static str),
}

/// Put one IAM user into the account, where `iam:CreateUser` writes it, and answer it.
fn seed_user(engine: &engine::InMemoryEngine, path: &str, user_name: &str) -> User {
    let user = User {
        path: path.to_string(),
        user_name: user_name.to_string(),
        user_id: format!("AIDA{}", user_name.to_ascii_uppercase()),
        arn: iam::user_arn(path, user_name),
        create_date: iam::now(),
        password_last_used: None,
        permissions_boundary: None,
        tags: None,
    };
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    state
        .lock()
        .users
        .insert(iam::name_key(user_name), user.clone());
    user
}

/// Register the credential `access_key_id` speaks for in STS's OWN store — the store this
/// service resolves a caller through — and answer the principal ARN it reports.
///
/// Written through the store rather than by calling `sts:AssumeRole` or
/// `sts:GetSessionToken`, because what is under test is what this service does with the
/// principal a credential names, not how the credential was minted.
fn register_principal(
    engine: &engine::InMemoryEngine,
    access_key_id: &str,
    principal: Principal,
    name: &str,
) -> String {
    let (arn, user_id, kind, role_arn) = match principal {
        Principal::AssumedRole => {
            let arn = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{name}/session-1");
            let user_id = format!("{}:session-1", sts::role_id(&arn));
            (
                arn,
                user_id,
                sts::SessionKind::AssumedRole,
                Some(iam::role_arn("/", name)),
            )
        }
        Principal::IamUser { path } => {
            let user = seed_user(engine, path, name);
            (
                user.arn,
                user.user_id,
                sts::SessionKind::SessionToken,
                None,
            )
        }
    };
    let now = SystemTime::now();
    let state = sts::service_state(engine).expect("the in-memory engine carries the STS store");
    state
        .lock()
        .insert(sts::Session {
            access_key_id: access_key_id.to_string(),
            created_at: now,
            expires_at: now + SESSION_LIFETIME,
            duration_seconds: SESSION_LIFETIME.as_secs() as i32,
            identity: sts::Identity {
                user_id,
                account: ACCOUNT.to_string(),
                arn: arn.clone(),
            },
            kind,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: sts::SessionPolicies::default(),
            external_id: None,
            role_arn,
            revoked: false,
        })
        .expect("the session registry has room for one session");
    arn
}

/// Embed an inline identity policy on the principal, where `iam:PutRolePolicy` and
/// `iam:PutUserPolicy` write one.
fn put_inline_policy(
    engine: &engine::InMemoryEngine,
    principal: Principal,
    name: &str,
    policy_name: &str,
    document: &str,
) {
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    let held = match principal {
        Principal::AssumedRole => &mut store.role_inline_policies,
        Principal::IamUser { .. } => &mut store.user_inline_policies,
    };
    held.entry(iam::name_key(name)).or_default().insert(
        iam::name_key(policy_name),
        iam::InlinePolicy {
            policy_name: policy_name.to_string(),
            policy_document: document.to_string(),
        },
    );
}

/// Attach the managed policy `policy_arn` to the principal, where
/// `iam:AttachRolePolicy` and `iam:AttachUserPolicy` write the attachment.
fn attach_policy(
    engine: &engine::InMemoryEngine,
    principal: Principal,
    name: &str,
    policy_arn: &str,
) {
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    let attached = match principal {
        Principal::AssumedRole => &mut store.role_attached_policies,
        Principal::IamUser { .. } => &mut store.user_attached_policies,
    };
    attached
        .entry(iam::name_key(name))
        .or_default()
        .insert(policy_arn.to_string());
}

/// Put a managed policy the account owns into the IAM store, where `iam:CreatePolicy`
/// (plus a `iam:CreatePolicyVersion` per extra version) writes it, and answer its ARN.
///
/// `versions` carries each version's id, document and whether it is the policy's DEFAULT
/// version — the one in force on every entity the policy is attached to.
fn create_managed_policy(
    engine: &engine::InMemoryEngine,
    policy_name: &str,
    versions: &[(&str, &str, bool)],
) -> String {
    let arn = iam::policy_arn("/", policy_name);
    let created = iam::now();
    let default_version_id = versions
        .iter()
        .find(|(_, _, is_default)| *is_default)
        .map(|(version_id, _, _)| (*version_id).to_string());
    let held: BTreeMap<String, PolicyVersion> = versions
        .iter()
        .map(|(version_id, document, is_default)| {
            (
                (*version_id).to_string(),
                PolicyVersion {
                    document: Some((*document).to_string()),
                    version_id: Some((*version_id).to_string()),
                    is_default_version: *is_default,
                    create_date: Some(created),
                },
            )
        })
        .collect();
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    state.lock().policies.insert(
        iam::name_key(policy_name),
        iam::ManagedPolicy {
            policy: Policy {
                arn: Some(arn.clone()),
                policy_id: Some(format!("ANPA{}", policy_name.to_ascii_uppercase())),
                policy_name: Some(policy_name.to_string()),
                path: Some("/".to_string()),
                default_version_id,
                attachment_count: Some(1),
                permissions_boundary_usage_count: Some(0),
                is_attachable: true,
                description: None,
                create_date: Some(created),
                update_date: Some(created),
                tags: None,
            },
            versions: held,
        },
    );
    arn
}

/// A caller: a credential registered for `principal`, with every document of `documents`
/// held where that variant says. Answers the access key id to sign with and the principal
/// ARN a refusal names.
fn caller(
    router: &Emulator,
    label: &str,
    principal: Principal,
    documents: &[Document],
) -> (String, String) {
    let access_key_id = format!("ASIA{}", label.to_ascii_uppercase().replace('-', ""));
    let name = format!("evs-{label}");
    let arn = register_principal(&router.engine, &access_key_id, principal, &name);
    for (index, document) in documents.iter().enumerate() {
        match document {
            Document::Inline(policy_document) => put_inline_policy(
                &router.engine,
                principal,
                &name,
                &format!("{label}-inline-{index}"),
                policy_document,
            ),
            Document::Attached(versions) => {
                let policy_arn = create_managed_policy(
                    &router.engine,
                    &format!("{label}-managed-{index}"),
                    versions,
                );
                attach_policy(&router.engine, principal, &name, &policy_arn);
            }
            Document::AttachedUnreadable(policy_arn) => {
                attach_policy(&router.engine, principal, &name, policy_arn)
            }
        }
    }
    (access_key_id, arn)
}

/// RULE: the documents in force on a principal are its inline policies plus the DEFAULT
/// version of every managed policy attached to it — the same two sources for an
/// assumed-role session and for an IAM user — so a deny held in any of them refuses the
/// call, and a document the store does not hold in force refuses nothing.
#[tokio::test]
async fn every_in_force_document_of_either_principal_kind_decides_the_request() {
    let router = emulator();
    let environment = create(&router, Some("document-sources")).await;
    let request = json!({ "environmentId": environment });

    // (case, the documents the account holds for the principal, whether the gate refuses)
    let rows: Vec<(&str, Vec<Document>, bool)> = vec![
        ("inline deny", vec![Document::Inline(DENY_EVS)], true),
        (
            "attached deny",
            vec![Document::Attached(&[("v1", DENY_EVS, true)])],
            true,
        ),
        (
            "attached deny in the default version",
            vec![Document::Attached(&[
                ("v1", ALLOW_EVS, false),
                ("v2", DENY_EVS, true),
            ])],
            true,
        ),
        (
            "attached deny outside the default version",
            vec![Document::Attached(&[
                ("v1", ALLOW_EVS, true),
                ("v2", DENY_EVS, false),
            ])],
            false,
        ),
        (
            "inline allow beside an attached deny",
            vec![
                Document::Inline(ALLOW_EVS),
                Document::Attached(&[("v1", DENY_EVS, true)]),
            ],
            true,
        ),
        (
            "inline deny beside an attached allow",
            vec![
                Document::Inline(DENY_EVS),
                Document::Attached(&[("v1", ALLOW_EVS, true)]),
            ],
            true,
        ),
        (
            "two attached policies, the second denying",
            vec![
                Document::Attached(&[("v1", ALLOW_EVS, true)]),
                Document::Attached(&[("v1", DENY_EVS, true)]),
            ],
            true,
        ),
        (
            "attached deny of another service",
            vec![Document::Attached(&[("v1", DENY_EC2, true)])],
            false,
        ),
        (
            "attached deny of another environment",
            vec![Document::Attached(&[("v1", DENY_OTHER_ENVIRONMENT, true)])],
            false,
        ),
        (
            "an attachment the account holds no document for",
            vec![Document::AttachedUnreadable(UNREADABLE_POLICY_ARN)],
            false,
        ),
        ("no document at all", Vec::new(), false),
    ];

    for (row, (case, documents, refused)) in rows.iter().enumerate() {
        for (principal_index, (principal_case, principal)) in PRINCIPALS.iter().enumerate() {
            let case = format!("{case} on {principal_case}");
            let (access_key_id, arn) = caller(
                &router,
                &format!("doc{row}p{principal_index}"),
                *principal,
                documents,
            );
            let (status, body) =
                call_as(&router, &access_key_id, REGION, "GetEnvironment", &request).await;
            if *refused {
                // Every CLIENT error of this awsJson1_0 service is reported with HTTP
                // 400 — `tests/all/error_wire_status.rs` owns that rule.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body}"
                );
                assert_eq!(
                    error_type(&body),
                    ACCESS_DENIED,
                    "case {case:?}: expected {ACCESS_DENIED}, got {} in {body}",
                    error_type(&body)
                );
                let message = body["message"].as_str().unwrap_or_default().to_string();
                for part in [
                    arn.as_str(),
                    "evs:GetEnvironment",
                    "with an explicit deny in an identity-based policy",
                ] {
                    assert!(
                        message.contains(part),
                        "case {case:?}: the refusal must name {part:?}, got {message:?}"
                    );
                }
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: no document in force denies this request, so it must \
                     reach this service's own handling, got {status} {body}"
                );
            }
        }
    }
}

/// RULE (the same one, from the other side): ONE managed policy decides every principal
/// it is attached to alike, whichever kind of principal that is and whichever operation
/// of this service is called.
#[tokio::test]
async fn one_attached_policy_decides_a_role_and_a_user_alike() {
    let router = emulator();
    let environment = create(&router, Some("shared-attachment")).await;
    let environment_arn = format!("arn:aws:evs:{REGION}:{ACCOUNT}:environment/{environment}");

    // One policy in the account, attached to every principal below: the deny the gate
    // must read, and an allow-only policy for the callers nothing denies.
    let deny_arn = create_managed_policy(&router.engine, "shared-deny", &[("v1", DENY_EVS, true)]);
    let allow_arn =
        create_managed_policy(&router.engine, "shared-allow", &[("v1", ALLOW_EVS, true)]);

    // (case, access key id, principal ARN, whether the gate refuses it)
    let mut callers: Vec<(String, String, String, bool)> = Vec::new();
    for (index, (principal_case, principal)) in PRINCIPALS.iter().enumerate() {
        for (effect, policy_arn, refused) in
            [("denied", &deny_arn, true), ("allowed", &allow_arn, false)]
        {
            let label = format!("shared{index}{effect}");
            let name = format!("evs-{label}");
            let access_key_id = format!("ASIA{}", label.to_ascii_uppercase());
            let arn = register_principal(&router.engine, &access_key_id, *principal, &name);
            attach_policy(&router.engine, *principal, &name, policy_arn);
            callers.push((
                format!("{principal_case} the policy {effect}"),
                access_key_id,
                arn,
                refused,
            ));
        }
    }

    // One request per operation shape the gate decides: one addressing an environment,
    // one addressing an environment through its ARN, and one addressing no resource at
    // all.
    let requests: Vec<(&str, Value)> = vec![
        ("GetEnvironment", json!({ "environmentId": environment })),
        (
            "AssociateEipToVlan",
            json!({
                "environmentId": environment,
                "vlanName": "hcx",
                "allocationId": "eipalloc-0123456789abcdef0",
            }),
        ),
        (
            "ListTagsForResource",
            json!({ "resourceArn": environment_arn }),
        ),
        ("ListEnvironments", json!({})),
        ("GetAccountSettings", json!({})),
    ];

    for (operation, request) in &requests {
        for (principal_case, access_key_id, arn, refused) in &callers {
            let case = format!("{operation} as {principal_case}");
            let (status, body) = call_as(&router, access_key_id, REGION, operation, request).await;
            if *refused {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body}"
                );
                assert_eq!(
                    error_type(&body),
                    ACCESS_DENIED,
                    "case {case:?}: expected {ACCESS_DENIED}, got {} in {body}",
                    error_type(&body)
                );
                let message = body["message"].as_str().unwrap_or_default().to_string();
                for part in [arn.as_str(), &format!("evs:{operation}")] {
                    assert!(
                        message.contains(part),
                        "case {case:?}: the refusal must name {part:?}, got {message:?}"
                    );
                }
            } else {
                assert_ne!(
                    error_type(&body),
                    ACCESS_DENIED,
                    "case {case:?}: the attached policy denies nothing, so the gate must \
                     not refuse it, got {body}"
                );
            }
        }
    }
}
