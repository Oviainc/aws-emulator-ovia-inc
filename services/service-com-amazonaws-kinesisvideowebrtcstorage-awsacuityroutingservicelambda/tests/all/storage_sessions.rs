// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the two operations this service binds, driven over HTTP
//! through the same `MultiServiceRouter::dispatch` the production front door uses.
//!
//! Both operations answer an EMPTY body, so "it worked" is only observable through
//! the shared store: the session `JoinStorageSession` creates for a channel is the
//! session `JoinStorageSessionAsViewer` registers a viewer on. Every test here
//! asserts the wire answer AND the state the call left behind.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_kinesisvideowebrtcstorage_awsacuityroutingservicelambda as webrtc_storage;
use smithy_server_runtime::MultiServiceRouter;
use webrtc_storage::state::{
    ComAmazonawsKinesisvideowebrtcstorageAwsacuityroutingservicelambdaState as StorageState,
    StorageSession,
};

/// Signaling channel ARNs spanning what the modelled `@pattern` on `ChannelArn`
/// (`^arn:(aws[a-zA-Z-]*):kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`,
/// from `smithy_query(query="operation",operation="JoinStorageSession")`) admits:
/// several partitions, regions, account ids, name spellings and creation times.
const CHANNEL_ARNS: &[&str] = &[
    "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000",
    "arn:aws:kinesisvideo:eu-west-2:000000000000:channel/edge.cam_01-A/1",
    "arn:aws-cn:kinesisvideo:cn-north-1:210987654321:channel/c/9999999999999",
    "arn:aws-us-gov:kinesisvideo:us-gov-west-1:111122223333:channel/Gov_Cam.2/1762000000000",
];

/// An answer from the emulator: the HTTP status and the wire error code
/// (restJson1 carries it as the body's `__type` and the `x-amzn-errortype` header).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_code: Option<String>,
}

impl Answer {
    fn ok(&self) -> bool {
        self.status == 200 && self.error_code.is_none()
    }
}

/// The emulator under test: one service on one shared engine, reached over HTTP.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the core's business, not this service's; the
        // requests below are unsigned, so no authenticator is registered.
        router.set_request_authentication(false);
        router.register_service(webrtc_storage::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn post(&self, path: &str, body: &serde_json::Value) -> Answer {
        let req = http::Request::builder()
            .method("POST")
            .uri(format!("http://kinesisvideo.us-east-1.amazonaws.com{path}"))
            // The routing key: this service's sigv4 signing name, `kinesisvideo`.
            .header("host", "kinesisvideo.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(Bytes::from(
                serde_json::to_vec(body).expect("a serializable request body"),
            ))
            .expect("a valid request");
        let resp = self.router.dispatch(req).await;
        let status = resp.status().as_u16();
        let error_code = serde_json::from_slice::<serde_json::Value>(resp.body())
            .ok()
            .and_then(|body| {
                body.get("__type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        Answer { status, error_code }
    }

    async fn join_as_producer(&self, channel_arn: &str) -> Answer {
        self.post(
            "/joinStorageSession",
            &serde_json::json!({ "channelArn": channel_arn }),
        )
        .await
    }

    async fn join_as_viewer(&self, channel_arn: &str, client_id: &str) -> Answer {
        self.post(
            "/joinStorageSessionAsViewer",
            &serde_json::json!({ "channelArn": channel_arn, "clientId": client_id }),
        )
        .await
    }

    /// Read the channel's session out of the shared store — the only place the
    /// outcome of an empty-bodied join is visible.
    fn with_session<R>(&self, channel_arn: &str, read: impl FnOnce(Option<&StorageSession>) -> R) -> R {
        let state = self.engine.state::<StorageState>();
        let guard = state.lock();
        read(guard.session(channel_arn))
    }

    fn has_no_session(&self, channel_arn: &str) -> bool {
        self.with_session(channel_arn, |session| session.is_none())
    }

    fn knows_channel(&self, channel_arn: &str) -> bool {
        self.engine
            .state::<StorageState>()
            .lock()
            .knows_channel(channel_arn)
    }

    fn viewer_ids(&self, channel_arn: &str) -> BTreeSet<String> {
        self.with_session(channel_arn, |session| {
            session
                .map(|s| s.viewers.keys().cloned().collect())
                .unwrap_or_default()
        })
    }
}

/// The whole happy path, per channel: a viewer cannot join a channel this emulator
/// has no registration for; a producer join registers it and creates the session;
/// the viewer join then lands on THAT session; and a second producer join
/// supersedes the first rather than erroring or no-op'ing.
#[tokio::test]
async fn a_producer_join_creates_the_session_every_later_join_lands_on() {
    let emulator = Emulator::new();

    for channel_arn in CHANNEL_ARNS {
        // Step 1 — nothing is known about the channel yet, so a viewer join fails.
        let before = emulator.join_as_viewer(channel_arn, "viewer-1").await;
        assert_eq!(
            before,
            Answer {
                status: 404,
                error_code: Some("ResourceNotFoundException".to_owned()),
            },
            "case {channel_arn:?}: a viewer join before any producer join must be 404 \
             ResourceNotFoundException, got {before:?}"
        );
        assert!(
            emulator.has_no_session(channel_arn),
            "case {channel_arn:?}: a refused viewer join must record no session, got {:?}",
            emulator.with_session(channel_arn, |s| s.cloned())
        );

        // Step 2 — the producer join registers the channel and creates the session.
        let producer = emulator.join_as_producer(channel_arn).await;
        assert!(
            producer.ok(),
            "case {channel_arn:?}: expected 200 with no error from JoinStorageSession, \
             got {producer:?}"
        );
        emulator.with_session(channel_arn, |session| {
            let session = session.unwrap_or_else(|| {
                panic!("case {channel_arn:?}: JoinStorageSession recorded no session")
            });
            assert_eq!(
                session.channel_arn, *channel_arn,
                "case {channel_arn:?}: the session is keyed by the ARN as supplied, got {:?}",
                session.channel_arn
            );
            assert_eq!(
                session.join_count, 1,
                "case {channel_arn:?}: expected join_count 1 after one producer join, got {}",
                session.join_count
            );
            let producer = session.producer.as_ref().unwrap_or_else(|| {
                panic!("case {channel_arn:?}: no video producing device was recorded")
            });
            assert_eq!(
                producer.generation, 1,
                "case {channel_arn:?}: expected producer generation 1, got {}",
                producer.generation
            );
            assert!(
                session.viewers.is_empty(),
                "case {channel_arn:?}: a producer join must register no viewers, got {:?}",
                session.viewers.keys().collect::<Vec<_>>()
            );
        });

        // Step 3 — the SAME viewer join now succeeds: its outcome changed because
        // of step 2, which is the whole point of sharing one store.
        let after = emulator.join_as_viewer(channel_arn, "viewer-1").await;
        assert!(
            after.ok(),
            "case {channel_arn:?}: expected 200 from JoinStorageSessionAsViewer after a \
             producer join, got {after:?}"
        );

        // Step 4 — a second clientId is a second viewer, not a replacement.
        let second = emulator.join_as_viewer(channel_arn, "viewer-2").await;
        assert!(
            second.ok(),
            "case {channel_arn:?}: expected 200 for a second viewer, got {second:?}"
        );
        assert_eq!(
            emulator.viewer_ids(channel_arn),
            ["viewer-1".to_owned(), "viewer-2".to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "case {channel_arn:?}: two distinct clientIds must be two viewers"
        );

        // Step 5 — a repeat join by viewer-1 refreshes that one viewer.
        let repeat = emulator.join_as_viewer(channel_arn, "viewer-1").await;
        assert!(
            repeat.ok(),
            "case {channel_arn:?}: expected 200 for a repeat viewer join, got {repeat:?}"
        );
        emulator.with_session(channel_arn, |session| {
            let session = session.expect("the session exists");
            assert_eq!(
                session.viewers.len(),
                2,
                "case {channel_arn:?}: a repeat join must not duplicate the viewer, got {:?}",
                session.viewers.keys().collect::<Vec<_>>()
            );
            let viewer = &session.viewers["viewer-1"];
            assert_eq!(
                viewer.join_count, 2,
                "case {channel_arn:?}: expected viewer-1 join_count 2, got {}",
                viewer.join_count
            );
            assert_eq!(
                viewer.client_id, "viewer-1",
                "case {channel_arn:?}: the viewer is keyed by its own clientId, got {:?}",
                viewer.client_id
            );
        });

        // Step 6 — the API is not idempotent and the most recent producer wins.
        let created_at = emulator.with_session(channel_arn, |s| s.expect("session").created_at);
        let again = emulator.join_as_producer(channel_arn).await;
        assert!(
            again.ok(),
            "case {channel_arn:?}: a second producer join must succeed, got {again:?}"
        );
        emulator.with_session(channel_arn, |session| {
            let session = session.expect("the session exists");
            assert_eq!(
                session.join_count, 2,
                "case {channel_arn:?}: expected join_count 2 after two producer joins, got {}",
                session.join_count
            );
            assert_eq!(
                session.producer.as_ref().expect("a producer").generation,
                2,
                "case {channel_arn:?}: the newer producer must supersede the incumbent, got {:?}",
                session.producer
            );
            assert_eq!(
                session.created_at, created_at,
                "case {channel_arn:?}: re-joining must not re-create the session"
            );
            assert_eq!(
                session.viewers.len(),
                2,
                "case {channel_arn:?}: re-joining as producer must keep the viewers, got {:?}",
                session.viewers.keys().collect::<Vec<_>>()
            );
        });
    }

    // Sessions are per channel: every ARN above kept its own.
    for channel_arn in CHANNEL_ARNS {
        assert_eq!(
            emulator.with_session(channel_arn, |s| s.expect("session").join_count),
            2,
            "case {channel_arn:?}: one channel's joins must not be counted on another"
        );
    }
}

/// A viewer join is refused for any channel ARN this emulator holds no
/// registration for — including ARNs that differ from a known one in a single
/// segment, so the decision is made from the stored channel and not from the ARN's
/// shape.
#[tokio::test]
async fn a_viewer_join_for_an_unregistered_channel_is_not_found() {
    let known = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000";
    // Each row changes exactly one segment of `known`: name, region, account,
    // creation time, partition.
    let unknown: &[&str] = &[
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/other-channel/1700000000000",
        "arn:aws:kinesisvideo:us-west-2:123456789012:channel/demo-channel/1700000000000",
        "arn:aws:kinesisvideo:us-east-1:210987654321:channel/demo-channel/1700000000000",
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000001",
        "arn:aws-cn:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000",
    ];

    let emulator = Emulator::new();
    assert!(
        emulator.join_as_producer(known).await.ok(),
        "the known channel must be registered by its producer join"
    );

    for channel_arn in unknown {
        let answer = emulator.join_as_viewer(channel_arn, "viewer-1").await;
        assert_eq!(
            answer,
            Answer {
                status: 404,
                error_code: Some("ResourceNotFoundException".to_owned()),
            },
            "case {channel_arn:?}: expected 404 ResourceNotFoundException, got {answer:?}"
        );
        assert!(
            !emulator.knows_channel(channel_arn),
            "case {channel_arn:?}: a refused viewer join must not register the channel"
        );
        assert!(
            emulator.has_no_session(channel_arn),
            "case {channel_arn:?}: a refused viewer join must create no session"
        );
    }

    // The registered channel is untouched by every refusal above.
    assert_eq!(
        emulator.viewer_ids(known),
        BTreeSet::new(),
        "refused joins on other ARNs must not land on the registered channel"
    );
    assert_eq!(
        emulator.with_session(known, |s| s.expect("session").join_count),
        1,
        "refused joins on other ARNs must not touch the registered channel's session"
    );
}

/// The modelled ARN pattern admits ANY lowercase resource type and puts no length
/// on the ARN, so an ARN that names something other than a signaling channel, or
/// that is longer than a `kinesisvideo` resource ARN can be, satisfies the schema
/// and must be refused on semantic grounds — with `InvalidArgumentException`, by
/// BOTH operations, recording nothing.
#[tokio::test]
async fn an_arn_that_cannot_name_a_signaling_channel_is_an_invalid_argument() {
    let emulator = Emulator::new();

    // Resource types a `kinesisvideo` ARN can carry; `[a-z]+` in the modelled
    // pattern keeps them lowercase. `stream` is the service's other resource type.
    let resource_types = ["stream", "mediastream", "account"];
    // Rows past a bound the Kinesis Video control-plane model publishes: a channel
    // name longer than `ChannelName`'s 256 (`smithy_query(query="shape",
    // shape="ChannelName")` on the kinesisvideo model), and an ARN longer than
    // `ResourceARN`'s 1024 — both still matching this service's `@pattern`.
    let over_long_name = format!(
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/{}/1700000000000",
        "n".repeat(257)
    );
    let over_long_arn = format!(
        "arn:aws:kinesisvideo:us-east-1:{}:channel/demo-channel/1700000000000",
        "1".repeat(1100)
    );
    let mut rows: Vec<(String, String)> = resource_types
        .iter()
        .map(|resource_type| {
            (
                format!("resource type {resource_type:?}"),
                format!(
                    "arn:aws:kinesisvideo:us-east-1:123456789012:{resource_type}/demo/1700000000000"
                ),
            )
        })
        .collect();
    rows.push(("channel name of 257 characters".to_owned(), over_long_name));
    rows.push(("ARN of more than 1024 characters".to_owned(), over_long_arn));

    for (case, channel_arn) in &rows {
        let producer = emulator.join_as_producer(channel_arn).await;
        assert_eq!(
            producer,
            Answer {
                status: 400,
                error_code: Some("InvalidArgumentException".to_owned()),
            },
            "case {case:?}: JoinStorageSession must refuse it, got {producer:?}"
        );
        let viewer = emulator.join_as_viewer(channel_arn, "viewer-1").await;
        assert_eq!(
            viewer,
            Answer {
                status: 400,
                error_code: Some("InvalidArgumentException".to_owned()),
            },
            "case {case:?}: JoinStorageSessionAsViewer must refuse it, got {viewer:?}"
        );
        assert!(
            !emulator.knows_channel(channel_arn),
            "case {case:?}: a refused join must register nothing"
        );
        assert!(
            emulator.has_no_session(channel_arn),
            "case {case:?}: a refused join must create no session"
        );
    }

    // The published bound itself is accepted: a channel name of exactly 256.
    let at_bound = format!(
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/{}/1700000000000",
        "n".repeat(256)
    );
    let answer = emulator.join_as_producer(&at_bound).await;
    assert!(
        answer.ok(),
        "case channel name of exactly 256 characters: a name on the published bound \
         must be accepted, got {answer:?}"
    );
}

/// Values outside the modelled constraints are refused by the framework before any
/// handler runs: 400, never a 501 and never a success. The rows walk each
/// constraint the model declares — `channelArn`'s pattern, and `clientId`'s
/// pattern and 1..256 length.
#[tokio::test]
async fn values_outside_the_modelled_constraints_are_refused_with_400() {
    let known = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000";
    let emulator = Emulator::new();
    assert!(
        emulator.join_as_producer(known).await.ok(),
        "the known channel must be registered by its producer join"
    );

    // (case, channelArn, clientId) — a `None` clientId exercises JoinStorageSession.
    let too_long = "v".repeat(257);
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("channelArn is not an ARN at all", "not-an-arn", None),
        ("channelArn has no creation time", "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel", None),
        ("channelArn resource type is not lowercase", "arn:aws:kinesisvideo:us-east-1:123456789012:Channel/demo-channel/1700000000000", None),
        ("channelArn account is not numeric", "arn:aws:kinesisvideo:us-east-1:acct:channel/demo-channel/1700000000000", None),
        ("channelArn is not an ARN at all, on the viewer path", "not-an-arn", Some("viewer-1")),
        ("clientId contains a space and a bang", known, Some("bad id!")),
        ("clientId contains a slash", known, Some("viewer/1")),
        ("clientId is empty (length minimum 1)", known, Some("")),
        ("clientId is 257 characters (length maximum 256)", known, Some(too_long.as_str())),
    ];

    for (case, channel_arn, client_id) in rows {
        let answer = match client_id {
            Some(client_id) => emulator.join_as_viewer(channel_arn, client_id).await,
            None => emulator.join_as_producer(channel_arn).await,
        };
        assert_eq!(
            answer,
            Answer {
                status: 400,
                error_code: Some("ValidationException".to_owned()),
            },
            "case {case:?}: a modelled-constraint violation must be refused by the \
             framework with 400 ValidationException, got {answer:?}"
        );
    }

    // The published `clientId` bounds themselves are accepted.
    for length in [1_usize, 256] {
        let client_id = "v".repeat(length);
        let answer = emulator.join_as_viewer(known, &client_id).await;
        assert!(
            answer.ok(),
            "case clientId of length {length}: a value on the published bound must be \
             accepted, got {answer:?}"
        );
    }
    assert_eq!(
        emulator.viewer_ids(known).len(),
        2,
        "only the two accepted viewers may have been registered, got {:?}",
        emulator.viewer_ids(known)
    );
}

/// The counting invariants, over a long deterministic interleaving of joins across
/// several channels and clients: the session's `join_count` is the number of
/// producer joins it has served, its producer's `generation` is the `join_count` of
/// the most recent one, and each viewer's `join_count` is the number of times that
/// `clientId` joined — whatever order the calls arrive in.
#[tokio::test]
async fn join_counts_track_the_calls_whatever_the_interleaving() {
    /// Fixed seed, so this sequence is the same on every run (printed here rather
    /// than generated, so a failure is reproducible from the source alone).
    const SEED: u64 = 0x5DEE_CE66_D15E_A5E5;
    /// Bounded case count: step budget for the interleaving.
    const STEPS: usize = 120;

    // xorshift64*: a deterministic sequence, no generator crate and nothing on disk.
    let mut rng_state = SEED;
    let mut next = move || {
        rng_state ^= rng_state >> 12;
        rng_state ^= rng_state << 25;
        rng_state ^= rng_state >> 27;
        rng_state = rng_state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        rng_state
    };

    let clients = ["cam.a", "cam_b", "cam-c", "c4"];
    let emulator = Emulator::new();

    // The independent model this test checks the emulator against.
    let mut expected_producer_joins = vec![0_u64; CHANNEL_ARNS.len()];
    let mut expected_viewer_joins = vec![[0_u64; 4]; CHANNEL_ARNS.len()];

    for step in 0..STEPS {
        let roll = next();
        let channel = (roll >> 3) as usize % CHANNEL_ARNS.len();
        let channel_arn = CHANNEL_ARNS[channel];
        let as_producer = expected_producer_joins[channel] == 0 || roll % 3 == 0;

        if as_producer {
            let answer = emulator.join_as_producer(channel_arn).await;
            assert!(
                answer.ok(),
                "case step {step} producer join on {channel_arn:?}: expected 200, got {answer:?}"
            );
            expected_producer_joins[channel] += 1;
        } else {
            let client = (roll >> 11) as usize % clients.len();
            let answer = emulator.join_as_viewer(channel_arn, clients[client]).await;
            assert!(
                answer.ok(),
                "case step {step} viewer join {:?} on {channel_arn:?}: expected 200, got {answer:?}",
                clients[client]
            );
            expected_viewer_joins[channel][client] += 1;
            // A viewer join is not a producer join: it must leave the incumbent
            // producing device, and its joined_at, exactly where they were.
            let producer = emulator.with_session(channel_arn, |session| {
                session.expect("the session exists").producer.clone()
            });
            assert_eq!(
                producer.as_ref().map(|p| p.generation),
                Some(expected_producer_joins[channel]),
                "case step {step} viewer join on {channel_arn:?}: a viewer join must not \
                 change the incumbent producer, got {producer:?}"
            );
        }
    }

    for (channel, channel_arn) in CHANNEL_ARNS.iter().enumerate() {
        let producer_joins = expected_producer_joins[channel];
        emulator.with_session(channel_arn, |session| {
            let session = session.unwrap_or_else(|| {
                panic!("case {channel_arn:?}: expected a session after {producer_joins} joins")
            });
            assert_eq!(
                session.join_count, producer_joins,
                "case {channel_arn:?}: expected join_count {producer_joins}, got {}",
                session.join_count
            );
            assert_eq!(
                session.producer.as_ref().map(|p| p.generation),
                Some(producer_joins),
                "case {channel_arn:?}: the incumbent producer's generation must be the \
                 join_count of the most recent producer join, got {:?}",
                session.producer
            );
            for (client, client_id) in clients.iter().enumerate() {
                let joins = expected_viewer_joins[channel][client];
                match session.viewers.get(*client_id) {
                    Some(viewer) => assert_eq!(
                        viewer.join_count, joins,
                        "case {channel_arn:?}/{client_id:?}: expected {joins} viewer joins, \
                         got {}",
                        viewer.join_count
                    ),
                    None => assert_eq!(
                        joins, 0,
                        "case {channel_arn:?}/{client_id:?}: {joins} joins were accepted but \
                         no viewer was registered"
                    ),
                }
            }
        });
    }
}

/// The two limits that keep request-keyed state bounded are enforced through the
/// modelled `ClientLimitExceededException`, and only for a join that would ADD an
/// entry: a repeat join of something already recorded still succeeds.
#[tokio::test]
async fn joins_past_a_limit_are_refused_with_client_limit_exceeded() {
    let limit_exceeded = Answer {
        status: 400,
        error_code: Some("ClientLimitExceededException".to_owned()),
    };
    let channel_arn =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000000";
    let emulator = Emulator::new();
    assert!(
        emulator.join_as_producer(channel_arn).await.ok(),
        "the channel must be registered by its producer join"
    );

    // Every viewer up to the per-channel maximum is admitted.
    for index in 0..webrtc_storage::state::MAX_VIEWERS_PER_SESSION {
        let client_id = format!("viewer-{index}");
        let answer = emulator.join_as_viewer(channel_arn, &client_id).await;
        assert!(
            answer.ok(),
            "case viewer {index} of {}: expected 200, got {answer:?}",
            webrtc_storage::state::MAX_VIEWERS_PER_SESSION
        );
    }
    assert_eq!(
        emulator.viewer_ids(channel_arn).len(),
        webrtc_storage::state::MAX_VIEWERS_PER_SESSION,
        "every viewer up to the limit must be registered, got {:?}",
        emulator.viewer_ids(channel_arn)
    );

    // One more DISTINCT viewer is refused, and records nothing.
    let answer = emulator.join_as_viewer(channel_arn, "viewer-one-too-many").await;
    assert_eq!(
        answer, limit_exceeded,
        "case one viewer past the limit: expected 400 ClientLimitExceededException, \
         got {answer:?}"
    );
    assert!(
        !emulator.viewer_ids(channel_arn).contains("viewer-one-too-many"),
        "case one viewer past the limit: the refused viewer must not be registered"
    );

    // A viewer already on the session still joins again at the limit.
    let answer = emulator.join_as_viewer(channel_arn, "viewer-0").await;
    assert!(
        answer.ok(),
        "case repeat join at the limit: an already-registered viewer must still join, \
         got {answer:?}"
    );
    emulator.with_session(channel_arn, |session| {
        let session = session.expect("the session exists");
        assert_eq!(
            session.viewers.len(),
            webrtc_storage::state::MAX_VIEWERS_PER_SESSION,
            "case repeat join at the limit: the viewer count must not move, got {:?}",
            session.viewers.len()
        );
        assert_eq!(
            session.viewers["viewer-0"].join_count, 2,
            "case repeat join at the limit: expected join_count 2 for viewer-0, got {}",
            session.viewers["viewer-0"].join_count
        );
    });

    // The channel limit, the same way: fill the store to its maximum through the
    // state API (the request path would be the same calls), then drive the next
    // join over HTTP.
    let filler = Emulator::new();
    let now = std::time::SystemTime::now();
    {
        let state = filler.engine.state::<StorageState>();
        let mut store = state.lock();
        for index in 0..webrtc_storage::state::MAX_SIGNALING_CHANNELS {
            let arn =
                format!("arn:aws:kinesisvideo:us-east-1:123456789012:channel/c{index}/1700000000000");
            store
                .join_as_producer(&arn, now)
                .unwrap_or_else(|refused| panic!("case channel {index}: {}", refused.message()));
        }
        assert_eq!(
            store.channel_count(),
            webrtc_storage::state::MAX_SIGNALING_CHANNELS,
            "the store must hold exactly the maximum, got {}",
            store.channel_count()
        );
    }
    let one_too_many =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/one-too-many/1700000000000";
    let answer = filler.join_as_producer(one_too_many).await;
    assert_eq!(
        answer, limit_exceeded,
        "case one channel past the limit: expected 400 ClientLimitExceededException, \
         got {answer:?}"
    );
    assert!(
        !filler.knows_channel(one_too_many),
        "case one channel past the limit: the refused channel must not be registered"
    );
    assert!(
        filler.has_no_session(one_too_many),
        "case one channel past the limit: the refused channel must have no session"
    );

    // A channel already in the store still accepts a producer join at the limit.
    let existing = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/c0/1700000000000";
    let answer = filler.join_as_producer(existing).await;
    assert!(
        answer.ok(),
        "case repeat producer join at the channel limit: expected 200, got {answer:?}"
    );
    assert_eq!(
        filler.with_session(existing, |s| s.expect("session").join_count),
        2,
        "case repeat producer join at the channel limit: the join must have been counted"
    );
}
