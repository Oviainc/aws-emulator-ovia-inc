// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTHORIZATION of this service's index operations —
//! `CreateIndex`, `GetIndex`, `UpdateIndex` and `DeleteIndex` — driven over the real wire
//! path (a `restJson1` HTTP request through the multi-service router into the mounted
//! service), so what is asserted is what a client sees.
//!
//! The rule under test: before it reads or writes the index, each index operation
//! evaluates the identity-based IAM policies in force on the principal the request's
//! credentials name against `es:<OperationName>` on the ARN of the domain the request
//! names, and a request no statement allows — or one an explicit `Deny` covers — is
//! refused with the modelled `AccessDeniedException` (HTTP 403) and performs nothing.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with. The harness comes
//! from [`super::domain_lifecycle`]; this suite adds no second copy of it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::INDEX_STATUS_UPDATED;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, DEFAULT_ACCESS_KEY_ID, PREFIX,
};
use super::index_registry::{created_index, stored_names};

/// The role every session below is assumed from.
const ROLE: &str = "opensearch-index-reader";

/// The session name the assumed-role credential carries.
const SESSION: &str = "index-session";

/// The access key id of the assumed-role session's credentials.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The domain every case addresses, and the index it holds.
pub(crate) const DOMAIN: &str = "search-target";
pub(crate) const INDEX: &str = "catalog";

/// The schema the fixture's index is created with.
pub(crate) fn created_schema() -> Value {
    json!({ "mappings": { "properties": { "title": { "type": "text" } } } })
}

/// The four operations of this service that act on an index of a domain
/// (`smithy_query(query="digest")`: each is bound under
/// `/opensearch/domain/{DomainName}/index…` and models `AccessDeniedException`, 403).
const INDEX_OPERATIONS: [&str; 4] = ["CreateIndex", "GetIndex", "UpdateIndex", "DeleteIndex"];

/// One emulator with a domain, an index on it and an assumed-role session recorded — the
/// precondition every case below starts from.
pub(crate) struct Fixture {
    pub(crate) emulator: Emulator,
    /// The domain's ARN as `DescribeDomain` reports it: what a policy scopes a statement
    /// to and what a refusal names.
    pub(crate) domain_arn: String,
}

impl Fixture {
    pub(crate) async fn new() -> Self {
        let emulator = Emulator::new();
        created_domain(&emulator, DOMAIN).await;
        created_index(&emulator, DOMAIN, INDEX, &created_schema(), "fixture").await;
        let described = emulator.describe(DOMAIN).await;
        assert_eq!(
            described.status, 200,
            "the fixture domain must be readable, got {} with {:?}",
            described.status, described.body
        );
        let domain_arn = string_member(&described.body["DomainStatus"], "ARN", "fixture");
        let fixture = Self {
            emulator,
            domain_arn,
        };
        fixture.given_session();
        fixture
    }

    /// Record the assumed-role session [`SESSION_KEY_ID`] speaks for, as `sts:AssumeRole`
    /// records one: the identity is the `assumed-role` ARN `sts:GetCallerIdentity`
    /// reports.
    fn given_session(&self) {
        let now = SystemTime::now();
        let account = ::engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{ROLE}");
        let store = sts::service_state(&**self.emulator.engine()).expect("the STS store");
        store
            .lock()
            .insert(sts::Session {
                access_key_id: SESSION_KEY_ID.to_string(),
                created_at: now,
                expires_at: now + Duration::from_secs(SESSION_SECONDS as u64),
                duration_seconds: SESSION_SECONDS,
                identity: sts::Identity {
                    user_id: sts::role_id(&role_arn),
                    account: account.to_string(),
                    arn: self.session_arn(),
                },
                kind: sts::SessionKind::AssumedRole,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                policies: sts::SessionPolicies::default(),
                external_id: None,
                role_arn: Some(role_arn),
                revoked: false,
            })
            .expect("the session is recorded");
    }

    /// The principal ARN the session speaks for — what a refusal must name.
    pub(crate) fn session_arn(&self) -> String {
        format!(
            "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
            ::engine::emulator_account_id()
        )
    }

    /// Attach `document` to the role as its inline policy, replacing whatever it carried
    /// — what `iam:PutRolePolicy` does.
    ///
    /// A `document` that is a JSON STRING is stored verbatim rather than as its JSON
    /// text, because that is how a percent-encoded document — the form IAM reports a
    /// policy in — and a document that is not a policy at all reach the store.
    pub(crate) fn given_role_policy(&self, document: &Value) {
        let policy_document = match document.as_str() {
            Some(verbatim) => verbatim.to_string(),
            None => document.to_string(),
        };
        let store = iam::service_state(&**self.emulator.engine()).expect("the IAM store");
        store.lock().role_inline_policies.insert(
            iam::name_key(ROLE),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document,
                },
            )]),
        );
    }

    /// Take every policy off the role — what `iam:DeleteRolePolicy` does.
    fn given_no_role_policy(&self) {
        let store = iam::service_state(&**self.emulator.engine()).expect("the IAM store");
        store
            .lock()
            .role_inline_policies
            .remove(&iam::name_key(ROLE));
    }

    /// Attach the managed policy `arn` to the role — what `iam:AttachRolePolicy` does.
    fn given_attached_policy(&self, arn: &str) {
        let store = iam::service_state(&**self.emulator.engine()).expect("the IAM store");
        store
            .lock()
            .role_attached_policies
            .entry(iam::name_key(ROLE))
            .or_default()
            .insert(arn.to_string());
    }

    /// One index operation, signed with `access_key_id`, against `index`.
    async fn index_call(&self, access_key_id: &str, operation: &str, index: &str) -> Answer {
        let one = format!("{PREFIX}/opensearch/domain/{DOMAIN}/index/{index}");
        match operation {
            "CreateIndex" => {
                self.emulator
                    .call_as(
                        access_key_id,
                        "POST",
                        &format!("{PREFIX}/opensearch/domain/{DOMAIN}/index"),
                        Some(json!({ "IndexName": index, "IndexSchema": created_schema() })),
                    )
                    .await
            }
            "GetIndex" => self.emulator.call_as(access_key_id, "GET", &one, None).await,
            "UpdateIndex" => {
                self.emulator
                    .call_as(
                        access_key_id,
                        "PUT",
                        &one,
                        Some(json!({ "IndexSchema": added_property("summary", "text") })),
                    )
                    .await
            }
            "DeleteIndex" => {
                self.emulator
                    .call_as(access_key_id, "DELETE", &one, None)
                    .await
            }
            other => panic!("case {other:?}: not an index operation of this service"),
        }
    }

    /// The schema `GetIndex` reports for the fixture's index, read as the account's own
    /// identity so the read is never the call under test.
    async fn stored_schema(&self, case: &str) -> Value {
        let read = super::index_registry::get(&self.emulator, DOMAIN, INDEX).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the fixture index must stay readable, got {} with {:?}",
            read.status, read.body
        );
        read.body["IndexSchema"].clone()
    }
}

/// An update document adding one mapping property of `kind`.
pub(crate) fn added_property(property: &str, kind: &str) -> Value {
    json!({ "mappings": { "properties": { property: { "type": kind } } } })
}

/// A policy allowing `actions` on `resources`.
pub(crate) fn allow(actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resources }]
    })
}

/// Which of IAM's two refusals a row expects: the default deny of a request no statement
/// allows, or the explicit deny of one a `Deny` statement covers.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Refusal {
    DefaultDeny,
    ExplicitDeny,
}

impl Refusal {
    /// The reason clause IAM closes the refusal with.
    pub(crate) fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The refusal an unauthorized call must carry, in IAM's own wording: who was refused,
/// which action, on which resource, and which rule refused it.
pub(crate) fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}")
}

/// Assert `answer` is the modelled authorization refusal naming `action` on `resource`.
pub(crate) fn assert_refused(
    answer: &Answer,
    case: &str,
    principal: &str,
    action: &str,
    resource: &str,
    refusal: Refusal,
) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str().unwrap_or_default(),
        not_authorized(principal, action, resource, &refusal.reason(action)),
        "case {case:?}: expected IAM's refusal wording, got {:?}",
        answer.body
    );
}

/// RULE: every operation that acts on an index takes the decision — not only the one a
/// report happened to name — so a principal granted every OTHER index action is refused
/// this one with 403 and performs nothing, while a grant of this very action lets the
/// request through to its own answer.
#[tokio::test]
async fn every_index_operation_is_authorized_against_the_action_it_names() {
    for operation in INDEX_OPERATIONS {
        let action = format!("es:{operation}");
        // The index the call addresses: a create must name one the domain does not hold,
        // every other operation the one the fixture created.
        let target = if operation == "CreateIndex" {
            "minted"
        } else {
            INDEX
        };

        // Each row: the case, the policy the role carries, and which refusal IAM reaches
        // — the default deny of a request no statement allows, the explicit deny of one a
        // `Deny` covers, or `None` for the request a matching `Allow` permits.
        let others: Vec<Value> = INDEX_OPERATIONS
            .iter()
            .filter(|other| **other != operation)
            .map(|other| json!(format!("es:{other}")))
            .collect();
        let rows: Vec<(&str, Value, Option<Refusal>)> = vec![
            (
                "every other index action, but not this one",
                allow(json!(others), json!("*")),
                Some(Refusal::DefaultDeny),
            ),
            (
                "an explicit deny of this action beside an allow of everything",
                json!({
                    "Statement": [
                        { "Effect": "Allow", "Action": "es:*", "Resource": "*" },
                        { "Effect": "Deny", "Action": action.clone(), "Resource": "*" }
                    ]
                }),
                Some(Refusal::ExplicitDeny),
            ),
            (
                "this very action on the domain",
                allow(json!(action.clone()), json!("*")),
                None,
            ),
        ];

        for (row, policy, refusal) in rows {
            let case = format!("{operation}: {row}");
            let fixture = Fixture::new().await;
            fixture.given_role_policy(&policy);
            let before = fixture.stored_schema(&case).await;

            let answer = fixture.index_call(SESSION_KEY_ID, operation, target).await;
            match refusal {
                Some(reason) => assert_refused(
                    &answer,
                    &case,
                    &fixture.session_arn(),
                    &action,
                    &fixture.domain_arn,
                    reason,
                ),
                None => {
                    assert_ne!(
                        answer.status, 403,
                        "case {case:?}: a permitted call must not be refused by authorization, \
                         got {} with {:?}",
                        answer.status, answer.body
                    );
                    assert_ne!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: a permitted call must not answer an \
                         AccessDeniedException, got {:?}",
                        answer.body
                    );
                    continue;
                }
            }

            // A refused call is a no-op: the domain still holds exactly the index the
            // fixture created, carrying the schema it was created with.
            assert_eq!(
                stored_names(&fixture.emulator, DOMAIN, &case),
                vec![INDEX.to_string()],
                "case {case:?}: a refused call must neither add nor remove an index"
            );
            assert_eq!(
                fixture.stored_schema(&case).await,
                before,
                "case {case:?}: a refused call must leave the stored schema untouched"
            );
        }
    }
}

/// RULE: the refused call performs nothing and the PERMITTED caller is the one whose
/// identical request lands the change — so the two callers are distinguishable, over a
/// table of update documents differing in the property name, its type and how many
/// properties one call writes.
#[tokio::test]
async fn a_refused_update_is_a_no_op_and_the_permitted_caller_lands_the_same_change() {
    // Each row: the case, the update document, and the mapping property it must add.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("one text property", added_property("summary", "text"), "summary"),
        (
            "a property of another type",
            added_property("views", "integer"),
            "views",
        ),
        (
            "two properties at once",
            json!({ "mappings": { "properties": {
                "author": { "type": "keyword" },
                "published": { "type": "date" }
            } } }),
            "author",
        ),
        (
            "a property nested under an object field",
            json!({ "mappings": { "properties": {
                "meta": { "properties": { "isbn": { "type": "keyword" } } }
            } } }),
            "meta",
        ),
    ];

    for (case, update, property) in rows {
        let fixture = Fixture::new().await;
        // The least-privilege shape: a read-only grant on the domain and its
        // sub-resources, which says nothing about changing an index.
        fixture.given_role_policy(&allow(
            json!(["es:DescribeDomain", "es:GetIndex"]),
            json!([
                fixture.domain_arn.clone(),
                format!("{}/*", fixture.domain_arn)
            ]),
        ));

        let refused = fixture
            .emulator
            .call_as(
                SESSION_KEY_ID,
                "PUT",
                &format!("{PREFIX}/opensearch/domain/{DOMAIN}/index/{INDEX}"),
                Some(json!({ "IndexSchema": update.clone() })),
            )
            .await;
        assert_refused(
            &refused,
            case,
            &fixture.session_arn(),
            "es:UpdateIndex",
            &fixture.domain_arn,
            Refusal::DefaultDeny,
        );

        // The denied call changed nothing: the property it would have added is absent.
        let unchanged = fixture.stored_schema(case).await;
        assert!(
            unchanged["mappings"]["properties"][property].is_null(),
            "case {case:?}: the refused update must not have added {property:?}, got \
             {unchanged:?}"
        );

        // The grant the role DOES hold still works, so the refusal is about the action
        // and not about the caller being unable to reach the domain at all.
        let read_as_role = fixture.index_call(SESSION_KEY_ID, "GetIndex", INDEX).await;
        assert_eq!(
            read_as_role.status, 200,
            "case {case:?}: the role holds es:GetIndex, so its read must be served, got {} \
             with {:?}",
            read_as_role.status, read_as_role.body
        );

        // …and the identical request from the permitted caller is the one that lands it.
        let applied = fixture
            .emulator
            .call_as(
                DEFAULT_ACCESS_KEY_ID,
                "PUT",
                &format!("{PREFIX}/opensearch/domain/{DOMAIN}/index/{INDEX}"),
                Some(json!({ "IndexSchema": update })),
            )
            .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: the permitted caller's update must be accepted, got {} with {:?}",
            applied.status, applied.body
        );
        assert_eq!(
            string_member(&applied.body, "Status", case),
            INDEX_STATUS_UPDATED,
            "case {case:?}: an accepted update reports the status of an update"
        );
        let after = fixture.stored_schema(case).await;
        assert!(
            !after["mappings"]["properties"][property].is_null(),
            "case {case:?}: the permitted update must have added {property:?}, got {after:?}"
        );
    }
}

/// RULE: the decision follows the role's identity policies in IAM's own order — a `Deny`
/// wins whichever side of an `Allow` it is written on, an `Allow` matching the action AND
/// the resource permits, and nothing matching is IAM's default deny. The rows differ along
/// what the rule generalises over: the action pattern's wildcards, `NotAction`, the
/// resource a statement is scoped to, the statement's effect and its position, the
/// document's encoding and a statement this emulator cannot decide.
#[tokio::test]
async fn an_update_is_decided_by_the_roles_policies_in_iams_own_order() {
    let fixture = Fixture::new().await;
    let domain_arn = fixture.domain_arn.clone();
    let account = ::engine::emulator_account_id();

    // Each row: the case, the policy the role carries (None takes every policy off it),
    // and the expected HTTP status of the update.
    let rows: Vec<(&str, Option<Value>, u16)> = vec![
        ("no policy at all is IAM's default deny", None, 403),
        (
            "the exact action on the domain",
            Some(allow(json!("es:UpdateIndex"), json!(domain_arn.clone()))),
            200,
        ),
        (
            "every action of this service",
            Some(allow(json!("es:*"), json!("*"))),
            200,
        ),
        (
            "every action of every service",
            Some(allow(json!("*"), json!("*"))),
            200,
        ),
        (
            "an action pattern matching by prefix",
            Some(allow(json!("es:Update*"), json!("*"))),
            200,
        ),
        (
            "an action pattern matching with ?",
            Some(allow(json!("es:UpdateInde?"), json!("*"))),
            200,
        ),
        (
            "only another action of this service",
            Some(allow(json!("es:GetIndex"), json!("*"))),
            403,
        ),
        (
            "another service's actions",
            Some(allow(json!("s3:*"), json!("*"))),
            403,
        ),
        (
            "an Allow scoped to every domain of the account",
            Some(allow(
                json!("es:UpdateIndex"),
                json!(format!("arn:aws:es:*:{account}:domain/*")),
            )),
            200,
        ),
        (
            "an Allow scoped to another domain",
            Some(allow(
                json!("es:UpdateIndex"),
                json!(format!("arn:aws:es:*:{account}:domain/search-elsewhere")),
            )),
            403,
        ),
        (
            "a NotAction naming another action",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "NotAction": "es:DeleteIndex",
                    "Resource": "*"
                }]
            })),
            200,
        ),
        (
            "a NotAction naming this one",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "NotAction": "es:UpdateIndex",
                    "Resource": "*"
                }]
            })),
            403,
        ),
        (
            "a Deny written before an Allow",
            Some(json!({
                "Statement": [
                    { "Effect": "Deny", "Action": "es:*", "Resource": "*" },
                    { "Effect": "Allow", "Action": "es:UpdateIndex", "Resource": "*" }
                ]
            })),
            403,
        ),
        (
            // Statement order must not matter: an explicit Deny wins whichever side of
            // the Allow it is written on.
            "a Deny written after an Allow",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "es:UpdateIndex", "Resource": "*" },
                    { "Effect": "Deny", "Action": "es:*", "Resource": "*" }
                ]
            })),
            403,
        ),
        (
            "a single statement object rather than a list",
            Some(json!({
                "Statement": { "Effect": "Allow", "Action": "es:UpdateIndex", "Resource": "*" }
            })),
            200,
        ),
        (
            // A statement this emulator cannot decide is applied in the direction that
            // cannot grant more than IAM would: an Allow carrying one grants nothing, so
            // the request falls to IAM's default deny.
            "an Allow carrying a Condition this emulator does not evaluate",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "es:UpdateIndex",
                    "Resource": "*",
                    "Condition": { "StringEquals": { "aws:RequestedRegion": "us-east-1" } }
                }]
            })),
            403,
        ),
        (
            // …and a Deny carrying one still denies: the condition might hold, and a Deny
            // that is in force wins. The evaluation fails closed on both sides.
            "a Deny carrying a Condition, beside an Allow",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "es:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "es:UpdateIndex",
                        "Resource": "*",
                        "Condition": { "StringEquals": { "aws:RequestedRegion": "us-east-1" } }
                    }
                ]
            })),
            403,
        ),
        (
            // IAM reports a policy document percent-encoded, so one stored in that form
            // is read rather than skipped — a Deny must not be missed because of its
            // encoding.
            "a percent-encoded Deny",
            Some(json!(
                "%7B%22Statement%22%3A%5B%7B%22Effect%22%3A%22Deny%22%2C\
                 %22Action%22%3A%22es%3A%2A%22%2C%22Resource%22%3A%22%2A%22%7D%5D%7D"
            )),
            403,
        ),
        (
            "a document that is not a policy at all",
            Some(json!("not a policy")),
            403,
        ),
    ];

    for (case, policy, expected) in rows {
        match &policy {
            Some(document) => fixture.given_role_policy(document),
            None => fixture.given_no_role_policy(),
        }
        let answer = fixture
            .index_call(SESSION_KEY_ID, "UpdateIndex", INDEX)
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected == 403 {
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a grant this emulator can READ decides the request, whichever kind of policy
/// carries it; a grant it cannot read is served rather than refused, because refusing on
/// evidence it does not have would deny access AWS permits. An explicit `Deny` it can read
/// still refuses either way.
#[tokio::test]
async fn an_attached_policy_decides_the_request_only_where_its_document_can_be_read() {
    let fixture = Fixture::new().await;
    fixture.given_no_role_policy();
    let customer_managed = format!(
        "arn:aws:iam::{}:policy/opensearch-index-access",
        ::engine::emulator_account_id()
    );

    // Each row: the case, the policy ARN attached to the role, the inline policy beside
    // it (none when `None`), and the expected HTTP status of the update.
    let rows: Vec<(&str, &str, Option<Value>, u16)> = vec![
        (
            "a customer-managed policy the account does not hold",
            customer_managed.as_str(),
            None,
            403,
        ),
        (
            // From the IAM emulator's own catalog of AWS-managed policies, whose
            // documents it publishes: this one allows every action.
            "an AWS-managed policy the emulator publishes, allowing everything",
            "arn:aws:iam::aws:policy/AdministratorAccess",
            None,
            200,
        ),
        (
            // Also published, and it grants nothing of this service.
            "an AWS-managed policy the emulator publishes, granting another service",
            "arn:aws:iam::aws:policy/AmazonEC2ReadOnlyAccess",
            None,
            403,
        ),
        (
            // An ARN in AWS's namespace the catalog does not carry: what it grants is
            // unknown here, so the request is served rather than refused.
            "an AWS-managed policy whose document AWS owns and the catalog lacks",
            "arn:aws:iam::aws:policy/AmazonOpenSearchServiceFullAccess",
            None,
            200,
        ),
        (
            "…and an explicit Deny this emulator can read still refuses",
            "arn:aws:iam::aws:policy/AmazonOpenSearchServiceFullAccess",
            Some(json!({
                "Statement": [{ "Effect": "Deny", "Action": "es:*", "Resource": "*" }]
            })),
            403,
        ),
    ];

    for (case, arn, inline, expected) in rows {
        // Each row stands on its own attachment set.
        {
            let store = iam::service_state(&**fixture.emulator.engine()).expect("the IAM store");
            store
                .lock()
                .role_attached_policies
                .remove(&iam::name_key(ROLE));
        }
        fixture.given_attached_policy(arn);
        match &inline {
            Some(document) => fixture.given_role_policy(document),
            None => fixture.given_no_role_policy(),
        }
        let answer = fixture
            .index_call(SESSION_KEY_ID, "UpdateIndex", INDEX)
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected == 403 {
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a request whose access key id names no session speaks for the account's own
/// identity — the long-term key this emulator publishes — so it is served whatever
/// policies the account's roles carry; the session's identical request is refused, so the
/// two really are different callers.
#[tokio::test]
async fn a_request_that_names_no_session_is_served_as_the_accounts_own_identity() {
    let fixture = Fixture::new().await;
    // Everything this service can do is denied to the ROLE…
    fixture.given_role_policy(&json!({
        "Statement": [{ "Effect": "Deny", "Action": "es:*", "Resource": "*" }]
    }));

    for operation in INDEX_OPERATIONS {
        let target = if operation == "CreateIndex" {
            "minted"
        } else {
            INDEX
        };
        let refused = fixture.index_call(SESSION_KEY_ID, operation, target).await;
        assert_eq!(
            refused.status, 403,
            "case {operation:?}: the denied role must be refused, got {} with {:?}",
            refused.status, refused.body
        );
    }

    // …which says nothing about a request that does not speak for that role: it reads,
    // writes and removes the index as before.
    for operation in ["GetIndex", "UpdateIndex", "DeleteIndex", "CreateIndex"] {
        let target = if operation == "CreateIndex" {
            "minted"
        } else {
            INDEX
        };
        let served = fixture
            .index_call(DEFAULT_ACCESS_KEY_ID, operation, target)
            .await;
        assert_eq!(
            served.status, 200,
            "case {operation:?}: the account's own identity must be served, got {} with {:?}",
            served.status, served.body
        );
    }
}
