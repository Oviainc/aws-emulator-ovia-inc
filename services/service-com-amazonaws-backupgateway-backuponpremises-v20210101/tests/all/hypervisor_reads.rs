// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the hypervisor a gateway connects to:
//! `ImportHypervisorConfiguration`, `GetHypervisor` and `DeleteHypervisor`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour, and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: each operation resolves the hypervisor its `ServerArn` names out
//! of the persisted store and answers from THAT record — so an import is visible to
//! every later read, an ARN the store holds no hypervisor under is refused as a missing
//! hypervisor, and a read reports the record without changing it. The `State` a read
//! reports is derived from the stored record and the clock, so the activation an import
//! starts becomes visible without anything polling for it.
//!
//! The tables below span that class — hypervisors differing in name, host form, which
//! optional members they carry, which `HypervisorState` they are in and whether their
//! activation is already due; and ARNs differing in resource type, partition, id and
//! length, including each published bound of the modelled `ServerArn` and one past it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, HypervisorConfiguration, HYPERVISOR_ONLINE, HYPERVISOR_PENDING,
    HYPERVISOR_PENDING_FOR,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A hypervisor ARN in `partition`, so the rows span every partition the modelled
/// `ServerArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`).
fn arn_in_partition(partition: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:hypervisor/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `ServerArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 500)`
/// (`smithy_query(query="shape",shape="ServerArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`.
fn arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:h/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// Whether `arn` satisfies the modelled `ServerArn`: `@length(min: 50, max: 500)` and
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="ServerArn")`). Spelled out here rather than reused
/// from the crate, so the assertion is independent of the code that mints the ARN.
fn matches_modelled_server_arn(arn: &str) -> bool {
    if arn.len() < 50 || arn.len() > 500 {
        return false;
    }
    let segment =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some((resource_type, id)) = resource.split_once('/') else {
        return false;
    };
    *literal == "arn"
        && matches!(*partition, "aws" | "aws-cn" | "aws-us-gov")
        && *service == "backup-gateway"
        && [*region, *account, resource_type, id]
            .into_iter()
            .all(segment)
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned
/// in the `Message` member.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// The `ImportHypervisorConfiguration` request body for `name` at `host`.
fn import_body(
    name: &str,
    host: &str,
    kms_key_arn: Option<&str>,
    tags: &BTreeMap<String, String>,
) -> Value {
    let mut body = json!({ "Name": name, "Host": host });
    if let Some(kms_key_arn) = kms_key_arn {
        body["KmsKeyArn"] = json!(kms_key_arn);
    }
    if !tags.is_empty() {
        body["Tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

async fn import(router: &MultiServiceRouter, name: &str, host: &str) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &import_body(name, host, None, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}

/// The persisted `HypervisorState` of the record stored under `arn`, read out of the
/// store rather than off the wire: what a read REPORTS is derived, and this is the value
/// it must not have changed.
fn persisted_state(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor(arn)
        .map(|record| record.state.clone())
        .unwrap_or_else(|| panic!("no hypervisor stored under {arn}"))
}

/// One hypervisor to put in the store, and what `GetHypervisor` must then report for it.
#[derive(Debug, Clone)]
struct Seed {
    case: &'static str,
    /// `Name` — len 1..100, pattern `^[a-zA-Z0-9-]*$`
    /// (`smithy_query(query="shape",shape="HypervisorDetails")`).
    name: String,
    /// `Host` — an IP address or an FQDN, len 3..128. Unique per row: one host, one
    /// hypervisor.
    host: &'static str,
    kms_key_arn: Option<&'static str>,
    log_group_arn: Option<&'static str>,
    /// The `HypervisorState` to persist.
    state: &'static str,
    /// When the record's activation falls due, in seconds from now: a negative offset is
    /// already due, a positive one is not yet, `None` is a record whose state no longer
    /// moves on its own.
    online_due_in: Option<i64>,
    /// The `HypervisorState` a read must report, given `state` and `online_due_in`.
    expected_state: &'static str,
    /// `LatestMetadataSyncStatus` — a `SyncMetadataStatus` value: `CREATED`, `RUNNING`,
    /// `FAILED`, `PARTIALLY_FAILED` or `SUCCEEDED`
    /// (`smithy_query(query="shape",shape="SyncMetadataStatus")`).
    latest_metadata_sync_status: Option<&'static str>,
    latest_metadata_sync_status_message: Option<&'static str>,
    /// `LastSuccessfulMetadataSyncTime`, as epoch seconds.
    last_successful_metadata_sync_time: Option<u64>,
}

impl Seed {
    /// A row carrying only what an import itself sets: the state an import leaves the
    /// record in, with its activation not yet due.
    fn imported(case: &'static str, name: &str, host: &'static str) -> Self {
        Self {
            case,
            name: name.to_string(),
            host,
            kms_key_arn: None,
            log_group_arn: None,
            state: HYPERVISOR_PENDING,
            online_due_in: Some(3_600),
            expected_state: HYPERVISOR_PENDING,
            latest_metadata_sync_status: None,
            latest_metadata_sync_status_message: None,
            last_successful_metadata_sync_time: None,
        }
    }
}

/// Import `seed` over the wire and then set the members no IMPLEMENTED operation writes
/// yet, straight on the stored record.
///
/// `Name`, `Host` and `KmsKeyArn` come from the import request itself. The rest —
/// `LogGroupArn` (`UpdateHypervisor`), a state the service reaches out of band
/// (`OFFLINE`, `ERROR`), and the metadata-sync members
/// (`StartVirtualMachinesMetadataSync`) — are written by operations that are still
/// scaffolds answering 501 under their own stub issues, so the store is the only place
/// they can come from until those are implemented. Seeding them here is what lets this
/// table span every member `HypervisorDetails` reports.
async fn import_seed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    seed: &Seed,
) -> String {
    let reply = call(
        router,
        "ImportHypervisorConfiguration",
        &import_body(&seed.name, seed.host, seed.kms_key_arn, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        seed.case, reply.status, reply.body
    );
    let arn = reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string();

    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    let record = store
        .hypervisor_mut(&arn)
        .expect("the record just imported must be in the store");
    record.log_group_arn = seed.log_group_arn.map(str::to_string);
    record.state = seed.state.to_string();
    record.online_due = seed.online_due_in.map(|offset| {
        let now = SystemTime::now();
        if offset < 0 {
            now - Duration::from_secs(offset.unsigned_abs())
        } else {
            now + Duration::from_secs(offset.unsigned_abs())
        }
    });
    record.latest_metadata_sync_status = seed.latest_metadata_sync_status.map(str::to_string);
    record.latest_metadata_sync_status_message =
        seed.latest_metadata_sync_status_message.map(str::to_string);
    record.last_successful_metadata_sync_time = seed
        .last_successful_metadata_sync_time
        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    arn
}

/// An imported hypervisor round-trips: the ARN the import hands back reads the record
/// back through `GetHypervisor`, with the members the import carried, and the activation
/// the import started becomes visible on its own.
#[tokio::test]
async fn an_imported_hypervisor_reads_back_through_get_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Name, Host, KmsKeyArn). The rows span host form (IPv4, FQDN, the 3-character
    // minimum), name length (1 character, 100 characters) and both forms of KMS ARN the
    // member's pattern accepts (`key/` and `alias/`).
    let rows: [(&str, String, &str, Option<&str>); 5] = [
        ("an IPv4 host, no KMS key", "hv".to_string(), "198.51.100.10", None),
        (
            "an FQDN host with a KMS key ARN",
            "hv-one".to_string(),
            "hypervisor.example.com",
            Some("arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab"),
        ),
        (
            "a KMS alias ARN",
            "hv-2".to_string(),
            "203.0.113.7",
            Some("arn:aws:kms:us-west-2:000000000000:alias/backup-gateway"),
        ),
        ("the shortest host and name", "A".to_string(), "a.b", None),
        (
            "the 100-character name maximum",
            "n".repeat(100),
            "very-long-hypervisor-host.subdomain.example.com",
            None,
        ),
    ];

    let mut arns = Vec::new();
    for (case, name, host, kms_key_arn) in &rows {
        let reply = call(
            &router,
            "ImportHypervisorConfiguration",
            &import_body(name, host, *kms_key_arn, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| panic!("case {case:?}: no HypervisorArn in {:?}", reply.body))
            .to_string();
        assert!(
            matches_modelled_server_arn(&arn),
            "case {case:?}: the minted ARN must satisfy the modelled ServerArn, got \
             {arn:?}"
        );
        assert!(
            arn.contains(&format!(":{REGION}:")),
            "case {case:?}: the ARN must carry the region the caller signed for, got \
             {arn:?}"
        );
        assert!(
            !arns.contains(&arn),
            "case {case:?}: each import must mint its own ARN, got {arn:?} again"
        );
        arns.push(arn);
    }

    for ((case, name, host, kms_key_arn), arn) in rows.iter().zip(&arns) {
        let reply = get_hypervisor(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply.str_at("/Hypervisor/HypervisorArn"),
                reply.str_at("/Hypervisor/Name"),
                reply.str_at("/Hypervisor/Host"),
                reply.str_at("/Hypervisor/KmsKeyArn"),
            ),
            (200, Some(arn.as_str()), Some(name.as_str()), Some(*host), *kms_key_arn),
            "case {case:?}: the import must read back through GetHypervisor, got {} {:?}",
            reply.status,
            reply.body
        );
        // The import started the activation and nothing has finished it, so the state is
        // the one an import leaves: a caller reading straight after the import still sees
        // it.
        assert_eq!(
            reply.str_at("/Hypervisor/State"),
            Some(HYPERVISOR_PENDING),
            "case {case:?}: a freshly imported hypervisor must report PENDING, got {:?}",
            reply.body
        );
        // Nothing has synced metadata for it, so those members are absent rather than
        // reported with a made-up value.
        for member in [
            "/Hypervisor/LogGroupArn",
            "/Hypervisor/LatestMetadataSyncStatus",
            "/Hypervisor/LatestMetadataSyncStatusMessage",
            "/Hypervisor/LastSuccessfulMetadataSyncTime",
        ] {
            assert!(
                reply.body.pointer(member).is_none(),
                "case {case:?}: {member} must be absent on a freshly imported \
                 hypervisor, got {:?}",
                reply.body
            );
        }
    }

    // The activation an import starts is due on the clock alone: no call has to trigger
    // it, and once the stored instant has passed every read reports ONLINE.
    assert!(
        HYPERVISOR_PENDING_FOR > Duration::ZERO,
        "the pending span must be non-zero, so a caller can observe PENDING at all"
    );
    let state = service_state(&*engine).expect("the in-memory engine must expose this state");
    state
        .lock()
        .hypervisor_mut(&arns[0])
        .expect("the imported record must be in the store")
        .online_due = Some(SystemTime::now() - Duration::from_millis(1));
    let reply = get_hypervisor(&router, &arns[0]).await;
    assert_eq!(
        (reply.status, reply.str_at("/Hypervisor/State")),
        (200, Some(HYPERVISOR_ONLINE)),
        "a hypervisor whose activation is due must report ONLINE, got {} {:?}",
        reply.status,
        reply.body
    );

    // One ARN, one record: each ARN reports its own hypervisor and never another's.
    let mut reported = Vec::new();
    for arn in &arns {
        reported.push(
            get_hypervisor(&router, arn)
                .await
                .str_at("/Hypervisor/Name")
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_eq!(
        reported,
        rows.iter().map(|row| row.1.clone()).collect::<Vec<_>>(),
        "each ARN must report its own record"
    );
}

/// Every member `GetHypervisor` reports comes out of the persisted record, for
/// hypervisors spanning the optional members, each `HypervisorState` and the sync
/// history.
#[tokio::test]
async fn get_hypervisor_reports_the_stored_record() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A `KmsKeyArn` and a `LogGroupArn` of the shapes those members' patterns accept
    // (`smithy_query(query="shape",shape="HypervisorDetails")`).
    let kms_key_arn = "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";
    let log_group_arn = "arn:aws:logs:us-west-2:000000000000:log-group:/aws/backup-gateway:*";

    let rows: Vec<Seed> = vec![
        Seed::imported("the fewest members a record can carry", "hv", "198.51.100.10"),
        Seed {
            kms_key_arn: Some(kms_key_arn),
            log_group_arn: Some(log_group_arn),
            online_due_in: Some(-1),
            expected_state: HYPERVISOR_ONLINE,
            ..Seed::imported(
                "an activation the clock has already made due",
                "hv-one",
                "hypervisor.example.com",
            )
        },
        Seed {
            state: "OFFLINE",
            online_due_in: None,
            expected_state: "OFFLINE",
            latest_metadata_sync_status: Some("FAILED"),
            latest_metadata_sync_status_message: Some("The hypervisor could not be reached."),
            ..Seed::imported("offline after a failed sync", "hv-2", "203.0.113.7")
        },
        Seed {
            state: "ERROR",
            online_due_in: None,
            expected_state: "ERROR",
            latest_metadata_sync_status: Some("PARTIALLY_FAILED"),
            ..Seed::imported("in the error state", "A", "a.b")
        },
        Seed {
            log_group_arn: Some(log_group_arn),
            state: HYPERVISOR_ONLINE,
            online_due_in: None,
            expected_state: HYPERVISOR_ONLINE,
            latest_metadata_sync_status: Some("SUCCEEDED"),
            latest_metadata_sync_status_message: Some("The metadata sync succeeded."),
            last_successful_metadata_sync_time: Some(1_700_000_000),
            ..Seed::imported(
                "online with a succeeded sync",
                &"n".repeat(100),
                "very-long-hypervisor-host.subdomain.example.com",
            )
        },
    ];

    let mut arns = Vec::new();
    for seed in &rows {
        arns.push(import_seed(&router, &engine, seed).await);
    }

    for (seed, arn) in rows.iter().zip(&arns) {
        let case = seed.case;
        let reply = get_hypervisor(&router, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetHypervisor on a stored hypervisor must answer 200, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/HypervisorArn"),
            Some(arn.as_str()),
            "case {case:?}: the report must carry the ARN it was asked for, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/Name"),
            Some(seed.name.as_str()),
            "case {case:?}: the report must carry the stored Name, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/Host"),
            Some(seed.host),
            "case {case:?}: the report must carry the stored Host, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/KmsKeyArn"),
            seed.kms_key_arn,
            "case {case:?}: the report must carry the stored KmsKeyArn, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/LogGroupArn"),
            seed.log_group_arn,
            "case {case:?}: the report must carry the stored LogGroupArn, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/State"),
            Some(seed.expected_state),
            "case {case:?}: the report must state the hypervisor's state, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/LatestMetadataSyncStatus"),
            seed.latest_metadata_sync_status,
            "case {case:?}: the report must carry the stored LatestMetadataSyncStatus, \
             got {:?}",
            reply.body
        );
        assert_eq!(
            reply.str_at("/Hypervisor/LatestMetadataSyncStatusMessage"),
            seed.latest_metadata_sync_status_message,
            "case {case:?}: the report must carry the stored \
             LatestMetadataSyncStatusMessage, got {:?}",
            reply.body
        );
        assert_eq!(
            reply
                .body
                .pointer("/Hypervisor/LastSuccessfulMetadataSyncTime")
                .and_then(Value::as_f64),
            seed.last_successful_metadata_sync_time
                .map(|secs| secs as f64),
            "case {case:?}: the report must carry the stored \
             LastSuccessfulMetadataSyncTime, got {:?}",
            reply.body
        );
    }
}

/// A well-formed `ServerArn` the store holds no hypervisor under is refused as a missing
/// hypervisor: the modelled `ResourceNotFoundException`, answered at HTTP 400 with
/// `ErrorCode` `HypervisorNotFound`, whatever the ARN's resource type, partition, id or
/// length — including at each published bound of the modelled shape's length.
#[tokio::test]
async fn an_arn_with_no_stored_hypervisor_is_refused_as_a_missing_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);
    // A gateway and a hypervisor that really exist, so a refusal below is about the ARN
    // that was asked for and not about an empty store.
    let live_gateway = create(&router, "gw-live").await;
    let live_hypervisor = import(&router, "hv-live", "198.51.100.1").await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-1111111111111111"),
        ),
        (
            "another hypervisor id",
            arn_of("hypervisor", "hv-2222222222nonexistent"),
        ),
        (
            "a gateway that exists, which is not a hypervisor",
            live_gateway.clone(),
        ),
        (
            "a gateway that was never created",
            arn_of("gateway", "bgw-0000000000000999"),
        ),
        ("a virtual machine", arn_of("vm", "vm-1111111111111111")),
        (
            "a resource type this service does not name",
            arn_of("appliance", "ap-1111111111111111"),
        ),
        (
            "the china partition",
            arn_in_partition("aws-cn", "hv-3333333333333333"),
        ),
        (
            "the govcloud partition",
            arn_in_partition("aws-us-gov", "hv-4444444444444444"),
        ),
        // The published bounds of the modelled `ServerArn` length: both are accepted by
        // the boundary validator, so both reach the handler, which resolves them.
        ("at the 50-character minimum length", arn_of_length(50)),
        ("at the 500-character maximum length", arn_of_length(500)),
    ];

    for (case, arn) in &rows {
        let refused = get_hypervisor(&router, arn).await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("HypervisorNotFound")
            ),
            "case {case:?}: GetHypervisor on an ARN with no stored hypervisor must \
             answer 400 ResourceNotFoundException / ErrorCode HypervisorNotFound, got {} \
             {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            message_of(&refused),
            "The hypervisor is not found.",
            "case {case:?}: the refusal must state that the hypervisor is missing, got \
             {:?}",
            refused.body
        );
        assert!(
            refused.body.pointer("/Hypervisor").is_none(),
            "case {case:?}: a refused read must report no Hypervisor, got {:?}",
            refused.body
        );
    }

    // The hypervisor that IS stored still reads back, so the refusals above are facts
    // about those ARNs.
    let found = get_hypervisor(&router, &live_hypervisor).await;
    assert_eq!(
        (found.status, found.str_at("/Hypervisor/Name")),
        (200, Some("hv-live")),
        "the stored hypervisor must still read back, got {} {:?}",
        found.status,
        found.body
    );

    // And a record the store no longer holds is refused exactly like one it never held.
    service_state(&*engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .remove_hypervisor(&live_hypervisor)
        .expect("the record must have been in the store");
    let gone = get_hypervisor(&router, &live_hypervisor).await;
    assert_eq!(
        (gone.status, gone.code.as_deref(), gone.str_at("/ErrorCode")),
        (
            400,
            Some("ResourceNotFoundException"),
            Some("HypervisorNotFound")
        ),
        "a hypervisor removed from the store must be refused like one never imported, \
         got {} {:?}",
        gone.status,
        gone.body
    );
}

/// A `HypervisorArn` outside the modelled constraints never reaches the lookup: it is
/// refused with HTTP 400 `ValidationException`, one past each published bound of
/// `ServerArn` (`@length(min: 50, max: 500)`, pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`).
#[tokio::test]
async fn get_hypervisor_refuses_an_arn_outside_the_modelled_constraints() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one character under the 50-character minimum",
            json!({ "HypervisorArn": arn_of_length(49) }),
        ),
        (
            "one character over the 500-character maximum",
            json!({ "HypervisorArn": arn_of_length(501) }),
        ),
        (
            "an underscore, which the pattern's character class excludes",
            json!({ "HypervisorArn": arn_of("hypervisor", "hv_1111111111111111") }),
        ),
        (
            "a partition the pattern does not accept",
            json!({ "HypervisorArn": arn_in_partition("aws-fake", "hv-1111111111111111") }),
        ),
        (
            "another service's ARN",
            json!({
                "HypervisorArn":
                    "arn:aws:storagegateway:us-west-2:000000000000:hypervisor/hv-1111111111111111",
            }),
        ),
        (
            "no resource suffix at all",
            json!({
                "HypervisorArn":
                    "arn:aws:backup-gateway:us-west-2:000000000000:hypervisorhv-11111111111",
            }),
        ),
        ("no HypervisorArn member", json!({})),
    ];

    for (case, body) in &rows {
        let refused = call(&router, "GetHypervisor", body).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a HypervisorArn outside the modelled constraints must be \
             refused with 400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            !message_of(&refused).is_empty(),
            "case {case:?}: the refusal must state why it refused, got {:?}",
            refused.body
        );
    }
}

/// An import is one record per HOST: the record stands for the machine at that address,
/// so importing a host this service already holds a hypervisor for is refused with the
/// modelled `ConflictException` and writes nothing, while another host mints its own
/// record.
#[tokio::test]
async fn importing_a_host_twice_is_refused_as_a_conflict() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The rows differ in host form and in the name the second import asks for: neither
    // is what the conflict keys on.
    let rows: [(&str, &str, &str, &str); 3] = [
        ("an IPv4 host", "198.51.100.10", "hv-first", "hv-second"),
        (
            "an FQDN host",
            "hypervisor.example.com",
            "hv-fqdn",
            "hv-fqdn-again",
        ),
        ("the shortest host", "a.b", "A", "B"),
    ];

    for (case, host, first, second) in &rows {
        let arn = import(&router, first, host).await;
        let refused = call(
            &router,
            "ImportHypervisorConfiguration",
            &import_body(second, host, None, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (
                409,
                Some("ConflictException"),
                Some("HypervisorAlreadyExists")
            ),
            "case {case:?}: importing a host already imported must answer 409 \
             ConflictException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.str_at("/HypervisorArn").is_none(),
            "case {case:?}: a refused import must hand back no HypervisorArn, got {:?}",
            refused.body
        );
        // Nothing was written: the host still reports the record the first import minted.
        let reply = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/Hypervisor/Name")),
            (200, Some(*first)),
            "case {case:?}: the refused import must leave the first record untouched, \
             got {} {:?}",
            reply.status,
            reply.body
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        rows.len(),
        "one record per host: the refused imports must have written nothing"
    );
}

/// The tags an import carries are persisted on the hypervisor, so the tagging operations
/// report and merge them for a hypervisor exactly as they do for a gateway; a tag rule
/// the model cannot express is refused and writes nothing.
#[tokio::test]
async fn the_tags_an_import_carries_are_persisted_on_the_hypervisor() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the tags the import carries, the tags a later TagResource merges in)
    type Tags = &'static [(&'static str, &'static str)];
    let rows: [(&str, Tags, Tags); 4] = [
        ("no tags at all", &[], &[]),
        ("one tag", &[("env", "test")], &[]),
        (
            "several tags, one of them overwritten",
            &[("env", "test"), ("team", "backup")],
            &[("env", "prod")],
        ),
        (
            "tags added only after the import",
            &[],
            &[("env", "prod"), ("owner", "ops")],
        ),
    ];

    for (index, (case, imported, merged)) in rows.iter().enumerate() {
        let tags: BTreeMap<String, String> = imported
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let reply = call(
            &router,
            "ImportHypervisorConfiguration",
            &import_body(
                &format!("hv-tags-{index}"),
                &format!("198.51.100.{}", index + 10),
                None,
                &tags,
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the import must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply
            .str_at("/HypervisorArn")
            .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
            .to_string();

        let listed = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            (listed.status, listed.tags()),
            (200, tags.clone()),
            "case {case:?}: the tags the import carried must read back, got {} {:?}",
            listed.status,
            listed.body
        );

        if !merged.is_empty() {
            let added: BTreeMap<String, String> = merged
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            let tagged = call(
                &router,
                "TagResource",
                &json!({
                    "ResourceARN": arn,
                    "Tags": added
                        .iter()
                        .map(|(key, value)| json!({ "Key": key, "Value": value }))
                        .collect::<Vec<_>>(),
                }),
            )
            .await;
            assert_eq!(
                (tagged.status, tagged.str_at("/ResourceARN")),
                (200, Some(arn.as_str())),
                "case {case:?}: tagging the hypervisor must answer 200 with its ARN, got \
                 {} {:?}",
                tagged.status,
                tagged.body
            );
            let mut expected = tags.clone();
            expected.extend(added);
            let listed = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
            assert_eq!(
                (listed.status, listed.tags()),
                (200, expected),
                "case {case:?}: tagging merges by key, got {} {:?}",
                listed.status,
                listed.body
            );
        }

        // The record itself is unchanged by any of the tagging: a read still reports it.
        let reply = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/Hypervisor/Name")),
            (200, Some(format!("hv-tags-{index}").as_str())),
            "case {case:?}: the hypervisor must still read back, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // A tag key with the reserved `aws:` prefix is refused — the rule is stated in the
    // modelled doc of `Tag$Key`, not in a trait — and the import writes nothing.
    let refused = call(
        &router,
        "ImportHypervisorConfiguration",
        &import_body(
            "hv-reserved",
            "198.51.100.200",
            None,
            &BTreeMap::from([("aws:created-by".to_string(), "me".to_string())]),
        ),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_deref()),
        (400, Some("ValidationException")),
        "a reserved tag key must be refused with 400 ValidationException, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        rows.len(),
        "a refused import must leave no hypervisor behind"
    );
}

/// Reading a hypervisor changes nothing a later caller sees: the `State` reported is
/// derived from the stored activation instant and the clock, so repeated reads agree, the
/// persisted record is untouched, and the transition a read reports is one the clock had
/// already made due.
#[tokio::test]
async fn reading_a_hypervisor_never_moves_its_state() {
    let engine = build_engine();
    let router = build_router(&engine);
    let state = service_state(&*engine).expect("the in-memory engine must expose this state");

    // Imported over the wire, with the store's own timing: the pending span is non-zero,
    // so a caller reading straight after the import still observes PENDING.
    let arn = import(&router, "hv-fresh", "198.51.100.44").await;
    state
        .lock()
        .hypervisor_mut(&arn)
        .expect("the imported record must be in the store")
        .online_due = Some(SystemTime::now() + Duration::from_secs(3_600));

    for read in 0..3 {
        let reply = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/Hypervisor/State")),
            (200, Some(HYPERVISOR_PENDING)),
            "read {read}: a hypervisor whose activation is not yet due must report \
             PENDING, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_state(&engine, &arn),
            HYPERVISOR_PENDING,
            "read {read}: a read must not write the state it reports"
        );
    }
    assert_eq!(
        state.lock().hypervisor_count(),
        1,
        "a read must not add or drop a record"
    );

    // Once the clock has passed the instant the import made the activation due, every
    // read reports ONLINE — and still writes nothing.
    state
        .lock()
        .hypervisor_mut(&arn)
        .expect("the imported record must be in the store")
        .online_due = Some(SystemTime::now() - Duration::from_secs(1));
    for read in 0..2 {
        let reply = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (reply.status, reply.str_at("/Hypervisor/State")),
            (200, Some(HYPERVISOR_ONLINE)),
            "read {read}: a hypervisor whose activation is due must report ONLINE, got \
             {} {:?}",
            reply.status,
            reply.body
        );
    }
    assert_eq!(
        persisted_state(&engine, &arn),
        HYPERVISOR_PENDING,
        "the reported transition is derived from the clock, so the read leaves the \
         persisted record alone"
    );

    // A gateway read is unaffected by any of this: the two resources are separate records
    // in the one store.
    let gateway = create(&router, "gw-beside-hypervisor").await;
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": gateway })).await;
    assert_eq!(
        (
            fetched.status,
            fetched.str_at("/Gateway/GatewayDisplayName")
        ),
        (200, Some("gw-beside-hypervisor")),
        "a gateway must still read back beside the hypervisors, got {} {:?}",
        fetched.status,
        fetched.body
    );
}

/// A hypervisor lifecycle round-trips through the store: an import is visible to every
/// later read, a delete hands back the ARN it removed and makes the record absent, and a
/// delete of an ARN the store holds nothing under is refused exactly like a read of one.
#[tokio::test]
async fn deleting_a_hypervisor_removes_it_from_the_store() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Name, Host). The rows differ in host form and name, which the delete keys
    // on none of: it resolves the ARN.
    let rows: [(&str, &str, &str); 3] = [
        ("an IPv4 host", "hv-one", "198.51.100.10"),
        ("an FQDN host", "hv-two", "hypervisor.example.com"),
        ("the shortest host and name", "A", "a.b"),
    ];

    let mut arns = Vec::new();
    for (_, name, host) in &rows {
        arns.push(import(&router, name, host).await);
    }

    for ((case, _, host), arn) in rows.iter().zip(&arns) {
        let deleted = call(
            &router,
            "DeleteHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_eq!(
            (deleted.status, deleted.str_at("/HypervisorArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: DeleteHypervisor must answer 200 with the ARN it removed, got \
             {} {:?}",
            deleted.status,
            deleted.body
        );

        // The record is gone, so a read answers as it does for one never imported.
        let gone = get_hypervisor(&router, arn).await;
        assert_eq!(
            (gone.status, gone.code.as_deref(), gone.str_at("/ErrorCode")),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("HypervisorNotFound")
            ),
            "case {case:?}: a deleted hypervisor must not read back, got {} {:?}",
            gone.status,
            gone.body
        );

        // Repeating the delete is refused the same way: @idempotent describes the effect
        // of repeating the call, not a success for a resource that is not there.
        let again = call(
            &router,
            "DeleteHypervisor",
            &json!({ "HypervisorArn": arn }),
        )
        .await;
        assert_eq!(
            (
                again.status,
                again.code.as_deref(),
                again.str_at("/ErrorCode")
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("HypervisorNotFound")
            ),
            "case {case:?}: deleting a hypervisor twice must be refused, got {} {:?}",
            again.status,
            again.body
        );

        // And the host is free again: the record that held it is gone, so an import of
        // the same host now mints a new one.
        let reimported = import(&router, "hv-reimported", host).await;
        assert_ne!(
            &reimported, arn,
            "case {case:?}: re-importing a deleted host must mint a new ARN"
        );
        let reread = get_hypervisor(&router, &reimported).await;
        assert_eq!(
            (reread.status, reread.str_at("/Hypervisor/Host")),
            (200, Some(*host)),
            "case {case:?}: the re-imported hypervisor must read back, got {} {:?}",
            reread.status,
            reread.body
        );
        call(
            &router,
            "DeleteHypervisor",
            &json!({ "HypervisorArn": reimported }),
        )
        .await;
    }

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .hypervisor_count(),
        0,
        "every record imported above was deleted, so the store must hold none"
    );

    // A gateway is a different resource: its ARN names no hypervisor, so a delete of it
    // through this operation is refused and the gateway survives.
    let gateway = create(&router, "gw-not-a-hypervisor").await;
    let refused = call(
        &router,
        "DeleteHypervisor",
        &json!({ "HypervisorArn": gateway }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "DeleteHypervisor on a gateway ARN must be refused, got {} {:?}",
        refused.status,
        refused.body
    );
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": gateway })).await;
    assert_eq!(
        fetched.status, 200,
        "the gateway must survive a DeleteHypervisor naming it, got {} {:?}",
        fetched.status, fetched.body
    );
}

/// The one import path this service has, reached through the state API, is the same one
/// the wire operation uses: a record put there with [`HypervisorConfiguration`] reads back
/// through `GetHypervisor` identically. This pins the state seam the sibling hypervisor
/// operations will extend.
#[tokio::test]
async fn a_record_imported_through_the_state_api_reads_back_the_same_way() {
    let engine = build_engine();
    let router = build_router(&engine);
    let state = service_state(&*engine).expect("the in-memory engine must expose this state");

    let stored = state.lock().import_hypervisor(
        REGION,
        HypervisorConfiguration {
            name: "hv-seeded".to_string(),
            host: "198.51.100.77".to_string(),
            kms_key_arn: None,
            tags: BTreeMap::from([("env".to_string(), "test".to_string())]),
        },
        SystemTime::now(),
    );
    assert!(
        matches_modelled_server_arn(&stored.arn),
        "the minted ARN must satisfy the modelled ServerArn, got {:?}",
        stored.arn
    );

    let reply = get_hypervisor(&router, &stored.arn).await;
    assert_eq!(
        (
            reply.status,
            reply.str_at("/Hypervisor/Name"),
            reply.str_at("/Hypervisor/Host"),
            reply.str_at("/Hypervisor/State"),
        ),
        (
            200,
            Some("hv-seeded"),
            Some("198.51.100.77"),
            Some(HYPERVISOR_PENDING)
        ),
        "the seeded record must read back, got {} {:?}",
        reply.status,
        reply.body
    );

    // And the same host cannot be imported again over the wire: the conflict is a fact
    // of the store, not of which path wrote the record.
    let refused = call(
        &router,
        "ImportHypervisorConfiguration",
        &import_body("hv-again", "198.51.100.77", None, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_deref()),
        (409, Some("ConflictException")),
        "importing the seeded host again must conflict, got {} {:?}",
        refused.status,
        refused.body
    );
}
