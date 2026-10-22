// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the service-network + service + service-association slice.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router that mounts it — and reads the answer back out of a later
//! request, so nothing here can pass on a canned body: an id that is never stored
//! fails the read-back, and a response echoed from the request fails the ARN and
//! status assertions.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. A create mints an id of the resource's modelled pattern and an ARN built from
//!    the request's region and the emulator's account, persists the record, and
//!    answers from it; a Get by the bare id and by the ARN both return it.
//! 2. `ListServices` reports every persisted service exactly once, in one stable
//!    order, honouring `maxResults` and round-tripping `nextToken`.
//! 3. An association resolves both identifiers against persisted records, links them
//!    (so the service network's association count follows it), and refuses a second
//!    association of the same pair.
//! 4. A resource that something is still associated with cannot be deleted; a delete
//!    that succeeds removes the record, so a later Get is 404 and a List omits it.
//! 5. An identifier that names no persisted resource is `ResourceNotFoundException`
//!    (404); input outside a modelled constraint is `ValidationException` (400).
//! 6. A `clientToken` retry returns the first resource instead of a second one.
//! 7. A create answers with the non-terminal `CREATE_IN_PROGRESS`, and the resource
//!    reaches `ACTIVE` once the span the clock owes it has passed.
//! 8. A list page is bounded: with no `maxResults` it carries at most the maximum the
//!    model allows that member, and `nextToken` pages the rest; the cursor is opaque and
//!    survives a delete between pages.
//! 9. The stores are bounded: the idempotency-token store evicts the oldest token, and a
//!    create past the emulator's resource ceiling is the modelled
//!    `ServiceQuotaExceededException` (402).

use std::sync::Arc;

use service_com_amazonaws_vpclattice_mercurycontrolplane::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for — deliberately not the emulator's
/// default, so a region echoed into an ARN must have come from the request.
pub(crate) const REGION: &str = "us-west-2";

/// One emulator holding one shared engine, driven over HTTP. Shared with the other
/// suites of this test target, so every suite drives one harness.
pub(crate) struct Lattice {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the parsed body, and the wire error code.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
    pub(crate) error_code: Option<String>,
}

impl Answer {
    pub(crate) fn str_field(&self, member: &str) -> String {
        self.body[member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "expected a string member {member:?} in the response, got {}",
                    self.body
                )
            })
            .to_string()
    }
}

impl Lattice {
    pub(crate) fn new() -> Self {
        // The single shared engine the router builds in production, handed to every
        // operation of this service through its constructor.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("the test body serializes"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://vpc-lattice.{REGION}.amazonaws.com{path}"))
            .header("host", format!("vpc-lattice.{REGION}.amazonaws.com"))
            // The SigV4 credential scope is what routes the request to this service
            // and what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/vpc-lattice/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a valid test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "response body is not JSON ({e}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    pub(crate) async fn create_service_network(&self, body: serde_json::Value) -> Answer {
        self.call("POST", "/servicenetworks", Some(body)).await
    }

    pub(crate) async fn create_service(&self, body: serde_json::Value) -> Answer {
        self.call("POST", "/services", Some(body)).await
    }

    pub(crate) async fn get_service(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/services/{identifier}"), None)
            .await
    }

    pub(crate) async fn get_service_network(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/servicenetworks/{identifier}"), None)
            .await
    }

    pub(crate) async fn associate(&self, service: &str, network: &str) -> Answer {
        self.call(
            "POST",
            "/servicenetworkserviceassociations",
            Some(serde_json::json!({
                "serviceIdentifier": service,
                "serviceNetworkIdentifier": network,
            })),
        )
        .await
    }
}

/// An ARN used as an `@httpLabel`, encoded the way an AWS SDK encodes a non-greedy
/// label: the `/` before the resource id becomes `%2F` so it stays one path segment.
pub(crate) fn as_label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// The `message` member of a modelled exception body, which every error shape of this
/// service declares required. Shared by the suites of this test target, so the
/// extraction has one spelling.
pub(crate) fn message_of(case: &str, answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the exception must carry the required message member, got {}",
                answer.body
            )
        })
        .to_string()
}

/// A timestamp member is present and carries a real instant, in the wire form the model
/// declares for it: every timestamp member of this service targets
/// `com.amazonaws.vpclattice#Timestamp`, which carries `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="Timestamp")`), so the member is an RFC 3339
/// string and the restJson1 default a member without a declared format falls back to —
/// a JSON number of epoch seconds — is not the modelled type at all.
pub(crate) fn is_timestamp(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|text| {
        ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .is_ok()
    })
}

/// An id of `prefix` plus 17 `[0-9a-z]` characters — the shape every modelled id
/// pattern of this service demands.
pub(crate) fn is_lattice_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 17
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

/// The ARN the emulator must build for `kind`/`id` in the request's region.
pub(crate) fn expected_arn(kind: &str, id: &str) -> String {
    format!(
        "arn:aws:vpc-lattice:{REGION}:{}:{kind}/{id}",
        state::account_id()
    )
}

/// Every service id `ListServices` reports, paging with `max_results` until the
/// service stops handing back a `nextToken`.
async fn list_every_service_id(lattice: &Lattice, max_results: Option<u32>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One page per persisted service is the worst case; the bound stops a broken
    // token round-trip from looping forever instead of failing.
    for _ in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(next) = &token {
            query.push(format!("nextToken={next}"));
        }
        let path = if query.is_empty() {
            "/services".to_string()
        } else {
            format!("/services?{}", query.join("&"))
        };
        let answer = lattice.call("GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={max_results:?} token={token:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let items = answer.body["items"].as_array().cloned().unwrap_or_default();
        if let Some(max) = max_results {
            assert!(
                items.len() <= max as usize,
                "case maxResults={max}: a page carried {} items, more than maxResults",
                items.len()
            );
        }
        for item in &items {
            ids.push(
                item["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a ServiceSummary without an id: {item}"))
                    .to_string(),
            );
        }
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!("ListServices never stopped paging: nextToken did not round-trip to a last page");
}

/// Rule 1 for service networks, over a table that varies name length, `authType`,
/// `sharingConfig` and tags.
#[tokio::test]
async fn creating_service_networks_persists_them_and_both_identifier_forms_read_them_back() {
    let lattice = Lattice::new();

    // (case, request body, expected authType in the answer)
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "shortest legal name, no optional member",
            serde_json::json!({ "name": "net" }),
            // The model documents NONE as the default auth type.
            "NONE",
        ),
        (
            "iam auth and sharing enabled",
            serde_json::json!({
                "name": "payments-net",
                "authType": "AWS_IAM",
                "sharingConfig": { "enabled": true },
                "tags": { "team": "payments" },
            }),
            "AWS_IAM",
        ),
        (
            "sharing explicitly disabled, hyphenated name",
            serde_json::json!({
                "name": "a-b-c",
                "authType": "NONE",
                "sharingConfig": { "enabled": false },
            }),
            "NONE",
        ),
        (
            "longest legal name (63 characters)",
            serde_json::json!({ "name": "n".repeat(63) }),
            "NONE",
        ),
    ];

    for (case, body, expected_auth) in cases {
        let requested_name = body["name"].as_str().expect("the case names a network");
        let created = lattice.create_service_network(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        assert!(
            is_lattice_id(&id, "sn-"),
            "case {case:?}: expected an id matching ^sn-[0-9a-z]{{17}}$, got {id:?}"
        );
        assert_eq!(
            created.str_field("arn"),
            expected_arn("servicenetwork", &id),
            "case {case:?}: the ARN must name this service network in the request's region"
        );
        assert_eq!(
            created.str_field("name"),
            requested_name,
            "case {case:?}: the stored name must come back"
        );
        assert_eq!(
            created.str_field("authType"),
            expected_auth,
            "case {case:?}: expected authType {expected_auth:?}, got {}",
            created.body
        );
        // `sharingConfig` is a member the service fills in: a create that sends no
        // value for it stores `state::DEFAULT_SHARING_ENABLED`, and the member is
        // reported either way.
        let expected_sharing = Some(
            body["sharingConfig"]["enabled"]
                .as_bool()
                .unwrap_or(state::DEFAULT_SHARING_ENABLED),
        );
        assert_eq!(
            created.body["sharingConfig"]["enabled"].as_bool(),
            expected_sharing,
            "case {case:?}: expected sharingConfig.enabled {expected_sharing:?}, got {}",
            created.body
        );

        // Rule 1: both modelled identifier forms read the same persisted record.
        let arn = created.str_field("arn");
        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = lattice.get_service_network(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                id,
                "case {case:?} by {form}: read back a different service network: {}",
                read.body
            );
            assert_eq!(
                read.str_field("name"),
                requested_name,
                "case {case:?} by {form}: expected name {requested_name:?}, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("arn"),
                arn,
                "case {case:?} by {form}: expected arn {arn:?}, got {}",
                read.body
            );
            assert_eq!(
                read.body["numberOfAssociatedServices"].as_i64(),
                Some(0),
                "case {case:?} by {form}: a fresh service network has no associated services, got {}",
                read.body
            );
            assert!(
                is_timestamp(&read.body["createdAt"]),
                "case {case:?} by {form}: expected a createdAt timestamp, got {}",
                read.body
            );
        }
    }

    // A second service network under a taken name conflicts (a name is unique to the
    // account), and the first one is untouched.
    let duplicate = lattice
        .create_service_network(serde_json::json!({ "name": "payments-net" }))
        .await;
    assert_eq!(
        duplicate.status, 409,
        "case duplicate name: expected 409 ConflictException, got {} / {}",
        duplicate.status, duplicate.body
    );
}

/// Rule 1 for services, over a table that varies every optional member.
#[tokio::test]
async fn creating_services_persists_them_with_a_dns_entry_and_reads_back_identically() {
    let lattice = Lattice::new();

    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "name only",
            serde_json::json!({ "name": "orders" }),
        ),
        (
            "custom domain and iam auth",
            serde_json::json!({
                "name": "checkout-api",
                "customDomainName": "checkout.example.com",
                "authType": "AWS_IAM",
                "tags": { "tier": "edge" },
            }),
        ),
        (
            "lowest legal idleTimeoutSeconds (60)",
            serde_json::json!({ "name": "idle-low", "idleTimeoutSeconds": 60 }),
        ),
        (
            "highest legal idleTimeoutSeconds (600)",
            serde_json::json!({ "name": "idle-high", "idleTimeoutSeconds": 600 }),
        ),
        (
            "longest legal name (40 characters)",
            serde_json::json!({ "name": "s".repeat(40) }),
        ),
    ];

    for (case, body) in cases {
        let created = lattice.create_service(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        assert!(
            is_lattice_id(&id, "svc-"),
            "case {case:?}: expected an id matching ^svc-[0-9a-z]{{17}}$, got {id:?}"
        );
        let arn = created.str_field("arn");
        assert_eq!(
            arn,
            expected_arn("service", &id),
            "case {case:?}: the ARN must name this service in the request's region"
        );
        let domain = created.body["dnsEntry"]["domainName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            domain.contains("vpc-lattice-svcs") && domain.contains(REGION),
            "case {case:?}: expected a generated lattice domain name, got {domain:?}"
        );
        assert!(
            created.body["dnsEntry"]["hostedZoneId"]
                .as_str()
                .is_some_and(|zone| zone.starts_with('Z') && zone.len() == 20),
            "case {case:?}: expected a hosted zone id, got {}",
            created.body["dnsEntry"]
        );
        let status = created.str_field("status");
        assert_eq!(
            status, "CREATE_IN_PROGRESS",
            "case {case:?}: a just-created service is still creating, got {status:?}"
        );

        // Rule 1: GetService by id and by ARN returns exactly what was persisted.
        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = lattice.get_service(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                id,
                "case {case:?} by {form}: read back a different service: {}",
                read.body
            );
            assert_eq!(
                read.str_field("name"),
                body["name"].as_str().expect("the case names a service"),
                "case {case:?} by {form}: the stored name must come back, got {}",
                read.body
            );
            assert_eq!(
                read.body["dnsEntry"]["domainName"].as_str(),
                Some(domain.as_str()),
                "case {case:?} by {form}: the dnsEntry must be the stored one, got {}",
                read.body["dnsEntry"]
            );
            assert_eq!(
                read.body["customDomainName"].as_str(),
                body["customDomainName"].as_str(),
                "case {case:?} by {form}: expected customDomainName {:?}, got {}",
                body["customDomainName"],
                read.body
            );
            // The member's own documentation: "If you don't specify a value, the
            // default is 60 seconds."
            let expected_idle = body["idleTimeoutSeconds"]
                .as_i64()
                .unwrap_or(i64::from(state::DEFAULT_IDLE_TIMEOUT_SECONDS));
            assert_eq!(
                read.body["idleTimeoutSeconds"].as_i64(),
                Some(expected_idle),
                "case {case:?} by {form}: expected idleTimeoutSeconds {expected_idle}, got {}",
                read.body
            );
            assert_eq!(
                read.body["authType"].as_str(),
                Some(body["authType"].as_str().unwrap_or("NONE")),
                "case {case:?} by {form}: expected authType {:?}, got {}",
                body["authType"],
                read.body
            );
            assert!(
                is_timestamp(&read.body["createdAt"]) && is_timestamp(&read.body["lastUpdatedAt"]),
                "case {case:?} by {form}: expected createdAt and lastUpdatedAt, got {}",
                read.body
            );
        }
    }
}

/// Rule 2: every page size reports every persisted service exactly once, in one
/// stable order.
#[tokio::test]
async fn list_services_pages_through_every_persisted_service() {
    let lattice = Lattice::new();

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let answer = lattice
            .create_service(serde_json::json!({ "name": format!("paged-{index}") }))
            .await;
        assert_eq!(
            answer.status, 201,
            "case setup service {index}: expected 201, got {} / {}",
            answer.status, answer.body
        );
        created.push(answer.str_field("id"));
    }

    let unpaged = list_every_service_id(&lattice, None).await;
    assert_eq!(
        unpaged.len(),
        created.len(),
        "case maxResults absent: expected {} services, got {unpaged:?}",
        created.len()
    );
    let mut sorted_created = created.clone();
    sorted_created.sort();
    let mut sorted_listed = unpaged.clone();
    sorted_listed.sort();
    assert_eq!(
        sorted_listed, sorted_created,
        "case maxResults absent: the list must be exactly the persisted services"
    );

    // Rule 2 over the page sizes: 1 (a page per service), 2 (a ragged last page),
    // 5 (exactly one page) and 100 (the modelled maximum).
    for max_results in [1_u32, 2, 5, 100] {
        let paged = list_every_service_id(&lattice, Some(max_results)).await;
        assert_eq!(
            paged, unpaged,
            "case maxResults={max_results}: paging must yield the same services in the same order as one page, got {paged:?}"
        );
    }

    // A token that names nothing is refused rather than silently restarting the scan.
    let bogus = lattice
        .call("GET", "/services?nextToken=svc-00000000000000000", None)
        .await;
    assert_eq!(
        bogus.status, 400,
        "case unknown nextToken: expected 400 ValidationException, got {} / {}",
        bogus.status, bogus.body
    );

    // A newly created service appears, a deleted one does not.
    let late = lattice
        .create_service(serde_json::json!({ "name": "paged-late" }))
        .await;
    let late_id = late.str_field("id");
    let after_create = list_every_service_id(&lattice, Some(2)).await;
    assert!(
        after_create.contains(&late_id),
        "case list after create: {late_id} must be listed, got {after_create:?}"
    );
    let deleted = lattice
        .call("DELETE", &format!("/services/{late_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the late service: expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    let after_delete = list_every_service_id(&lattice, Some(2)).await;
    assert!(
        !after_delete.contains(&late_id),
        "case list after delete: {late_id} must be gone, got {after_delete:?}"
    );
}

/// Rules 3 and 4: the association links the two records, a duplicate is refused, and
/// neither side can be deleted while it exists.
#[tokio::test]
async fn associations_link_both_records_and_block_deletion_until_removed() {
    let lattice = Lattice::new();

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "linked-net" }))
        .await;
    let network_id = network.str_field("id");
    let network_arn = network.str_field("arn");

    // Two services, associated through the two different identifier forms, so the
    // resolution rule is covered on both.
    let mut associations: Vec<(String, String)> = Vec::new();
    for (case, name, use_arn) in [
        ("by bare ids", "linked-a", false),
        ("by ARNs", "linked-b", true),
    ] {
        let service = lattice
            .create_service(serde_json::json!({ "name": name }))
            .await;
        let service_id = service.str_field("id");
        let service_arn = service.str_field("arn");
        let (service_ref, network_ref) = if use_arn {
            (service_arn.as_str(), network_arn.as_str())
        } else {
            (service_id.as_str(), network_id.as_str())
        };
        let created = lattice.associate(service_ref, network_ref).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            created.status, created.body
        );
        let association_id = created.str_field("id");
        assert!(
            is_lattice_id(&association_id, "snsa-"),
            "case {case:?}: expected an id matching ^snsa-[0-9a-z]{{17}}$, got {association_id:?}"
        );
        assert_eq!(
            created.str_field("arn"),
            expected_arn("servicenetworkserviceassociation", &association_id),
            "case {case:?}: the association ARN must name it in the request's region"
        );
        assert_eq!(
            created.str_field("createdBy"),
            state::account_id(),
            "case {case:?}: createdBy is the caller's account, got {}",
            created.body
        );
        assert_eq!(
            created.str_field("status"),
            "CREATE_IN_PROGRESS",
            "case {case:?}: a just-created association is still creating, got {}",
            created.body
        );

        // The association reads back, carrying both sides it linked.
        let read = lattice
            .call(
                "GET",
                &format!("/servicenetworkserviceassociations/{association_id}"),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?} read back: expected 200, got {} / {}",
            read.status, read.body
        );
        assert_eq!(
            read.str_field("serviceId"),
            service_id,
            "case {case:?} read back: wrong serviceId in {}",
            read.body
        );
        assert_eq!(
            read.str_field("serviceNetworkId"),
            network_id,
            "case {case:?} read back: wrong serviceNetworkId in {}",
            read.body
        );
        assert_eq!(
            read.str_field("serviceName"),
            name,
            "case {case:?} read back: wrong serviceName in {}",
            read.body
        );

        // A second association of the same pair conflicts.
        let again = lattice.associate(service_ref, network_ref).await;
        assert_eq!(
            again.status, 409,
            "case {case:?} duplicate: expected 409 ConflictException, got {} / {}",
            again.status, again.body
        );

        associations.push((association_id, service_id));
    }

    // Rule 3: the service network's association count is derived from the links.
    let read_network = lattice.get_service_network(&network_id).await;
    assert_eq!(
        read_network.body["numberOfAssociatedServices"].as_i64(),
        Some(2),
        "case two associations: expected numberOfAssociatedServices 2, got {}",
        read_network.body
    );

    // Rule 4: neither side can be deleted while the association exists.
    let blocked_network = lattice
        .call("DELETE", &format!("/servicenetworks/{network_id}"), None)
        .await;
    assert_eq!(
        blocked_network.status, 409,
        "case delete an associated service network: expected 409, got {} / {}",
        blocked_network.status, blocked_network.body
    );
    let (_, first_service_id) = &associations[0];
    let blocked_service = lattice
        .call("DELETE", &format!("/services/{first_service_id}"), None)
        .await;
    assert_eq!(
        blocked_service.status, 409,
        "case delete an associated service: expected 409, got {} / {}",
        blocked_service.status, blocked_service.body
    );

    // Removing the associations unblocks both deletes, and the removed association is
    // gone for good.
    for (association_id, _) in &associations {
        let removed = lattice
            .call(
                "DELETE",
                &format!("/servicenetworkserviceassociations/{association_id}"),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case delete association {association_id}: expected 200, got {} / {}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.str_field("id"),
            *association_id,
            "case delete association {association_id}: the answer must name it, got {}",
            removed.body
        );
        let gone = lattice
            .call(
                "GET",
                &format!("/servicenetworkserviceassociations/{association_id}"),
                None,
            )
            .await;
        assert_eq!(
            gone.status, 404,
            "case read a deleted association: expected 404, got {} / {}",
            gone.status, gone.body
        );
    }

    let unlinked = lattice.get_service_network(&network_id).await;
    assert_eq!(
        unlinked.body["numberOfAssociatedServices"].as_i64(),
        Some(0),
        "case associations removed: expected numberOfAssociatedServices 0, got {}",
        unlinked.body
    );

    for (_, service_id) in &associations {
        let deleted = lattice
            .call("DELETE", &format!("/services/{service_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case delete {service_id}: expected 200, got {} / {}",
            deleted.status, deleted.body
        );
        let gone = lattice.get_service(service_id).await;
        assert_eq!(
            gone.status, 404,
            "case read a deleted service: expected 404, got {} / {}",
            gone.status, gone.body
        );
    }

    let deleted_network = lattice
        .call("DELETE", &format!("/servicenetworks/{network_id}"), None)
        .await;
    assert_eq!(
        deleted_network.status, 204,
        "case delete an unassociated service network: expected 204, got {} / {}",
        deleted_network.status, deleted_network.body
    );
    let gone_network = lattice.get_service_network(&network_id).await;
    assert_eq!(
        gone_network.status, 404,
        "case read a deleted service network: expected 404, got {} / {}",
        gone_network.status, gone_network.body
    );
}

/// Rule 2's bound: a list call carries at most the maximum the model allows
/// `maxResults`, even when the caller sends none, and `nextToken` still reaches
/// everything.
#[tokio::test]
async fn an_unpaged_list_is_capped_at_the_modelled_maximum_page_size() {
    let lattice = Lattice::new();

    let total = state::MAX_PAGE_SIZE + 1;
    for index in 0..total {
        let answer = lattice
            .create_service(serde_json::json!({ "name": format!("capped-{index}") }))
            .await;
        assert_eq!(
            answer.status, 201,
            "case setup service {index}: expected 201, got {} / {}",
            answer.status, answer.body
        );
    }

    let first_page = lattice.call("GET", "/services", None).await;
    assert_eq!(
        first_page.status, 200,
        "case no maxResults: expected 200, got {} / {}",
        first_page.status, first_page.body
    );
    let items = first_page.body["items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        items,
        state::MAX_PAGE_SIZE,
        "case no maxResults: expected a page of at most {} services, got {items}",
        state::MAX_PAGE_SIZE
    );
    assert!(
        first_page.body["nextToken"].is_string(),
        "case no maxResults: the remaining services need a nextToken, got {}",
        first_page.body
    );

    let every = list_every_service_id(&lattice, None).await;
    assert_eq!(
        every.len(),
        total,
        "case paging an unpaged list: expected {total} services across the pages, got {}",
        every.len()
    );
}

/// The idempotency-token store is bounded: it remembers at most its cap, forgetting the
/// oldest token rather than growing without limit.
#[test]
fn the_idempotency_token_store_is_bounded_and_evicts_the_oldest_token() {
    let mut tokens = state::IdempotencyTokens::default();
    assert!(
        tokens.is_empty(),
        "case a fresh store: expected it empty, got {} entries",
        tokens.len()
    );

    // Record one more than any cap the store could have, then check it stopped growing
    // and that the newest token is the one still remembered.
    let recorded = 4096;
    for index in 0..recorded {
        tokens.record(format!("CreateService/token-{index}"), format!("svc-{index}"));
    }
    assert!(
        tokens.len() < recorded,
        "case {recorded} recorded tokens: the store must be bounded, got {} entries",
        tokens.len()
    );
    assert_eq!(
        tokens.get(&format!("CreateService/token-{}", recorded - 1)),
        Some(&format!("svc-{}", recorded - 1)),
        "case the newest token: it must still be remembered"
    );
    assert_eq!(
        tokens.get("CreateService/token-0"),
        None,
        "case the oldest token: it must have been evicted, not kept"
    );

    // Re-recording a remembered key updates it in place rather than adding an entry.
    let before = tokens.len();
    let newest = format!("CreateService/token-{}", recorded - 1);
    tokens.record(newest.clone(), "svc-replaced".to_string());
    assert_eq!(
        tokens.len(),
        before,
        "case re-recording a known key: the entry count must not change, got {}",
        tokens.len()
    );
    assert_eq!(
        tokens.get(&newest),
        Some(&"svc-replaced".to_string()),
        "case re-recording a known key: the new value must win"
    );
}

/// Rule 2's cursor contract: a token this service minted keeps working when the service
/// the page ended on is deleted before the next page, and a token it never minted is
/// refused.
#[tokio::test]
async fn a_page_cursor_survives_a_delete_between_pages() {
    let lattice = Lattice::new();

    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        let answer = lattice
            .create_service(serde_json::json!({ "name": format!("cursor-{index}") }))
            .await;
        assert_eq!(
            answer.status, 201,
            "case setup service {index}: expected 201, got {} / {}",
            answer.status, answer.body
        );
        ids.push(answer.str_field("id"));
    }

    let first_page = lattice.call("GET", "/services?maxResults=1", None).await;
    let first_id = first_page.body["items"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        first_id, ids[0],
        "case first page: creation order must come first, got {}",
        first_page.body
    );
    let token = first_page.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: expected a nextToken, got {}", first_page.body))
        .to_string();
    assert_ne!(
        token, ids[0],
        "case first page: the cursor must be opaque, not a bare resource id"
    );

    // Delete the service the first page ended on, then resume: the cursor names the
    // ORDER the page reached, so the next page is the next surviving service.
    let deleted = lattice
        .call("DELETE", &format!("/services/{}", ids[0]), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the service the page ended on: expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    let resumed = lattice
        .call("GET", &format!("/services?maxResults=1&nextToken={token}"), None)
        .await;
    assert_eq!(
        resumed.status, 200,
        "case resume after that delete: expected 200, got {} / {}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed.body["items"][0]["id"].as_str(),
        Some(ids[1].as_str()),
        "case resume after that delete: expected the next surviving service, got {}",
        resumed.body
    );

    // A token this service never minted is still a 400.
    for (case, token) in [
        ("not hex at all", "svc-00000000000000000".to_string()),
        ("hex of text no cursor uses", hex::encode("before:7")),
    ] {
        let refused = lattice
            .call("GET", &format!("/services?nextToken={token}"), None)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 ValidationException, got {} / {}",
            refused.status, refused.body
        );
    }
}

/// The resource stores are bounded: a create that would pass the emulator's ceiling is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402) rather than
/// growing the process, and the store is left at the ceiling.
#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused_with_the_modelled_quota_error() {
    let lattice = Lattice::new();

    let mut first_id = String::new();
    for index in 0..state::MAX_SERVICE_NETWORKS {
        let answer = lattice
            .create_service_network(serde_json::json!({ "name": format!("quota-{index}") }))
            .await;
        assert_eq!(
            answer.status, 201,
            "case service network {index} of the ceiling: expected 201, got {} / {}",
            answer.status, answer.body
        );
        if index == 0 {
            first_id = answer.str_field("id");
        }
    }

    let past_ceiling = lattice
        .create_service_network(serde_json::json!({ "name": "quota-one-too-many" }))
        .await;
    assert_eq!(
        past_ceiling.status, 402,
        "case one past the ceiling: expected 402, got {} / {}",
        past_ceiling.status, past_ceiling.body
    );
    assert_eq!(
        past_ceiling.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case one past the ceiling: expected the modelled quota error, got {:?} / {}",
        past_ceiling.error_code,
        past_ceiling.body
    );

    // The ceiling bounds what is STORED, not how many creates have been served: freeing
    // one slot lets the next create through, and the refused create stored nothing.
    let removed = lattice
        .call("DELETE", &format!("/servicenetworks/{first_id}"), None)
        .await;
    assert_eq!(
        removed.status, 204,
        "case free one slot: expected 204, got {} / {}",
        removed.status, removed.body
    );
    let accepted = lattice
        .create_service_network(serde_json::json!({ "name": "quota-one-too-many" }))
        .await;
    assert_eq!(
        accepted.status, 201,
        "case create into the freed slot: expected 201, got {} / {}",
        accepted.status, accepted.body
    );
    let read_back = lattice.get_service_network(&accepted.str_field("id")).await;
    assert_eq!(
        read_back.str_field("name"),
        "quota-one-too-many",
        "case create into the freed slot: it must be readable, got {}",
        read_back.body
    );
}

/// Rule 5: an unknown identifier is a 404 `ResourceNotFoundException`, whichever
/// modelled form it takes and whichever operation reads it.
#[tokio::test]
async fn unknown_identifiers_answer_resource_not_found() {
    let lattice = Lattice::new();
    let account = state::account_id();

    let cases: Vec<(&str, &str, String)> = vec![
        (
            "GET a service by an unknown id",
            "GET",
            "/services/svc-00000000000000000".to_string(),
        ),
        (
            "GET a service by an unknown ARN",
            "GET",
            format!(
                "/services/{}",
                as_label(&format!(
                    "arn:aws:vpc-lattice:{REGION}:{account}:service/svc-00000000000000000"
                ))
            ),
        ),
        (
            "DELETE a service by an unknown id",
            "DELETE",
            "/services/svc-zzzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "GET a service network by an unknown id",
            "GET",
            "/servicenetworks/sn-00000000000000000".to_string(),
        ),
        (
            "DELETE a service network by an unknown ARN",
            "DELETE",
            format!(
                "/servicenetworks/{}",
                as_label(&format!(
                    "arn:aws:vpc-lattice:{REGION}:{account}:servicenetwork/sn-00000000000000000"
                ))
            ),
        ),
        (
            "GET an association by an unknown id",
            "GET",
            "/servicenetworkserviceassociations/snsa-00000000000000000".to_string(),
        ),
        (
            "DELETE an association by an unknown id",
            "DELETE",
            "/servicenetworkserviceassociations/snsa-00000000000000000".to_string(),
        ),
    ];

    for (case, method, path) in cases {
        let answer = lattice.call(method, &path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }

    // An association that names a service network that is not there is the same 404,
    // and nothing is persisted for it.
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "half-there" }))
        .await;
    let service = lattice
        .create_service(serde_json::json!({ "name": "half-there-svc" }))
        .await;
    for (case, service_ref, network_ref) in [
        (
            "associate an unknown service",
            "svc-00000000000000000".to_string(),
            network.str_field("id"),
        ),
        (
            "associate an unknown service network",
            service.str_field("id"),
            "sn-00000000000000000".to_string(),
        ),
    ] {
        let answer = lattice.associate(&service_ref, &network_ref).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
    }
    let still_empty = lattice.get_service_network(&network.str_field("id")).await;
    assert_eq!(
        still_empty.body["numberOfAssociatedServices"].as_i64(),
        Some(0),
        "case a refused association: nothing may be linked, got {}",
        still_empty.body
    );
}

/// Rule 5: input outside a modelled constraint is a 400 `ValidationException` and
/// nothing is stored for it. The rows are each published constraint of the two names,
/// plus one value just past the `idleTimeoutSeconds` range at each end.
#[tokio::test]
async fn input_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let lattice = Lattice::new();

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "service network name shorter than 3",
            "/servicenetworks",
            serde_json::json!({ "name": "ab" }),
        ),
        (
            "service network name longer than 63",
            "/servicenetworks",
            serde_json::json!({ "name": "n".repeat(64) }),
        ),
        (
            "service network name starting with a hyphen",
            "/servicenetworks",
            serde_json::json!({ "name": "-net" }),
        ),
        (
            "service network name ending with a hyphen",
            "/servicenetworks",
            serde_json::json!({ "name": "net-" }),
        ),
        (
            "service network name with two adjacent hyphens",
            "/servicenetworks",
            serde_json::json!({ "name": "ne--t" }),
        ),
        (
            "service network name with an uppercase character",
            "/servicenetworks",
            serde_json::json!({ "name": "Net" }),
        ),
        (
            "service network authType outside the enum",
            "/servicenetworks",
            serde_json::json!({ "name": "enum-net", "authType": "IAM" }),
        ),
        (
            "service name longer than 40",
            "/services",
            serde_json::json!({ "name": "s".repeat(41) }),
        ),
        (
            "service name using the reserved svc- prefix",
            "/services",
            serde_json::json!({ "name": "svc-orders" }),
        ),
        (
            "idleTimeoutSeconds below the range",
            "/services",
            serde_json::json!({ "name": "idle-too-low", "idleTimeoutSeconds": 59 }),
        ),
        (
            "idleTimeoutSeconds above the range",
            "/services",
            serde_json::json!({ "name": "idle-too-high", "idleTimeoutSeconds": 601 }),
        ),
        (
            "customDomainName shorter than 3",
            "/services",
            serde_json::json!({ "name": "short-domain", "customDomainName": "ab" }),
        ),
        (
            "more tags than the modelled TagMap ceiling",
            "/services",
            serde_json::json!({
                "name": "too-many-tags",
                "tags": (0..=state::MAX_TAGS)
                    .map(|index| (format!("key-{index}"), serde_json::json!("v")))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            }),
        ),
        (
            "more tags than the modelled TagMap ceiling on a service network",
            "/servicenetworks",
            serde_json::json!({
                "name": "too-many-tags-net",
                "tags": (0..=state::MAX_TAGS)
                    .map(|index| (format!("key-{index}"), serde_json::json!("v")))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            }),
        ),
    ];

    for (case, path, body) in cases {
        let answer = lattice.call("POST", path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
    }

    // Nothing above was stored.
    let listed = list_every_service_id(&lattice, None).await;
    assert!(
        listed.is_empty(),
        "case refused creates: no service may be persisted, got {listed:?}"
    );
}

/// Rule 6: a `clientToken` retry answers with the first resource rather than creating
/// a second one.
#[tokio::test]
async fn repeating_a_client_token_returns_the_first_resource() {
    let lattice = Lattice::new();

    let first = lattice
        .create_service(serde_json::json!({ "name": "idem-svc", "clientToken": "token-one" }))
        .await;
    assert_eq!(
        first.status, 201,
        "case first create: expected 201, got {} / {}",
        first.status, first.body
    );
    let retry = lattice
        .create_service(serde_json::json!({ "name": "idem-svc", "clientToken": "token-one" }))
        .await;
    assert_eq!(
        retry.status, 201,
        "case retry with the same token: expected 201, got {} / {}",
        retry.status, retry.body
    );
    assert_eq!(
        retry.str_field("id"),
        first.str_field("id"),
        "case retry with the same token: expected the first service's id, got {}",
        retry.body
    );
    let listed = list_every_service_id(&lattice, None).await;
    assert_eq!(
        listed.len(),
        1,
        "case retry with the same token: exactly one service may exist, got {listed:?}"
    );

    // A different token is a different resource.
    let other = lattice
        .create_service(serde_json::json!({ "name": "idem-other", "clientToken": "token-two" }))
        .await;
    assert_ne!(
        other.str_field("id"),
        first.str_field("id"),
        "case a different token: expected a second service, got {}",
        other.body
    );

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "idem-net", "clientToken": "net-token" }))
        .await;
    let network_retry = lattice
        .create_service_network(serde_json::json!({ "name": "idem-net", "clientToken": "net-token" }))
        .await;
    assert_eq!(
        network_retry.str_field("id"),
        network.str_field("id"),
        "case service network retry with the same token: expected the first id, got {}",
        network_retry.body
    );

    // "If the parameters aren't identical, the retry fails" — for each create that takes
    // a clientToken.
    let changed_name = lattice
        .create_service(serde_json::json!({ "name": "idem-changed", "clientToken": "token-one" }))
        .await;
    assert_eq!(
        changed_name.status, 409,
        "case a token reused for a different service name: expected 409, got {} / {}",
        changed_name.status, changed_name.body
    );
    let changed_network = lattice
        .create_service_network(
            serde_json::json!({ "name": "idem-net-other", "clientToken": "net-token" }),
        )
        .await;
    assert_eq!(
        changed_network.status, 409,
        "case a token reused for a different service network name: expected 409, got {} / {}",
        changed_network.status, changed_network.body
    );

    // The association create takes one too: the same token with a different pair is a
    // conflict, the same token with the same pair replays.
    let first_association = lattice
        .call(
            "POST",
            "/servicenetworkserviceassociations",
            Some(serde_json::json!({
                "serviceIdentifier": first.str_field("id"),
                "serviceNetworkIdentifier": network.str_field("id"),
                "clientToken": "assoc-token",
            })),
        )
        .await;
    assert_eq!(
        first_association.status, 200,
        "case the first association: expected 200, got {} / {}",
        first_association.status, first_association.body
    );
    let replayed_association = lattice
        .call(
            "POST",
            "/servicenetworkserviceassociations",
            Some(serde_json::json!({
                "serviceIdentifier": first.str_field("id"),
                "serviceNetworkIdentifier": network.str_field("id"),
                "clientToken": "assoc-token",
            })),
        )
        .await;
    assert_eq!(
        replayed_association.str_field("id"),
        first_association.str_field("id"),
        "case association retry with the same token and pair: expected the first id, got {}",
        replayed_association.body
    );
    let other_pair = lattice
        .call(
            "POST",
            "/servicenetworkserviceassociations",
            Some(serde_json::json!({
                "serviceIdentifier": other.str_field("id"),
                "serviceNetworkIdentifier": network.str_field("id"),
                "clientToken": "assoc-token",
            })),
        )
        .await;
    assert_eq!(
        other_pair.status, 409,
        "case a token reused for a different association pair: expected 409, got {} / {}",
        other_pair.status, other_pair.body
    );
}

/// Rule 7: the create answers with the non-terminal status, and the resource reaches
/// `ACTIVE` once the span the clock owes it has passed — without any caller asking for
/// the transition.
#[tokio::test]
async fn a_created_resource_reaches_active_after_the_transition_the_clock_owes_it() {
    let lattice = Lattice::new();

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "clock-net" }))
        .await;
    let service = lattice
        .create_service(serde_json::json!({ "name": "clock-svc" }))
        .await;
    let service_id = service.str_field("id");
    let association = lattice
        .associate(&service_id, &network.str_field("id"))
        .await;
    let association_id = association.str_field("id");

    for (case, status) in [
        ("service", service.str_field("status")),
        ("association", association.str_field("status")),
    ] {
        assert_eq!(
            status, "CREATE_IN_PROGRESS",
            "case {case:?} immediately after the create: expected CREATE_IN_PROGRESS, got {status:?}"
        );
    }

    // Past the end of the transition window this project fixes for a clock-owed
    // change, with nothing but the clock moving.
    tokio::time::sleep(state::CREATE_TRANSITION + std::time::Duration::from_millis(300)).await;

    let read_service = lattice.get_service(&service_id).await;
    assert_eq!(
        read_service.str_field("status"),
        "ACTIVE",
        "case service after the transition: expected ACTIVE, got {}",
        read_service.body
    );
    let read_association = lattice
        .call(
            "GET",
            &format!("/servicenetworkserviceassociations/{association_id}"),
            None,
        )
        .await;
    assert_eq!(
        read_association.str_field("status"),
        "ACTIVE",
        "case association after the transition: expected ACTIVE, got {}",
        read_association.body
    );
}
