// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListCommands`: the page of the account's COMMANDS
//! computed from the records `CreateCommand` wrote and `UpdateCommand` moved.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `GET /commands` signed for the `iot` signing name — so what is asserted is
//! what an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * the listing reports EVERY command the store holds as the subset of members
//!   `CommandSummary` declares, each one the projection of the very record
//!   `GetCommand` reads, so the two cannot drift;
//! * the order is the commands' CREATION order — "By default, the API returns all
//!   commands in the descending order based on the time that they were created"
//!   (`ListCommandsRequest$sortOrder`) — and `sortOrder` is what reverses it; the id
//!   breaks a tie between two commands created in the same instant, which is what
//!   keeps the order, and therefore every page boundary, total;
//! * `namespace` selects the commands created in it, a command created in none being
//!   in the base namespace, and `commandParameterName` selects "the list of commands
//!   that have a specific command parameter name" — the commands whose mandatory
//!   parameters declare it;
//! * `maxResults` and `nextToken` walk that one order, every command exactly once,
//!   with the page the member documents as the default (25) when the request names
//!   none; a token belongs to the listing that issued it.
//!
//! The tables span what those rules generalise over — how many commands the store
//! holds, the form and length of a command id, which namespace a command was created
//! in, how many parameters it declares and of which name form, which optional members
//! it carries, which way the order runs, and the page size (one per page, a size that
//! divides the store, one that does not, the modelled bounds and a page larger than
//! the store) — rather than one recorded request.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::CreateCommandInput;
use service_com_amazonaws_iot_awsiotservice::state::{
    command_arn, service_state, RegisteredCommand,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region every call below is signed for.
const REGION: &str = "us-east-1";

/// The namespace a command created without one is in — `CommandNamespace` admits
/// `AWS-IoT` and `AWS-IoT-FleetWise`
/// (`smithy_query(query="shape",shape="CommandNamespace")`), and `CreateCommand$roleArn`
/// is "not supported when you use the AWS-IoT namespace", which is the namespace a
/// command carrying no role is in.
const BASE_NAMESPACE: &str = "AWS-IoT";
/// The other `CommandNamespace` value, whose commands carry the role that builds their
/// payload.
const FLEETWISE_NAMESPACE: &str = "AWS-IoT-FleetWise";

/// The page a request that names no `maxResults` gets: "By default, the API returns up
/// to a maximum of 25 results" (`ListCommandsRequest$maxResults`).
const DEFAULT_PAGE: usize = 25;
/// The largest page the operation serves: "You can override this default value to
/// return up to a maximum of 100 results for this operation"
/// (`ListCommandsRequest$maxResults`, `CommandMaxResults` is `range 1..100`).
const MAX_PAGE: usize = 100;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and [`REGION`]: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A member of the body as it was serialized, or `None` when it is absent.
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name)
    }

    /// The `commands` the listing reported, in the order it reported them.
    fn commands(&self) -> Vec<&Value> {
        self.body
            .get("commands")
            .and_then(Value::as_array)
            .map(|entries| entries.iter().collect())
            .unwrap_or_default()
    }

    /// The ids the listing reported, in the order it reported them.
    fn command_ids(&self) -> Vec<String> {
        self.commands()
            .into_iter()
            .map(|entry| {
                entry
                    .get("commandId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The summary of one command, when the listing reported it.
    fn summary(&self, command_id: &str) -> Option<&Value> {
        self.commands().into_iter().find(|entry| {
            entry.get("commandId").and_then(Value::as_str) == Some(command_id)
        })
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it. The engine is kept so a case can
/// seed the command store directly where an HTTP create cannot express what it needs
/// (two commands created in the SAME instant).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
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
            error_code,
            body,
        }
    }

    async fn create_command(&self, command_id: &str, body: &Value) -> Reply {
        self.call("PUT", &format!("/commands/{command_id}"), Some(body))
            .await
    }

    async fn update_command(&self, command_id: &str, body: &Value) -> Reply {
        self.call("PATCH", &format!("/commands/{command_id}"), Some(body))
            .await
    }

    async fn get_command(&self, command_id: &str) -> Reply {
        self.call("GET", &format!("/commands/{command_id}"), None)
            .await
    }

    /// `ListCommands` with `query` as its query string (every member of this
    /// operation's input is bound to one).
    async fn list(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/commands".to_string()
        } else {
            format!("/commands?{query}")
        };
        self.call("GET", &path, None).await
    }

    /// Record one command in the store directly, at the creation instant `created_at`:
    /// the only way to give two commands the SAME instant, which an HTTP create (whose
    /// instant is a clock reading) cannot.
    fn seed_command_at(&self, command_id: &str, created_at: SystemTime) {
        let state = service_state(&*self.engine).expect("the IoT state store");
        let mut store = state.lock();
        let command = RegisteredCommand::new(
            command_id.to_string(),
            command_arn(REGION, command_id),
            created_at,
            &CreateCommandInput {
                command_id: command_id.to_string(),
                ..CreateCommandInput::default()
            },
        );
        store.commands.insert(command_id.to_string(), command);
    }
}

/// The ARN a command created in [`REGION`] must carry.
fn expected_arn(command_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:command/{command_id}")
}

/// One command the store is seeded with, through the wire path a client uses.
struct Seed {
    label: &'static str,
    command_id: String,
    body: Value,
}

/// A command id of the longest form `CommandId` admits (`length 1..64`), so the table
/// spans the bound rather than only ids of a convenient length.
fn longest_command_id() -> String {
    format!("{}{}", "l".repeat(63), "9")
}

/// The commands every case below lists, in the order they are created — which is the
/// order the listing's `sortOrder` runs over.
///
/// The rows span:
///
/// * the form and length of `commandId` — modelled `length 1..64,
///   pattern ^[a-zA-Z0-9_-]+$` (`smithy_query(query="operation",operation="CreateCommand")`)
///   — so they include the shortest legal id, the longest, and one using every
///   character class the pattern allows;
/// * which namespace the command is created in: both `CommandNamespace` values plus a
///   request that names none;
/// * how many parameters it declares and of which name form —
///   `CommandParameterName` is `length 1..192, pattern ^[.$a-zA-Z0-9_-]+$`
///   (`smithy_query(query="shape",shape="CommandParameterName")`) — including a name
///   two commands share and names using the `.`, `$`, `_` and `-` characters the
///   pattern allows;
/// * which optional members it carries: `displayName`, `description`, a static
///   `payload`, a `payloadTemplate` with its `preprocessor`, `tags`.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            label: "only the required command id",
            command_id: "a".to_string(),
            body: json!({}),
        },
        Seed {
            label: "a static AWS-IoT command with a display name",
            command_id: "reboot_device-1".to_string(),
            body: json!({
                "namespace": BASE_NAMESPACE,
                "displayName": "Reboot the device",
                "description": "Ask the device to reboot immediately",
                "payload": { "content": "AAEC", "contentType": "application/octet-stream" },
            }),
        },
        Seed {
            label: "a dynamic command declaring two parameters",
            command_id: "set-temperature".to_string(),
            body: json!({
                "namespace": BASE_NAMESPACE,
                "displayName": "Set the target temperature",
                "payloadTemplate": "{\"target\":\"${target}\"}",
                "preprocessor": { "awsJsonSubstitution": { "outputFormat": "JSON" } },
                "mandatoryParameters": [
                    { "name": "target", "type": "DOUBLE", "defaultValue": { "D": 21.5 } },
                    { "name": "zone.name", "type": "STRING", "value": { "S": "living" } },
                ],
            }),
        },
        Seed {
            label: "a FleetWise command declaring a parameter the row above also declares",
            command_id: longest_command_id(),
            body: json!({
                "namespace": FLEETWISE_NAMESPACE,
                "roleArn": "arn:aws:iam::000000000000:role/iot-fleetwise-command",
                "payloadTemplate": "{\"signal\":\"${target}\"}",
                "preprocessor": { "awsJsonSubstitution": { "outputFormat": "CBOR" } },
                "mandatoryParameters": [
                    { "name": "target", "type": "STRING" },
                    { "name": "$fleet-id_0", "type": "STRING" },
                ],
            }),
        },
        Seed {
            label: "a tagged FleetWise command with no parameters",
            command_id: "EU_Command-01".to_string(),
            body: json!({
                "namespace": FLEETWISE_NAMESPACE,
                "roleArn": "arn:aws:iam::000000000000:role/iot-fleetwise-eu",
                "displayName": "EU command",
                "tags": [{ "Key": "team", "Value": "platform" }],
            }),
        },
    ]
}

/// The emulator with [`seeds`] created in order, and the ids in CREATION order.
async fn seeded() -> (Emulator, Vec<String>) {
    let emulator = Emulator::new();
    let mut created = Vec::new();
    for seed in seeds() {
        let reply = emulator.create_command(&seed.command_id, &seed.body).await;
        assert_eq!(
            reply.status, 200,
            "seeding {:?}: expected 200, got {} with {:?}",
            seed.label, reply.status, reply.body
        );
        created.push(seed.command_id.clone());
    }
    (emulator, created)
}

/// `ids` reversed — the order a listing that names no `sortOrder` reports them in.
fn newest_first(ids: &[String]) -> Vec<String> {
    ids.iter().rev().cloned().collect()
}

/// RULE: the listing reports every command the store holds as the summary of the very
/// record `GetCommand` reads — the id, the minted ARN, the display name, the lifecycle
/// flags and the two instants — newest first.
#[tokio::test]
async fn the_listing_reports_every_stored_command_as_its_summary() {
    let (emulator, created) = seeded().await;

    let listed = emulator.list("").await;
    assert_eq!(
        listed.status, 200,
        "expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.command_ids(),
        newest_first(&created),
        "the listing must report every created command, newest first, got {:?}",
        listed.body
    );
    assert_eq!(
        listed.next_token(),
        None,
        "a store smaller than one page carries no nextToken, got {:?}",
        listed.body
    );

    for seed in seeds() {
        let case = seed.label;
        let summary = listed
            .summary(&seed.command_id)
            .unwrap_or_else(|| panic!("case {case:?}: the listing must report this command"));
        assert_eq!(
            summary.get("commandArn").and_then(Value::as_str),
            Some(expected_arn(&seed.command_id).as_str()),
            "case {case:?}: expected the minted ARN, got {summary:?}"
        );
        assert_eq!(
            summary.get("displayName"),
            seed.body.get("displayName"),
            "case {case:?}: expected displayName {:?}, got {summary:?}",
            seed.body.get("displayName")
        );
        // A created command is neither deprecated nor being deleted: both flags are
        // part of the record, not of the request.
        for (member, expected) in [("deprecated", false), ("pendingDeletion", false)] {
            assert_eq!(
                summary.get(member).and_then(Value::as_bool),
                Some(expected),
                "case {case:?}: expected {member} {expected}, got {summary:?}"
            );
        }
        // The summary is a projection of the stored record, so every member it shares
        // with the single-command reader is the SAME value — including the instants,
        // which are real clock readings rather than anything derived from the request.
        let read = emulator.get_command(&seed.command_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created command must be readable, got {read:?}"
        );
        for member in [
            "commandId",
            "commandArn",
            "displayName",
            "deprecated",
            "pendingDeletion",
            "createdAt",
            "lastUpdatedAt",
        ] {
            assert_eq!(
                summary.get(member),
                read.member(member),
                "case {case:?}: the listing and the reader must agree about {member}, got \
                 {summary:?} and {read:?}"
            );
        }
        // The summary is the subset `CommandSummary` declares: a listing is not a read
        // of the whole command.
        for member in ["payload", "payloadTemplate", "mandatoryParameters", "roleArn"] {
            assert_eq!(
                summary.get(member),
                None,
                "case {case:?}: a summary carries no {member}, got {summary:?}"
            );
        }
    }
}

/// RULE: a store that holds no command answers with an EMPTY list and no continuation
/// token — "nothing matches" is an answer, not a missing one.
#[tokio::test]
async fn an_empty_store_lists_nothing() {
    let emulator = Emulator::new();

    for case in ["", "sortOrder=ASCENDING", "namespace=AWS-IoT", "maxResults=1"] {
        let listed = emulator.list(case).await;
        assert_eq!(
            (listed.status, listed.member("commands"), listed.next_token()),
            (200, Some(&json!([])), None),
            "case {case:?}: expected an empty listing, got {} with {:?}",
            listed.status,
            listed.body
        );
    }
}

/// RULE: `sortOrder` decides which way the CREATION order runs, and the default is
/// descending — "By default, the API returns all commands in the descending order based
/// on the time that they were created".
#[tokio::test]
async fn sort_order_decides_which_way_the_creation_order_runs() {
    let (emulator, created) = seeded().await;

    let cases: [(&str, &str, Vec<String>); 3] = [
        ("no sortOrder", "", newest_first(&created)),
        ("DESCENDING", "sortOrder=DESCENDING", newest_first(&created)),
        ("ASCENDING", "sortOrder=ASCENDING", created.clone()),
    ];
    for (case, query, expected) in cases {
        let listed = emulator.list(query).await;
        assert_eq!(
            listed.command_ids(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE: the id breaks a tie between two commands created in the same instant, so the
/// order — and therefore every page boundary — is total.
#[tokio::test]
async fn commands_created_in_the_same_instant_are_ordered_by_id() {
    let emulator = Emulator::new();

    // One instant, three commands: only the id can tell them apart.
    let instant = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    let ids = ["tie-a", "tie-b", "tie-c"];
    for command_id in ids {
        emulator.seed_command_at(command_id, instant);
    }

    let descending = emulator.list("").await;
    assert_eq!(
        descending.command_ids(),
        vec!["tie-c", "tie-b", "tie-a"],
        "the descending listing must order a tie by id, got {:?}",
        descending.body
    );
    let ascending = emulator.list("sortOrder=ASCENDING").await;
    assert_eq!(
        ascending.command_ids(),
        vec!["tie-a", "tie-b", "tie-c"],
        "the ascending listing must order a tie by id, got {:?}",
        ascending.body
    );

    // A page boundary inside the tie resumes after the command it ended on, so the walk
    // reports each of them exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..ids.len() {
        let query = match token.as_deref() {
            Some(token) => format!("maxResults=1&nextToken={token}"),
            None => "maxResults=1".to_string(),
        };
        let page = emulator.list(&query).await;
        walked.extend(page.command_ids());
        token = page.next_token();
    }
    assert_eq!(
        walked,
        vec!["tie-c", "tie-b", "tie-a"],
        "a one-per-page walk of a tie must report each command once, got {walked:?}"
    );
    assert_eq!(
        token, None,
        "the walk must end once every command was reported, got {token:?}"
    );
}

/// RULE: `namespace` selects the commands created in that namespace, and a command
/// created in none is in the base namespace.
#[tokio::test]
async fn the_namespace_filter_selects_the_commands_created_in_it() {
    let (emulator, created) = seeded().await;

    // The ids each namespace selects, read off the seeds: a command created without a
    // namespace is in the base one.
    let expected = |namespace: &str| -> Vec<String> {
        newest_first(&created)
            .into_iter()
            .filter(|command_id| {
                let seed = seeds()
                    .into_iter()
                    .find(|seed| &seed.command_id == command_id)
                    .expect("every listed command was seeded");
                seed.body
                    .get("namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(BASE_NAMESPACE)
                    == namespace
            })
            .collect()
    };

    for namespace in [BASE_NAMESPACE, FLEETWISE_NAMESPACE] {
        let listed = emulator.list(&format!("namespace={namespace}")).await;
        let expected = expected(namespace);
        assert!(
            !expected.is_empty(),
            "case {namespace:?}: the table must cover this namespace"
        );
        assert_eq!(
            listed.command_ids(),
            expected,
            "case {namespace:?}: expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE: `commandParameterName` is "a filter that can be used to display the list of
/// commands that have a specific command parameter name" — the commands whose mandatory
/// parameters declare it.
#[tokio::test]
async fn the_parameter_name_filter_selects_the_commands_declaring_it() {
    let (emulator, created) = seeded().await;

    // The rows span a name two commands declare, a name only one declares, names using
    // the `.`, `$`, `_` and `-` characters `CommandParameterName` admits, and a name no
    // command declares at all.
    let expected = |name: &str| -> Vec<String> {
        newest_first(&created)
            .into_iter()
            .filter(|command_id| {
                let seed = seeds()
                    .into_iter()
                    .find(|seed| &seed.command_id == command_id)
                    .expect("every listed command was seeded");
                seed.body
                    .get("mandatoryParameters")
                    .and_then(Value::as_array)
                    .is_some_and(|parameters| {
                        parameters.iter().any(|parameter| {
                            parameter.get("name").and_then(Value::as_str) == Some(name)
                        })
                    })
            })
            .collect()
    };

    for name in ["target", "zone.name", "$fleet-id_0", "no_such_parameter"] {
        let listed = emulator
            .list(&format!("commandParameterName={name}"))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: expected 200, got {} with {:?}",
            listed.status, listed.body
        );
        let expected = expected(name);
        assert_eq!(
            listed.command_ids(),
            expected,
            "case {name:?}: expected {expected:?}, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {name:?}: a selection smaller than one page carries no nextToken, got {:?}",
            listed.body
        );
    }
}

/// RULE: `maxResults` and `nextToken` walk ONE order — every command exactly once, in
/// the order an unpaged listing reports it — the page a request naming no `maxResults`
/// gets being the documented default of 25, and a token belongs to the listing that
/// issued it.
#[tokio::test]
async fn pagination_walks_the_listing_exactly_once() {
    let emulator = Emulator::new();

    // One more command than the default page, so the default page itself is truncated.
    let total = DEFAULT_PAGE + 5;
    let mut created = Vec::new();
    for ordinal in 0..total {
        let command_id = format!("page-command-{ordinal:02}");
        let reply = emulator.create_command(&command_id, &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "seeding {command_id}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        created.push(command_id);
    }
    let unpaged = newest_first(&created);

    // The rows differ in page size: one per page, a size that divides the store, one
    // that does not, the documented default, and both modelled bounds — the smallest
    // page and a page larger than the store.
    for page_size in [1usize, 5, 7, DEFAULT_PAGE, MAX_PAGE] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let listed = emulator.list(&query).await;
            assert_eq!(
                listed.status, 200,
                "case page size {page_size}: expected 200, got {} with {:?}",
                listed.status, listed.body
            );
            let ids = listed.command_ids();
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page holds at most {page_size}, got {}",
                ids.len()
            );
            walked.extend(ids);
            pages += 1;
            token = listed.next_token();
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total,
                "case page size {page_size}: the walk must end, got {pages} pages so far \
                 with {walked:?}"
            );
        }
        assert_eq!(
            walked, unpaged,
            "case page size {page_size}: the walk must report every command exactly once \
             in the unpaged order, got {walked:?}"
        );
        let expected_pages = total.div_ceil(page_size.min(total));
        assert_eq!(
            pages, expected_pages,
            "case page size {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A request that names no `maxResults` gets the documented default page, and a
    // token because the store holds more.
    let defaulted = emulator.list("").await;
    assert_eq!(
        defaulted.command_ids(),
        unpaged[..DEFAULT_PAGE].to_vec(),
        "a request naming no maxResults must get {DEFAULT_PAGE} results, got {:?}",
        defaulted.body
    );
    let token = defaulted
        .next_token()
        .expect("a truncated listing carries a token");
    let rest = emulator.list(&format!("nextToken={token}")).await;
    assert_eq!(
        rest.command_ids(),
        unpaged[DEFAULT_PAGE..].to_vec(),
        "the default page's token must resume after it, got {:?}",
        rest.body
    );
    assert_eq!(
        rest.next_token(),
        None,
        "the last page carries no token, got {:?}",
        rest.body
    );
}

/// RULE: a request the operation does not admit is refused `ValidationException` (HTTP
/// 400) and lists nothing; a `maxResults` outside the modelled range is refused 400
/// before the listing runs.
#[tokio::test]
async fn a_request_the_operation_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    for ordinal in 0..4 {
        let reply = emulator
            .create_command(&format!("refusal-command-{ordinal}"), &json!({}))
            .await;
        assert_eq!(reply.status, 200, "seeding: got {reply:?}");
    }

    // A token is a position in ONE listing: a token the ascending listing issued names
    // no position of the descending one, and one a filtered listing issued names no
    // position of the unfiltered one.
    let ascending = emulator.list("sortOrder=ASCENDING&maxResults=2").await;
    let ascending_token = ascending
        .next_token()
        .expect("a truncated ascending listing carries a token");
    let filtered = emulator.list("namespace=AWS-IoT&maxResults=2").await;
    let filtered_token = filtered
        .next_token()
        .expect("a truncated filtered listing carries a token");

    let cases: Vec<(&str, String)> = vec![
        (
            "a nextToken that is not a token of any listing",
            "nextToken=not-a-token".to_string(),
        ),
        (
            "a nextToken that is hex but names no position",
            format!("nextToken={}", hex::encode("nonsense")),
        ),
        (
            "a token the ascending listing issued, replayed on the descending one",
            format!("maxResults=2&nextToken={ascending_token}"),
        ),
        (
            "a token a filtered listing issued, replayed on the unfiltered one",
            format!("maxResults=2&nextToken={filtered_token}"),
        ),
    ];
    for (case, query) in cases {
        let refused = emulator.list(&query).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
        assert_eq!(
            refused.member("commands"),
            None,
            "case {case:?}: a refused request lists nothing, got {:?}",
            refused.body
        );
    }

    // `CommandMaxResults` is `range 1..100`: both bounds are served, and a value
    // outside the range is refused before the listing runs.
    for served in ["1", "100"] {
        let listed = emulator.list(&format!("maxResults={served}")).await;
        assert_eq!(
            listed.status, 200,
            "case maxResults={served}: the modelled bound must be served, got {listed:?}"
        );
    }
    for out_of_range in ["0", "-1", "101"] {
        let refused = emulator.list(&format!("maxResults={out_of_range}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={out_of_range}: must be refused 400, got {refused:?}"
        );
    }

    // A token the issuing listing replays still works: refusing a token is about the
    // listing it belongs to, not about tokens.
    let resumed = emulator
        .list(&format!("sortOrder=ASCENDING&maxResults=2&nextToken={ascending_token}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "the issuing listing must accept its own token, got {resumed:?}"
    );
}

/// RULE: the listing projects the record as it IS, so a write another operation made is
/// what the next listing reports — `UpdateCommand` moving the display name, the
/// deprecation flag and the last-update instant.
#[tokio::test]
async fn an_update_is_reported_by_the_next_listing() {
    let (emulator, created) = seeded().await;
    let command_id = created.first().expect("a seeded command").clone();

    let before = emulator
        .list("")
        .await
        .summary(&command_id)
        .cloned()
        .expect("the created command is listed");

    let updated = emulator
        .update_command(
            &command_id,
            &json!({ "displayName": "Renamed command", "deprecated": true }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "the update must be served, got {updated:?}"
    );

    let after = emulator
        .list("")
        .await
        .summary(&command_id)
        .cloned()
        .expect("the updated command is still listed");
    assert_eq!(
        after.get("displayName").and_then(Value::as_str),
        Some("Renamed command"),
        "the listing must report the display name the update wrote, got {after:?}"
    );
    assert_eq!(
        after.get("deprecated").and_then(Value::as_bool),
        Some(true),
        "the listing must report the deprecation the update made, got {after:?}"
    );
    assert_eq!(
        after.get("createdAt"),
        before.get("createdAt"),
        "an update does not move the creation instant, got {after:?} after {before:?}"
    );
    let last_updated = after
        .get("lastUpdatedAt")
        .and_then(Value::as_f64)
        .expect("a listed command reports when it was last updated");
    let created_at = after
        .get("createdAt")
        .and_then(Value::as_f64)
        .expect("a listed command reports when it was created");
    assert!(
        last_updated >= created_at,
        "the update instant must not precede the creation instant, got {last_updated} \
         after {created_at}"
    );

    // The listing and the single-command reader report the SAME record.
    let read = emulator.get_command(&command_id).await;
    for member in ["displayName", "deprecated", "createdAt", "lastUpdatedAt"] {
        assert_eq!(
            after.get(member),
            read.member(member),
            "the listing and the reader must agree about {member}, got {after:?} and {read:?}"
        );
    }
}
