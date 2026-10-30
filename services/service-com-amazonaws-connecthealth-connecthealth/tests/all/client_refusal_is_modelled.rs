// SPDX-License-Identifier: CC0-1.0

//! Which ERROR SHAPE this service answers a handler-enforced CLIENT refusal with.
//!
//! The RULE under test: when a handler of this service refuses a request because of what
//! the REQUEST says — a page size outside the bound, a `nextToken` this service never
//! issued, a `status` outside the modelled enum, a tag entry or tag key it does not
//! accept — the error value it returns is the shape THIS service's model declares for
//! invalid client input, `com.amazonaws.connecthealth#ValidationException` (`@error
//! client`, `@httpError 400`, one member `message`), widened into the operation's own
//! error enum through `extra_errors` where that operation's `errors` list omits it — and
//! never a shape from outside the model that merely renders the same status and code.
//!
//! Sources for every value written by hand here:
//!
//! * `smithy_query(query="shape",shape="ValidationException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — the shape id, `errorKind: client` and `httpError: 400` asserted below.
//! * `smithy_query(query="operation",operation="ListDomains,ListSubscriptions,CreateDomain,TagResource,UntagResource",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — each operation's own `errors` list (empty for four of the five, so the shape is
//!   widened in) and the `@range{min:1,max:100}` on `maxResults`, which the page-size
//!   rows walk at each published bound and one past each.
//! * `smithy_query(query="shape",shape="DomainStatus,TagMap,TagKey,TagValue,TagKeyList",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — the enum's three values (`ops::types::DOMAIN_STATUS_VALUES`) and the tag lengths
//!   (key 1..128, value 0..256), both held as constants in `state` and read from there.
//!
//! Each row is asserted on the value the HANDLER returns, called directly with a typed
//! input: the wire form of a restJson1 error carries only the status, the `__type` code
//! and the detail text, so the shape identity — the thing the rule is about — is
//! observable only on the typed error. It is read off the SCHEMA the error channel
//! renders with (`OperationError::variant_schema`), which is what would carry any member
//! the modelled shape grows. The rows are then driven over the wire too, so the status,
//! the code and the detail text a signed request observes are pinned alongside.
//!
//! The rows differ along what the rule generalises over: WHICH operation refuses (five,
//! four of which declare no error of their own), which member is at fault (`maxResults`,
//! `nextToken`, `status`, `tags`, `tagKeys`), and the FORM of the fault (each side of a
//! numeric bound, a token of the wrong form and a token this service issued for another
//! collection, an enum value in the wrong case and one outside the enum, a key or value
//! past a length and one below it, a reserved key, and several faults at once). The
//! positive controls are the requests each operation SERVES, including both published
//! bounds of the page size and both bounds of each tag length.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_connecthealth_connecthealth as connecthealth;

use connecthealth::ops::create_domain::{CreateDomain, CreateDomainError};
use connecthealth::ops::list_domains::{ListDomains, ListDomainsError};
use connecthealth::ops::list_subscriptions::{ListSubscriptions, ListSubscriptionsError};
use connecthealth::ops::tag_resource::{TagResource, TagResourceError};
use connecthealth::ops::types::{
    CreateDomainInput, ListDomainsInput, ListDomainsOutput, ListSubscriptionsInput,
    ListSubscriptionsOutput, TagResourceInput, UntagResourceInput, DOMAIN_STATUS_VALUES,
};
use connecthealth::ops::untag_resource::{UntagResource, UntagResourceError};
use connecthealth::state;
use connecthealth::EmulatedService;

use crate::domain_subscription_lifecycle::{dispatch_signed, error_code, string_at};
use crate::resource_tag_removal::untag_path;
use crate::resource_tagging::tags_path;

/// The Region every request of this suite is signed for.
const REGION: &str = "us-west-2";

/// The shape id this SERVICE declares for invalid client input, which every refusal below
/// has to carry (see the module doc for the `smithy_query` it comes from).
const MODELLED_VALIDATION_SHAPE: &str = "com.amazonaws.connecthealth#ValidationException";

/// The wire error code that shape renders with on this service's restJson1 protocol.
const VALIDATION_CODE: &str = "ValidationException";

/// The body member this service reports an exception's detail text in.
const DETAIL_MEMBER: &str = "Message";

/// One emulator: the shared engine, this service mounted on a router (for the callback a
/// handler is given, and for the wire leg of each row) and the request metadata a handler
/// reads its Region from.
struct Refusals {
    engine: Arc<engine::InMemoryEngine>,
    service: EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Refusals {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    /// The context a handler is given, naming the Region this suite signs for.
    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// `ListDomains` as the HANDLER answers it, so the refusal's own type is observable.
    async fn list_domains(
        &self,
        status: Option<&str>,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> Result<ListDomainsOutput, ListDomainsError> {
        ListDomains::new(Arc::clone(&self.engine))
            .handle(
                ListDomainsInput {
                    status: status.map(str::to_string),
                    max_results,
                    next_token: next_token.map(str::to_string),
                },
                &self.ctx(),
            )
            .await
    }

    /// `ListSubscriptions` as the HANDLER answers it — the sibling listing, which declares
    /// the shape outright rather than widening it in.
    async fn list_subscriptions(
        &self,
        domain_id: &str,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> Result<ListSubscriptionsOutput, ListSubscriptionsError> {
        ListSubscriptions::new(Arc::clone(&self.engine))
            .handle(
                ListSubscriptionsInput {
                    domain_id: domain_id.to_string(),
                    max_results,
                    next_token: next_token.map(str::to_string),
                },
                &self.ctx(),
            )
            .await
    }

    /// `CreateDomain` as the HANDLER answers it, for the `tags` the request carries.
    async fn create_domain(
        &self,
        name: &str,
        tags: &HashMap<String, String>,
    ) -> Result<(), CreateDomainError> {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: name.to_string(),
                    kms_key_arn: None,
                    web_app_setup_configuration: None,
                    tags: Some(tags.clone()),
                },
                &self.ctx(),
            )
            .await
            .map(|_| ())
    }

    /// `TagResource` as the HANDLER answers it.
    async fn tag_resource(
        &self,
        resource_arn: &str,
        tags: &HashMap<String, String>,
    ) -> Result<(), TagResourceError> {
        TagResource::new(Arc::clone(&self.engine))
            .handle(
                TagResourceInput {
                    resource_arn: resource_arn.to_string(),
                    tags: tags.clone(),
                },
                &self.ctx(),
            )
            .await
            .map(|_| ())
    }

    /// `UntagResource` as the HANDLER answers it.
    async fn untag_resource(
        &self,
        resource_arn: &str,
        tag_keys: &[String],
    ) -> Result<(), UntagResourceError> {
        UntagResource::new(Arc::clone(&self.engine))
            .handle(
                UntagResourceInput {
                    resource_arn: resource_arn.to_string(),
                    tag_keys: tag_keys.to_vec(),
                },
                &self.ctx(),
            )
            .await
            .map(|_| ())
    }

    /// Status and parsed body of one signed restJson1 request against the same store.
    async fn send(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        dispatch_signed(&self.router, REGION, method, path, body).await
    }

    /// A domain created over the wire, as `(domainId, arn)` — the resource the tagging
    /// rows address and the collection the listing rows page through.
    async fn created_domain(&self, name: &str) -> (String, String) {
        let (status, created) = self
            .send("POST", "/domain", Some(&json!({ "name": name })))
            .await;
        assert_eq!(
            status, 201,
            "CreateDomain {name:?}: expected 201, got {status} with {created}"
        );
        (string_at(&created, "domainId"), string_at(&created, "arn"))
    }
}

/// Request metadata naming this service's signing name and a Region, which is how a
/// request names the Region a handler reads.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("GET")
        .uri("/domain")
        .header("host", format!("health-agent.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/health-agent/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// The rule, asserted on the value a handler returned: the shape THIS service models for
/// invalid client input, at its modelled status and under its wire code.
fn assert_modelled_client_refusal<E: OperationError + std::fmt::Debug>(case: &str, refused: &E) {
    let shape = refused.variant_schema().shape_id().to_string();
    assert_eq!(
        shape, MODELLED_VALIDATION_SHAPE,
        "case {case:?}: expected the shape this service models \
         ({MODELLED_VALIDATION_SHAPE}), got {shape:?} from {refused:?}"
    );
    assert_eq!(
        refused.http_status(),
        400,
        "case {case:?}: expected 400, got {} from {refused:?}",
        refused.http_status()
    );
    assert_eq!(
        refused.shape_name(),
        VALIDATION_CODE,
        "case {case:?}: expected the wire code {VALIDATION_CODE}, got {:?} from {refused:?}",
        refused.shape_name()
    );
}

/// The same refusal as a signed request observes it: the status, the code, and a detail
/// text saying what was wrong.
fn assert_refused_over_the_wire(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 over the wire, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        VALIDATION_CODE,
        "case {case:?}: expected {VALIDATION_CODE} over the wire, got {body}"
    );
    let detail = body.get(DETAIL_MEMBER).and_then(Value::as_str);
    assert!(
        detail.is_some_and(|text| !text.is_empty()),
        "case {case:?}: a refusal must say what was wrong in its {DETAIL_MEMBER}, got {body}"
    );
}

/// One request a listing cannot serve: the case it stands for, and the members that make
/// it unservable.
struct Unservable {
    case: String,
    status: Option<String>,
    max_results: Option<i32>,
    next_token: Option<String>,
}

/// `ListDomains` requests this listing cannot serve.
///
/// The page-size rows walk each published bound of `maxResults` from the outside (one
/// below the minimum, one past the maximum) and the extremes an `Integer` can carry; the
/// token rows cover the forms a token this service never issued can take; the `status`
/// rows cover an enum value spelled in the wrong case, one outside the enum, and none at
/// all.
fn unservable_listings() -> Vec<Unservable> {
    let mut rows: Vec<Unservable> = Vec::new();
    for (case, size) in [
        ("one below the published minimum", state::MIN_PAGE_SIZE - 1),
        ("one past the published maximum", state::MAX_PAGE_SIZE + 1),
        ("a negative page size", -7),
        ("far past the published maximum", 10_000),
        ("the largest Integer", i32::MAX),
        ("the smallest Integer", i32::MIN),
    ] {
        rows.push(Unservable {
            case: format!("a page size {case}"),
            status: None,
            max_results: Some(size),
            next_token: None,
        });
    }
    for (case, token) in [
        ("text that is not a token at all", "00"),
        ("the empty token", ""),
        ("hex of a plausible cursor nothing issued", "646f6d2d30303031"),
        ("a token with an odd number of hex digits", "abc"),
        ("letters outside the hex alphabet", "zzzz"),
    ] {
        rows.push(Unservable {
            case: format!("a nextToken that is {case}"),
            status: None,
            max_results: None,
            next_token: Some(token.to_string()),
        });
    }
    for (case, status) in [
        ("outside the modelled enum", "ARCHIVED"),
        ("a modelled value in the wrong case", "active"),
        ("the empty string", ""),
        ("the enum's own name", "DomainStatus"),
    ] {
        rows.push(Unservable {
            case: format!("a status {case}"),
            status: Some(status.to_string()),
            max_results: None,
            next_token: None,
        });
    }
    // Several faults at once: the refusal is still the one shape, whichever fault the
    // handler reaches first.
    rows.push(Unservable {
        case: "a status outside the enum and a page size past the bound".to_string(),
        status: Some("ARCHIVED".to_string()),
        max_results: Some(state::MAX_PAGE_SIZE + 1),
        next_token: Some("00".to_string()),
    });
    rows
}

/// `tags` maps this service does not accept, as `(case, the map)`.
///
/// The lengths are the modelled ones, held in `state`: a key one character past the
/// maximum and one below the minimum, a value one past its maximum, keys in the reserved
/// namespace in two spellings, and a map carrying several unacceptable entries at once.
fn unacceptable_tag_maps() -> Vec<(String, HashMap<String, String>)> {
    let long_key = "k".repeat(state::MAX_TAG_KEY_LEN + 1);
    let long_value = "v".repeat(state::MAX_TAG_VALUE_LEN + 1);
    let reserved = format!("{}owner", state::RESERVED_TAG_KEY_PREFIX);
    vec![
        (
            "a key one character past the modelled maximum".to_string(),
            HashMap::from([(long_key.clone(), "clinic".to_string())]),
        ),
        (
            "a key one character below the modelled minimum".to_string(),
            HashMap::from([(String::new(), "clinic".to_string())]),
        ),
        (
            "a value one character past the modelled maximum".to_string(),
            HashMap::from([("env".to_string(), long_value.clone())]),
        ),
        (
            "a key in the reserved namespace".to_string(),
            HashMap::from([(reserved.clone(), "mine".to_string())]),
        ),
        (
            "a reserved key spelled in another case".to_string(),
            HashMap::from([("AWS:Owner".to_string(), "mine".to_string())]),
        ),
        (
            "several unacceptable entries at once".to_string(),
            HashMap::from([
                (long_key, "clinic".to_string()),
                ("env".to_string(), long_value),
                (reserved, "mine".to_string()),
            ]),
        ),
    ]
}

/// `tagKeys` lists this service does not accept, as `(case, the keys)` — the same per-key
/// rules as a key named for a WRITE, since the reserved namespace names a tag no caller
/// may set or remove.
fn unacceptable_tag_keys() -> Vec<(String, Vec<String>)> {
    let long_key = "k".repeat(state::MAX_TAG_KEY_LEN + 1);
    let reserved = format!("{}owner", state::RESERVED_TAG_KEY_PREFIX);
    vec![
        (
            "a key one character past the modelled maximum".to_string(),
            vec![long_key.clone()],
        ),
        (
            "a key one character below the modelled minimum".to_string(),
            vec![String::new()],
        ),
        (
            "a key in the reserved namespace".to_string(),
            vec![reserved.clone()],
        ),
        (
            "a reserved key spelled in another case".to_string(),
            vec!["AWS:Owner".to_string()],
        ),
        (
            "an acceptable key beside an unacceptable one".to_string(),
            vec!["env".to_string(), reserved],
        ),
        (
            "several unacceptable keys at once".to_string(),
            vec![long_key, String::new()],
        ),
    ]
}

/// Every listing refusal of both paginated operations carries the shape this service
/// models for invalid client input — including the ones whose operation declares no error
/// of its own and widens the shape in.
#[tokio::test]
async fn a_listing_this_service_cannot_serve_is_refused_with_the_shape_it_models() {
    let emulator = Refusals::new();
    let (first_domain, _) = emulator.created_domain("refusal-shape-one").await;
    let (second_domain, _) = emulator.created_domain("refusal-shape-two").await;
    let (subscriptions_status, _) = emulator
        .send(
            "POST",
            &format!("/domains/{first_domain}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        subscriptions_status, 201,
        "the subscription the listing rows page through must be created"
    );

    for row in unservable_listings() {
        let case = &row.case;
        let refused = emulator
            .list_domains(
                row.status.as_deref(),
                row.max_results,
                row.next_token.as_deref(),
            )
            .await
            .expect_err(&format!("case {case:?}: ListDomains must refuse it"));
        assert_modelled_client_refusal(case, &refused);

        // The same row over the wire, as an SDK addresses this listing.
        let mut path = "/domain".to_string();
        for (name, value) in [
            ("status", row.status.clone()),
            ("maxResults", row.max_results.map(|size| size.to_string())),
            ("nextToken", row.next_token.clone()),
        ] {
            if let Some(value) = value {
                let separator = if path.contains('?') { '&' } else { '?' };
                path = format!("{path}{separator}{name}={value}");
            }
        }
        let (wire_status, answered) = emulator.send("GET", &path, None).await;
        assert_refused_over_the_wire(case, wire_status, &answered);
    }

    // The SAME pagination faults on the sibling listing, whose own `errors` list names
    // the shape: one rule, one shape, on both listings.
    for row in unservable_listings() {
        if row.max_results.is_none() && row.next_token.is_none() {
            // A `status` filter is the other listing's member alone.
            continue;
        }
        let case = format!("ListSubscriptions: {}", row.case);
        let refused = emulator
            .list_subscriptions(&first_domain, row.max_results, row.next_token.as_deref())
            .await
            .expect_err(&format!("case {case:?}: ListSubscriptions must refuse it"));
        assert_modelled_client_refusal(&case, &refused);
    }

    // A cursor this service DID issue, for another collection: a resume point for the
    // listing it was issued for and no other.
    let unfiltered = emulator
        .list_domains(None, Some(1), None)
        .await
        .expect("a page of one domain is served");
    let issued = unfiltered
        .next_token
        .expect("a token when a domain is left after the page");
    // The same cursor with its last hex digit CHANGED — the digit is chosen from the one
    // it carries, so the mutation is never a no-op whatever cursor this run issued.
    let last = issued
        .chars()
        .next_back()
        .expect("an issued cursor is not empty");
    let mutated = format!(
        "{}{}",
        &issued[..issued.len() - last.len_utf8()],
        if last == '0' { '1' } else { '0' }
    );
    assert_ne!(
        mutated, issued,
        "the mutated cursor must differ from the one this listing issued"
    );
    for (case, status, token) in [
        (
            "a cursor issued for the unfiltered listing, replayed under a filter",
            Some(DOMAIN_STATUS_VALUES[0]),
            issued.clone(),
        ),
        (
            "a cursor issued for the unfiltered listing with its last character changed",
            None,
            mutated,
        ),
    ] {
        let refused = emulator
            .list_domains(status, None, Some(&token))
            .await
            .expect_err(&format!("case {case:?}: ListDomains must refuse it"));
        assert_modelled_client_refusal(case, &refused);
    }

    // The boundary of the rule: what the listing SERVES. Both published bounds of the
    // page size, every value of the modelled enum, and the cursor this listing issued for
    // the listing it was issued for.
    for (case, status, max_results, token) in [
        ("no page size at all", None, None, None),
        (
            "the published minimum page size",
            None,
            Some(state::MIN_PAGE_SIZE),
            None,
        ),
        (
            "the published maximum page size",
            None,
            Some(state::MAX_PAGE_SIZE),
            None,
        ),
        (
            "the cursor this listing issued",
            None,
            None,
            Some(issued.clone()),
        ),
    ] {
        let served = emulator
            .list_domains(status, max_results, token.as_deref())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: must be served, got {error:?}"));
        assert!(
            served.domains.len() <= usize::try_from(state::MAX_PAGE_SIZE).expect("a usize"),
            "case {case:?}: a page may not exceed the bound, got {}",
            served.domains.len()
        );
    }
    for status in DOMAIN_STATUS_VALUES {
        let served = emulator
            .list_domains(Some(status), None, None)
            .await
            .unwrap_or_else(|error| panic!("case {status:?}: must be served, got {error:?}"));
        for summary in &served.domains {
            assert_eq!(
                &summary.status, status,
                "case {status:?}: the filter must hold, got {:?}",
                summary.status
            );
        }
    }
    let both_served = emulator
        .list_subscriptions(&second_domain, Some(state::MAX_PAGE_SIZE), None)
        .await
        .expect("an empty collection is served, not refused");
    assert!(
        both_served.subscriptions.is_empty(),
        "a domain with no subscription reports none, got {:?}",
        both_served.subscriptions
    );
}

/// Every tagging refusal — on the create that carries a `TagMap`, on the write that
/// merges one, and on the removal that names keys — carries the same modelled shape.
#[tokio::test]
async fn a_tag_this_service_does_not_accept_is_refused_with_the_shape_it_models() {
    let emulator = Refusals::new();
    let (_, arn) = emulator.created_domain("refusal-shape-tags").await;

    for (case, tags) in unacceptable_tag_maps() {
        // The create that carries the map. A refusal mints nothing, so the account's
        // domain cap is untouched however many rows there are.
        let create_case = format!("CreateDomain: {case}");
        let refused = emulator
            .create_domain("refusal-shape-create", &tags)
            .await
            .expect_err(&format!("case {create_case:?}: CreateDomain must refuse it"));
        assert_modelled_client_refusal(&create_case, &refused);
        let (status, answered) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": "refusal-shape-create", "tags": tags })),
            )
            .await;
        assert_refused_over_the_wire(&create_case, status, &answered);

        // The write that merges the map onto a resource that exists.
        let tag_case = format!("TagResource: {case}");
        let refused = emulator
            .tag_resource(&arn, &tags)
            .await
            .expect_err(&format!("case {tag_case:?}: TagResource must refuse it"));
        assert_modelled_client_refusal(&tag_case, &refused);
        let (status, answered) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
            .await;
        assert_refused_over_the_wire(&tag_case, status, &answered);
    }

    for (case, keys) in unacceptable_tag_keys() {
        let case = format!("UntagResource: {case}");
        let refused = emulator
            .untag_resource(&arn, &keys)
            .await
            .expect_err(&format!("case {case:?}: UntagResource must refuse it"));
        assert_modelled_client_refusal(&case, &refused);
        let (status, answered) = emulator.send("DELETE", &untag_path(&arn, &keys), None).await;
        assert_refused_over_the_wire(&case, status, &answered);
    }

    // The boundary of the rule: the entries this service DOES accept — each modelled
    // length at both of its bounds — are served, not refused.
    let longest_key = "k".repeat(state::MAX_TAG_KEY_LEN);
    let shortest_key = "k".repeat(state::MIN_TAG_KEY_LEN);
    let longest_value = "v".repeat(state::MAX_TAG_VALUE_LEN);
    let shortest_value = "v".repeat(state::MIN_TAG_VALUE_LEN);
    for (case, tags) in [
        (
            "a key at the modelled maximum length",
            HashMap::from([(longest_key.clone(), "clinic".to_string())]),
        ),
        (
            "a key at the modelled minimum length",
            HashMap::from([(shortest_key.clone(), "clinic".to_string())]),
        ),
        (
            "a value at the modelled maximum length",
            HashMap::from([("env".to_string(), longest_value)]),
        ),
        (
            "a value at the modelled minimum length",
            HashMap::from([("env".to_string(), shortest_value)]),
        ),
        ("no entries at all", HashMap::new()),
    ] {
        emulator
            .tag_resource(&arn, &tags)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: must be served, got {error:?}"));
    }
    for (case, keys) in [
        ("a key the resource carries", vec![longest_key]),
        ("a key the resource never carried", vec!["absent".to_string()]),
        ("no keys at all", Vec::new()),
    ] {
        emulator
            .untag_resource(&arn, &keys)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: must be served, got {error:?}"));
    }
    emulator
        .create_domain(
            "refusal-shape-accepted",
            &HashMap::from([(shortest_key, "clinic".to_string())]),
        )
        .await
        .expect("a create carrying an acceptable tag is served");
}
