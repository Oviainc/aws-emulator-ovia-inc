// SPDX-License-Identifier: CC0-1.0

//! The Lightsail domain recordset — create and read — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateDomain` persists ONE domain recordset per domain
//! name the request names — the name and tags verbatim, every server-assigned
//! member (ARN, support code, creation time, location, resource type) computed
//! from the request plus the store's own allocators — so a later `GetDomain`
//! reports exactly that record; a name that is not a DNS domain name, or one the
//! account already holds, is refused and stores nothing; and a name that was
//! never created is refused with the service's `DoesNotExist` envelope.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The ARN region token of a domain recordset, from the modelled `Domain$arn`
/// documentation (`arn:aws:lightsail:global:123456789101:Domain/824cede0-…`).
const ARN_REGION: &str = "global";

/// The region a recordset reports in its location: the modelled `RegionName`
/// enum has no `global` member, and the modelled `InvalidInputException` says the
/// domain APIs are available only in us-east-1.
const DOMAIN_REGION: &str = "us-east-1";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// The stored recordset for `name`, as `GetDomain` reports it.
    async fn domain(&self, name: &str) -> Value {
        self.ok("GetDomain", json!({"domainName": name})).await["domain"].clone()
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` has the shape of the UUID Lightsail's resource and operation
/// ids take: five hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// A domain name of exactly `total` characters, built from `label` repeated as
/// often as it fits plus one short trailing label.
///
/// Used to sit a case on a published bound (253 characters for the whole name,
/// 63 for one label — RFC 1035 §2.3.4) and one character past it.
fn name_of_length(total: usize) -> String {
    let mut name = String::new();
    while name.len() + 64 <= total {
        name.push_str(&"a".repeat(63));
        name.push('.');
    }
    let remaining = total - name.len();
    name.push_str(&"b".repeat(remaining));
    assert_eq!(
        name.len(),
        total,
        "case building a {total}-character name: got {} characters",
        name.len()
    );
    name
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateDomain` request the service must serve, and the tags it carries.
///
/// The rows differ along everything the rule generalises over: the number of
/// dot-separated labels, the characters in them (letters, digits, hyphens), the
/// length of one label and of the whole name — including the published bounds —
/// and the number of tags.
struct Row {
    case: &'static str,
    /// `None` where the name is computed (a bound case), in which case
    /// [`Row::domain_name`] builds it.
    domain_name: Option<&'static str>,
    /// The length the computed name must have.
    computed_length: usize,
    tags: &'static [(&'static str, &'static str)],
}

impl Row {
    fn domain_name(&self) -> String {
        match self.domain_name {
            Some(name) => name.to_string(),
            None => name_of_length(self.computed_length),
        }
    }
}

/// The table. The issue's own case is an ordinary two-label name, the first row.
const ROWS: &[Row] = &[
    Row {
        case: "two labels, no tags",
        domain_name: Some("example.com"),
        computed_length: 0,
        tags: &[],
    },
    Row {
        case: "four labels under a multi-part suffix, two tags",
        domain_name: Some("dns.zone.example.co.uk"),
        computed_length: 0,
        tags: &[("stack", "prod"), ("owner", "dns-team")],
    },
    Row {
        case: "digits and hyphens in every label, one tag",
        domain_name: Some("3rd-zone-7.my-example-9.net"),
        computed_length: 0,
        tags: &[("tier", "edge")],
    },
    Row {
        case: "a 63-character label, the longest RFC 1035 allows",
        domain_name: None,
        // 63 `a`s, a dot and a 7-character label.
        computed_length: 71,
        tags: &[],
    },
    Row {
        case: "a 253-character name, the longest the presentation form allows",
        domain_name: None,
        computed_length: 253,
        tags: &[],
    },
];

/// The `CreateDomain` body for `row`, with `name` as the domain name.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({"domainName": name});
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    body
}

/// RULE: every member `GetDomain` reports comes from the one record
/// `CreateDomain` stored — the caller's name and tags verbatim, and every
/// server-assigned member computed from them plus the store's allocators.
#[tokio::test]
async fn a_domain_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let name = row.domain_name();

        // The create answers ONE operation-log entry: a `Domain` carries no
        // status member, so the zone exists as the call returns and the entry is
        // terminal.
        let created = emulator
            .ok("CreateDomain", create_body(row, &name))
            .await;
        let operation = created["operation"].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDomain"),
            "case {case}: expected the CreateDomain operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Domain"),
            "case {case}: expected the Domain resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the operation to name the domain, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected a Succeeded operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected a terminal operation, got {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used \
             {operation_id:?}"
        );

        // The stored record, read back through the implemented reader.
        let domain = emulator.domain(&name).await;
        assert_eq!(
            domain["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the domain name the request carried, got {domain:?}"
        );
        assert_eq!(
            domain["resourceType"].as_str(),
            Some("Domain"),
            "case {case}: expected the Domain resource type, got {domain:?}"
        );
        assert_eq!(
            domain["location"]["regionName"].as_str(),
            Some(DOMAIN_REGION),
            "case {case}: expected the domain region, got {:?}",
            domain["location"]
        );
        assert!(
            domain["location"]["availabilityZone"].is_null(),
            "case {case}: a DNS zone is not zonal, so no availability zone may be \
             reported, got {:?}",
            domain["location"]
        );
        assert_eq!(
            domain["domainEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a new zone holds no recordsets, got {:?}",
            domain["domainEntries"]
        );
        assert!(
            domain["registeredDomainDelegationInfo"].is_null(),
            "case {case}: nothing delegated a Route 53 domain here, got {:?}",
            domain["registeredDomainDelegationInfo"]
        );

        // The tags the request carried, verbatim and in order.
        let reported_tags: Vec<(String, String)> = domain["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {domain:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, want_tags,
            "case {case}: expected the tags the request carried, got {:?}",
            domain["tags"]
        );

        // The ARN carries `global` and the canonical emulator account, and ends
        // in a uuid no other recordset is using; the support code names the same
        // id.
        let arn = domain["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{ARN_REGION}:{ACCOUNT}:Domain/");
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            domain["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            domain["supportCode"]
        );
        assert!(
            domain["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            domain["createdAt"]
        );

        // Reading changes nothing a later caller sees.
        let again = emulator.domain(&name).await;
        assert_eq!(
            again, domain,
            "case {case}: expected the second read to report the same record, got \
             {again:?}"
        );
    }
}

/// RULE: each created recordset is its own record, so creating one never
/// disturbs another and every one of them is still readable afterwards.
#[tokio::test]
async fn each_domain_is_stored_independently() {
    let emulator = Emulator::new();
    let mut names = Vec::new();
    for row in ROWS {
        let name = row.domain_name();
        emulator.ok("CreateDomain", create_body(row, &name)).await;
        names.push(name);
    }

    for (row, name) in ROWS.iter().zip(&names) {
        let domain = emulator.domain(name).await;
        assert_eq!(
            domain["name"].as_str(),
            Some(name.as_str()),
            "case {}: expected '{name}' to still read back, got {domain:?}",
            row.case
        );
        assert_eq!(
            domain["tags"].as_array().map(Vec::len),
            Some(row.tags.len()),
            "case {}: expected its own tags to be untouched, got {:?}",
            row.case,
            domain["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a `domainName` that is not a DNS domain name is refused with the
/// modelled `InvalidInputException` (400) and stores nothing.
#[tokio::test]
async fn a_name_that_is_not_a_domain_name_is_refused_and_stores_nothing() {
    // Each case is one way the name is not a domain name: too few labels, an
    // empty label (including the trailing dot of a fully qualified name), a
    // character DNS labels do not carry, a hyphen at either edge of a label, and
    // one character past each published length bound (RFC 1035 §2.3.4: 63 per
    // label, 253 for the whole presentation form).
    let cases: Vec<(&str, String)> = vec![
        ("one label only", "localhost".to_string()),
        ("an empty leading label", ".example.com".to_string()),
        ("an empty inner label", "example..com".to_string()),
        ("a trailing dot", "example.com.".to_string()),
        ("an underscore in a label", "my_zone.example.com".to_string()),
        ("a space in a label", "exa mple.com".to_string()),
        ("a leading hyphen", "-example.com".to_string()),
        ("a trailing hyphen", "example-.com".to_string()),
        ("a 64-character label", format!("{}.com", "a".repeat(64))),
        ("a 254-character name", name_of_length(254)),
        ("the empty name", String::new()),
    ];

    for (case, name) in cases {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call("CreateDomain", json!({"domainName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal for {name:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("domainName"),
            "case {case}: expected the refusal to name the member, got {body:?}"
        );

        let (read_status, read_body) = emulator
            .call("GetDomain", json!({"domainName": name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create stores nothing, so the read must be refused \
             too, got {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: a refused create stores nothing, so the read must answer code \
             DoesNotExist, got {read_body:?}"
        );
    }
}

/// RULE: a domain name addresses ONE recordset, so creating one the account
/// already holds is refused and leaves the stored zone exactly as it was.
#[tokio::test]
async fn an_existing_domain_cannot_be_created_again() {
    let emulator = Emulator::new();
    let name = "example.org";
    emulator
        .ok(
            "CreateDomain",
            json!({"domainName": name, "tags": [{"key": "keep", "value": "me"}]}),
        )
        .await;
    let before = emulator.domain(name).await;

    let (status, body) = emulator
        .call("CreateDomain", json!({"domainName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case creating an existing domain: expected a 400 refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case creating an existing domain: expected the modelled InvalidInputException, \
         got {body:?}"
    );

    let after = emulator.domain(name).await;
    assert_eq!(
        after, before,
        "case creating an existing domain: expected the stored zone to be untouched, got \
         {after:?}"
    );
}

/// RULE: a well-formed name that names no stored recordset is refused with the
/// service's `DoesNotExist` envelope — HTTP 400, `NotFoundException`, body code
/// `DoesNotExist` and a message naming the resource type and the requested name —
/// never a 501 and never a hollow 200.
#[tokio::test]
async fn an_unknown_domain_does_not_exist() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateDomain", json!({"domainName": "present.example"}))
        .await;

    for name in ["absent.example", "present.example.extra", "other.test"] {
        let (status, body) = emulator.call("GetDomain", json!({"domainName": name})).await;
        assert_eq!(
            status, 400,
            "case reading {name}: expected a 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case reading {name}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case reading {name}: expected code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Domain does not exist: {name}").as_str()),
            "case reading {name}: expected the resource-type/name message, got {body:?}"
        );
    }
}
