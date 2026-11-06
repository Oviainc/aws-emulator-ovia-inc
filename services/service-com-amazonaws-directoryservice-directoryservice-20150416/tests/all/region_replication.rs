// SPDX-License-Identifier: CC0-1.0

//! Region replication — `AddRegion` — and the IAM authorization gate every operation of
//! this service that models `AccessDeniedException` takes before it touches the store.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The credential a case signs with is its own, so
//! the identity a request speaks for is what decides it.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a request is authorized at the IAM layer BEFORE the directory it names is resolved:
//!   a caller whose identity-based policies explicitly deny `ds:<Operation>` — or do not
//!   allow it at all — is refused with `AccessDeniedException` naming the principal, the
//!   action and the resource, and the store is left untouched; a caller those same
//!   policies allow reaches the business logic. The rows span the shapes a policy takes
//!   (no policy, an allow of everything, an allow of everything with an explicit deny of
//!   this one action, an allow of exactly this action, an allow scoped to another
//!   directory, an allow of another service) and the gate is exercised on EVERY
//!   implemented operation whose model declares the error;
//! * the Region inventory is reported from the store: `DescribeRegions` answers with the
//!   directory's primary Region and every replication an add recorded, each carrying the
//!   status its own clock-owed transition reached, the placement the add named and the
//!   controller count it adds, narrowed to the Region the request asked for;
//! * the replication itself is state: an accepted `AddRegion` records the Region, so a
//!   second add of it is refused as a Region the directory is already in and
//!   `RemoveRegion` signed for it can stop the replication again (after which the same add
//!   is accepted once more), the directory's own primary Region is refused with
//!   `UnsupportedOperationException`, and so is a directory whose type or edition does not
//!   replicate across Regions.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

/// The `X-Amz-Target` prefix: the service shape's name.
const TARGET_PREFIX: &str = "DirectoryService_20150416";

/// The Region every case below is signed for unless it says otherwise — the one the
/// directories are created in, and so their primary one.
const HOME_REGION: &str = "us-east-1";

/// The Region the accepted replications add.
const OTHER_REGION: &str = "us-west-2";

/// The administrator password the creates below use: the modelled `Password` pattern
/// demands 8..64 characters from three of four character classes.
const PASSWORD: &str = "Sim2Real-Directory-Pw1!";

/// How long the sessions these tests register last — long enough that none expires while
/// the test runs.
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// The access key id the UNRESTRICTED caller signs with: a key neither STS nor IAM holds a
/// record for, which is the account's own published credential.
const ACCOUNT_ACCESS_KEY_ID: &str = "AKIDDSTEST";

/// The wire error code the authorization layer refuses with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The emulator under test: the router and the engine its stores live in, so a case can
/// register the IAM and STS records a credential speaks through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// A router with this service mounted on a fresh engine.
fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise the service, not SigV4: requests carry a credential scope so the
    // router can name the target service and the gate can read the access key id, but no
    // real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// One `awsJson1_1` call signed with `access_key_id` for `region`.
async fn call_as(
    emulator: &Emulator,
    access_key_id: &str,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ds.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/ds/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call as the account's own published credential that must succeed, returning its body.
async fn ok(emulator: &Emulator, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) =
        call_as(emulator, ACCOUNT_ACCESS_KEY_ID, HOME_REGION, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// The modelled error code a refused answer names.
fn error_code(parsed: &Value) -> String {
    parsed["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Register an assumed-role session in STS's OWN store under `access_key_id`, so a request
/// signed with that key speaks for the role, and hold the role itself in IAM's store with
/// one inline policy — the way `iam:CreateRole` and `iam:PutRolePolicy` leave them.
///
/// Written through the two stores rather than by calling those operations, because what is
/// under test is what THIS service does with the principal a credential names, not how the
/// credential or the policy was created.
fn caller(emulator: &Emulator, access_key_id: &str, role_name: &str, document: Option<&str>) {
    let account = ::engine::emulator_account_id();
    let session_arn = format!("arn:aws:sts::{account}:assumed-role/{role_name}/session-1");
    let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
    let now = SystemTime::now();

    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: format!("{}:session-1", sts::role_id(&session_arn)),
                account: account.to_string(),
                arn: session_arn.clone(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_LIFETIME.as_secs() as i32,
        )
    };
    let sts_state =
        sts::service_state(&*emulator.engine).expect("the in-memory engine carries the STS store");
    sts_state
        .lock()
        .insert(vend.into_session(access_key_id, now))
        .expect("the session registry has room for one session");

    let iam_state =
        iam::service_state(&*emulator.engine).expect("the in-memory engine carries the IAM store");
    let mut store = iam_state.lock();
    store.roles.insert(
        iam::name_key(role_name),
        Role {
            path: "/".to_string(),
            role_name: role_name.to_string(),
            role_id: sts::role_id(&role_arn),
            arn: role_arn,
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    if let Some(document) = document {
        store
            .role_inline_policies
            .entry(iam::name_key(role_name))
            .or_default()
            .insert(
                iam::name_key("ds-access"),
                iam::InlinePolicy {
                    policy_name: "ds-access".to_string(),
                    policy_document: document.to_string(),
                },
            );
    }
}

/// A `CreateMicrosoftAD` body for `name`, with `edition` when one is named.
fn microsoft_ad(name: &str, edition: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": {
            "VpcId": "vpc-0a1b2c3d",
            "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
        }
    });
    if let Some(edition) = edition {
        body["Edition"] = json!(edition);
    }
    body
}

/// The VPC settings an `AddRegion` places the added Region's servers in.
fn vpc_settings() -> Value {
    json!({
        "VpcId": "vpc-11223344",
        "SubnetIds": ["subnet-11223344", "subnet-55667788"]
    })
}

/// An `AddRegion` body for `directory_id` and `region`.
fn add_region(directory_id: &str, region: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "RegionName": region,
        "VPCSettings": vpc_settings(),
    })
}

/// Create an Active Enterprise Managed Microsoft AD, the directory every case replicates.
async fn active_directory(emulator: &Emulator, name: &str, edition: Option<&str>) -> String {
    let answer = ok(
        emulator,
        "CreateMicrosoftAD",
        microsoft_ad(name, edition),
        name,
    )
    .await;
    let id = answer["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected a DirectoryId, got {answer}"))
        .to_string();
    // The stage transition the create owes falls due on its own.
    tokio::time::sleep(Duration::from_millis(900)).await;
    id
}

/// RULE: a request is authorized before the directory it names is resolved, so the
/// identity the credential speaks for is what decides whether the call is served.
#[tokio::test]
async fn the_identity_policies_of_the_caller_decide_whether_a_region_is_added() {
    let emulator = emulator();
    let directory = active_directory(&emulator, "gated.example.com", None).await;
    let other = active_directory(&emulator, "another.example.com", None).await;
    let directory_arn = format!(
        "arn:aws:ds:{HOME_REGION}:{account}:directory/{directory}",
        account = ::engine::emulator_account_id()
    );
    let other_arn = format!(
        "arn:aws:ds:{HOME_REGION}:{account}:directory/{other}",
        account = ::engine::emulator_account_id()
    );

    // (case, the inline policy on the caller's role, whether the call is authorized).
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no identity policy at all", None, false),
        (
            "an allow of everything",
            Some(r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#.to_string()),
            true,
        ),
        (
            "an allow of this whole service",
            Some(r#"{"Statement":[{"Effect":"Allow","Action":"ds:*","Resource":"*"}]}"#.to_string()),
            true,
        ),
        (
            "an allow of everything with an explicit deny of this action",
            Some(
                r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"},
                     {"Effect":"Deny","Action":"ds:AddRegion","Resource":"*"}]}"#
                    .to_string(),
            ),
            false,
        ),
        (
            "an allow of exactly this action on exactly this directory",
            Some(format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"ds:AddRegion","Resource":"{directory_arn}"}}]}}"#
            )),
            true,
        ),
        (
            "an allow of this action on another directory only",
            Some(format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"ds:AddRegion","Resource":"{other_arn}"}}]}}"#
            )),
            false,
        ),
        (
            "an allow of another service's actions",
            Some(r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#.to_string()),
            false,
        ),
        (
            "an allow of another ds action",
            Some(
                r#"{"Statement":[{"Effect":"Allow","Action":"ds:RemoveRegion","Resource":"*"}]}"#
                    .to_string(),
            ),
            false,
        ),
    ];

    for (index, (case, document, authorized)) in rows.iter().enumerate() {
        let access_key_id = format!("AKIDROLE{index:08}");
        let role_name = format!("ds-caller-{index}");
        caller(
            &emulator,
            &access_key_id,
            &role_name,
            document.as_deref(),
        );
        // Each row adds its own Region, so one row's success cannot refuse the next with
        // the already-replicated refusal.
        let region = format!("us-west-{}", index + 1);
        let (status, parsed) = call_as(
            &emulator,
            &access_key_id,
            HOME_REGION,
            "AddRegion",
            add_region(&directory, &region),
        )
        .await;
        if *authorized {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the authorized caller to be served, got {status} \
                 with {parsed}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the unauthorized caller to be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            ACCESS_DENIED,
            "case {case:?}: expected {ACCESS_DENIED}, got {parsed}"
        );
        // The refusal names who was refused, which action and which resource — every part
        // taken from the request and the stored policies.
        let message = parsed["Message"].as_str().unwrap_or_default();
        for part in [
            role_name.as_str(),
            "ds:AddRegion",
            directory_arn.as_str(),
        ] {
            assert!(
                message.contains(part),
                "case {case:?}: expected the refusal to name {part:?}, got {message:?}"
            );
        }
    }

    // The refused rows recorded nothing: only the Regions the authorized rows asked for
    // are replicated, which a read in each Region is what proves.
    for (index, (case, _, authorized)) in rows.iter().enumerate() {
        let region = format!("us-west-{}", index + 1);
        let (status, parsed) = call_as(
            &emulator,
            ACCOUNT_ACCESS_KEY_ID,
            &region,
            "RemoveRegion",
            json!({ "DirectoryId": &directory }),
        )
        .await;
        if *authorized {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the replication the authorized row added to be \
                 removable in {region}, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                error_code(&parsed),
                "DirectoryDoesNotExistException",
                "case {case:?}: expected no replication in {region} after the refusal, \
                 got {parsed}"
            );
        }
    }
}

/// RULE: `DescribeRegions` reports the Regions the store holds for a directory — its
/// primary Region and every replication `AddRegion` recorded — each with the status its
/// own transition reached, narrowed to the Region the request asked for.
#[tokio::test]
async fn the_region_inventory_is_reported_from_persisted_state() {
    let emulator = emulator();
    let directory = active_directory(&emulator, "inventory.example.com", None).await;

    // Before any add: the directory's own Region alone, reported as the primary one with
    // the stage the directory itself is in.
    let answer = ok(
        &emulator,
        "DescribeRegions",
        json!({ "DirectoryId": &directory }),
        "a directory replicated nowhere",
    )
    .await;
    let described = answer["RegionsDescription"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        described.len(),
        1,
        "case \"a directory replicated nowhere\": expected its primary Region alone, got \
         {described:?}"
    );
    assert_eq!(
        described[0]["RegionName"],
        json!(HOME_REGION),
        "case \"a directory replicated nowhere\": expected the Region it was created in, \
         got {described:?}"
    );
    assert_eq!(
        described[0]["RegionType"],
        json!("Primary"),
        "case \"a directory replicated nowhere\": expected the primary Region type, got \
         {described:?}"
    );
    assert_eq!(
        described[0]["Status"],
        json!("Active"),
        "case \"a directory replicated nowhere\": expected the directory's own stage, got \
         {described:?}"
    );

    // Two replications, each reported as an additional Region. A replication answers
    // while its Region is still being brought up, so a read straight after the add sees
    // the transient stage, and the move to Active falls due on its own.
    let added = ["eu-west-1", OTHER_REGION];
    for region in added {
        ok(
            &emulator,
            "AddRegion",
            add_region(&directory, region),
            region,
        )
        .await;
    }
    let answer = ok(
        &emulator,
        "DescribeRegions",
        json!({ "DirectoryId": &directory }),
        "straight after the adds",
    )
    .await;
    let described = answer["RegionsDescription"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        described.len(),
        3,
        "case \"straight after the adds\": expected the primary Region and the two \
         replications, got {described:?}"
    );
    for region in added {
        let entry = described
            .iter()
            .find(|entry| entry["RegionName"] == json!(region))
            .unwrap_or_else(|| {
                panic!("case {region:?}: expected the Region to be reported, got {described:?}")
            });
        assert_eq!(
            entry["RegionType"],
            json!("Additional"),
            "case {region:?}: expected the additional Region type, got {entry}"
        );
        assert_eq!(
            entry["Status"],
            json!("Requested"),
            "case {region:?}: expected the transient status a replication starts in, got \
             {entry}"
        );
        assert_eq!(
            entry["DirectoryId"],
            json!(&directory),
            "case {region:?}: expected the entry to name the directory, got {entry}"
        );
        // The placement the add named is reported back, and so is the controller count
        // this operation adds in the Region.
        assert_eq!(
            entry["VpcSettings"]["VpcId"], vpc_settings()["VpcId"],
            "case {region:?}: expected the requested VpcId, got {entry}"
        );
        assert_eq!(
            entry["VpcSettings"]["SubnetIds"], vpc_settings()["SubnetIds"],
            "case {region:?}: expected the requested SubnetIds, got {entry}"
        );
        assert_eq!(
            entry["DesiredNumberOfDomainControllers"],
            json!(2),
            "case {region:?}: expected the two controllers an add places, got {entry}"
        );
        assert!(
            entry["LaunchTime"].is_number(),
            "case {region:?}: expected a LaunchTime, got {entry}"
        );
    }

    // The transition each replication owes falls due with nobody writing.
    tokio::time::sleep(Duration::from_millis(900)).await;
    for region in added {
        let case = format!("the inventory of {region} once its transition fell due");
        let answer = ok(
            &emulator,
            "DescribeRegions",
            json!({ "DirectoryId": &directory, "RegionName": region }),
            &case,
        )
        .await;
        let described = answer["RegionsDescription"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: expected the one Region the filter named, got {described:?}"
        );
        assert_eq!(
            described[0]["Status"],
            json!("Active"),
            "case {case:?}: expected the replication to have settled, got {described:?}"
        );
    }

    // A Region the directory does not live in is an empty listing, not a refusal: the
    // member is a filter.
    let answer = ok(
        &emulator,
        "DescribeRegions",
        json!({ "DirectoryId": &directory, "RegionName": "ap-south-1" }),
        "a Region the directory does not live in",
    )
    .await;
    assert_eq!(
        answer["RegionsDescription"],
        json!([]),
        "case \"a Region the directory does not live in\": expected an empty listing, got \
         {answer}"
    );

    // Removing a replication takes it out of the inventory.
    let (status, parsed) = call_as(
        &emulator,
        ACCOUNT_ACCESS_KEY_ID,
        OTHER_REGION,
        "RemoveRegion",
        json!({ "DirectoryId": &directory }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"removing a replication\": expected 200, got {status} with {parsed}"
    );
    let answer = ok(
        &emulator,
        "DescribeRegions",
        json!({ "DirectoryId": &directory }),
        "after the removal",
    )
    .await;
    let names: Vec<String> = answer["RegionsDescription"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["RegionName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec![HOME_REGION.to_string(), "eu-west-1".to_string()],
        "case \"after the removal\": expected the removed Region to be gone, got {names:?}"
    );

    // An id no directory is stored under, and a token this operation never issued.
    for (case, body, expected) in [
        (
            "an id naming no directory",
            json!({ "DirectoryId": "d-0000000000" }),
            "DirectoryDoesNotExistException",
        ),
        (
            "a token this operation did not issue",
            json!({ "DirectoryId": &directory, "NextToken": "not-a-token" }),
            "InvalidNextTokenException",
        ),
    ] {
        let (status, parsed) =
            call_as(&emulator, ACCOUNT_ACCESS_KEY_ID, HOME_REGION, "DescribeRegions", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            expected,
            "case {case:?}: expected {expected}, got {parsed}"
        );
    }
}
#[tokio::test]
async fn every_operation_that_models_the_error_takes_the_same_decision() {
    let emulator = emulator();
    let directory = active_directory(&emulator, "gated-ops.example.com", None).await;

    // Every implemented operation of this service whose model declares
    // `AccessDeniedException`, with a request the model's own constraints accept — the
    // framework validates a request's SHAPE before any handler, and so before this gate,
    // runs.
    let operations: Vec<(&str, Value)> = vec![
        ("AddRegion", add_region(&directory, OTHER_REGION)),
        (
            "RemoveRegion",
            json!({ "DirectoryId": &directory }),
        ),
        (
            "DescribeRegions",
            json!({ "DirectoryId": &directory }),
        ),
        (
            "ShareDirectory",
            json!({
                "DirectoryId": &directory,
                "ShareMethod": "ORGANIZATIONS",
                "ShareTarget": { "Id": "111111111111", "Type": "ACCOUNT" },
            }),
        ),
        (
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": &directory }),
        ),
        (
            "DescribeDirectoryDataAccess",
            json!({ "DirectoryId": &directory }),
        ),
        (
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": &directory }),
        ),
    ];

    for (index, (operation, body)) in operations.iter().enumerate() {
        // One caller per operation, denied exactly that action beside an allow of
        // everything: the deny is what refuses, and the allow is what proves the refusal
        // is the deny's and not a missing grant.
        let denied_key = format!("AKIDDENY{index:08}");
        let denied_role = format!("ds-denied-{index}");
        caller(
            &emulator,
            &denied_key,
            &denied_role,
            Some(&format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"*","Resource":"*"}},
                     {{"Effect":"Deny","Action":"ds:{operation}","Resource":"*"}}]}}"#
            )),
        );
        let (status, parsed) =
            call_as(&emulator, &denied_key, HOME_REGION, operation, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected the denied caller to be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            ACCESS_DENIED,
            "case {operation:?}: expected {ACCESS_DENIED} for the denied caller, got {parsed}"
        );
        assert!(
            parsed["Message"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("ds:{operation}")),
            "case {operation:?}: expected the refusal to name the action, got {parsed}"
        );

        // The same request from a caller those policies allow is decided on its merits:
        // whatever it answers, it is not the authorization refusal.
        let allowed_key = format!("AKIDALLOW{index:07}");
        let allowed_role = format!("ds-allowed-{index}");
        caller(
            &emulator,
            &allowed_key,
            &allowed_role,
            Some(r#"{"Statement":[{"Effect":"Allow","Action":"ds:*","Resource":"*"}]}"#),
        );
        let (_, served) =
            call_as(&emulator, &allowed_key, HOME_REGION, operation, body.clone()).await;
        assert_ne!(
            error_code(&served),
            ACCESS_DENIED,
            "case {operation:?}: expected the allowed caller to reach the business logic, \
             got {served}"
        );
    }
}

/// RULE: an accepted `AddRegion` records the replication, and the Regions a directory
/// cannot be replicated to are refused with the modelled error for each case.
#[tokio::test]
async fn a_replication_is_recorded_and_the_refusals_follow_from_the_store() {
    let emulator = emulator();
    let enterprise = active_directory(&emulator, "enterprise.example.com", None).await;

    // A Region the directory is not in yet: accepted, and the replication is then
    // RECORDED — which a second add of the same Region is what proves, because it is
    // refused as a Region the directory is already in.
    ok(
        &emulator,
        "AddRegion",
        add_region(&enterprise, OTHER_REGION),
        "a Region the directory is not in yet",
    )
    .await;

    // (case, the body, the modelled error it is refused with).
    let mut rows: Vec<(String, Value, &str)> = vec![
        (
            "a Region the directory is already replicated to".to_string(),
            add_region(&enterprise, OTHER_REGION),
            "DirectoryAlreadyInRegionException",
        ),
        (
            "the directory's own primary Region".to_string(),
            add_region(&enterprise, HOME_REGION),
            "UnsupportedOperationException",
        ),
        (
            "an id no directory is stored under".to_string(),
            add_region("d-0000000000", OTHER_REGION),
            "DirectoryDoesNotExistException",
        ),
    ];
    // The settings that place the added Region's servers follow the same two-subnet rule
    // every create of this service follows.
    let mut one_subnet = add_region(&enterprise, "eu-west-1");
    one_subnet["VPCSettings"] =
        json!({ "VpcId": "vpc-11223344", "SubnetIds": ["subnet-11223344"] });
    rows.push((
        "VPC settings naming one subnet".to_string(),
        one_subnet,
        "InvalidParameterException",
    ));

    // A directory whose type or edition does not replicate across Regions.
    let standard = active_directory(&emulator, "standard.example.com", Some("Standard")).await;
    rows.push((
        "a Standard edition Managed Microsoft AD".to_string(),
        add_region(&standard, OTHER_REGION),
        "UnsupportedOperationException",
    ));
    let simple = ok(
        &emulator,
        "CreateDirectory",
        json!({ "Name": "simple.example.com", "Password": PASSWORD, "Size": "Small" }),
        "a Simple AD directory",
    )
    .await;
    let simple = simple["DirectoryId"].as_str().unwrap_or_default().to_string();
    tokio::time::sleep(Duration::from_millis(900)).await;
    rows.push((
        "a Simple AD directory".to_string(),
        add_region(&simple, OTHER_REGION),
        "UnsupportedOperationException",
    ));

    for (case, body, expected) in &rows {
        let (status, parsed) =
            call_as(&emulator, ACCOUNT_ACCESS_KEY_ID, HOME_REGION, "AddRegion", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {parsed}"
        );
        assert_eq!(
            &error_code(&parsed),
            expected,
            "case {case:?}: expected {expected}, got {parsed}"
        );
    }

    // The refusals changed nothing: the one replication the accepted call recorded is
    // still the only one, so it can be removed in exactly that Region — and once it is,
    // the same add is accepted again, because the Region the directory was in is gone.
    let (status, parsed) = call_as(
        &emulator,
        ACCOUNT_ACCESS_KEY_ID,
        OTHER_REGION,
        "RemoveRegion",
        json!({ "DirectoryId": &enterprise }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"removing the recorded replication\": expected 200, got {status} with {parsed}"
    );
    let (status, parsed) = call_as(
        &emulator,
        ACCOUNT_ACCESS_KEY_ID,
        OTHER_REGION,
        "RemoveRegion",
        json!({ "DirectoryId": &enterprise }),
    )
    .await;
    assert_eq!(
        error_code(&parsed),
        "DirectoryDoesNotExistException",
        "case \"a second removal in that Region\": expected the replication to be gone, \
         got {status} with {parsed}"
    );
    ok(
        &emulator,
        "AddRegion",
        add_region(&enterprise, OTHER_REGION),
        "the same Region once the replication was removed",
    )
    .await;
}
