// SPDX-License-Identifier: CC0-1.0

//! WHO a flow names as its author and its last editor, checked against the service that
//! owns that question: `sts:GetCallerIdentity`.
//!
//! The rule under test: a member that records the principal that performed a write
//! (`createdBy`, "The ARN of the user who created the flow"; `lastUpdatedBy`, "Specifies
//! the user name of the account that performed the most recent update" —
//! `smithy_query(query="doc",shape="DescribeFlowResponse",member="createdBy")`) carries the
//! ARN of the identity the request's access key id RESOLVES to — the session that key
//! names, the IAM user the account holds the key for, or the account's own principal for a
//! key no store holds — which is exactly the identity `sts:GetCallerIdentity` answers for
//! the same credential. Every read path that projects a flow (`DescribeFlow`, and
//! `ListFlows`' `FlowDefinition`) reports that one stored value.
//!
//! Nothing here writes an expected ARN by hand: each row ASKS STS, mounted beside AppFlow
//! on the one shared engine, and compares. The rows span what the rule generalises over —
//! the kind of credential a call is signed with (a key no store holds, a long-term key the
//! account holds for a user, a second such key held for a DIFFERENT user, a session minted
//! by `sts:GetSessionToken`, a session of an assumed role) — and each IAM user's name is
//! deliberately unrelated to the text of the access key id it holds, so an identity
//! synthesised from that text cannot pass any row.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{Role, User};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::flow_lifecycle::{
    appflow_with_engine, call_as, map_task, on_demand_trigger, s3_destination, s3_source, Answer,
    ACCOUNT,
};

/// STS's awsQuery API version and SigV4 signing name: the scope its calls are made under.
const STS_API_VERSION: &str = "2011-06-15";
const STS_SIGNING_NAME: &str = "sts";

/// The IAM users the long-term rows' keys are held for, and the role the assumed-role row
/// is a session of. None of these names appears in the access key id that reaches them.
const KEY_HOLDING_USER: &str = "Mary_Major";
const OTHER_KEY_HOLDING_USER: &str = "Li_Juan";
const SESSION_USER: &str = "Saanvi_Sarkar";
const TRANSFER_ROLE: &str = "AppFlowTransfer";

/// A second principal of the account: it updates every row's flow, so `lastUpdatedBy` has
/// to move to IT while `createdBy` stays with the creator.
const EDITOR_USER: &str = "Carlos_Salazar";
const EDITOR_ACCESS_KEY_ID: &str = "AKIDFLOWEDITOR";

/// A third principal, which only READS: a read is answered from the store, so the ARNs it
/// reports must be the writers' and never the reader's.
const READER_ACCESS_KEY_ID: &str = "AKIDFLOWREADER";

/// The Amazon S3 locations the flows of this suite move data between — two of the buckets
/// the shared harness stands up with the policy AppFlow asks for.
const SOURCE_BUCKET: &str = "orders-landing";
const DESTINATION_BUCKET: &str = "orders-curated";

/// AppFlow and STS on ONE shared engine, so the identity STS answers for a credential is
/// resolved from the very records AppFlow resolves it from.
fn appflow_with_sts() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = appflow_with_engine();
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One STS awsQuery call, signed (in the routing sense) with `access_key_id`.
async fn sts_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={STS_API_VERSION}");
    for (name, value) in params {
        // Only the delimiters of an urlencoded body need escaping here; an ARN's `:` and
        // `/` are carried as a client's own encoder carries them.
        let encoded = value
            .replace('%', "%25")
            .replace('&', "%26")
            .replace('=', "%3D")
            .replace('+', "%2B");
        body.push_str(&format!("&{name}={encoded}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{STS_SIGNING_NAME}.amazonaws.com/"))
        .header("host", format!("{STS_SIGNING_NAME}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/\
                 {STS_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the one `<name>…</name>` element an awsQuery response carries.
fn element(case: &str, body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// The ARN `sts:GetCallerIdentity` answers for a request signed with `access_key_id`: the
/// identity that credential resolves to, asked of the service that owns the question.
async fn identity_arn_from_sts(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> String {
    let (status, body) = sts_call(router, access_key_id, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: sts:GetCallerIdentity must answer the signing credential, got \
         {status} with {body}"
    );
    element(case, &body, "Arn")
}

/// Record that the account holds `access_key_id` for the IAM user `user_name` — the user
/// record `iam:CreateUser` writes and the access-key record `iam:CreateAccessKey` writes —
/// and answer that user's ARN.
fn hold_long_term_key_for(
    engine: &engine::InMemoryEngine,
    user_name: &str,
    access_key_id: &str,
) -> String {
    let arn = format!("arn:aws:iam::{ACCOUNT}:user/{user_name}");
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut account = state.lock();
    account.users.insert(
        iam::name_key(user_name),
        User {
            path: "/".to_string(),
            user_name: user_name.to_string(),
            user_id: sts::state::unique_id("AIDA", &arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    account.access_keys.insert(
        access_key_id.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(user_name),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
        },
    );
    arn
}

/// Record a role of the account assumable by any principal of it, as `iam:CreateRole`
/// records one, and answer its ARN.
fn hold_role(engine: &engine::InMemoryEngine, role_name: &str) -> String {
    let arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
         \"Action\":[\"sts:AssumeRole\"]}}]}}"
    );
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    state.lock().roles.insert(
        iam::name_key(role_name),
        Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: sts::state::role_id(&arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust),
            description: None,
            max_session_duration: Some(3_600),
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// Which credential a row's flow is created with.
#[derive(Debug, Clone, Copy)]
enum Credential {
    /// A long-term key no store holds a record for: the position the credential this
    /// emulator publishes at start is in.
    UnheldKey,
    /// A long-term key the account holds for one of its IAM users.
    IamUserKey { user: &'static str },
    /// The temporary credentials of an `sts:GetSessionToken` session, minted here for an
    /// IAM user's own long-term key.
    SessionToken { user: &'static str },
    /// The temporary credentials of a role session, minted here by `sts:AssumeRole`.
    AssumedRole {
        role: &'static str,
        session: &'static str,
    },
}

impl Credential {
    /// Put this credential in place and answer the access key id a call signed with it
    /// carries. A session is MINTED through STS over the wire, so the registry the identity
    /// is later resolved from is the one that operation wrote.
    async fn access_key_id(
        self,
        router: &MultiServiceRouter,
        engine: &engine::InMemoryEngine,
        case: &str,
    ) -> String {
        match self {
            Self::UnheldKey => "AKIDNOSTOREHOLDSTHIS".to_string(),
            Self::IamUserKey { user } => {
                let access_key_id = format!("AKIDHELDKEY{}", user.len());
                hold_long_term_key_for(engine, user, &access_key_id);
                access_key_id
            }
            Self::SessionToken { user } => {
                let long_term = format!("AKIDTOKENHOLDER{}", user.len());
                hold_long_term_key_for(engine, user, &long_term);
                let (status, body) = sts_call(router, &long_term, "GetSessionToken", &[]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: sts:GetSessionToken must mint a session, got {status} with \
                     {body}"
                );
                element(case, &body, "AccessKeyId")
            }
            Self::AssumedRole { role, session } => {
                let role_arn = hold_role(engine, role);
                let (status, body) = sts_call(
                    router,
                    EDITOR_ACCESS_KEY_ID,
                    "AssumeRole",
                    &[
                        ("RoleArn", role_arn),
                        ("RoleSessionName", session.to_string()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: sts:AssumeRole must mint a session, got {status} with {body}"
                );
                element(case, &body, "AccessKeyId")
            }
        }
    }
}

/// One row: a credential kind, and the label its failures are reported under.
struct Row {
    case: &'static str,
    credential: Credential,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a long-term key no store holds",
            credential: Credential::UnheldKey,
        },
        Row {
            case: "a long-term key the account holds for one of its users",
            credential: Credential::IamUserKey {
                user: KEY_HOLDING_USER,
            },
        },
        Row {
            case: "a long-term key held for a different user",
            credential: Credential::IamUserKey {
                user: OTHER_KEY_HOLDING_USER,
            },
        },
        Row {
            case: "a session token speaking for the user that asked for it",
            credential: Credential::SessionToken { user: SESSION_USER },
        },
        Row {
            case: "a session of an assumed role",
            credential: Credential::AssumedRole {
                role: TRANSFER_ROLE,
                session: "transfer-session-1",
            },
        },
    ]
}

/// The definition every row creates and re-defines: on-demand, Amazon S3 to Amazon S3,
/// one field mapping.
fn flow_definition(flow_name: &str, description: &str) -> Value {
    json!({
        "flowName": flow_name,
        "description": description,
        "triggerConfig": on_demand_trigger(),
        "sourceFlowConfig": s3_source(SOURCE_BUCKET),
        "destinationFlowConfigList": [s3_destination(DESTINATION_BUCKET)],
        "tasks": [map_task("order_id", "id")]
    })
}

/// The `FlowDefinition` `ListFlows` reports for `flow_name`, read as `access_key_id`.
async fn listed_flow(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
    flow_name: &str,
) -> Value {
    let listed = call_as(router, access_key_id, "/list-flows", &json!({})).await;
    assert!(
        listed.is_ok(),
        "case {case:?}: expected ListFlows to answer, got {} {:?}",
        listed.status,
        listed.body
    );
    listed
        .get("flows")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|entry| entry.get("flowName").and_then(Value::as_str) == Some(flow_name))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected ListFlows to enumerate {flow_name:?}, got {:?}",
                listed.body
            )
        })
}

/// The member `member` of a `DescribeFlow` answer.
fn member_of(case: &str, described: &Answer, member: &str) -> String {
    described
        .get(member)
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {member} on the described flow, got {:?}",
                described.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_flow_names_the_principal_sts_reports_for_the_credential_that_wrote_it() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (router, engine) = appflow_with_sts();
        // The two other principals of the account: the one that updates every row's flow,
        // and the one that only reads it.
        let editor_arn = hold_long_term_key_for(&engine, EDITOR_USER, EDITOR_ACCESS_KEY_ID);
        hold_long_term_key_for(&engine, "Akua_Mansa", READER_ACCESS_KEY_ID);
        let access_key_id = row.credential.access_key_id(&router, &engine, case).await;

        // The identity the row's credential resolves to, asked of STS rather than written
        // out here.
        let creator_arn = identity_arn_from_sts(&router, case, &access_key_id).await;
        assert_eq!(
            editor_arn,
            identity_arn_from_sts(&router, case, EDITOR_ACCESS_KEY_ID).await,
            "case {case:?}: the editor's own identity must be the one STS reports for its key"
        );

        let flow_name = format!("principal-flow-{index}");
        let created = call_as(
            &router,
            &access_key_id,
            "/create-flow",
            &flow_definition(&flow_name, "created by the row principal"),
        )
        .await;
        assert!(
            created.is_ok(),
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.body
        );

        // Read back by a THIRD principal: the ARNs a read reports come from the store, so
        // they name the writers and never the reader.
        let described = call_as(
            &router,
            READER_ACCESS_KEY_ID,
            "/describe-flow",
            &json!({ "flowName": flow_name }),
        )
        .await;
        assert!(
            described.is_ok(),
            "case {case:?}: expected DescribeFlow to answer, got {} {:?}",
            described.status,
            described.body
        );
        for member in ["createdBy", "lastUpdatedBy"] {
            let who = member_of(case, &described, member);
            assert_eq!(
                who, creator_arn,
                "case {case:?}: expected {member} to be the principal STS reports for the \
                 creating credential ({creator_arn:?}), got {who:?}"
            );
        }

        // `ListFlows` projects the same stored record.
        let listed = listed_flow(&router, case, READER_ACCESS_KEY_ID, &flow_name).await;
        for member in ["createdBy", "lastUpdatedBy"] {
            let who = listed.get(member).and_then(Value::as_str).unwrap_or_default();
            assert_eq!(
                who, creator_arn,
                "case {case:?}: expected the listed {member} to be {creator_arn:?}, got {who:?} \
                 in {listed:?}"
            );
        }

        // A re-definition by ANOTHER principal moves `lastUpdatedBy` to it and leaves
        // `createdBy` with the creator: two members, two writers, one resolution each.
        let updated = call_as(
            &router,
            EDITOR_ACCESS_KEY_ID,
            "/update-flow",
            &flow_definition(&flow_name, "re-defined by the editor principal"),
        )
        .await;
        assert!(
            updated.is_ok(),
            "case {case:?}: expected UpdateFlow to succeed, got {} {:?}",
            updated.status,
            updated.body
        );
        let after = call_as(
            &router,
            READER_ACCESS_KEY_ID,
            "/describe-flow",
            &json!({ "flowName": flow_name }),
        )
        .await;
        let created_by = member_of(case, &after, "createdBy");
        let last_updated_by = member_of(case, &after, "lastUpdatedBy");
        assert_eq!(
            created_by, creator_arn,
            "case {case:?}: expected createdBy to survive the update as {creator_arn:?}, got \
             {created_by:?}"
        );
        assert_eq!(
            last_updated_by, editor_arn,
            "case {case:?}: expected lastUpdatedBy to name the updating principal \
             ({editor_arn:?}), got {last_updated_by:?}"
        );
        let listed_after = listed_flow(&router, case, READER_ACCESS_KEY_ID, &flow_name).await;
        assert_eq!(
            listed_after.get("createdBy").and_then(Value::as_str),
            Some(creator_arn.as_str()),
            "case {case:?}: expected the listed createdBy to stay {creator_arn:?}, got \
             {listed_after:?}"
        );
        assert_eq!(
            listed_after.get("lastUpdatedBy").and_then(Value::as_str),
            Some(editor_arn.as_str()),
            "case {case:?}: expected the listed lastUpdatedBy to be {editor_arn:?}, got \
             {listed_after:?}"
        );

        // Nothing about either identity depends on the access key id's TEXT: an ARN
        // spelled out of that text is not what the service recorded.
        for (presented, recorded) in [
            (access_key_id.as_str(), creator_arn.as_str()),
            (EDITOR_ACCESS_KEY_ID, editor_arn.as_str()),
        ] {
            let derived_from_key_text = format!("arn:aws:iam::{ACCOUNT}:user/{presented}");
            assert_ne!(
                recorded, derived_from_key_text,
                "case {case:?}: the recorded principal must come from the credential's record, \
                 not from the key id's text"
            );
        }

        // Two principals that have to be two principals are two: no row collapses the
        // creator and the editor into one identity.
        assert_ne!(
            creator_arn, editor_arn,
            "case {case:?}: two credentials of two principals must resolve apart"
        );

        // The resolution is a READ: STS still answers the same identity for the row's
        // credential afterwards, and so does a second DescribeFlow.
        let again = identity_arn_from_sts(&router, case, &access_key_id).await;
        assert_eq!(
            again, creator_arn,
            "case {case:?}: expected a stable identity across reads, got {again:?}"
        );
        let read_again = call_as(
            &router,
            READER_ACCESS_KEY_ID,
            "/describe-flow",
            &json!({ "flowName": flow_name }),
        )
        .await;
        assert_eq!(
            member_of(case, &read_again, "createdBy"),
            creator_arn,
            "case {case:?}: expected a second read to report the same createdBy"
        );
        assert_eq!(
            member_of(case, &read_again, "lastUpdatedBy"),
            editor_arn,
            "case {case:?}: expected a second read to report the same lastUpdatedBy"
        );
    }
}
