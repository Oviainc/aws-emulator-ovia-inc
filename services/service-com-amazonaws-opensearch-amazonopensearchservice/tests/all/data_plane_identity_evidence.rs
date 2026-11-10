// SPDX-License-Identifier: CC0-1.0

//! What a signed data-plane request is decided by, and which caller a domain's access
//! policy does not refuse on its own.
//!
//! The rule under test: **a request to the address a domain publishes is authorized by IAM's
//! two sides together — the domain's own access policy and the signing principal's
//! identity-based policies — and the ACCOUNT'S OWN published credential, which no identity
//! policy of this emulator bounds (the position AWS account root credentials are in), is
//! therefore served unless a policy explicitly DENIES it, while a principal whose identity
//! policies this emulator reads is decided by those policies beside the access policy.**
//! Refusing the account's own credential because the access policy says nothing about it
//! would deny a request the side this emulator cannot read may allow; an explicit `Deny`
//! refuses it, because that rule wins over every allow whichever side states it.
//!
//! The rows differ along what the rule generalises over: who signs (the account's own
//! credential, and a session whose role IAM holds), what the domain's access policy states
//! about that caller (nothing at all, an allow about someone else, an allow about the caller,
//! an explicit deny about its account, an explicit deny about everyone) and which action the
//! request asks for (a read of the cluster and a document write), so neither the verdict nor
//! the action in a refusal is a fixed answer.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, sent to the address the control plane PUBLISHED for the domain.
//! The control-plane harness comes from [`super::domain_lifecycle`] and the session, policy
//! and published-address helpers from [`super::published_domain_address`]; this suite adds no
//! second copy of them.

use serde_json::{json, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use super::domain_lifecycle::{Answer, Emulator, DEFAULT_ACCESS_KEY_ID, REGION, SETTLE_WAIT};
use super::published_domain_address::{
    at_address, given_session, published_endpoint, refusal_message, role_arn, session_arn,
};

/// The session name the seeded credential carries.
const SESSION: &str = "evidence-session";

/// A role this emulator HOLDS a session for, carrying no identity policy of its own — a
/// principal whose identity-based policies this emulator READS (and reads nothing in), which
/// is what separates it from the account's own credential below.
const READER_ROLE: &str = "evidence-reader";
const READER_KEY: &str = "ASIAEXAMPLEEVIDENCE1";

/// The role an access policy names when it is about SOMEONE ELSE.
const OTHER_ROLE: &str = "evidence-someone-else";

/// The clause a refusal carries when the policy that refused is an explicit `Deny`.
const EXPLICIT_DENY: &str = "with an explicit deny in a resource-based policy";

/// The clause it carries when the domain carries an access policy and nothing in it allows
/// the request.
fn nothing_allows(action: &str) -> String {
    format!("because no resource-based policy allows the {action} action")
}

/// The ARN pattern an access policy scopes a statement about ONE domain to.
fn domain_resource(domain: &str) -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/{domain}/*",
        ::engine::emulator_account_id()
    )
}

/// One domain of the table: its name and the access policy it is created with.
struct Domain {
    name: &'static str,
    /// The `Statement` the policy carries, or `None` for a domain created with no access
    /// policy at all.
    statements: Option<fn(&str) -> Value>,
}

/// An allow naming a role the caller is NOT.
fn allows_someone_else(domain: &str) -> Value {
    json!([{
        "Effect": "Allow",
        "Principal": { "AWS": role_arn(OTHER_ROLE) },
        "Action": "es:ESHttp*",
        "Resource": domain_resource(domain),
    }])
}

/// An allow naming the seeded reader's own role.
fn allows_the_reader(domain: &str) -> Value {
    json!([{
        "Effect": "Allow",
        "Principal": { "AWS": role_arn(READER_ROLE) },
        "Action": "es:ESHttp*",
        "Resource": domain_resource(domain),
    }])
}

/// An allow for everyone with an explicit deny about the account every caller here belongs
/// to.
fn denies_the_account(domain: &str) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Principal": "*",
            "Action": "es:ESHttp*",
            "Resource": domain_resource(domain),
        },
        {
            "Effect": "Deny",
            "Principal": { "AWS": ::engine::emulator_account_id() },
            "Action": "es:ESHttp*",
            "Resource": domain_resource(domain),
        },
    ])
}

/// An explicit deny about every principal.
fn denies_everyone(domain: &str) -> Value {
    json!([{
        "Effect": "Deny",
        "Principal": "*",
        "Action": "es:ESHttp*",
        "Resource": domain_resource(domain),
    }])
}

/// The domains the table's rows are read on, one per access policy under test.
const DOMAINS: &[Domain] = &[
    Domain {
        name: "evidence-open",
        statements: None,
    },
    Domain {
        name: "evidence-elsewhere",
        statements: Some(allows_someone_else),
    },
    Domain {
        name: "evidence-reader-allowed",
        statements: Some(allows_the_reader),
    },
    Domain {
        name: "evidence-account-denied",
        statements: Some(denies_the_account),
    },
    Domain {
        name: "evidence-all-denied",
        statements: Some(denies_everyone),
    },
];

/// One row: who signs, which domain it reads, what it asks for, and what the domain must
/// answer.
struct Row {
    case: &'static str,
    /// The access key id the request is signed with.
    access_key_id: &'static str,
    domain: &'static str,
    method: &'static str,
    path: &'static str,
    /// The status the request must be answered with.
    status: u16,
    /// The clause the refusal ends with, for a row IAM refuses; `None` for one it serves.
    clause: Option<&'static str>,
    /// The `es:ESHttp*` action the method asks for, which a refusal's sentence names.
    action: &'static str,
}

/// The table: the account's own credential against every access policy, and a session whose
/// role IAM holds against the two the access policy decides, over a cluster read and a
/// document write.
const ROWS: &[Row] = &[
    Row {
        case: "the account's own credential on a domain carrying no access policy",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-open",
        method: "GET",
        path: "/_cluster/health",
        status: 200,
        clause: None,
        action: "es:ESHttpGet",
    },
    Row {
        case: "the account's own credential writing to a domain carrying no access policy",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-open",
        method: "PUT",
        path: "/probe/_doc/1",
        status: 201,
        clause: None,
        action: "es:ESHttpPut",
    },
    Row {
        case: "the account's own credential on a policy that allows only another role",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-elsewhere",
        method: "GET",
        path: "/_cluster/health",
        status: 200,
        clause: None,
        action: "es:ESHttpGet",
    },
    Row {
        case: "the account's own credential writing where the policy allows only another role",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-elsewhere",
        method: "PUT",
        path: "/probe/_doc/1",
        status: 201,
        clause: None,
        action: "es:ESHttpPut",
    },
    Row {
        case: "the account's own credential where the policy explicitly denies its account",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-account-denied",
        method: "GET",
        path: "/_cluster/health",
        status: 403,
        clause: Some(EXPLICIT_DENY),
        action: "es:ESHttpGet",
    },
    Row {
        case: "the account's own credential writing where the policy explicitly denies everyone",
        access_key_id: DEFAULT_ACCESS_KEY_ID,
        domain: "evidence-all-denied",
        method: "PUT",
        path: "/probe/_doc/1",
        status: 403,
        clause: Some(EXPLICIT_DENY),
        action: "es:ESHttpPut",
    },
    Row {
        case: "a session whose role IAM holds, where the policy allows only another role",
        access_key_id: READER_KEY,
        domain: "evidence-elsewhere",
        method: "GET",
        path: "/_cluster/health",
        status: 403,
        clause: None,
        action: "es:ESHttpGet",
    },
    Row {
        case: "a session whose role IAM holds, where the policy allows that role",
        access_key_id: READER_KEY,
        domain: "evidence-reader-allowed",
        method: "GET",
        path: "/_cluster/health",
        status: 200,
        clause: None,
        action: "es:ESHttpGet",
    },
];

/// Create one domain of the table, with the access policy its row states.
async fn given_domain(emulator: &Emulator, domain: &Domain) {
    let mut body = json!({
        "DomainName": domain.name,
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    });
    if let Some(statements) = domain.statements {
        body["AccessPolicies"] = Value::String(
            json!({ "Version": "2012-10-17", "Statement": statements(domain.name) }).to_string(),
        );
    }
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateDomain must succeed, got {} with {:?}",
        domain.name, created.status, created.body
    );
}

/// The body a document write sends.
fn document() -> Value {
    json!({ "title": "the evidence probe" })
}

/// RULE: the two sides of IAM decide a data-plane request together, so the account's own
/// published credential — which no identity policy here bounds — is refused only by a policy
/// that explicitly denies it, and a principal whose identity policies this emulator reads is
/// decided by them beside the domain's access policy.
#[tokio::test]
async fn the_accounts_own_credential_is_refused_only_by_an_explicit_deny() {
    let emulator = Emulator::new();
    given_session(&emulator, READER_ROLE, SESSION, READER_KEY);
    for domain in DOMAINS {
        given_domain(&emulator, domain).await;
    }
    // A domain publishes no address until the cluster behind it is up, and the address is
    // what every row's request is sent to: the span is waited out ONCE for all of them.
    tokio::time::sleep(SETTLE_WAIT).await;

    for row in ROWS {
        let case = row.case;
        let address = published_endpoint(&emulator, row.domain, case).await;
        let body = (row.method == "PUT").then(document);
        let answer: Answer = at_address(
            &emulator,
            &address,
            row.access_key_id,
            row.method,
            row.path,
            body,
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected {} from {} {}, got {} with {:?}",
            row.status, row.method, row.path, answer.status, answer.body
        );
        if row.status != 403 {
            continue;
        }
        let message = refusal_message(&answer);
        let clause = row
            .clause
            .map(ToString::to_string)
            .unwrap_or_else(|| nothing_allows(row.action));
        // Whoever signed, the sentence is IAM's own: the principal, the action the method
        // asks for, and which of IAM's rules refused. The principal is the one the control
        // plane names for the credential that signed — the session an assumed-role key speaks
        // for, and the identity STS reports for a key no IAM user of this account holds.
        let principal = if row.access_key_id == READER_KEY {
            session_arn(READER_ROLE, SESSION)
        } else {
            sts::ambient_identity().arn
        };
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {} {clause}",
                row.action
            ),
            "case {case:?}: expected IAM's own refusal sentence, got {message:?}"
        );
    }
}
