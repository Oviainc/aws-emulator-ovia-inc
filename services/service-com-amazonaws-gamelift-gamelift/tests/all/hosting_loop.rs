// SPDX-License-Identifier: CC0-1.0

//! The GameLift hosting loop, end to end over the wire: provision a fleet, place a game
//! session on it, reserve a player slot in that session, read both back, and tear the
//! fleet down.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! real requests with, as an awsJson1_1 `GameLift.<Operation>` request, so what these
//! tests assert is the response body a client receives.
//!
//! The harness below (`GameLift` and the helpers under it) is shared with the other
//! suites of this test target — see `unknown_resource_lookups.rs`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_gamelift_gamelift::state::WORKFLOW_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for. The emulator serves a request in the
/// region of its SigV4 credential scope, and the ARNs it mints name that region.
pub(crate) const REGION: &str = "us-west-2";

/// An ARN this service mints for `resource`, in [`REGION`] and carrying the one account
/// every emulated response presents (`engine::emulator_account_id`).
pub(crate) fn gamelift_arn(resource: &str) -> String {
    format!(
        "arn:aws:gamelift:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    )
}

/// One emulator holding only the GameLift service, driven through the production
/// dispatch path.
pub(crate) struct GameLift {
    router: MultiServiceRouter,
    /// The one engine the services below are mounted on, so a suite can read the state a
    /// request wrote (or set up state whose span no test can wait out) through
    /// [`Self::state`].
    engine: Arc<engine::InMemoryEngine>,
}

impl GameLift {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not signature verification, and send
        // an unsigned-but-scoped Authorization header for routing and the region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_gamelift_gamelift::EmulatedService::new(Arc::clone(&engine)),
        );
        // Amazon S3 on the SAME engine, the way the router mounts every service: the
        // location `CreateBuild` hands a caller to upload build files to is a bucket in
        // that one object store, so the two are mounted together here as well. It claims
        // only requests scoped to `s3`, so every call below still reaches GameLift.
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// This service's store in that engine, exactly as a handler reaches it
    /// (`crate::state::service_state(&*self.engine)`).
    pub(crate) fn state(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_gamelift_gamelift::ComAmazonawsGameliftGameliftState,
    > {
        service_com_amazonaws_gamelift_gamelift::state::service_state(&*self.engine)
            .expect("the GameLift store is reachable through the engine the service is mounted on")
    }

    /// Send `GameLift.<operation>` with `body` and return the status and parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// [`Self::call`], signed for `region` rather than for [`REGION`]: the emulator serves a
    /// request in the Region of its SigV4 credential scope, so this is how a suite reaches
    /// the same operation in another Region.
    pub(crate) async fn call_in(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let credential =
            format!("AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/gamelift/aws4_request, SignedHeaders=host, Signature=00");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://gamelift.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("GameLift.{operation}"))
            .header("authorization", credential)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// The body of a call that must succeed.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        self.ok_in(REGION, operation, body).await
    }

    /// [`Self::ok`] for a call served in `region`.
    pub(crate) async fn ok_in(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call_in(region, operation, &body).await;
        assert_eq!(
            status, 200,
            "{operation} in {region} {body}: expected HTTP 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// The HTTP status and wire error code of a call that must fail.
    pub(crate) async fn err(&self, operation: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, &body).await;
        assert_ne!(
            status, 200,
            "{operation} {body}: expected a failure, got HTTP 200 with body {parsed}"
        );
        (status, error_code(&parsed))
    }

    /// One Amazon S3 request against the object store this emulator holds, as a client
    /// sends it path-style (`PUT /<bucket>`, `PUT /<bucket>/<key>`): the data plane behind
    /// the locations a build's files are read from and uploaded to. Answers the HTTP status
    /// and the response body.
    pub(crate) async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Bytes) {
        // The credential scope's service field is what routes the request, so this one says
        // `s3` where the GameLift calls above say `gamelift`.
        let credential =
            format!("AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/s3/aws4_request, SignedHeaders=host, Signature=00");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", credential)
            .body(Bytes::copy_from_slice(body))
            .expect("build the object request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_parts().1)
    }
}

/// The `Message` member of a modelled error body. awsJson renders the member under its
/// modelled name (`NotFoundException$Message`); the lower-case spelling is accepted too
/// so this helper reads the body whichever casing the protocol layer emits.
pub(crate) fn error_message(body: &Value) -> String {
    for key in ["Message", "message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    format!("<no Message in {body}>")
}

/// The wire error code of an awsJson error body (`__type`), without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// Wait past the span the service's clock-owed status changes are due on, so the next
/// read observes the state the service would be in whether or not anybody called.
pub(crate) fn wait_for_due_transitions() {
    std::thread::sleep(WORKFLOW_DELAY + Duration::from_millis(250));
}

/// A managed EC2 fleet request that satisfies the documented required parameters.
pub(crate) fn ec2_fleet(name: &str) -> Value {
    json!({
        "Name": name,
        "BuildId": "build-demo",
        "EC2InstanceType": "c5.large",
        "ComputeType": "EC2",
    })
}

/// An Anywhere fleet request deployed to `locations`, which `CreateFleet` requires for
/// this compute type.
pub(crate) fn anywhere_fleet(name: &str, locations: &[&str]) -> Value {
    json!({
        "Name": name,
        "ComputeType": "ANYWHERE",
        "Locations": locations
            .iter()
            .map(|location| json!({"Location": location}))
            .collect::<Vec<_>>(),
    })
}

/// Create a fleet and return its `FleetId`.
pub(crate) async fn create_fleet(gamelift: &GameLift, request: Value) -> String {
    let created = gamelift.ok("CreateFleet", request).await;
    created["FleetAttributes"]["FleetId"]
        .as_str()
        .expect("CreateFleet must report a FleetId")
        .to_string()
}

/// The hosting loop the bootstrap issue describes, with every response read back out of
/// stored state.
#[tokio::test]
async fn the_hosting_loop_runs_end_to_end_against_stored_state() {
    let gamelift = GameLift::new();

    // 1. CreateFleet — the response is built from the record just persisted.
    let created = gamelift
        .ok(
            "CreateFleet",
            json!({
                "Name": "demo-fleet",
                "Description": "the demo fleet",
                "BuildId": "build-demo",
                "EC2InstanceType": "m5.2xlarge",
                "ComputeType": "EC2",
                "FleetType": "SPOT",
                "NewGameSessionProtectionPolicy": "FullProtection",
                "MetricGroups": ["demo-metrics"],
                "EC2InboundPermissions": [
                    {"FromPort": 7777, "ToPort": 7780, "IpRange": "0.0.0.0/0", "Protocol": "UDP"}
                ],
                "Tags": [{"Key": "owner", "Value": "demo"}],
            }),
        )
        .await;
    let attributes = created["FleetAttributes"].clone();
    let fleet_id = attributes["FleetId"]
        .as_str()
        .expect("CreateFleet must report a FleetId")
        .to_string();
    assert!(
        fleet_id.starts_with("fleet-") && fleet_id.len() > "fleet-".len(),
        "CreateFleet: FleetId must match the modelled ^[a-z]*fleet-[a-zA-Z0-9\\-]+$, got {fleet_id}"
    );
    // `FleetAttributes$FleetArn` doc: "Format is arn:aws:gamelift:<region>::fleet/fleet-…",
    // with the account segment populated (see `state::gamelift_arn`).
    assert_eq!(
        attributes["FleetArn"],
        json!(gamelift_arn(&format!("fleet/{fleet_id}"))),
        "CreateFleet: FleetArn must be the documented format, got {}",
        attributes["FleetArn"]
    );
    // A managed EC2 fleet is "placed in NEW status while Amazon GameLift Servers
    // initiates the fleet creation workflow".
    assert_eq!(
        attributes["Status"],
        json!("NEW"),
        "CreateFleet: a managed EC2 fleet starts NEW, got {}",
        attributes["Status"]
    );
    assert_eq!(attributes["Name"], json!("demo-fleet"));
    assert_eq!(attributes["Description"], json!("the demo fleet"));
    assert_eq!(attributes["InstanceType"], json!("m5.2xlarge"));
    assert_eq!(attributes["FleetType"], json!("SPOT"));
    assert_eq!(attributes["ComputeType"], json!("EC2"));
    assert_eq!(attributes["BuildId"], json!("build-demo"));
    assert_eq!(
        attributes["BuildArn"],
        json!(gamelift_arn("build/build-demo"))
    );
    assert_eq!(
        attributes["NewGameSessionProtectionPolicy"],
        json!("FullProtection")
    );
    assert_eq!(attributes["MetricGroups"], json!(["demo-metrics"]));
    assert!(
        attributes["CreationTime"].is_number(),
        "CreateFleet: CreationTime must be a modelled timestamp, got {}",
        attributes["CreationTime"]
    );
    // "This list includes an entry for the fleet's home Region. For fleets with no
    // remote locations, only one entry … is returned." Its status is the fleet's.
    assert_eq!(
        created["LocationStates"],
        json!([{"Location": REGION, "Status": "NEW"}]),
        "CreateFleet: LocationStates must carry the home Region, got {}",
        created["LocationStates"]
    );

    // A game session cannot be placed on a fleet that is not ACTIVE yet.
    let (status, code) = gamelift
        .err(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 4}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidFleetStatusException"),
        "CreateGameSession on a NEW fleet must be refused: got {status} {code}"
    );

    // 2. DescribeFleetAttributes — the fleet-creation workflow has come due, so the
    // stored fleet and each of its locations are ACTIVE.
    wait_for_due_transitions();
    let described = gamelift
        .ok(
            "DescribeFleetAttributes",
            json!({"FleetIds": [&fleet_id]}),
        )
        .await;
    let described = described["FleetAttributes"][0].clone();
    assert_eq!(
        described["Status"],
        json!("ACTIVE"),
        "DescribeFleetAttributes: the fleet must be ACTIVE once its workflow came due, got {}",
        described["Status"]
    );
    assert_eq!(
        described["Name"], attributes["Name"],
        "DescribeFleetAttributes must report the stored Name"
    );
    assert_eq!(
        described["InstanceType"], attributes["InstanceType"],
        "DescribeFleetAttributes must report the stored InstanceType"
    );
    assert_eq!(
        described["FleetArn"], attributes["FleetArn"],
        "DescribeFleetAttributes must report the stored FleetArn"
    );

    // 3. CreateGameSession — returned ACTIVATING, with no connection information yet.
    let session = gamelift
        .ok(
            "CreateGameSession",
            json!({
                "FleetId": &fleet_id,
                "MaximumPlayerSessionCount": 4,
                "Name": "demo-session",
                "CreatorId": "creator-1",
                "GameSessionData": "level=1",
                "GameProperties": [{"Key": "mode", "Value": "ranked"}],
            }),
        )
        .await;
    let session = session["GameSession"].clone();
    let game_session_id = session["GameSessionId"]
        .as_str()
        .expect("CreateGameSession must report a GameSessionId")
        .to_string();
    // `GameSession$GameSessionId` doc: "arn:aws:gamelift:<home_region>::gamesession/<fleet
    // ID>/<ID string>".
    let home_prefix = gamelift_arn(&format!("gamesession/{fleet_id}/"));
    assert!(
        game_session_id.starts_with(&home_prefix)
            && game_session_id.len() > home_prefix.len(),
        "CreateGameSession: GameSessionId must be a full ARN of the documented form, got {game_session_id}"
    );
    assert_eq!(
        session["Status"],
        json!("ACTIVATING"),
        "CreateGameSession: a new session starts ACTIVATING, got {}",
        session["Status"]
    );
    assert_eq!(session["CurrentPlayerSessionCount"], json!(0));
    assert_eq!(session["MaximumPlayerSessionCount"], json!(4));
    assert_eq!(
        session["PlayerSessionCreationPolicy"],
        json!("ACCEPT_ALL"),
        "CreateGameSession: \"newly created game sessions are open to new players\", got {}",
        session["PlayerSessionCreationPolicy"]
    );
    assert_eq!(session["GameProperties"], json!([{"Key": "mode", "Value": "ranked"}]));
    assert_eq!(session["GameSessionData"], json!("level=1"));
    assert_eq!(session["CreatorId"], json!("creator-1"));
    assert_eq!(session["FleetId"], json!(&fleet_id));
    assert_eq!(session["FleetArn"], attributes["FleetArn"]);
    assert_eq!(session["Location"], json!(REGION));
    assert!(
        session.get("IpAddress").is_none() && session.get("Port").is_none(),
        "CreateGameSession: connection information arrives only when the session is ACTIVE, got {session}"
    );

    // 4. A slot cannot be reserved until the session is running.
    let (status, code) = gamelift
        .err(
            "CreatePlayerSession",
            json!({"GameSessionId": &game_session_id, "PlayerId": "player-1"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidGameSessionStatusException"),
        "CreatePlayerSession on an ACTIVATING session must be refused: got {status} {code}"
    );

    // 5. DescribeGameSessions — the session is ACTIVE and now carries connection
    // information taken from the fleet instance hosting it.
    wait_for_due_transitions();
    let listed = gamelift
        .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
        .await;
    let sessions = listed["GameSessions"]
        .as_array()
        .expect("DescribeGameSessions must report a list")
        .clone();
    assert_eq!(
        sessions.len(),
        1,
        "DescribeGameSessions: exactly the one stored session, got {listed}"
    );
    let running = sessions[0].clone();
    assert_eq!(running["GameSessionId"], json!(&game_session_id));
    assert_eq!(
        running["Status"],
        json!("ACTIVE"),
        "DescribeGameSessions: the session must be ACTIVE once its start-up came due, got {}",
        running["Status"]
    );
    let ip_address = running["IpAddress"]
        .as_str()
        .expect("an ACTIVE session must report an IpAddress")
        .to_string();
    let port = running["Port"]
        .as_i64()
        .expect("an ACTIVE session must report a Port");
    assert!(
        (7777..=7780).contains(&port),
        "DescribeGameSessions: the port must come from the fleet's open range 7777-7780, got {port}"
    );
    // `GameSession$DnsName` doc, non-TLS-enabled fleets:
    // "ec2-<unique identifier>.compute.amazonaws.com".
    assert_eq!(
        running["DnsName"],
        json!(format!(
            "ec2-{}.compute.amazonaws.com",
            ip_address.replace('.', "-")
        )),
        "DescribeGameSessions: DnsName must be the documented non-TLS form, got {}",
        running["DnsName"]
    );

    // 6. CreatePlayerSession — the reservation points at the parent session and fleet.
    let reserved = gamelift
        .ok(
            "CreatePlayerSession",
            json!({
                "GameSessionId": &game_session_id,
                "PlayerId": "player-1",
                "PlayerData": "rank=gold",
            }),
        )
        .await;
    let player = reserved["PlayerSession"].clone();
    let player_session_id = player["PlayerSessionId"]
        .as_str()
        .expect("CreatePlayerSession must report a PlayerSessionId")
        .to_string();
    assert!(
        player_session_id.starts_with("psess-")
            && !player_session_id["psess-".len()..].contains(char::is_whitespace)
            && player_session_id.len() > "psess-".len(),
        "CreatePlayerSession: PlayerSessionId must match the modelled ^psess-\\S+$, got {player_session_id}"
    );
    assert_eq!(
        player["Status"],
        json!("RESERVED"),
        "CreatePlayerSession: \"a slot is reserved\", got {}",
        player["Status"]
    );
    assert_eq!(player["PlayerId"], json!("player-1"));
    assert_eq!(player["PlayerData"], json!("rank=gold"));
    assert_eq!(player["GameSessionId"], json!(&game_session_id));
    assert_eq!(player["FleetId"], json!(&fleet_id));
    assert_eq!(player["FleetArn"], attributes["FleetArn"]);
    assert_eq!(
        (player["IpAddress"].clone(), player["Port"].clone(), player["DnsName"].clone()),
        (running["IpAddress"].clone(), running["Port"].clone(), running["DnsName"].clone()),
        "CreatePlayerSession: the player connects to the session's own instance and port"
    );

    // 7. The reservation is visible on the session the next read returns.
    let listed = gamelift
        .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
        .await;
    assert_eq!(
        listed["GameSessions"][0]["CurrentPlayerSessionCount"],
        json!(1),
        "DescribeGameSessions: the reserved slot must be counted, got {}",
        listed["GameSessions"][0]
    );

    // 8. DeleteFleet — "During the deletion process, the fleet status is changed to
    // DELETING", and the fleet is gone once the deletion comes due.
    gamelift
        .ok("DeleteFleet", json!({"FleetId": &fleet_id}))
        .await;
    let deleting = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
        .await;
    assert_eq!(
        deleting["FleetAttributes"][0]["Status"],
        json!("DELETING"),
        "DeleteFleet: the fleet must move to DELETING, got {}",
        deleting["FleetAttributes"][0]
    );
    assert!(
        deleting["FleetAttributes"][0]["TerminationTime"].is_number(),
        "DeleteFleet: the fleet must carry a TerminationTime, got {}",
        deleting["FleetAttributes"][0]
    );

    wait_for_due_transitions();
    let purged = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
        .await;
    assert_eq!(
        purged["FleetAttributes"],
        json!([]),
        "DescribeFleetAttributes after the delete came due must report the fleet as gone \
         with an empty page, got {purged}"
    );
    // The game sessions placed on the fleet, and the slots reserved in them, went with it.
    let purged_sessions = gamelift
        .ok(
            "DescribeGameSessions",
            json!({"GameSessionId": &game_session_id}),
        )
        .await;
    assert_eq!(
        purged_sessions["GameSessions"],
        json!([]),
        "a game session on a deleted fleet must be gone too, got {purged_sessions}"
    );
    let (status, code) = gamelift
        .err(
            "CreatePlayerSession",
            json!({"GameSessionId": &game_session_id, "PlayerId": "player-2"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NotFoundException"),
        "a player slot cannot be reserved in a session that is gone: got {status} {code}"
    );
}

/// `CreateFleet`'s documented required parameters, per compute type: an Anywhere fleet
/// needs a custom location and is created ACTIVE; a managed EC2 fleet needs exactly one
/// of `BuildId`/`ScriptId` plus an `EC2InstanceType`, and is created NEW.
///
/// The instance types and fleet types are drawn from the `EC2InstanceType` /
/// `FleetType` enums (`smithy_query shape=EC2InstanceType,FleetType`).
#[tokio::test]
async fn create_fleet_enforces_the_required_parameters_of_each_compute_type() {
    struct Case {
        label: &'static str,
        request: Value,
        expected: Result<&'static str, &'static str>,
    }

    let cases = vec![
        Case {
            label: "EC2 with a build",
            request: json!({
                "Name": "with-build",
                "ComputeType": "EC2",
                "BuildId": "build-one",
                "EC2InstanceType": "c5.large",
            }),
            expected: Ok("NEW"),
        },
        Case {
            label: "EC2 with a script and a SPOT fleet type",
            request: json!({
                "Name": "with-script",
                "ComputeType": "EC2",
                "ScriptId": "script-one",
                "EC2InstanceType": "r6i.large",
                "FleetType": "SPOT",
            }),
            expected: Ok("NEW"),
        },
        Case {
            label: "EC2 with no ComputeType at all (EC2 is the default)",
            request: json!({
                "Name": "default-compute-type",
                "BuildId": "build-two",
                "EC2InstanceType": "t2.micro",
            }),
            expected: Ok("NEW"),
        },
        Case {
            label: "EC2 with both a build and a script",
            request: json!({
                "Name": "build-and-script",
                "ComputeType": "EC2",
                "BuildId": "build-three",
                "ScriptId": "script-three",
                "EC2InstanceType": "c5a.4xlarge",
            }),
            expected: Err("InvalidRequestException"),
        },
        Case {
            label: "EC2 with neither a build nor a script",
            request: json!({
                "Name": "no-build-no-script",
                "ComputeType": "EC2",
                "EC2InstanceType": "c5.large",
            }),
            expected: Err("InvalidRequestException"),
        },
        Case {
            label: "EC2 with no instance type",
            request: json!({
                "Name": "no-instance-type",
                "ComputeType": "EC2",
                "BuildId": "build-four",
            }),
            expected: Err("InvalidRequestException"),
        },
        Case {
            label: "Anywhere with a custom location",
            request: json!({
                "Name": "anywhere",
                "ComputeType": "ANYWHERE",
                "Locations": [{"Location": "custom-home"}],
            }),
            // "If successful, this operation creates a new fleet resource and places it
            // in ACTIVE status."
            expected: Ok("ACTIVE"),
        },
        Case {
            label: "Anywhere with no location",
            request: json!({
                "Name": "anywhere-no-location",
                "ComputeType": "ANYWHERE",
            }),
            expected: Err("InvalidRequestException"),
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        match case.expected {
            Ok(expected_status) => {
                let created = gamelift.ok("CreateFleet", case.request.clone()).await;
                let status = created["FleetAttributes"]["Status"].clone();
                assert_eq!(
                    status,
                    json!(expected_status),
                    "case {}: expected status {expected_status}, got {status}",
                    case.label
                );
                // Whatever the compute type, the response is the stored record: reading
                // it back gives the same thing.
                let fleet_id = created["FleetAttributes"]["FleetId"]
                    .as_str()
                    .expect("a created fleet has a FleetId");
                let described = gamelift
                    .ok("DescribeFleetAttributes", json!({"FleetIds": [fleet_id]}))
                    .await;
                assert_eq!(
                    described["FleetAttributes"][0], created["FleetAttributes"],
                    "case {}: a describe must return exactly what was created",
                    case.label
                );
            }
            Err(expected_code) => {
                let (status, code) = gamelift.err("CreateFleet", case.request.clone()).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected_code),
                    "case {}: expected 400 {expected_code}, got {status} {code}",
                    case.label
                );
            }
        }
    }
}

/// `MaximumPlayerSessionCount` spans the modelled lower bound (`WholeNumber`, range min
/// 0) up to the service's own published maximum: "The maximum number of players per game
/// session is 200. It is not adjustable." Each published bound is tested, and one past
/// the top.
#[tokio::test]
async fn create_game_session_accepts_the_whole_published_player_count_range() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("counts")).await;
    wait_for_due_transitions();

    for count in [0, 1, 17, 199, 200] {
        let created = gamelift
            .ok(
                "CreateGameSession",
                json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": count}),
            )
            .await;
        assert_eq!(
            created["GameSession"]["MaximumPlayerSessionCount"],
            json!(count),
            "case {count}: the stored maximum must be the requested one, got {}",
            created["GameSession"]["MaximumPlayerSessionCount"]
        );
    }

    for count in [201, 1_000] {
        let (status, code) = gamelift
            .err(
                "CreateGameSession",
                json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": count}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {count}: a player count past the published maximum must be refused, got {status} {code}"
        );
    }
}

/// A game session with every slot taken refuses the next reservation, whatever the
/// session's size.
#[tokio::test]
async fn a_game_session_refuses_a_slot_once_it_is_full() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("full")).await;
    wait_for_due_transitions();

    for capacity in [1, 2, 3] {
        let created = gamelift
            .ok(
                "CreateGameSession",
                json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": capacity}),
            )
            .await;
        let game_session_id = created["GameSession"]["GameSessionId"]
            .as_str()
            .expect("a created session has an id")
            .to_string();
        wait_for_due_transitions();

        for slot in 0..capacity {
            let reserved = gamelift
                .ok(
                    "CreatePlayerSession",
                    json!({
                        "GameSessionId": &game_session_id,
                        "PlayerId": format!("player-{slot}"),
                    }),
                )
                .await;
            assert_eq!(
                reserved["PlayerSession"]["Status"],
                json!("RESERVED"),
                "case capacity {capacity} slot {slot}: expected RESERVED, got {}",
                reserved["PlayerSession"]["Status"]
            );
        }

        let listed = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"GameSessionId": &game_session_id}),
            )
            .await;
        assert_eq!(
            listed["GameSessions"][0]["CurrentPlayerSessionCount"],
            json!(capacity),
            "case capacity {capacity}: every slot must be counted, got {}",
            listed["GameSessions"][0]
        );

        let (status, code) = gamelift
            .err(
                "CreatePlayerSession",
                json!({"GameSessionId": &game_session_id, "PlayerId": "one-too-many"}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "GameSessionFullException"),
            "case capacity {capacity}: the slot past the last must be refused, got {status} {code}"
        );
    }
}

/// A repeated `CreateGameSession` carrying the same custom string returns the session
/// the first one created; a different request carrying it is a mismatch. The custom
/// string may arrive as `IdempotencyToken` or as `GameSessionId` — both are the same
/// `IdStringModel` custom id.
#[tokio::test]
async fn a_repeated_create_game_session_is_idempotent_on_its_custom_id() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("idempotent")).await;
    wait_for_due_transitions();

    for member in ["IdempotencyToken", "GameSessionId"] {
        let token = format!("token-{member}");
        let request = json!({
            "FleetId": &fleet_id,
            "MaximumPlayerSessionCount": 8,
            "Name": "same-session",
            member: &token,
        });

        let first = gamelift.ok("CreateGameSession", request.clone()).await;
        let second = gamelift.ok("CreateGameSession", request.clone()).await;
        assert_eq!(
            first["GameSession"], second["GameSession"],
            "case {member}: a repeat must return the very same session, got {} then {}",
            first["GameSession"], second["GameSession"]
        );
        let id = first["GameSession"]["GameSessionId"]
            .as_str()
            .expect("a created session has an id")
            .to_string();
        assert!(
            id.ends_with(&format!("/{token}")),
            "case {member}: the custom id must be the ARN's id string, got {id}"
        );

        // One session, not two.
        let listed = gamelift
            .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
            .await;
        let count = listed["GameSessions"]
            .as_array()
            .map_or(0, |sessions| {
                sessions
                    .iter()
                    .filter(|session| session["GameSessionId"] == json!(&id))
                    .count()
            });
        assert_eq!(
            count, 1,
            "case {member}: the repeat must not have created a second session, got {listed}"
        );

        // A different configuration under the same custom string is a mismatch.
        let mut conflicting = request.clone();
        conflicting["MaximumPlayerSessionCount"] = json!(9);
        let (status, code) = gamelift.err("CreateGameSession", conflicting).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "IdempotentParameterMismatchException"),
            "case {member}: a conflicting repeat must be refused, got {status} {code}"
        );
    }
}

/// Pagination is a page-size-independent walk: for EVERY page size, the pages a client
/// follows concatenate, in order and without repeats, to exactly the unpaginated answer.
///
/// The sweep is exhaustive over page sizes 1..=(fleets + 1), which covers the whole
/// parameter space rather than sampling it.
#[tokio::test]
async fn the_paginated_describes_walk_the_whole_set_at_every_page_size() {
    const FLEETS: usize = 5;
    let gamelift = GameLift::new();
    for index in 0..FLEETS {
        create_fleet(&gamelift, ec2_fleet(&format!("page-{index}"))).await;
    }
    wait_for_due_transitions();

    let whole = gamelift
        .ok("DescribeFleetAttributes", json!({}))
        .await["FleetAttributes"]
        .as_array()
        .expect("DescribeFleetAttributes must report a list")
        .clone();
    assert_eq!(
        whole.len(),
        FLEETS,
        "an unpaginated describe must report every stored fleet, got {}",
        whole.len()
    );

    for page_size in 1..=FLEETS + 1 {
        let mut walked: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"Limit": page_size});
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = gamelift
                .ok("DescribeFleetAttributes", request.clone())
                .await;
            let items = page["FleetAttributes"]
                .as_array()
                .expect("a page must report a list")
                .clone();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page must hold at most Limit items, got {}",
                items.len()
            );
            walked.extend(items);
            pages += 1;
            assert!(
                pages <= FLEETS + 1,
                "case page size {page_size}: the walk must terminate, it took {pages} pages"
            );
            match page["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case page size {page_size}: the pages must concatenate to the whole set in order, got {} items",
            walked.len()
        );
    }

    // A token this service never issued is refused rather than silently restarting.
    // (An EMPTY token is refused one step earlier, by the modelled `NonZeroAndMaxString`
    // minimum length, so it never reaches the handler.)
    for bogus in ["not-a-token", "zzzzzzzzzzzzzzzz", "0000000000000", "00000000000000000"] {
        let (status, code) = gamelift
            .err(
                "DescribeFleetAttributes",
                json!({"Limit": 2, "NextToken": bogus}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case token {bogus:?}: a foreign pagination token must be refused, got {status} {code}"
        );
    }

    // And so is a token of the OTHER paginated collection: a fleet token names an offset
    // into the fleets, which is not an offset into anybody's game sessions.
    let fleet_token = gamelift
        .ok("DescribeFleetAttributes", json!({"Limit": 1}))
        .await["NextToken"]
        .as_str()
        .expect("a page short of the whole set must carry a NextToken")
        .to_string();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("cross-token")).await;
    let (status, code) = gamelift
        .err(
            "DescribeGameSessions",
            json!({"FleetId": &fleet_id, "Limit": 1, "NextToken": &fleet_token}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a token minted while walking fleets must not be read as a game-session offset, got {status} {code}"
    );
}

/// A fleet hosts one game session per running server process, so its concurrent game
/// sessions are the total `ConcurrentExecutions` its `RuntimeConfiguration` asks for,
/// and the request past that is refused with `FleetCapacityExceededException`.
#[tokio::test]
async fn a_fleet_hosts_one_game_session_per_server_process() {
    struct Case {
        label: &'static str,
        /// The `ConcurrentExecutions` of each server process the fleet configures.
        processes: Vec<i64>,
        /// The resulting number of game sessions the fleet can host.
        capacity: usize,
    }

    let cases = vec![
        Case {
            label: "one process running once",
            processes: vec![1],
            capacity: 1,
        },
        Case {
            label: "one process running twice",
            processes: vec![2],
            capacity: 2,
        },
        Case {
            label: "two processes, three executions between them",
            processes: vec![1, 2],
            capacity: 3,
        },
    ];

    for case in &cases {
        let gamelift = GameLift::new();
        let mut request = ec2_fleet(case.label);
        request["RuntimeConfiguration"] = json!({
            "ServerProcesses": case
                .processes
                .iter()
                .enumerate()
                .map(|(index, executions)| json!({
                    "LaunchPath": format!("/local/game/server-{index}"),
                    "ConcurrentExecutions": executions,
                }))
                .collect::<Vec<_>>(),
        });
        let fleet_id = create_fleet(&gamelift, request).await;
        wait_for_due_transitions();

        for session in 0..case.capacity {
            let created = gamelift
                .ok(
                    "CreateGameSession",
                    json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2}),
                )
                .await;
            assert!(
                created["GameSession"]["GameSessionId"].is_string(),
                "case {}: session {session} of {} must be placed, got {created}",
                case.label,
                case.capacity
            );
        }

        let (status, code) = gamelift
            .err(
                "CreateGameSession",
                json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {}: the session past the fleet's {} server processes must be refused, got {status} {code}",
            case.label,
            case.capacity
        );
    }

    // And a runtime configuration asking for more processes than one compute may run is
    // refused outright: "An Amazon GameLift Servers instance is limited to 50 processes
    // running concurrently."
    let gamelift = GameLift::new();
    for (label, executions, expected) in [
        ("the documented per-instance maximum", 50, None),
        (
            "one past the documented per-instance maximum",
            51,
            Some("InvalidRequestException"),
        ),
    ] {
        let mut request = ec2_fleet(label);
        request["RuntimeConfiguration"] = json!({
            "ServerProcesses": [
                {"LaunchPath": "/local/game/server", "ConcurrentExecutions": executions}
            ],
        });
        match expected {
            None => {
                let created = gamelift.ok("CreateFleet", request).await;
                assert!(
                    created["FleetAttributes"]["FleetId"].is_string(),
                    "case {label}: {executions} processes must be accepted, got {created}"
                );
            }
            Some(expected_code) => {
                let (status, code) = gamelift.err("CreateFleet", request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected_code),
                    "case {label}: {executions} processes must be refused, got {status} {code}"
                );
            }
        }
    }
}

/// The account holds a bounded number of fleets: the create past the quota answers the
/// modelled `LimitExceededException` rather than growing the store without limit.
#[tokio::test]
async fn create_fleet_refuses_the_fleet_past_the_quota() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_FLEETS_PER_ACCOUNT;
    let gamelift = GameLift::new();
    for index in 0..quota {
        create_fleet(&gamelift, ec2_fleet(&format!("quota-{index}"))).await;
    }

    let (status, code) = gamelift.err("CreateFleet", ec2_fleet("one-too-many")).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "the fleet past the quota of {quota} must be refused, got {status} {code}"
    );

    // Deleting one makes room again, once the deletion comes due.
    let listed = gamelift
        .ok("DescribeFleetAttributes", json!({"Limit": 1}))
        .await;
    let doomed = listed["FleetAttributes"][0]["FleetId"]
        .as_str()
        .expect("a stored fleet has an id")
        .to_string();
    gamelift.ok("DeleteFleet", json!({"FleetId": &doomed})).await;
    wait_for_due_transitions();
    let created = gamelift.ok("CreateFleet", ec2_fleet("room-again")).await;
    assert!(
        created["FleetAttributes"]["FleetId"].is_string(),
        "a fleet must be creatable again once a deletion freed a slot, got {created}"
    );
}

/// A game session's port comes from the inbound range the fleet opens, whatever that
/// range is; a fleet that opens none draws from the range `IpPermission$FromPort`
/// documents as valid ("only ports 1026-60000 are valid").
#[tokio::test]
async fn a_game_session_port_comes_from_the_range_its_fleet_opens() {
    struct Case {
        label: &'static str,
        /// The range the fleet opens for inbound traffic, if any.
        opened: Option<(i64, i64)>,
        /// The range a game session's port must fall in.
        expected: (i64, i64),
    }

    let cases = vec![
        Case {
            label: "a single-port range",
            opened: Some((1026, 1026)),
            expected: (1026, 1026),
        },
        Case {
            label: "a small range",
            opened: Some((7777, 7780)),
            expected: (7777, 7780),
        },
        Case {
            label: "a wide range",
            opened: Some((50_000, 60_000)),
            expected: (50_000, 60_000),
        },
        Case {
            label: "no inbound permissions at all",
            opened: None,
            expected: (1026, 60_000),
        },
    ];

    let gamelift = GameLift::new();
    let mut fleets = Vec::new();
    for case in &cases {
        let mut request = ec2_fleet(case.label);
        if let Some((from, to)) = case.opened {
            request["EC2InboundPermissions"] = json!([
                {"FromPort": from, "ToPort": to, "IpRange": "10.0.0.0/8", "Protocol": "UDP"}
            ]);
        }
        fleets.push((case, create_fleet(&gamelift, request).await));
    }
    wait_for_due_transitions();

    for (_, fleet_id) in &fleets {
        gamelift
            .ok(
                "CreateGameSession",
                json!({"FleetId": fleet_id, "MaximumPlayerSessionCount": 2}),
            )
            .await;
    }
    wait_for_due_transitions();

    for (case, fleet_id) in &fleets {
        let (low, high) = case.expected;
        let label = case.label;
        let listed = gamelift
            .ok("DescribeGameSessions", json!({"FleetId": fleet_id}))
            .await;
        let session = listed["GameSessions"][0].clone();
        let port = session["Port"]
            .as_i64()
            .expect("an ACTIVE session must report a Port");
        assert!(
            (low..=high).contains(&port),
            "case {label}: the port must fall in {low}-{high}, got {port}"
        );
        let ip_address = session["IpAddress"]
            .as_str()
            .expect("an ACTIVE session must report an IpAddress");
        assert!(
            ip_address.starts_with("203.0.113."),
            "case {label}: the instance address must come from the documentation range, got {ip_address}"
        );
    }
}

/// The size of a `DescribeFleetAttributes` response is set by the fleets that exist, not
/// by the size of the request: naming one fleet repeatedly (by id or by ARN) reports it
/// once, and a list longer than the documented maximum is refused before any record is
/// read.
#[tokio::test]
async fn naming_a_fleet_more_than_once_reports_it_once() {
    let max = service_com_amazonaws_gamelift_gamelift::state::MAX_FLEET_IDS_PER_REQUEST;
    let gamelift = GameLift::new();
    let created = gamelift.ok("CreateFleet", ec2_fleet("repeated")).await;
    let fleet_id = created["FleetAttributes"]["FleetId"]
        .as_str()
        .expect("a FleetId")
        .to_string();
    let fleet_arn = created["FleetAttributes"]["FleetArn"]
        .as_str()
        .expect("a FleetArn")
        .to_string();

    // Each row names the same single fleet, differing in how many times and in which
    // form; every row must report exactly one FleetAttributes.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("once by id", vec![fleet_id.clone()]),
        ("once by ARN", vec![fleet_arn.clone()]),
        ("twice by id", vec![fleet_id.clone(), fleet_id.clone()]),
        ("by id and by ARN", vec![fleet_id.clone(), fleet_arn.clone()]),
        ("the maximum, all the same id", vec![fleet_id.clone(); max]),
    ];
    for (label, fleet_ids) in rows {
        let described = gamelift
            .ok("DescribeFleetAttributes", json!({"FleetIds": fleet_ids}))
            .await;
        let reported = described["FleetAttributes"].as_array().expect("a list");
        assert_eq!(
            reported.len(),
            1,
            "case {label}: one distinct fleet must be reported once, got {} records",
            reported.len()
        );
        assert_eq!(
            reported[0]["FleetId"].as_str(),
            Some(fleet_id.as_str()),
            "case {label}: the reported fleet must be the one named, got {described}"
        );
    }

    // One past the documented maximum is refused, and the message carries the maximum.
    let (status, code) = gamelift
        .err(
            "DescribeFleetAttributes",
            json!({"FleetIds": vec![fleet_id.clone(); max + 1]}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "case one past the maximum of {max}: the request must be refused, got {status} {code}"
    );
}

/// No two live sessions on one fleet listen on the same port, and a fleet that opens
/// fewer ports than its processes could run hosts only as many sessions as it has ports:
/// the request past that is refused with `FleetCapacityExceededException`. The rows span
/// the widths a fleet can open, from the single remote-access port upwards.
#[tokio::test]
async fn a_fleet_hosts_one_session_per_port_it_opens() {
    // Each row is (label, opened range, how many ports that range covers). The valid
    // set comes from `IpPermission$FromPort`'s doc: "only ports 22 and 1026-60000 are
    // valid."
    let cases: [(&str, (i64, i64), usize); 3] = [
        ("the remote-access port alone", (22, 22), 1),
        ("a two-port range", (7777, 7778), 2),
        ("a five-port range", (1026, 1030), 5),
    ];

    for (label, (from, to), ports) in cases {
        let gamelift = GameLift::new();
        let mut request = ec2_fleet(label);
        request["EC2InboundPermissions"] = json!([
            {"FromPort": from, "ToPort": to, "IpRange": "10.0.0.0/8", "Protocol": "UDP"}
        ]);
        let fleet_id = create_fleet(&gamelift, request).await;
        wait_for_due_transitions();

        for index in 0..ports {
            gamelift
                .ok(
                    "CreateGameSession",
                    json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2, "Name": format!("{label}-{index}")}),
                )
                .await;
        }
        let (status, code) = gamelift
            .err(
                "CreateGameSession",
                json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2, "Name": "one-too-many"}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {label}: a fleet opening {ports} port(s) must refuse the session past them, got {status} {code}"
        );

        wait_for_due_transitions();
        let listed = gamelift
            .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
            .await;
        let sessions = listed["GameSessions"].as_array().expect("a list").clone();
        assert_eq!(
            sessions.len(),
            ports,
            "case {label}: all {ports} session(s) must be stored, got {listed}"
        );
        let mut seen = std::collections::BTreeSet::new();
        for session in &sessions {
            let port = session["Port"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {label}: an ACTIVE session must report a Port, got {session}"));
            assert!(
                (from..=to).contains(&port),
                "case {label}: the port must fall in {from}-{to}, got {port}"
            );
            assert!(
                seen.insert(port),
                "case {label}: port {port} is already held by another live session on the fleet"
            );
        }
    }
}

/// A game session placed in one of a fleet's remote locations carries the location
/// segment its id's documented remote form has; a location the fleet does not have is
/// refused.
#[tokio::test]
async fn a_remote_location_game_session_carries_the_location_in_its_id() {
    let gamelift = GameLift::new();
    let mut request = ec2_fleet("multi-location");
    request["Locations"] = json!([
        {"Location": REGION},
        {"Location": "us-east-1"},
        {"Location": "eu-west-1"},
    ]);
    let created = gamelift.ok("CreateFleet", request).await;
    let fleet_id = created["FleetAttributes"]["FleetId"]
        .as_str()
        .expect("a created fleet has a FleetId")
        .to_string();
    // The home Region appears exactly once even though the request named it too.
    assert_eq!(
        created["LocationStates"],
        json!([
            {"Location": REGION, "Status": "NEW"},
            {"Location": "us-east-1", "Status": "NEW"},
            {"Location": "eu-west-1", "Status": "NEW"},
        ]),
        "CreateFleet: LocationStates must carry the home Region once, then each remote location, got {}",
        created["LocationStates"]
    );
    wait_for_due_transitions();

    for location in ["us-east-1", "eu-west-1"] {
        let session = gamelift
            .ok(
                "CreateGameSession",
                json!({
                    "FleetId": &fleet_id,
                    "MaximumPlayerSessionCount": 2,
                    "Location": location,
                }),
            )
            .await;
        let id = session["GameSession"]["GameSessionId"]
            .as_str()
            .expect("a created session has an id")
            .to_string();
        assert!(
            id.starts_with(&gamelift_arn(&format!(
                "gamesession/{fleet_id}/{location}/"
            ))),
            "case {location}: a remote-location session id must carry its location, got {id}"
        );
        assert_eq!(
            session["GameSession"]["Location"],
            json!(location),
            "case {location}: the stored location must be the requested one, got {}",
            session["GameSession"]["Location"]
        );

        // And the location filter picks out exactly that session.
        let listed = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "Location": location}),
            )
            .await;
        let ids: Vec<&str> = listed["GameSessions"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|session| session["GameSessionId"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![id.as_str()],
            "case {location}: the Location filter must select only that location's sessions, got {listed}"
        );
    }

    // A home-Region session has no location segment.
    let home = gamelift
        .ok(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2}),
        )
        .await;
    let home_id = home["GameSession"]["GameSessionId"]
        .as_str()
        .expect("a created session has an id")
        .to_string();
    assert_eq!(
        home_id.matches('/').count(),
        2,
        "a home-Region session id has the form gamesession/<fleet>/<id>, got {home_id}"
    );

    // A location the fleet does not deploy to is refused.
    let (status, code) = gamelift
        .err(
            "CreateGameSession",
            json!({
                "FleetId": &fleet_id,
                "MaximumPlayerSessionCount": 2,
                "Location": "ap-south-1",
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a location the fleet does not have must be refused, got {status} {code}"
    );
}

/// Every way of naming a resource an operation must ACT ON that is not there, across
/// the slice, answers the modelled `NotFoundException`, and a request that names no
/// selector at all is an invalid request rather than a silent full scan. (A reporting
/// operation's unknown selector is a 200 with an empty collection instead — see
/// `unknown_resource_lookups.rs`.)
#[tokio::test]
async fn absent_resources_and_missing_selectors_are_refused() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("refusals")).await;
    wait_for_due_transitions();

    struct Case {
        label: &'static str,
        operation: &'static str,
        request: Value,
        expected: &'static str,
    }

    let cases = vec![
        Case {
            label: "place a session on an unknown fleet",
            operation: "CreateGameSession",
            request: json!({"FleetId": "fleet-does-not-exist", "MaximumPlayerSessionCount": 2}),
            expected: "NotFoundException",
        },
        Case {
            label: "place a session through an alias, which no stored alias resolves",
            operation: "CreateGameSession",
            request: json!({"AliasId": "alias-unknown", "MaximumPlayerSessionCount": 2}),
            expected: "NotFoundException",
        },
        Case {
            label: "place a session naming neither a fleet nor an alias",
            operation: "CreateGameSession",
            request: json!({"MaximumPlayerSessionCount": 2}),
            expected: "InvalidRequestException",
        },
        Case {
            label: "place a session naming both a fleet and an alias",
            operation: "CreateGameSession",
            request: json!({
                "FleetId": &fleet_id,
                "AliasId": "alias-unknown",
                "MaximumPlayerSessionCount": 2,
            }),
            expected: "InvalidRequestException",
        },
        Case {
            label: "reserve a slot in an unknown session",
            operation: "CreatePlayerSession",
            request: json!({
                "GameSessionId": gamelift_arn(&format!("gamesession/{fleet_id}/nope")),
                "PlayerId": "player-1",
            }),
            expected: "NotFoundException",
        },
        Case {
            label: "describe sessions with no selector",
            operation: "DescribeGameSessions",
            request: json!({}),
            expected: "InvalidRequestException",
        },
        Case {
            label: "describe sessions with two selectors",
            operation: "DescribeGameSessions",
            request: json!({"FleetId": &fleet_id, "AliasId": "alias-unknown"}),
            expected: "InvalidRequestException",
        },
        Case {
            label: "delete an unknown fleet",
            operation: "DeleteFleet",
            request: json!({"FleetId": "fleet-does-not-exist"}),
            expected: "NotFoundException",
        },
    ];

    for case in &cases {
        let (status, code) = gamelift.err(case.operation, case.request.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.expected),
            "case {}: expected 400 {}, got {status} {code}",
            case.label,
            case.expected
        );
    }

    // The fleet that IS there is untouched by all of that: every refusal above was a
    // read or a rejected write, so the stored fleet still reads back ACTIVE.
    let described = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
        .await;
    assert_eq!(
        described["FleetAttributes"][0]["Status"],
        json!("ACTIVE"),
        "the stored fleet must survive the refused requests, got {}",
        described["FleetAttributes"][0]
    );
}

/// A TLS-enabled fleet reports the other documented `DnsName` form.
#[tokio::test]
async fn a_tls_enabled_fleet_reports_the_amazongamelift_dns_name() {
    let gamelift = GameLift::new();
    let mut request = ec2_fleet("tls");
    request["CertificateConfiguration"] = json!({"CertificateType": "GENERATED"});
    let fleet_id = create_fleet(&gamelift, request).await;
    wait_for_due_transitions();

    gamelift
        .ok(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2}),
        )
        .await;
    wait_for_due_transitions();

    let listed = gamelift
        .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
        .await;
    // `GameSession$DnsName` doc, TLS-enabled fleets:
    // "<unique identifier>.<region identifier>.amazongamelift.com".
    assert_eq!(
        listed["GameSessions"][0]["DnsName"],
        json!(format!("{fleet_id}.{REGION}.amazongamelift.com")),
        "a TLS-enabled fleet must report the amazongamelift.com DnsName form, got {}",
        listed["GameSessions"][0]["DnsName"]
    );
}

/// `CreateFleet` refuses an `EC2InboundPermissions` port range outside the one
/// `IpPermission$FromPort` documents as valid ("only ports 22 and 1026-60000 are
/// valid") or that runs backwards (`ToPort`: "This value must be equal to or greater
/// than FromPort"). Each published bound is tested, and one past it — where a value
/// outside the modelled `PortNumber` range (1-60000) is refused one step earlier, by
/// the boundary walk, as `ValidationException`.
#[tokio::test]
async fn create_fleet_validates_inbound_port_ranges_against_the_documented_bounds() {
    struct Case {
        label: &'static str,
        range: (i64, i64),
        /// `None` when the range is accepted, else the wire error code expected.
        expected: Option<&'static str>,
    }

    let cases = vec![
        Case {
            label: "the remote-access port alone",
            range: (22, 22),
            expected: None,
        },
        Case {
            label: "the low bound of the valid range",
            range: (1026, 1026),
            expected: None,
        },
        Case {
            label: "the high bound of the valid range",
            range: (60_000, 60_000),
            expected: None,
        },
        Case {
            label: "the whole valid range",
            range: (1026, 60_000),
            expected: None,
        },
        Case {
            label: "one below the low bound of the valid set",
            range: (1025, 2000),
            expected: Some("InvalidRequestException"),
        },
        Case {
            label: "a range that runs backwards",
            range: (8000, 7000),
            expected: Some("InvalidRequestException"),
        },
        Case {
            label: "a range spanning the remote-access port and the valid range",
            range: (22, 1026),
            expected: Some("InvalidRequestException"),
        },
        Case {
            label: "one above the modelled PortNumber maximum",
            range: (59_000, 60_001),
            expected: Some("ValidationException"),
        },
        Case {
            label: "one below the modelled PortNumber minimum",
            range: (0, 2000),
            expected: Some("ValidationException"),
        },
    ];

    let gamelift = GameLift::new();
    for case in &cases {
        let (from, to) = case.range;
        let mut request = ec2_fleet(case.label);
        request["EC2InboundPermissions"] = json!([
            {"FromPort": from, "ToPort": to, "IpRange": "0.0.0.0/0", "Protocol": "TCP"}
        ]);
        match case.expected {
            None => {
                let created = gamelift.ok("CreateFleet", request).await;
                assert!(
                    created["FleetAttributes"]["FleetId"].is_string(),
                    "case {}: range {from}-{to} must be accepted, got {created}",
                    case.label
                );
            }
            Some(expected_code) => {
                let (status, code) = gamelift.err("CreateFleet", request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected_code),
                    "case {}: range {from}-{to} must be refused with {expected_code}, got {status} {code}",
                    case.label
                );
            }
        }
    }
}

/// `StatusFilter` narrows the describe to sessions in that status, and a read never
/// changes what a later read sees.
#[tokio::test]
async fn the_status_filter_selects_sessions_and_reads_do_not_disturb_them() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("status-filter")).await;
    wait_for_due_transitions();

    gamelift
        .ok(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 3}),
        )
        .await;

    // While the session is still starting up it answers the ACTIVATING filter only.
    for (filter, expected) in [("ACTIVATING", 1), ("ACTIVE", 0), ("TERMINATED", 0)] {
        let listed = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "StatusFilter": filter}),
            )
            .await;
        let count = listed["GameSessions"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected,
            "case filter {filter} before start-up came due: expected {expected} session(s), got {listed}"
        );
    }

    wait_for_due_transitions();

    // And once it is running, the ACTIVE filter only — and repeating the read reports
    // the same thing, so the read moved nothing.
    for (filter, expected) in [("ACTIVE", 1), ("ACTIVATING", 0)] {
        let first = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "StatusFilter": filter}),
            )
            .await;
        let second = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "StatusFilter": filter}),
            )
            .await;
        let count = first["GameSessions"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected,
            "case filter {filter} after start-up came due: expected {expected} session(s), got {first}"
        );
        assert_eq!(
            first, second,
            "case filter {filter}: a second read must report exactly what the first did"
        );
    }

    // Every status the filter's doc names is accepted; anything else is a bad request,
    // not a silently empty page.
    for filter in ["ACTIVE", "TERMINATED", "ACTIVATING", "TERMINATING"] {
        let listed = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "StatusFilter": filter}),
            )
            .await;
        assert!(
            listed["GameSessions"].is_array(),
            "case filter {filter}: a documented status must be accepted, got {listed}"
        );
    }
    for filter in ["active", "RUNNING", "ACTIVE "] {
        let (status, code) = gamelift
            .err(
                "DescribeGameSessions",
                json!({"FleetId": &fleet_id, "StatusFilter": filter}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case filter {filter:?}: a status outside the documented set must be refused, got {status} {code}"
        );
    }
}

/// The same custom ID string is scoped to one fleet: used on another fleet it creates a
/// fresh session rather than reporting a mismatch.
#[tokio::test]
async fn a_custom_game_session_id_is_scoped_to_its_fleet() {
    let gamelift = GameLift::new();
    let first_fleet = create_fleet(&gamelift, ec2_fleet("scope-one")).await;
    let second_fleet = create_fleet(&gamelift, ec2_fleet("scope-two")).await;
    wait_for_due_transitions();

    let mut created = Vec::new();
    for fleet_id in [&first_fleet, &second_fleet] {
        let session = gamelift
            .ok(
                "CreateGameSession",
                json!({
                    "FleetId": fleet_id,
                    "MaximumPlayerSessionCount": 4,
                    "IdempotencyToken": "shared-token",
                }),
            )
            .await;
        let id = session["GameSession"]["GameSessionId"]
            .as_str()
            .expect("a created session has an id")
            .to_string();
        assert!(
            id.contains(fleet_id.as_str()),
            "case fleet {fleet_id}: the session must belong to the fleet asked for, got {id}"
        );
        created.push(id);
    }
    assert_ne!(
        created[0], created[1],
        "the same token on two fleets must name two sessions, got {created:?}"
    );

    // And on each fleet the token is still idempotent.
    for (index, fleet_id) in [&first_fleet, &second_fleet].into_iter().enumerate() {
        let repeat = gamelift
            .ok(
                "CreateGameSession",
                json!({
                    "FleetId": fleet_id,
                    "MaximumPlayerSessionCount": 4,
                    "IdempotencyToken": "shared-token",
                }),
            )
            .await;
        assert_eq!(
            repeat["GameSession"]["GameSessionId"],
            json!(&created[index]),
            "case fleet {fleet_id}: the repeat must return that fleet's own session, got {}",
            repeat["GameSession"]["GameSessionId"]
        );
    }
}

/// `Limit` and `NextToken` are "ignored when the request specifies one or a list of
/// fleet IDs": such a request gets every fleet it named, whole, and no token.
#[tokio::test]
async fn naming_fleet_ids_ignores_the_pagination_parameters() {
    let gamelift = GameLift::new();
    let mut fleet_ids = Vec::new();
    for index in 0..3 {
        fleet_ids.push(create_fleet(&gamelift, ec2_fleet(&format!("named-{index}"))).await);
    }
    wait_for_due_transitions();

    for limit in [1, 2, 3, 50] {
        let described = gamelift
            .ok(
                "DescribeFleetAttributes",
                json!({"FleetIds": &fleet_ids, "Limit": limit}),
            )
            .await;
        let reported: Vec<&str> = described["FleetAttributes"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|fleet| fleet["FleetId"].as_str())
            .collect();
        assert_eq!(
            reported,
            fleet_ids.iter().map(String::as_str).collect::<Vec<_>>(),
            "case Limit {limit}: a request naming fleet ids must report all of them in order, got {described}"
        );
        assert!(
            described.get("NextToken").is_none(),
            "case Limit {limit}: a request naming fleet ids must report no NextToken, got {described}"
        );
    }
}

/// No two live fleets report the same connection information: each fleet's instance
/// address, and the `DnsName` derived from it, is its own. The row set is a whole
/// quota's worth of fleets — the largest set the store can hold at once — so this spans
/// the class of live fleets rather than sampling it.
#[tokio::test]
async fn every_live_fleet_reports_its_own_connection_address() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_FLEETS_PER_ACCOUNT;
    let gamelift = GameLift::new();

    let mut fleet_ids = Vec::with_capacity(quota);
    for index in 0..quota {
        fleet_ids.push(create_fleet(&gamelift, ec2_fleet(&format!("address-{index}"))).await);
    }
    // A game session carries its host instance's address, so it is how a fleet's
    // address is observable over the wire.
    wait_for_due_transitions();
    for fleet_id in &fleet_ids {
        gamelift
            .ok(
                "CreateGameSession",
                json!({"FleetId": fleet_id, "MaximumPlayerSessionCount": 1}),
            )
            .await;
    }
    wait_for_due_transitions();

    let mut addresses = std::collections::BTreeSet::new();
    let mut dns_names = std::collections::BTreeSet::new();
    for fleet_id in &fleet_ids {
        let described = gamelift
            .ok("DescribeGameSessions", json!({"FleetId": fleet_id}))
            .await;
        let session = &described["GameSessions"][0];
        let address = session["IpAddress"].as_str().unwrap_or_else(|| {
            panic!("case fleet {fleet_id}: an ACTIVE game session must report an IpAddress, got {described}")
        });
        let dns_name = session["DnsName"].as_str().unwrap_or_else(|| {
            panic!("case fleet {fleet_id}: an ACTIVE game session must report a DnsName, got {described}")
        });
        assert!(
            addresses.insert(address.to_string()),
            "case fleet {fleet_id}: address {address} is already held by another live fleet"
        );
        assert!(
            dns_names.insert(dns_name.to_string()),
            "case fleet {fleet_id}: DnsName {dns_name} is already held by another live fleet"
        );
    }
    assert_eq!(
        addresses.len(),
        quota,
        "each of the {quota} live fleets must report its own address, got {} distinct",
        addresses.len()
    );
}
