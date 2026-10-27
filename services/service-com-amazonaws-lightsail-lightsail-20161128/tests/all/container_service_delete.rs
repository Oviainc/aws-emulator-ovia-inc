// SPDX-License-Identifier: CC0-1.0

//! Deleting a Lightsail container service, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. A fetch of a service's published public URL is an unsigned plain
//! HTTP request, exactly as a client following that URL makes it. The harness is
//! this suite's own: no sibling suite deletes a container service, and these
//! cases need both the AWS caller and that raw fetch.
//!
//! The rule under test: `DeleteContainerService` removes the ONE container
//! service record the name it carries addresses in the Region the request is made
//! in, together with everything that lives on that record — its power and scale,
//! its tags, its deployments, the container images registered to it and the
//! public domain names it served — so no later read reports any of it, the
//! address it published stops answering, and the name is free for a service that
//! inherits nothing from this one. Nothing else moves with it: another service of
//! the account, and a service of the SAME name in another Region, both survive
//! the delete. A name that addresses no container service in that Region —
//! never created, created only in another Region, or deleted a moment ago — is
//! the service's `DoesNotExist` rejection rather than a reported deletion that
//! did not happen.
//!
//! So the cases are TABLES: one row per lifecycle state the service is in when
//! the delete is made, crossed with the Region, the power, the scale, how many
//! tags it carries, whether it is running a deployment that publishes an endpoint
//! and how short its name is; and the refusals are a table over the kinds of name
//! that address no container service.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The operation under test, by the name its `X-Amz-Target` carries.
const DELETE: &str = "DeleteContainerService";

/// The endpoint every call in this suite reaches the emulator on, which is the
/// address a container service's `url` is therefore announced under.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The path space a container service's public URL is published under
/// (`crate::state::CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`).
const PUBLIC_PATH_PREFIX: &str = "/lightsail-container-service";

/// The wire error code (awsJson `__type`) of Lightsail's rejection of a name
/// that addresses nothing, and the `code` its body carries — which a Lightsail
/// SDK client branches on rather than on the HTTP status.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The modelled `ResourceType` member a container service reports, which the
/// refusal's message names.
const RESOURCE_TYPE: &str = "ContainerService";

/// How long a clock-owed lifecycle phase is given to arrive.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: a control-plane call routes by `X-Amz-Target`
        // and names its Region in the credential SCOPE alone, and a fetch of a
        // published public URL is unsigned by construction.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body` in `region`, answering the HTTP status and
    /// the parsed body (`Value::Null` for an answer that carries none).
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
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

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(&self, case: &str, region: &str, operation: &str, body: Value, want: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want,
            "case {case}: expected HTTP {want} from {operation} in {region}, got {status} \
             {parsed:?}"
        );
        parsed
    }

    /// Create the container service `body` describes, answering the
    /// `containerService` the create reports. `CreateContainerService` answers
    /// HTTP 201, its modelled success status.
    async fn create(&self, case: &str, region: &str, body: Value) -> Value {
        self.ok(case, region, "CreateContainerService", body, 201)
            .await["containerService"]
            .clone()
    }

    /// The container service `name` in `region`, as `GetContainerServices`
    /// reports it.
    async fn read(&self, case: &str, region: &str, name: &str) -> Value {
        let body = self
            .ok(
                case,
                region,
                "GetContainerServices",
                json!({"serviceName": name}),
                200,
            )
            .await;
        let services = body["containerServices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an array, got {body:?}"))
            .clone();
        assert_eq!(
            services.len(),
            1,
            "case {case}: expected exactly the one named service, got {services:?}"
        );
        services[0].clone()
    }

    /// Every container service name `GetContainerServices` reports in `region`.
    async fn list(&self, case: &str, region: &str) -> BTreeSet<String> {
        let body = self
            .ok(case, region, "GetContainerServices", json!({}), 200)
            .await;
        body["containerServices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an array, got {body:?}"))
            .iter()
            .map(|service| {
                service["containerServiceName"]
                    .as_str()
                    .unwrap_or("<no name>")
                    .to_string()
            })
            .collect()
    }

    /// Every name `GetActiveNames` reports in `region` — the account's held
    /// names, whatever kind of resource holds each.
    async fn active_names(&self, case: &str, region: &str) -> BTreeSet<String> {
        let body = self
            .ok(case, region, "GetActiveNames", json!({}), 200)
            .await;
        body["activeNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an array, got {body:?}"))
            .iter()
            .map(|name| name.as_str().unwrap_or("<no name>").to_string())
            .collect()
    }

    /// Call `DeleteContainerService` for `name` in `region`.
    async fn delete(&self, region: &str, name: &str) -> (u16, Value) {
        self.call(region, DELETE, json!({"serviceName": name})).await
    }

    /// Follow a published public URL: `GET path`, unsigned and with no AWS
    /// header of any kind, answering the status that address serves.
    async fn fetch(&self, path: &str) -> u16 {
        let request = ::http::Request::builder()
            .method("GET")
            .uri(path)
            .header("host", ENDPOINT_HOST)
            .body(bytes::Bytes::new())
            .expect("the request is well formed");
        self.router.dispatch(request).await.status().as_u16()
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

/// Assert that `refused` is this service's `DoesNotExist` envelope for `name`:
/// HTTP 400, wire error `NotFoundException`, body `code` `DoesNotExist`, and a
/// message naming the resource type and the identifier the request carried.
fn assert_does_not_exist(case: &str, name: &str, status: u16, refused: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(refused),
        NOT_FOUND,
        "case {case}: expected the {NOT_FOUND} wire code, got {refused:?}"
    );
    assert_eq!(
        refused["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the {DOES_NOT_EXIST} body code, got {refused:?}"
    );
    let message = refused["message"].as_str().unwrap_or("<no message>");
    assert!(
        message.contains(RESOURCE_TYPE) && message.contains(name),
        "case {case}: expected a message naming the {RESOURCE_TYPE} and '{name}', got \
         {refused:?}"
    );
}

/// Assert that `body` is `DeleteContainerService`'s modelled success: HTTP 204
/// (the operation's modelled success status) carrying no output member, the
/// modelled `DeleteContainerServiceResult` having none.
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 204,
        "case {case}: expected HTTP 204, got {status} {body:?}"
    );
    assert!(
        *body == Value::Null || *body == json!({}),
        "case {case}: expected no output members, got {body:?}"
    );
}

/// Poll `GetContainerServices` until the service `name` reports `want`,
/// answering the whole record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, case: &str, region: &str, name: &str, want: &str) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let service = emulator.read(case, region, name).await;
        if service["state"].as_str() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected '{want}' within {SETTLE_TIMEOUT:?}, got {:?}",
            service["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The path of the public URL `service` announces, which must be published under
/// this emulator's own endpoint and path space.
fn public_path(case: &str, service: &Value) -> String {
    let url = service["url"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an announced url, got {service:?}"));
    let (_, path) = url
        .split_once(ENDPOINT_HOST)
        .unwrap_or_else(|| panic!("case {case}: expected a url at {ENDPOINT_HOST}, got {url}"));
    assert!(
        path.starts_with(PUBLIC_PATH_PREFIX),
        "case {case}: expected the url under {PUBLIC_PATH_PREFIX}, got {url}"
    );
    path.to_string()
}

// ---------------------------------------------------------------------------
// The table: what the service looked like when the delete was made
// ---------------------------------------------------------------------------

/// One container service to create and then delete. The rows differ along
/// everything the rule generalises over: the Region the requests are made in, the
/// power, the scale, how many tags the service carries, whether it is running a
/// deployment that publishes a public endpoint, how short the name is, and which
/// lifecycle state the service is in when the delete arrives — including one made
/// while the create's own transitions are still outstanding.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    power: &'static str,
    scale: i64,
    tags: &'static [(&'static str, &'static str)],
    /// Whether the create carries a deployment that declares a public endpoint.
    with_deployment: bool,
    /// Whether `UpdateContainerService` disables the service before the delete.
    disable_first: bool,
    /// The state to wait for before deleting, or `None` to delete while the
    /// create's own lifecycle is still outstanding (the service is `PENDING`).
    delete_in_state: Option<&'static str>,
}

/// The table. The powers are `crate::state::CONTAINER_SERVICE_POWERS`' published
/// plans and the scales the bounds of the modelled `ContainerServiceScale`
/// range (1..20); `ab` is as short as the modelled `ContainerServiceName`
/// pattern (`^[a-z0-9]{1,2}|[a-z0-9][a-z0-9-]+[a-z0-9]$`) admits.
const ROWS: &[Row] = &[
    Row {
        case: "deleted while the create's own lifecycle is still outstanding",
        region: "us-east-1",
        name: "pending-service",
        power: "nano",
        scale: 1,
        tags: &[],
        with_deployment: false,
        disable_first: false,
        delete_in_state: None,
    },
    Row {
        case: "deleted once it has settled with no deployment to serve",
        region: "us-east-2",
        name: "ready-service",
        power: "micro",
        scale: 2,
        tags: &[("env", "test")],
        with_deployment: false,
        disable_first: false,
        delete_in_state: Some("READY"),
    },
    Row {
        case: "deleted while it is running a deployment that publishes an endpoint",
        region: "us-west-2",
        name: "running-service",
        power: "small",
        scale: 4,
        tags: &[],
        with_deployment: true,
        disable_first: false,
        delete_in_state: Some("RUNNING"),
    },
    Row {
        case: "deleted while disabled, at the top of the modelled scale range",
        region: "eu-central-1",
        name: "disabled-service",
        power: "medium",
        scale: 20,
        tags: &[("env", "staging"), ("owner", "platform")],
        with_deployment: false,
        disable_first: true,
        delete_in_state: Some("DISABLED"),
    },
    Row {
        case: "a name as short as the modelled pattern admits",
        region: "ap-northeast-1",
        name: "ab",
        power: "xlarge",
        scale: 1,
        tags: &[],
        with_deployment: false,
        disable_first: false,
        delete_in_state: Some("READY"),
    },
];

/// The `CreateContainerService` body for `row`, under `name`.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({"serviceName": name, "power": row.power, "scale": row.scale});
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    if row.with_deployment {
        body["deployment"] = json!({
            "containers": {"web": {"image": "nginx:stable", "ports": {"80": "HTTP"}}},
            "publicEndpoint": {"containerName": "web", "containerPort": 80},
        });
    }
    body
}

/// The Region a row's bystander of the same name is created in: another Region
/// than the row's own, so the delete's Region scoping is observable.
fn other_region(row: &Row) -> &'static str {
    if row.region == "us-east-1" {
        "us-west-2"
    } else {
        "us-east-1"
    }
}

// ---------------------------------------------------------------------------
// A delete removes the whole record, and nothing else
// ---------------------------------------------------------------------------

/// RULE: the delete removes the one record its name addresses in the Region the
/// request is made in, with everything that lives on it, whatever state the
/// service is in — so every reader stops serving it, the address it published
/// stops answering, the name is free for a service that inherits nothing from it,
/// and another service of the account and a service of the same name in another
/// Region both survive it.
#[tokio::test]
async fn a_delete_removes_the_whole_record_and_nothing_else() {
    for row in ROWS {
        // One emulator per row: each row stands up its own account, so the names
        // it asserts over are exactly the ones it created.
        let emulator = Emulator::new();
        let case = row.case;
        let region = row.region;
        let elsewhere = other_region(row);
        // A bystander beside it in the same Region, and a service of the SAME
        // name in another Region: neither may move with the delete.
        let bystander = format!("{}-bystander", row.name);

        let created = emulator.create(case, region, create_body(row, row.name)).await;
        emulator
            .create(case, region, create_body(row, &bystander))
            .await;
        emulator
            .create(case, elsewhere, create_body(row, row.name))
            .await;
        let created_arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an ARN, got {created:?}"))
            .to_string();

        if row.disable_first {
            emulator
                .ok(
                    case,
                    region,
                    "UpdateContainerService",
                    json!({"serviceName": row.name, "isDisabled": true}),
                    200,
                )
                .await;
        }
        if let Some(want) = row.delete_in_state {
            wait_for_state(&emulator, case, region, row.name, want).await;
        }

        // What the record serves BEFORE the delete: the registry reader answers
        // for it (an empty list — nothing was ever registered), and a published
        // endpoint answers at the announced address.
        let images_before = emulator
            .call(
                region,
                "GetContainerImages",
                json!({"serviceName": row.name}),
            )
            .await;
        assert_eq!(
            images_before.0, 200,
            "case {case}: expected the registry of a held service to be reported, got \
             {images_before:?}"
        );
        let path = public_path(case, &created);
        // What the address the service published answers while the service is
        // held, read for every row so the post-delete answer below is read
        // against it. A row whose deployment declares a public endpoint answers
        // something OTHER than the 404 of an address with nothing published
        // behind it; WHICH status it answers is
        // `container_service_public_endpoint.rs`'s rule, not this suite's, so
        // only that contrast is pinned here.
        let published_before = emulator.fetch(&path).await;
        if row.with_deployment {
            assert_ne!(
                published_before, 404,
                "case {case}: expected the published endpoint of a held service to answer, got \
                 {published_before}"
            );
        }
        // The delete itself: the modelled empty success.
        let (status, answered) = emulator.delete(region, row.name).await;
        assert_empty_success(case, status, &answered);

        // Every reader has stopped serving the record.
        let (read_status, read) = emulator
            .call(
                region,
                "GetContainerServices",
                json!({"serviceName": row.name}),
            )
            .await;
        assert_does_not_exist(case, row.name, read_status, &read);
        let (images_status, images) = emulator
            .call(
                region,
                "GetContainerImages",
                json!({"serviceName": row.name}),
            )
            .await;
        assert_does_not_exist(case, row.name, images_status, &images);
        assert_eq!(
            emulator.list(case, region).await,
            BTreeSet::from([bystander.clone()]),
            "case {case}: expected only the bystander left in {region}"
        );
        assert!(
            !emulator.active_names(case, region).await.contains(row.name),
            "case {case}: expected the deleted name to hold no name in {region}"
        );
        // The address the deleted service published serves nothing: the modelled
        // 404 of an address with no public endpoint behind it, which is also the
        // answer for a URL naming no stored service at all.
        let published_after = emulator.fetch(&path).await;
        assert_eq!(
            published_after, 404,
            "case {case}: expected the published address of a deleted service to answer nothing, \
             got {published_after} where the held service answered {published_before}"
        );
        // Deleting it again removes nothing a second time.
        let (again_status, again) = emulator.delete(region, row.name).await;
        assert_does_not_exist(case, row.name, again_status, &again);

        // The service of the same name in the other Region is untouched: the
        // name was resolved in the Region the delete was made in.
        let elsewhere_service = emulator.read(case, elsewhere, row.name).await;
        assert_eq!(
            elsewhere_service["containerServiceName"].as_str(),
            Some(row.name),
            "case {case}: expected the same name in {elsewhere} to survive, got \
             {elsewhere_service:?}"
        );
        // The name is free again, and what is created under it inherits nothing
        // from what was deleted: a different power and scale, no tags, no
        // deployment, a fresh ARN and the lifecycle of a brand new service.
        let reused = emulator
            .create(
                case,
                region,
                json!({"serviceName": row.name, "power": "nano", "scale": 1}),
            )
            .await;
        assert_eq!(
            reused["power"].as_str(),
            Some("nano"),
            "case {case}: expected the re-created service's own power, got {reused:?}"
        );
        assert_eq!(
            reused["scale"].as_i64(),
            Some(1),
            "case {case}: expected the re-created service's own scale, got {reused:?}"
        );
        assert_eq!(
            reused["tags"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected no tags on the re-created service, got {reused:?}"
        );
        assert!(
            reused["currentDeployment"].is_null() && reused["nextDeployment"].is_null(),
            "case {case}: expected no deployment on the re-created service, got {reused:?}"
        );
        assert_eq!(
            reused["state"].as_str(),
            Some("PENDING"),
            "case {case}: expected the re-created service to start its own lifecycle, got \
             {reused:?}"
        );
        assert_ne!(
            reused["arn"].as_str(),
            Some(created_arn.as_str()),
            "case {case}: expected a fresh ARN for the re-created service, got {reused:?}"
        );
        // Its registry starts empty rather than reporting the deleted service's.
        let registry = emulator
            .ok(
                case,
                region,
                "GetContainerImages",
                json!({"serviceName": row.name}),
                200,
            )
            .await;
        assert_eq!(
            registry["containerImages"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected an empty registry on the re-created service, got {registry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A name that addresses no container service
// ---------------------------------------------------------------------------

/// How a refusal row's name comes to address no container service in the Region
/// the delete is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Nothing was ever created under the name.
    NeverCreated,
    /// A service of that name was created in this Region and deleted already.
    AlreadyDeleted,
    /// A service of that name is held, but in ANOTHER Region.
    OnlyInAnotherRegion,
}

/// One refusal case: a well-formed name that addresses no container service in
/// the Region the delete is made in. The rows differ along how the name comes to
/// address nothing and along the form of the name itself.
#[derive(Debug, Clone, Copy)]
struct RefusalRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    standing: Standing,
}

/// The table.
const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "a name nothing was ever created under",
        region: "us-east-1",
        name: "no-such-service",
        standing: Standing::NeverCreated,
    },
    RefusalRow {
        case: "the shortest name the modelled pattern admits, addressing nothing",
        region: "eu-west-1",
        name: "z",
        standing: Standing::NeverCreated,
    },
    RefusalRow {
        case: "a hyphenated name addressing nothing",
        region: "ap-south-1",
        name: "a-b-c-d",
        standing: Standing::NeverCreated,
    },
    RefusalRow {
        case: "a name whose service was deleted a moment ago",
        region: "us-west-2",
        name: "gone-service",
        standing: Standing::AlreadyDeleted,
    },
    RefusalRow {
        case: "a name whose service is held in another Region",
        region: "ca-central-1",
        name: "elsewhere-service",
        standing: Standing::OnlyInAnotherRegion,
    },
];

/// The Region a refusal row's service is held in when it is held elsewhere.
const HELD_ELSEWHERE_REGION: &str = "us-east-1";

/// RULE: a name that addresses no container service in the Region the request is
/// made in is the service's `DoesNotExist` rejection — the same envelope the
/// reader answers for that name — and the refused call removes nothing.
#[tokio::test]
async fn a_name_that_addresses_no_container_service_is_refused() {
    for row in REFUSAL_ROWS {
        let emulator = Emulator::new();
        let case = row.case;
        match row.standing {
            Standing::NeverCreated => {}
            Standing::AlreadyDeleted => {
                emulator
                    .create(
                        case,
                        row.region,
                        json!({"serviceName": row.name, "power": "nano", "scale": 1}),
                    )
                    .await;
                let (status, answered) = emulator.delete(row.region, row.name).await;
                assert_empty_success(case, status, &answered);
            }
            Standing::OnlyInAnotherRegion => {
                emulator
                    .create(
                        case,
                        HELD_ELSEWHERE_REGION,
                        json!({"serviceName": row.name, "power": "nano", "scale": 1}),
                    )
                    .await;
            }
        }

        let (status, refused) = emulator.delete(row.region, row.name).await;
        assert_does_not_exist(case, row.name, status, &refused);
        // The delete's refusal of a name is the one the READER answers for it:
        // one `DoesNotExist` envelope serves the whole service.
        let (read_status, read) = emulator
            .call(
                row.region,
                "GetContainerServices",
                json!({"serviceName": row.name}),
            )
            .await;
        assert_eq!(
            (status, refused["message"].as_str()),
            (read_status, read["message"].as_str()),
            "case {case}: expected the delete's refusal to match the reader's, got {refused:?} \
             against {read:?}"
        );

        // The refused call removed nothing: the Region it was made in holds the
        // same services it did, and a service held elsewhere is still held.
        assert_eq!(
            emulator.list(case, row.region).await,
            BTreeSet::new(),
            "case {case}: expected the refused delete to leave {} holding nothing",
            row.region
        );
        if row.standing == Standing::OnlyInAnotherRegion {
            let held = emulator
                .read(case, HELD_ELSEWHERE_REGION, row.name)
                .await;
            assert_eq!(
                held["containerServiceName"].as_str(),
                Some(row.name),
                "case {case}: expected the service held in {HELD_ELSEWHERE_REGION} to survive, \
                 got {held:?}"
            );
        }
    }
}
