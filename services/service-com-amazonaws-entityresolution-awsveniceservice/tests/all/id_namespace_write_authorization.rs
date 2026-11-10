// SPDX-License-Identifier: CC0-1.0

//! The ID-namespace write path, driven by callers IAM permits and callers it does not.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request whose SigV4 credential scope carries the caller's access key id, through the
//! IAM/STS harness the sibling `identity_authorization` suite owns — so what is asserted is
//! what an AWS SDK client would see, and the principals and their policy documents are made
//! with IAM's and STS's OWN operations over the same engine.
//!
//! The rule under test: `AccessDeniedException` (HTTP 403) is reserved for a caller whose
//! identity-based policies do not allow that operation's own action
//! (`entityresolution:<OperationName>`) on the resource the request addresses, so a caller
//! those policies DO permit is served the operation's modelled success — the whole ID
//! namespace lifecycle (`CreateIdNamespace` 200 with the minted `idNamespaceArn`,
//! `GetIdNamespace` 200, `UpdateIdNamespace` 200, `DeleteIdNamespace` 200 with its
//! @required `message`) closes for such a caller, and `ResourceNotFoundException` (HTTP 404)
//! is reserved for a name the store does not hold rather than answered for one the same
//! caller just created.
//!
//! The rows differ along what the rule generalises over: WHICH caller is permitted and where
//! its grant comes from (the account's own published credential, which holds no IAM policies;
//! an IAM user with an inline wildcard `Allow`; an IAM user with an `Allow` scoped to the one
//! namespace's ARN; an assumed-role session with a managed policy covering every ID namespace
//! of the account in the request's region), WHICH lifecycle operation is asked for, and the
//! `idNamespaceName` over the character classes the modelled `EntityName` pattern
//! `^[a-zA-Z_0-9-]*$` admits at its `length` bounds of 1 and 255, paired with the modelled
//! `IdNamespaceType` catalog (`SOURCE | TARGET`). The recorded create/delete pair is ONE row.

use serde_json::{json, Value};

use crate::identity_authorization::{
    action_of, allow, allow_all_but, assert_denied, explicit_deny, no_policy_allows, resource_arn,
    Emulator, Principal, Reply, ACCOUNT_KEY,
};
// The member readers every suite of this crate shares, so this one reads a response body
// exactly as the lifecycle suites do.
use crate::schema_mapping_workflow_lifecycle::{string_member, timestamp_member};

/// One ID namespace a row drives the lifecycle of.
struct NamespaceRow {
    case: &'static str,
    name: String,
    /// `type` — @required on `CreateIdNamespace`, one of the modelled `IdNamespaceType`
    /// catalog (`smithy_query(query="shape",shape="IdNamespaceType")`: `SOURCE | TARGET`).
    namespace_type: &'static str,
}

/// The table of namespaces: the modelled `EntityName` character classes at both `length`
/// bounds, across the whole modelled `IdNamespaceType` catalog.
fn namespace_rows() -> Vec<NamespaceRow> {
    vec![
        NamespaceRow {
            case: "a single letter at the modelled EntityName length minimum of 1, SOURCE",
            name: "a".to_string(),
            namespace_type: "SOURCE",
        },
        NamespaceRow {
            case: "a digits-only name, TARGET",
            name: "0".to_string(),
            namespace_type: "TARGET",
        },
        NamespaceRow {
            case: "mixed case with the underscore and hyphen the pattern admits, SOURCE",
            name: "UPPER_lower-123".to_string(),
            namespace_type: "SOURCE",
        },
        NamespaceRow {
            case: "a name at the modelled EntityName length maximum of 255, TARGET",
            name: "n".repeat(255),
            namespace_type: "TARGET",
        },
    ]
}

/// Where the caller's permission for this namespace comes from.
enum Permitted {
    /// The account's own published credential: neither STS nor IAM holds a record for it, so
    /// it holds no IAM policies at all — the position AWS account root credentials are in.
    AccountCredential,
    /// An IAM user carrying an inline `Allow` of every action of this service on everything.
    UserWithAWildcardGrant,
    /// An IAM user carrying an inline `Allow` scoped to this one namespace's own ARN.
    UserGrantedThisNamespace,
    /// An assumed-role session whose attached managed policy covers every ID namespace of
    /// the account in the request's region.
    SessionGrantedEveryNamespace,
}

impl Permitted {
    fn case(&self) -> &'static str {
        match self {
            Self::AccountCredential => "the account's own published credential",
            Self::UserWithAWildcardGrant => "an IAM user with an inline wildcard Allow",
            Self::UserGrantedThisNamespace => {
                "an IAM user granted this service's actions on this namespace's ARN"
            }
            Self::SessionGrantedEveryNamespace => {
                "an assumed-role session granted this service's actions on every ID namespace"
            }
        }
    }
}

/// The `/idnamespaces/{idNamespaceName}` path of one namespace. The label is a single path
/// segment of an `EntityName`, whose pattern admits no `/`, so it needs no encoding.
fn namespace_path(name: &str) -> String {
    format!("/idnamespaces/{name}")
}

/// A `CreateIdNamespace` body carrying only the two @required members.
fn create_request(row: &NamespaceRow) -> Value {
    json!({ "idNamespaceName": row.name, "type": row.namespace_type })
}

/// Assert `reply` is the modelled resource miss: HTTP 404 `ResourceNotFoundException`.
fn assert_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_code
    );
}

/// Build the caller `kind` describes for the namespace `arn`, answering `None` for the
/// account's own credential (which is no IAM principal at all).
async fn permitted_caller(
    emulator: &Emulator,
    kind: &Permitted,
    index: usize,
    arn: &str,
) -> Option<Principal> {
    match kind {
        Permitted::AccountCredential => None,
        Permitted::UserWithAWildcardGrant => Some(
            emulator
                .user(
                    &format!("ns-authz-wild-{index}"),
                    Some(&allow("entityresolution:*", "*")),
                )
                .await,
        ),
        Permitted::UserGrantedThisNamespace => Some(
            emulator
                .user(
                    &format!("ns-authz-scoped-{index}"),
                    Some(&allow("entityresolution:*", arn)),
                )
                .await,
        ),
        Permitted::SessionGrantedEveryNamespace => {
            let role_name = format!("ns-authz-role-{index}");
            let session = emulator.assumed_role(&role_name, None).await;
            emulator
                .attach_role_policy(
                    &role_name,
                    &format!("ns-authz-managed-{index}"),
                    &allow("entityresolution:*", &resource_arn("idnamespace", "*")),
                )
                .await;
            Some(session)
        }
    }
}

/// RULE: a caller whose identity-based policies permit this service's ID-namespace actions on
/// the namespace the request addresses is served the modelled success of every step of the
/// lifecycle — never a 403, and never a 404 for the namespace it just created. The 404 is
/// reserved for the name once it is gone.
#[tokio::test]
async fn a_permitted_caller_runs_the_whole_id_namespace_lifecycle() {
    let emulator = Emulator::new();
    let kinds = [
        Permitted::AccountCredential,
        Permitted::UserWithAWildcardGrant,
        Permitted::UserGrantedThisNamespace,
        Permitted::SessionGrantedEveryNamespace,
    ];

    for (index, row) in namespace_rows().iter().enumerate() {
        for (kind_index, kind) in kinds.iter().enumerate() {
            let case = format!("{}, {}", row.case, kind.case());
            let arn = resource_arn("idnamespace", &row.name);
            let principal =
                permitted_caller(&emulator, kind, index * kinds.len() + kind_index, &arn).await;
            let akid = principal
                .as_ref()
                .map_or(ACCOUNT_KEY, |held| held.access_key_id.as_str());
            let path = namespace_path(&row.name);

            // The create: HTTP 200 with the modelled body, from a caller AWS permits.
            let created = emulator
                .call(akid, "POST", "/idnamespaces", Some(&create_request(row)))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
            );
            assert_eq!(
                string_member(&created.body, "idNamespaceName"),
                row.name,
                "case {case:?}: expected the requested name back, got {:?}",
                created.body
            );
            assert_eq!(
                string_member(&created.body, "idNamespaceArn"),
                arn,
                "case {case:?}: expected the minted ARN {arn:?}, got {:?}",
                created.body
            );
            assert_eq!(
                string_member(&created.body, "type"),
                row.namespace_type,
                "case {case:?}: expected the requested type back, got {:?}",
                created.body
            );
            for stamp in ["createdAt", "updatedAt"] {
                let value = timestamp_member(&created.body, stamp);
                assert!(
                    value > 0.0,
                    "case {case:?}: {stamp} must be a real instant, got {value}"
                );
            }

            // The read: the same caller reads back what the create persisted.
            let read = emulator.call(akid, "GET", &path, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetIdNamespace, got {read:?}"
            );
            assert_eq!(
                read.body, created.body,
                "case {case:?}: GetIdNamespace must answer what CreateIdNamespace persisted, \
                 got {:?} after {:?}",
                read.body, created.body
            );

            // The update: served too, and it answers the stored record.
            let updated = emulator
                .call(
                    akid,
                    "PUT",
                    &path,
                    Some(&json!({ "description": "permitted-caller update" })),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateIdNamespace, got {updated:?}"
            );
            assert_eq!(
                string_member(&updated.body, "description"),
                "permitted-caller update",
                "case {case:?}: expected the new description back, got {:?}",
                updated.body
            );

            // The delete: HTTP 200 with the @required `message`, NOT a 404 for a namespace
            // this very caller created.
            let deleted = emulator.call(akid, "DELETE", &path, None).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected HTTP 200 from DeleteIdNamespace, got {deleted:?}"
            );
            assert!(
                !string_member(&deleted.body, "message").is_empty(),
                "case {case:?}: `message` is modelled @required, got {:?}",
                deleted.body
            );

            // Only now is the name absent, so the modelled miss is what the repeat and the
            // read answer.
            assert_not_found(&case, &emulator.call(akid, "DELETE", &path, None).await);
            assert_not_found(&case, &emulator.call(akid, "GET", &path, None).await);
        }
    }
}

/// RULE: the 403 belongs to the caller IAM does not permit. A principal whose policies say
/// nothing about this service is refused every ID-namespace write with the modelled
/// `AccessDeniedException` naming it, the action and the namespace's ARN, and the refused
/// write changes nothing — while the permitted caller's identical calls are served.
#[tokio::test]
async fn a_caller_with_no_grant_is_refused_every_id_namespace_write_and_changes_nothing() {
    let emulator = Emulator::new();

    for (index, row) in namespace_rows().iter().enumerate() {
        let arn = resource_arn("idnamespace", &row.name);
        let path = namespace_path(&row.name);
        let bare_user = emulator.user(&format!("ns-noperm-user-{index}"), None).await;
        let bare_session = emulator
            .assumed_role(&format!("ns-noperm-role-{index}"), None)
            .await;

        for (principal_case, principal) in [
            ("an IAM user with no policy at all", &bare_user),
            ("an assumed-role session with no policy at all", &bare_session),
        ] {
            let case = format!("{}, {principal_case}", row.case);
            let akid = principal.access_key_id.as_str();

            let refused = emulator
                .call(akid, "POST", "/idnamespaces", Some(&create_request(row)))
                .await;
            assert_denied(
                &case,
                &refused,
                &no_policy_allows(&principal.arn, &action_of("CreateIdNamespace"), &arn),
            );
            // The refused create stored nothing: the permitted caller finds no such name.
            assert_not_found(
                &case,
                &emulator.call(ACCOUNT_KEY, "GET", &path, None).await,
            );

            // With the namespace in place — created by a caller AWS permits — the refusals
            // are still refusals, and still change nothing.
            let created = emulator
                .call(
                    ACCOUNT_KEY,
                    "POST",
                    "/idnamespaces",
                    Some(&create_request(row)),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the permitted caller must be served, got {created:?}"
            );

            let refused_update = emulator
                .call(akid, "PUT", &path, Some(&json!({ "description": "nope" })))
                .await;
            assert_denied(
                &case,
                &refused_update,
                &no_policy_allows(&principal.arn, &action_of("UpdateIdNamespace"), &arn),
            );
            let refused_delete = emulator.call(akid, "DELETE", &path, None).await;
            assert_denied(
                &case,
                &refused_delete,
                &no_policy_allows(&principal.arn, &action_of("DeleteIdNamespace"), &arn),
            );

            let still_there = emulator.call(ACCOUNT_KEY, "GET", &path, None).await;
            assert_eq!(
                still_there.body, created.body,
                "case {case:?}: the refused writes must leave the stored record exactly as it \
                 was, got {:?} after {:?}",
                still_there.body, created.body
            );

            // Hand the name back for the next principal of this row.
            let removed = emulator.call(ACCOUNT_KEY, "DELETE", &path, None).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: expected HTTP 200 from DeleteIdNamespace, got {removed:?}"
            );
        }
    }
}

/// RULE: the decision is per ACTION, so a policy that withholds exactly one of this service's
/// ID-namespace actions refuses exactly that call — with the explicit deny's own wording — and
/// serves every other step of the lifecycle. A caller permitted the action is never refused.
#[tokio::test]
async fn an_explicit_deny_refuses_only_the_one_action_it_names() {
    let emulator = Emulator::new();

    for (index, row) in namespace_rows().iter().enumerate() {
        let arn = resource_arn("idnamespace", &row.name);
        let path = namespace_path(&row.name);

        // A caller allowed everything of this service EXCEPT the create.
        let create_action = action_of("CreateIdNamespace");
        let no_create = emulator
            .user(
                &format!("ns-nocreate-{index}"),
                Some(&allow_all_but(&create_action)),
            )
            .await;
        let case = format!("{}, a wildcard Allow denying the create", row.case);
        let refused = emulator
            .call(
                &no_create.access_key_id,
                "POST",
                "/idnamespaces",
                Some(&create_request(row)),
            )
            .await;
        assert_denied(
            &case,
            &refused,
            &explicit_deny(&no_create.arn, &create_action, &arn),
        );

        // The rest of the lifecycle is this caller's to run: with the namespace created by a
        // caller AWS permits, its read, update and delete are served.
        let created = emulator
            .call(
                ACCOUNT_KEY,
                "POST",
                "/idnamespaces",
                Some(&create_request(row)),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the permitted caller must be served, got {created:?}"
        );
        for (step, method, body) in [
            ("GetIdNamespace", "GET", None),
            (
                "UpdateIdNamespace",
                "PUT",
                Some(json!({ "description": "allowed by the wildcard grant" })),
            ),
            ("DeleteIdNamespace", "DELETE", None),
        ] {
            let served = emulator
                .call(&no_create.access_key_id, method, &path, body.as_ref())
                .await;
            assert_eq!(
                served.status, 200,
                "case {case:?}: {step} is not the denied action, so it must be served, got \
                 {served:?}"
            );
        }

        // ...and the mirror image: a caller allowed everything EXCEPT the delete creates the
        // namespace and is refused only the delete, which removes nothing.
        let delete_action = action_of("DeleteIdNamespace");
        let no_delete = emulator
            .user(
                &format!("ns-nodelete-{index}"),
                Some(&allow_all_but(&delete_action)),
            )
            .await;
        let case = format!("{}, a wildcard Allow denying the delete", row.case);
        let recreated = emulator
            .call(
                &no_delete.access_key_id,
                "POST",
                "/idnamespaces",
                Some(&create_request(row)),
            )
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the create is not the denied action, so it must be served, got \
             {recreated:?}"
        );
        let refused_delete = emulator
            .call(&no_delete.access_key_id, "DELETE", &path, None)
            .await;
        assert_denied(
            &case,
            &refused_delete,
            &explicit_deny(&no_delete.arn, &delete_action, &arn),
        );
        let still_there = emulator.call(ACCOUNT_KEY, "GET", &path, None).await;
        assert_eq!(
            still_there.body, recreated.body,
            "case {case:?}: the refused delete must remove nothing, got {:?} after {:?}",
            still_there.body, recreated.body
        );

        // Hand the name back for the next row.
        let removed = emulator.call(ACCOUNT_KEY, "DELETE", &path, None).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteIdNamespace, got {removed:?}"
        );
    }
}
