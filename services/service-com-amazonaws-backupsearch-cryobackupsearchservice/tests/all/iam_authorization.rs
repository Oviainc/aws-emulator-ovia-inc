// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this service,
//! so EVERY operation evaluates the identity-based IAM policies in force on the principal
//! its credentials name — against `backup-search:<OperationName>` on this service's ARN
//! for the PATH the request addressed — BEFORE it reads or writes anything. A request no
//! statement allows (IAM's default deny), or one an explicit `Deny` covers, is answered
//! HTTP 403 `AccessDeniedException`, performs nothing and leaves no resource behind, while
//! a request a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the
//! one `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all twelve of them: a
//! create, a resource-scoped read, three listings, two bound listings, a cancel, an export
//! start and read, and the three tag APIs), the document in force (none at all, another
//! action of this service, another service's actions, an explicit deny beside an allow, an
//! allow of exactly the action, of the action in another case, of the whole service and of
//! everything), the resource an allow is scoped to (every resource, the search-job
//! collection, one job's path, a path in another Region) and the principal (an
//! assumed-role session and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::search_jobs::percent_encode;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the assumed-role session every row signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "backup-searcher";
const SESSION: &str = "search-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "backup-search";

/// A well-formed identifier of this service that no call ever minted, so a row that gets
/// past authorization meets the operation's own not-found answer rather than a second
/// authorization decision.
const UNKNOWN_JOB: &str = "48434c42-40a0-4057-b04d-3b484c610e2b";

/// A router serving this service beside the IAM and STS services, over ONE shared engine,
/// so the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backupsearch_cryobackupsearchservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// What one call answered: its HTTP status, the wire error code and the parsed body.
struct Answer {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Answer {
    /// The refusal's `message`, or the empty string when the answer carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Call one operation of this service signed with `access_key_id` — what names the
/// principal the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &Operation,
) -> Answer {
    let payload = operation
        .body
        .as_ref()
        .map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let request = http::Request::builder()
        .method(operation.method)
        .uri(format!("{}{}", operation.path, operation.query))
        .header("host", "backup-search.amazonaws.com")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20261021/{REGION}/{ACTION_PREFIX}/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_code,
        body,
    }
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEBACKUPSEARCH1".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One operation of this service, as a client addresses it: the request line the rule is
/// judged on, and the body it needs to be a request the operation would otherwise serve or
/// answer on its own terms.
struct Operation {
    name: &'static str,
    method: &'static str,
    /// The URI PATH, which is what the resource the request is judged against is built
    /// from — so it is held apart from the query string.
    path: String,
    query: &'static str,
    body: Option<Value>,
}

/// An ARN of this service naming a search job nothing created, for the tag APIs: the tag
/// routes carry their resource as a path label, so the ARN travels inside the path.
fn unresolvable_job_arn() -> String {
    format!(
        "arn:aws:{ACTION_PREFIX}:{REGION}:{}:search-job/{UNKNOWN_JOB}",
        ::engine::emulator_account_id()
    )
}

/// Every operation this service serves, each with a request of its own. The rule covers
/// all of them, so the table is the whole surface rather than a sample of it.
fn operations() -> Vec<Operation> {
    let tag_path = format!("/tags/{}", percent_encode(&unresolvable_job_arn()));
    vec![
        Operation {
            name: "StartSearchJob",
            method: "PUT",
            path: "/search-jobs".to_string(),
            query: "",
            body: Some(json!({
                "Name": "authorized",
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            })),
        },
        Operation {
            name: "ListSearchJobs",
            method: "GET",
            path: "/search-jobs".to_string(),
            query: "",
            body: None,
        },
        Operation {
            name: "GetSearchJob",
            method: "GET",
            path: format!("/search-jobs/{UNKNOWN_JOB}"),
            query: "",
            body: None,
        },
        Operation {
            name: "ListSearchJobResults",
            method: "GET",
            path: format!("/search-jobs/{UNKNOWN_JOB}/search-results"),
            query: "",
            body: None,
        },
        Operation {
            name: "ListSearchJobBackups",
            method: "GET",
            path: format!("/search-jobs/{UNKNOWN_JOB}/backups"),
            query: "",
            body: None,
        },
        Operation {
            name: "StopSearchJob",
            method: "PUT",
            path: format!("/search-jobs/{UNKNOWN_JOB}/actions/cancel"),
            query: "",
            body: None,
        },
        Operation {
            name: "StartSearchResultExportJob",
            method: "PUT",
            path: "/export-search-jobs".to_string(),
            query: "",
            body: Some(json!({
                "SearchJobIdentifier": UNKNOWN_JOB,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "exports"}
                }
            })),
        },
        Operation {
            name: "GetSearchResultExportJob",
            method: "GET",
            path: format!("/export-search-jobs/{UNKNOWN_JOB}"),
            query: "",
            body: None,
        },
        Operation {
            name: "ListSearchResultExportJobs",
            method: "GET",
            path: "/export-search-jobs".to_string(),
            query: "",
            body: None,
        },
        Operation {
            name: "ListTagsForResource",
            method: "GET",
            path: tag_path.clone(),
            query: "",
            body: None,
        },
        Operation {
            name: "TagResource",
            method: "POST",
            path: tag_path.clone(),
            query: "",
            body: Some(json!({"Tags": {"env": "test"}})),
        },
        Operation {
            name: "UntagResource",
            method: "DELETE",
            path: tag_path,
            query: "?tagKeys=env",
            body: None,
        },
    ]
}

/// The documents a row puts in force on the role, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("backup:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["backup:StartBackupJob", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// The resource ARN a request addressing `path` is judged against: this service's ARN with
/// that path as its resource field, in the Region the request was signed for.
fn resource_of(path: &str) -> String {
    format!(
        "arn:aws:{ACTION_PREFIX}:{REGION}:{}:{path}",
        ::engine::emulator_account_id()
    )
}

/// THE RULE: every operation is judged against its OWN `backup-search:<OperationName>`
/// action before it performs anything; a refusal is HTTP 403 `AccessDeniedException`
/// naming the principal, the action, the resource the request addressed and which of
/// IAM's rules refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for operation in operations() {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        // Another action of this service, which a row grants instead of the one under
        // test.
        let other = operations()
            .into_iter()
            .map(|row| format!("{ACTION_PREFIX}:{}", row.name))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.name);
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let answer = call_as(&router, SESSION_KEY_ID, &operation).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected HTTP 403, got {} ({:?})",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.error_code.as_deref(),
                        Some("AccessDeniedException"),
                        "case {case:?}: expected an AccessDeniedException, got {:?} ({:?})",
                        answer.error_code,
                        answer.body
                    );
                    assert_eq!(
                        answer.message(),
                        format!(
                            "User: {} is not authorized to perform: {action} on resource: {} {}",
                            session_arn(),
                            resource_of(&operation.path),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {:?}",
                        answer.body
                    );
                }
                None => {
                    // Authorization answers BEFORE anything is read, so an allowed call
                    // falls through to the operation's own verdict — here a refusal about
                    // the job nothing created, or a success for an operation that needs
                    // none — and never to another authorization decision.
                    assert_ne!(
                        answer.status, 403,
                        "case {case:?}: an allowed request must not be refused, got {:?}",
                        answer.body
                    );
                    assert_ne!(
                        answer.error_code.as_deref(),
                        Some("AccessDeniedException"),
                        "case {case:?}: an allowed request must not be refused, got {:?}",
                        answer.body
                    );
                }
            }
        }
    }
}

/// RULE: the decision is taken before the handler runs, so a refused call PERFORMS
/// NOTHING: a `StartSearchJob` no policy allows leaves no job behind, which the account's
/// own credential — which the gate does not bound — then reads back.
#[tokio::test]
async fn a_refused_start_creates_no_search_job() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_role_policy(&engine, None);

    let start = operations()
        .into_iter()
        .find(|row| row.name == "StartSearchJob")
        .expect("the start operation");
    let refused = call_as(&router, SESSION_KEY_ID, &start).await;
    assert_eq!(
        refused.status, 403,
        "the unauthorized start must be refused, got {} ({:?})",
        refused.status, refused.body
    );
    assert!(
        refused.body.get("SearchJobArn").is_none()
            && refused.body.get("SearchJobIdentifier").is_none(),
        "a refused start must report no job, got {:?}",
        refused.body
    );

    let listing = operations()
        .into_iter()
        .find(|row| row.name == "ListSearchJobs")
        .expect("the listing operation");
    let listed = call_as(&router, ACCOUNT_KEY_ID, &listing).await;
    assert_eq!(
        listed.status, 200,
        "the account's own credential must be served, got {} ({:?})",
        listed.status, listed.body
    );
    assert_eq!(
        listed
            .body
            .get("SearchJobs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused start must have created no job, got {:?}",
        listed.body
    );
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for operation in operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION above.
        given_role_policy(&engine, None);

        let answer = call_as(&router, ACCOUNT_KEY_ID, &operation).await;
        assert_ne!(
            answer.status, 403,
            "case {:?}: the account's own credential must not be refused, got {:?}",
            operation.name, answer.body
        );
        assert_ne!(
            answer.error_code.as_deref(),
            Some("AccessDeniedException"),
            "case {:?}: the account's own credential must not be refused, got {:?}",
            operation.name,
            answer.body
        );
    }
}

/// RULE: a request is judged against the ARN of the PATH it addressed, so an allow scoped
/// to one path covers the operations that address it and no other — including a path of
/// the same shape in another Region.
#[tokio::test]
async fn an_action_is_judged_against_the_path_the_request_addressed() {
    let collection = resource_of("/search-jobs");
    let one_job = resource_of(&format!("/search-jobs/{UNKNOWN_JOB}"));
    let any_job = resource_of("/search-jobs/*");
    let another_region = format!(
        "arn:aws:{ACTION_PREFIX}:eu-west-1:{}:/search-jobs",
        ::engine::emulator_account_id()
    );

    // (case, the Resource the allow is scoped to, which operations it covers)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "every resource",
            "*".to_string(),
            vec!["StartSearchJob", "ListSearchJobs", "GetSearchJob"],
        ),
        (
            "the search-job collection alone",
            collection,
            vec!["StartSearchJob", "ListSearchJobs"],
        ),
        ("exactly one job's path", one_job, vec!["GetSearchJob"]),
        ("any job's path", any_job, vec!["GetSearchJob"]),
        (
            "the collection in another Region",
            another_region,
            Vec::new(),
        ),
    ];

    for (case, resource, covered) in rows {
        for operation in operations() {
            if !["StartSearchJob", "ListSearchJobs", "GetSearchJob"].contains(&operation.name) {
                continue;
            }
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(
                &engine,
                Some(&json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": format!("{ACTION_PREFIX}:*"),
                    "Resource": resource.clone(),
                    }],
                })),
            );

            let answer = call_as(&router, SESSION_KEY_ID, &operation).await;
            let allowed = covered.contains(&operation.name);
            assert_eq!(
                answer.error_code.as_deref() == Some("AccessDeniedException"),
                !allowed,
                "case {case:?}/{}: an allow scoped to {resource:?} must {}cover this \
                 operation, got {} ({:?})",
                operation.name,
                if allowed { "" } else { "not " },
                answer.status,
                answer.body
            );
        }
    }
}
