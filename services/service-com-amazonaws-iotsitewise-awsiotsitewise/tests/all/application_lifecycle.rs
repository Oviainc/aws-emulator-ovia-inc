// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace application slice
//! (`CreateApplication` -> `DescribeApplication`), driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created application is PERSISTED and reads back through the describe
//!   exactly as it was created, under the ARN that carries the id the create
//!   minted;
//! * every created application reports an id of the modelled shape, a
//!   `dnsSubdomain` that is a DNS label derived from that id, and an
//!   `idcApplicationArn` that names the Identity Center instance the create named
//!   — all three unique per application and stable across reads;
//! * `clientToken` is an idempotency token: a retry carrying a token an earlier
//!   create used answers from the application that create minted and adds no
//!   second application;
//! * an application is addressed by `(workspaceName, id)`, so the same id read
//!   from another workspace, and an id no create minted, are both modelled
//!   not-found refusals;
//! * an application is created IN a workspace, so a create naming one this
//!   service does not hold is a modelled not-found refusal too;
//! * the create owes a `CREATING` -> `ACTIVE` transition the clock makes due, so a
//!   read taken straight afterwards still observes `CREATING`;
//! * a request outside a modelled constraint is refused and persists nothing.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ACTIVATION_DELAY, TAG_KEY_MAX_LENGTH,
};

use crate::asset_lifecycle::{
    call, error_code, is_modelled_id, router, router_with_engine, REGION,
};

/// `POST /applications`, as `OPERATIONS.md` reports the route: the workspace
/// travels in the BODY, not in the path.
const APPLICATIONS: &str = "/applications";

/// `GET /workspaces/{workspaceName}/applications/{id}`.
fn application_uri(workspace: &str, id: &str) -> String {
    format!("/workspaces/{workspace}/applications/{id}")
}

/// The ARN form this emulator mints for an application: its own id, which
/// identifies it across every workspace.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:application/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// An INDEPENDENT oracle for `idcApplicationArn`: Identity Center nests an
/// application under its instance, so the instance ARN's `instance/<instanceId>`
/// resource section becomes `application/<instanceId>/<applicationId>`. Spelled
/// out here rather than reached from the handler's helper, so one derivation bug
/// cannot agree with itself and pass.
fn expected_idc_application_arn(idc_instance_arn: &str, id: &str) -> String {
    match idc_instance_arn.split_once("instance/") {
        Some((prefix, instance_id)) => format!("{prefix}application/{instance_id}/{id}"),
        None => format!("{idc_instance_arn}/application/{id}"),
    }
}

/// Whether `label` is what the modelled `DnsSubdomain` admits: 1..63 characters
/// of `[a-z0-9-]` beginning and ending with an alphanumeric one (smithy_query
/// query="shape" shape="DnsSubdomain": len 1..63, pattern
/// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`).
fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    (1..=63).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| alphanumeric(*byte) || *byte == b'-')
        && bytes.first().copied().is_some_and(alphanumeric)
        && bytes.last().copied().is_some_and(alphanumeric)
}

/// One row of the create-and-read-back table.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    workspace: String,
    name: String,
    description: Option<&'static str>,
    idc_instance_arn: &'static str,
    client_token: Option<&'static str>,
    tags: Option<Value>,
}

/// The rows: the members `CreateApplicationRequest` carries, each varied along
/// what it is allowed to be (smithy_query query="operation"
/// operation="CreateApplication") — `workspaceName` at both ends of its 1..64
/// length and in each form its `^[a-zA-Z0-9_-]+$` pattern admits, `name` spanning
/// the punctuation, spaces and 1..256 length `ApplicationName` admits,
/// `description` present and absent, `idcInstanceArn` in each partition its `ARN`
/// pattern admits and in a form carrying no instance segment at all,
/// `clientToken` present and absent, and `tags` absent, one and several.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the smallest request: required members only",
            workspace: "w".to_string(),
            name: "A".to_string(),
            description: None,
            idc_instance_arn: "arn:aws:sso:::instance/ssoins-1111111111111111",
            client_token: None,
            tags: None,
        },
        CreateCase {
            label: "a hyphenated workspace and a spaced, punctuated name",
            workspace: "plant-floor".to_string(),
            name: "Line 1 (east) v2.0_beta-rc".to_string(),
            description: Some("the application the floor operators open"),
            idc_instance_arn: "arn:aws:sso:::instance/ssoins-2222222222222222",
            client_token: Some("11111111-2222-3333-4444-555555555555"),
            tags: Some(json!({ "team": "ops" })),
        },
        CreateCase {
            label: "an underscored workspace, a description and several tags",
            workspace: "plant_floor_2".to_string(),
            name: "Mixing.Station".to_string(),
            description: Some("second floor"),
            idc_instance_arn: "arn:aws-us-gov:sso:::instance/ssoins-3333333333333333",
            client_token: None,
            tags: Some(json!({ "team": "data", "env": "test" })),
        },
        CreateCase {
            label: "a workspace at the top of its 64-character length",
            workspace: "w".repeat(64),
            name: "Boundary".to_string(),
            description: None,
            idc_instance_arn: "arn:aws-cn:sso:::instance/ssoins-4444444444444444",
            client_token: Some("66666666-7777-8888-9999-aaaaaaaaaaaa"),
            tags: None,
        },
        CreateCase {
            label: "a name at the top of its 256-character length",
            workspace: "boundaries".to_string(),
            name: format!("N{}", "a".repeat(255)),
            description: Some("a description of its own"),
            idc_instance_arn: "arn:aws:sso::123456789012:instance/ssoins-5555555555555555",
            client_token: None,
            tags: Some(json!({ "only": "one" })),
        },
        CreateCase {
            label: "an instance ARN carrying no instance segment",
            workspace: "odd-arns".to_string(),
            name: "No.Instance.Segment".to_string(),
            description: None,
            idc_instance_arn: "arn:aws:sso:::identitystore/d-9999999999",
            client_token: None,
            tags: None,
        },
    ]
}

/// The body one row sends.
fn create_body(case: &CreateCase) -> Value {
    let mut body = json!({
        "workspaceName": case.workspace,
        "name": case.name,
        "idcInstanceArn": case.idc_instance_arn,
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(token) = case.client_token {
        object.insert("clientToken".to_string(), json!(token));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

/// `CreateApplication` with `body`.
pub(crate) async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", APPLICATIONS, Some(body)).await
}

/// `DescribeApplication`.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    workspace: &str,
    id: &str,
) -> (u16, Value) {
    call(router, "GET", &application_uri(workspace, id), None).await
}

/// `CreateWorkspace` for `name`, with the service's default encryption: an
/// application is created IN a workspace, so every suite below opens the
/// workspace it works in first.
pub(crate) async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 200, "create workspace {name}: got {status} {body}");
}

#[tokio::test]
async fn created_application_reads_back_through_describe_for_every_shape() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();
    let mut subdomains: Vec<String> = Vec::new();

    for case in create_cases() {
        create_workspace(&router, &case.workspace).await;
        let (status, created) = create(&router, create_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateApplication expected 200, got {status} {created}",
            case.label
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: expected an id of the modelled shape, got {created}",
            case.label
        );
        assert_eq!(
            created["arn"].as_str().unwrap_or_default(),
            expected_arn(&id),
            "case {:?}: unexpected ARN in {created}",
            case.label
        );
        assert_eq!(
            created["name"].as_str().unwrap_or_default(),
            case.name,
            "case {:?}: the created name must be the one sent, got {created}",
            case.label
        );
        assert_eq!(
            created["status"].as_str().unwrap_or_default(),
            "CREATING",
            "case {:?}: a freshly created application reports CREATING, got {created}",
            case.label
        );
        let subdomain = created["dnsSubdomain"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_dns_label(&subdomain),
            "case {:?}: expected a DNS label as dnsSubdomain, got {created}",
            case.label
        );

        // The round trip: every member the create carried reads back off the
        // stored record, and the two members the create minted agree with what it
        // answered.
        let (status, described) = describe(&router, &case.workspace, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeApplication expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            (
                described["id"].as_str().unwrap_or_default(),
                described["arn"].as_str().unwrap_or_default(),
                described["name"].as_str().unwrap_or_default(),
                described["workspaceName"].as_str().unwrap_or_default(),
                described["dnsSubdomain"].as_str().unwrap_or_default(),
                described["description"].as_str(),
            ),
            (
                id.as_str(),
                expected_arn(&id).as_str(),
                case.name.as_str(),
                case.workspace.as_str(),
                subdomain.as_str(),
                case.description,
            ),
            "case {:?}: the application must read back as it was created, got {described}",
            case.label
        );
        assert_eq!(
            described["idcApplicationArn"].as_str().unwrap_or_default(),
            expected_idc_application_arn(case.idc_instance_arn, &id),
            "case {:?}: the Identity Center application ARN must name the instance \
             the create named, got {described}",
            case.label
        );
        assert_eq!(
            described["status"].as_str().unwrap_or_default(),
            "CREATING",
            "case {:?}: a read taken during the create still observes CREATING, got {described}",
            case.label
        );
        // Nothing has written to the application since the create, so the two
        // instants it reports are the one the create landed at.
        assert_eq!(
            described["createdAt"], described["updatedAt"],
            "case {:?}: a never-updated application reports one instant, got {described}",
            case.label
        );
        assert!(
            described["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {:?}: expected a createdAt instant, got {described}",
            case.label
        );

        // A read changes nothing a later caller sees: the second read answers the
        // first one's body apart from the lifecycle status, which time moves on its
        // own. Compared member by member rather than through a body-minus-status
        // helper, of which this crate's suites already hold copies.
        let (status, again) = describe(&router, &case.workspace, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second read expected 200, got {status} {again}",
            case.label
        );
        for member in [
            "arn",
            "id",
            "name",
            "workspaceName",
            "dnsSubdomain",
            "idcApplicationArn",
            "description",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                again[member], described[member],
                "case {:?}: the second read answered a different {member}: {again}",
                case.label
            );
        }

        // The tags the create carried are persisted on the application and read
        // back through `ListTagsForResource`, which resolves them by its ARN.
        let (status, listed) =
            call(&router, "GET", &format!("/tags?resourceArn={}", expected_arn(&id)), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: unexpected tags in {listed}",
            case.label
        );

        assert!(
            !minted.contains(&id),
            "case {:?}: every create mints a fresh id, but {id} was minted twice",
            case.label
        );
        assert!(
            !subdomains.contains(&subdomain),
            "case {:?}: every application has a subdomain of its own, but {subdomain} repeated",
            case.label
        );
        minted.push(id);
        subdomains.push(subdomain);
    }
}

#[tokio::test]
async fn an_application_is_addressed_by_its_workspace_and_its_id() {
    let router = router();
    create_workspace(&router, "plant-floor").await;
    create_workspace(&router, "other-workspace").await;
    let (status, created) = create(
        &router,
        json!({
            "workspaceName": "plant-floor",
            "name": "Scoped",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1212121212121212",
        }),
    )
    .await;
    assert_eq!(status, 200, "the create got {status} {created}");
    let id = created["id"].as_str().unwrap_or_default().to_string();

    // The same id, read from a workspace the application does not live in.
    let (status, refused) = describe(&router, "other-workspace", &id).await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "another workspace's read of the same id must be refused, got {status} {refused}"
    );

    // An id no create ever minted.
    let unknown = "00000000-0000-4000-8000-0000000000ff";
    let (status, refused) = describe(&router, "plant-floor", unknown).await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "an id no create minted must be refused, got {status} {refused}"
    );

    // The application itself still reads back in its own workspace.
    let (status, described) = describe(&router, "plant-floor", &id).await;
    assert_eq!(
        status, 200,
        "the application must still read back in its own workspace, got {status} {described}"
    );

    // A workspace no create opened is not a scope this service holds.
    let (status, refused) = describe(&router, "no-such-workspace", &id).await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "a read scoped to an unknown workspace must be refused, got {status} {refused}"
    );
}

#[tokio::test]
async fn an_application_is_created_only_in_a_workspace_this_service_holds() {
    let (router, engine) = router_with_engine();
    let body = json!({
        "workspaceName": "not-opened-yet",
        "name": "Early",
        "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1616161616161616",
    });

    let (status, refused) = create(&router, body.clone()).await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "a create naming a workspace no create opened must be refused, got {status} {refused}"
    );
    let state = service_state(&*engine).expect("the iotsitewise store");
    let held = state.lock().applications.len();
    assert_eq!(held, 0, "a refused create must persist nothing, got {held}");

    // Absence is resolved FIRST: a create that both names a workspace this
    // service does not hold and carries a value outside a constraint is still
    // answered as absence, so the refusal names what is actually missing rather
    // than whichever check happens to run first.
    let (status, refused) = create(
        &router,
        json!({
            "workspaceName": "not-opened-yet",
            "name": "EarlyWithABadTagKey",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1616161616161616",
            "tags": { "k".repeat(TAG_KEY_MAX_LENGTH + 1): "value" },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "a create naming an absent workspace is absence however else it is out of \
         bounds, got {status} {refused}"
    );

    // The same request, once the workspace it names exists.
    create_workspace(&router, "not-opened-yet").await;
    let (status, accepted) = create(&router, body).await;
    assert_eq!(
        status, 200,
        "the same create must be accepted once the workspace exists, got {status} {accepted}"
    );
}

#[tokio::test]
async fn a_retried_create_with_the_same_client_token_replays_the_first_answer() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, "plant-floor").await;
    let token = "dcba4321-0000-1111-2222-333344445555";
    let body = json!({
        "workspaceName": "plant-floor",
        "name": "Retried",
        "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1313131313131313",
        "clientToken": token,
    });

    let (status, first) = create(&router, body.clone()).await;
    assert_eq!(status, 200, "the first create got {status} {first}");
    let (status, retried) = create(&router, body.clone()).await;
    assert_eq!(status, 200, "the retry got {status} {retried}");

    assert_eq!(
        (
            retried["id"].as_str(),
            retried["arn"].as_str(),
            retried["dnsSubdomain"].as_str(),
            retried["name"].as_str(),
        ),
        (
            first["id"].as_str(),
            first["arn"].as_str(),
            first["dnsSubdomain"].as_str(),
            first["name"].as_str(),
        ),
        "a retry with the same client token must replay the first answer, got {retried}"
    );

    let state = service_state(&*engine).expect("the iotsitewise store");
    let held = state.lock().applications.len();
    assert_eq!(
        held, 1,
        "a retry with the same client token must add no second application, got {held}"
    );

    // A create carrying no token is not a retry of anything: it mints its own
    // application.
    let (status, untokened) = create(
        &router,
        json!({
            "workspaceName": "plant-floor",
            "name": "Untokened",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1313131313131313",
        }),
    )
    .await;
    assert_eq!(status, 200, "the tokenless create got {status} {untokened}");
    assert_ne!(
        untokened["id"].as_str(),
        first["id"].as_str(),
        "a create carrying no token must mint its own application, got {untokened}"
    );
    let held = state.lock().applications.len();
    assert_eq!(held, 2, "expected two stored applications, got {held}");

    // A token already spent on a DIFFERENT request is not a retry of it: it
    // collides with the application that request made rather than answering from
    // it.
    let (status, collided) = create(
        &router,
        json!({
            "workspaceName": "plant-floor",
            "name": "NotTheSameRequest",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1313131313131313",
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&collided)),
        (409, "ConflictingOperationException"),
        "a token spent on another request must collide, got {status} {collided}"
    );
    assert_eq!(
        collided["resourceId"].as_str(),
        first["id"].as_str(),
        "the conflict must name the application holding the token, got {collided}"
    );
    let held = state.lock().applications.len();
    assert_eq!(
        held, 2,
        "a refused create must persist nothing, got {held} stored applications"
    );

    // The first request in every member but its tags is not a retry of it
    // either: replaying would silently drop the tags the retry carried.
    let (status, tagged) = create(
        &router,
        json!({
            "workspaceName": "plant-floor",
            "name": "Retried",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1313131313131313",
            "clientToken": token,
            "tags": { "team": "ops" },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&tagged)),
        (409, "ConflictingOperationException"),
        "a token spent on a request carrying other tags must collide, got {status} {tagged}"
    );
}

#[tokio::test]
async fn the_create_owes_a_creating_to_active_transition_the_clock_makes_due() {
    let router = router();
    let workspace = "plant-floor";
    create_workspace(&router, workspace).await;
    let (status, created) = create(
        &router,
        json!({
            "workspaceName": workspace,
            "name": "Settling",
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1414141414141414",
        }),
    )
    .await;
    assert_eq!(status, 200, "the create got {status} {created}");
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let (status, during_create) = describe(&router, workspace, &id).await;
    assert_eq!(
        status, 200,
        "the read during the create got {status} {during_create}"
    );
    assert_eq!(
        during_create["status"].as_str().unwrap_or_default(),
        "CREATING",
        "an application reports CREATING until the owed transition falls due, got {during_create}"
    );

    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let (status, settled) = describe(&router, workspace, &id).await;
    assert_eq!(
        status, 200,
        "the read after the create settled got {status} {settled}"
    );
    assert_eq!(
        settled["status"].as_str().unwrap_or_default(),
        "ACTIVE",
        "the owed CREATING -> ACTIVE transition must fall due, got {settled}"
    );
}

#[tokio::test]
async fn a_request_outside_a_constraint_is_refused_and_persists_nothing() {
    let (router, engine) = router_with_engine();
    let instance = "arn:aws:sso:::instance/ssoins-1515151515151515";
    // The workspaces the accepted rows are created in; a refused row never
    // reaches the store, so the workspace it names makes no difference to it.
    create_workspace(&router, &"w".repeat(64)).await;
    create_workspace(&router, "bounds").await;

    // Each row walks one modelled bound: the value just outside it, and — where
    // the bound is a length — the value at it, which must be accepted. Sources:
    // smithy_query query="operation" operation="CreateApplication"
    // (`workspaceName` len 1..64, `name` len 1..256, `clientToken` len 36..64
    // pattern `^\S{36,64}$`) and query="shape" shape="TagKey" for the key bound,
    // which the boundary validator does not reach. Every one of them is refused
    // with the modelled `InvalidRequestException` — "The request isn't valid" —
    // which is what this service answers a constraint violation with.
    let token = "cafebabe-0000-1111-2222-33334444555";
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "a workspace at the top of its length",
            json!({ "workspaceName": "w".repeat(64), "name": "At", "idcInstanceArn": instance }),
            true,
        ),
        (
            "a workspace one character past its length",
            json!({ "workspaceName": "w".repeat(65), "name": "Past", "idcInstanceArn": instance }),
            false,
        ),
        (
            "a workspace below its minimum length",
            json!({ "workspaceName": "", "name": "Empty", "idcInstanceArn": instance }),
            false,
        ),
        (
            "a workspace outside its pattern",
            json!({ "workspaceName": "plant floor", "name": "Spaced", "idcInstanceArn": instance }),
            false,
        ),
        (
            "a name one character past its length",
            json!({
                "workspaceName": "bounds",
                "name": format!("N{}", "a".repeat(256)),
                "idcInstanceArn": instance,
            }),
            false,
        ),
        (
            "a client token one character below its minimum length",
            json!({
                "workspaceName": "bounds",
                "name": "ShortToken",
                "idcInstanceArn": instance,
                "clientToken": token,
            }),
            false,
        ),
        (
            "an instance ARN outside its pattern",
            json!({
                "workspaceName": "bounds",
                "name": "NotAnArn",
                "idcInstanceArn": "not-an-arn",
            }),
            false,
        ),
        (
            "a tag key at the top of its length",
            json!({
                "workspaceName": "bounds",
                "name": "TagKeyAt",
                "idcInstanceArn": instance,
                "tags": { "k".repeat(TAG_KEY_MAX_LENGTH): "value" },
            }),
            true,
        ),
        (
            "a tag key one character past its length",
            json!({
                "workspaceName": "bounds",
                "name": "TagKeyPast",
                "idcInstanceArn": instance,
                "tags": { "k".repeat(TAG_KEY_MAX_LENGTH + 1): "value" },
            }),
            false,
        ),
    ];

    let mut accepted_count = 0usize;
    for (case, body, accepted) in cases {
        let (status, answer) = create(&router, body).await;
        if accepted {
            accepted_count += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: a value inside the bound is accepted, got {status} {answer}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a value outside the bound must be refused, got {status} {answer}"
            );
        }
        // A refused create stores nothing: the store holds exactly the
        // applications the accepted rows created.
        let state = service_state(&*engine).expect("the iotsitewise store");
        let held = state.lock().applications.len();
        assert_eq!(
            held, accepted_count,
            "case {case:?}: expected {accepted_count} stored applications, got {held}"
        );
    }
}
