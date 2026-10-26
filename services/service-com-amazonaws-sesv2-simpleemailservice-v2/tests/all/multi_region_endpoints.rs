// SPDX-License-Identifier: CC0-1.0

//! The multi-region endpoint (global-endpoint) surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the engine's own state where a response cannot show what was
//! persisted.
//!
//! The rules these cases pin:
//!
//! * a created endpoint is a RESOURCE: `CreateMultiRegionEndpoint` persists it under the
//!   name the request names, with the Region the call executes in as its primary region
//!   and the route details' Region as its secondary region, mints it a unique and stable
//!   `EndpointId`, and reports it in the non-terminal `CREATING` status until the
//!   provisioning transition the clock owes falls due, after which every read reports
//!   `READY`;
//! * every read of it computes its answer from that record, so the create, the read, the
//!   listing and the delete never disagree;
//! * a delete starts a tear-down the clock owes: the endpoint reads as `DELETING` while
//!   it runs, a second delete collides with the one in flight, and once it is done the
//!   endpoint and its name are gone;
//! * an endpoint is visible only from its primary region, and a call this service cannot
//!   serve is refused with its own modelled, terminal error (a 4xx naming the member or
//!   the resource), never with a status a client would retry.
//!
//! The tables below span what the rules generalise over: the endpoint name's form and
//! length, the secondary region, the number of tags, the Region the call executes in,
//! the number of stored endpoints and the page size, and every refusal leg.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, MultiRegionEndpointRecord,
    ENDPOINT_DELETION_SETTLE, ENDPOINT_PROVISIONING_SETTLE, MAX_MULTI_REGION_ENDPOINTS,
    MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases execute in: the one their `Authorization` credential scope
/// names, which is what makes it the created endpoint's primary region.
const HOME: &str = "us-east-1";

/// Another Region, for the cases that read an endpoint from somewhere other than its
/// primary region.
const ELSEWHERE: &str = "eu-west-1";

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The message member as the error document carries it: `Some(Value::Null)` when the
    /// service supplied none, `Some(a string)` when it did, `None` when the key is
    /// missing from the document altogether.
    fn message_member(&self) -> Option<&Value> {
        self.body.get("message").or_else(|| self.body.get("Message"))
    }

    /// A timestamp member, rendered by restJson1 as epoch seconds.
    fn seconds(&self, member: &str) -> Option<f64> {
        self.body.get(member).and_then(Value::as_f64)
    }

    /// The regions a `Routes` list reports, in the order it reported them.
    fn routes(&self) -> Vec<String> {
        self.body
            .get("Routes")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get("Region")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine so a test can read
/// what a handler persisted and drive the clock-owed transition.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Send one request, signed for `region` — the credential scope is what tells the
    /// handler which AWS-Region the operation executes in.
    async fn call(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, region: &str, body: Value) -> Reply {
        self.call(region, "POST", "/v2/email/multi-region-endpoints", Some(body))
            .await
    }

    async fn get(&self, region: &str, name: &str) -> Reply {
        self.call(
            region,
            "GET",
            &format!("/v2/email/multi-region-endpoints/{name}"),
            None,
        )
        .await
    }

    async fn delete(&self, region: &str, name: &str) -> Reply {
        self.call(
            region,
            "DELETE",
            &format!("/v2/email/multi-region-endpoints/{name}"),
            None,
        )
        .await
    }

    async fn list(&self, region: &str, query: &str) -> Reply {
        self.call(
            region,
            "GET",
            &format!("/v2/email/multi-region-endpoints{query}"),
            None,
        )
        .await
    }

    /// Create `name` in `region` with one secondary region and no tags, asserting the
    /// create succeeded so a later assertion cannot be fooled by a failed setup.
    async fn create_ok(&self, region: &str, name: &str, secondary: &str) -> Reply {
        let reply = self
            .create(
                region,
                json!({
                    "EndpointName": name,
                    "Details": {"RoutesDetails": [{"Region": secondary}]},
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the create must succeed, got {} {:?}",
            reply.status, reply.body
        );
        reply
    }

    /// Move the instant at which `name`'s provisioning transition falls due.
    ///
    /// The handler stores a due instant rather than sleeping, so a test drives the
    /// transition by moving that instant instead of waiting on the wall clock — which is
    /// what keeps assertions about the transient state and about the settled state
    /// independent of how loaded the machine is.
    fn set_ready_due(&self, region: &str, name: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .multi_region_endpoints
            .get_mut(&(region.to_string(), name.to_string()))
            .unwrap_or_else(|| panic!("case {name:?}: the endpoint must exist in the store"));
        record.ready_due_at = due_at;
    }

    /// Hold the clock-owed provisioning transition off, so an assertion about the
    /// transient `CREATING` state cannot race the span.
    fn hold_creating(&self, region: &str, name: &str) {
        self.set_ready_due(
            region,
            name,
            SystemTime::now() + ENDPOINT_PROVISIONING_SETTLE + Duration::from_secs(3600),
        );
    }

    /// Bring the clock-owed provisioning transition due, without waiting for it.
    fn settle_provisioning(&self, region: &str, name: &str) {
        self.set_ready_due(region, name, SystemTime::now() - Duration::from_secs(1));
    }

    /// The span the handler stored between creating `name` and its provisioning
    /// transition falling due.
    fn provisioning_span(&self, region: &str, name: &str) -> Duration {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .multi_region_endpoints
            .get(&(region.to_string(), name.to_string()))
            .unwrap_or_else(|| panic!("case {name:?}: the endpoint must exist in the store"));
        record
            .ready_due_at
            .duration_since(record.created_at)
            .unwrap_or_default()
    }

    /// How long the deletion of `name` still has to run, as the handler stored it.
    fn deletion_due_in(&self, region: &str, name: &str) -> Duration {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let due_at = store
            .multi_region_endpoints
            .get(&(region.to_string(), name.to_string()))
            .and_then(|record| record.deletion_due_at)
            .unwrap_or_else(|| panic!("case {name:?}: a deletion of it must be in flight"));
        due_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
    }

    /// Bring the in-flight deletion of `name` due, without waiting for it.
    fn complete_deletion(&self, region: &str, name: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .multi_region_endpoints
            .get_mut(&(region.to_string(), name.to_string()))
            .unwrap_or_else(|| panic!("case {name:?}: the endpoint must exist in the store"));
        assert!(
            record.deletion_due_at.is_some(),
            "case {name:?}: a deletion of it must be in flight"
        );
        record.deletion_due_at = Some(SystemTime::now() - Duration::from_secs(1));
    }

    /// The tags the record holds, as the store keeps them.
    fn stored_tags(&self, region: &str, name: &str) -> Vec<(String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .multi_region_endpoints
            .get(&(region.to_string(), name.to_string()))
            .map(|record| record.tags.clone())
            .unwrap_or_default()
    }
}

/// The endpoint names a `ListMultiRegionEndpoints` page reports, in the order it
/// reported them.
fn listed(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("MultiRegionEndpoints")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("EndpointName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One listed row, by endpoint name.
fn listed_row(reply: &Reply, name: &str) -> Value {
    reply
        .body
        .get("MultiRegionEndpoints")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("EndpointName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The `NextToken` a page handed back, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Percent-encode a `NextToken` for the query string: the shared helper of this test
/// binary (`contact_lists::query_escaped`), since `ListMultiRegionEndpoints` binds
/// `NextToken` to `@httpQuery` exactly as `ListContactLists` does.
use crate::contact_lists::query_escaped;

/// One row of the create/read-back table.
struct EndpointCase {
    case: &'static str,
    /// Within `EndpointName`'s modelled bounds: len 1..64, pattern `^[\w\-_]+$`.
    name: &'static str,
    /// The secondary region the route details name.
    secondary: &'static str,
    /// The tags the create request carries, in order.
    tags: &'static [(&'static str, &'static str)],
}

/// The rows differ along everything the create rule generalises over: the name's form
/// (a single character, digits, an underscore, a hyphen, mixed case, the 64-character
/// maximum the model allows), the secondary region, and the number of tags (none, one,
/// several).
const ENDPOINT_CASES: &[EndpointCase] = &[
    EndpointCase {
        case: "a one-character name, no tags",
        name: "e",
        secondary: "us-west-2",
        tags: &[],
    },
    EndpointCase {
        case: "a hyphenated name with one tag",
        name: "marketing-endpoint",
        secondary: "eu-central-1",
        tags: &[("Team", "marketing")],
    },
    EndpointCase {
        case: "an underscored, mixed-case name with several tags",
        name: "Transactional_Mail_1",
        secondary: "ap-southeast-1",
        tags: &[("Team", "platform"), ("Env", "prod"), ("Owner", "")],
    },
    EndpointCase {
        case: "an all-digit name",
        name: "20260101",
        secondary: "us-west-1",
        tags: &[],
    },
    EndpointCase {
        case: "the 64-character maximum the model allows",
        name: "012345678901234567890123456789012345678901234567890123456789abcd",
        secondary: "sa-east-1",
        tags: &[("Scope", "global")],
    },
];

/// The body a row's create request carries.
fn create_body(row: &EndpointCase) -> Value {
    let mut body = json!({
        "EndpointName": row.name,
        "Details": {"RoutesDetails": [{"Region": row.secondary}]},
    });
    if !row.tags.is_empty() {
        body["Tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    body
}

/// RULE: a create persists the endpoint and answers with its minted `EndpointId` and
/// the non-terminal `CREATING` status; a read computes its answer from that same record,
/// so the id, the name, the routes and the tags round-trip, and the ids are unique.
#[tokio::test]
async fn a_created_endpoint_reads_back_as_it_was_created() {
    let ses = Ses::new();
    let mut ids = BTreeSet::new();
    for row in ENDPOINT_CASES {
        let case = row.case;
        assert!(
            !row.name.is_empty() && row.name.len() <= 64,
            "case {case:?}: the row must stay inside EndpointName's modelled 1..64 bound, got {}",
            row.name.len()
        );

        let created = ses.create(HOME, create_body(row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from the create, got {} {:?}",
            created.status, created.body
        );
        let endpoint_id = created.text("EndpointId");
        assert!(
            !endpoint_id.is_empty() && !endpoint_id.contains(char::is_whitespace),
            "case {case:?}: expected a minted EndpointId, got {endpoint_id:?}"
        );
        assert!(
            ids.insert(endpoint_id.clone()),
            "case {case:?}: EndpointId {endpoint_id:?} was already minted for another endpoint"
        );
        // The transition is held off so the transient status is not a race.
        ses.hold_creating(HOME, row.name);
        assert_eq!(
            created.text("Status"),
            "CREATING",
            "case {case:?}: expected the status right after the create, got {:?}",
            created.body
        );

        let read = ses.get(HOME, row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 reading the endpoint back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("EndpointName"),
            row.name,
            "case {case:?}: the read must report the name it was created with, got {:?}",
            read.body
        );
        assert_eq!(
            read.text("EndpointId"),
            endpoint_id,
            "case {case:?}: the read must report the minted id, got {:?}",
            read.body
        );
        assert_eq!(
            read.text("Status"),
            "CREATING",
            "case {case:?}: an endpoint still being provisioned reads as CREATING, got {:?}",
            read.body
        );
        // "Sending traffic is divided between the two": the primary region the call
        // executed in, then the secondary region the route details named.
        assert_eq!(
            read.routes(),
            vec![HOME.to_string(), row.secondary.to_string()],
            "case {case:?}: expected the primary and secondary regions as routes, got {:?}",
            read.body
        );
        let created_at = read.seconds("CreatedTimestamp");
        let updated_at = read.seconds("LastUpdatedTimestamp");
        assert!(
            created_at.is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected a CreatedTimestamp, got {:?}",
            read.body
        );
        assert_eq!(
            updated_at, created_at,
            "case {case:?}: nothing has changed the endpoint yet, got {:?}",
            read.body
        );

        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            ses.stored_tags(HOME, row.name),
            expected_tags,
            "case {case:?}: expected the tags the create carried, in order"
        );
    }
}

/// RULE: the provisioning transition is owed by the CLOCK — it falls due a fixed,
/// non-zero span after the create, and every read past that instant reports `READY`
/// without anything else having been called.
#[tokio::test]
async fn provisioning_settles_when_the_clock_says_so() {
    let ses = Ses::new();
    for row in ENDPOINT_CASES {
        let case = row.case;
        ses.create_ok(HOME, row.name, row.secondary).await;

        let span = ses.provisioning_span(HOME, row.name);
        assert_eq!(
            span, ENDPOINT_PROVISIONING_SETTLE,
            "case {case:?}: expected the transition to fall due {ENDPOINT_PROVISIONING_SETTLE:?} \
             after the create, got {span:?}"
        );

        ses.hold_creating(HOME, row.name);
        assert_eq!(
            ses.get(HOME, row.name).await.text("Status"),
            "CREATING",
            "case {case:?}: before the due instant the endpoint is still being provisioned"
        );

        ses.settle_provisioning(HOME, row.name);
        let settled = ses.get(HOME, row.name).await;
        assert_eq!(
            settled.text("Status"),
            "READY",
            "case {case:?}: past the due instant the endpoint is ready, got {:?}",
            settled.body
        );
        assert_eq!(
            listed_row(&ses.list(HOME, "").await, row.name)
                .get("Status")
                .and_then(Value::as_str),
            Some("READY"),
            "case {case:?}: the listing must report the same status as the read"
        );
        // A read reports the transition; it does not cause one, so reading again twice
        // answers exactly the same.
        let again = ses.get(HOME, row.name).await;
        assert_eq!(
            (again.text("Status"), again.seconds("LastUpdatedTimestamp")),
            (
                settled.text("Status"),
                settled.seconds("LastUpdatedTimestamp")
            ),
            "case {case:?}: a second read must answer exactly as the first, got {:?}",
            again.body
        );
    }
}

/// RULE: a second create of a name the Region already holds is refused with the
/// modelled `AlreadyExistsException`, and leaves the stored endpoint exactly as it was.
#[tokio::test]
async fn a_duplicate_endpoint_name_is_refused() {
    let ses = Ses::new();
    for row in ENDPOINT_CASES {
        let case = row.case;
        let first = ses.create(HOME, create_body(row)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {:?}",
            first.body
        );

        // A second create of the same name, whatever else the request says.
        let again = ses
            .create(
                HOME,
                json!({
                    "EndpointName": row.name,
                    "Details": {"RoutesDetails": [{"Region": "us-west-2"}]},
                }),
            )
            .await;
        assert_eq!(
            (again.status, again.error_code()),
            (400, "AlreadyExistsException".to_string()),
            "case {case:?}: expected a 400 AlreadyExistsException, got {} {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            again.message(),
            "Endpoint already exists.",
            "case {case:?}: expected the refusal this error class is worded with, got {:?}",
            again.body
        );

        let read = ses.get(HOME, row.name).await;
        assert_eq!(
            read.text("EndpointId"),
            first.text("EndpointId"),
            "case {case:?}: the refused create must not replace the stored endpoint"
        );
        assert_eq!(
            read.routes(),
            vec![HOME.to_string(), row.secondary.to_string()],
            "case {case:?}: the refused create must not change the stored routes, got {:?}",
            read.body
        );
    }
}

/// One row of the invalid-details table.
struct BadDetailsCase {
    case: &'static str,
    details: fn() -> Value,
}

/// `Details$RoutesDetails` is documented as "a list of route configuration details. Must
/// contain exactly one route configuration", and that one configuration names the
/// secondary region — so no route, two routes and a blank Region are each outside the
/// documented rule. A route naming the primary region is NOT a row here: nothing
/// documents it as invalid, so the emulator takes it as given rather than over-refusing.
const BAD_DETAILS_CASES: &[BadDetailsCase] = &[
    BadDetailsCase {
        case: "no route configuration",
        details: || json!({"RoutesDetails": []}),
    },
    BadDetailsCase {
        case: "two route configurations",
        details: || json!({"RoutesDetails": [{"Region": "us-west-2"}, {"Region": "eu-west-1"}]}),
    },
    BadDetailsCase {
        case: "a blank secondary region",
        details: || json!({"RoutesDetails": [{"Region": "   "}]}),
    },
];

/// RULE: route details outside that rule are refused with the modelled
/// `BadRequestException` — a terminal 4xx — and nothing is persisted.
#[tokio::test]
async fn invalid_route_details_are_refused() {
    let ses = Ses::new();
    for (index, row) in BAD_DETAILS_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("refused_{index}");
        let reply = ses
            .create(
                HOME,
                json!({"EndpointName": name, "Details": (row.details)()}),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException".to_string()),
            "case {case:?}: expected a 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            !reply.message().is_empty(),
            "case {case:?}: the refusal must say what was wrong with the request"
        );
        assert_eq!(
            ses.get(HOME, &name).await.status,
            404,
            "case {case:?}: a refused create must persist nothing"
        );
    }
    assert!(
        listed(&ses.list(HOME, "").await).is_empty(),
        "no refused create may be listed, got {:?}",
        ses.list(HOME, "").await.body
    );
}

/// RULE: a delete starts a tear-down and reports the non-terminal status the endpoint
/// carries right after the request; while that tear-down runs the endpoint reads as
/// `DELETING` and a second delete collides with the one already in flight; once the
/// clock says the tear-down is done the endpoint and its name are gone, so every later
/// call answers as for an endpoint the Region does not hold.
#[tokio::test]
async fn a_delete_tears_the_endpoint_down_and_then_it_is_gone() {
    let ses = Ses::new();
    for row in ENDPOINT_CASES {
        let case = row.case;
        ses.create_ok(HOME, row.name, row.secondary).await;

        let deleted = ses.delete(HOME, row.name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from the delete, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("Status"),
            "DELETING",
            "case {case:?}: expected the status right after the delete, got {:?}",
            deleted.body
        );
        let due_in = ses.deletion_due_in(HOME, row.name);
        assert!(
            due_in > Duration::ZERO && due_in <= ENDPOINT_DELETION_SETTLE,
            "case {case:?}: the tear-down must still be running for at most \
             {ENDPOINT_DELETION_SETTLE:?}, got {due_in:?}"
        );

        // While the tear-down runs the endpoint is still there, in the status the
        // delete reported.
        let read = ses.get(HOME, row.name).await;
        assert_eq!(
            (read.status, read.text("Status")),
            (200, "DELETING".to_string()),
            "case {case:?}: an endpoint being torn down reads as DELETING, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            listed_row(&ses.list(HOME, "").await, row.name)
                .get("Status")
                .and_then(Value::as_str),
            Some("DELETING"),
            "case {case:?}: the listing must report the same status as the read"
        );

        // A second delete racing the first collides with the tear-down already in
        // flight rather than starting another one.
        let racing = ses.delete(HOME, row.name).await;
        assert_eq!(
            (racing.status, racing.error_code(), racing.message()),
            (
                500,
                "ConcurrentModificationException".to_string(),
                "Unable to delete resource in DEPROVISIONING status.".to_string()
            ),
            "case {case:?}: expected the competing delete to collide, got {} {:?}",
            racing.status,
            racing.body
        );
        // The name is still held while the tear-down runs.
        let early = ses
            .create(
                HOME,
                json!({
                    "EndpointName": row.name,
                    "Details": {"RoutesDetails": [{"Region": row.secondary}]},
                }),
            )
            .await;
        assert_eq!(
            (early.status, early.error_code()),
            (400, "AlreadyExistsException".to_string()),
            "case {case:?}: the name is not free until the tear-down is done, got {} {:?}",
            early.status,
            early.body
        );

        // Once the clock says the tear-down is done, the endpoint is gone.
        ses.complete_deletion(HOME, row.name);
        let gone = ses.get(HOME, row.name).await;
        assert_eq!(
            (gone.status, gone.error_code()),
            (404, "NotFoundException".to_string()),
            "case {case:?}: expected a 404 NotFoundException reading a deleted endpoint, got {} {:?}",
            gone.status,
            gone.body
        );
        assert_eq!(
            gone.message_member(),
            Some(&Value::Null),
            "case {case:?}: this error class supplies no message, and the document carries \
             the member as null rather than dropping it, got {:?}",
            gone.body
        );
        let again = ses.delete(HOME, row.name).await;
        assert_eq!(
            (again.status, again.error_code(), again.message()),
            (
                404,
                "NotFoundException".to_string(),
                "Endpoint does not exist.".to_string()
            ),
            "case {case:?}: expected a 404 NotFoundException once the tear-down is done, got {} {:?}",
            again.status,
            again.body
        );

        assert!(
            !listed(&ses.list(HOME, "").await).contains(&row.name.to_string()),
            "case {case:?}: a deleted endpoint must not be listed"
        );

        // The name is free again, and a fresh create mints a fresh resource.
        let recreated = ses.create_ok(HOME, row.name, row.secondary).await;
        assert!(
            !recreated.text("EndpointId").is_empty(),
            "case {case:?}: recreating the name must mint an id, got {:?}",
            recreated.body
        );
    }
}

/// RULE: a name the Region does not hold is refused with the modelled
/// `NotFoundException` by every operation that takes an `EndpointName` — a read with no
/// message at all, a delete saying the endpoint is not there.
#[tokio::test]
async fn an_unknown_endpoint_name_is_not_found() {
    let ses = Ses::new();
    ses.create_ok(HOME, "known", "us-west-2").await;
    // Names that differ from a stored one in case, in a suffix and in nothing but their
    // absence; each stays inside the modelled pattern.
    for name in ["unknown", "KNOWN", "known_2", "k"] {
        for (operation, expected_message, reply) in [
            (
                "GetMultiRegionEndpoint",
                Value::Null,
                ses.get(HOME, name).await,
            ),
            (
                "DeleteMultiRegionEndpoint",
                Value::String("Endpoint does not exist.".to_string()),
                ses.delete(HOME, name).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException".to_string()),
                "case {name:?} on {operation}: expected a 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message_member(),
                Some(&expected_message),
                "case {name:?} on {operation}: expected the message this error class carries \
                 there, present even when it is none, got {:?}",
                reply.body
            );
        }
    }
    assert_eq!(
        ses.get(HOME, "known").await.status,
        200,
        "the stored endpoint must survive every refused call"
    );
}

/// RULE: an endpoint belongs to the Region the create executed in — its primary region —
/// and is read, listed and deleted only from there; the same name in another Region names
/// another resource.
#[tokio::test]
async fn an_endpoint_is_visible_only_from_its_primary_region() {
    let ses = Ses::new();
    for row in ENDPOINT_CASES {
        let case = row.case;
        let home = ses.create_ok(HOME, row.name, row.secondary).await;

        for (operation, reply) in [
            ("GetMultiRegionEndpoint", ses.get(ELSEWHERE, row.name).await),
            (
                "DeleteMultiRegionEndpoint",
                ses.delete(ELSEWHERE, row.name).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException".to_string()),
                "case {case:?} on {operation}: an endpoint of {HOME} is not {ELSEWHERE}'s to \
                 serve, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        assert!(
            !listed(&ses.list(ELSEWHERE, "").await).contains(&row.name.to_string()),
            "case {case:?}: {ELSEWHERE} must not list an endpoint whose primary region is {HOME}"
        );
        assert!(
            listed(&ses.list(HOME, "").await).contains(&row.name.to_string()),
            "case {case:?}: {HOME} must list the endpoint it holds"
        );

        // The same name in another Region is another resource, with its own id and its
        // own routes.
        let elsewhere = ses.create_ok(ELSEWHERE, row.name, row.secondary).await;
        assert_ne!(
            elsewhere.text("EndpointId"),
            home.text("EndpointId"),
            "case {case:?}: two Regions' endpoints of one name are two resources"
        );
        assert_eq!(
            ses.get(ELSEWHERE, row.name).await.routes(),
            vec![ELSEWHERE.to_string(), row.secondary.to_string()],
            "case {case:?}: the second endpoint's primary region is the one it was created in"
        );
        assert_eq!(
            ses.get(HOME, row.name).await.routes(),
            vec![HOME.to_string(), row.secondary.to_string()],
            "case {case:?}: the first endpoint is untouched by the second"
        );
    }
}

/// One row of the pagination table: how many endpoints are stored and the `PageSize` the
/// walk asks for.
struct PageCase {
    case: &'static str,
    stored: usize,
    page_size: Option<usize>,
}

/// The pagination rule spans the item count and the page size independently, so the rows
/// cross an empty, a single and a multi-item store with an absent page size, a page size
/// of one, one that divides the count and one larger than it.
const PAGE_CASES: &[PageCase] = &[
    PageCase {
        case: "nothing stored, no page size",
        stored: 0,
        page_size: None,
    },
    PageCase {
        case: "nothing stored, page size 1",
        stored: 0,
        page_size: Some(1),
    },
    PageCase {
        case: "one endpoint, page size 1",
        stored: 1,
        page_size: Some(1),
    },
    PageCase {
        case: "one endpoint, no page size",
        stored: 1,
        page_size: None,
    },
    PageCase {
        case: "two endpoints, page size 1",
        stored: 2,
        page_size: Some(1),
    },
    PageCase {
        case: "seven endpoints, page size 3",
        stored: 7,
        page_size: Some(3),
    },
    PageCase {
        case: "six endpoints, page size 2",
        stored: 6,
        page_size: Some(2),
    },
    PageCase {
        case: "four endpoints, page size 10",
        stored: 4,
        page_size: Some(10),
    },
    PageCase {
        case: "five endpoints, no page size",
        stored: 5,
        page_size: None,
    },
];

/// RULE: a listing answers with a page of at most `PageSize` endpoints of the Region the
/// call executes in, and a `NextToken` only while endpoints remain behind that page, so a
/// walk that follows the token terminates and visits every endpoint exactly once.
#[tokio::test]
async fn a_listing_walks_every_endpoint_exactly_once() {
    for row in PAGE_CASES {
        let case = row.case;
        let ses = Ses::new();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..row.stored {
            let name = format!("endpoint_{index:02}");
            ses.create_ok(HOME, &name, "us-west-2").await;
            expected.push(name);
        }
        // The store is ordered by name, which is the order a page reports.
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(size) = row.page_size {
                query.push(format!("PageSize={size}"));
            }
            if let Some(value) = &token {
                query.push(format!("NextToken={}", query_escaped(value)));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let page = ses.list(HOME, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from the listing, got {} {:?}",
                page.status, page.body
            );
            let names = listed(&page);
            if let Some(size) = row.page_size {
                assert!(
                    names.len() <= size,
                    "case {case:?}: a page must hold at most {size} endpoints, got {names:?}"
                );
            }
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= row.stored + 2,
                "case {case:?}: the walk must terminate, got {pages} pages for {} endpoints",
                row.stored
            );
            token = next_token(&page);
            if token.is_none() {
                break;
            }
            assert!(
                !seen.is_empty(),
                "case {case:?}: a NextToken must only follow a page that reported something"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: the walk must visit every endpoint exactly once, in name order"
        );
    }
}

/// RULE: a `NextToken` of this listing's own making resumes it; anything else is refused
/// with the modelled `BadRequestException` rather than silently restarting the walk.
#[tokio::test]
async fn a_token_from_anywhere_else_is_refused() {
    let ses = Ses::new();
    for index in 0..3 {
        ses.create_ok(HOME, &format!("endpoint_{index}"), "us-west-2")
            .await;
    }
    let mine = next_token(&ses.list(HOME, "?PageSize=1").await)
        .expect("a page of one of three endpoints hands back a token");

    let resumed = ses
        .list(HOME, &format!("?NextToken={}", query_escaped(&mine)))
        .await;
    assert_eq!(
        (resumed.status, listed(&resumed)),
        (
            200,
            vec!["endpoint_1".to_string(), "endpoint_2".to_string()]
        ),
        "case {mine:?}: this listing's own token must resume it, got {:?}",
        resumed.body
    );

    // Tokens from anywhere else: a made-up base64 string, one of another listing's, and
    // text that is not base64 at all. Each stays inside `NextToken`'s modelled bounds.
    let foreign = next_token(&ses.list(HOME, "?PageSize=1").await).map(|token| {
        // One character of the tag changed is still base64, and still not this
        // listing's token.
        let mut bytes = token.into_bytes();
        bytes[0] = if bytes[0] == b'c' { b'd' } else { b'c' };
        String::from_utf8(bytes).expect("still ASCII")
    });
    let mut cases: Vec<String> = vec!["bm90LWEtdG9rZW4=".to_string(), "YWJjZA==".to_string()];
    cases.extend(foreign);
    for token in cases {
        let reply = ses
            .list(HOME, &format!("?NextToken={}", query_escaped(&token)))
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException".to_string()),
            "case {token:?}: expected a 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// RULE: a resource carries at most the documented number of tags — "a resource can
/// have as many as 50 tags" (the model's `Tag` documentation) — so a create at the bound
/// is served and one past it is refused with the modelled `BadRequestException`. The
/// rows walk the published bound and one past it, plus the counts either side of it.
#[tokio::test]
async fn the_documented_tag_count_bounds_a_create() {
    let ses = Ses::new();
    for count in [0, 1, MAX_TAGS_PER_RESOURCE - 1, MAX_TAGS_PER_RESOURCE] {
        let name = format!("tagged_{count}");
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({"Key": format!("Key{index}"), "Value": format!("Value{index}")}))
            .collect();
        let created = ses
            .create(
                HOME,
                json!({
                    "EndpointName": name,
                    "Details": {"RoutesDetails": [{"Region": "us-west-2"}]},
                    "Tags": tags,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {count} tags: a count inside the documented bound is served, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            ses.stored_tags(HOME, &name).len(),
            count,
            "case {count} tags: every tag the create carried must be stored"
        );
    }

    for count in [MAX_TAGS_PER_RESOURCE + 1, MAX_TAGS_PER_RESOURCE + 10] {
        let name = format!("over_tagged_{count}");
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({"Key": format!("Key{index}"), "Value": format!("Value{index}")}))
            .collect();
        let refused = ses
            .create(
                HOME,
                json!({
                    "EndpointName": name,
                    "Details": {"RoutesDetails": [{"Region": "us-west-2"}]},
                    "Tags": tags,
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException".to_string()),
            "case {count} tags: a count past the documented bound is refused, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            ses.get(HOME, &name).await.status,
            404,
            "case {count} tags: a refused create must persist nothing"
        );
    }
}

/// RULE: the create refuses past the store's endpoint limit with the modelled
/// `LimitExceededException`, which the model binds for exactly that; the limit is on
/// the whole table, so the Region a request names cannot buy a fresh budget, and the
/// stored endpoints are untouched.
#[tokio::test]
async fn the_endpoint_limit_bounds_the_whole_store() {
    let ses = Ses::new();
    let now = SystemTime::now();
    {
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        for index in 0..MAX_MULTI_REGION_ENDPOINTS {
            let name = format!("filling_{index:05}");
            let record = MultiRegionEndpointRecord::new(
                &name,
                format!("{index:08}-0000-0000-0000-000000000000"),
                HOME,
                vec!["us-west-2".to_string()],
                now,
            );
            store
                .multi_region_endpoints
                .insert((HOME.to_string(), name), record);
        }
    }

    // A full store refuses the create whatever Region the request executes in: the
    // Region is the caller's to choose, so it cannot be what the bound is scoped to.
    for region in [HOME, ELSEWHERE, "ap-northeast-3"] {
        let refused = ses
            .create(
                region,
                json!({
                    "EndpointName": "one_too_many",
                    "Details": {"RoutesDetails": [{"Region": "us-west-2"}]},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "LimitExceededException".to_string()),
            "case {region}: a full store refuses the create, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            ses.get(region, "one_too_many").await.status,
            404,
            "case {region}: the refused create must persist nothing"
        );
    }

    // Freeing one slot makes room for exactly one more endpoint.
    assert_eq!(
        ses.delete(HOME, "filling_00000").await.status,
        200,
        "the delete of a stored endpoint must be served"
    );
    ses.complete_deletion(HOME, "filling_00000");
    assert_eq!(
        ses.create(
            ELSEWHERE,
            json!({
                "EndpointName": "one_more",
                "Details": {"RoutesDetails": [{"Region": "us-west-2"}]},
            }),
        )
        .await
        .status,
        200,
        "a freed slot must serve the next create"
    );
}
