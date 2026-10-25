// SPDX-License-Identifier: CC0-1.0

//! Change data capture on an Amazon Keyspaces table, end to end over the wire: from the
//! control plane that enables it (`CreateTable`/`UpdateTable` with a `cdcSpecification`,
//! `GetTable` reporting it) to the KeyspacesStreams surface that serves the stream it
//! created (`ListStreams`, `GetStream`, `GetShardIterator`, `GetRecords`).
//!
//! Both services are mounted on ONE router over ONE shared engine, because that is the
//! claim under test: the stream the control plane enables is the stream the data plane
//! reports. Every assertion goes through the HTTP surface a real client uses.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A modelled optional input member of an implemented operation is ACCEPTED and
//!    applied to state, never refused: `cdcSpecification` on `CreateTable` and
//!    `UpdateTable` creates (or removes) the table's CDC stream, and `GetTable` reports
//!    it back as `cdcSpecification` + `latestStreamArn`. The rows span the whole
//!    `ViewType` value set, plus the member being omitted (its documented default).
//! 2. `CdcStatus` decides whether the table has a stream at all: `ENABLED`/`ENABLING`
//!    ask for one, `DISABLED`/`DISABLING` for none. The rows are the whole value set.
//! 3. A stream Keyspaces has just created is reported in its transient status and
//!    reaches the terminal one on the clock alone.
//! 4. The view type is fixed when the stream is created, so a request naming another one
//!    for an existing stream is refused with the modelled `ValidationException`; turning
//!    CDC off and on again gives the table a NEW stream, which may publish another view.
//! 5. The tags a `cdcSpecification` carries are the STREAM's tags, reported for the
//!    stream's own ARN — the table's own tags on top exactly when `propagateTags` says
//!    `TABLE`.
//! 6. A stream cannot outlive the table it follows: deleting the table (or its keyspace)
//!    takes the stream with it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    CDC_PROPAGATE_TAGS_VALUES, CDC_STATUS_VALUES, VIEW_TYPE_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;
use smithy_server_runtime::MultiServiceRouter;

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, table_arn, REGION,
};

/// How long a test waits before expecting a stream's transient CDC status to have
/// settled: the span the stream store stores, plus a margin for a loaded test machine.
fn cdc_settle_wait() -> Duration {
    keyspaces::CDC_STATUS_TRANSITION_DELAY + Duration::from_millis(250)
}

/// Both services that sign as `cassandra`, on one router over one shared engine: the
/// Keyspaces control plane and the KeyspacesStreams data plane.
///
/// `tests/all/keyspace_table_lifecycle.rs`'s harness mounts only the control plane,
/// which is all its suites need; this one mounts the pair because the rules here are
/// precisely about what crosses between them.
pub(crate) struct Cdc {
    router: MultiServiceRouter,
}

impl Cdc {
    pub(crate) fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these requests carry a well-formed credential scope so they route
        // and carry a region, but they are not really signed.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One Amazon Keyspaces call (`X-Amz-Target: KeyspacesService.<Op>`).
    pub(crate) async fn keyspaces(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch("cassandra", "KeyspacesService", operation, body)
            .await
    }

    /// One KeyspacesStreams call (`X-Amz-Target: KeyspacesStreams.<Op>`), at that
    /// service's own endpoint host.
    pub(crate) async fn streams(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch("cassandra-streams", "KeyspacesStreams", operation, body)
            .await
    }

    async fn dispatch(
        &self,
        endpoint: &str,
        target_prefix: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header("host", format!("{endpoint}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cassandra/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=abc123"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered {status} with a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(raw)
                )
            })
        };
        (status, parsed)
    }

    /// Create `keyspace`, failing the test if it cannot be created.
    pub(crate) async fn create_keyspace(&self, keyspace: &str) {
        assert_ok(
            keyspace,
            self.keyspaces("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
    }

    /// `GetTable` for one table, as a 200 body.
    pub(crate) async fn get_table(&self, case: &str, keyspace: &str, table: &str) -> Value {
        assert_ok(
            case,
            self.keyspaces(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
        )
    }

    /// The stream ARNs KeyspacesStreams lists for one table, in the order it lists them.
    pub(crate) async fn listed_stream_arns(
        &self,
        case: &str,
        keyspace: &str,
        table: &str,
    ) -> Vec<String> {
        let listed = assert_ok(
            case,
            self.streams(
                "ListStreams",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
        );
        listed["streams"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListStreams must report a list: {listed}"))
            .iter()
            .map(|stream| stream["streamArn"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// The CDC members of a `GetTable` report, as `(status, viewType, latestStreamArn)`.
pub(crate) fn reported_cdc(table: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let text = |value: &Value| value.as_str().map(str::to_string);
    (
        text(&table["cdcSpecification"]["status"]),
        text(&table["cdcSpecification"]["viewType"]),
        text(&table["latestStreamArn"]),
    )
}

/// RULE 1: `CreateTable` accepts a `cdcSpecification`, the stream it asks for exists in
/// the shared store, `GetTable` reports it, and the whole KeyspacesStreams read surface
/// serves it.
///
/// The rows are every `ViewType` the model declares — the service's own generated
/// catalog `ops::types::VIEW_TYPE_VALUES`, so this test cannot drift from the model —
/// plus the member omitted, whose documented default is `NEW_AND_OLD_IMAGES`
/// (`CdcSpecification$viewType`: "both versions of the row … This is the default").
#[tokio::test]
async fn create_table_with_a_cdc_specification_enables_the_stream_the_streams_api_serves() {
    let emulator = Cdc::start();
    let keyspace = "cdc_create_ks";
    emulator.create_keyspace(keyspace).await;

    let mut rows: Vec<(String, Option<&str>, &str)> = vec![(
        "view_type_omitted".to_string(),
        None,
        "NEW_AND_OLD_IMAGES",
    )];
    for view_type in VIEW_TYPE_VALUES {
        rows.push((
            format!("view_type_{}", view_type.to_lowercase()),
            Some(view_type),
            view_type,
        ));
    }

    for (case, supplied, expected_view_type) in rows {
        let case = case.as_str();
        let mut specification = json!({"status": "ENABLED"});
        if let Some(view_type) = supplied {
            specification["viewType"] = json!(view_type);
        }
        let created = assert_ok(
            case,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": specification,
                    }),
                )
                .await,
        );
        assert_eq!(
            created["resourceArn"],
            json!(table_arn(keyspace, case)),
            "case {case:?}: a create carrying a modelled member answers with the table's \
             ARN, got {created}"
        );

        // The control plane reports the stream it created, by ARN and by setting.
        let table = emulator.get_table(case, keyspace, case).await;
        let (status, view_type, stream_arn) = reported_cdc(&table);
        let status = status.unwrap_or_default();
        assert!(
            CDC_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: cdcSpecification.status {status:?} is not a modelled CdcStatus \
             value: {table}"
        );
        assert_eq!(
            view_type.as_deref(),
            Some(expected_view_type),
            "case {case:?}: expected viewType {expected_view_type:?}, got {table}"
        );
        let stream_arn = stream_arn.unwrap_or_else(|| {
            panic!("case {case:?}: a table with CDC enabled reports a latestStreamArn: {table}")
        });
        assert!(
            stream_arn.starts_with(&format!("arn:aws:cassandra:{REGION}:"))
                && stream_arn.contains(&format!("/keyspace/{keyspace}/table/{case}/stream/")),
            "case {case:?}: the stream ARN must name the table it follows (modelled \
             StreamArn pattern ^arn:(aws[a-zA-Z0-9-]*):cassandra:.+), got {stream_arn}"
        );

        // The data plane answers for exactly that stream.
        assert_eq!(
            emulator.listed_stream_arns(case, keyspace, case).await,
            vec![stream_arn.clone()],
            "case {case:?}: ListStreams must report the stream the control plane enabled"
        );
        let stream = assert_ok(
            case,
            emulator
                .streams("GetStream", json!({"streamArn": stream_arn}))
                .await,
        );
        assert_eq!(
            (
                stream["streamViewType"].as_str(),
                stream["keyspaceName"].as_str(),
                stream["tableName"].as_str()
            ),
            (Some(expected_view_type), Some(keyspace), Some(case)),
            "case {case:?}: GetStream must report the table and view type the create \
             asked for, got {stream}"
        );
        let shard_id = stream["shards"][0]["shardId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a new stream must hold a shard, got {stream}")
            })
            .to_string();

        // And its records are readable: the table has taken no write, so the shard
        // serves an empty page rather than failing.
        let iterator = assert_ok(
            case,
            emulator
                .streams(
                    "GetShardIterator",
                    json!({
                        "streamArn": stream_arn,
                        "shardId": shard_id,
                        "shardIteratorType": "TRIM_HORIZON",
                    }),
                )
                .await,
        );
        let records = assert_ok(
            case,
            emulator
                .streams(
                    "GetRecords",
                    json!({"shardIterator": iterator["shardIterator"]}),
                )
                .await,
        );
        assert_eq!(
            records["changeRecords"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a stream of a table nothing wrote to must serve no record, \
             got {records}"
        );
        assert!(
            records["nextShardIterator"].is_string(),
            "case {case:?}: an open shard must hand back the iterator to poll next, got \
             {records}"
        );
    }
}

/// RULE 2: the `CdcStatus` a `cdcSpecification` carries decides whether the table has a
/// stream. The rows are the whole modelled value set (`ops::types::CDC_STATUS_VALUES`).
#[tokio::test]
async fn the_cdc_status_decides_whether_the_table_has_a_stream() {
    let emulator = Cdc::start();
    let keyspace = "cdc_status_ks";
    emulator.create_keyspace(keyspace).await;

    for status in CDC_STATUS_VALUES {
        let case = format!("status_{}", status.to_lowercase());
        let case = case.as_str();
        // Every value is accepted: none of them is a reason to refuse a create.
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": {"status": status, "viewType": "KEYS_ONLY"},
                    }),
                )
                .await,
        );
        let asks_for_a_stream = *status == "ENABLED" || *status == "ENABLING";
        let table = emulator.get_table(case, keyspace, case).await;
        let (reported_status, _, stream_arn) = reported_cdc(&table);
        assert_eq!(
            stream_arn.is_some(),
            asks_for_a_stream,
            "case {case:?}: expected a stream to exist: {asks_for_a_stream}, got {table}"
        );
        assert_eq!(
            reported_status.is_some(),
            asks_for_a_stream,
            "case {case:?}: a table with no CDC stream reports no cdcSpecification, got \
             {table}"
        );
        assert_eq!(
            emulator.listed_stream_arns(case, keyspace, case).await.len(),
            usize::from(asks_for_a_stream),
            "case {case:?}: ListStreams must agree with the control plane"
        );
    }
}

/// RULE 1, from the other side: a create that is REFUSED stores nothing — including the
/// stream its `cdcSpecification` asked for. A stream that outlived a refused create would
/// be reported by `ListStreams` for a table that does not exist, and would then be handed
/// to the next create of that name whatever view type IT asks for.
///
/// The rows are refusals that can only be settled once the store has been reached, so
/// each one happens at the point in the handler where the stream is minted: an incomplete
/// warm throughput, each half of it in turn.
#[tokio::test]
async fn a_refused_create_leaves_no_cdc_stream_behind() {
    let emulator = Cdc::start();
    let keyspace = "cdc_refused_ks";
    emulator.create_keyspace(keyspace).await;

    // "Specifies the warm throughput settings for a table": a create carries both unit
    // counts or neither, so each half on its own is the modelled refusal.
    let rows: [(&str, Value); 2] = [
        ("read_units_only", json!({"readUnitsPerSecond": 12000})),
        ("write_units_only", json!({"writeUnitsPerSecond": 4000})),
    ];

    for (case, warm_throughput) in rows {
        assert_failure(
            case,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": {"status": "ENABLED", "viewType": "NEW_IMAGE"},
                        "warmThroughputSpecification": warm_throughput,
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        assert!(
            emulator
                .listed_stream_arns(case, keyspace, case)
                .await
                .is_empty(),
            "case {case:?}: a refused create must leave no stream behind"
        );

        // And the next create of that same name gets a stream of its own, publishing the
        // view type IT asked for rather than the refused request's.
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": {"status": "ENABLED", "viewType": "KEYS_ONLY"},
                    }),
                )
                .await,
        );
        let table = emulator.get_table(case, keyspace, case).await;
        let (_, view_type, stream_arn) = reported_cdc(&table);
        assert_eq!(
            view_type.as_deref(),
            Some("KEYS_ONLY"),
            "case {case:?}: the accepted create's own view type must be reported, got {table}"
        );
        assert_eq!(
            emulator.listed_stream_arns(case, keyspace, case).await,
            vec![stream_arn.unwrap_or_default()],
            "case {case:?}: the table must have exactly the one stream its create enabled"
        );
    }
}

/// RULE 3: a stream the control plane has just created is reported in the transient
/// status the model carries for it, and the clock alone moves it to the terminal one —
/// through BOTH services, which read the one stored status.
#[tokio::test]
async fn a_new_cdc_stream_is_enabling_until_the_clock_enables_it() {
    let emulator = Cdc::start();
    let keyspace = "cdc_transient_ks";
    emulator.create_keyspace(keyspace).await;
    assert_ok(
        "create",
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "tbl",
                    "schemaDefinition": minimal_schema("pk"),
                    "cdcSpecification": {"status": "ENABLED"},
                }),
            )
            .await,
    );

    let fresh = emulator.get_table("fresh", keyspace, "tbl").await;
    let (status, _, stream_arn) = reported_cdc(&fresh);
    assert_eq!(
        status.as_deref(),
        Some(keyspaces::CDC_STATUS_ENABLING),
        "case fresh: a stream just created reports its transient status, got {fresh}"
    );
    let stream_arn = stream_arn.expect("the created stream's ARN");
    let fresh_stream = assert_ok(
        "fresh",
        emulator
            .streams("GetStream", json!({"streamArn": stream_arn.clone()}))
            .await,
    );
    assert_eq!(
        fresh_stream["streamStatus"].as_str(),
        Some(keyspaces::CDC_STATUS_ENABLING),
        "case fresh: both services read the one stored status, got {fresh_stream}"
    );

    tokio::time::sleep(cdc_settle_wait()).await;
    let settled = emulator.get_table("settled", keyspace, "tbl").await;
    assert_eq!(
        reported_cdc(&settled).0.as_deref(),
        Some(keyspaces::CDC_STATUS_ENABLED),
        "case settled: the clock must move the stream to ENABLED, got {settled}"
    );
    let settled_stream = assert_ok(
        "settled",
        emulator
            .streams("GetStream", json!({"streamArn": stream_arn}))
            .await,
    );
    assert_eq!(
        settled_stream["streamStatus"].as_str(),
        Some(keyspaces::CDC_STATUS_ENABLED),
        "case settled: GetStream must report the same settled status, got {settled_stream}"
    );
}

/// RULES 1 and 4: `UpdateTable` turns CDC on and off for a stored table, `GetTable`
/// reports each change, the view type cannot be changed while the stream exists, and a
/// table CDC is re-enabled on gets a NEW stream.
#[tokio::test]
async fn update_table_turns_cdc_on_and_off_and_reports_every_change() {
    let emulator = Cdc::start();
    let keyspace = "cdc_update_ks";
    let table = "tbl";
    emulator.create_keyspace(keyspace).await;
    assert_ok(
        "create",
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // A table created without a CDC specification has no stream at all.
    let without = emulator.get_table("without_cdc", keyspace, table).await;
    assert_eq!(
        reported_cdc(&without),
        (None, None, None),
        "case without_cdc: a table nothing enabled CDC for reports none of it, got {without}"
    );

    // Turning CDC on is an update like any other: one setting, accepted, then reported.
    assert_ok(
        "enable",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "ENABLED", "viewType": "NEW_IMAGE"},
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait().max(cdc_settle_wait())).await;
    let enabled = emulator.get_table("enable", keyspace, table).await;
    let (status, view_type, first_arn) = reported_cdc(&enabled);
    assert_eq!(
        (status.as_deref(), view_type.as_deref()),
        (
            Some(keyspaces::CDC_STATUS_ENABLED),
            Some("NEW_IMAGE")
        ),
        "case enable: the update must be reported back, got {enabled}"
    );
    let first_arn = first_arn.expect("the enabled stream's ARN");

    // "After you create the stream, you can't make changes to this selection": another
    // view type for the same stream is the modelled refusal, and it changes nothing.
    assert_failure(
        "view_type_change",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "ENABLED", "viewType": "OLD_IMAGE"},
                }),
            )
            .await,
        400,
        "ValidationException",
    );
    assert_eq!(
        reported_cdc(&emulator.get_table("view_type_change", keyspace, table).await),
        (
            Some(keyspaces::CDC_STATUS_ENABLED.to_string()),
            Some("NEW_IMAGE".to_string()),
            Some(first_arn.clone())
        ),
        "case view_type_change: a refused update must leave the stream exactly as it was"
    );

    // "you can only update one specific table setting per update operation".
    assert_failure(
        "cdc_with_another_setting",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "DISABLED"},
                    "defaultTimeToLive": 86400,
                }),
            )
            .await,
        400,
        "ValidationException",
    );

    // Turning CDC off stops the stream and settles it into DISABLED; the stream stays
    // readable, and the table still reports it as its latest.
    assert_ok(
        "disable",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "DISABLED"},
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait().max(cdc_settle_wait())).await;
    let disabled = emulator.get_table("disable", keyspace, table).await;
    assert_eq!(
        reported_cdc(&disabled),
        (
            Some(keyspaces::CDC_STATUS_DISABLED.to_string()),
            Some("NEW_IMAGE".to_string()),
            Some(first_arn.clone())
        ),
        "case disable: a disabled stream is reported as DISABLED, got {disabled}"
    );
    let stream = assert_ok(
        "disable",
        emulator
            .streams("GetStream", json!({"streamArn": first_arn.clone()}))
            .await,
    );
    assert_eq!(
        stream["streamStatus"].as_str(),
        Some(keyspaces::CDC_STATUS_DISABLED),
        "case disable: GetStream must report the stream as DISABLED, got {stream}"
    );

    // Re-enabling gives the table a NEW stream, which is free to publish another view:
    // the old one keeps the records it captured.
    assert_ok(
        "re_enable",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "ENABLED", "viewType": "KEYS_ONLY"},
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait().max(cdc_settle_wait())).await;
    let re_enabled = emulator.get_table("re_enable", keyspace, table).await;
    let (status, view_type, second_arn) = reported_cdc(&re_enabled);
    let second_arn = second_arn.expect("the re-enabled stream's ARN");
    assert_eq!(
        (status.as_deref(), view_type.as_deref()),
        (Some(keyspaces::CDC_STATUS_ENABLED), Some("KEYS_ONLY")),
        "case re_enable: the new stream's own settings are reported, got {re_enabled}"
    );
    assert_ne!(
        second_arn, first_arn,
        "case re_enable: a re-enabled table must get a new stream, got {second_arn}"
    );
    let mut listed = emulator.listed_stream_arns("re_enable", keyspace, table).await;
    listed.sort();
    let mut expected = vec![first_arn, second_arn];
    expected.sort();
    assert_eq!(
        listed, expected,
        "case re_enable: both the disabled and the new stream must be listed"
    );
}

/// RULE 5: the tags a `cdcSpecification` carries belong to the STREAM, and are reported
/// for the stream's own ARN — with the table's tags on top exactly when `propagateTags`
/// names `TABLE`. The rows are the whole modelled `CdcPropagateTags` value set
/// (`ops::types::CDC_PROPAGATE_TAGS_VALUES`) plus the member omitted.
#[tokio::test]
async fn the_cdc_specifications_tags_are_the_streams_own_tags() {
    let emulator = Cdc::start();
    let keyspace = "cdc_tags_ks";
    emulator.create_keyspace(keyspace).await;

    let mut rows: Vec<(String, Option<&str>)> = vec![("propagate_omitted".to_string(), None)];
    for propagate in CDC_PROPAGATE_TAGS_VALUES {
        rows.push((
            format!("propagate_{}", propagate.to_lowercase()),
            Some(propagate),
        ));
    }

    for (case, propagate) in rows {
        let case = case.as_str();
        let mut specification = json!({
            "status": "ENABLED",
            "tags": [{"key": "stream_tag", "value": case}],
        });
        if let Some(propagate) = propagate {
            specification["propagateTags"] = json!(propagate);
        }
        assert_ok(
            case,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "tags": [{"key": "table_tag", "value": case}],
                        "cdcSpecification": specification,
                    }),
                )
                .await,
        );
        let table = emulator.get_table(case, keyspace, case).await;
        let stream_arn = reported_cdc(&table).2.expect("the created stream's ARN");

        let inherits = propagate == Some("TABLE");
        let mut expected: Vec<Value> = vec![json!({"key": "stream_tag", "value": case})];
        if inherits {
            expected.push(json!({"key": "table_tag", "value": case}));
        }
        expected.sort_by_key(|tag| tag["key"].as_str().unwrap_or_default().to_string());

        let listed = assert_ok(
            case,
            emulator
                .keyspaces(
                    "ListTagsForResource",
                    json!({"resourceArn": stream_arn}),
                )
                .await,
        );
        let mut got: Vec<Value> = listed["tags"].as_array().cloned().unwrap_or_default();
        got.sort_by_key(|tag| tag["key"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            got, expected,
            "case {case:?}: expected the stream's tags to be {expected:?}, got {listed}"
        );

        // The table's own tags are untouched by what the stream carries.
        let table_tags = assert_ok(
            case,
            emulator
                .keyspaces(
                    "ListTagsForResource",
                    json!({"resourceArn": table_arn(keyspace, case)}),
                )
                .await,
        );
        assert_eq!(
            table_tags["tags"],
            json!([{"key": "table_tag", "value": case}]),
            "case {case:?}: the table keeps exactly its own tags, got {table_tags}"
        );
    }

    // A stream whose specification carried no tags holds none, which the modelled
    // `TagList` (len 1..60) expresses by the member being absent.
    assert_ok(
        "no_tags",
        emulator
            .keyspaces(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "no_tags",
                    "schemaDefinition": minimal_schema("pk"),
                    "cdcSpecification": {"status": "ENABLED"},
                }),
            )
            .await,
    );
    let table = emulator.get_table("no_tags", keyspace, "no_tags").await;
    let stream_arn = reported_cdc(&table).2.expect("the created stream's ARN");
    let listed = assert_ok(
        "no_tags",
        emulator
            .keyspaces("ListTagsForResource", json!({"resourceArn": stream_arn}))
            .await,
    );
    assert!(
        listed.get("tags").is_none_or(Value::is_null),
        "case no_tags: a stream holding no tags reports no tags member, got {listed}"
    );

    // A tag the per-resource rules refuse is refused wherever it is supplied, and
    // whatever the CDC status asks for: the reserved key prefix is bad input even in a
    // specification that asks for no stream at all. (`keyspaces::RESERVED_TAG_KEY_PREFIX`
    // is this service's own constant, so the rows cannot drift from the rule.)
    for status in CDC_STATUS_VALUES {
        let case = format!("reserved_key_{}", status.to_lowercase());
        assert_failure(
            case.as_str(),
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": case,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": {
                            "status": status,
                            "tags": [{
                                "key": format!("{}reserved", keyspaces::RESERVED_TAG_KEY_PREFIX),
                                "value": "x",
                            }],
                        },
                    }),
                )
                .await,
            400,
            "ValidationException",
        );
        // And nothing was stored for it.
        assert_failure(
            case.as_str(),
            emulator
                .keyspaces(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": case}),
                )
                .await,
            404,
            "ResourceNotFoundException",
        );
    }
}

/// RULE 6: a stream follows one table's changes, so deleting the table — or the keyspace
/// that holds it — takes the stream with it.
#[tokio::test]
async fn a_deleted_table_takes_its_cdc_stream_with_it() {
    let emulator = Cdc::start();
    let keyspace = "cdc_delete_ks";
    emulator.create_keyspace(keyspace).await;
    for table in ["kept", "dropped"] {
        assert_ok(
            table,
            emulator
                .keyspaces(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                        "cdcSpecification": {"status": "ENABLED"},
                    }),
                )
                .await,
        );
    }
    tokio::time::sleep(settle_wait()).await;
    let dropped_arn = reported_cdc(&emulator.get_table("dropped", keyspace, "dropped").await)
        .2
        .expect("the dropped table's stream ARN");

    assert_ok(
        "delete_table",
        emulator
            .keyspaces(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "dropped"}),
            )
            .await,
    );
    assert!(
        emulator
            .listed_stream_arns("delete_table", keyspace, "dropped")
            .await
            .is_empty(),
        "case delete_table: the deleted table's stream must be gone"
    );
    assert_failure(
        "delete_table",
        emulator
            .streams("GetStream", json!({"streamArn": dropped_arn}))
            .await,
        404,
        "ResourceNotFoundException",
    );
    // The other table's stream is untouched.
    assert_eq!(
        emulator
            .listed_stream_arns("delete_table", keyspace, "kept")
            .await
            .len(),
        1,
        "case delete_table: deleting one table must not touch another's stream"
    );

    assert_ok(
        "delete_keyspace",
        emulator
            .keyspaces("DeleteKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let listed = assert_ok("delete_keyspace", emulator.streams("ListStreams", json!({})).await);
    assert_eq!(
        listed["streams"].as_array().map(Vec::len),
        Some(0),
        "case delete_keyspace: no table of the keyspace is left to follow, got {listed}"
    );
}
