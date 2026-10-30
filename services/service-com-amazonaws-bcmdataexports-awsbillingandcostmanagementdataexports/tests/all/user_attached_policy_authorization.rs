// SPDX-License-Identifier: CC0-1.0

//! Where the identity-based documents a request is judged on LIVE, for every kind of
//! principal, over the awsJson1_1 wire.
//!
//! The rule under test: the identity-based decision an operation that models
//! `AccessDeniedException` (HTTP 403) makes is taken from EVERY identity-based document in
//! force on the calling principal at the instant of the call — the principal's INLINE
//! policies and the default version of every MANAGED policy ATTACHED to it — and those two
//! sources are the same for every kind of principal, so one document decides one request
//! the same way whether it sits inline on an IAM user, is attached to that user, or is
//! attached to a role the caller assumes. The one caller this decision cannot refuse is a
//! principal carrying an attachment whose document this account does not hold: an
//! AWS-managed policy belongs to AWS, so refusing on evidence that is not in the account's
//! store would deny access AWS permits — which again holds for every kind of principal.
//!
//! The decision is the real one: the principals, their policies and their attachments are
//! made with IAM's and STS's OWN operations over the same engine — `CreateUser`,
//! `PutUserPolicy`, `CreatePolicy`, `AttachUserPolicy`, `CreateAccessKey`, `CreateRole`,
//! `AttachRolePolicy`, `AssumeRole` — through the harness
//! [`crate::front_door_authorization::Emulator`], so the documents this service reads are
//! the ones those services wrote.
//!
//! It is asserted over tables that differ along what the rule generalises over: WHERE the
//! documents live (nowhere, inline, in one attachment, in two attachments, inline and
//! attached at once), WHAT they say (this action, a different action of the same service,
//! this action on a different resource, an explicit `Deny`, a document the account does not
//! hold), the KIND of principal carrying them (an IAM user signing a long-term key, an
//! assumed-role session), and WHICH operation is asked for (each of the four whose model
//! declares `AccessDeniedException`).

use serde_json::{json, Value};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

use crate::export_lifecycle::error_code;
use crate::front_door_authorization::{
    allow, any_export_arn_pattern, create_request, deny, export_arn_pattern, Emulator, ACCOUNT_KEY,
};

/// An action of this service that NONE of the four authorized operations is decided on, so
/// a document granting it grants none of them.
const UNRELATED_ACTION: &str = "bcm-data-exports:GetExport";

/// One row of the grant-PLACEMENT table: the user, the inline policy it carries (when
/// any), the managed policies attached to it, and whether the rule must serve it.
struct Placement {
    label: &'static str,
    user: &'static str,
    inline: Option<String>,
    attachments: Vec<String>,
    allowed: bool,
}

/// The message of a refusal, under either spelling of the member.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The request this suite drives `operation` with: a create of the export called `name`,
/// or a tagging call on `arn` (`name` is the one tag key a tagging write touches, so no
/// two rows contend for the same key).
fn request_for(operation: &str, arn: &str, name: &str) -> Value {
    match operation {
        "CreateExport" => create_request(name),
        "TagResource" => json!({
            "ResourceArn": arn,
            "ResourceTags": [{ "Key": name, "Value": "probe" }],
        }),
        "UntagResource" => json!({ "ResourceArn": arn, "ResourceTagKeys": [name] }),
        "ListTagsForResource" => json!({ "ResourceArn": arn }),
        other => panic!("this suite drives no operation called {other:?}"),
    }
}

/// How many exports this account holds, read with the account's own credential.
async fn export_count(emulator: &Emulator) -> usize {
    let (status, listed) = emulator.call(ACCOUNT_KEY, "ListExports", &json!({})).await;
    assert_eq!(status, 200, "ListExports must be served, got {listed}");
    listed["Exports"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| panic!("ListExports must answer an Exports list, got {listed}"))
}

/// The managed policies an IAM user's attachments can name, each written through IAM's own
/// `CreatePolicy` so the document read back is the one IAM stored.
struct Catalog {
    grant: String,
    wildcard: String,
    unrelated: String,
    other_resource: String,
    refusal: String,
    aws_managed: String,
}

impl Catalog {
    async fn new(emulator: &Emulator) -> Self {
        Self {
            grant: emulator
                .managed_policy(
                    "bcmde-user-attached-grant",
                    &allow("bcm-data-exports:CreateExport", &any_export_arn_pattern()),
                )
                .await,
            wildcard: emulator
                .managed_policy(
                    "bcmde-user-attached-wildcard",
                    &allow("bcm-data-exports:*", "*"),
                )
                .await,
            unrelated: emulator
                .managed_policy(
                    "bcmde-user-attached-unrelated",
                    &allow(UNRELATED_ACTION, "*"),
                )
                .await,
            other_resource: emulator
                .managed_policy(
                    "bcmde-user-attached-other-resource",
                    &allow(
                        "bcm-data-exports:CreateExport",
                        &export_arn_pattern("somebody_elses_export"),
                    ),
                )
                .await,
            refusal: emulator
                .managed_policy(
                    "bcmde-user-attached-deny",
                    &deny("bcm-data-exports:CreateExport", "*"),
                )
                .await,
            // An AWS-managed policy's account segment is IAM's own `aws`
            // (`iam_store::AWS_MANAGED_POLICY_ACCOUNT`), so this account's store holds no
            // document for it; `ReadOnlyAccess` is one such published policy.
            aws_managed: format!(
                "arn:aws:iam::{}:policy/ReadOnlyAccess",
                iam_store::AWS_MANAGED_POLICY_ACCOUNT
            ),
        }
    }
}

#[tokio::test]
async fn a_users_decision_is_made_from_its_attachments_as_well_as_its_inline_policies() {
    let emulator = Emulator::new();
    let catalog = Catalog::new(&emulator).await;

    // One row per placement of the documents in force on an IAM USER signing with a
    // long-term key: nowhere, inline, in one attachment, in two, or inline and attached at
    // once — the same table `tests/all/untagging_authorization.rs` drives over a role.
    let rows: Vec<Placement> = vec![
        Placement {
            label: "no policy at all",
            user: "bcmde-ua-bare",
            inline: None,
            attachments: vec![],
            allowed: false,
        },
        Placement {
            label: "the grant inline on the user",
            user: "bcmde-ua-inline",
            inline: Some(allow(
                "bcm-data-exports:CreateExport",
                &any_export_arn_pattern(),
            )),
            attachments: vec![],
            allowed: true,
        },
        Placement {
            label: "the grant in a managed policy attached to the user",
            user: "bcmde-ua-attached",
            inline: None,
            attachments: vec![catalog.grant.clone()],
            allowed: true,
        },
        Placement {
            label: "a wildcard grant in a managed policy attached to the user",
            user: "bcmde-ua-attached-wildcard",
            inline: None,
            attachments: vec![catalog.wildcard.clone()],
            allowed: true,
        },
        Placement {
            label: "an attached policy granting only another action",
            user: "bcmde-ua-attached-unrelated",
            inline: None,
            attachments: vec![catalog.unrelated.clone()],
            allowed: false,
        },
        Placement {
            label: "an attached policy granting this action on another resource",
            user: "bcmde-ua-attached-other-resource",
            inline: None,
            attachments: vec![catalog.other_resource.clone()],
            allowed: false,
        },
        Placement {
            label: "the grant one of two attached policies",
            user: "bcmde-ua-attached-two",
            inline: None,
            attachments: vec![catalog.unrelated.clone(), catalog.grant.clone()],
            allowed: true,
        },
        Placement {
            label: "an inline grant beside an attached deny",
            user: "bcmde-ua-inline-grant-attached-deny",
            inline: Some(allow("bcm-data-exports:*", "*")),
            attachments: vec![catalog.refusal.clone()],
            allowed: false,
        },
        Placement {
            label: "an attached grant beside an inline deny",
            user: "bcmde-ua-attached-grant-inline-deny",
            inline: Some(deny("bcm-data-exports:CreateExport", "*")),
            attachments: vec![catalog.grant.clone()],
            allowed: false,
        },
        Placement {
            label: "an attachment whose document this account does not hold",
            user: "bcmde-ua-attached-aws-managed",
            inline: None,
            attachments: vec![catalog.aws_managed.clone()],
            allowed: true,
        },
    ];

    let mut served = 0;
    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let access_key_id = emulator.user_key(row.user, row.inline.as_deref()).await;
        for attachment in &row.attachments {
            emulator.attach_user_policy(row.user, attachment).await;
        }

        let before = export_count(&emulator).await;
        let name = format!("user_attached_export_{index}");
        let (status, body) = emulator
            .call(&access_key_id, "CreateExport", &create_request(&name))
            .await;
        let after = export_count(&emulator).await;

        if row.allowed {
            assert_eq!(
                status, 200,
                "case {label:?}: a user whose documents allow the action must be served, \
                 got {status} with {body}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {label:?}: the served create must have stored its export, got {after} \
                 exports where {before} stood before"
            );
            served += 1;
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: a user whose documents do not allow the action must be \
                 refused, got {status} with {body}"
            );
            let refusal = message(&body);
            assert!(
                refusal.contains("bcm-data-exports:CreateExport")
                    && refusal.contains("not authorized to perform"),
                "case {label:?}: the refusal must name the action it was decided about, got \
                 {refusal:?}"
            );
            assert_eq!(
                after, before,
                "case {label:?}: a refused create must store nothing, got {after} exports \
                 where {before} stood before"
            );
        }
    }

    assert_eq!(
        export_count(&emulator).await,
        served,
        "exactly the served rows' exports must exist, with {served} rows served"
    );
}

/// The kind of principal a document is carried by is no part of the rule: the SAME managed
/// policy, attached to an IAM user and attached to a role, decides the same request the
/// same way — for each of the four operations whose model declares
/// `AccessDeniedException`, each on its OWN action.
#[tokio::test]
async fn the_same_attached_managed_policy_decides_a_user_and_a_role_alike() {
    let emulator = Emulator::new();

    // The export the tagging operations name, created by the account's own credential.
    let (status, created) = emulator
        .call(
            ACCOUNT_KEY,
            "CreateExport",
            &create_request("user_attached_tagged_export"),
        )
        .await;
    assert_eq!(status, 200, "the setup create must succeed, got {created}");
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // smithy_query: these are the operations whose error list carries
    // `AccessDeniedException`, each decided on the action of its own name.
    let operations = [
        ("CreateExport", "bcm-data-exports:CreateExport"),
        ("TagResource", "bcm-data-exports:TagResource"),
        ("UntagResource", "bcm-data-exports:UntagResource"),
        ("ListTagsForResource", "bcm-data-exports:ListTagsForResource"),
    ];

    for (index, (operation, action)) in operations.iter().enumerate() {
        // Two documents per operation: one granting exactly its action, one granting an
        // action it is not decided on.
        let documents = [
            (
                "a grant for this action",
                emulator
                    .managed_policy(&format!("bcmde-ua-pair-grant-{index}"), &allow(action, "*"))
                    .await,
                true,
            ),
            (
                "a grant for another action",
                emulator
                    .managed_policy(
                        &format!("bcmde-ua-pair-unrelated-{index}"),
                        &allow(UNRELATED_ACTION, "*"),
                    )
                    .await,
                false,
            ),
        ];

        for (slot, (document, policy_arn, allowed)) in documents.iter().enumerate() {
            let user = format!("bcmde-ua-pair-user-{index}-{slot}");
            let user_key_id = emulator.user_key(&user, None).await;
            emulator.attach_user_policy(&user, policy_arn).await;

            let role = format!("bcmde-ua-pair-role-{index}-{slot}");
            let role_key_id = emulator.assumed_role_key(&role, None).await;
            emulator.attach_role_policy(&role, policy_arn).await;

            for (kind_slot, (kind, access_key_id)) in [
                ("an IAM user", &user_key_id),
                ("an assumed-role session", &role_key_id),
            ]
            .iter()
            .enumerate()
            {
                let label = format!("{operation} with {document} attached to {kind}");
                let name = format!("user_attached_pair_{index}_{slot}_{kind_slot}");
                let (status, body) = emulator
                    .call(access_key_id, operation, &request_for(operation, &arn, &name))
                    .await;
                if *allowed {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: an attached grant must serve every kind of \
                         principal alike, got {status} with {body}"
                    );
                } else {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (403, "AccessDeniedException"),
                        "case {label:?}: a document that grants another action must refuse \
                         every kind of principal alike, got {status} with {body}"
                    );
                    let refusal = message(&body);
                    assert!(
                        refusal.contains(*action) && refusal.contains("not authorized to perform"),
                        "case {label:?}: the refusal must name the action it was decided \
                         about, got {refusal:?}"
                    );
                }
            }
        }
    }
}
