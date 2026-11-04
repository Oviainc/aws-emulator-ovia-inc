// SPDX-License-Identifier: CC0-1.0

//! `CreateDataLakeNamespace` and `GetDataLakeNamespace`, driven over the wire (restJson1,
//! signing name `scn`) through the same `MultiServiceRouter` the emulator's front door
//! uses — so every assertion here is about what a real client observes, not about a
//! handler's internals.
//!
//! THE RULE UNDER TEST: a data lake namespace is identified by its instance and its name,
//! and the create PERSISTS one record under that pair — so a read reports exactly the
//! values the create stored, the name's first create owns it (a second one is a conflict,
//! whatever the losing request carries), the same name under another instance is a
//! different namespace, a name the service reserves is refused, and an `instanceId` this
//! account holds no instance for is refused by the gateway's authorization step before any
//! namespace is looked up.
//!
//! The tables span what the rule generalises over: the `DataLakeNamespaceName` form and
//! length (both published bounds), whether a `description` is sent and its length (both
//! published bounds), how many tags the create carries, which reserved prefix a rejected
//! name starts with and whether the name is the prefix itself or extends it, and which
//! instance the call is addressed to.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_supplychain_galaxypublicapigateway::state::MAX_NAMESPACES_PER_INSTANCE;
use service_com_amazonaws_supplychain_galaxypublicapigateway::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service (`scn`), which
/// is how the router picks the service that claims the request. Signature verification is
/// switched off for the test router, so the signature itself is never read.
const SCN_CREDENTIAL: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/scn/aws4_request, SignedHeaders=host, Signature=00";

/// The Region the credential scope above names, and so the Region every ARN these calls
/// mint carries.
const REGION: &str = "us-east-1";

/// The sentence the service refuses a namespace name it reserves with, written out HERE
/// rather than read from the crate: the wording is the requirement, so a drift in the
/// constant the handler answers from fails this suite instead of travelling with it. Its
/// SOURCE is the `name` member of `CreateDataLakeNamespaceRequest` — "Noted you cannot
/// create namespace with name starting with asc, default, scn, aws, amazon, amzn".
const RESERVED_NAME_SENTENCE: &str =
    "Cannot create namespace with name starting with asc, default, scn, aws, amazon, amzn";

/// The sentence a namespace read refuses a name the instance's data lake does not hold
/// with — fixed, and echoing nothing of the request.
const NAMESPACE_NOT_FOUND_SENTENCE: &str = "Namespace not found.";

/// The published bounds of `DataLakeNamespaceName` (`@length(min: 1, max: 50)`).
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 50;

/// The published bounds of `DataLakeNamespaceDescription` (`@length(min: 1, max: 500)`).
const DESCRIPTION_MIN_LENGTH: usize = 1;
const DESCRIPTION_MAX_LENGTH: usize = 500;

/// The emulator under test: one engine, one service, driven through the router.
struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the service was built from, so a test can read the store back and
    /// assert what a wire call released as well as what it reported.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Keep the test independent of how this process was launched: no authenticator,
        // so a request routes on its credential scope alone.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// How many namespace records the store holds for `instance_id`.
    fn stored_namespaces(&self, instance_id: &str) -> usize {
        service_com_amazonaws_supplychain_galaxypublicapigateway::state::service_state(
            &*self.engine,
        )
        .expect("the supplychain store is reachable on this engine")
        .lock()
        .namespace_count(instance_id)
    }

    /// Send one request and return its HTTP status and JSON body.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let body_bytes = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://scn.{REGION}.amazonaws.com{path}"))
            .header("host", format!("scn.{REGION}.amazonaws.com"))
            .header("authorization", SCN_CREDENTIAL)
            .header("content-type", "application/json")
            .body(body_bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path}: body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// A live instance of this account, created through `CreateInstance`.
    async fn given_instance(&self, name: &str) -> String {
        let (status, body) = self
            .send("POST", "/api/instance", Some(json!({ "instanceName": name })))
            .await;
        assert_eq!(
            status, 200,
            "CreateInstance expected 200, got {status} with {body}"
        );
        body.get("instance")
            .and_then(|instance| instance.get("instanceId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateInstance carried no instanceId: {body}"))
            .to_string()
    }

    /// `CreateDataLakeNamespace` for `name` under `instance_id`, carrying `request`.
    async fn create_namespace(
        &self,
        instance_id: &str,
        name: &str,
        request: Value,
    ) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/api/datalake/instance/{instance_id}/namespaces/{name}"),
            Some(request),
        )
        .await
    }

    /// `GetDataLakeNamespace` for `name` under `instance_id`.
    async fn get_namespace(&self, instance_id: &str, name: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!("/api/datalake/instance/{instance_id}/namespaces/{name}"),
            None,
        )
        .await
    }
}

/// The `namespace` member of a success body.
fn namespace_of(case: &str, body: &Value) -> Value {
    body.get("namespace")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carries no `namespace` member: {body}"))
}

/// The error code of a refusal envelope (restJson1 renders it in `__type`).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: refusal carries no `__type`: {body}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of a refusal envelope.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: refusal carries no message: {body}"))
        .to_string()
}

/// A `namespace` member as an `f64`, for the epoch-second timestamps restJson1 renders as
/// JSON numbers.
fn number_of(case: &str, namespace: &Value, member: &str) -> f64 {
    namespace.get(member).and_then(Value::as_f64).unwrap_or_else(|| {
        panic!("case {case:?}: namespace carries no numeric `{member}`: {namespace}")
    })
}

/// The ARN the service mints for a namespace: the instance segment carries the
/// `instanceId` and the rest names the namespace within it, as the modelled
/// `AscResourceArn` pattern requires.
fn expected_arn(instance_id: &str, name: &str) -> String {
    engine::emulator_arn(
        "scn",
        REGION,
        &format!("instance/{instance_id}/namespaces/{name}"),
    )
}

/// How the gateway refuses a call whose `instanceId` names no instance of this account.
fn expected_authorization_failure(instance_id: &str) -> String {
    format!(
        "Authorization Failed (Public). The instanceId {instance_id} does not exist, or does not \
         belong to the account {}",
        engine::emulator_account_id()
    )
}

/// RULE: the create persists one namespace under its instance and name, and answers from
/// that record — so a read reports the identical values, including the ARN, the description
/// the request supplied (and no description when it supplied none) and the creation
/// timestamps.
#[tokio::test]
async fn a_created_namespace_is_reported_back_exactly_as_it_was_persisted() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;

    let longest_name = "n".repeat(NAME_MAX_LENGTH);
    let shortest_description = "d".repeat(DESCRIPTION_MIN_LENGTH);
    let longest_description = "x".repeat(DESCRIPTION_MAX_LENGTH);

    // Rows differ along the name's form and length (both published bounds of
    // `DataLakeNamespaceName`), whether a description is sent and its length (both
    // published bounds of `DataLakeNamespaceDescription`), and how many tags the create
    // carries (none, an empty map, two).
    let rows: Vec<(&str, &str, Option<&str>, Option<Value>)> = vec![
        ("the shortest name, no description", "a", None, None),
        (
            "the longest name",
            longest_name.as_str(),
            Some("fifty characters of name"),
            None,
        ),
        (
            "digits and underscores",
            "orders_2026_q1",
            Some("order lines for 2026 Q1"),
            None,
        ),
        ("only digits", "2026", None, None),
        (
            "the shortest description",
            "min_desc",
            Some(shortest_description.as_str()),
            None,
        ),
        (
            "the longest description",
            "max_desc",
            Some(longest_description.as_str()),
            None,
        ),
        (
            "two tags",
            "tagged_space",
            Some("tagged namespace"),
            Some(json!({ "team": "supply", "tier": "gold" })),
        ),
        ("an empty tag map", "untagged_space", None, Some(json!({}))),
        (
            "a reserved word that is not a prefix",
            "my_aws_orders",
            Some("reserved words only matter at the start"),
            None,
        ),
    ];

    for (case, name, description, tags) in rows {
        assert!(
            name.len() >= NAME_MIN_LENGTH && name.len() <= NAME_MAX_LENGTH,
            "case {case:?}: the row's own name is outside the modelled bounds: {name:?}"
        );
        let mut request = json!({});
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        if let Some(tags) = tags {
            request["tags"] = tags;
        }

        let (status, body) = emulator.create_namespace(&instance, name, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateDataLakeNamespace expected 200, got {status} with {body}"
        );
        let created = namespace_of(case, &body);

        assert_eq!(
            created.get("instanceId").and_then(Value::as_str),
            Some(instance.as_str()),
            "case {case:?}: the namespace must name the instance it was created under, got {created}"
        );
        assert_eq!(
            created.get("name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: the namespace must carry the requested name, got {created}"
        );
        let arn = expected_arn(&instance, name);
        assert_eq!(
            created.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the ARN {arn:?}, got {created}"
        );
        assert_eq!(
            created.get("description").and_then(Value::as_str),
            description,
            "case {case:?}: the description must be the one the request carried, got {created}"
        );
        let created_time = number_of(case, &created, "createdTime");
        assert_eq!(
            number_of(case, &created, "lastModifiedTime"),
            created_time,
            "case {case:?}: a namespace nothing has modified was last modified when it was \
             created, got {created}"
        );
        // The create's own `tags` member is not part of the namespace shape, so a tagged
        // create must not grow the payload a client reads.
        assert!(
            created.get("tags").is_none(),
            "case {case:?}: the modelled namespace carries no `tags` member, got {created}"
        );

        // The round trip: the read answers from the same record, so it is byte-identical.
        let (status, body) = emulator.get_namespace(&instance, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDataLakeNamespace expected 200, got {status} with {body}"
        );
        let fetched = namespace_of(case, &body);
        assert_eq!(
            fetched, created,
            "case {case:?}: the read must report the created namespace unchanged, got {fetched}"
        );

        // A read changes nothing a later reader sees.
        let (_, body) = emulator.get_namespace(&instance, name).await;
        assert_eq!(
            namespace_of(case, &body),
            created,
            "case {case:?}: a second read must report the same namespace, got {body}"
        );
    }
}

/// RULE: the first create of a name owns it — a later create of the same instance and name
/// is refused as a conflict, whatever that request carries, and the stored record is left
/// as the winner wrote it. The name is scoped to the instance, so the same name under
/// another instance is a different namespace.
#[tokio::test]
async fn a_name_is_claimed_once_per_instance() {
    let emulator = Emulator::new();
    let first = emulator.given_instance("namespace-host-one").await;
    let second = emulator.given_instance("namespace-host-two").await;

    let (status, body) = emulator
        .create_namespace(&first, "shipments", json!({ "description": "the winner" }))
        .await;
    assert_eq!(
        status, 200,
        "the first create must succeed, got {status} with {body}"
    );
    let winner = namespace_of("the winner", &body);

    // Rows differ along what the LOSING request carries: nothing, a different
    // description, the same description, and tags.
    let losers: Vec<(&str, Value)> = vec![
        ("an empty retry", json!({})),
        (
            "a different description",
            json!({ "description": "the loser" }),
        ),
        ("the same description", json!({ "description": "the winner" })),
        (
            "a retry carrying tags",
            json!({ "description": "the loser", "tags": { "team": "supply" } }),
        ),
    ];
    for (case, request) in losers {
        let (status, body) = emulator
            .create_namespace(&first, "shipments", request)
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate name expected 409, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ConflictException",
            "case {case:?}: expected a ConflictException envelope, got {body}"
        );
        // The sentence names the namespace whose name was already taken, and nothing else
        // of the request.
        let message = error_message(case, &body);
        assert_eq!(
            message, "Namespace shipments already exist",
            "case {case:?}: unexpected conflict sentence {message:?}"
        );
        assert!(
            !message.contains(first.as_str()),
            "case {case:?}: the sentence must not carry the instanceId: {message:?}"
        );

        let (status, body) = emulator.get_namespace(&first, "shipments").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored namespace must survive the refusal, got {status} with {body}"
        );
        assert_eq!(
            namespace_of(case, &body),
            winner,
            "case {case:?}: a refused create must change nothing, got {body}"
        );
    }

    // The same name under another instance is a different namespace, with its own ARN.
    let (status, body) = emulator
        .create_namespace(&second, "shipments", json!({ "description": "elsewhere" }))
        .await;
    assert_eq!(
        status, 200,
        "the same name under another instance must succeed, got {status} with {body}"
    );
    let elsewhere = namespace_of("another instance", &body);
    assert_eq!(
        elsewhere.get("arn").and_then(Value::as_str),
        Some(expected_arn(&second, "shipments").as_str()),
        "case \"another instance\": the ARN must name the second instance, got {elsewhere}"
    );
    assert_eq!(
        elsewhere.get("description").and_then(Value::as_str),
        Some("elsewhere"),
        "case \"another instance\": the second namespace keeps its own description, got {elsewhere}"
    );
    let (_, body) = emulator.get_namespace(&first, "shipments").await;
    assert_eq!(
        namespace_of("the winner after the second instance", &body),
        winner,
        "the first instance's namespace must be untouched, got {body}"
    );
}

/// RULE: a name starting with a prefix the service reserves cannot be created — the refusal
/// is the modelled `ValidationException` (400) carrying the documented sentence, and nothing
/// is persisted. The rule is on the PREFIX, so a name that merely begins with one is
/// refused too.
#[tokio::test]
async fn a_reserved_name_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;

    // Rows: each reserved prefix exactly as the `name` member documents it (`asc`,
    // `default`, `scn`, `aws`, `amazon`, `amzn`), plus names that EXTEND one — including
    // `ascending`, which no part of the issue cites.
    let rows = [
        "asc",
        "default",
        "scn",
        "aws",
        "amazon",
        "amzn",
        "ascending",
        "defaults",
        "aws_orders",
        "amzn_feed",
        "scn123",
    ];

    for name in rows {
        let (status, body) = emulator
            .create_namespace(&instance, name, json!({ "description": "reserved" }))
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a reserved name expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(name, &body),
            "ValidationException",
            "case {name:?}: expected a ValidationException envelope, got {body}"
        );
        let message = error_message(name, &body);
        assert_eq!(
            message, RESERVED_NAME_SENTENCE,
            "case {name:?}: expected {RESERVED_NAME_SENTENCE:?}, got {message:?}"
        );

        // Nothing came into existence, so the read reports nothing either.
        let (status, body) = emulator.get_namespace(&instance, name).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a refused create must persist nothing, got {status} with {body}"
        );
    }

    // The control: a name that is not reserved is created, so the refusals above are about
    // the prefix and nothing else.
    let (status, body) = emulator
        .create_namespace(&instance, "sales", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case \"sales\": an unreserved name must be created, got {status} with {body}"
    );
}

/// RULE: a read of a name the instance's data lake does not hold is refused with the
/// modelled `ResourceNotFoundException` (404) and the service's fixed not-found sentence,
/// which echoes nothing of the request.
#[tokio::test]
async fn reading_an_absent_namespace_is_a_not_found() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;
    let (status, body) = emulator
        .create_namespace(&instance, "shipments", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "the positive control must be created, got {status} with {body}"
    );

    // Rows differ along the name read: one never created, one that is a prefix of a stored
    // name, one that extends it, and the longest name the model admits.
    let longest = "z".repeat(NAME_MAX_LENGTH);
    let rows = ["orders", "ship", "shipments_2026", longest.as_str()];
    for name in rows {
        let (status, body) = emulator.get_namespace(&instance, name).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(name, &body),
            "ResourceNotFoundException",
            "case {name:?}: expected a ResourceNotFoundException envelope, got {body}"
        );
        let message = error_message(name, &body);
        assert_eq!(
            message, NAMESPACE_NOT_FOUND_SENTENCE,
            "case {name:?}: expected {NAMESPACE_NOT_FOUND_SENTENCE:?}, got {message:?}"
        );
        assert!(
            !message.contains(name) && !message.contains(instance.as_str()),
            "case {name:?}: the sentence must state nothing of the request: {message:?}"
        );
    }
}

/// RULE: an instance-scoped namespace call whose `instanceId` names no instance of this
/// account is refused by the gateway's authorization step — before the namespace is looked
/// up at all — so both halves answer `AccessDeniedException` (403) with the gateway's
/// sentence, and the create persists nothing.
#[tokio::test]
async fn a_call_naming_an_unknown_instance_is_refused_by_authorization() {
    let emulator = Emulator::new();
    let live = emulator.given_instance("namespace-host").await;

    // Rows differ along the well-formed `UUID` addressed: none of them names an instance
    // this account holds.
    let rows = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0123456f-89ab-cdef-0123-456789abcdef",
    ];

    for instance_id in rows {
        for (half, (status, body)) in [
            (
                "create",
                emulator
                    .create_namespace(instance_id, "shipments", json!({ "description": "nope" }))
                    .await,
            ),
            ("read", emulator.get_namespace(instance_id, "shipments").await),
        ] {
            let case = format!("{half} against {instance_id}");
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status} with {body}"
            );
            assert_eq!(
                error_code(&case, &body),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException envelope, got {body}"
            );
            let expected = expected_authorization_failure(instance_id);
            assert_eq!(
                error_message(&case, &body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }

    // The live instance is unaffected: the name is still free there.
    let (status, body) = emulator
        .create_namespace(&live, "shipments", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "the live instance must still accept the name, got {status} with {body}"
    );
}

/// RULE: a namespace a caller created is a namespace of that instance, so what the create
/// persisted is what the rest of the data lake resolves against: a dataset addressed to a
/// created namespace is served, and one addressed to a namespace nobody created is a
/// not-found.
#[tokio::test]
async fn a_created_namespace_is_the_namespace_a_dataset_resolves_against() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;
    let (status, body) = emulator
        .create_namespace(&instance, "orders", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "the namespace must be created first, got {status} with {body}"
    );

    // The smallest schema the model admits (`DataLakeDatasetSchema$fields` is @length
    // 1..500), so the row is about the NAMESPACE and nothing else.
    let dataset = json!({
        "schema": {
            "name": "MinimalSchema",
            "fields": [{ "name": "id", "type": "STRING", "isRequired": true }],
        },
    });

    // Rows differ along the namespace the dataset is addressed to: one a caller created,
    // one the instance comes with, and one nobody created.
    let rows: Vec<(&str, &str, u16)> = vec![
        ("a namespace this caller created", "orders", 200),
        ("a namespace the instance comes with", "default", 200),
        ("a namespace nobody created", "shipments", 404),
    ];

    for (case, namespace, expected) in rows {
        let (status, body) = emulator
            .send(
                "PUT",
                &format!("/api/datalake/instance/{instance}/namespaces/{namespace}/datasets/lines"),
                Some(dataset.clone()),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} creating a dataset, got {status} with {body}"
        );
    }
}

/// RULE: a namespace is addressed by its instance, so it does not outlive it — deleting the
/// instance releases every namespace record keyed by it, and leaves another instance's
/// namespaces alone. (This is also what keeps the per-instance cap dominating the store's
/// growth while instances come and go.)
#[tokio::test]
async fn deleting_an_instance_releases_its_namespaces() {
    let emulator = Emulator::new();
    let doomed = emulator.given_instance("namespace-host-doomed").await;
    let kept = emulator.given_instance("namespace-host-kept").await;

    // Rows differ in count: several namespaces under the instance that is deleted, one
    // under the instance that is not.
    for name in ["orders", "shipments", "forecast"] {
        let (status, body) = emulator.create_namespace(&doomed, name, json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the namespace must be created first, got {status} with {body}"
        );
    }
    let (status, body) = emulator.create_namespace(&kept, "orders", json!({})).await;
    assert_eq!(
        status, 200,
        "the surviving instance's namespace must be created, got {status} with {body}"
    );
    assert_eq!(
        emulator.stored_namespaces(&doomed),
        3,
        "case \"before the delete\": the store must hold the three namespaces created"
    );

    let (status, body) = emulator
        .send("DELETE", &format!("/api/instance/{doomed}"), None)
        .await;
    assert_eq!(
        status, 200,
        "DeleteInstance expected 200, got {status} with {body}"
    );

    assert_eq!(
        emulator.stored_namespaces(&doomed),
        0,
        "case \"after the delete\": the deleted instance's namespace records must be released"
    );
    assert_eq!(
        emulator.stored_namespaces(&kept),
        1,
        "case \"after the delete\": another instance's namespaces must be untouched"
    );
    let (status, body) = emulator.get_namespace(&kept, "orders").await;
    assert_eq!(
        status, 200,
        "the surviving instance's namespace must still read back, got {status} with {body}"
    );
}

/// RULE: the store holds its namespaces for the life of the process and the create is the
/// only thing that adds one, so the count one instance's data lake can hold is capped: the
/// create past the cap is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402) and nothing is added, while another instance is unaffected.
#[tokio::test]
async fn a_create_past_the_cap_is_refused_and_adds_nothing() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;
    let other = emulator.given_instance("namespace-host-other").await;

    for index in 0..MAX_NAMESPACES_PER_INSTANCE {
        let (status, body) = emulator
            .create_namespace(&instance, &format!("space_{index}"), json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case \"filling to the cap\": create {index} expected 200, got {status} with {body}"
        );
    }
    assert_eq!(
        emulator.stored_namespaces(&instance),
        MAX_NAMESPACES_PER_INSTANCE,
        "case \"filling to the cap\": the store must hold exactly the cap"
    );

    // Rows differ in the name the over-cap create asks for: a fresh one is refused, and a
    // name already held is still the conflict it was (the cap does not change that).
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "one past the cap",
            "space_over",
            402,
            "ServiceQuotaExceededException",
        ),
        (
            "a duplicate at the cap",
            "space_0",
            409,
            "ConflictException",
        ),
    ];
    for (case, name, expected_status, expected_code) in rows {
        let (status, body) = emulator.create_namespace(&instance, name, json!({})).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            expected_code,
            "case {case:?}: expected a {expected_code} envelope, got {body}"
        );
        assert_eq!(
            emulator.stored_namespaces(&instance),
            MAX_NAMESPACES_PER_INSTANCE,
            "case {case:?}: a refused create must add nothing"
        );
    }

    // The cap is per instance, so another instance's data lake is still empty and usable.
    let (status, body) = emulator.create_namespace(&other, "orders", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"another instance at the cap\": expected 200, got {status} with {body}"
    );
}

/// RULE (the same claim, under concurrency): the check and the insert happen together, so N
/// creates of one instance and name in flight at once produce exactly ONE winner and N-1
/// conflicts, and the name resolves to the winner afterwards.
#[tokio::test]
async fn concurrent_creates_of_one_name_leave_a_single_winner() {
    let emulator = Emulator::new();
    let instance = emulator.given_instance("namespace-host").await;

    // Each racer sends a different description, so the winner is identifiable.
    let answers = tokio::join!(
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-0" })),
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-1" })),
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-2" })),
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-3" })),
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-4" })),
        emulator.create_namespace(&instance, "contested", json!({ "description": "racer-5" })),
    );
    let answers = [
        answers.0, answers.1, answers.2, answers.3, answers.4, answers.5,
    ];

    let winners: Vec<&(u16, Value)> = answers.iter().filter(|(status, _)| *status == 200).collect();
    assert_eq!(
        winners.len(),
        1,
        "expected exactly one winner, got {}: {answers:?}",
        winners.len()
    );
    for (status, body) in answers.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 409,
            "every loser is a conflict, got {status} with {body}"
        );
        assert_eq!(
            error_code("a loser", body),
            "ConflictException",
            "every loser carries a ConflictException envelope, got {body}"
        );
    }

    let winner = namespace_of("the winner", &winners[0].1);
    let (status, body) = emulator.get_namespace(&instance, "contested").await;
    assert_eq!(
        status, 200,
        "the contested name must resolve after the race, got {status} with {body}"
    );
    assert_eq!(
        namespace_of("after the race", &body),
        winner,
        "the name must resolve to the winner's record, got {body}"
    );
}
