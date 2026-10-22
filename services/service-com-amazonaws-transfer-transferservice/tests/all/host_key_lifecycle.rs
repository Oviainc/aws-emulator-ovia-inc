// SPDX-License-Identifier: CC0-1.0

//! A server's HOST KEYS, over the whole wire path.
//!
//! Six rules about the stored host-key record, each asserted over a TABLE whose rows
//! differ along what the rule generalises over:
//!
//! 1. `ImportHostKey` stores a key under a minted `HostKeyId` and resolves its
//!    properties ONCE, from the supplied material and the request, so `DescribeHostKey`
//!    reports that record: the key's genuine OpenSSH fingerprint and algorithm `Type`,
//!    its ARN, the `Description` and `Tags` the import carried, the resolved
//!    `StorageType`, and the import instant at the clock's own precision.
//! 2. The MATERIAL is a key's identity within a server, so re-importing material the
//!    server already holds is refused with `ResourceExistsException` (409) whatever
//!    else the request carries — while the same material on another server, or after
//!    the key is deleted, is a new key.
//! 3. A body the emulator cannot read as a private key is refused with
//!    `InvalidRequestException` (400) and mints no id.
//! 4. A key is addressed by `(ServerId, HostKeyId)`: an id no import minted, one minted
//!    on another server, or one already deleted, answers the 400
//!    `ResourceNotFoundException`, and `DeleteHostKey` removes exactly the key it names.
//! 5. A key operation leaves the server's `State` alone: importing or deleting a host
//!    key neither takes a serving server down nor starts a stopped one.
//! 6. A server's key set is bounded by `state::MAX_HOST_KEYS_PER_SERVER`: every import
//!    up to the bound is stored, the one past it is refused, and a delete frees exactly
//!    one slot.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::harness::{
    assert_members, generated_host_key, host_key_algorithm, is_modelled_host_key_id, member_of,
    Emulator, ACCOUNT, PAST_START_SPAN, REGION, SSH_KEY,
};

/// `SERVER_STOP_SPAN` in `src/state.rs` is 500 ms; wait past it. Kept here, next to the
/// only host-key test that stops a server, rather than in the shared harness.
const PAST_STOP_SPAN: Duration = Duration::from_millis(900);

/// How far outside the window measured around a call a reported timestamp may fall.
///
/// The window is read from the same clock the handler stamps the record with; this
/// covers only the rounding of the epoch-seconds `double` the protocol puts a timestamp
/// on the wire as, NOT a coarser clock: a value truncated to whole seconds is seconds
/// away from the window, not a millisecond.
const CLOCK_SLACK_SECONDS: f64 = 0.001;

/// `HostKeyStorageType`'s two values, from the model's enum (`smithy_query(query="shape",
/// shape="HostKeyStorageType")` -> `[SERVICE_MANAGED, SECRETS_MANAGER]`).
const STORAGE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
const STORAGE_SECRETS_MANAGER: &str = "SECRETS_MANAGER";

/// Syntactically valid `HostKeyId`s (`len 25..25`, `^hostkey-[0-9a-f]{17}$`, per the
/// `HostKeyId` shape) that no import ever minted: the low end of the id space, a mid
/// value, and the high end.
const NEVER_MINTED_KEYS: [&str; 3] = [
    "hostkey-00000000000000000",
    "hostkey-0000000000000dead",
    "hostkey-fffffffffffffffff",
];

/// A syntactically valid `ServerId` (`^s-([0-9a-f]{17})$`) that no `CreateServer`
/// minted.
const NEVER_MINTED_SERVER: &str = "s-0000000000000beef";

/// The members an `ImportHostKey` request carries beyond the server and the key
/// material.
struct ImportCase {
    name: &'static str,
    description: Option<String>,
    storage_type: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The import table: the rows differ along which optional members the request carries,
/// the SIZE of `Description` (absent, the empty string its `len 0..200` admits, a
/// sentence, the 200-character maximum), the characters in it (`^[\p{Print}]*$` admits
/// punctuation and digits), which `StorageType` it selects, and how many tags it
/// carries. Each row imports freshly generated key material, so the rows also walk a
/// server's key count up from one.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            name: "no optional member at all",
            description: None,
            storage_type: None,
            tags: Vec::new(),
        },
        ImportCase {
            name: "the empty description its modelled length admits",
            description: Some(String::new()),
            storage_type: None,
            tags: Vec::new(),
        },
        ImportCase {
            name: "a description, service-managed storage and one tag",
            description: Some("nightly rotation key".to_string()),
            storage_type: Some(STORAGE_SERVICE_MANAGED),
            tags: vec![("env", "prod")],
        },
        ImportCase {
            name: "a description at the modelled maximum length and two tags",
            description: Some("d".repeat(200)),
            storage_type: None,
            tags: vec![("env", "staging"), ("team", "platform")],
        },
        ImportCase {
            name: "secrets-manager storage",
            description: Some("stored in secrets manager".to_string()),
            storage_type: Some(STORAGE_SECRETS_MANAGER),
            tags: Vec::new(),
        },
        ImportCase {
            name: "punctuation and digits in the description",
            description: Some("key #1 (primary) 50% ascii".to_string()),
            storage_type: Some(STORAGE_SERVICE_MANAGED),
            tags: vec![("owner", "team-a")],
        },
    ]
}

/// The `Tags` list as the wire carries it.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// Seconds since the Unix epoch, at the clock's own precision — the unit the awsJson
/// protocol puts a timestamp on the wire in.
fn unix_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is past the Unix epoch")
        .as_secs_f64()
}

/// One `ImportHostKey` request built from a case.
fn import_request(server_id: &str, material: &str, case: &ImportCase) -> Value {
    let mut request = serde_json::Map::new();
    request.insert("ServerId".to_string(), json!(server_id));
    request.insert("HostKeyBody".to_string(), json!(material));
    if let Some(description) = &case.description {
        request.insert("Description".to_string(), json!(description));
    }
    if let Some(storage_type) = case.storage_type {
        request.insert("StorageType".to_string(), json!(storage_type));
    }
    // `Tags` is modelled `len 1..50`, so a row with no tags sends no member.
    if !case.tags.is_empty() {
        request.insert("Tags".to_string(), tag_list(&case.tags));
    }
    Value::Object(request)
}

/// Import a host key, assert the import succeeded and hand back its `HostKeyId`.
async fn import(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("ImportHostKey", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ImportHostKey 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("HostKeyId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a HostKeyId, got {}", answer.body))
        .to_string()
}

/// The `DescribedHostKey` the reader reports for a stored key.
async fn described(emulator: &Emulator, case: &str, server_id: &str, host_key_id: &str) -> Value {
    let answer = emulator
        .call(
            "DescribeHostKey",
            json!({"ServerId": server_id, "HostKeyId": host_key_id}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeHostKey 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("HostKey").clone()
}

/// Delete a host key and assert the delete succeeded.
async fn delete(emulator: &Emulator, case: &str, server_id: &str, host_key_id: &str) {
    let answer = emulator
        .call(
            "DeleteHostKey",
            json!({"ServerId": server_id, "HostKeyId": host_key_id}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteHostKey 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// Assert one call answers the 400 `ResourceNotFoundException` for `resource_type` /
/// `resource`.
async fn assert_not_found(
    emulator: &Emulator,
    case: &str,
    op: &str,
    request: Value,
    resource_type: &str,
    resource: &str,
) {
    let answer = emulator.call(op, request).await;
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: expected 400 ResourceNotFoundException, got {} with body {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "ResourceType").as_str(),
        Some(resource_type),
        "case {case:?}: expected ResourceType {resource_type:?}, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Resource").as_str(),
        Some(resource),
        "case {case:?}: expected the requested identifier {resource:?} in Resource, got {}",
        answer.body
    );
}

/// The server's `State`, as `DescribeServer` reports it.
async fn server_state(emulator: &Emulator, case: &str, server_id: &str) -> String {
    let answer = emulator
        .call("DescribeServer", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeServer 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(answer.get("Server"), "State")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a State, got {}", answer.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the record the import resolves is the record the reader reports.
// ---------------------------------------------------------------------------

/// RULE: an import stores one host key under a minted `HostKeyId`, with its fingerprint
/// and algorithm `Type` read out of the supplied material, its ARN naming the server
/// that holds it, the description and tags the request carried, the `StorageType` the
/// request selected (`SERVICE_MANAGED` when it selected none) and the instant of the
/// call — and `DescribeHostKey` reports exactly that, twice over.
#[tokio::test]
async fn an_imported_host_key_is_the_record_describe_host_key_reports() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({"Domain": "EFS"})).await;

    let mut minted: Vec<String> = Vec::new();
    let mut stamps: Vec<f64> = Vec::new();

    for case in import_cases() {
        let name = case.name;
        // Fresh material per row: the fingerprint and the algorithm are computed from
        // the key itself by the OpenSSH key-format crate, so the expectation comes from
        // the material rather than from the handler under test.
        let (material, fingerprint) = generated_host_key();
        let algorithm = host_key_algorithm(&material);

        let before = unix_seconds(SystemTime::now());
        let answer = emulator
            .call(
                "ImportHostKey",
                import_request(&server_id, &material, &case),
            )
            .await;
        let after = unix_seconds(SystemTime::now());

        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected ImportHostKey 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("ServerId").as_str(),
            Some(server_id.as_str()),
            "case {name:?}: expected the request's ServerId back, got {}",
            answer.body
        );
        let host_key_id = answer
            .get("HostKeyId")
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: expected a HostKeyId, got {}", answer.body))
            .to_string();
        assert!(
            is_modelled_host_key_id(&host_key_id),
            "case {name:?}: expected a modelled HostKeyId (hostkey- + 17 hex digits), \
             got {host_key_id:?}"
        );
        assert!(
            !minted.contains(&host_key_id),
            "case {name:?}: expected an id distinct from the ones already minted \
             ({minted:?}), got {host_key_id:?}"
        );
        minted.push(host_key_id.clone());

        let record = described(&emulator, name, &server_id, &host_key_id).await;
        assert_members(
            name,
            &record,
            &json!({
                "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:host-key/{server_id}/{host_key_id}"),
                "HostKeyId": &host_key_id,
                "HostKeyFingerprint": &fingerprint,
                "Type": &algorithm,
                "Description": case.description.clone().map_or(Value::Null, Value::from),
                "StorageType": case.storage_type.unwrap_or(STORAGE_SERVICE_MANAGED),
                "Tags": tag_list(&case.tags),
                // The emulator stores no Secrets Manager secret, so it reports no
                // SecretId rather than an ARN naming a secret nothing holds.
                "SecretId": Value::Null,
            }),
        );

        let stamp = member_of(&record, "DateImported")
            .as_f64()
            .unwrap_or_else(|| panic!("case {name:?}: expected a DateImported, got {record}"));
        assert!(
            stamp >= before - CLOCK_SLACK_SECONDS && stamp <= after + CLOCK_SLACK_SECONDS,
            "case {name:?}: expected DateImported inside the window of the call \
             [{before}, {after}], got {stamp}"
        );
        stamps.push(stamp);

        // A read changes nothing: the second read answers the same record.
        let again = described(&emulator, name, &server_id, &host_key_id).await;
        assert_eq!(
            again, record,
            "case {name:?}: expected a second read to report the same record {record}, \
             got {again}"
        );
    }

    // The import instant keeps the precision the clock has: a value truncated to whole
    // seconds would leave every fraction at zero.
    assert!(
        stamps.iter().any(|stamp| stamp.fract() != 0.0),
        "expected DateImported to keep the sub-second precision of the clock, got {stamps:?}"
    );
    assert!(
        stamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "expected DateImported to be non-decreasing across successive imports, got {stamps:?}"
    );

    // The ARN names the server that HOLDS the key, so the same rule on another server
    // reports that server's resource path.
    let (material, fingerprint) = generated_host_key();
    let elsewhere = import(
        &emulator,
        "a key on a second server",
        json!({"ServerId": &other_server, "HostKeyBody": &material}),
    )
    .await;
    let record = described(&emulator, "a key on a second server", &other_server, &elsewhere).await;
    assert_members(
        "a key on a second server",
        &record,
        &json!({
            "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:host-key/{other_server}/{elsewhere}"),
            "HostKeyFingerprint": &fingerprint,
        }),
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the material is the key's identity within a server.
// ---------------------------------------------------------------------------

/// RULE: material a server already holds is one key, so a repeat import is refused with
/// `ResourceExistsException` (409) naming the key already held — whatever description,
/// tags or storage type the repeat carries — while the same material on another server,
/// or after the held key is deleted, is a new key.
#[tokio::test]
async fn material_a_server_already_holds_is_refused_whatever_else_the_request_carries() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({})).await;
    let (material, fingerprint) = generated_host_key();

    let held = import(
        &emulator,
        "the first import",
        json!({"ServerId": &server_id, "HostKeyBody": &material, "Description": "primary"}),
    )
    .await;

    // The rows differ along what ELSE the repeat carries: none of it makes the material
    // a different key.
    let repeats: [(&str, Value); 4] = [
        (
            "the identical request",
            json!({"ServerId": &server_id, "HostKeyBody": &material, "Description": "primary"}),
        ),
        (
            "a different description",
            json!({"ServerId": &server_id, "HostKeyBody": &material, "Description": "a copy"}),
        ),
        (
            "no description at all",
            json!({"ServerId": &server_id, "HostKeyBody": &material}),
        ),
        (
            "tags and the other storage type",
            json!({
                "ServerId": &server_id,
                "HostKeyBody": &material,
                "StorageType": STORAGE_SECRETS_MANAGER,
                "Tags": [{"Key": "env", "Value": "prod"}],
            }),
        ),
    ];
    for (case, request) in repeats {
        let answer = emulator.call("ImportHostKey", request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (409, "ResourceExistsException"),
            "case {case:?}: expected 409 ResourceExistsException, got {} with body {}",
            answer.status,
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "Resource").as_str(),
            Some(held.as_str()),
            "case {case:?}: expected the held key's id in Resource, got {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "ResourceType").as_str(),
            Some("HostKey"),
            "case {case:?}: expected ResourceType \"HostKey\", got {}",
            answer.body
        );
        assert!(
            !member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: Message is modelled required, got {}",
            answer.body
        );
        assert!(
            member_of(&answer.body, "HostKeyId").is_null(),
            "case {case:?}: expected a refusal to mint no id, got {}",
            answer.body
        );
    }

    // The refusals changed nothing: the first import's record still stands.
    let record = described(&emulator, "after the refused repeats", &server_id, &held).await;
    assert_members(
        "after the refused repeats",
        &record,
        &json!({
            "HostKeyFingerprint": &fingerprint,
            "Description": "primary",
            "StorageType": STORAGE_SERVICE_MANAGED,
            "Tags": [],
        }),
    );

    // The same material on ANOTHER server is that server's own key: identity is per
    // server, because a host key belongs to the server it was imported on.
    let elsewhere = import(
        &emulator,
        "the same material on another server",
        json!({"ServerId": &other_server, "HostKeyBody": &material}),
    )
    .await;
    assert_ne!(
        elsewhere, held,
        "case \"the same material on another server\": expected a key of its own, \
         got the id {held:?} the first server holds"
    );

    // Deleting the held key releases the material, so importing it again is a NEW key.
    delete(&emulator, "releasing the material", &server_id, &held).await;
    let reimported = import(
        &emulator,
        "the same material after deleting the key",
        json!({"ServerId": &server_id, "HostKeyBody": &material}),
    )
    .await;
    assert_ne!(
        reimported, held,
        "case \"the same material after deleting the key\": expected a newly minted id, \
         got the deleted key's id {held:?}"
    );
    let record = described(
        &emulator,
        "the same material after deleting the key",
        &server_id,
        &reimported,
    )
    .await;
    assert_members(
        "the same material after deleting the key",
        &record,
        &json!({"HostKeyFingerprint": &fingerprint, "Description": Value::Null}),
    );
}

// ---------------------------------------------------------------------------
// Rule 3: material the emulator cannot read is refused, not stored.
// ---------------------------------------------------------------------------

/// RULE: a `HostKeyBody` the emulator cannot read as a private key has no fingerprint
/// and no algorithm to report, so the import is refused with `InvalidRequestException`
/// (400) and mints nothing — the server is left exactly as it was.
///
/// The rows differ along the FORM of the unreadable body: empty (the shortest
/// `HostKeyBody`'s `len 0..4096` admits), prose, a PUBLIC key instead of a private one,
/// a PEM envelope with no key in it, and a truncated OpenSSH private key.
#[tokio::test]
async fn a_body_the_emulator_cannot_read_as_a_private_key_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let (good_material, good_fingerprint) = generated_host_key();
    let truncated: String = good_material.chars().take(40).collect();

    let unreadable: [(&str, String); 5] = [
        ("an empty body", String::new()),
        ("prose", "this is not a private key".to_string()),
        ("an SSH public key", SSH_KEY.to_string()),
        (
            "a PEM envelope with no key in it",
            "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n".to_string(),
        ),
        ("a truncated OpenSSH private key", truncated),
    ];
    for (case, body) in unreadable {
        let answer = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": body}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            answer.status,
            answer.body
        );
        assert!(
            member_of(&answer.body, "HostKeyId").is_null(),
            "case {case:?}: expected a refusal to mint no id, got {}",
            answer.body
        );
    }

    // The server is untouched: readable material still imports, and it is the only key
    // the server holds.
    let host_key_id = import(
        &emulator,
        "readable material after the refusals",
        json!({"ServerId": &server_id, "HostKeyBody": &good_material}),
    )
    .await;
    let record = described(
        &emulator,
        "readable material after the refusals",
        &server_id,
        &host_key_id,
    )
    .await;
    assert_members(
        "readable material after the refusals",
        &record,
        &json!({"HostKeyFingerprint": &good_fingerprint}),
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a key is addressed by (ServerId, HostKeyId).
// ---------------------------------------------------------------------------

/// RULE: every host-key operation resolves the SERVER first and then the key on that
/// server, so an unminted server id is a missing `Server`, and a key id the resolved
/// server does not hold — never minted, minted on another server, or already deleted —
/// is a missing `HostKey`; a delete removes exactly the key it names.
#[tokio::test]
async fn a_host_key_the_server_does_not_hold_is_a_not_found() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({})).await;

    let (material, _) = generated_host_key();

    // Every operation that resolves a `ServerId`, against an id no create minted.
    for (op, request) in [
        (
            "ImportHostKey",
            json!({"ServerId": NEVER_MINTED_SERVER, "HostKeyBody": &material}),
        ),
        (
            "DescribeHostKey",
            json!({"ServerId": NEVER_MINTED_SERVER, "HostKeyId": NEVER_MINTED_KEYS[0]}),
        ),
        (
            "DeleteHostKey",
            json!({"ServerId": NEVER_MINTED_SERVER, "HostKeyId": NEVER_MINTED_KEYS[0]}),
        ),
    ] {
        let case = format!("{op} on the unminted server {NEVER_MINTED_SERVER}");
        assert_not_found(
            &emulator,
            &case,
            op,
            request,
            "Server",
            NEVER_MINTED_SERVER,
        )
        .await;
    }

    // Three keys on one server: the rows below address one of them on the WRONG server,
    // ids that were never minted, and the one that is deleted.
    let mut ids: Vec<String> = Vec::new();
    for index in 0..3u32 {
        let (material, _) = generated_host_key();
        let case = format!("key {index} of the addressing table");
        ids.push(
            import(
                &emulator,
                &case,
                json!({"ServerId": &server_id, "HostKeyBody": material}),
            )
            .await,
        );
    }

    for absent in NEVER_MINTED_KEYS {
        assert!(
            is_modelled_host_key_id(absent),
            "the probe id {absent:?} must satisfy HostKeyId's modelled pattern"
        );
        for op in ["DescribeHostKey", "DeleteHostKey"] {
            let case = format!("{op} for the never-minted key {absent}");
            assert_not_found(
                &emulator,
                &case,
                op,
                json!({"ServerId": &server_id, "HostKeyId": absent}),
                "HostKey",
                absent,
            )
            .await;
        }
    }

    // A key of THIS server addressed on another one is not that server's key.
    for op in ["DescribeHostKey", "DeleteHostKey"] {
        let case = format!("{op} for a key of another server");
        assert_not_found(
            &emulator,
            &case,
            op,
            json!({"ServerId": &other_server, "HostKeyId": &ids[0]}),
            "HostKey",
            &ids[0],
        )
        .await;
    }

    // The delete removes EXACTLY the key it names: the middle one goes, the other two
    // stay readable, and the deleted id is missing in the same way a never-minted one is
    // — including on the second delete a teardown retries.
    delete(&emulator, "deleting the middle key", &server_id, &ids[1]).await;
    for op in ["DescribeHostKey", "DeleteHostKey"] {
        let case = format!("{op} after deleting the middle key");
        assert_not_found(
            &emulator,
            &case,
            op,
            json!({"ServerId": &server_id, "HostKeyId": &ids[1]}),
            "HostKey",
            &ids[1],
        )
        .await;
    }
    for kept in [&ids[0], &ids[2]] {
        let record = described(&emulator, "a key the delete did not name", &server_id, kept).await;
        assert_members(
            "a key the delete did not name",
            &record,
            &json!({"HostKeyId": kept}),
        );
    }

    // A deleted SERVER takes its keys with it, so the server lookup fails first.
    let doomed = emulator.create_server(json!({})).await;
    let (material, _) = generated_host_key();
    let on_doomed = import(
        &emulator,
        "a key on a server about to be deleted",
        json!({"ServerId": &doomed, "HostKeyBody": material}),
    )
    .await;
    let deleted = emulator
        .call("DeleteServer", json!({"ServerId": &doomed}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteServer 200, got {} with body {}",
        deleted.status, deleted.body
    );
    assert_not_found(
        &emulator,
        "DescribeHostKey after the server was deleted",
        "DescribeHostKey",
        json!({"ServerId": &doomed, "HostKeyId": &on_doomed}),
        "Server",
        &doomed,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Rule 5: a key operation leaves the server's State alone.
// ---------------------------------------------------------------------------

/// RULE: importing or deleting a host key changes no server `State` and begins no
/// transition, so the server a caller was using keeps the state it had — there is no
/// downtime window around a key operation, and a stopped server is not started by one.
///
/// The rows are the two states a settled server can be in (`ONLINE`, `OFFLINE`), each
/// observed before and after an import and a delete.
#[tokio::test]
async fn a_host_key_operation_leaves_the_servers_state_alone() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    tokio::time::sleep(PAST_START_SPAN).await;

    for phase in ["ONLINE", "OFFLINE"] {
        if phase == "OFFLINE" {
            let stopped = emulator
                .call("StopServer", json!({"ServerId": &server_id}))
                .await;
            assert_eq!(
                stopped.status, 200,
                "expected StopServer 200, got {} with body {}",
                stopped.status, stopped.body
            );
            tokio::time::sleep(PAST_STOP_SPAN).await;
        }

        let before = server_state(&emulator, phase, &server_id).await;
        assert_eq!(
            before, phase,
            "case {phase:?}: expected the server to be {phase} before the key operations, \
             got {before:?}"
        );

        let (material, fingerprint) = generated_host_key();
        let host_key_id = import(
            &emulator,
            phase,
            json!({"ServerId": &server_id, "HostKeyBody": material}),
        )
        .await;
        let after_import = server_state(&emulator, phase, &server_id).await;
        assert_eq!(
            after_import, phase,
            "case {phase:?}: expected ImportHostKey to leave the server {phase}, \
             got {after_import:?}"
        );

        // And the key is readable in that state: a key operation is not gated on the
        // server serving.
        let record = described(&emulator, phase, &server_id, &host_key_id).await;
        assert_members(phase, &record, &json!({"HostKeyFingerprint": &fingerprint}));

        delete(&emulator, phase, &server_id, &host_key_id).await;
        let after_delete = server_state(&emulator, phase, &server_id).await;
        assert_eq!(
            after_delete, phase,
            "case {phase:?}: expected DeleteHostKey to leave the server {phase}, \
             got {after_delete:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: a server's key set is bounded.
// ---------------------------------------------------------------------------

/// RULE: a server holds at most `state::MAX_HOST_KEYS_PER_SERVER` host keys, so every
/// import up to the bound is stored, the one past it is refused with
/// `InvalidRequestException` (400) without touching the stored set, and a delete frees
/// exactly one slot.
///
/// The rows are the counts around the bound — up to it, AT it, one past it, and then the
/// refused import accepted after a delete. The bound comes from the constant, so the
/// test follows the store's bound rather than restating a number.
#[tokio::test]
async fn a_server_holds_at_most_the_bounded_number_of_host_keys() {
    const BOUND: usize =
        service_com_amazonaws_transfer_transferservice::state::MAX_HOST_KEYS_PER_SERVER;

    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let keys: Vec<(String, String)> = (0..=BOUND).map(|_| generated_host_key()).collect();
    let mut stored: Vec<String> = Vec::new();

    for (index, (material, _)) in keys.iter().enumerate() {
        let answer = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": material}),
            )
            .await;
        let case = format!("key {} of a {BOUND}-key bound", index + 1);
        if index < BOUND {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the import to be accepted, got {} with body {}",
                answer.status, answer.body
            );
            stored.push(
                answer
                    .get("HostKeyId")
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a HostKeyId, got {}", answer.body)
                    })
                    .to_string(),
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: expected the import past the bound to be refused 400 \
                 InvalidRequestException, got {} with body {}",
                answer.status,
                answer.body
            );
        }
    }

    // Everything stored up to the bound is still readable: the refusal touched nothing.
    for host_key_id in &stored {
        let record = described(&emulator, "at the bound", &server_id, host_key_id).await;
        assert_members("at the bound", &record, &json!({"HostKeyId": host_key_id}));
    }

    // A delete frees exactly one slot, so the import refused above is accepted now.
    delete(&emulator, "freeing one slot", &server_id, &stored[0]).await;
    let (material, fingerprint) = keys.last().cloned().expect("the table has rows");
    let accepted = import(
        &emulator,
        "the refused import after a delete",
        json!({"ServerId": &server_id, "HostKeyBody": material}),
    )
    .await;
    let record = described(
        &emulator,
        "the refused import after a delete",
        &server_id,
        &accepted,
    )
    .await;
    assert_members(
        "the refused import after a delete",
        &record,
        &json!({"HostKeyFingerprint": &fingerprint}),
    );
}
