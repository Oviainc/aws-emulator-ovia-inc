// SPDX-License-Identifier: CC0-1.0

//! The Snowball address and job lifecycle, driven end to end over HTTP.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: AWSIESnowballJobManagementService.<Operation>` and an `awsJson1_1`
//! body, dispatched by the multi-service router. Nothing reaches into the state struct,
//! so a response that is not computed from persisted state cannot pass.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (which optional members are present, the enum value from the
//! modelled catalog, the value's length, the page size, the token's form, the number of
//! jobs), and the address and job from the bootstrap issue are only rows among them.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
// The store ceilings this emulator imposes, read from the crate rather than restated, so
// the ceiling case cannot drift from the bound it exercises. Nothing in this API deletes
// an address or a job, so each create is refused once its store is full.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::JOB_STATE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    JobState, MAX_STORED_ADDRESSES, MAX_STORED_JOBS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: this service's shape name, per the crate's OPERATIONS.md
/// "Wire protocol" (`awsJson1_1`, `POST /`).
const TARGET_PREFIX: &str = "AWSIESnowballJobManagementService";

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// `MaxResults` on `ListJobs` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListJobs")`).
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// The `Address` members the model documents as "This field is no longer used and the
/// value is ignored" (`smithy_query(query="shape", shape="Address")`), so a submitted
/// value is neither stored nor reported.
const IGNORED_ADDRESS_MEMBERS: &[&str] = &["PrefectureOrDistrict", "Landmark"];

/// The values the modelled `AddressType` enum admits (`shared_types::ADDRESS_TYPE_VALUES`).
const ADDRESS_TYPES: &[&str] = &["AWS_SHIP", "CUST_PICKUP"];

/// The values the modelled `JobType` enum admits (`shared_types::JOB_TYPE_VALUES`).
const JOB_TYPES: &[&str] = &["EXPORT", "IMPORT", "LOCAL_USE"];

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the
    // SigV4 credential scope below.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDSNOWBALLTEST/20260101/{SIGNED_REGION}\
                 /snowball/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the modelled error `error_code`.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    error_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(error_code),
        "case {case:?}: expected a {error_code} from {operation}, got {parsed}"
    );
}

/// Assert `id` is `prefix` followed by a lowercase-hex `8-4-4-4-12` group — the shape
/// the modelled `AddressId` / `JobId` patterns admit.
fn assert_prefixed_uuid(id: &str, prefix: &str, case: &str) {
    let Some(body) = id.strip_prefix(prefix) else {
        panic!("case {case:?}: expected an id starting {prefix:?}, got {id:?}");
    };
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = body.split('-').collect();
    let well_formed = groups.len() == widths.len()
        && groups
            .iter()
            .zip(widths.iter())
            .all(|(group, width)| {
                group.len() == *width
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
    assert!(
        well_formed && id.len() == prefix.len() + 36,
        "case {case:?}: expected {prefix}<8-4-4-4-12 lowercase hex>, got {id:?} ({} chars)",
        id.len()
    );
}

/// A required string member of a response body.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// A boolean member, reading an absent one as `false`: both `Address$IsRestricted` and
/// `JobListEntry$IsMaster` are modelled with a `false` default, which the protocol may
/// leave off the wire.
fn boolean(body: &Value, member: &str) -> bool {
    body.get(member).and_then(Value::as_bool).unwrap_or(false)
}

/// Create an address and return the `AddressId` the service minted for it.
async fn create_address(router: &MultiServiceRouter, address: Value, case: &str) -> String {
    let created = ok(router, "CreateAddress", json!({ "Address": address }), case).await;
    let address_id = string(&created, "AddressId", case);
    assert_prefixed_uuid(&address_id, "ADID", case);
    address_id
}

/// Every address row: one per combination of present members, value length, `Type` from
/// the modelled catalog and `IsRestricted`. The first row is the one the bootstrap issue
/// recorded.
fn address_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "every documented member",
            json!({
                "Name": "Bootstrap Receiver",
                "Company": "Example Corp",
                "Street1": "123 Main Street",
                "Street2": "Suite 400",
                "Street3": "Mailstop 7",
                "City": "Seattle",
                "StateOrProvince": "WA",
                "Country": "US",
                "PostalCode": "98101",
                "PhoneNumber": "425-555-0100",
                "IsRestricted": false,
                "Type": ADDRESS_TYPES[0],
            }),
        ),
        (
            "only the members a carrier needs",
            json!({
                "Name": "Minimal Receiver",
                "Street1": "1 Short Road",
                "City": "Portland",
                "Country": "US",
                "PostalCode": "97201",
            }),
        ),
        (
            "restricted pickup address",
            json!({
                "Name": "Pickup Contact",
                "Street1": "500 Dock Way",
                "City": "Dublin",
                "Country": "IE",
                "PostalCode": "D02 AF30",
                "PhoneNumber": "+353 1 555 0199",
                "IsRestricted": true,
                "Type": ADDRESS_TYPES[1],
            }),
        ),
        (
            "long and non-ascii values",
            json!({
                "Name": "Núñez Ólafsdóttir",
                "Company": "ドコモ・システムズ株式会社",
                "Street1": "x".repeat(1024),
                "City": "São Paulo",
                "StateOrProvince": "SP",
                "Country": "BR",
                "PostalCode": "01310-100",
                "PhoneNumber": "+55 11 5555 0188",
            }),
        ),
        (
            "members the model documents as ignored",
            json!({
                "Name": "Ignored Member Receiver",
                "Street1": "9 Legacy Lane",
                "City": "Tokyo",
                "PrefectureOrDistrict": "Chiyoda",
                "Landmark": "Near the station",
                "Country": "JP",
                "PostalCode": "100-0001",
            }),
        ),
    ]
}

/// Assert the reported address is exactly what was submitted, under the minted id, with
/// the ignored members dropped and nothing invented.
fn assert_address_matches(reported: &Value, submitted: &Value, address_id: &str, case: &str) {
    assert_eq!(
        reported.get("AddressId").and_then(Value::as_str),
        Some(address_id),
        "case {case:?}: expected the reported AddressId to be the minted {address_id}, \
         got {reported}"
    );
    for ignored in IGNORED_ADDRESS_MEMBERS {
        let got = reported.get(*ignored).unwrap_or(&Value::Null);
        assert!(
            got.is_null(),
            "case {case:?}: the model documents {ignored} as ignored, so it must not be \
             reported, got {got}"
        );
    }
    let submitted = submitted
        .as_object()
        .expect("the submitted address is a JSON object");
    for (member, expected) in submitted {
        if IGNORED_ADDRESS_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        let got = reported.get(member).unwrap_or(&Value::Null);
        // A `false` `IsRestricted` is the member's modelled default, which the protocol
        // may leave off the wire.
        if got.is_null() && expected == &Value::Bool(false) {
            continue;
        }
        assert_eq!(
            got, expected,
            "case {case:?}: expected {member} to read back as {expected}, got {got}"
        );
    }
    for member in reported
        .as_object()
        .expect("the reported address is a JSON object")
        .keys()
    {
        // `IsRestricted` is modelled with a `false` default, so the protocol renders it
        // on an address that never set it.
        let is_default_bool =
            member == "IsRestricted" && reported[member] == Value::Bool(false);
        assert!(
            member == "AddressId" || is_default_bool || submitted.contains_key(member),
            "case {case:?}: the reported address carries {member}, which was never \
             submitted: {reported}"
        );
    }
}

/// Every state the lifecycle enum spells, so the set below is checked against the model
/// rather than against itself.
const ALL_JOB_STATES: &[JobState] = &[
    JobState::New,
    JobState::PreparingAppliance,
    JobState::PreparingShipment,
    JobState::InTransitToCustomer,
    JobState::WithCustomer,
    JobState::InTransitToAWS,
    JobState::WithAWSSortingFacility,
    JobState::WithAWS,
    JobState::InProgress,
    JobState::Complete,
    JobState::Cancelled,
    JobState::Listing,
    JobState::Pending,
];

#[test]
fn the_lifecycle_enum_spells_exactly_the_modelled_job_states() {
    let mut spelled: Vec<&str> = ALL_JOB_STATES.iter().map(|state| state.as_wire()).collect();
    spelled.sort_unstable();
    let mut modelled: Vec<&str> = JOB_STATE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        spelled, modelled,
        "the JobState enum must spell exactly the values the model admits"
    );
    // Only the state a job is created in is cancellable, per CancelJob's documentation.
    let cancellable: Vec<&str> = ALL_JOB_STATES
        .iter()
        .filter(|state| state.is_cancellable())
        .map(|state| state.as_wire())
        .collect();
    assert_eq!(
        cancellable,
        vec!["New"],
        "a job can only be cancelled before its JobState changes to PreparingAppliance"
    );
}

#[tokio::test]
async fn describe_address_reports_the_address_create_address_stored() {
    let router = router();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, submitted) in address_rows() {
        let address_id = create_address(&router, submitted.clone(), case).await;
        assert!(
            minted.insert(address_id.clone()),
            "case {case:?}: CreateAddress minted {address_id}, which it had minted before"
        );

        let described = ok(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            case,
        )
        .await;
        let reported = &described["Address"];
        assert_address_matches(reported, &submitted, &address_id, case);

        // A read must not change what a later caller sees.
        let described_again = ok(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            case,
        )
        .await;
        assert_eq!(
            described_again["Address"], *reported,
            "case {case:?}: two DescribeAddress calls reported different addresses"
        );
    }
}

#[tokio::test]
async fn every_create_address_mints_a_new_id() {
    let router = router();
    // One address submitted repeatedly: the id is the service's, so each call must mint
    // a fresh one rather than key on the submitted values.
    let submitted = json!({
        "Name": "Repeat Receiver",
        "Street1": "2 Same Street",
        "City": "Austin",
        "Country": "US",
        "PostalCode": "73301",
    });
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for attempt in 0..32u32 {
        let case = format!("identical address, attempt {attempt}");
        let address_id = create_address(&router, submitted.clone(), &case).await;
        assert!(
            minted.insert(address_id.clone()),
            "case {case:?}: CreateAddress reused the id {address_id}"
        );
    }
    assert_eq!(
        minted.len(),
        32,
        "expected 32 distinct address ids, got {}",
        minted.len()
    );
}

#[tokio::test]
async fn describe_address_refuses_an_id_no_address_answers_to() {
    let router = router();
    let known = create_address(
        &router,
        json!({ "Name": "Known", "Street1": "3 Known Way", "Country": "US" }),
        "a known address",
    )
    .await;

    // Each row is a well-formed `AddressId` — the modelled pattern is satisfied, so the
    // handler is reached — that names no stored address.
    let mut mutated = known.clone();
    mutated.pop();
    mutated.push(if known.ends_with('0') { '1' } else { '0' });
    let rows: Vec<(&str, String)> = vec![
        ("an all-zero id", "ADID00000000-0000-0000-0000-000000000000".to_string()),
        (
            "an id from another account",
            "ADIDdeadbeef-1234-5678-9abc-def012345678".to_string(),
        ),
        ("a known id with its last digit changed", mutated),
    ];
    for (case, address_id) in rows {
        assert_eq!(address_id.len(), 40, "case {case:?}: {address_id} is not 40 chars");
        refused(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            "InvalidResourceException",
            case,
        )
        .await;
    }
}

/// Every job row: one per `JobType` from the modelled catalog, differing in which
/// optional members it carries, its `SnowballType`, `ShippingOption`, capacity
/// preference and resource shape. The first row is the one the bootstrap issue recorded.
fn job_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "import job with s3 resources",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Description": "bootstrap job",
                "RoleARN": "arn:aws:iam::000000000000:role/snowball-import",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::bootstrap-bucket" }]
                },
            }),
        ),
        (
            "export job with a key range and notification",
            json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
                "SnowballCapacityPreference": "T98",
                "Description": "export the archive",
                "RoleARN": "arn:aws:iam::000000000000:role/snowball-export",
                "KmsKeyARN": "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "Resources": {
                    "S3Resources": [{
                        "BucketArn": "arn:aws:s3:::export-bucket",
                        "KeyRange": { "BeginMarker": "a/", "EndMarker": "m/" }
                    }]
                },
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:000000000000:snowball-events",
                    "JobStatesToNotify": ["New", "Complete"],
                    "NotifyAll": false
                },
            }),
        ),
        (
            "local-use job with on-device services and remote management",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "NEXT_DAY",
                "SnowballCapacityPreference": "T14",
                "RemoteManagement": "INSTALLED_AUTOSTART",
                "OnDeviceServiceConfiguration": {
                    "NFSOnDeviceService": { "StorageLimit": 10, "StorageUnit": "TB" }
                },
                "DeviceConfiguration": {
                    "SnowconeDeviceConfiguration": {
                        "WirelessConnection": { "IsWifiEnabled": true }
                    }
                },
            }),
        ),
        (
            "the smallest job the service accepts",
            json!({ "JobType": "IMPORT" }),
        ),
        (
            "job with tax documents, impact level and pickup details",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "RACK_5U_C",
                "ShippingOption": "STANDARD",
                "Description": "x".repeat(1024),
                "ImpactLevel": "IL4",
                "TaxDocuments": { "IND": { "GSTIN": "27AAPFU0939F1ZV" } },
                "PickupDetails": {
                    "Name": "Pickup Person",
                    "PhoneNumber": "+1 206 555 0144",
                    "Email": "pickup@example.com"
                },
            }),
        ),
    ]
}

/// Assert `metadata` reports exactly what `submitted` asked for, for a job in `state`.
fn assert_job_metadata_matches(metadata: &Value, submitted: &Value, state: &str, case: &str) {
    assert_eq!(
        metadata.get("JobState").and_then(Value::as_str),
        Some(state),
        "case {case:?}: expected JobState {state}, got {metadata}"
    );
    for (member, expected) in submitted
        .as_object()
        .expect("the submitted job is a JSON object")
    {
        match member.as_str() {
            // `CreateJob` takes a shipping speed; `JobMetadata` reports it inside
            // `ShippingDetails`, beside the shipments a moving device would have.
            "ShippingOption" => assert_eq!(
                metadata["ShippingDetails"].get("ShippingOption"),
                Some(expected),
                "case {case:?}: expected ShippingDetails.ShippingOption {expected}, \
                 got {metadata}"
            ),
            other => assert_eq!(
                metadata.get(other).unwrap_or(&Value::Null),
                expected,
                "case {case:?}: expected {other} to read back as {expected}, got {metadata}"
            ),
        }
    }
    // Nothing has shipped, transferred or been logged for a job nobody has prepared a
    // device for, so those members are absent rather than zeroed.
    for absent in [
        "DataTransferProgress",
        "JobLogInfo",
        "SnowballId",
        "ClusterId",
    ] {
        let got = metadata.get(absent).unwrap_or(&Value::Null);
        assert!(
            got.is_null(),
            "case {case:?}: expected no {absent} on a job in {state}, got {got}"
        );
    }
    if let Some(details) = metadata.get("ShippingDetails") {
        for absent in ["InboundShipment", "OutboundShipment"] {
            let got = details.get(absent).unwrap_or(&Value::Null);
            assert!(
                got.is_null(),
                "case {case:?}: expected no {absent} before a device moves, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn describe_job_reports_the_job_create_job_stored() {
    let router = router();
    let address_id = create_address(
        &router,
        json!({
            "Name": "Job Receiver",
            "Street1": "123 Main Street",
            "City": "Seattle",
            "StateOrProvince": "WA",
            "Country": "US",
            "PostalCode": "98101",
            "PhoneNumber": "425-555-0100",
        }),
        "the job's shipping address",
    )
    .await;

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, row) in job_rows() {
        let mut submitted = row;
        submitted["AddressId"] = json!(address_id);

        let created = ok(&router, "CreateJob", submitted.clone(), case).await;
        let job_id = string(&created, "JobId", case);
        assert_prefixed_uuid(&job_id, "JID", case);
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: CreateJob minted {job_id}, which it had minted before"
        );

        let described = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
        let metadata = &described["JobMetadata"];
        assert_eq!(
            metadata.get("JobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: expected JobMetadata.JobId {job_id}, got {metadata}"
        );
        assert_job_metadata_matches(metadata, &submitted, "New", case);
        let creation_date = metadata
            .get("CreationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationDate, got {metadata}"));
        // 2020-01-01T00:00:00Z: any instant this emulator records is later.
        assert!(
            creation_date > 1_577_836_800.0,
            "case {case:?}: expected a CreationDate after 2020, got {creation_date}"
        );

        // A job with no sub-jobs reports no `SubJobMetadata` member at all.
        let sub_jobs = described.get("SubJobMetadata").unwrap_or(&Value::Null);
        assert!(
            sub_jobs.is_null(),
            "case {case:?}: expected no SubJobMetadata, got {sub_jobs}"
        );

        // Reading does not change what the next caller sees.
        let described_again = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
        assert_eq!(
            described_again["JobMetadata"], *metadata,
            "case {case:?}: two DescribeJob calls reported different metadata"
        );
    }
}

#[tokio::test]
async fn create_job_refuses_a_resource_no_record_answers_to() {
    let router = router();
    let address_id = create_address(
        &router,
        json!({ "Name": "Known", "Street1": "3 Known Way", "Country": "US" }),
        "a known address",
    )
    .await;
    let unknown_address = "ADID00000000-0000-0000-0000-000000000000";

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an AddressId that was never created",
            json!({ "JobType": "IMPORT", "AddressId": unknown_address }),
            "InvalidResourceException",
        ),
        (
            "a ForwardingAddressId that was never created",
            json!({
                "JobType": "IMPORT",
                "AddressId": address_id,
                "ForwardingAddressId": unknown_address
            }),
            "InvalidResourceException",
        ),
        (
            "a ClusterId that was never created",
            json!({
                "JobType": "IMPORT",
                "ClusterId": "CID00000000-0000-0000-0000-000000000000"
            }),
            "InvalidResourceException",
        ),
        (
            "neither an address nor a cluster",
            json!({ "JobType": "IMPORT" }),
            "InvalidInputCombinationException",
        ),
    ];

    for (case, body, error_code) in rows {
        refused(&router, "CreateJob", body, error_code, case).await;
    }

    // A refused CreateJob stores nothing: the list is still empty.
    let listed = ok(&router, "ListJobs", json!({}), "after the refusals").await;
    let entries = listed["JobListEntries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        entries, 0,
        "expected a refused CreateJob to store no job, got {entries} in {listed}"
    );
}

#[tokio::test]
async fn describe_job_refuses_an_id_no_job_answers_to() {
    let router = router();
    for (case, job_id) in [
        ("an all-zero id", "JID00000000-0000-0000-0000-000000000000"),
        (
            "a main job's id form",
            "MID11111111-2222-3333-4444-555555555555",
        ),
    ] {
        assert_eq!(job_id.len(), 39, "case {case:?}: {job_id} is not 39 chars");
        refused(
            &router,
            "DescribeJob",
            json!({ "JobId": job_id }),
            "InvalidResourceException",
            case,
        )
        .await;
    }
}

/// An emulator holding `count` jobs, in creation order, on one address.
async fn router_with_jobs(count: usize) -> (MultiServiceRouter, Vec<String>) {
    let router = router();
    let address_id = create_address(
        &router,
        json!({ "Name": "List Receiver", "Street1": "7 List Way", "Country": "US" }),
        "the listed jobs' address",
    )
    .await;
    let mut job_ids = Vec::new();
    for index in 0..count {
        let case = format!("job {index} of {count}");
        let created = ok(
            &router,
            "CreateJob",
            json!({
                "JobType": JOB_TYPES[index % JOB_TYPES.len()],
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "AddressId": address_id,
                "Description": format!("job {index}"),
            }),
            &case,
        )
        .await;
        job_ids.push(string(&created, "JobId", &case));
    }
    (router, job_ids)
}

/// Walk `ListJobs` with `max_results` as the page size, returning the ids in the order
/// the pages reported them.
async fn page_through(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = ok(router, "ListJobs", body, case).await;
        let entries = listed["JobListEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected JobListEntries, got {listed}"))
            .clone();
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: ListJobs produced {pages} pages for {expected_total} jobs"
        );
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                entries.len() <= usize::try_from(max).expect("a positive page size"),
                "case {case:?}: expected at most {max} entries in a page, got {}",
                entries.len()
            );
            if next_token.is_some() {
                assert_eq!(
                    i64::try_from(entries.len()).expect("a small page"),
                    max,
                    "case {case:?}: a page with more jobs after it must be full"
                );
            }
        }
        for entry in entries {
            seen.push(string(&entry, "JobId", case));
        }
        if next_token.is_none() {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn list_jobs_pages_over_every_stored_job_in_creation_order() {
    let total = 5usize;
    let (router, created) = router_with_jobs(total).await;

    // Each row is a page size: the published bounds, sizes that divide the list and
    // sizes that do not, and a request that declares none.
    let rows: Vec<(&str, Option<i64>)> = vec![
        ("no MaxResults", None),
        ("MaxResults at the published minimum", Some(MAX_RESULTS_MIN)),
        ("MaxResults 1", Some(1)),
        ("MaxResults 2", Some(2)),
        ("MaxResults 5", Some(5)),
        ("MaxResults 7", Some(7)),
        ("MaxResults at the published maximum", Some(MAX_RESULTS_MAX)),
    ];
    for (case, max_results) in rows {
        let seen = page_through(&router, max_results, total, case).await;
        assert_eq!(
            seen, created,
            "case {case:?}: expected the pages to report every job once, in creation order"
        );
    }

    // One past the published maximum, and one below the published minimum, are outside
    // the modelled range and are refused before the handler runs.
    for (case, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        let (status, parsed) = call(&router, "ListJobs", json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults {max_results} to be refused, got {status} \
             with {parsed}"
        );
    }
}

#[tokio::test]
async fn list_jobs_entries_report_the_persisted_job() {
    let (router, created) = router_with_jobs(3).await;
    let listed = ok(&router, "ListJobs", json!({}), "every job").await;
    let entries = listed["JobListEntries"]
        .as_array()
        .expect("JobListEntries")
        .clone();
    assert_eq!(
        entries.len(),
        created.len(),
        "expected one entry per stored job, got {listed}"
    );

    for (index, entry) in entries.iter().enumerate() {
        let case = format!("entry {index}");
        let job_id = string(entry, "JobId", &case);
        assert_eq!(
            job_id, created[index],
            "case {case:?}: expected the entry for {}, got {job_id}",
            created[index]
        );
        let described = ok(&router, "DescribeJob", json!({ "JobId": job_id }), &case).await;
        let metadata = &described["JobMetadata"];
        // The entry is the same record the metadata is built from, so every member they
        // share carries the same value.
        for member in [
            "JobState",
            "JobType",
            "SnowballType",
            "CreationDate",
            "Description",
        ] {
            assert_eq!(
                entry.get(member),
                metadata.get(member),
                "case {case:?}: ListJobs and DescribeJob disagree on {member}: \
                 {entry} vs {metadata}"
            );
        }
        assert!(
            !boolean(entry, "IsMaster"),
            "case {case:?}: a job this service minted is not a main job, got {entry}"
        );
    }
}

#[tokio::test]
async fn list_jobs_refuses_a_token_it_did_not_mint() {
    let (router, _created) = router_with_jobs(3).await;
    let first_page = ok(
        &router,
        "ListJobs",
        json!({ "MaxResults": 1 }),
        "the first page",
    )
    .await;
    let token = string(&first_page, "NextToken", "the first page");

    // Each row is a token this service did not issue: not a cursor at all, a cursor for
    // a job the store does not hold, and a cursor that was altered in transit.
    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });
    let rows: Vec<(&str, String)> = vec![
        ("not a cursor", "garbage".to_string()),
        ("a cursor for a job that does not exist", {
            let mut hex_id = String::new();
            for byte in "JID00000000-0000-0000-0000-000000000000".bytes() {
                hex_id.push_str(&format!("{byte:02x}"));
            }
            hex_id
        }),
        ("a truncated cursor", token[..token.len() / 2].to_string()),
        ("an altered cursor", altered),
    ];
    for (case, next_token) in rows {
        refused(
            &router,
            "ListJobs",
            json!({ "NextToken": next_token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }

    // The token this service did mint still resumes the walk.
    let second_page = ok(
        &router,
        "ListJobs",
        json!({ "MaxResults": 1, "NextToken": token }),
        "resuming with the minted token",
    )
    .await;
    assert_eq!(
        second_page["JobListEntries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        1,
        "expected the minted token to resume the walk, got {second_page}"
    );
}

#[tokio::test]
async fn cancel_job_moves_the_job_to_cancelled_once() {
    let total = 3usize;
    let (router, created) = router_with_jobs(total).await;

    for (index, job_id) in created.iter().enumerate() {
        let case = format!("cancelling job {index} of {total}");
        ok(&router, "CancelJob", json!({ "JobId": job_id }), &case).await;

        // The new state is what a later reader observes, through both readers.
        let described = ok(&router, "DescribeJob", json!({ "JobId": job_id }), &case).await;
        assert_eq!(
            described["JobMetadata"].get("JobState").and_then(Value::as_str),
            Some("Cancelled"),
            "case {case:?}: expected DescribeJob to report Cancelled, got {described}"
        );
        let listed = ok(&router, "ListJobs", json!({}), &case).await;
        let entries = listed["JobListEntries"].as_array().expect("JobListEntries");
        for (position, entry) in entries.iter().enumerate() {
            let expected_state = if position <= index { "Cancelled" } else { "New" };
            assert_eq!(
                entry.get("JobState").and_then(Value::as_str),
                Some(expected_state),
                "case {case:?}: expected job {position} to be {expected_state}, got {entry}"
            );
        }

        // A job that is no longer cancellable is refused, and stays as it was.
        refused(
            &router,
            "CancelJob",
            json!({ "JobId": job_id }),
            "InvalidJobStateException",
            &case,
        )
        .await;
        let described_again = ok(&router, "DescribeJob", json!({ "JobId": job_id }), &case).await;
        assert_eq!(
            described_again["JobMetadata"], described["JobMetadata"],
            "case {case:?}: a refused CancelJob changed the job"
        );
    }
}

#[tokio::test]
async fn cancel_job_refuses_an_id_no_job_answers_to() {
    let (router, _created) = router_with_jobs(1).await;
    refused(
        &router,
        "CancelJob",
        json!({ "JobId": "JID00000000-0000-0000-0000-000000000000" }),
        "InvalidResourceException",
        "an id no job answers to",
    )
    .await;
}


#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    let router = router();
    let address = json!({ "Name": "Bounded", "Street1": "1 Bound Way", "Country": "US" });

    // Each row is one bounded store: fill it to its ceiling, then assert the request one
    // past the ceiling is refused rather than accepted, and that what is already stored
    // is still readable.
    let mut address_id = String::new();
    for index in 0..MAX_STORED_ADDRESSES {
        let case = format!("address {index} of {MAX_STORED_ADDRESSES}");
        address_id = create_address(&router, address.clone(), &case).await;
    }
    let (status, parsed) = call(
        &router,
        "CreateAddress",
        json!({ "Address": address.clone() }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one address past the ceiling\": expected a refusal, got {status} with {parsed}"
    );
    ok(
        &router,
        "DescribeAddress",
        json!({ "AddressId": address_id }),
        "the last stored address after the refusal",
    )
    .await;

    let job = json!({
        "JobType": "IMPORT",
        "SnowballType": "EDGE",
        "ShippingOption": "SECOND_DAY",
        "AddressId": address_id,
    });
    let mut job_id = String::new();
    for index in 0..MAX_STORED_JOBS {
        let case = format!("job {index} of {MAX_STORED_JOBS}");
        let created = ok(&router, "CreateJob", job.clone(), &case).await;
        job_id = string(&created, "JobId", &case);
    }
    let (status, parsed) = call(&router, "CreateJob", job.clone()).await;
    assert_eq!(
        status, 400,
        "case \"one job past the ceiling\": expected a refusal, got {status} with {parsed}"
    );
    ok(
        &router,
        "DescribeJob",
        json!({ "JobId": job_id }),
        "the last stored job after the refusal",
    )
    .await;

    // Cancelling does not free a slot: the job keeps its record, so the create is still
    // refused.
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": job_id }),
        "cancelling the last stored job",
    )
    .await;
    let (status, parsed) = call(&router, "CreateJob", job).await;
    assert_eq!(
        status, 400,
        "case \"one job past the ceiling after a cancel\": expected a refusal, got {status} \
         with {parsed}"
    );
}
