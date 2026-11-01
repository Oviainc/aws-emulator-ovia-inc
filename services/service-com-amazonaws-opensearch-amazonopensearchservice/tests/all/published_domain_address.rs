// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a domain publishes for its own data plane, and what that address serves.
//!
//! Three rules, each over a SET of inputs rather than one recorded call:
//!
//! * **every address a control-plane answer publishes for a domain's data plane is one this
//!   emulator SERVES from the endpoint the reading request reached.** `DomainStatus$Endpoint`
//!   is the "domain-specific endpoint used to submit index, search, and data upload
//!   requests" and `Endpoints`/`EndpointV2` are the same thing for a domain reached inside a
//!   VPC or over both address families — so a caller that reached this emulator at its own
//!   endpoint, which resolves none of AWS's hosts, is told that endpoint with the path this
//!   service's data plane serves the domain at, while a caller that addressed the service's
//!   AWS host is told the host AWS publishes. The rows span every access mode a domain
//!   publishes an address for and every reader that publishes one;
//! * **a domain's access policy decides each principal its statements NAME, and a statement
//!   naming an IAM role is about every session of that role** — because a resource policy is
//!   written with the identity a client created, which for a role is the role's own ARN. An
//!   explicit `Deny` wins over every allow, and the refusal is IAM's own sentence naming the
//!   principal, the action, the resource and which rule refused. The rows span the spellings
//!   a `Principal` element names a caller by and both effects;
//! * **a refresh reports the shards of the indexes it ran on**, computed from each index's
//!   own settings and the domain's node count, over a table of shard and replica counts.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service — so what is asserted is what a client sees. The control-plane
//! harness comes from [`super::domain_lifecycle`] and the created domain from
//! [`super::data_source_registry`]; the session and policy seeding below is parameterised by
//! role, which the sibling data-plane suite's own seeding (pinned to its one delivery role)
//! is not.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use super::domain_lifecycle::{Answer, Emulator, DEFAULT_ACCESS_KEY_ID, PREFIX, REGION, SETTLE_WAIT};

/// The endpoint a client on the machine this emulator runs on reaches it at: a loopback
/// address, which names no service, so it is the emulator's OWN endpoint.
const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";

/// The host AWS publishes this service's control plane at for the Region every request here
/// is signed for — a host that names a service, so a caller addressing it resolves AWS's own
/// hosts.
const SERVICE_HOST: &str = "es.us-east-1.amazonaws.com";

/// How long a seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// Drive one request with a chosen Host header and credential, so a suite can read the same
/// resource from the emulator's own endpoint and from the service's AWS host.
async fn dispatch(
    emulator: &Emulator,
    host: &str,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// `CreateDomain`, addressed at `host`.
async fn create_at(emulator: &Emulator, host: &str, body: Value) -> Answer {
    dispatch(
        emulator,
        host,
        DEFAULT_ACCESS_KEY_ID,
        "POST",
        &format!("{PREFIX}/opensearch/domain"),
        Some(body),
    )
    .await
}

/// `DescribeDomain`, addressed at `host`.
async fn describe_at(emulator: &Emulator, host: &str, name: &str) -> Answer {
    dispatch(
        emulator,
        host,
        DEFAULT_ACCESS_KEY_ID,
        "GET",
        &format!("{PREFIX}/opensearch/domain/{name}"),
        None,
    )
    .await
}

/// `DescribeDomains`, addressed at `host`.
async fn describe_batch_at(emulator: &Emulator, host: &str, name: &str) -> Answer {
    dispatch(
        emulator,
        host,
        DEFAULT_ACCESS_KEY_ID,
        "POST",
        &format!("{PREFIX}/opensearch/domain-info"),
        Some(json!({ "DomainNames": [name] })),
    )
    .await
}

/// The public address the control plane publishes for a domain that is UP, read through
/// `DescribeDomain` at the emulator's own endpoint.
///
/// A domain is still being provisioned when its create answers and publishes no address at
/// all then, so every suite below reads the address it uses here — after waiting the
/// provisioning span out — rather than off the create's own answer.
async fn published_endpoint(emulator: &Emulator, domain: &str, case: &str) -> String {
    let read = describe_at(emulator, LOCAL_ENDPOINT, domain).await;
    read.body["DomainStatus"]["Endpoint"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a published Endpoint for {domain:?} once it is up, got \
                 {:?}",
                read.body
            )
        })
        .to_string()
}

/// Every address a `DomainStatus` publishes for the domain's data plane, as
/// `(which member published it, the address)` in member order: the public endpoint, its
/// dual-stack twin, and each host of the VPC-access map.
fn published_addresses(status: &Value, case: &str) -> Vec<(String, String)> {
    let mut addresses = Vec::new();
    for member in ["Endpoint", "EndpointV2"] {
        match &status[member] {
            Value::Null => {}
            Value::String(address) => addresses.push((member.to_string(), address.clone())),
            other => panic!("case {case:?}: expected {member} to be a string, got {other:?}"),
        }
    }
    if let Value::Object(map) = &status["Endpoints"] {
        for (key, host) in map {
            let host = host.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a host under Endpoints[{key:?}], got {host:?}")
            });
            addresses.push((format!("Endpoints.{key}"), host.to_string()));
        }
    }
    addresses.sort();
    addresses
}

/// The host and request URI a client reaches `path` of a domain at, from the ADDRESS the
/// control plane published: an address that carries a path (the emulator's own endpoint with
/// the data plane's reserved path) keeps it ahead of the request path, and a bare host is
/// addressed as it stands.
fn addressed(address: &str, path: &str) -> (String, String) {
    match address.split_once('/') {
        Some((host, prefix)) => (host.to_string(), format!("/{prefix}{path}")),
        None => (address.to_string(), path.to_string()),
    }
}

/// One data-plane request to the ADDRESS the control plane published for a domain.
async fn at_address(
    emulator: &Emulator,
    address: &str,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let (host, uri) = addressed(address, path);
    dispatch(emulator, &host, access_key_id, method, &uri, body).await
}

/// The `error.type` a refused data-plane answer carries.
fn error_type(answer: &Answer) -> String {
    answer.body["error"]["type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `error.reason` a refused data-plane answer carries — the sentence the refusal states.
fn error_reason(answer: &Answer) -> String {
    answer.body["error"]["reason"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One domain to publish an address for: how it is reached, and which members the rule says
/// publish one.
struct AccessRow {
    case: &'static str,
    domain: &'static str,
    /// The `VPCOptions` container the create carries, if any — a domain is VPC-access
    /// exactly when it holds one.
    vpc_options: Option<Value>,
    /// The `IPAddressType` the create asks for, if any: one of `ipv4` and `dualstack`
    /// (`smithy_query(query="shape",shape="IPAddressType")`), or neither.
    ip_address_type: Option<&'static str>,
    /// The members that must publish an address for this domain.
    members: &'static [&'static str],
}

/// The table: both access modes, over the `IPAddressType` values that decide whether a
/// domain has a dual-stack address beside its IPv4 one.
fn access_rows() -> Vec<AccessRow> {
    vec![
        AccessRow {
            case: "a public domain naming no IPAddressType",
            domain: "addr-public",
            vpc_options: None,
            ip_address_type: None,
            members: &["Endpoint"],
        },
        AccessRow {
            case: "a public dualstack domain",
            domain: "addr-public-dual",
            vpc_options: None,
            ip_address_type: Some("dualstack"),
            members: &["Endpoint", "EndpointV2"],
        },
        AccessRow {
            case: "a domain reached inside a VPC",
            domain: "addr-vpc",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            })),
            ip_address_type: Some("ipv4"),
            members: &["Endpoints.vpc"],
        },
        AccessRow {
            case: "a dualstack domain reached inside a VPC",
            domain: "addr-vpc-dual",
            vpc_options: Some(json!({
                "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            })),
            ip_address_type: Some("dualstack"),
            members: &["Endpoints.vpc", "Endpoints.vpcv2"],
        },
    ]
}

/// A `CreateDomain` body for one row.
fn create_body(row: &AccessRow) -> Value {
    let mut body = json!({
        "DomainName": row.domain,
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    });
    if let Some(vpc_options) = &row.vpc_options {
        body["VPCOptions"] = vpc_options.clone();
    }
    if let Some(ip_address_type) = row.ip_address_type {
        body["IPAddressType"] = json!(ip_address_type);
    }
    body
}

/// RULE: every address a control-plane answer publishes for a domain's data plane is one
/// this emulator serves THAT domain at, from the endpoint the reading request reached — the
/// emulator's own endpoint with this plane's reserved path for a caller that reached it
/// there, and the host AWS publishes for a caller that addressed the service's AWS host.
///
/// Each row is checked on every reader that publishes such an address (the create's own
/// answer, the single-domain read and the batch read), on every member it publishes one in,
/// and by USING it: the banner it answers names that domain's cluster, and a document
/// written through it reads back through it.
#[tokio::test]
async fn every_published_domain_address_is_served_from_the_endpoint_that_read_it() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id();

    for row in access_rows() {
        let case = row.case;
        let created = create_at(&emulator, LOCAL_ENDPOINT, create_body(&row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
        // A domain still being provisioned publishes no address at all: the endpoint it is
        // reached on appears only once the cluster behind it is up.
        assert!(
            published_addresses(&created.body["DomainStatus"], case).is_empty(),
            "case {case:?}: a domain still Creating publishes no address, got {:?}",
            created.body["DomainStatus"]
        );
        tokio::time::sleep(SETTLE_WAIT).await;

        let local = describe_at(&emulator, LOCAL_ENDPOINT, row.domain).await;
        assert_eq!(
            local.status, 200,
            "case {case:?}: expected 200 from DescribeDomain, got {} with {:?}",
            local.status, local.body
        );
        let addresses = published_addresses(&local.body["DomainStatus"], case);
        let members: Vec<String> = addresses.iter().map(|(member, _)| member.clone()).collect();
        assert_eq!(
            members, row.members,
            "case {case:?}: expected the members {:?} to publish an address, got {addresses:?}",
            row.members
        );

        // The address a local caller is told is the endpoint it is already talking to, with
        // the path this plane serves THIS domain at — not a host it resolves nowhere.
        let expected_local = format!("{LOCAL_ENDPOINT}/_aws/es/{}", row.domain);
        for (member, address) in &addresses {
            assert_eq!(
                address, &expected_local,
                "case {case:?}: expected {member} to publish {expected_local:?}, got {address:?}"
            );
        }

        // Every reader publishes the SAME address, and a second read publishes it again: a
        // caller that stored what one reader told it keeps an address that works.
        let again = describe_at(&emulator, LOCAL_ENDPOINT, row.domain).await;
        assert_eq!(
            published_addresses(&again.body["DomainStatus"], case),
            addresses,
            "case {case:?}: expected two reads to publish the same addresses"
        );
        let batch = describe_batch_at(&emulator, LOCAL_ENDPOINT, row.domain).await;
        assert_eq!(
            published_addresses(&batch.body["DomainStatusList"][0], case),
            addresses,
            "case {case:?}: expected the batch read to publish the same addresses"
        );

        // And every published address SERVES this domain: the banner it answers names this
        // domain's cluster, and a document written through it is readable back through it.
        for (member, address) in &addresses {
            let banner = at_address(
                &emulator,
                address,
                DEFAULT_ACCESS_KEY_ID,
                "GET",
                "/",
                None,
            )
            .await;
            assert_eq!(
                banner.status, 200,
                "case {case:?} ({member}): expected 200 from GET / at {address:?}, got {} with \
                 {:?}",
                banner.status, banner.body
            );
            assert_eq!(
                banner.body["cluster_name"].as_str(),
                Some(format!("{account}:{}", row.domain).as_str()),
                "case {case:?} ({member}): expected the address to serve this domain, got {:?}",
                banner.body
            );

            let written = at_address(
                &emulator,
                address,
                DEFAULT_ACCESS_KEY_ID,
                "PUT",
                "/catalog/_doc/1",
                Some(json!({ "title": member })),
            )
            .await;
            assert!(
                matches!(written.status, 200 | 201),
                "case {case:?} ({member}): expected the write to be taken, got {} with {:?}",
                written.status,
                written.body
            );
            let read = at_address(
                &emulator,
                address,
                DEFAULT_ACCESS_KEY_ID,
                "GET",
                "/catalog/_doc/1",
                None,
            )
            .await;
            assert_eq!(
                read.body["_source"]["title"].as_str(),
                Some(member.as_str()),
                "case {case:?} ({member}): expected the document to read back through the same \
                 address, got {} with {:?}",
                read.status,
                read.body
            );
        }

        // A caller that addressed the service's AWS host resolves AWS's own hosts, so it is
        // told the host AWS publishes: a bare host, never the emulator's endpoint.
        let aws = describe_at(&emulator, SERVICE_HOST, row.domain).await;
        let aws_addresses = published_addresses(&aws.body["DomainStatus"], case);
        let aws_members: Vec<String> = aws_addresses
            .iter()
            .map(|(member, _)| member.clone())
            .collect();
        assert_eq!(
            aws_members, row.members,
            "case {case:?}: expected the same members to publish an AWS host, got \
             {aws_addresses:?}"
        );
        for (member, host) in &aws_addresses {
            assert!(
                !host.contains('/')
                    && (host.ends_with(".amazonaws.com") || host.ends_with(".on.aws")),
                "case {case:?} ({member}): expected the host AWS publishes, got {host:?}"
            );
            assert_ne!(
                host, &expected_local,
                "case {case:?} ({member}): expected the AWS host to differ from the emulator's \
                 own endpoint"
            );
        }
    }
}

/// The ARN of the IAM role a seeded session speaks for.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", ::engine::emulator_account_id())
}

/// The ARN of the SESSION a seeded credential speaks for — the principal
/// `sts:GetCallerIdentity` reports for it, which is the name the cluster knows the caller by.
fn session_arn(role: &str, session: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role}/{session}",
        ::engine::emulator_account_id()
    )
}

/// Record the assumed-role session `access_key_id` speaks for, as `sts:AssumeRole` records
/// one.
fn given_session(emulator: &Emulator, role: &str, session: &str, access_key_id: &str) {
    let now = SystemTime::now();
    let arn = role_arn(role);
    let store = sts::service_state(&**emulator.engine()).expect("the STS store");
    store
        .lock()
        .insert(sts::Session {
            access_key_id: access_key_id.to_string(),
            created_at: now,
            expires_at: now + Duration::from_secs(SESSION_SECONDS.unsigned_abs().into()),
            duration_seconds: SESSION_SECONDS,
            identity: sts::Identity {
                user_id: sts::role_id(&arn),
                account: ::engine::emulator_account_id().to_string(),
                arn: session_arn(role, session),
            },
            kind: sts::SessionKind::AssumedRole,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            external_id: None,
            role_arn: Some(arn),
            revoked: false,
        })
        .expect("the session is recorded");
}

/// Give `role` an inline policy carrying `document` — the identity-based grant IAM holds for
/// the role a session speaks for.
fn given_role_policy(emulator: &Emulator, role: &str, document: Value) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    store.lock().role_inline_policies.insert(
        iam::name_key(role),
        BTreeMap::from([(
            iam::name_key("inline"),
            iam::InlinePolicy {
                policy_name: "inline".to_string(),
                policy_document: document.to_string(),
            },
        )]),
    );
}

/// The role whose data-plane requests the recorded policy ALLOWS.
const ALLOWED_ROLE: &str = "search-writer";

/// The access key id that role's seeded session carries.
const ALLOWED_KEY: &str = "ASIAEXAMPLEALLOWED01";

/// The role whose data-plane requests the recorded policy DENIES.
const DENIED_ROLE: &str = "search-denied";

/// The access key id that role's seeded session carries.
const DENIED_KEY: &str = "ASIAEXAMPLEDENIED001";

/// The session name both seeded credentials carry.
const SESSION: &str = "acl-session";

/// An identity-based policy allowing every data-plane action of this service — the grant
/// both roles carry, so what is left to decide is the domain's own access policy.
fn data_plane_grant() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "es:ESHttp*", "Resource": "*" }],
    })
}

/// The access policy the recorded deployment carries: the writer role allowed and the denied
/// role denied, both on every path of the one domain.
fn swapped_policy(domain: &str) -> String {
    let resource = format!(
        "arn:aws:es:{REGION}:{}:domain/{domain}/*",
        ::engine::emulator_account_id()
    );
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": { "AWS": role_arn(ALLOWED_ROLE) },
                "Action": ["es:ESHttpGet", "es:ESHttpHead", "es:ESHttpPost", "es:ESHttpPut"],
                "Resource": resource,
            },
            {
                "Effect": "Deny",
                "Principal": { "AWS": role_arn(DENIED_ROLE) },
                "Action": ["es:ESHttpGet", "es:ESHttpHead", "es:ESHttpPost", "es:ESHttpPut"],
                "Resource": resource,
            },
        ],
    })
    .to_string()
}

/// RULE: a domain's access policy decides each principal its statements name, and a
/// statement naming an IAM role is about every SESSION of that role — so a deployed policy
/// that allows one role and denies another is observable per principal on the domain's own
/// data plane, with an explicit `Deny` winning over every allow.
#[tokio::test]
async fn the_access_policy_decides_the_sessions_of_the_roles_it_names() {
    let emulator = Emulator::new();
    let domain = "acl-target";
    given_session(&emulator, ALLOWED_ROLE, SESSION, ALLOWED_KEY);
    given_session(&emulator, DENIED_ROLE, SESSION, DENIED_KEY);
    given_role_policy(&emulator, ALLOWED_ROLE, data_plane_grant());
    given_role_policy(&emulator, DENIED_ROLE, data_plane_grant());

    let created = create_at(
        &emulator,
        LOCAL_ENDPOINT,
        json!({
            "DomainName": domain,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            "AccessPolicies": swapped_policy(domain),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain with an access policy\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    let address = {
        tokio::time::sleep(SETTLE_WAIT).await;
        published_endpoint(&emulator, domain, "CreateDomain with an access policy").await
    };
    let domain_arn = created.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The ALLOWED role's session gets the engine's own verdicts: a create, a refresh, a read
    // and a search over what it wrote.
    let written = at_address(
        &emulator,
        &address,
        ALLOWED_KEY,
        "PUT",
        "/acl-index/_doc/1",
        Some(json!({ "title": "allowed" })),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"the allowed session's first write\": expected 201, got {} with {:?}",
        written.status, written.body
    );
    assert_eq!(
        written.body["result"].as_str(),
        Some("created"),
        "case \"the allowed session's first write\": expected result created, got {:?}",
        written.body
    );
    let refreshed = at_address(
        &emulator,
        &address,
        ALLOWED_KEY,
        "POST",
        "/acl-index/_refresh",
        None,
    )
    .await;
    assert_eq!(
        refreshed.status, 200,
        "case \"the allowed session's refresh\": expected 200, got {} with {:?}",
        refreshed.status, refreshed.body
    );
    let read = at_address(
        &emulator,
        &address,
        ALLOWED_KEY,
        "GET",
        "/acl-index/_doc/1",
        None,
    )
    .await;
    assert_eq!(
        read.body["_source"]["title"].as_str(),
        Some("allowed"),
        "case \"the allowed session's read\": expected the stored document, got {} with {:?}",
        read.status,
        read.body
    );
    let searched = at_address(
        &emulator,
        &address,
        ALLOWED_KEY,
        "POST",
        "/acl-index/_search",
        Some(json!({ "query": { "match_all": {} } })),
    )
    .await;
    assert_eq!(
        searched.body["hits"]["total"]["value"].as_i64(),
        Some(1),
        "case \"the allowed session's search\": expected one hit, got {} with {:?}",
        searched.status,
        searched.body
    );

    // The DENIED role's session is refused every one of them, with IAM's own sentence: the
    // policy named its role, and the explicit deny wins over the grant its identity carries.
    // (case, the method, the path)
    let refused_cases: &[(&str, &str, &str)] = &[
        ("a read of the document the policy denies", "GET", "/acl-index/_doc/1"),
        ("a write the policy denies", "PUT", "/acl-index/_doc/2"),
        ("a search the policy denies", "POST", "/acl-index/_search"),
        ("a refresh the policy denies", "POST", "/acl-index/_refresh"),
    ];
    for (case, method, path) in refused_cases {
        let answer = at_address(&emulator, &address, DENIED_KEY, method, path, None).await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403, got {} with {:?}",
            answer.status, answer.body
        );
        let reason = error_reason(&answer);
        let expected_prefix = format!(
            "User: {} is not authorized to perform: es:",
            session_arn(DENIED_ROLE, SESSION)
        );
        assert!(
            reason.starts_with(&expected_prefix),
            "case {case:?}: expected the refusal to name the signing session, got {reason:?}"
        );
        assert!(
            reason.contains(&format!(" on resource: {domain_arn}/acl-index"))
                && reason.ends_with(" with an explicit deny in a resource-based policy"),
            "case {case:?}: expected the resource and the explicit-deny clause, got {reason:?}"
        );
        assert!(
            !answer.body.to_string().contains("allowed"),
            "case {case:?}: expected the refusal to carry no document content, got {:?}",
            answer.body
        );
    }

    // And the refused write stored nothing: the index still holds only the allowed
    // session's document.
    let counted = at_address(
        &emulator,
        &address,
        ALLOWED_KEY,
        "GET",
        "/acl-index/_count",
        None,
    )
    .await;
    assert_eq!(
        counted.body["count"].as_i64(),
        Some(1),
        "case \"after the refused write\": expected the index to hold one document, got {} with \
         {:?}",
        counted.status,
        counted.body
    );
}

/// One access policy to decide ONE principal against: how its statements name a caller, and
/// what the domain must answer that caller's read.
struct PolicyRow {
    case: &'static str,
    domain: &'static str,
    /// The `Statement` element of the domain's access policy, with `@PRINCIPAL@` standing
    /// for the role ARN the reading session speaks for, `@SESSION@` for the session's own
    /// ARN, `@ACCOUNT@` for the account id and `@DOMAIN@` for the domain's ARN.
    statements: &'static str,
    /// The status the read is answered with.
    status: u16,
    /// The clause the refusal ends with, for a row the policy refuses.
    clause: Option<&'static str>,
}

/// The table: every spelling a `Principal` element names a caller by, both effects, and a
/// statement about another principal or another resource — read by a session whose role
/// carries NO identity policy, so each row is decided by the access policy alone.
const POLICY_ROWS: &[PolicyRow] = &[
    PolicyRow {
        case: "an allow naming the role the session speaks for",
        domain: "acl-row-role",
        statements: r#"{"Effect":"Allow","Principal":{"AWS":"@PRINCIPAL@"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 200,
        clause: None,
    },
    PolicyRow {
        case: "an allow naming the session's own ARN",
        domain: "acl-row-session",
        statements: r#"{"Effect":"Allow","Principal":{"AWS":"@SESSION@"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 200,
        clause: None,
    },
    PolicyRow {
        case: "an allow naming the account root",
        domain: "acl-row-root",
        statements: r#"{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::@ACCOUNT@:root"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 200,
        clause: None,
    },
    PolicyRow {
        case: "an allow naming the bare account id",
        domain: "acl-row-account",
        statements: r#"{"Effect":"Allow","Principal":{"AWS":"@ACCOUNT@"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 200,
        clause: None,
    },
    PolicyRow {
        case: "an allow naming every principal",
        domain: "acl-row-any",
        statements: r#"{"Effect":"Allow","Principal":"*",
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 200,
        clause: None,
    },
    PolicyRow {
        case: "an allow naming another role only",
        domain: "acl-row-other",
        statements: r#"{"Effect":"Allow",
                        "Principal":{"AWS":"arn:aws:iam::@ACCOUNT@:role/someone-else"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 403,
        clause: Some("because no resource-based policy allows the es:ESHttpGet action"),
    },
    PolicyRow {
        case: "an allow for everyone with a deny naming the caller's role",
        domain: "acl-row-deny-role",
        statements: r#"{"Effect":"Allow","Principal":"*",
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"},
                       {"Effect":"Deny","Principal":{"AWS":"@PRINCIPAL@"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 403,
        clause: Some("with an explicit deny in a resource-based policy"),
    },
    PolicyRow {
        case: "an allow for everyone with a deny naming the caller's account",
        domain: "acl-row-deny-acct",
        statements: r#"{"Effect":"Allow","Principal":"*",
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"},
                       {"Effect":"Deny","Principal":{"AWS":"@ACCOUNT@"},
                        "Action":"es:ESHttp*","Resource":"@DOMAIN@/*"}"#,
        status: 403,
        clause: Some("with an explicit deny in a resource-based policy"),
    },
    PolicyRow {
        case: "an allow scoped to another domain's paths",
        domain: "acl-row-elsewhere",
        statements: r#"{"Effect":"Allow","Principal":"*","Action":"es:ESHttp*",
                        "Resource":"arn:aws:es:us-east-1:@ACCOUNT@:domain/somewhere-else/*"}"#,
        status: 403,
        clause: Some("because no resource-based policy allows the es:ESHttpGet action"),
    },
];

/// The role the policy table's reads are made as — one carrying NO identity policy, so each
/// row is decided by the domain's own access policy alone.
const READER_ROLE: &str = "search-reader";

/// The access key id that role's seeded session carries.
const READER_KEY: &str = "ASIAEXAMPLEREADER001";

/// RULE: the access policy is read as IAM reads a resource-based policy — a statement is
/// about a caller when its `Principal` element names any identity that caller IS, an
/// explicit `Deny` wins over every allow, and nothing the policy does not state about the
/// request grants it.
#[tokio::test]
async fn each_access_policy_statement_decides_the_callers_it_names() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id();
    given_session(&emulator, READER_ROLE, SESSION, READER_KEY);

    // Every row's domain is created first and the provisioning span waited out ONCE: a domain
    // publishes no address until the cluster behind it is up, and the address is what each
    // row's request is sent to.
    for row in POLICY_ROWS {
        let case = row.case;
        let domain_arn = format!("arn:aws:es:{REGION}:{account}:domain/{}", row.domain);
        let policy = format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}", row.statements)
            .replace("@PRINCIPAL@", &role_arn(READER_ROLE))
            .replace("@SESSION@", &session_arn(READER_ROLE, SESSION))
            .replace("@DOMAIN@", &domain_arn)
            .replace("@ACCOUNT@", account);
        let created = create_at(
            &emulator,
            LOCAL_ENDPOINT,
            json!({
                "DomainName": row.domain,
                "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
                "AccessPolicies": policy,
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for row in POLICY_ROWS {
        let case = row.case;
        let domain_arn = format!("arn:aws:es:{REGION}:{account}:domain/{}", row.domain);
        let address = published_endpoint(&emulator, row.domain, case).await;

        let answer = at_address(&emulator, &address, READER_KEY, "GET", "/", None).await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected {}, got {} with {:?}",
            row.status, answer.status, answer.body
        );
        match row.clause {
            None => assert_eq!(
                answer.body["cluster_name"].as_str(),
                Some(format!("{account}:{}", row.domain).as_str()),
                "case {case:?}: expected the cluster banner, got {:?}",
                answer.body
            ),
            Some(clause) => {
                assert_eq!(
                    error_type(&answer),
                    "security_exception",
                    "case {case:?}: expected the engine's security error type, got {:?}",
                    answer.body
                );
                let reason = error_reason(&answer);
                assert_eq!(
                    reason,
                    format!(
                        "User: {} is not authorized to perform: es:ESHttpGet on resource: \
                         {domain_arn}/ {clause}",
                        session_arn(READER_ROLE, SESSION)
                    ),
                    "case {case:?}: expected IAM's own sentence, got {reason:?}"
                );
            }
        }
    }
}

/// One refresh to run: the index settings it runs against, and the shards it reports.
struct RefreshRow {
    case: &'static str,
    index: &'static str,
    shards: i64,
    replicas: i64,
}

/// The table: a shard count and a replica count on each axis — the default single shard with
/// a replica, several shards with none, and several of both — so the summary is computed from
/// the settings rather than fixed.
const REFRESH_ROWS: &[RefreshRow] = &[
    RefreshRow {
        case: "one shard with one replica",
        index: "refresh-one",
        shards: 1,
        replicas: 1,
    },
    RefreshRow {
        case: "three shards with no replica",
        index: "refresh-three",
        shards: 3,
        replicas: 0,
    },
    RefreshRow {
        case: "two shards with two replicas",
        index: "refresh-two",
        shards: 2,
        replicas: 2,
    },
];

/// RULE: a refresh reports the shards of the indexes it ran on — every copy the index's own
/// settings asked for as `total`, and the ones a node of this domain can hold as
/// `successful` — for the one index its path names and for the whole cluster when it names
/// none.
#[tokio::test]
async fn a_refresh_reports_the_shards_of_the_indexes_it_ran_on() {
    let emulator = Emulator::new();
    let domain = "refresh-target";
    let created = create_at(
        &emulator,
        LOCAL_ENDPOINT,
        json!({
            "DomainName": domain,
            // One data node, so a replica copy has no second node to live on.
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain(refresh-target)\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let address = published_endpoint(&emulator, domain, "CreateDomain(refresh-target)").await;

    let mut total_everywhere = 0;
    let mut successful_everywhere = 0;
    for row in REFRESH_ROWS {
        let case = row.case;
        let made = at_address(
            &emulator,
            &address,
            DEFAULT_ACCESS_KEY_ID,
            "PUT",
            &format!("/{}", row.index),
            Some(json!({
                "settings": {
                    "number_of_shards": row.shards,
                    "number_of_replicas": row.replicas,
                },
            })),
        )
        .await;
        assert_eq!(
            made.status, 200,
            "case {case:?}: expected 200 from the index create, got {} with {:?}",
            made.status, made.body
        );

        // (the method a refresh is accepted under)
        for method in ["POST", "GET"] {
            let answer = at_address(
                &emulator,
                &address,
                DEFAULT_ACCESS_KEY_ID,
                method,
                &format!("/{}/_refresh", row.index),
                None,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({method}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["_shards"]["total"].as_i64(),
                Some(row.shards * (row.replicas + 1)),
                "case {case:?} ({method}): expected every copy the settings asked for, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["_shards"]["successful"].as_i64(),
                Some(row.shards),
                "case {case:?} ({method}): expected only the primaries on a single node, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["_shards"]["failed"].as_i64(),
                Some(0),
                "case {case:?} ({method}): expected no failed shard, got {:?}",
                answer.body
            );
        }
        total_everywhere += row.shards * (row.replicas + 1);
        successful_everywhere += row.shards;
    }

    // A refresh naming no index runs on every index the domain holds.
    let everywhere = at_address(
        &emulator,
        &address,
        DEFAULT_ACCESS_KEY_ID,
        "POST",
        "/_refresh",
        None,
    )
    .await;
    assert_eq!(
        everywhere.status, 200,
        "case \"the whole cluster\": expected 200, got {} with {:?}",
        everywhere.status, everywhere.body
    );
    assert_eq!(
        (
            everywhere.body["_shards"]["total"].as_i64(),
            everywhere.body["_shards"]["successful"].as_i64(),
        ),
        (Some(total_everywhere), Some(successful_everywhere)),
        "case \"the whole cluster\": expected the shards of every index it holds, got {:?}",
        everywhere.body
    );

    // An index the domain does not hold is not refreshed, and a method the route does not
    // take is not either.
    let missing = at_address(
        &emulator,
        &address,
        DEFAULT_ACCESS_KEY_ID,
        "POST",
        "/refresh-absent/_refresh",
        None,
    )
    .await;
    assert_eq!(
        (missing.status, error_type(&missing).as_str()),
        (404, "index_not_found_exception"),
        "case \"an index the domain does not hold\": expected the engine's not-found refusal, \
         got {} with {:?}",
        missing.status,
        missing.body
    );
    let wrong_method = at_address(
        &emulator,
        &address,
        DEFAULT_ACCESS_KEY_ID,
        "PUT",
        "/refresh-one/_refresh",
        None,
    )
    .await;
    assert_eq!(
        wrong_method.status, 405,
        "case \"a method the refresh route does not take\": expected 405, got {} with {:?}",
        wrong_method.status, wrong_method.body
    );
}
