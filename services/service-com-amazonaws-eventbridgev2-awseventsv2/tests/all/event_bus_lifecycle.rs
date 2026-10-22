// SPDX-License-Identifier: CC0-1.0

//! The event-bus lifecycle and event publication, end to end.
//!
//! Two harnesses, because they prove different things:
//!
//! * [`wire`] drives the service through the real `MultiServiceRouter` over
//!   HTTP with rpcv2Cbor bodies, so routing, deserialization, the modelled
//!   constraints, the HTTP statuses and the response bodies are all the
//!   emulator's own.
//! * [`Harness`] calls the typed `Operation::handle` over one shared engine, so
//!   a table can span dozens of inputs per rule without re-encoding a request
//!   each time, and so an error's wire code (`OperationError::shape_name`) can
//!   be asserted directly.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use engine::InMemoryEngine;
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError,
};

use service_com_amazonaws_eventbridgev2_awseventsv2 as eventsv2;
use eventsv2::ops::create_event_bus::CreateEventBus;
use eventsv2::ops::delete_event_bus::DeleteEventBus;
use eventsv2::ops::describe_event_bus::DescribeEventBus;
use eventsv2::ops::list_event_buses::ListEventBuses;
use eventsv2::ops::put_events::PutEvents;
use eventsv2::ops::types::{
    CreateEventBusInput, CreateEventBusOutput, DeduplicationConfiguration, DeleteEventBusInput,
    DeleteEventBusOutput, DescribeEventBusInput, DescribeEventBusOutput, EncryptionConfiguration,
    ListEventBusesInput, ListEventBusesOutput, PutEventsInput, PutEventsOutput,
    PutEventsRequestEntry, PutEventsSystemMetadata, StorageConfiguration,
};

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The region the test harness signs for; it must reach the minted ARN.
const REGION: &str = "us-west-2";

/// Long enough for a due lifecycle transition to have come due. The span itself
/// is the service's own constant, so this tracks it.
fn past_the_lifecycle_span() -> Duration {
    eventsv2::state::LIFECYCLE_SPAN + Duration::from_millis(250)
}

/// A SigV4 `Authorization` header naming `region` and this service's signing
/// name, which is how a request names both.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/events/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// The request metadata of a call signed for `region`: the credential scope is
/// where a request names the region it addresses.
fn parts_signed_for(region: &str) -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/AWSEventsV2/operation/CreateEventBus")
        .header("authorization", authorization(region))
        .header("content-type", "application/cbor")
        .body(())
        .expect("the harness request is well formed")
        .into_parts();
    parts
}

// ---------------------------------------------------------------------------
// The typed harness
// ---------------------------------------------------------------------------

/// One emulator over one shared engine, with a request context every typed call
/// reuses.
struct Harness {
    engine: Arc<InMemoryEngine>,
    definition: eventsv2::ServiceDefinition<InMemoryEngine>,
    callback: EmbeddedEmulator,
    parts: http::request::Parts,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let definition = eventsv2::ServiceDefinition::new(Arc::clone(&engine));
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        let callback = EmbeddedEmulator::from_router(router);
        Self {
            engine,
            definition,
            callback,
            parts: parts_signed_for(REGION),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(&self.parts, self.definition.identity(), self.callback.clone())
    }

    /// A request context naming `region`, for the rules that are about the
    /// region a request addresses rather than about one bus.
    fn ctx_signed_for<'a>(&'a self, parts: &'a http::request::Parts) -> OperationContext<'a> {
        OperationContext::new(parts, self.definition.identity(), self.callback.clone())
    }

    async fn create_signed_for(
        &self,
        region: &str,
        input: CreateEventBusInput,
    ) -> Result<CreateEventBusOutput, eventsv2::ops::create_event_bus::CreateEventBusError> {
        let parts = parts_signed_for(region);
        CreateEventBus::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx_signed_for(&parts))
            .await
    }

    async fn list_signed_for(
        &self,
        region: &str,
        input: ListEventBusesInput,
    ) -> Result<ListEventBusesOutput, eventsv2::ops::list_event_buses::ListEventBusesError> {
        let parts = parts_signed_for(region);
        ListEventBuses::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx_signed_for(&parts))
            .await
    }

    /// The service's own store on the shared engine, for the few assertions that
    /// are about what is STORED rather than about a response — the bounds on the
    /// event log, which no implemented operation reads yet.
    fn store(&self) -> engine::ServiceState<eventsv2::ComAmazonawsEventbridgev2Awseventsv2State> {
        eventsv2::state::service_state(&*self.engine)
            .expect("the in-memory engine always has this service's store")
    }

    async fn create(
        &self,
        input: CreateEventBusInput,
    ) -> Result<CreateEventBusOutput, eventsv2::ops::create_event_bus::CreateEventBusError> {
        CreateEventBus::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx())
            .await
    }

    async fn describe(
        &self,
        arn: &str,
    ) -> Result<DescribeEventBusOutput, eventsv2::ops::describe_event_bus::DescribeEventBusError>
    {
        DescribeEventBus::new(Arc::clone(&self.engine))
            .handle(
                DescribeEventBusInput {
                    event_bus_arn: arn.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn list(
        &self,
        input: ListEventBusesInput,
    ) -> Result<ListEventBusesOutput, eventsv2::ops::list_event_buses::ListEventBusesError> {
        ListEventBuses::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx())
            .await
    }

    async fn put_events(
        &self,
        input: PutEventsInput,
    ) -> Result<PutEventsOutput, eventsv2::ops::put_events::PutEventsError> {
        PutEvents::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx())
            .await
    }

    async fn delete(
        &self,
        arn: &str,
    ) -> Result<DeleteEventBusOutput, eventsv2::ops::delete_event_bus::DeleteEventBusError> {
        DeleteEventBus::new(Arc::clone(&self.engine))
            .handle(
                DeleteEventBusInput {
                    event_bus_arn: arn.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Create a bus and wait out its provisioning span, so it is ACTIVE.
    async fn active_bus(&self, name: &str) -> String {
        let created = self
            .create(CreateEventBusInput {
                name: name.to_string(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {name:?}: CreateEventBus must succeed, got {e:?}"));
        tokio::time::sleep(past_the_lifecycle_span()).await;
        created
            .event_bus_arn
            .unwrap_or_else(|| panic!("case {name:?}: CreateEventBus must report an EventBusArn"))
    }
}

// ---------------------------------------------------------------------------
// The wire harness
// ---------------------------------------------------------------------------

/// One emulator reachable only over HTTP, exactly as the production router
/// mounts it.
struct Wire {
    router: MultiServiceRouter,
}

impl Wire {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(eventsv2::EmulatedService::new(engine));
        Self { router }
    }

    /// Encode `input` as this service's wire protocol, dispatch it through the
    /// router, and hand back the HTTP status and the raw response body.
    async fn call<O: Operation>(&self, input: &O::Input) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(O::INPUT_SCHEMA, input as &dyn SerializableStruct)
            .unwrap_or_else(|e| panic!("{}: encoding the input must succeed, got {e:?}", O::NAME));
        let body = serializer.finish();

        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/AWSEventsV2/operation/{}", O::NAME))
            .header("authorization", authorization(REGION))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor")
            .body(bytes::Bytes::from(body))
            .unwrap_or_else(|e| panic!("{}: the request must be well formed, got {e:?}", O::NAME));

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// The same call, requiring a 2xx and decoding the typed output body.
    async fn ok<O: Operation>(&self, input: &O::Input, case: &str) -> O::Output
    where
        O::Output: DeserializableStruct,
    {
        let (status, body) = self.call::<O>(input).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: {} must answer 200, got {status} with body {body:?}",
            O::NAME
        );
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&body);
        O::Output::deserialize(&mut deserializer).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: {}'s body must decode as its modelled output, got {e:?}",
                O::NAME
            )
        })
    }
}

// ---------------------------------------------------------------------------
// ARN assertions
// ---------------------------------------------------------------------------

/// The id segment of an `EventBusArn`, per the modelled pattern
/// `^arn:aws…:events:<region>:<account>:event-busv2\/<name>\/[a-z0-9]{25}$`.
fn assert_bus_arn(arn: &str, region: &str, name: &str, case: &str) -> String {
    let prefix = format!("arn:aws:events:{region}:{ACCOUNT}:event-busv2/{name}/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: EventBusArn must start with {prefix:?}, got {arn:?}")
    });
    assert_eq!(
        id.len(),
        eventsv2::state::BUS_ID_LEN,
        "case {case:?}: the ARN id segment must be {} characters, got {id:?}",
        eventsv2::state::BUS_ID_LEN
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: the ARN id segment must be lowercase alphanumeric, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The whole documented workflow over the wire, asserting on response BODIES
/// and HTTP statuses: create -> describe -> list -> put -> delete -> describe.
#[tokio::test]
async fn the_event_bus_workflow_runs_over_the_wire() {
    let wire = Wire::new();

    let create = CreateEventBusInput {
        name: "demo-bus".to_string(),
        description: Some("bootstrap demo".to_string()),
        storage_configuration: Some(StorageConfiguration {
            retention_period_in_days: Some(7),
        }),
        ..Default::default()
    };
    let created = wire.ok::<CreateEventBus<InMemoryEngine>>(&create, "create").await;
    let arn = created
        .event_bus_arn
        .clone()
        .expect("case \"create\": CreateEventBus must report an EventBusArn");
    assert_bus_arn(&arn, REGION, "demo-bus", "create");
    assert_eq!(
        created.name.as_deref(),
        Some("demo-bus"),
        "case \"create\": Name must be what was created, got {:?}",
        created.name
    );
    assert_eq!(
        created.description.as_deref(),
        Some("bootstrap demo"),
        "case \"create\": Description must be what was created, got {:?}",
        created.description
    );
    // Provisioning is asynchronous: the bus is reported CREATING, as
    // CreateEventBus documents, and the caller can still see that state.
    assert_eq!(
        created.state.as_deref(),
        Some("CREATING"),
        "case \"create\": State must be CREATING while provisioning, got {:?}",
        created.state
    );
    assert!(
        created.creation_time.is_some(),
        "case \"create\": CreationTime must be reported, got {:?}",
        created.creation_time
    );
    assert_eq!(
        created
            .storage_configuration
            .as_ref()
            .and_then(|config| config.retention_period_in_days),
        Some(7),
        "case \"create\": the requested retention period must be reported, got {:?}",
        created.storage_configuration
    );

    // An event cannot be published to a bus that is still being created.
    let early_put = PutEventsInput {
        event_bus_arn: arn.clone(),
        entries: vec![entry("demo.app", "Demo", None)],
        ..Default::default()
    };
    let (status, _) = wire.call::<PutEvents<InMemoryEngine>>(&early_put).await;
    assert_eq!(
        status, 409,
        "case \"put while CREATING\": PutEvents must answer 409 InvalidStateException, got {status}"
    );

    tokio::time::sleep(past_the_lifecycle_span()).await;

    let described = wire
        .ok::<DescribeEventBus<InMemoryEngine>>(
            &DescribeEventBusInput {
                event_bus_arn: arn.clone(),
            },
            "describe",
        )
        .await;
    assert_eq!(
        described.event_bus_arn.as_deref(),
        Some(arn.as_str()),
        "case \"describe\": EventBusArn must be the bus asked for, got {:?}",
        described.event_bus_arn
    );
    assert_eq!(
        described.name.as_deref(),
        Some("demo-bus"),
        "case \"describe\": Name must be read back from state, got {:?}",
        described.name
    );
    assert_eq!(
        described.description.as_deref(),
        Some("bootstrap demo"),
        "case \"describe\": Description must be read back from state, got {:?}",
        described.description
    );
    assert_eq!(
        described.state.as_deref(),
        Some("ACTIVE"),
        "case \"describe\": State must have settled to ACTIVE, got {:?}",
        described.state
    );
    assert_eq!(
        described.creation_time, created.creation_time,
        "case \"describe\": CreationTime must be the stored one, got {:?}",
        described.creation_time
    );
    assert!(
        described.last_modified_time >= described.creation_time,
        "case \"describe\": LastModifiedTime {:?} must not precede CreationTime {:?}",
        described.last_modified_time,
        described.creation_time
    );

    let listed = wire
        .ok::<ListEventBuses<InMemoryEngine>>(
            &ListEventBusesInput {
                name_prefix: Some("demo".to_string()),
                max_results: 1,
                ..Default::default()
            },
            "list",
        )
        .await;
    let names: Vec<String> = listed
        .event_buses
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|summary| summary.name)
        .collect();
    assert_eq!(
        names,
        vec!["demo-bus".to_string()],
        "case \"list\": the prefixed bus must be listed, got {names:?}"
    );
    assert_eq!(
        listed.next_token, None,
        "case \"list\": no NextToken when nothing remains, got {:?}",
        listed.next_token
    );

    let put = wire
        .ok::<PutEvents<InMemoryEngine>>(
            &PutEventsInput {
                event_bus_arn: arn.clone(),
                entries: vec![
                    entry("demo.app", "OrderPlaced", Some(r#"{"id":1}"#)),
                    entry("demo.app", "OrderShipped", Some(r#"{"id":2}"#)),
                ],
                ..Default::default()
            },
            "put",
        )
        .await;
    assert_eq!(
        put.failed_entry_count, 0,
        "case \"put\": FailedEntryCount must be 0 on full success, got {}",
        put.failed_entry_count
    );
    let ids: Vec<String> = put
        .entries
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|result| result.event_id)
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "case \"put\": one EventId per request entry, got {ids:?}"
    );
    assert_ne!(
        ids[0], ids[1],
        "case \"put\": each entry must get its own EventId, got {ids:?}"
    );

    // A well-formed ARN naming no bus is a 404, never a 501.
    let missing = format!(
        "arn:aws:events:{REGION}:{ACCOUNT}:event-busv2/ghost-bus/aaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    let (status, _) = wire
        .call::<PutEvents<InMemoryEngine>>(&PutEventsInput {
            event_bus_arn: missing.clone(),
            entries: vec![entry("demo.app", "Demo", None)],
            ..Default::default()
        })
        .await;
    assert_eq!(
        status, 404,
        "case \"put to a missing bus\": PutEvents must answer 404, got {status}"
    );

    let (status, body) = wire
        .call::<DeleteEventBus<InMemoryEngine>>(&DeleteEventBusInput {
            event_bus_arn: arn.clone(),
        })
        .await;
    assert_eq!(
        status, 200,
        "case \"delete\": DeleteEventBus must answer 200, got {status}"
    );
    assert!(
        !body.is_empty(),
        "case \"delete\": the empty output still serializes as an empty CBOR map"
    );

    // Deletion is asynchronous, so the bus is DELETING first and gone after.
    let deleting = wire
        .ok::<DescribeEventBus<InMemoryEngine>>(
            &DescribeEventBusInput {
                event_bus_arn: arn.clone(),
            },
            "describe while DELETING",
        )
        .await;
    assert_eq!(
        deleting.state.as_deref(),
        Some("DELETING"),
        "case \"describe while DELETING\": State must be DELETING, got {:?}",
        deleting.state
    );

    tokio::time::sleep(past_the_lifecycle_span()).await;

    let (status, _) = wire
        .call::<DescribeEventBus<InMemoryEngine>>(&DescribeEventBusInput {
            event_bus_arn: arn.clone(),
        })
        .await;
    assert_eq!(
        status, 404,
        "case \"describe after delete\": DescribeEventBus must answer 404, got {status}"
    );

    let after = wire
        .ok::<ListEventBuses<InMemoryEngine>>(
            &ListEventBusesInput {
                max_results: 100,
                ..Default::default()
            },
            "list after delete",
        )
        .await;
    assert_eq!(
        after.event_buses.unwrap_or_default().len(),
        0,
        "case \"list after delete\": the deleted bus must be gone from the enumeration"
    );
}

/// A `PutEventsRequestEntry` with the given source, detail type and detail.
fn entry(source: &str, detail_type: &str, detail: Option<&str>) -> PutEventsRequestEntry {
    PutEventsRequestEntry {
        source: source.to_string(),
        detail_type: detail_type.to_string(),
        detail: detail.map(str::to_string),
        ..Default::default()
    }
}

/// RULE: every created bus is addressed by an ARN of the modelled form
/// `arn:aws:events:<region>:<account>:event-busv2/<name>/<25 lowercase
/// alphanumerics>`, that ARN is stored, and reading it back reports the bus that
/// was created — for ANY name the `EventBusName` grammar admits.
///
/// The rows span that grammar: its shortest and longest admissible names, and
/// each of the extra characters (`.`, `-`, `_`) it allows after the first.
#[tokio::test]
async fn every_created_bus_is_addressable_by_its_minted_arn() {
    // Names drawn from the `EventBusName` member: len 1..256, pattern
    // ^[A-Za-z0-9][\.\-_A-Za-z0-9]*$ (smithy_query shape="EventBusName").
    let long_name = format!("z{}", "9".repeat(255));
    let cases: Vec<String> = vec![
        "a".to_string(),                      // the shortest admissible name
        "9".to_string(),                      // a digit first character
        "demo-bus".to_string(),               // the hyphen
        "Bus.with.dots".to_string(),          // the dot
        "A_b-c.9".to_string(),                // every extra character at once
        long_name.clone(),                    // the length bound, 256
    ];

    let harness = Harness::new();
    let mut ids = Vec::new();
    for name in &cases {
        let created = harness
            .create(CreateEventBusInput {
                name: name.clone(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {name:?}: CreateEventBus must succeed, got {e:?}"));
        let arn = created
            .event_bus_arn
            .clone()
            .unwrap_or_else(|| panic!("case {name:?}: CreateEventBus must report an EventBusArn"));
        ids.push(assert_bus_arn(&arn, REGION, name, name));

        let described = harness
            .describe(&arn)
            .await
            .unwrap_or_else(|e| panic!("case {name:?}: DescribeEventBus must succeed, got {e:?}"));
        assert_eq!(
            described.name.as_ref(),
            Some(name),
            "case {name:?}: the stored Name must read back, got {:?}",
            described.name
        );
        assert_eq!(
            described.event_bus_arn.as_deref(),
            Some(arn.as_str()),
            "case {name:?}: the stored EventBusArn must read back, got {:?}",
            described.event_bus_arn
        );
    }

    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "each bus must get its own ARN id segment, got {ids:?}"
    );
}

/// RULE: a bus name is unique, so a second create of a name already taken is
/// refused as `ResourceAlreadyExistsException` (409), whatever the rest of the
/// request says.
#[tokio::test]
async fn a_name_already_taken_is_refused() {
    let harness = Harness::new();
    let cases: Vec<(&str, CreateEventBusInput)> = vec![
        (
            "same request again",
            CreateEventBusInput {
                name: "taken".to_string(),
                ..Default::default()
            },
        ),
        (
            "different description",
            CreateEventBusInput {
                name: "taken".to_string(),
                description: Some("another".to_string()),
                ..Default::default()
            },
        ),
        (
            "different storage",
            CreateEventBusInput {
                name: "taken".to_string(),
                storage_configuration: Some(StorageConfiguration {
                    retention_period_in_days: Some(30),
                }),
                ..Default::default()
            },
        ),
        (
            "fresh client token",
            CreateEventBusInput {
                name: "taken".to_string(),
                client_token: Some("token-for-the-second-try".to_string()),
                ..Default::default()
            },
        ),
    ];

    harness
        .create(CreateEventBusInput {
            name: "taken".to_string(),
            ..Default::default()
        })
        .await
        .expect("the first create must succeed");

    for (case, input) in cases {
        let error = harness
            .create(input)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: the second create must be refused"));
        assert_eq!(
            error.shape_name(),
            "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {:?}",
            error.shape_name()
        );
        assert_eq!(
            error.http_status(),
            409,
            "case {case:?}: expected HTTP 409, got {}",
            error.http_status()
        );
    }
}

/// RULE: a repeat create carrying a `ClientToken` already used replays the bus
/// that token minted when every other parameter is identical, and is refused
/// with `IdempotentParameterMismatchException` (409) when ANY of them differs.
#[tokio::test]
async fn a_client_token_replays_only_an_identical_request() {
    let token = "a-client-token";
    let original = || CreateEventBusInput {
        name: "idempotent-bus".to_string(),
        description: Some("first".to_string()),
        encryption_configuration: Some(EncryptionConfiguration {
            kms_key_identifier: Some("alias/one".to_string()),
        }),
        storage_configuration: Some(StorageConfiguration {
            retention_period_in_days: Some(5),
        }),
        tags: Some([("team".to_string(), "platform".to_string())].into_iter().collect()),
        client_token: Some(token.to_string()),
    };

    let harness = Harness::new();
    let first = harness
        .create(original())
        .await
        .expect("the first create must succeed");
    let arn = first
        .event_bus_arn
        .clone()
        .expect("the first create must report an EventBusArn");

    let replay = harness
        .create(original())
        .await
        .expect("case \"identical\": a replay must succeed");
    assert_eq!(
        replay.event_bus_arn.as_deref(),
        Some(arn.as_str()),
        "case \"identical\": the replay must report the original bus, got {:?}",
        replay.event_bus_arn
    );

    // One row per member the create carries, each differing in exactly that
    // member while the token stays the same.
    let mut differing: Vec<(&str, CreateEventBusInput)> = Vec::new();
    differing.push(("Name", CreateEventBusInput {
        name: "another-bus".to_string(),
        ..original()
    }));
    differing.push(("Description", CreateEventBusInput {
        description: Some("second".to_string()),
        ..original()
    }));
    differing.push(("Description absent", CreateEventBusInput {
        description: None,
        ..original()
    }));
    differing.push(("EncryptionConfiguration", CreateEventBusInput {
        encryption_configuration: Some(EncryptionConfiguration {
            kms_key_identifier: Some("alias/two".to_string()),
        }),
        ..original()
    }));
    differing.push(("EncryptionConfiguration absent", CreateEventBusInput {
        encryption_configuration: None,
        ..original()
    }));
    differing.push(("StorageConfiguration", CreateEventBusInput {
        storage_configuration: Some(StorageConfiguration {
            retention_period_in_days: Some(6),
        }),
        ..original()
    }));
    differing.push(("Tags", CreateEventBusInput {
        tags: Some([("team".to_string(), "other".to_string())].into_iter().collect()),
        ..original()
    }));
    differing.push(("Tags absent", CreateEventBusInput {
        tags: None,
        ..original()
    }));

    for (case, input) in differing {
        let error = harness
            .create(input)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: a mismatched replay must be refused"));
        assert_eq!(
            error.shape_name(),
            "IdempotentParameterMismatchException",
            "case {case:?}: expected IdempotentParameterMismatchException, got {:?}",
            error.shape_name()
        );
        assert_eq!(
            error.http_status(),
            409,
            "case {case:?}: expected HTTP 409, got {}",
            error.http_status()
        );
    }
}

/// RULE: `ListEventBuses` enumerates every stored bus that passes its filters,
/// in name order, in pages of at most `MaxResults`, and the `NextToken` of a
/// page resumes exactly where that page stopped — so following the tokens yields
/// each matching bus exactly once, for ANY page size.
#[tokio::test]
async fn paging_yields_every_bus_exactly_once_for_any_page_size() {
    let harness = Harness::new();
    let names = [
        "alpha", "beta", "beta.two", "charlie", "delta", "echo", "foxtrot",
    ];
    for name in names {
        harness
            .create(CreateEventBusInput {
                name: name.to_string(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("creating {name:?} must succeed, got {e:?}"));
    }
    let mut sorted: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    sorted.sort();

    // Page sizes spanning the published 1..100 range: both bounds, and sizes
    // smaller than, equal to and larger than the bus count. Row 0 stands for a
    // request that names no `MaxResults` at all (the in-process default), which
    // must page at the member's MODELLED default of 100, not at the range's
    // minimum of 1.
    for page_size in [0_i32, 1, 2, 3, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token = None;
        loop {
            let page = harness
                .list(ListEventBusesInput {
                    max_results: page_size,
                    next_token: next_token.clone(),
                    ..Default::default()
                })
                .await
                .unwrap_or_else(|e| panic!("case {page_size}: ListEventBuses must succeed, got {e:?}"));
            let batch: Vec<String> = page
                .event_buses
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|summary| summary.name)
                .collect();
            assert!(
                batch.len()
                    <= usize::try_from(page_size)
                        .unwrap_or(100)
                        .max(if page_size <= 0 { 100 } else { 1 }),
                "case {page_size}: a page must not exceed MaxResults, got {batch:?}"
            );
            if page_size <= 0 {
                // An absent `MaxResults` takes the modelled default of 100, so
                // this population fits in ONE page and no token is issued.
                assert_eq!(
                    batch.len(),
                    names.len(),
                    "case {page_size}: an absent MaxResults must page at the modelled default \
                     of 100, got {batch:?}"
                );
            }
            seen.extend(batch);
            next_token = page.next_token;
            if next_token.is_none() {
                break;
            }
            assert!(
                seen.len() <= names.len(),
                "case {page_size}: paging must terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, sorted,
            "case {page_size}: paging must yield every bus once, in name order, got {seen:?}"
        );
    }

    // The same rule under `NamePrefix`: only the matching buses, still in order.
    let prefix_cases: Vec<(&str, Vec<&str>)> = vec![
        ("alpha", vec!["alpha"]),
        ("beta", vec!["beta", "beta.two"]),
        ("beta.", vec!["beta.two"]),
        ("z", vec![]),
    ];
    for (prefix, expected) in prefix_cases {
        let page = harness
            .list(ListEventBusesInput {
                name_prefix: Some(prefix.to_string()),
                max_results: 100,
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {prefix:?}: ListEventBuses must succeed, got {e:?}"));
        let got: Vec<String> = page
            .event_buses
            .unwrap_or_default()
            .into_iter()
            .filter_map(|summary| summary.name)
            .collect();
        assert_eq!(
            got,
            expected
                .iter()
                .map(|n| (*n).to_string())
                .collect::<Vec<_>>(),
            "case {prefix:?}: NamePrefix must select exactly the matching buses, got {got:?}"
        );
    }

    // `EventBusAccountId` scopes the list to one owner account.
    let owned = harness
        .list(ListEventBusesInput {
            event_bus_account_id: Some(ACCOUNT.to_string()),
            max_results: 100,
            ..Default::default()
        })
        .await
        .expect("case \"own account\": ListEventBuses must succeed");
    assert_eq!(
        owned.event_buses.unwrap_or_default().len(),
        names.len(),
        "case \"own account\": every bus belongs to the emulator account"
    );
    let foreign = harness
        .list(ListEventBusesInput {
            event_bus_account_id: Some("123456789012".to_string()),
            max_results: 100,
            ..Default::default()
        })
        .await
        .expect("case \"another account\": ListEventBuses must succeed");
    assert_eq!(
        foreign.event_buses.unwrap_or_default().len(),
        0,
        "case \"another account\": no bus belongs to another account"
    );
}

/// RULE: `PutEvents` answers with one result per request entry, in the request's
/// order; a published entry carries its own `EventId` and `SequenceNumber`, an
/// entry the service rejects carries `ErrorCode`/`ErrorMessage` instead, and
/// `FailedEntryCount` is the number of rejected entries — for ANY batch within
/// the modelled 1..100 size range and ANY mix of accepted and rejected entries.
#[tokio::test]
async fn put_events_answers_positionally_and_counts_its_failures() {
    let harness = Harness::new();
    let arn = harness.active_bus("publish-bus").await;

    // Batch sizes across the published `Entries` range (len 1..100): both
    // bounds and sizes between them.
    for size in [1_usize, 2, 5, 99, 100] {
        let entries: Vec<PutEventsRequestEntry> = (0..size)
            .map(|i| entry("demo.app", &format!("Type{i}"), Some(&format!("{{\"i\":{i}}}"))))
            .collect();
        let result = harness
            .put_events(PutEventsInput {
                event_bus_arn: arn.clone(),
                entries,
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {size}: PutEvents must succeed, got {e:?}"));
        let results = result.entries.clone().unwrap_or_default();
        assert_eq!(
            results.len(),
            size,
            "case {size}: one result per request entry, got {}",
            results.len()
        );
        assert_eq!(
            result.failed_entry_count, 0,
            "case {size}: FailedEntryCount must be 0 when every entry is accepted, got {}",
            result.failed_entry_count
        );
        let ids: std::collections::BTreeSet<&String> =
            results.iter().filter_map(|r| r.event_id.as_ref()).collect();
        assert_eq!(
            ids.len(),
            size,
            "case {size}: every accepted entry must carry a distinct EventId, got {ids:?}"
        );
        for (position, r) in results.iter().enumerate() {
            assert_eq!(
                r.success_code.as_deref(),
                Some("PUBLISHED"),
                "case {size}: entry {position} must report PUBLISHED, got {:?}",
                r.success_code
            );
            assert!(
                r.error_code.is_none() && r.error_message.is_none(),
                "case {size}: entry {position} succeeded, so it carries no error, got {r:?}"
            );
            assert!(
                r.sequence_number.is_some(),
                "case {size}: entry {position} must carry a SequenceNumber, got {r:?}"
            );
        }
    }

    // A mixed batch: the `aws.` prefix is service-reserved for Source, so those
    // positions fail and the rest publish. One row per layout of the mix.
    let mixed: Vec<(&str, Vec<&str>)> = vec![
        ("only reserved", vec!["aws.ec2"]),
        ("reserved first", vec!["aws.ec2", "demo.app"]),
        ("reserved last", vec!["demo.app", "aws.s3"]),
        (
            "reserved in the middle",
            vec!["demo.app", "aws.events", "demo.other"],
        ),
        ("none reserved", vec!["demo.app", "demo.other"]),
    ];
    for (case, sources) in mixed {
        let expected_failures = sources
            .iter()
            .filter(|source| source.starts_with("aws."))
            .count();
        let result = harness
            .put_events(PutEventsInput {
                event_bus_arn: arn.clone(),
                entries: sources
                    .iter()
                    .map(|source| entry(source, "Mixed", None))
                    .collect(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {case:?}: PutEvents must succeed, got {e:?}"));
        assert_eq!(
            usize::try_from(result.failed_entry_count).unwrap_or(usize::MAX),
            expected_failures,
            "case {case:?}: FailedEntryCount must count the rejected entries, got {}",
            result.failed_entry_count
        );
        let results = result.entries.clone().unwrap_or_default();
        for (position, source) in sources.iter().enumerate() {
            let r = &results[position];
            if source.starts_with("aws.") {
                assert_eq!(
                    r.error_code.as_deref(),
                    Some("InvalidArgument"),
                    "case {case:?}: entry {position} is reserved, so it must carry an ErrorCode, got {r:?}"
                );
                assert!(
                    r.event_id.is_none(),
                    "case {case:?}: entry {position} was not published, so it has no EventId, got {r:?}"
                );
            } else {
                assert!(
                    r.event_id.is_some() && r.error_code.is_none(),
                    "case {case:?}: entry {position} must be published, got {r:?}"
                );
            }
        }
    }

    // Published events keep accumulating on the bus, so sequence numbers across
    // calls never repeat.
    let first = harness
        .put_events(PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![entry("demo.app", "One", None)],
            ..Default::default()
        })
        .await
        .expect("case \"sequence\": PutEvents must succeed");
    let second = harness
        .put_events(PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![entry("demo.app", "Two", None)],
            ..Default::default()
        })
        .await
        .expect("case \"sequence\": PutEvents must succeed");
    let sequence_of = |out: &PutEventsOutput| {
        out.entries
            .clone()
            .unwrap_or_default()
            .first()
            .and_then(|r| r.sequence_number.clone())
            .and_then(|s| s.parse::<u64>().ok())
    };
    let (a, b) = (sequence_of(&first), sequence_of(&second));
    assert!(
        a.is_some() && b > a,
        "case \"sequence\": SequenceNumber must advance with the log, got {a:?} then {b:?}"
    );
}

/// RULE: an entry's `Time` is the instant it occurred, defaulting to the instant
/// the service receives it; a `Timestamp` carries no modelled range, so the
/// conversion is TOTAL — every value of the member's `i64` seconds field either
/// publishes the entry or is refused in that entry's own position with an
/// `ErrorCode`, and none of them unwinds the handler or silently substitutes an
/// instant the caller never sent.
#[tokio::test]
async fn any_entry_time_either_publishes_or_is_refused_in_its_own_position() {
    let harness = Harness::new();
    let arn = harness.active_bus("time-bus").await;

    // Rows spanning the `i64` seconds field of the modelled `Timestamp`: both
    // extremes, the epoch, ordinary times either side of it, and the member
    // omitted altogether.
    let cases: Vec<(&str, Option<i64>, bool)> = vec![
        ("absent", None, true),
        ("epoch", Some(0), true),
        ("recent past", Some(1_700_000_000), true),
        ("distant past", Some(-1_700_000_000), true),
        ("i64::MAX", Some(i64::MAX), false),
        ("i64::MIN", Some(i64::MIN), false),
    ];

    for (case, secs, must_publish) in cases {
        let result = harness
            .put_events(PutEventsInput {
                event_bus_arn: arn.clone(),
                entries: vec![PutEventsRequestEntry {
                    source: "demo.app".to_string(),
                    detail_type: "Timed".to_string(),
                    time: secs.map(aws_smithy_types::DateTime::from_secs),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {case:?}: PutEvents must answer, got {e:?}"));
        let results = result.entries.clone().unwrap_or_default();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: one result per request entry, got {results:?}"
        );
        let answered = &results[0];
        let published = answered.event_id.is_some();

        // Totality, for EVERY row: exactly one of the two halves of the result
        // shape is filled, and `FailedEntryCount` agrees with it.
        assert_eq!(
            published,
            answered.error_code.is_none(),
            "case {case:?}: a result is published OR refused, never both or neither, got {answered:?}"
        );
        assert_eq!(
            result.failed_entry_count,
            i32::from(!published),
            "case {case:?}: FailedEntryCount must agree with the result, got {result:?}"
        );
        if published {
            assert_eq!(
                answered.success_code.as_deref(),
                Some("PUBLISHED"),
                "case {case:?}: a published entry reports PUBLISHED, got {answered:?}"
            );
        } else {
            assert_eq!(
                answered.error_code.as_deref(),
                Some("InvalidArgument"),
                "case {case:?}: a refused Time is an argument error, got {answered:?}"
            );
        }

        // A time the service can represent must publish; the two extremes are
        // allowed to go either way, since whether they are representable is a
        // platform fact rather than part of the rule.
        if must_publish {
            assert!(
                published,
                "case {case:?}: an ordinary Time must publish, got {answered:?}"
            );
        }
    }
}

/// The same entry carrying a caller-supplied `DeduplicationId`.
fn with_deduplication_id(mut base: PutEventsRequestEntry, id: &str) -> PutEventsRequestEntry {
    base.system_metadata = Some(PutEventsSystemMetadata {
        deduplication_id: Some(id.to_string()),
        ..Default::default()
    });
    base
}

/// The same entry carrying `Resources`, in the order given.
fn with_resources(mut base: PutEventsRequestEntry, resources: &[&str]) -> PutEventsRequestEntry {
    base.resources = Some(resources.iter().map(|arn| arn.to_string()).collect());
    base
}

/// The same entry carrying an explicit `Time`.
fn at_time(mut base: PutEventsRequestEntry, secs: i64) -> PutEventsRequestEntry {
    base.time = Some(aws_smithy_types::DateTime::from_secs(secs));
    base
}

/// One row of the deduplication table: two entries published in turn on a bus of
/// their own, and whether the second must come back as a duplicate of the first.
struct DedupCase {
    case: &'static str,
    /// Whether the request carries `DeduplicationConfiguration` naming
    /// `CONTENT_BASED` (the only value `DeduplicationType` declares, per
    /// `smithy_query(query="shape",shape="DeduplicationType")`).
    content_based: bool,
    first: PutEventsRequestEntry,
    second: PutEventsRequestEntry,
    duplicate: bool,
}

/// RULE: a publish request deduplicates by whatever its
/// `DeduplicationConfiguration` selects — a hash of the event content under
/// `CONTENT_BASED`, otherwise each entry's caller-supplied `DeduplicationId`,
/// and by nothing at all when the request offers neither — and an entry whose key
/// is already on the bus is reported `DEDUPLICATED` carrying the `EventId` and
/// `SequenceNumber` of the event that first carried that key, instead of being
/// published a second time.
///
/// Source: `DeduplicationType`'s own documentation — "How duplicate events are
/// detected: by a hash of the event content (CONTENT_BASED). To deduplicate by a
/// caller-supplied token instead, omit DeduplicationConfiguration and set
/// DeduplicationId on each entry."
#[tokio::test]
async fn deduplication_keys_on_whatever_the_request_selects() {
    let harness = Harness::new();

    let base = || entry("demo.app", "Deduplicated", Some("{\"n\":1}"));
    // Rows differ along WHAT the two publishes vary: the selected mode, and
    // each member of the event content and of the system metadata.
    let cases = vec![
        DedupCase {
            case: "content-based, identical content",
            content_based: true,
            first: base(),
            second: base(),
            duplicate: true,
        },
        DedupCase {
            case: "content-based, different Detail",
            content_based: true,
            first: base(),
            second: entry("demo.app", "Deduplicated", Some("{\"n\":2}")),
            duplicate: false,
        },
        DedupCase {
            case: "content-based, absent Detail against a present one",
            content_based: true,
            first: base(),
            second: entry("demo.app", "Deduplicated", None),
            duplicate: false,
        },
        DedupCase {
            case: "content-based, different Source",
            content_based: true,
            first: base(),
            second: entry("other.app", "Deduplicated", Some("{\"n\":1}")),
            duplicate: false,
        },
        DedupCase {
            case: "content-based, different DetailType",
            content_based: true,
            first: base(),
            second: entry("demo.app", "Other", Some("{\"n\":1}")),
            duplicate: false,
        },
        DedupCase {
            case: "content-based, identical Resources",
            content_based: true,
            first: with_resources(base(), &["arn:aws:sqs:us-west-2:000000000000:a"]),
            second: with_resources(base(), &["arn:aws:sqs:us-west-2:000000000000:a"]),
            duplicate: true,
        },
        DedupCase {
            case: "content-based, more Resources",
            content_based: true,
            first: with_resources(base(), &["arn:aws:sqs:us-west-2:000000000000:a"]),
            second: with_resources(
                base(),
                &[
                    "arn:aws:sqs:us-west-2:000000000000:a",
                    "arn:aws:sqs:us-west-2:000000000000:b",
                ],
            ),
            duplicate: false,
        },
        DedupCase {
            case: "content-based, Resources in another order",
            content_based: true,
            first: with_resources(
                base(),
                &[
                    "arn:aws:sqs:us-west-2:000000000000:a",
                    "arn:aws:sqs:us-west-2:000000000000:b",
                ],
            ),
            second: with_resources(
                base(),
                &[
                    "arn:aws:sqs:us-west-2:000000000000:b",
                    "arn:aws:sqs:us-west-2:000000000000:a",
                ],
            ),
            duplicate: false,
        },
        DedupCase {
            // The content is the key, so the token the entries carry is not.
            case: "content-based, identical content under different tokens",
            content_based: true,
            first: with_deduplication_id(base(), "one"),
            second: with_deduplication_id(base(), "two"),
            duplicate: true,
        },
        DedupCase {
            // `Time` "defaults to the time the service receives the event when
            // omitted", so it cannot be part of the content hash.
            case: "content-based, identical content at different times",
            content_based: true,
            first: at_time(base(), 1_700_000_000),
            second: at_time(base(), 1_700_000_001),
            duplicate: true,
        },
        DedupCase {
            case: "token mode, same token with different content",
            content_based: false,
            first: with_deduplication_id(base(), "token-a"),
            second: with_deduplication_id(
                entry("other.app", "Other", Some("{\"n\":9}")),
                "token-a",
            ),
            duplicate: true,
        },
        DedupCase {
            case: "token mode, different tokens with identical content",
            content_based: false,
            first: with_deduplication_id(base(), "token-a"),
            second: with_deduplication_id(base(), "token-b"),
            duplicate: false,
        },
        DedupCase {
            // Neither mode selected: nothing is deduplicated.
            case: "no configuration and no token, identical content",
            content_based: false,
            first: base(),
            second: base(),
            duplicate: false,
        },
    ];

    // One bus per row, so no row can observe another's log. They are created up
    // front and the provisioning span is waited out once.
    let mut buses = Vec::new();
    for index in 0..cases.len() {
        let created = harness
            .create(CreateEventBusInput {
                name: format!("dedup-bus-{index}"),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("row {index}: CreateEventBus must succeed, got {e:?}"));
        buses.push(
            created
                .event_bus_arn
                .unwrap_or_else(|| panic!("row {index}: an EventBusArn must be reported")),
        );
    }
    tokio::time::sleep(past_the_lifecycle_span()).await;

    for (row, expected) in cases.into_iter().enumerate() {
        let case = expected.case;
        let arn = buses[row].clone();
        let configuration = expected.content_based.then(|| DeduplicationConfiguration {
            deduplication_type: "CONTENT_BASED".to_string(),
        });
        let publish = |entry: PutEventsRequestEntry| PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![entry],
            deduplication_configuration: configuration.clone(),
        };

        let first = harness
            .put_events(publish(expected.first))
            .await
            .unwrap_or_else(|e| panic!("case {case:?}: the first publish must succeed, got {e:?}"));
        let first_result = first
            .entries
            .clone()
            .unwrap_or_default()
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: one result per entry, got {first:?}"));
        assert_eq!(
            first_result.success_code.as_deref(),
            Some("PUBLISHED"),
            "case {case:?}: the first entry is new, got {first_result:?}"
        );

        let second = harness
            .put_events(publish(expected.second))
            .await
            .unwrap_or_else(|e| panic!("case {case:?}: the second publish must succeed, got {e:?}"));
        let second_result = second
            .entries
            .clone()
            .unwrap_or_default()
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: one result per entry, got {second:?}"));
        assert_eq!(
            second.failed_entry_count, 0,
            "case {case:?}: neither publishing nor deduplicating is a failure, got {second:?}"
        );

        if expected.duplicate {
            assert_eq!(
                second_result.success_code.as_deref(),
                Some("DEDUPLICATED"),
                "case {case:?}: the second entry repeats the first's key, got {second_result:?}"
            );
            assert_eq!(
                second_result.event_id, first_result.event_id,
                "case {case:?}: a deduplicated entry reports the original EventId {:?}, got {:?}",
                first_result.event_id, second_result.event_id
            );
            assert_eq!(
                second_result.sequence_number, first_result.sequence_number,
                "case {case:?}: a deduplicated entry reports the original SequenceNumber {:?}, \
                 got {:?}",
                first_result.sequence_number, second_result.sequence_number
            );
        } else {
            assert_eq!(
                second_result.success_code.as_deref(),
                Some("PUBLISHED"),
                "case {case:?}: the second entry carries a key of its own, got {second_result:?}"
            );
            assert_ne!(
                second_result.event_id, first_result.event_id,
                "case {case:?}: a published entry gets its own EventId, got {:?}",
                second_result.event_id
            );
        }

        // Whatever the mode, the stored log holds exactly the events that were
        // published — one row's duplicate is never a second stored event.
        let stored = {
            let store = harness.store();
            let state = store.lock();
            let bus = state
                .event_buses
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case:?}: the bus is still stored"));
            bus.events.len()
        };
        assert_eq!(
            stored,
            if expected.duplicate { 1 } else { 2 },
            "case {case:?}: the stored log must hold only the published events, got {stored}"
        );
    }
}

/// RULE: an ARN that names no stored bus is `ResourceNotFoundException` (404) on
/// every bus-addressed operation, whatever else the request carries — and that
/// includes a bus whose deletion has completed.
#[tokio::test]
async fn an_unknown_arn_is_not_found_on_every_bus_addressed_operation() {
    let harness = Harness::new();
    let settled = harness.active_bus("settled-bus").await;
    harness
        .delete(&settled)
        .await
        .expect("DeleteEventBus must succeed");
    tokio::time::sleep(past_the_lifecycle_span()).await;

    // Rows: a never-created bus, a bus in another region, a bus whose name
    // exists nowhere, and the ARN of a bus whose deletion has completed.
    let never = format!("arn:aws:events:{REGION}:{ACCOUNT}:event-busv2/never/bbbbbbbbbbbbbbbbbbbbbbbbb");
    let other_region =
        format!("arn:aws:events:eu-central-1:{ACCOUNT}:event-busv2/settled-bus/ccccccccccccccccccccccccc");
    let cases = [
        ("never created", never.as_str()),
        ("another region", other_region.as_str()),
        ("deleted", settled.as_str()),
    ];

    for (case, arn) in cases {
        let describe_error = harness
            .describe(arn)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: DescribeEventBus must be refused"));
        assert_eq!(
            describe_error.shape_name(),
            "ResourceNotFoundException",
            "case {case:?}: DescribeEventBus expected ResourceNotFoundException, got {:?}",
            describe_error.shape_name()
        );
        assert_eq!(
            describe_error.http_status(),
            404,
            "case {case:?}: DescribeEventBus expected HTTP 404, got {}",
            describe_error.http_status()
        );

        let put_error = harness
            .put_events(PutEventsInput {
                event_bus_arn: arn.to_string(),
                entries: vec![entry("demo.app", "Demo", None)],
                ..Default::default()
            })
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: PutEvents must be refused"));
        assert_eq!(
            put_error.shape_name(),
            "ResourceNotFoundException",
            "case {case:?}: PutEvents expected ResourceNotFoundException, got {:?}",
            put_error.shape_name()
        );
        assert_eq!(
            put_error.http_status(),
            404,
            "case {case:?}: PutEvents expected HTTP 404, got {}",
            put_error.http_status()
        );

        let delete_error = harness
            .delete(arn)
            .await
            .err()
            .unwrap_or_else(|| panic!("case {case:?}: DeleteEventBus must be refused"));
        assert_eq!(
            delete_error.shape_name(),
            "ResourceNotFoundException",
            "case {case:?}: DeleteEventBus expected ResourceNotFoundException, got {:?}",
            delete_error.shape_name()
        );
        assert_eq!(
            delete_error.http_status(),
            404,
            "case {case:?}: DeleteEventBus expected HTTP 404, got {}",
            delete_error.http_status()
        );
    }
}

/// RULE: a bus that has not finished its lifecycle transition cannot accept
/// events, and a repeated delete of a bus already on its way out is the same
/// request (`@idempotent`), not a second one.
#[tokio::test]
async fn a_bus_outside_active_refuses_events_and_a_repeat_delete_is_the_same_request() {
    let harness = Harness::new();
    let created = harness
        .create(CreateEventBusInput {
            name: "transient-bus".to_string(),
            ..Default::default()
        })
        .await
        .expect("CreateEventBus must succeed");
    let arn = created
        .event_bus_arn
        .clone()
        .expect("CreateEventBus must report an EventBusArn");

    // CREATING, then DELETING: both reject events with the same modelled error.
    let creating = harness
        .put_events(PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![entry("demo.app", "Demo", None)],
            ..Default::default()
        })
        .await
        .expect_err("case \"CREATING\": PutEvents must be refused");
    assert_eq!(
        creating.shape_name(),
        "InvalidStateException",
        "case \"CREATING\": expected InvalidStateException, got {:?}",
        creating.shape_name()
    );
    assert_eq!(
        creating.http_status(),
        409,
        "case \"CREATING\": expected HTTP 409, got {}",
        creating.http_status()
    );

    tokio::time::sleep(past_the_lifecycle_span()).await;
    harness
        .delete(&arn)
        .await
        .expect("DeleteEventBus must succeed");
    harness
        .delete(&arn)
        .await
        .expect("case \"repeat delete\": an idempotent retry must succeed");

    let deleting = harness
        .put_events(PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![entry("demo.app", "Demo", None)],
            ..Default::default()
        })
        .await
        .expect_err("case \"DELETING\": PutEvents must be refused");
    assert_eq!(
        deleting.shape_name(),
        "InvalidStateException",
        "case \"DELETING\": expected InvalidStateException, got {:?}",
        deleting.shape_name()
    );

    let described = harness
        .describe(&arn)
        .await
        .expect("case \"DELETING\": DescribeEventBus must still report the bus");
    assert_eq!(
        described.state.as_deref(),
        Some("DELETING"),
        "case \"DELETING\": State must be DELETING, got {:?}",
        described.state
    );

    // And the repeat did not restart the span: the bus still disappears one
    // span after the first delete.
    tokio::time::sleep(past_the_lifecycle_span()).await;
    assert!(
        harness.describe(&arn).await.is_err(),
        "case \"repeat delete\": the bus must still disappear when its span elapses"
    );
}

/// RULE: a stored retention period is reported back with a
/// `RetentionWindowStartTime` — the earliest instant stored events are still
/// available — which never precedes the bus's own creation.
#[tokio::test]
async fn a_retention_period_is_reported_with_its_window_start() {
    let harness = Harness::new();
    // Rows across the published `RetentionPeriodInDays` range (1..365): both
    // bounds and a value between them.
    for days in [1_i32, 30, 365] {
        let name = format!("retention-{days}");
        let created = harness
            .create(CreateEventBusInput {
                name: name.clone(),
                storage_configuration: Some(StorageConfiguration {
                    retention_period_in_days: Some(days),
                }),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {days}: CreateEventBus must succeed, got {e:?}"));
        let arn = created
            .event_bus_arn
            .clone()
            .unwrap_or_else(|| panic!("case {days}: CreateEventBus must report an EventBusArn"));

        let described = harness
            .describe(&arn)
            .await
            .unwrap_or_else(|e| panic!("case {days}: DescribeEventBus must succeed, got {e:?}"));
        let storage = described
            .storage_configuration
            .clone()
            .unwrap_or_else(|| panic!("case {days}: StorageConfiguration must be reported"));
        assert_eq!(
            storage.retention_period_in_days,
            Some(days),
            "case {days}: the stored retention period must read back, got {:?}",
            storage.retention_period_in_days
        );
        assert_eq!(
            storage.retention_window_start_time, described.creation_time,
            "case {days}: the window cannot open before the bus existed, got {:?}",
            storage.retention_window_start_time
        );
    }

    // A bus created without one reports no storage configuration at all.
    let created = harness
        .create(CreateEventBusInput {
            name: "no-retention".to_string(),
            ..Default::default()
        })
        .await
        .expect("case \"absent\": CreateEventBus must succeed");
    let described = harness
        .describe(created.event_bus_arn.as_deref().expect("an ARN"))
        .await
        .expect("case \"absent\": DescribeEventBus must succeed");
    assert!(
        described.storage_configuration.is_none(),
        "case \"absent\": nothing was configured, so nothing is reported, got {:?}",
        described.storage_configuration
    );
}

/// RULE: the stored event log holds only what the reported retention window
/// covers — an event whose receipt instant is before `RetentionWindowStartTime`
/// has expired from retention and is no longer stored — and never more than the
/// emulator's own cap, which it keeps by forgetting the oldest events rather than
/// by failing a publish.
#[tokio::test]
async fn the_stored_event_log_stays_within_retention_and_the_cap() {
    let harness = Harness::new();

    // Retention expiry. The clock cannot be moved, so the bus is aged in the
    // store instead: created four days ago, retention one day, with events
    // received two days ago (outside the window) and just now (inside it).
    let created = harness
        .create(CreateEventBusInput {
            name: "expiring-bus".to_string(),
            storage_configuration: Some(StorageConfiguration {
                retention_period_in_days: Some(1),
            }),
            ..Default::default()
        })
        .await
        .expect("CreateEventBus must succeed");
    let arn = created
        .event_bus_arn
        .clone()
        .expect("CreateEventBus must report an EventBusArn");
    tokio::time::sleep(past_the_lifecycle_span()).await;
    harness
        .put_events(PutEventsInput {
            event_bus_arn: arn.clone(),
            entries: vec![
                entry("demo.app", "Old", None),
                entry("demo.app", "AlsoOld", None),
                entry("demo.app", "Fresh", None),
            ],
            ..Default::default()
        })
        .await
        .expect("PutEvents must succeed");

    const DAY: Duration = Duration::from_secs(86_400);
    {
        let store = harness.store();
        let mut state = store.lock();
        let bus = state
            .event_buses
            .get_mut(&arn)
            .expect("the bus is in the store");
        bus.creation_time -= DAY * 4;
        for stored in bus.events.iter_mut().take(2) {
            stored.received_at -= DAY * 2;
        }
    }

    // Any call settles what the clock owes, retention included.
    let described = harness
        .describe(&arn)
        .await
        .expect("DescribeEventBus must succeed");
    let window_start = described
        .storage_configuration
        .as_ref()
        .and_then(|config| config.retention_window_start_time)
        .expect("a retention window start must be reported");
    {
        let store = harness.store();
        let state = store.lock();
        let bus = state.event_buses.get(&arn).expect("the bus is still stored");
        let detail_types: Vec<&str> = bus
            .events
            .iter()
            .map(|stored| stored.detail_type.as_str())
            .collect();
        assert_eq!(
            detail_types,
            vec!["Fresh"],
            "case \"retention\": only events inside the reported window \
             ({window_start:?}) stay stored, got {detail_types:?}"
        );
        assert!(
            bus.events.iter().all(|stored| {
                eventsv2::state::to_date_time(stored.received_at) >= window_start
            }),
            "case \"retention\": every stored event must sit inside the reported window \
             {window_start:?}"
        );
    }

    // The cap. Publishing past it keeps succeeding and keeps the NEWEST events.
    let capped = harness.active_bus("capped-bus").await;
    let cap = eventsv2::state::MAX_STORED_EVENTS_PER_BUS;
    let batch = 100; // the modelled maximum `Entries` length
    let calls = cap / batch + 1;
    for call in 0..calls {
        let result = harness
            .put_events(PutEventsInput {
                event_bus_arn: capped.clone(),
                entries: (0..batch)
                    .map(|i| entry("demo.app", &format!("Event{call}-{i}"), None))
                    .collect(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case \"cap\": call {call} must succeed, got {e:?}"));
        assert_eq!(
            result.failed_entry_count, 0,
            "case \"cap\": reaching the cap must not fail a publish, got {result:?}"
        );
    }
    {
        let store = harness.store();
        let state = store.lock();
        let bus = state
            .event_buses
            .get(&capped)
            .expect("the bus is in the store");
        assert_eq!(
            bus.events.len(),
            cap,
            "case \"cap\": the stored log must stay at the cap, got {}",
            bus.events.len()
        );
        let published = (calls * batch) as u64;
        assert_eq!(
            bus.events.first().map(|stored| stored.sequence_number),
            Some(published - cap as u64 + 1),
            "case \"cap\": the oldest events are the ones forgotten, got {:?}",
            bus.events.first().map(|stored| stored.sequence_number)
        );
        assert_eq!(
            bus.events.last().map(|stored| stored.sequence_number),
            Some(published),
            "case \"cap\": the newest event must still be stored, got {:?}",
            bus.events.last().map(|stored| stored.sequence_number)
        );
        assert!(
            bus.deduplicated_events.len() <= bus.events.len(),
            "case \"cap\": the deduplication index must not outlive the log, got {} index entries \
             for {} events",
            bus.deduplicated_events.len(),
            bus.events.len()
        );
    }

    // The deduplication index is under the SAME bound as the log, and never
    // names an event the log has forgotten: publish past the cap with
    // content-based deduplication (so every entry adds an index entry), then
    // repeat the first batch's content. Its event is long gone, so the repeat
    // must PUBLISH again rather than report a DEDUPLICATED event nothing holds.
    let indexed = harness.active_bus("dedup-capped-bus").await;
    let content_based = Some(DeduplicationConfiguration {
        deduplication_type: "CONTENT_BASED".to_string(),
    });
    let first_detail = "{\"n\":0-0}".to_string();
    for call in 0..calls {
        let result = harness
            .put_events(PutEventsInput {
                event_bus_arn: indexed.clone(),
                entries: (0..batch)
                    .map(|i| entry("demo.app", "Indexed", Some(&format!("{{\"n\":{call}-{i}}}"))))
                    .collect(),
                deduplication_configuration: content_based.clone(),
            })
            .await
            .unwrap_or_else(|e| panic!("case \"indexed cap\": call {call} must succeed, got {e:?}"));
        assert_eq!(
            result.failed_entry_count, 0,
            "case \"indexed cap\": reaching the cap must not fail a publish, got {result:?}"
        );
    }
    {
        let store = harness.store();
        let state = store.lock();
        let bus = state
            .event_buses
            .get(&indexed)
            .expect("the bus is in the store");
        assert_eq!(
            bus.events.len(),
            cap,
            "case \"indexed cap\": the stored log must stay at the cap, got {}",
            bus.events.len()
        );
        assert!(
            bus.deduplicated_events.len() <= cap,
            "case \"indexed cap\": the deduplication index must stay within the log's cap of \
             {cap}, got {} entries",
            bus.deduplicated_events.len()
        );
    }
    let repeat = harness
        .put_events(PutEventsInput {
            event_bus_arn: indexed.clone(),
            entries: vec![entry("demo.app", "Indexed", Some(&first_detail))],
            deduplication_configuration: content_based,
        })
        .await
        .expect("case \"indexed cap\": the repeat must succeed");
    let repeated = repeat
        .entries
        .clone()
        .unwrap_or_default()
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case \"indexed cap\": one result per entry, got {repeat:?}"));
    assert_eq!(
        repeated.success_code.as_deref(),
        Some("PUBLISHED"),
        "case \"indexed cap\": a key the log has forgotten must not deduplicate, got {repeated:?}"
    );
}

/// RULE: an account holds at most `MAX_EVENT_BUSES` event buses PER REGION; a
/// create past that quota is refused with the modelled `LimitExceededException`
/// (400), another region still has room, and deleting a bus frees a slot again.
#[tokio::test]
async fn the_account_event_bus_quota_is_enforced() {
    let harness = Harness::new();
    let quota = eventsv2::state::MAX_EVENT_BUSES;
    let mut arns = Vec::new();
    for index in 0..quota {
        let created = harness
            .create(CreateEventBusInput {
                name: format!("quota-bus-{index}"),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("case {index}: CreateEventBus must succeed, got {e:?}"));
        arns.push(
            created
                .event_bus_arn
                .clone()
                .unwrap_or_else(|| panic!("case {index}: an EventBusArn must be reported")),
        );
    }

    let error = harness
        .create(CreateEventBusInput {
            name: "one-too-many".to_string(),
            ..Default::default()
        })
        .await
        .expect_err("case \"past the quota\": the create must be refused");
    assert_eq!(
        error.shape_name(),
        "LimitExceededException",
        "case \"past the quota\": expected LimitExceededException, got {:?}",
        error.shape_name()
    );
    assert_eq!(
        error.http_status(),
        400,
        "case \"past the quota\": expected HTTP 400, got {}",
        error.http_status()
    );

    // The quota is per account AND Region, so another region still has room.
    harness
        .create_signed_for(
            "eu-west-1",
            CreateEventBusInput {
                name: "one-too-many".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("case \"another region at the quota\": the create must succeed");

    // Freeing a slot lets the next create through.
    harness
        .delete(&arns[0])
        .await
        .expect("DeleteEventBus must succeed");
    tokio::time::sleep(past_the_lifecycle_span()).await;
    harness
        .create(CreateEventBusInput {
            name: "one-too-many".to_string(),
            ..Default::default()
        })
        .await
        .expect("case \"after freeing a slot\": the create must succeed");
}

/// RULE: an event-bus name is unique per account AND REGION — the same name is
/// free once in every region a request names, each bus's ARN carries the region
/// it was created in, and an enumeration reports only the buses of the region
/// the request addresses — while a repeat inside one region is still
/// `ResourceAlreadyExistsException` (409).
#[tokio::test]
async fn a_bus_name_is_unique_per_region_and_a_listing_is_scoped_to_one() {
    let harness = Harness::new();
    // Rows: regions of the shapes the `EventBusArn` region segment admits
    // (`[a-z][a-z0-9]*(-[a-z0-9]+)*`), in three partitions' naming styles.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-1"];
    let name = "shared-name-bus";

    for region in regions {
        let created = harness
            .create_signed_for(
                region,
                CreateEventBusInput {
                    name: name.to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| {
                panic!("case {region:?}: the name must be free in each region, got {e:?}")
            });
        let arn = created
            .event_bus_arn
            .clone()
            .unwrap_or_else(|| panic!("case {region:?}: an EventBusArn must be reported"));
        assert_bus_arn(&arn, region, name, region);

        // Unique WITHIN the region, all the same.
        let repeat = harness
            .create_signed_for(
                region,
                CreateEventBusInput {
                    name: name.to_string(),
                    ..Default::default()
                },
            )
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("case {region:?}: a repeat in the same region must be refused")
            });
        assert_eq!(
            repeat.shape_name(),
            "ResourceAlreadyExistsException",
            "case {region:?}: expected ResourceAlreadyExistsException, got {:?}",
            repeat.shape_name()
        );
        assert_eq!(
            repeat.http_status(),
            409,
            "case {region:?}: expected HTTP 409, got {}",
            repeat.http_status()
        );
    }

    // A `ClientToken` records a create per region too: the same token against a
    // second region mints that region's own bus rather than replaying the first.
    let mut replayed: Vec<String> = Vec::new();
    for region in regions {
        let created = harness
            .create_signed_for(
                region,
                CreateEventBusInput {
                    name: format!("token-bus-{region}"),
                    client_token: Some("one-token".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| {
                panic!("case {region:?}: a token used in another region must not replay, got {e:?}")
            });
        let arn = created
            .event_bus_arn
            .unwrap_or_else(|| panic!("case {region:?}: an EventBusArn must be reported"));
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {region:?}: the minted bus belongs to another region, got {arn:?}"
        );
        assert!(
            !replayed.contains(&arn),
            "case {region:?}: the token replayed another region's bus {arn:?}"
        );
        replayed.push(arn);
    }

    for region in regions {
        let listed = harness
            .list_signed_for(
                region,
                ListEventBusesInput {
                    max_results: 100,
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| panic!("case {region:?}: ListEventBuses must succeed, got {e:?}"));
        let buses = listed.event_buses.unwrap_or_default();
        // Exactly the two this region was given, and nothing from the others.
        assert_eq!(
            buses.len(),
            2,
            "case {region:?}: a listing reports only this region's buses, got {buses:?}"
        );
        for bus in &buses {
            let arn = bus.event_bus_arn.clone().unwrap_or_default();
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {region:?}: the enumerated bus belongs to another region, got {arn:?}"
            );
        }
    }
}

/// RULE: the registry is bounded across EVERY region, not only within one. The
/// region a request addresses is request-derived, so the per-region quota bounds
/// each bucket and not how many buckets exist; one emulator therefore holds at
/// most `MAX_EVENT_BUSES_PER_EMULATOR` buses in total, and a create past that is
/// refused with the same modelled `LimitExceededException` (400) — leaving the
/// store finite whatever a caller names.
#[tokio::test]
async fn the_registry_is_bounded_across_every_region() {
    let harness = Harness::new();
    let per_region = eventsv2::state::MAX_EVENT_BUSES;
    let buckets = eventsv2::state::MAX_EMULATED_REGIONS;
    assert_eq!(
        eventsv2::state::MAX_EVENT_BUSES_PER_EMULATOR,
        per_region * buckets,
        "the registry bound must be the per-region quota times the regions kept, got {}",
        eventsv2::state::MAX_EVENT_BUSES_PER_EMULATOR
    );

    for bucket in 0..buckets {
        let region = format!("r{bucket}-emul-1");
        for index in 0..per_region {
            harness
                .create_signed_for(
                    &region,
                    CreateEventBusInput {
                        name: format!("bulk-bus-{bucket}-{index}"),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_or_else(|e| {
                    panic!("case {region:?} bus {index}: the create must succeed, got {e:?}")
                });
        }
    }

    // A region nothing has used yet: its own bucket is empty, so only the
    // registry bound can refuse this.
    let error = harness
        .create_signed_for(
            "one-emul-too-many",
            CreateEventBusInput {
                name: "past-the-registry-bound".to_string(),
                ..Default::default()
            },
        )
        .await
        .expect_err("case \"past the registry bound\": the create must be refused");
    assert_eq!(
        error.shape_name(),
        "LimitExceededException",
        "case \"past the registry bound\": expected LimitExceededException, got {:?}",
        error.shape_name()
    );
    assert_eq!(
        error.http_status(),
        400,
        "case \"past the registry bound\": expected HTTP 400, got {}",
        error.http_status()
    );

    let stored = harness.store().lock().event_buses.len();
    assert_eq!(
        stored,
        eventsv2::state::MAX_EVENT_BUSES_PER_EMULATOR,
        "case \"past the registry bound\": the refused create must store nothing, got {stored}"
    );
}
