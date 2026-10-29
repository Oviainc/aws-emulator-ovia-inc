// SPDX-License-Identifier: CC0-1.0

//! Where the identity-based documents a CloudWatch RUM authorization decision is made from
//! come from, for every kind of IAM principal a request can be signed as.
//!
//! The rule under test: the identity-based documents in force on the calling principal are
//! its INLINE policies together with the DEFAULT version of every managed policy ATTACHED
//! to it, read from the IAM store's maps for that kind of principal — the role behind an
//! assumed-role session, or an IAM user — so one grant decides one way whichever of the two
//! places it is held in and whichever kind of principal holds it, and an explicit `Deny` in
//! either place refuses whatever the other says. A policy whose document the account does
//! not hold is a grant this emulator cannot read: an AWS-managed one (account segment
//! `aws`) is not refused on.
//!
//! The tables span what the rule generalises over: the kind of principal, where the grant
//! is held (inline, attached, both), what the document says (the operation's own action or
//! another one, over every resource or over one ARN, allowed or explicitly denied), which
//! version of an attached policy is its default, and the operation the decision is taken
//! for (a control-plane read and the data plane). Nothing is seeded into another service's
//! store by hand: the users, roles and managed policies are made through IAM's own API, the
//! session credential by STS's own `AssumeRole`, and the IAM-user credential by IAM's own
//! `CreateAccessKey` — the same path a client takes.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

use crate::authorization::{
    identity_policy, monitor_arn, rum_events_body, statement, Emulator, Reply,
};

/// The wire error code, and the status, a request IAM does not allow is refused with.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The status a call IAM allows answers with, for both operations below.
const ALLOWED_STATUS: u16 = 200;

/// A monitor ARN of this account that the calls below do NOT address, for the rows whose
/// statement is scoped to another resource. A statement is matched against the ARN the call
/// addresses, so nothing has to exist at this one.
const OTHER_MONITOR: &str = "sources-elsewhere";

/// The kind of IAM principal a request is signed as — the two the IAM store holds
/// identity-based policies for.
#[derive(Debug, Clone, Copy)]
enum PrincipalKind {
    AssumedRole,
    IamUser,
}

impl PrincipalKind {
    fn label(self) -> &'static str {
        match self {
            Self::AssumedRole => "an assumed-role session",
            Self::IamUser => "an IAM user",
        }
    }
}

/// The principal a case's call is signed as, once IAM holds it.
struct Signer {
    /// The access key id the RUM call carries in its credential scope.
    key: String,
    /// The IAM entity's own name, which an attachment names it by.
    name: String,
    kind: PrincipalKind,
}

/// Create the principal of `kind` named `name`, carrying `inline` as its one inline policy
/// when one is given, and answer the credential a caller signs with for it.
async fn signer(
    emulator: &Emulator,
    kind: PrincipalKind,
    name: &str,
    inline: Option<&str>,
) -> Signer {
    let key = match kind {
        PrincipalKind::AssumedRole => emulator.assumed_role_key(name, inline).await,
        PrincipalKind::IamUser => emulator.iam_user_key(name, inline).await,
    };
    Signer {
        key,
        name: name.to_string(),
        kind,
    }
}

/// Attach the managed policy `policy_arn` to this principal, through IAM's own attach call
/// for its kind.
async fn attach(emulator: &Emulator, signer: &Signer, policy_arn: &str) {
    match signer.kind {
        PrincipalKind::AssumedRole => emulator.attach_role_policy(&signer.name, policy_arn).await,
        PrincipalKind::IamUser => emulator.attach_user_policy(&signer.name, policy_arn).await,
    }
}

/// The operation the decision under test is taken for.
#[derive(Debug, Clone, Copy)]
enum Operation {
    GetAppMonitor,
    PutRumEvents,
}

impl Operation {
    /// The IAM action this operation is authorized against
    /// (`crate::state::authorization`).
    fn action(self) -> &'static str {
        match self {
            Self::GetAppMonitor => "rum:GetAppMonitor",
            Self::PutRumEvents => "rum:PutRumEvents",
        }
    }

    /// Another action of this service, which this operation is NOT authorized against, so a
    /// document granting it grants this operation nothing.
    fn other_action(self) -> &'static str {
        match self {
            Self::GetAppMonitor => "rum:PutRumEvents",
            Self::PutRumEvents => "rum:GetAppMonitor",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::GetAppMonitor => "GetAppMonitor",
            Self::PutRumEvents => "PutRumEvents",
        }
    }

    /// Call the operation on the app monitor `monitor`, whose minted id is `id`, signed
    /// with `key`.
    async fn call(self, emulator: &Emulator, key: &str, monitor: &str, id: &str) -> Reply {
        match self {
            Self::GetAppMonitor => {
                emulator
                    .rum(key, "GET", &format!("/appmonitor/{monitor}"), None)
                    .await
            }
            Self::PutRumEvents => {
                emulator
                    .rum(
                        key,
                        "POST",
                        &format!("/appmonitors/{id}/"),
                        Some(rum_events_body(id, None)),
                    )
                    .await
            }
        }
    }
}

/// One statement a case's document carries, written for the operation under test.
#[derive(Debug, Clone, Copy)]
enum Grant {
    /// Allow the operation's own action on every resource.
    AllowAnyResource,
    /// Allow the operation's own action on the ARN the call addresses.
    AllowThisMonitor,
    /// Allow the operation's own action on another monitor's ARN.
    AllowOtherMonitor,
    /// Allow another action of this service.
    AllowOtherAction,
    /// Explicitly deny the operation's own action.
    DenyAnyResource,
}

impl Grant {
    /// The identity-based document carrying this statement, for `operation` on the app
    /// monitor named `monitor`.
    fn document(self, operation: Operation, monitor: &str) -> String {
        let (effect, action, resource) = match self {
            Self::AllowAnyResource => ("Allow", operation.action(), "*".to_string()),
            Self::AllowThisMonitor => ("Allow", operation.action(), monitor_arn(monitor)),
            Self::AllowOtherMonitor => ("Allow", operation.action(), monitor_arn(OTHER_MONITOR)),
            Self::AllowOtherAction => ("Allow", operation.other_action(), "*".to_string()),
            Self::DenyAnyResource => ("Deny", operation.action(), "*".to_string()),
        };
        identity_policy(&statement(effect, action, &resource))
    }
}

/// What a case attaches to the principal, when it attaches anything.
#[derive(Debug, Clone, Copy)]
enum Attached {
    /// A customer-managed policy this account holds, carrying one statement.
    Held(Grant),
    /// An AWS-managed ARN. IAM accepts the attachment without the account holding the
    /// policy, so this emulator holds no document for it and cannot see what it grants.
    AwsManaged,
}

/// One row of the document-source table.
struct Case {
    case: &'static str,
    /// The statement the principal's inline policy carries, when it has one.
    inline: Option<Grant>,
    /// What is attached to the principal, when anything is.
    attached: Option<Attached>,
    /// Whether the call must be allowed.
    allowed: bool,
}

/// The rows, which hold for either kind of principal and either operation.
const CASES: &[Case] = &[
    Case {
        case: "an attached managed policy allowing the action on every resource",
        inline: None,
        attached: Some(Attached::Held(Grant::AllowAnyResource)),
        allowed: true,
    },
    Case {
        case: "an attached managed policy allowing the action on the addressed monitor",
        inline: None,
        attached: Some(Attached::Held(Grant::AllowThisMonitor)),
        allowed: true,
    },
    Case {
        case: "an attached managed policy allowing the action on another monitor",
        inline: None,
        attached: Some(Attached::Held(Grant::AllowOtherMonitor)),
        allowed: false,
    },
    Case {
        case: "an attached managed policy allowing another action",
        inline: None,
        attached: Some(Attached::Held(Grant::AllowOtherAction)),
        allowed: false,
    },
    Case {
        case: "an attached AWS-managed policy, whose document the account does not hold",
        inline: None,
        attached: Some(Attached::AwsManaged),
        allowed: true,
    },
    Case {
        case: "an inline policy allowing the action, with nothing attached",
        inline: Some(Grant::AllowAnyResource),
        attached: None,
        allowed: true,
    },
    Case {
        case: "an inline policy allowing another action and an attachment allowing this one",
        inline: Some(Grant::AllowOtherAction),
        attached: Some(Attached::Held(Grant::AllowAnyResource)),
        allowed: true,
    },
    Case {
        case: "an inline policy allowing the action and an attachment denying it",
        inline: Some(Grant::AllowAnyResource),
        attached: Some(Attached::Held(Grant::DenyAnyResource)),
        allowed: false,
    },
    Case {
        case: "an inline policy denying the action and an attachment allowing it",
        inline: Some(Grant::DenyAnyResource),
        attached: Some(Attached::Held(Grant::AllowAnyResource)),
        allowed: false,
    },
    Case {
        case: "no identity-based policy at all",
        inline: None,
        attached: None,
        allowed: false,
    },
];

/// RULE: the documents in force on the calling principal are its inline policies together
/// with the default versions of the managed policies attached to it, for EVERY kind of
/// principal the IAM store holds — so the same grant decides the same way whether it is
/// inline or attached, and whether the caller is an assumed-role session or an IAM user.
#[tokio::test]
async fn identity_documents_come_from_inline_and_attached_policies_for_every_principal_kind() {
    for operation in [Operation::GetAppMonitor, Operation::PutRumEvents] {
        for kind in [PrincipalKind::AssumedRole, PrincipalKind::IamUser] {
            for (index, row) in CASES.iter().enumerate() {
                let case = format!(
                    "{} signed by {} with {}",
                    operation.label(),
                    kind.label(),
                    row.case
                );
                let emulator = Emulator::new();
                let monitor = format!("sources-{index}");
                let id = emulator.create_monitor(&monitor).await;
                let name = format!("sources-caller-{index}");

                let inline = row
                    .inline
                    .map(|grant| grant.document(operation, &monitor));
                let signer = signer(&emulator, kind, &name, inline.as_deref()).await;
                if let Some(attached) = row.attached {
                    let policy_arn = match attached {
                        Attached::Held(grant) => {
                            emulator
                                .create_managed_policy(
                                    &format!("{name}-managed"),
                                    &grant.document(operation, &monitor),
                                )
                                .await
                        }
                        // The account segment `aws` is what makes a policy ARN
                        // AWS-managed, rather than account state
                        // (`iam_store::AWS_MANAGED_POLICY_ACCOUNT`).
                        Attached::AwsManaged => format!(
                            "arn:aws:iam::{}:policy/AmazonCloudWatchRUMReadOnlyAccess",
                            iam_store::AWS_MANAGED_POLICY_ACCOUNT
                        ),
                    };
                    attach(&emulator, &signer, &policy_arn).await;
                }

                let reply = operation.call(&emulator, &signer.key, &monitor, &id).await;
                let expected = if row.allowed {
                    ALLOWED_STATUS
                } else {
                    ACCESS_DENIED_STATUS
                };
                assert_eq!(
                    reply.status, expected,
                    "case {case:?}: expected HTTP {expected}, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                if !row.allowed {
                    let code = reply.error_code(&case);
                    assert_eq!(
                        code, ACCESS_DENIED_CODE,
                        "case {case:?}: expected the {ACCESS_DENIED_CODE} error code, got \
                         {code:?}"
                    );
                    let message = reply.message(&case);
                    assert!(
                        message.contains(operation.action()) && message.contains(&name),
                        "case {case:?}: the refusal must name the principal {name:?} and the \
                         action {:?}, got {message:?}",
                        operation.action()
                    );
                }
            }
        }
    }
}

/// RULE, along the version axis: the document an attachment contributes is its policy's
/// DEFAULT version — for every kind of principal, so a version added without being made
/// the default changes no decision.
#[tokio::test]
async fn the_default_version_of_an_attached_policy_is_the_one_in_force() {
    // (case, the statement version v1 carries, the statement the added version carries,
    //  whether the added version becomes the default, whether the call must be allowed)
    let rows: [(&str, Grant, Grant, bool, bool); 4] = [
        (
            "a later version allowing the action, made the default",
            Grant::AllowOtherAction,
            Grant::AllowAnyResource,
            true,
            true,
        ),
        (
            "a later version allowing the action, NOT made the default",
            Grant::AllowOtherAction,
            Grant::AllowAnyResource,
            false,
            false,
        ),
        (
            "a later version allowing another action, made the default",
            Grant::AllowAnyResource,
            Grant::AllowOtherAction,
            true,
            false,
        ),
        (
            "a later version allowing another action, NOT made the default",
            Grant::AllowAnyResource,
            Grant::AllowOtherAction,
            false,
            true,
        ),
    ];

    let operation = Operation::GetAppMonitor;
    for kind in [PrincipalKind::AssumedRole, PrincipalKind::IamUser] {
        for (index, (row, first, second, set_as_default, allowed)) in
            rows.into_iter().enumerate()
        {
            let case = format!("{} with {row}", kind.label());
            let emulator = Emulator::new();
            let monitor = format!("versions-{index}");
            let id = emulator.create_monitor(&monitor).await;
            let name = format!("versions-caller-{index}");
            let signer = signer(&emulator, kind, &name, None).await;
            let policy_arn = emulator
                .create_managed_policy(
                    &format!("{name}-managed"),
                    &first.document(operation, &monitor),
                )
                .await;
            emulator
                .create_policy_version(
                    &policy_arn,
                    &second.document(operation, &monitor),
                    set_as_default,
                )
                .await;
            attach(&emulator, &signer, &policy_arn).await;

            let reply = operation.call(&emulator, &signer.key, &monitor, &id).await;
            let expected = if allowed {
                ALLOWED_STATUS
            } else {
                ACCESS_DENIED_STATUS
            };
            assert_eq!(
                reply.status, expected,
                "case {case:?}: expected HTTP {expected}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        }
    }
}
