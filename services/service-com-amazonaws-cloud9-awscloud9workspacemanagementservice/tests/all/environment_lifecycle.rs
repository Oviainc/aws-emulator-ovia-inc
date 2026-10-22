// SPDX-License-Identifier: CC0-1.0

//! End-to-end `awsJson1_1` tests for the Cloud9 environment lifecycle:
//! `CreateEnvironmentEC2` -> `DescribeEnvironments` ->
//! `DescribeEnvironmentStatus` -> `ListEnvironments` -> `UpdateEnvironment` ->
//! `DeleteEnvironment`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSCloud9WorkspaceManagementService.<Operation>`), so the
//! boundary validator, the protocol codec and the handlers all run exactly as
//! they do for an SDK client.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_cloud9_awscloud9workspacemanagementservice as cloud9;

use cloud9::ops::types::{
    CONNECTION_TYPE_VALUES, ENVIRONMENT_LIFECYCLE_STATUS_VALUES, ENVIRONMENT_STATUS_VALUES,
    ENVIRONMENT_TYPE_VALUES, MANAGED_CREDENTIALS_ACTION_VALUES, MANAGED_CREDENTIALS_STATUS_VALUES,
    PERMISSIONS_VALUES,
};
use cloud9::state::{
    ComAmazonawsCloud9Awscloud9workspacemanagementserviceState as Cloud9State, ConnectionType,
    Environment, EnvironmentLifecycleStatus, EnvironmentStatus, EnvironmentType,
    ManagedCredentialsAction, ManagedCredentialsStatus, Permissions, CREATION_SETTLES_AFTER,
    ENVIRONMENTS_PER_ACCOUNT, MAX_ENVIRONMENTS_PER_PAGE,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope every request in these tests carries. Its 3rd
/// field is the region the handlers build ARNs in and its 1st is the access-key
/// id they derive the default `ownerArn` from.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDCLOUD9TEST/20260101/us-west-2/cloud9/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
const REGION: &str = "us-west-2";
/// `engine::EMULATOR_ACCOUNT_ID`, restated so an ARN assertion reads whole.
const ACCOUNT: &str = "000000000000";
/// What `ownerArn` defaults to for the credential above.
const DEFAULT_OWNER_ARN: &str = "arn:aws:iam::000000000000:user/AKIDCLOUD9TEST";

/// One emulator holding one Cloud9 service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(cloud9::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation over the wire; returns the HTTP status and the parsed
    /// JSON body (`Value::Null` for an empty body).
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCloud9WorkspaceManagementService.{operation}"),
            )
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("request body serializes"),
            ))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require a 200.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {status}: {parsed}");
        parsed
    }

    /// The stored record, straight out of the shared engine. Used for the
    /// members `Environment` does not put on the wire (instance type, image id,
    /// subnet, stop time, tags, memberships), which no implemented reader
    /// exposes yet.
    fn stored(&self, environment_id: &str) -> Option<Environment> {
        self.engine
            .state::<Cloud9State>()
            .lock()
            .environments
            .get(environment_id)
            .cloned()
    }

    fn stored_count(&self) -> usize {
        self.engine.state::<Cloud9State>().lock().environments.len()
    }

    fn idempotency_count(&self) -> usize {
        self.engine.state::<Cloud9State>().lock().idempotency.len()
    }
}

/// The error code an error body names, for an assertion that does not depend on
/// whether the protocol prefixes the shape id with its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `CreateEnvironmentEC2` request, as a row of the table below.
#[derive(Debug, Clone)]
struct CreateCase {
    label: &'static str,
    name: String,
    instance_type: &'static str,
    image_id: &'static str,
    description: Option<String>,
    subnet_id: Option<&'static str>,
    connection_type: Option<&'static str>,
    automatic_stop_time_minutes: Option<i32>,
    owner_arn: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    fn body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "instanceType": self.instance_type,
            "imageId": self.image_id,
        });
        let map = body.as_object_mut().expect("object");
        if let Some(description) = &self.description {
            map.insert("description".into(), json!(description));
        }
        if let Some(subnet_id) = self.subnet_id {
            map.insert("subnetId".into(), json!(subnet_id));
        }
        if let Some(connection_type) = self.connection_type {
            map.insert("connectionType".into(), json!(connection_type));
        }
        if let Some(minutes) = self.automatic_stop_time_minutes {
            map.insert("automaticStopTimeMinutes".into(), json!(minutes));
        }
        if let Some(owner_arn) = self.owner_arn {
            map.insert("ownerArn".into(), json!(owner_arn));
        }
        if !self.tags.is_empty() {
            map.insert(
                "tags".into(),
                Value::Array(
                    self.tags
                        .iter()
                        // The modeled member names are `Key` and `Value`.
                        .map(|(key, value)| json!({"Key": key, "Value": value}))
                        .collect(),
                ),
            );
        }
        body
    }

    /// The connection type the model documents as the default when the request
    /// names none.
    fn expected_connection_type(&self) -> &str {
        self.connection_type.unwrap_or("CONNECT_SSH")
    }

    /// "If this value is not specified, the ARN defaults to this environment's
    /// creator."
    fn expected_owner_arn(&self) -> &str {
        self.owner_arn.unwrap_or(DEFAULT_OWNER_ARN)
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The table every create/describe assertion runs over. The rows differ along
/// what the create rule generalises over: name length (both modeled `@length`
/// bounds), instance-type length and shape, image-id length (its `min: 0`
/// bound included), whether a description / subnet / owner / connection type is
/// given, the `automaticStopTimeMinutes` `@range` bounds, and tag count.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members, name at @length(min: 1)",
            name: "a".to_string(),
            instance_type: "t2.micro",
            image_id: "amazonlinux-2-x86_64",
            description: None,
            subnet_id: None,
            connection_type: None,
            automatic_stop_time_minutes: None,
            owner_arn: None,
            tags: Vec::new(),
        },
        CreateCase {
            label: "every member, at the upper @length/@range bounds",
            name: "n".repeat(60),
            // 20 characters: `InstanceType`'s @length(max: 20).
            instance_type: "t2.aaaaaaaaaaaaaaaaa",
            image_id: "resolve:ssm:/aws/service/cloud9/amis/amazonlinux-2023-x86_64",
            description: Some("d".repeat(200)),
            // 24 characters: the 17-hex form of `SubnetId`.
            subnet_id: Some("subnet-0123456789abcdef0"),
            connection_type: Some("CONNECT_SSM"),
            automatic_stop_time_minutes: Some(20160),
            owner_arn: Some("arn:aws:iam::000000000000:user/other-owner"),
            tags: vec![("Team", "dev"), ("Env", "test"), ("Cost", "42")],
        },
        CreateCase {
            label: "lower bounds: empty description, zero stop time, 8-hex subnet",
            name: "demo-env".to_string(),
            // 5 characters: `InstanceType`'s @length(min: 5).
            instance_type: "t2.sm",
            image_id: "",
            description: Some(String::new()),
            // 15 characters: the 8-hex form of `SubnetId`.
            subnet_id: Some("subnet-0123abcd"),
            connection_type: Some("CONNECT_SSH"),
            automatic_stop_time_minutes: Some(0),
            owner_arn: None,
            tags: vec![("Only", "one")],
        },
        CreateCase {
            label: "an assumed-role owner and a large instance type",
            name: "big build box".to_string(),
            instance_type: "m5.24xlarge",
            image_id: "ubuntu-22.04-x86_64",
            description: Some("builds the release artifacts".to_string()),
            subnet_id: None,
            connection_type: Some("CONNECT_SSM"),
            automatic_stop_time_minutes: Some(30),
            owner_arn: Some("arn:aws:sts::000000000000:assumed-role/Admin/alice"),
            tags: Vec::new(),
        },
        CreateCase {
            label: "the account root as owner",
            name: "root-owned".to_string(),
            instance_type: "c5.9xlarge",
            image_id: "amazonlinux-2023-x86_64",
            description: None,
            subnet_id: Some("subnet-00ff11ee"),
            connection_type: None,
            automatic_stop_time_minutes: Some(1),
            owner_arn: Some("arn:aws:iam::000000000000:root"),
            tags: vec![("Owner", "root")],
        },
    ]
}

/// The whole slice, over the whole table: a created environment is persisted,
/// gets an id of the modeled shape, and reads back through
/// `DescribeEnvironments` with the values its creator supplied — never an echo
/// of the describe request, and never a canned value.
#[tokio::test]
async fn a_created_environment_reads_back_with_the_values_it_was_created_with() {
    let emulator = Emulator::new();
    let cases = create_cases();

    for (index, case) in cases.iter().enumerate() {
        let created = emulator.ok("CreateEnvironmentEC2", case.body()).await;
        let id = created
            .get("environmentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_environment_id(&id),
            "case {:?}: environmentId must match ^[a-zA-Z0-9]{{8,32}}$, got {id:?}",
            case.label
        );
        assert_eq!(
            emulator.stored_count(),
            index + 1,
            "case {:?}: the create must persist exactly one new environment, store holds {}",
            case.label,
            emulator.stored_count()
        );

        let described = emulator
            .ok("DescribeEnvironments", json!({"environmentIds": [&id]}))
            .await;
        let environments = described
            .get("environments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            environments.len(),
            1,
            "case {:?}: expected exactly one environment, got {described}",
            case.label
        );
        let described = &environments[0];

        let field = |name: &str| -> String {
            described
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
                .to_string()
        };
        assert_eq!(
            field("id"),
            id,
            "case {:?}: id must be the created id, got {described}",
            case.label
        );
        assert_eq!(
            field("name"),
            case.name,
            "case {:?}: name must be the created name, got {described}",
            case.label
        );
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: description must be the created description, got {described}",
            case.label
        );
        assert_eq!(
            field("type"),
            "ec2",
            "case {:?}: CreateEnvironmentEC2 makes `ec2` environments, got {described}",
            case.label
        );
        assert_eq!(
            field("connectionType"),
            case.expected_connection_type(),
            "case {:?}: connectionType must be the created one (CONNECT_SSH by default), got {described}",
            case.label
        );
        assert_eq!(
            field("ownerArn"),
            case.expected_owner_arn(),
            "case {:?}: ownerArn must be the created one, or the creator's, got {described}",
            case.label
        );
        assert_eq!(
            field("arn"),
            format!("arn:aws:cloud9:{REGION}:{ACCOUNT}:environment:{id}"),
            "case {:?}: arn must name this region, account and environment, got {described}",
            case.label
        );
        assert_eq!(
            field("managedCredentialsStatus"),
            ManagedCredentialsStatus::EnabledOnCreate.as_wire(),
            "case {:?}: credentials enabled as part of the create, got {described}",
            case.label
        );

        // Members the `Environment` shape does not carry are still persisted,
        // so the operations that expose them have a record to read.
        let record = emulator
            .stored(&id)
            .unwrap_or_else(|| panic!("case {:?}: environment {id} must be stored", case.label));
        assert_eq!(
            record.instance_type, case.instance_type,
            "case {:?}: instanceType must be stored, got {:?}",
            case.label, record.instance_type
        );
        assert_eq!(
            record.image_id, case.image_id,
            "case {:?}: imageId must be stored, got {:?}",
            case.label, record.image_id
        );
        assert_eq!(
            record.subnet_id.as_deref(),
            case.subnet_id,
            "case {:?}: subnetId must be stored, got {:?}",
            case.label,
            record.subnet_id
        );
        assert_eq!(
            record.automatic_stop_time_minutes, case.automatic_stop_time_minutes,
            "case {:?}: automaticStopTimeMinutes must be stored, got {:?}",
            case.label, record.automatic_stop_time_minutes
        );
        assert_eq!(
            record.tags,
            case.expected_tags(),
            "case {:?}: tags must be stored, got {:?}",
            case.label,
            record.tags
        );
        assert_eq!(
            record.members.len(),
            1,
            "case {:?}: the creator is seeded as the single member, got {:?}",
            case.label,
            record.members
        );
        assert_eq!(
            record.members[0].permissions,
            Permissions::Owner,
            "case {:?}: the creator's membership is `owner`, got {:?}",
            case.label,
            record.members[0]
        );
        assert_eq!(
            record.members[0].user_arn,
            case.expected_owner_arn(),
            "case {:?}: the owner member names the owner ARN, got {:?}",
            case.label,
            record.members[0]
        );
        assert_eq!(
            record.members[0].environment_id, id,
            "case {:?}: the membership names its environment, got {:?}",
            case.label, record.members[0]
        );
        assert!(
            !record.members[0].user_id.is_empty(),
            "case {:?}: the owner member carries a userId, got {:?}",
            case.label,
            record.members[0]
        );
    }

    // Distinct creates mint distinct ids, and nothing collides.
    let ids: Vec<String> = emulator
        .engine
        .state::<Cloud9State>()
        .lock()
        .environments
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        ids.len(),
        cases.len(),
        "every create must produce its own environment, got {ids:?}"
    );
}

/// Two environments created from the same ARN share one stable `userId`; two
/// different owners do not. The id is computed from the owner ARN, so it is
/// neither random per call nor shared by unrelated principals.
#[tokio::test]
async fn the_owner_members_user_id_is_stable_per_principal() {
    let emulator = Emulator::new();
    let owners = [
        "arn:aws:iam::000000000000:user/alice",
        "arn:aws:iam::000000000000:user/alice",
        "arn:aws:iam::000000000000:user/bob",
        "arn:aws:iam::000000000000:root",
    ];
    let mut user_ids = Vec::new();
    for owner in owners {
        let created = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": "owned",
                    "instanceType": "t3.small",
                    "imageId": "amazonlinux-2023-x86_64",
                    "ownerArn": owner,
                }),
            )
            .await;
        let id = created["environmentId"].as_str().expect("an id").to_string();
        let record = emulator.stored(&id).expect("stored");
        user_ids.push(record.members[0].user_id.clone());
    }
    assert_eq!(
        user_ids[0], user_ids[1],
        "the same owner ARN must give the same userId, got {user_ids:?}"
    );
    assert_ne!(
        user_ids[1], user_ids[2],
        "different owners must give different userIds, got {user_ids:?}"
    );
    assert_ne!(
        user_ids[2], user_ids[3],
        "different owners must give different userIds, got {user_ids:?}"
    );
    for user_id in &user_ids {
        assert!(
            user_id.starts_with("AIDA") && user_id.len() == 21,
            "a userId keeps IAM's shape, got {user_id:?}"
        );
    }
}

/// A fresh environment is reported `creating` (lifecycle `CREATING`) until the
/// transition the clock owes falls due, then `ready` (lifecycle `CREATED`) —
/// with a non-empty `message` in both states, which the model makes
/// `@required`.
#[tokio::test]
async fn a_fresh_environment_settles_from_creating_to_ready() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateEnvironmentEC2",
            json!({
                "name": "settling",
                "instanceType": "t2.micro",
                "imageId": "amazonlinux-2023-x86_64",
            }),
        )
        .await;
    let id = created["environmentId"].as_str().expect("an id").to_string();

    let transient = emulator
        .ok("DescribeEnvironmentStatus", json!({"environmentId": &id}))
        .await;
    assert_eq!(
        transient.get("status").and_then(Value::as_str),
        Some(EnvironmentStatus::Creating.as_wire()),
        "a just-created environment is still creating, got {transient}"
    );
    assert!(
        !transient
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "the @required message must not be empty, got {transient}"
    );
    let described = emulator
        .ok("DescribeEnvironments", json!({"environmentIds": [&id]}))
        .await;
    assert_eq!(
        described["environments"][0]["lifecycle"]["status"].as_str(),
        Some(EnvironmentLifecycleStatus::Creating.as_wire()),
        "the lifecycle agrees with the status while creating, got {described}"
    );

    // Past the due instant the transition has been owed, so it shows.
    // `std::thread::sleep`, not `tokio::time::sleep`: the workspace pins tokio
    // with only the `macros`/`rt`/`rt-multi-thread` features, so `tokio::time`
    // is not available, and the root manifest is not this crate's to change.
    // Each `#[tokio::test]` gets its own current-thread runtime, so blocking
    // this one holds up nothing else.
    std::thread::sleep(CREATION_SETTLES_AFTER + Duration::from_millis(200));

    let settled = emulator
        .ok("DescribeEnvironmentStatus", json!({"environmentId": &id}))
        .await;
    assert_eq!(
        settled.get("status").and_then(Value::as_str),
        Some(EnvironmentStatus::Ready.as_wire()),
        "once the transition is due the environment is ready, got {settled}"
    );
    assert!(
        !settled
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "the @required message must not be empty, got {settled}"
    );
    let described = emulator
        .ok("DescribeEnvironments", json!({"environmentIds": [&id]}))
        .await;
    assert_eq!(
        described["environments"][0]["lifecycle"]["status"].as_str(),
        Some(EnvironmentLifecycleStatus::Created.as_wire()),
        "the lifecycle agrees with the status once ready, got {described}"
    );

    // A read does not move the status on: it stays ready for the next caller.
    let again = emulator
        .ok("DescribeEnvironmentStatus", json!({"environmentId": &id}))
        .await;
    assert_eq!(
        again.get("status").and_then(Value::as_str),
        Some(EnvironmentStatus::Ready.as_wire()),
        "a second read reports the same status, got {again}"
    );
}

/// `ListEnvironments` returns every stored id, in one stable order, split into
/// pages of at most `maxResults`; the token resumes exactly where the previous
/// page ended and is absent on the last page. The table varies the two things
/// the rule generalises over: how many environments exist and how large a page
/// the caller asks for (including the `@range(min: 0)` bound and no request at
/// all).
#[tokio::test]
async fn list_environments_pages_the_whole_store_in_a_stable_order() {
    // (environments to create, requested maxResults)
    let cases: [(usize, Option<i32>); 6] = [
        (2, Some(1)),
        (5, Some(2)),
        (7, Some(3)),
        (4, Some(25)),
        (3, None),
        (4, Some(0)),
    ];

    for (total, max_results) in cases {
        let emulator = Emulator::new();
        for index in 0..total {
            emulator
                .ok(
                    "CreateEnvironmentEC2",
                    json!({
                        "name": format!("env-{index}"),
                        "instanceType": "t2.micro",
                        "imageId": "amazonlinux-2023-x86_64",
                    }),
                )
                .await;
        }
        let expected: Vec<String> = emulator
            .engine
            .state::<Cloud9State>()
            .lock()
            .environments
            .keys()
            .cloned()
            .collect();

        // An absent or zero page size means "no caller-chosen limit": the
        // member's own modeled maximum.
        let page_size = match max_results {
            Some(requested) if requested > 0 => requested as usize,
            _ => MAX_ENVIRONMENTS_PER_PAGE,
        };

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            let map = body.as_object_mut().expect("object");
            if let Some(requested) = max_results {
                map.insert("maxResults".into(), json!(requested));
            }
            if let Some(token) = &next_token {
                map.insert("nextToken".into(), json!(token));
            }
            let listed = emulator.ok("ListEnvironments", body).await;
            let page: Vec<String> = listed
                .get("environmentIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {total}/{max_results:?}: a page must hold at most {page_size} ids, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {total}/{max_results:?}: pagination must terminate, saw {pages} pages"
            );
            next_token = listed
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {total}/{max_results:?}: the pages must concatenate to the whole store, in order"
        );
        assert_eq!(
            pages,
            total.div_ceil(page_size).max(1),
            "case {total}/{max_results:?}: {total} ids at {page_size} per page"
        );
    }
}

/// A `nextToken` the service did not issue is a `BadRequestException`, not a
/// silent first page — including one that decodes cleanly but carries no
/// resume key of this service's own form, which a plain hex token would.
#[tokio::test]
async fn an_unrecognized_next_token_is_refused() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": format!("env-{index}"),
                    "instanceType": "t2.micro",
                    "imageId": "amazonlinux-2023-x86_64",
                }),
            )
            .await;
    }

    let tokens = [
        // Not an encoding this service produces at all.
        "not-a-token".to_string(),
        "zz".to_string(),
        "GGGG".to_string(),
        // Decodes, but to text that is not one of this service's resume keys.
        hex::encode("hello world!"),
        // Decodes to a bare environment id without the service's own marker.
        hex::encode("aaaaaaaaaaaaaaaa"),
        // Carries the marker but no id of the modeled `EnvironmentId` shape.
        hex::encode("cloud9:environment:not an id!"),
        hex::encode("cloud9:environment:short"),
    ];
    for token in tokens {
        let (status, body) = emulator
            .call("ListEnvironments", json!({"nextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "token {token:?} must be refused with 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "token {token:?} must be a BadRequestException, got {body}"
        );
    }
}

/// One `UpdateEnvironment` request, as a row of the table below.
#[derive(Debug, Clone, Copy)]
struct UpdateCase {
    label: &'static str,
    name: Option<&'static str>,
    description: Option<&'static str>,
    managed_credentials_action: Option<&'static str>,
}

/// `UpdateEnvironment` replaces the members the request carries and leaves the
/// ones it omits exactly as they were. The table covers each member alone, both
/// together, each `managedCredentialsAction`, and a request that carries none.
#[tokio::test]
async fn update_environment_replaces_present_members_and_leaves_absent_ones() {
    let cases = [
        UpdateCase {
            label: "name only",
            name: Some("renamed"),
            description: None,
            managed_credentials_action: None,
        },
        UpdateCase {
            label: "description only",
            name: None,
            description: Some("a new description"),
            managed_credentials_action: None,
        },
        UpdateCase {
            label: "name and description",
            name: Some("both"),
            description: Some("changed"),
            managed_credentials_action: None,
        },
        UpdateCase {
            label: "credentials disabled",
            name: None,
            description: None,
            managed_credentials_action: Some("DISABLE"),
        },
        UpdateCase {
            label: "credentials enabled",
            name: None,
            description: None,
            managed_credentials_action: Some("ENABLE"),
        },
        UpdateCase {
            label: "nothing at all",
            name: None,
            description: None,
            managed_credentials_action: None,
        },
    ];

    for case in cases {
        let UpdateCase {
            label,
            name: new_name,
            description: new_description,
            managed_credentials_action: action,
        } = case;
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": "original",
                    "instanceType": "t2.micro",
                    "imageId": "amazonlinux-2023-x86_64",
                    "description": "the original description",
                }),
            )
            .await;
        let id = created["environmentId"].as_str().expect("an id").to_string();

        let mut body = json!({"environmentId": &id});
        let map = body.as_object_mut().expect("object");
        if let Some(name) = new_name {
            map.insert("name".into(), json!(name));
        }
        if let Some(description) = new_description {
            map.insert("description".into(), json!(description));
        }
        if let Some(action) = action {
            map.insert("managedCredentialsAction".into(), json!(action));
        }
        let updated = emulator.ok("UpdateEnvironment", body).await;
        assert!(
            updated.as_object().is_some_and(serde_json::Map::is_empty),
            "case {label:?}: UpdateEnvironment has an empty output, got {updated}"
        );

        let described = emulator
            .ok("DescribeEnvironments", json!({"environmentIds": [&id]}))
            .await;
        let environment = &described["environments"][0];
        assert_eq!(
            environment["name"].as_str(),
            Some(new_name.unwrap_or("original")),
            "case {label:?}: an absent name leaves the stored one, got {described}"
        );
        assert_eq!(
            environment["description"].as_str(),
            Some(new_description.unwrap_or("the original description")),
            "case {label:?}: an absent description leaves the stored one, got {described}"
        );
        let expected_credentials = match action {
            None => ManagedCredentialsStatus::EnabledOnCreate,
            Some(action) => ManagedCredentialsAction::from_wire(action)
                .expect("a modeled action")
                .resulting_status(),
        };
        assert_eq!(
            environment["managedCredentialsStatus"].as_str(),
            Some(expected_credentials.as_wire()),
            "case {label:?}: the credentials status follows the action, got {described}"
        );
    }
}

/// `DeleteEnvironment` empties the body and removes the environment from every
/// reader: describe, status and update all answer `NotFoundException`, and the
/// list no longer carries it. The surviving environments are untouched.
#[tokio::test]
async fn delete_environment_removes_it_from_every_reader() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    for index in 0..3 {
        let created = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": format!("env-{index}"),
                    "instanceType": "t2.micro",
                    "imageId": "amazonlinux-2023-x86_64",
                }),
            )
            .await;
        ids.push(created["environmentId"].as_str().expect("an id").to_string());
    }
    let doomed = ids[1].clone();

    let deleted = emulator
        .ok("DeleteEnvironment", json!({"environmentId": &doomed}))
        .await;
    assert!(
        deleted.as_object().is_some_and(serde_json::Map::is_empty),
        "DeleteEnvironment has an empty output, got {deleted}"
    );

    let readers: [(&str, Value); 3] = [
        ("DescribeEnvironments", json!({"environmentIds": [&doomed]})),
        ("DescribeEnvironmentStatus", json!({"environmentId": &doomed})),
        ("UpdateEnvironment", json!({"environmentId": &doomed, "name": "zombie"})),
    ];
    for (operation, body) in readers {
        let (status, parsed) = emulator.call(operation, body).await;
        assert_eq!(
            status, 400,
            "{operation} on a deleted environment is a 400, got {status}: {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "NotFoundException",
            "{operation} on a deleted environment is a NotFoundException, got {parsed}"
        );
    }

    // A second delete is a NotFoundException too: the record is gone.
    let (status, parsed) = emulator
        .call("DeleteEnvironment", json!({"environmentId": &doomed}))
        .await;
    assert_eq!(status, 400, "a second delete is a 400, got {status}: {parsed}");
    assert_eq!(
        error_code(&parsed),
        "NotFoundException",
        "a second delete is a NotFoundException, got {parsed}"
    );

    let listed = emulator.ok("ListEnvironments", json!({})).await;
    let remaining: Vec<String> = listed["environmentIds"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    assert!(
        !remaining.contains(&doomed),
        "the deleted id must not be listed, got {remaining:?}"
    );
    for survivor in [&ids[0], &ids[2]] {
        assert!(
            remaining.contains(survivor),
            "{survivor} must survive the delete, got {remaining:?}"
        );
        let described = emulator
            .ok("DescribeEnvironments", json!({"environmentIds": [survivor]}))
            .await;
        assert_eq!(
            described["environments"][0]["id"].as_str(),
            Some(survivor.as_str()),
            "{survivor} must still describe, got {described}"
        );
    }
}

/// An id that satisfies the modeled pattern but names no stored environment is a
/// `NotFoundException` on every operation that resolves one.
#[tokio::test]
async fn an_unknown_environment_id_is_not_found_everywhere() {
    let emulator = Emulator::new();
    // Pattern-valid ids the store has never held: at the `{8,32}` bounds and
    // between them.
    let longest = "f".repeat(32);
    let unknown = ["aaaaaaaa", "A1b2C3d4E5f6", longest.as_str()];
    for id in unknown {
        let probes: [(&str, Value); 4] = [
            ("DescribeEnvironments", json!({"environmentIds": [id]})),
            ("DescribeEnvironmentStatus", json!({"environmentId": id})),
            ("UpdateEnvironment", json!({"environmentId": id, "name": "x"})),
            ("DeleteEnvironment", json!({"environmentId": id})),
        ];
        for (operation, body) in probes {
            let (status, parsed) = emulator.call(operation, body).await;
            assert_eq!(
                status, 400,
                "{operation} with unknown id {id:?} is a 400, got {status}: {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "NotFoundException",
                "{operation} with unknown id {id:?} is a NotFoundException, got {parsed}"
            );
        }
    }
}

/// A request that breaks one of the model's own rules — an absent `@required`
/// member, or a value outside a declared `@length` / `@pattern` / `@range` — is
/// refused with HTTP 400 and the error code this service models for an invalid
/// request, `BadRequestException`. One row per rule, each one step past the
/// published bound.
#[tokio::test]
async fn a_request_that_breaks_a_modeled_rule_is_a_bad_request() {
    let emulator = Emulator::new();
    let long_name = "n".repeat(61);
    let long_description = "d".repeat(201);
    let long_instance_type = format!("t2.{}", "a".repeat(18));
    let too_many_ids: Vec<String> = (0..26).map(|index| format!("env{index:05}")).collect();

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name shorter than @length(min: 1)",
            "CreateEnvironmentEC2",
            json!({"name": "", "instanceType": "t2.micro", "imageId": "ami"}),
        ),
        (
            "name longer than @length(max: 60)",
            "CreateEnvironmentEC2",
            json!({"name": long_name, "instanceType": "t2.micro", "imageId": "ami"}),
        ),
        (
            "description longer than @length(max: 200)",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "description": long_description}),
        ),
        (
            "instanceType shorter than @length(min: 5)",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.a", "imageId": "ami"}),
        ),
        (
            "instanceType longer than @length(max: 20)",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": long_instance_type, "imageId": "ami"}),
        ),
        (
            "instanceType outside @pattern",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "Bogus", "imageId": "ami"}),
        ),
        (
            "subnetId outside @pattern",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "subnetId": "subnet-zzzzzzzz"}),
        ),
        (
            "ownerArn outside @pattern",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "ownerArn": "not-an-arn"}),
        ),
        (
            "clientRequestToken outside @pattern",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "clientRequestToken": "short"}),
        ),
        (
            "automaticStopTimeMinutes below @range(min: 0)",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "automaticStopTimeMinutes": -1}),
        ),
        (
            "automaticStopTimeMinutes above @range(max: 20160)",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro", "imageId": "ami", "automaticStopTimeMinutes": 20161}),
        ),
        (
            "absent @required name",
            "CreateEnvironmentEC2",
            json!({"instanceType": "t2.micro", "imageId": "ami"}),
        ),
        (
            "absent @required instanceType",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "imageId": "ami"}),
        ),
        (
            "absent @required imageId",
            "CreateEnvironmentEC2",
            json!({"name": "ok", "instanceType": "t2.micro"}),
        ),
        (
            "environmentIds shorter than @length(min: 1)",
            "DescribeEnvironments",
            json!({"environmentIds": []}),
        ),
        (
            "environmentIds longer than @length(max: 25)",
            "DescribeEnvironments",
            json!({"environmentIds": too_many_ids}),
        ),
        (
            "absent @required environmentIds",
            "DescribeEnvironments",
            json!({}),
        ),
        (
            "environmentId outside @pattern",
            "DescribeEnvironmentStatus",
            json!({"environmentId": "not a valid id!"}),
        ),
        (
            "absent @required environmentId",
            "DescribeEnvironmentStatus",
            json!({}),
        ),
        (
            "environmentId shorter than the @pattern's {8,32}",
            "UpdateEnvironment",
            json!({"environmentId": "short"}),
        ),
        (
            "environmentId longer than the @pattern's {8,32}",
            "DeleteEnvironment",
            json!({"environmentId": "a".repeat(33)}),
        ),
        (
            "maxResults above @range(max: 25)",
            "ListEnvironments",
            json!({"maxResults": 26}),
        ),
        (
            "maxResults below @range(min: 0)",
            "ListEnvironments",
            json!({"maxResults": -1}),
        ),
    ];

    for (label, operation, body) in cases {
        let (status, parsed) = emulator.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {label:?} on {operation}: expected HTTP 400, got {status}: {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "BadRequestException",
            "case {label:?} on {operation}: expected BadRequestException, got {parsed}"
        );
    }
    assert_eq!(
        emulator.stored_count(),
        0,
        "a refused request must persist nothing, store holds {}",
        emulator.stored_count()
    );
}

/// `CreateEnvironmentEC2` is `@idempotent`: the same `clientRequestToken`
/// answers with the id it created the first time and creates no second
/// environment, while distinct tokens each create their own.
#[tokio::test]
async fn a_repeated_client_request_token_creates_one_environment() {
    let emulator = Emulator::new();
    // Tokens at the pattern's `{10,128}` bounds and between them.
    let longest = "t".repeat(128);
    let tokens = ["0123456789", "a token with spaces", longest.as_str()];
    let mut first_ids = Vec::new();
    for token in tokens {
        let created = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": "idempotent",
                    "instanceType": "t2.micro",
                    "imageId": "amazonlinux-2023-x86_64",
                    "clientRequestToken": token,
                }),
            )
            .await;
        first_ids.push(created["environmentId"].as_str().expect("an id").to_string());
    }
    assert_eq!(
        emulator.stored_count(),
        tokens.len(),
        "each distinct token creates its own environment, store holds {}",
        emulator.stored_count()
    );

    for (token, expected_id) in tokens.iter().zip(&first_ids) {
        // A repeat with the SAME token, and even a different name, resolves to
        // the environment the token already created.
        let repeated = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": "a different name",
                    "instanceType": "m5.large",
                    "imageId": "ubuntu-22.04-x86_64",
                    "clientRequestToken": token,
                }),
            )
            .await;
        assert_eq!(
            repeated["environmentId"].as_str(),
            Some(expected_id.as_str()),
            "token {token:?} must resolve to the id it created, got {repeated}"
        );
    }
    assert_eq!(
        emulator.stored_count(),
        tokens.len(),
        "a repeated token must create nothing new, store holds {}",
        emulator.stored_count()
    );

    // And the first create's values are the ones that stuck.
    let described = emulator
        .ok(
            "DescribeEnvironments",
            json!({"environmentIds": [&first_ids[0]]}),
        )
        .await;
    assert_eq!(
        described["environments"][0]["name"].as_str(),
        Some("idempotent"),
        "a repeat must not overwrite the stored record, got {described}"
    );
}

/// `dryRun` validates and persists nothing, and says so with a modeled error
/// rather than an empty success: the model documents an EC2-style dry run as
/// returning an error response.
#[tokio::test]
async fn a_dry_run_persists_nothing() {
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateEnvironmentEC2",
            json!({
                "name": "real",
                "instanceType": "t2.micro",
                "imageId": "amazonlinux-2023-x86_64",
                "clientRequestToken": "already-used-token",
            }),
        )
        .await;
    let real_id = created["environmentId"].as_str().expect("an id").to_string();

    for dry_run_body in [
        json!({"name": "dry", "instanceType": "t2.micro", "imageId": "ami", "dryRun": true}),
        json!({"name": "dry with a token", "instanceType": "m5.large", "imageId": "ami", "dryRun": true, "clientRequestToken": "0123456789"}),
        // A dry run must be refused even when its token ALREADY created an
        // environment: the idempotency replay must not turn a dry run into a
        // success that reports an id.
        json!({"name": "dry with a used token", "instanceType": "t2.micro", "imageId": "ami", "dryRun": true, "clientRequestToken": "already-used-token"}),
    ] {
        let (status, dry) = emulator.call("CreateEnvironmentEC2", dry_run_body).await;
        assert_eq!(
            status, 400,
            "a dry run is refused, not answered with an empty success, got {status}: {dry}"
        );
        assert_eq!(
            error_code(&dry),
            "BadRequestException",
            "a dry run carries the modeled client-input error, got {dry}"
        );
        assert_eq!(
            dry.get("environmentId").and_then(Value::as_str),
            None,
            "a dry run mints no environmentId, got {dry}"
        );
    }
    assert_eq!(
        emulator.stored_count(),
        1,
        "a dry run persists nothing, store holds {}",
        emulator.stored_count()
    );

    let listed = emulator.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        listed["environmentIds"],
        json!([real_id]),
        "a dry run leaves the list alone, got {listed}"
    );

    // `dryRun: false` is an ordinary create.
    let wet = emulator
        .ok(
            "CreateEnvironmentEC2",
            json!({"name": "not dry", "instanceType": "t2.micro", "imageId": "ami", "dryRun": false}),
        )
        .await;
    assert!(
        wet.get("environmentId").and_then(Value::as_str).is_some(),
        "dryRun: false creates for real, got {wet}"
    );
    assert_eq!(
        emulator.stored_count(),
        2,
        "dryRun: false persists, store holds {}",
        emulator.stored_count()
    );
}

/// The enums `crate::state` stores values in must cover exactly the value sets
/// the Smithy model declares — the generated `*_VALUES` constants in
/// `ops::types::shared_types`. Without this, a stored value could serialize to
/// something the model does not allow.
#[test]
fn the_stored_enums_cover_exactly_the_modeled_value_sets() {
    fn check(label: &str, mine: Vec<&'static str>, modeled: &[&str]) {
        let mut mine_sorted = mine;
        mine_sorted.sort_unstable();
        let mut modeled_sorted = modeled.to_vec();
        modeled_sorted.sort_unstable();
        assert_eq!(
            mine_sorted, modeled_sorted,
            "{label}: the stored enum must cover exactly the modeled values"
        );
    }

    check(
        "EnvironmentType",
        EnvironmentType::ALL.iter().map(|v| v.as_wire()).collect(),
        ENVIRONMENT_TYPE_VALUES,
    );
    check(
        "ConnectionType",
        ConnectionType::ALL.iter().map(|v| v.as_wire()).collect(),
        CONNECTION_TYPE_VALUES,
    );
    check(
        "EnvironmentStatus",
        EnvironmentStatus::ALL.iter().map(|v| v.as_wire()).collect(),
        ENVIRONMENT_STATUS_VALUES,
    );
    check(
        "EnvironmentLifecycleStatus",
        EnvironmentLifecycleStatus::ALL
            .iter()
            .map(|v| v.as_wire())
            .collect(),
        ENVIRONMENT_LIFECYCLE_STATUS_VALUES,
    );
    check(
        "ManagedCredentialsStatus",
        ManagedCredentialsStatus::ALL
            .iter()
            .map(|v| v.as_wire())
            .collect(),
        MANAGED_CREDENTIALS_STATUS_VALUES,
    );
    check(
        "ManagedCredentialsAction",
        ManagedCredentialsAction::ALL
            .iter()
            .map(|v| v.as_wire())
            .collect(),
        MANAGED_CREDENTIALS_ACTION_VALUES,
    );
    check(
        "Permissions",
        Permissions::ALL.iter().map(|v| v.as_wire()).collect(),
        PERMISSIONS_VALUES,
    );

    // Every wire value parses back to the variant it came from.
    for value in ConnectionType::ALL {
        assert_eq!(
            ConnectionType::from_wire(value.as_wire()),
            Some(*value),
            "ConnectionType {:?} must round-trip",
            value.as_wire()
        );
    }
    for value in ManagedCredentialsAction::ALL {
        assert_eq!(
            ManagedCredentialsAction::from_wire(value.as_wire()),
            Some(*value),
            "ManagedCredentialsAction {:?} must round-trip",
            value.as_wire()
        );
    }
    for value in Permissions::ALL {
        assert_eq!(
            Permissions::from_wire(value.as_wire()),
            Some(*value),
            "Permissions {:?} must round-trip",
            value.as_wire()
        );
    }
    assert_eq!(
        ConnectionType::from_wire("CONNECT_TELEPATHY"),
        None,
        "an unmodeled connection type must not parse"
    );
}

/// `^[a-zA-Z0-9]{8,32}$`, the modeled `EnvironmentId` pattern.
fn is_environment_id(candidate: &str) -> bool {
    (8..=32).contains(&candidate.len())
        && candidate.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The store is BOUNDED: once the account holds `ENVIRONMENTS_PER_ACCOUNT`
/// environments a further create is refused with the modeled
/// `LimitExceededException` and persists nothing, and deleting one makes room
/// again. The idempotency map never outgrows the store, so cycling distinct
/// `clientRequestToken` values cannot grow it past the cap either.
#[tokio::test]
async fn the_environment_store_is_bounded_by_the_account_quota() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    for index in 0..ENVIRONMENTS_PER_ACCOUNT {
        let created = emulator
            .ok(
                "CreateEnvironmentEC2",
                json!({
                    "name": format!("env-{index}"),
                    "instanceType": "t2.micro",
                    "imageId": "amazonlinux-2023-x86_64",
                    "clientRequestToken": format!("token-{index:06}-pad"),
                }),
            )
            .await;
        ids.push(created["environmentId"].as_str().expect("an id").to_string());
    }
    assert_eq!(
        emulator.stored_count(),
        ENVIRONMENTS_PER_ACCOUNT,
        "the store must fill to exactly the cap, holds {}",
        emulator.stored_count()
    );

    // One past the cap — with no token, and with tokens nothing has used yet —
    // is refused, and nothing is recorded for those tokens.
    for token in [None, Some("a-brand-new-token"), Some("another-new-token")] {
        let mut body = json!({
            "name": "one too many",
            "instanceType": "t2.micro",
            "imageId": "amazonlinux-2023-x86_64",
        });
        if let Some(token) = token {
            body.as_object_mut()
                .expect("object")
                .insert("clientRequestToken".into(), json!(token));
        }
        let (status, refused) = emulator.call("CreateEnvironmentEC2", body).await;
        assert_eq!(
            status, 400,
            "a create past the cap (token {token:?}) is refused, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "LimitExceededException",
            "a create past the cap (token {token:?}) is a LimitExceededException, got {refused}"
        );
    }
    assert_eq!(
        emulator.stored_count(),
        ENVIRONMENTS_PER_ACCOUNT,
        "a refused create persists nothing, store holds {}",
        emulator.stored_count()
    );
    assert!(
        emulator.idempotency_count() <= emulator.stored_count(),
        "the idempotency map must never outgrow the store: {} tokens for {} environments",
        emulator.idempotency_count(),
        emulator.stored_count()
    );

    // Deleting one makes room for exactly one more, and the delete drops the
    // token that named it.
    emulator
        .ok("DeleteEnvironment", json!({"environmentId": &ids[0]}))
        .await;
    assert_eq!(
        emulator.stored_count(),
        ENVIRONMENTS_PER_ACCOUNT - 1,
        "the delete must free a slot, store holds {}",
        emulator.stored_count()
    );
    assert!(
        emulator.idempotency_count() <= emulator.stored_count(),
        "the delete must drop the token that named the removed environment: {} tokens for {} environments",
        emulator.idempotency_count(),
        emulator.stored_count()
    );
    let created = emulator
        .ok(
            "CreateEnvironmentEC2",
            json!({
                "name": "replacement",
                "instanceType": "t2.micro",
                "imageId": "amazonlinux-2023-x86_64",
            }),
        )
        .await;
    assert!(
        created
            .get("environmentId")
            .and_then(Value::as_str)
            .is_some_and(is_environment_id),
        "a create below the cap succeeds again, got {created}"
    );
}
