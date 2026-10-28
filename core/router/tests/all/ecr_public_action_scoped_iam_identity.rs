// SPDX-License-Identifier: CC0-1.0

//! One credential, three services: the IAM user an `iam:CreateAccessKey` key belongs to is
//! the principal `sts:GetCallerIdentity` reports for it AND the principal whose
//! identity-based policy decides every `ecr-public` call signed with it.
//!
//! The rule under test: a request speaks for the IAM user that holds the access key id it
//! presents — one principal for one credential, whichever service answers — and that
//! principal may perform only an action an identity-based policy in force on it ALLOWS.
//! An action no statement allows is refused with the AWS-common `AccessDeniedException`
//! BEFORE the operation runs, so the registry is left exactly as it was, the refusal names
//! that same principal, the action and the resource the action takes, and adding the
//! missing `Allow` to the very same policy makes the IDENTICAL call succeed.
//!
//! Only a cross-service suite can pin this: IAM holds the access-key registry and the
//! policy documents, STS answers which principal a key speaks for, and `ecr-public` takes
//! the decision — so each crate's own tests can assert only its own half, and an emulator
//! that resolved the key to one principal in STS and judged another in `ecr-public` would
//! pass all three crate suites. A client that narrows a policy to block exactly one
//! `ecr-public` action, checks the block, then widens it again, is reading all three at
//! once.
//!
//! The rows vary what the rule generalises over: the OPERATION withheld (a create, an
//! update, a delete, and a registry-wide read), hence the ACTION the refusal names; the
//! RESOURCE the decision is made against (one repository's ARN for an operation that
//! addresses a repository, every resource for a registry-wide action that takes no resource
//! type); the FORM of the repository name (flat, namespaced, dot-separated); and the EFFECT
//! the grant unlocks, which must be absent while the action is withheld and present once it
//! is granted.
//!
//! The access-key-to-user binding on its own — which user a presented key speaks for across
//! IAM and STS, including the keys this suite does not mint — has its primary home in
//! `iam_access_key_signs_as_its_user.rs`. It is read here only as the precondition the
//! ecr-public decision rests on: the new ground below is that the principal ecr-public
//! judges, and blames, is that same one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
    repository_arn, API_REGION, ARN_NAMESPACE,
};

// The awsQuery form encoder and the element readers this test crate already owns.
use crate::query_wire::{element, form_encode, opt_element};

/// The `X-Amz-Target` prefix of this service's awsJson1_1 wire, from the ecr-public crate's
/// `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// The access key id the administrative calls present. Neither STS nor IAM holds a record
/// of it, so it resolves to the account's own ambient identity — the position AWS account
/// root credentials are in, which carries no IAM policy and is not refused.
const ADMIN_KEY: &str = "AKIDECRPUBLICADMIN";

/// The status and wire error code an authorization refusal carries: the AWS "Common Errors"
/// table's `AccessDeniedException`, which the ecr-public model does not declare because the
/// decision is taken before the operation runs.
const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// The resource a decision about an action that takes NO resource type is made against,
/// spelled with IAM's own resource wildcard.
const NO_RESOURCE: &str = "*";

/// The two grants every scoped principal below carries beside its row's own: the registry
/// read used as the positive control, and the `sts` action an ecr-public client needs to
/// exchange a token. Neither is an `ecr-public` write, so neither can stand in for one.
const BASE_GRANTS: &[&str] = &["ecr-public:DescribeRepositories", "sts:GetServiceBearerToken"];

/// The catalog description the withheld `PutRepositoryCatalogData` row writes, so "the
/// effect happened" is a value the call carried rather than the mere presence of a member.
const CATALOG_DESCRIPTION: &str = "the description the granted call wrote";

/// What the granted call changes, so the suite can see that the refusal left the registry
/// alone and that the grant really unlocked the operation rather than the operation having
/// been a no-op all along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// The repository comes into existence.
    RepositoryCreated,
    /// The repository's catalog description becomes the one the call carried.
    CatalogDescribed,
    /// The repository stops existing.
    RepositoryDeleted,
    /// Nothing in the registry changes: the grant is observable only in the answer, which
    /// must carry a usable authorization token.
    TokenIssued,
}

/// One row of the table: which action is withheld from an otherwise-granted principal.
struct Row {
    case: &'static str,
    /// The IAM user name the row's principal is created under.
    user_name: &'static str,
    /// The repository the row addresses; unused by a registry-wide row.
    repository_name: &'static str,
    /// The operation the scoped principal calls, and the `ecr-public` action it is
    /// authorized under.
    operation: &'static str,
    /// Whether the repository must already exist before the withheld call is made.
    precreated: bool,
    /// What the granted call changes.
    effect: Effect,
}

/// The table. Each row's principal is granted [`BASE_GRANTS`] and nothing else, so its
/// row's own action is the one thing it may not do.
const ROWS: &[Row] = &[
    Row {
        case: "a create, on a flat repository name",
        user_name: "scoped-create",
        repository_name: "create-probe",
        operation: "CreateRepository",
        precreated: false,
        effect: Effect::RepositoryCreated,
    },
    Row {
        case: "an update, on a namespaced repository name",
        user_name: "scoped-catalog",
        repository_name: "team/catalog-probe",
        operation: "PutRepositoryCatalogData",
        precreated: true,
        effect: Effect::CatalogDescribed,
    },
    Row {
        case: "a delete, on a dot-separated repository name",
        user_name: "scoped-delete",
        repository_name: "delete.probe",
        operation: "DeleteRepository",
        precreated: true,
        effect: Effect::RepositoryDeleted,
    },
    Row {
        case: "a registry-wide read, which takes no resource type",
        user_name: "scoped-token",
        repository_name: "",
        operation: "GetAuthorizationToken",
        precreated: false,
        effect: Effect::TokenIssued,
    },
];

/// The emulator these rows drive: ONE engine carrying ecr-public beside the IAM and STS
/// services, exactly as `core/router/src/lib.rs` mounts them, so the registry
/// `iam:CreateAccessKey` writes is the one STS resolves against and the documents
/// `iam:PutUserPolicy` wrote are the ones ecr-public reads.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One ecr-public call over awsJson1_1, with `access_key_id` in the credential scope.
    async fn ecr_public(
        &self,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("api.ecr-public.{API_REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{API_REGION}\
                     /{ARN_NAMESPACE}/aws4_request, SignedHeaders=host;x-amz-target, \
                     Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed ecr-public request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// One awsQuery call against `signing_name`, with `access_key_id` in the credential
    /// scope.
    async fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        parameters: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in parameters {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{API_REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }

    /// One administrative IAM action, which must succeed.
    async fn iam(&self, case: &str, action: &str, parameters: &[(&str, String)]) -> String {
        let (status, body) = self
            .query(ADMIN_KEY, "iam", "2010-05-08", action, parameters)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture {action} must succeed, got {status} {body}"
        );
        body
    }

    /// The `Arn` `sts:GetCallerIdentity` reports for `access_key_id` — the one place the
    /// principal of a credential is published, and so the ARN every refusal of that
    /// credential must blame.
    async fn caller_arn(&self, case: &str, access_key_id: &str) -> String {
        let (status, body) = self
            .query(
                access_key_id,
                "sts",
                "2011-06-15",
                "GetCallerIdentity",
                &[],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} {body}"
        );
        element(&body, "Arn")
    }

    /// Create the user `user_name`, give it exactly `actions` as its one inline policy, and
    /// mint a long-term access key for it. Answers that key id beside the user's own ARN as
    /// `iam:CreateUser` reported it.
    async fn scoped_principal(
        &self,
        case: &str,
        user_name: &str,
        actions: &[&str],
    ) -> (String, String) {
        let created = self
            .iam(case, "CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let user_arn = element(&created, "Arn");
        self.put_inline_policy(case, user_name, actions).await;
        let minted = self
            .iam(
                case,
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        // The answer carries a minted secret, so a failure names the member rather than
        // quoting the body.
        let access_key_id = opt_element(&minted, "AccessKeyId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateAccessKey must answer an AccessKeyId")
        });
        (access_key_id, user_arn)
    }

    /// Write (or rewrite) the one inline policy of `user_name` under the same policy name,
    /// so a later put REPLACES what the principal carried rather than adding a second
    /// document.
    async fn put_inline_policy(&self, case: &str, user_name: &str, actions: &[&str]) {
        self.iam(
            case,
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", "scoped".to_string()),
                ("PolicyDocument", inline_policy(actions)),
            ],
        )
        .await;
    }

    /// Whether the registry holds `repository_name`, read as the administrative principal:
    /// `DescribeRepositories` naming it answers it, or `RepositoryNotFoundException` when
    /// nothing answers to the name.
    async fn repository_exists(&self, case: &str, repository_name: &str) -> bool {
        let (status, body) = self
            .ecr_public(
                ADMIN_KEY,
                "DescribeRepositories",
                &json!({ "repositoryNames": [repository_name] }),
            )
            .await;
        if status == 200 {
            let held = body["repositories"][0]["repositoryName"].as_str();
            assert_eq!(
                held,
                Some(repository_name),
                "case {case:?}: the administrative read must answer the repository it named, \
                 got {body:?}"
            );
            return true;
        }
        assert_eq!(
            (status, code_of(&body).as_str()),
            (400, "RepositoryNotFoundException"),
            "case {case:?}: a name the registry does not hold must answer \
             RepositoryNotFoundException, got {status} {body:?}"
        );
        false
    }

    /// The catalog description the registry holds for `repository_name`, read as the
    /// administrative principal; `None` when it holds none.
    async fn catalog_description(&self, case: &str, repository_name: &str) -> Option<String> {
        let (status, body) = self
            .ecr_public(
                ADMIN_KEY,
                "GetRepositoryCatalogData",
                &json!({ "repositoryName": repository_name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the administrative catalog read must succeed, got {status} {body:?}"
        );
        body["catalogData"]["description"]
            .as_str()
            .map(str::to_string)
    }

    /// Create `repository_name` as the administrative principal, for a row whose withheld
    /// operation addresses a repository that must already exist.
    async fn precreate(&self, case: &str, repository_name: &str) {
        let (status, body) = self
            .ecr_public(
                ADMIN_KEY,
                "CreateRepository",
                &json!({ "repositoryName": repository_name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateRepository must succeed, got {status} {body:?}"
        );
    }
}

/// An identity-based policy whose one statement Allows exactly `actions` on every resource.
fn inline_policy(actions: &[&str]) -> String {
    let listed = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[{listed}],\"Resource\":\"*\"}}]}}"
    )
}

/// The error code an awsJson response carries, and the empty string for an answer that is
/// no error at all.
fn code_of(body: &Value) -> String {
    // `__type` is often qualified (`com.amazonaws…#Code`); the code is its last segment.
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', '.'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `ecr-public` action an operation is authorized under, and the resource a row's
/// decision is made against.
fn action_of(operation: &str) -> String {
    format!("{ARN_NAMESPACE}:{operation}")
}

/// The request body the row's withheld operation is called with.
fn body_of(row: &Row) -> Value {
    match row.operation {
        "CreateRepository" | "DeleteRepository" => {
            json!({ "repositoryName": row.repository_name })
        }
        "PutRepositoryCatalogData" => json!({
            "repositoryName": row.repository_name,
            "catalogData": { "description": CATALOG_DESCRIPTION },
        }),
        "GetAuthorizationToken" => json!({}),
        other => panic!("the table has no body for {other}"),
    }
}

/// The IAM refusal template, built from the principal STS reported, the action the
/// operation is authorized under and the resource that action takes.
fn expected_refusal(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// A narrowed identity policy blocks exactly the one `ecr-public` action it omits, in the
/// name of the principal STS reports for the signing key, and widening the same policy
/// unblocks that action and nothing else.
#[tokio::test]
async fn an_action_scoped_identity_policy_decides_ecr_public_calls() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let (access_key_id, user_arn) = emulator
            .scoped_principal(case, row.user_name, BASE_GRANTS)
            .await;

        // 1. ONE principal for one credential: the key IAM minted speaks for the user it
        //    was minted for, and that is the ARN a refusal must blame.
        let principal = emulator.caller_arn(case, &access_key_id).await;
        assert_eq!(
            principal, user_arn,
            "case {case:?}: GetCallerIdentity must report the user the key was minted for, \
             got {principal:?}"
        );

        // 2. The positive control: the action the policy DOES allow is permitted, so the
        //    refusal below is about this action rather than about the credential.
        let (status, body) = emulator
            .ecr_public(&access_key_id, "DescribeRepositories", &json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the granted DescribeRepositories must succeed for the scoped \
             principal, got {status} {body:?}"
        );

        if row.precreated {
            emulator.precreate(case, row.repository_name).await;
        }

        // 3. The withheld action is refused, naming the principal, the action and the
        //    resource the action takes.
        let resource = if row.repository_name.is_empty() {
            NO_RESOURCE.to_string()
        } else {
            repository_arn(row.repository_name)
        };
        let action = action_of(row.operation);
        let request = body_of(row);
        let (status, refused) = emulator
            .ecr_public(&access_key_id, row.operation, &request)
            .await;
        assert_eq!(
            (status, code_of(&refused).as_str()),
            ACCESS_DENIED,
            "case {case:?}: {action} must be refused for a principal no policy allows it to, \
             got {status} {refused:?}"
        );
        let stated = refused["message"].as_str().unwrap_or_default();
        let expected = expected_refusal(&principal, &action, &resource);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the refusal {expected:?}, got {stated:?}"
        );

        // 4. The refusal was taken BEFORE the operation ran, so the registry is untouched.
        assert_effect(&emulator, case, row, false).await;

        // 5. Widening the SAME inline policy with the missing Allow makes the IDENTICAL
        //    call succeed — nothing else about the request changes.
        let mut widened = BASE_GRANTS.to_vec();
        widened.push(action.as_str());
        emulator
            .put_inline_policy(case, row.user_name, &widened)
            .await;
        let (status, allowed) = emulator
            .ecr_public(&access_key_id, row.operation, &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the identical call must succeed once the policy allows \
             {action}, got {status} {allowed:?}"
        );
        if row.effect == Effect::TokenIssued {
            let token = allowed["authorizationData"]["authorizationToken"]
                .as_str()
                .unwrap_or_default();
            assert!(
                !token.is_empty(),
                "case {case:?}: the granted {action} must answer a usable authorization token"
            );
        }

        // 6. And the effect the grant unlocked is the one an administrative principal
        //    reads back.
        assert_effect(&emulator, case, row, true).await;
    }
}

/// Assert the row's effect is absent (`applied` false, the refusal left nothing behind) or
/// present (`applied` true, the granted call did what it says), read as the administrative
/// principal so the observation does not depend on the scoped principal's own grants.
async fn assert_effect(emulator: &Emulator, case: &str, row: &Row, applied: bool) {
    match row.effect {
        Effect::RepositoryCreated => {
            let held = emulator.repository_exists(case, row.repository_name).await;
            assert_eq!(
                held, applied,
                "case {case:?}: expected the repository to be held == {applied}, got {held}"
            );
        }
        Effect::RepositoryDeleted => {
            let held = emulator.repository_exists(case, row.repository_name).await;
            assert_eq!(
                held, !applied,
                "case {case:?}: expected the repository to be held == {}, got {held}",
                !applied
            );
        }
        Effect::CatalogDescribed => {
            let described = emulator
                .catalog_description(case, row.repository_name)
                .await;
            let wrote = described.as_deref() == Some(CATALOG_DESCRIPTION);
            assert_eq!(
                wrote, applied,
                "case {case:?}: expected the catalog description to be the written one == \
                 {applied}, got {described:?}"
            );
        }
        // Nothing in the registry changes; the answer itself is asserted at the call site,
        // and before the grant there was no answer at all.
        Effect::TokenIssued => {}
    }
}
