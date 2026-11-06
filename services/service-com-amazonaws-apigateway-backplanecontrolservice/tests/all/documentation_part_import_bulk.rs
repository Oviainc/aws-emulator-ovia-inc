// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning `ImportDocumentationParts` in BULK, and the atomicity of an
//! import the service cannot carry out in full.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. [`Gateway`], `create_api`, `string_member` and `assert_failure` are this crate's
//! one wire-test harness, and the documentation-part helpers (`import_body`,
//! `import_parts`, `listed_ids`, `listed_entities`, `get_part`) are reused from the
//! sibling `documentation_parts` suite rather than written a second way.
//!
//! The RULE these cases assert, over sets of inputs rather than one recorded call:
//! an import stores one documentation part per DISTINCT API entity its body documents,
//! answers with the identifier of each entity's part, and — when it cannot be carried out
//! in full — leaves the API's documentation set exactly as it was. `documentation_parts`
//! covers one- and two-entry bodies, the `PutMode` values and the malformed-body
//! refusals; this suite covers the entry COUNT the rule generalises over (up to API
//! Gateway's published per-API quota and one entity past it), a body that documents one
//! entity several times, and the all-or-nothing refusals.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use service_com_amazonaws_apigateway_backplanecontrolservice::state::AUDIT_TARGET;

use crate::documentation_parts::{
    content_map, get_part, import_body, import_parts, imported_content_map, listed_entities,
    listed_ids, BAD_REQUEST,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// The modelled error code for an import that would grow an API past its
/// documentation-part quota; `LimitExceededException` is modelled at HTTP 429.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// API Gateway's published default quota "Documentation parts: 2000 per API", which
/// `crate::state::MAX_DOCUMENTATION_PARTS_PER_API` carries in the service.
const DOCUMENTATION_PARTS_PER_API: usize = 2_000;

/// The most entries the import reader accepts in one body: an emulator resource guard
/// (`import_documentation_parts::MAX_IMPORT_ENTRIES`), a small multiple of the quota
/// above, so a body may document one entity many times and still be read.
const MAX_IMPORT_ENTRIES: usize = 4 * DOCUMENTATION_PARTS_PER_API;

/// `count` entries, each documenting a DIFFERENT API entity.
///
/// `METHOD` makes `path` and `method` valid and nothing else, so every entry is stored
/// with the location it was given; a documentation part documents an entity by name, so
/// the paths need no `Resource` behind them.
fn distinct_entries(count: usize) -> Vec<(Value, Value)> {
    (0..count)
        .map(|index| {
            (
                json!({
                    "type": "METHOD",
                    "path": format!("/entity-{index}"),
                    "method": "GET",
                }),
                json!({ "description": format!("entity {index}") }),
            )
        })
        .collect()
}

/// The `ids` an accepted import answered with, asserting the modelled 200.
fn imported_ids(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let ids = answer.body["ids"].as_array().unwrap_or_else(|| {
        panic!(
            "case {case}: expected an ids array, got {:?}",
            answer.body
        )
    });
    ids.iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string id, got {id:?}"))
                .to_string()
        })
        .collect()
}

/// One import this service cannot carry out in full, and the refusal it must answer.
struct RefusedImport {
    case: &'static str,
    mode: Option<&'static str>,
    fail_on_warnings: bool,
    body: String,
    status: u16,
    code: &'static str,
}

/// The `(location, content map)` pairs of `entries`, as a set: what the API's
/// documentation set must hold after importing them, whatever order it reports them in.
fn documented(entries: &[(Value, Value)]) -> BTreeSet<(String, String)> {
    entries
        .iter()
        .map(|(location, properties)| (location.to_string(), properties.to_string()))
        .collect()
}

#[tokio::test]
async fn an_import_stores_one_part_per_entity_whatever_the_bodys_entry_count() {
    // Each row is one entry COUNT — the thing the rule generalises over: a single entry,
    // a handful, and a body filling a whole reported page.
    for count in [1usize, 2, 7, 25, 50] {
        let case = format!("an import of {count} entries");
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-import-bulk").await;

        let entries = distinct_entries(count);
        let answer = import_parts(&gateway, &api, None, false, &import_body(&entries)).await;
        let ids = imported_ids(&answer, &case);
        assert!(
            answer.body["warnings"].is_null(),
            "case {case}: expected no warnings for a body of storable entries, got {:?}",
            answer.body["warnings"]
        );
        assert_eq!(
            ids.len(),
            count,
            "case {case}: expected one identifier per entry, got {ids:?}"
        );
        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            count,
            "case {case}: expected a distinct identifier per entity, got {ids:?}"
        );

        // Each identifier reads back as the entry it was minted for, through the
        // single-part reader: the ids are reported in the body's order.
        for (index, id) in ids.iter().enumerate() {
            let read = get_part(&gateway, &api, id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected GetDocumentationPart({id}) to answer 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["location"], entries[index].0,
                "case {case}: expected the location of entry {index}, got {:?}",
                read.body["location"]
            );
            assert_eq!(
                string_member(&read.body, "properties", &case),
                imported_content_map(&format!("entity {index}")),
                "case {case}: expected the content map of entry {index}, got {:?}",
                read.body["properties"]
            );
        }

        // And the collection reader reports exactly the imported documentation set.
        let stored = listed_entities(&gateway, &api, &case).await;
        assert_eq!(
            stored.len(),
            count,
            "case {case}: expected one stored part per entity, got {stored:?}"
        );
        let stored = documented(&stored);
        assert_eq!(
            stored,
            documented(&entries),
            "case {case}: expected the imported documentation set, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_body_that_documents_one_entity_several_times_stores_one_part_for_it() {
    // Each row is how many times the SAME entity is documented in one body; the entity
    // is one, so the stored documentation set must be one part however many entries
    // name it.
    for repeats in [2usize, 3, 5] {
        let case = format!("{repeats} entries for one entity");
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-import-duplicates").await;

        let location = json!({ "type": "METHOD", "path": "/pets", "method": "GET" });
        let entries: Vec<(Value, Value)> = (0..repeats)
            .map(|revision| {
                (
                    location.clone(),
                    json!({ "description": format!("revision {revision}") }),
                )
            })
            .collect();
        let answer = import_parts(&gateway, &api, None, false, &import_body(&entries)).await;
        let ids = imported_ids(&answer, &case);

        let stored = listed_entities(&gateway, &api, &case).await;
        assert_eq!(
            stored.len(),
            1,
            "case {case}: expected one part for the one documented entity, got {stored:?}"
        );
        // The last entry for an entity is the one that documents it: a later entry lands
        // on the part the earlier one created.
        let last = entries
            .last()
            .map(|(location, properties)| (location.clone(), properties.clone()))
            .expect("the row documents at least one entry");
        assert_eq!(
            stored,
            vec![last],
            "case {case}: expected the last entry's content map, got {stored:?}"
        );

        // The answered identifiers name the parts the import left behind: one entity, so
        // one identifier, however many entries documented it.
        let part_ids = listed_ids(&gateway, &api, "", &case).await;
        assert_eq!(
            ids, part_ids,
            "case {case}: expected the answered ids to be the API's part ids {part_ids:?}, got {ids:?}"
        );
        for id in &ids {
            let read = get_part(&gateway, &api, id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected GetDocumentationPart({id}) to answer 200, got {} with {:?}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn an_import_that_cannot_be_carried_out_in_full_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-import-atomic").await;

    // The documentation set every row below must leave exactly as it is.
    let kept = created_part(&gateway, &api).await;

    // A body past the reader's byte bound. The bound itself is an emulator resource
    // guard, not a published quota, so the row asserts only that a body well past it is
    // refused without touching the records — the padding sits beside the documentation
    // set, so the entries in it are storable.
    let oversized = {
        let mut document: Value = serde_json::from_str(&import_body(&distinct_entries(2)))
            .expect("the import body is JSON");
        document["description"] = json!("padding ".repeat(1024 * 1024 / 8));
        document.to_string()
    };
    assert!(
        oversized.len() > 1024 * 1024,
        "case an oversized body: expected a body past 1 MiB, got {} bytes",
        oversized.len()
    );

    // A body carrying more entries than the import reader accepts in one request. That
    // bound is an emulator resource guard — a small multiple of the per-API part quota —
    // and the entries here are storable and the body stays well inside the byte guard, so
    // the ENTRY count is what this row exercises.
    let too_many_entries = {
        // The smallest storable entry there is — the `API` entity type makes every other
        // location member invalid — so the body stays far inside the byte guard and it is
        // the ENTRY count this row trips.
        let entry = (json!({ "type": "API" }), json!({}));
        let entries: Vec<(Value, Value)> =
            (0..=MAX_IMPORT_ENTRIES).map(|_| entry.clone()).collect();
        import_body(&entries)
    };
    assert!(
        too_many_entries.len() < 1024 * 1024,
        "case more entries than the reader accepts: expected a body inside the byte guard, got {} bytes",
        too_many_entries.len()
    );

    // Each row is an import this service cannot carry out in full, differing along WHY:
    // the body is past the reader's bound, the body documents more entities than an API
    // may hold, or the body carries an entry that cannot be stored while the caller asked
    // to roll the import back on a warning.
    let rows = vec![
        RefusedImport {
            case: "an oversized body",
            mode: None,
            fail_on_warnings: false,
            body: oversized,
            status: 400,
            code: BAD_REQUEST,
        },
        RefusedImport {
            case: "more entries than the reader accepts",
            mode: None,
            fail_on_warnings: false,
            body: too_many_entries,
            status: 400,
            code: BAD_REQUEST,
        },
        RefusedImport {
            // One entity past the quota in a SINGLE body, with nothing surviving to
            // merge with: the quota is on the parts the API would hold, so the refusal
            // is the modelled 429 whether the entities arrive in one call or two.
            case: "a body documenting one entity more than an API may hold",
            mode: Some("overwrite"),
            fail_on_warnings: false,
            body: import_body(&distinct_entries(DOCUMENTATION_PARTS_PER_API + 1)),
            status: 429,
            code: LIMIT_EXCEEDED,
        },
        RefusedImport {
            // Beyond the sibling suite's `failonwarnings` case, this row runs in
            // `overwrite` mode: the rollback has to happen BEFORE the existing set is
            // cleared, so the kept part surviving is what this row proves.
            case: "failonwarnings with an unstorable entry, overwrite",
            mode: Some("overwrite"),
            fail_on_warnings: true,
            body: import_body(&[
                (
                    json!({ "type": "METHOD", "path": "/pets", "method": "POST" }),
                    json!({ "description": "add a pet" }),
                ),
                // `name` is not a valid field for the `API` entity type.
                (json!({ "type": "API", "name": "nope" }), json!({})),
            ]),
            status: 400,
            code: BAD_REQUEST,
        },
    ];
    for row in &rows {
        let case = row.case;
        let answer =
            import_parts(&gateway, &api, row.mode, row.fail_on_warnings, &row.body).await;
        assert_failure(&answer, row.status, row.code, case);
        let stored = listed_entities(&gateway, &api, case).await;
        assert_eq!(
            stored,
            vec![kept.clone()],
            "case {case}: expected the documentation set unchanged, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn an_import_is_accepted_at_the_per_api_quota_and_refused_one_entity_past_it() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-import-quota").await;

    // AT the published bound: accepted, with one identifier per entity.
    let case = format!("an import of {DOCUMENTATION_PARTS_PER_API} entries");
    let entries = distinct_entries(DOCUMENTATION_PARTS_PER_API);
    let answer = import_parts(&gateway, &api, None, false, &import_body(&entries)).await;
    let ids = imported_ids(&answer, &case);
    assert_eq!(
        ids.len(),
        DOCUMENTATION_PARTS_PER_API,
        "case {case}: expected one identifier per entity, got {} ids",
        ids.len()
    );
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        DOCUMENTATION_PARTS_PER_API,
        "case {case}: expected a distinct identifier per entity, got {} distinct of {}",
        distinct.len(),
        ids.len()
    );

    // ONE entity past it, merged into the full API: the modelled 429, and the entity the
    // import would have added is absent.
    let case = "one entity past the quota, merged";
    let answer = import_parts(
        &gateway,
        &api,
        Some("merge"),
        false,
        &import_body(&[(
            json!({ "type": "METHOD", "path": "/past-the-quota", "method": "GET" }),
            json!({ "description": "one too many" }),
        )]),
    )
    .await;
    assert_failure(&answer, 429, LIMIT_EXCEEDED, case);
    let listed = listed_ids(&gateway, &api, "path=/past-the-quota", case).await;
    assert!(
        listed.is_empty(),
        "case {case}: expected nothing written for the refused entity, got {listed:?}"
    );

    // The same entity re-documented, not an extra one, stays inside the quota: a merge
    // that documents nothing new is accepted on a full API.
    let case = "re-documenting an entity on a full API";
    let again = vec![(
        entries[0].0.clone(),
        json!({ "description": "the same entity, documented again" }),
    )];
    let answer = import_parts(&gateway, &api, Some("merge"), false, &import_body(&again)).await;
    let ids = imported_ids(&answer, case);
    assert_eq!(
        ids.len(),
        1,
        "case {case}: expected the one entity's identifier, got {ids:?}"
    );
    let read = get_part(&gateway, &api, &ids[0]).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        string_member(&read.body, "properties", case),
        imported_content_map("the same entity, documented again"),
        "case {case}: expected the re-imported content map, got {:?}",
        read.body["properties"]
    );

    // The quota counts the parts the API HOLDS, which is one per documented entity — not
    // the entries the body carries. A body with one more entry than the quota, all of
    // them for ONE entity, stores one part and is accepted.
    let case = format!(
        "{} entries for one entity",
        DOCUMENTATION_PARTS_PER_API + 1
    );
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-import-quota-entities").await;
    let repeated: Vec<(Value, Value)> = (0..=DOCUMENTATION_PARTS_PER_API)
        .map(|revision| {
            (
                json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                json!({ "description": format!("revision {revision}") }),
            )
        })
        .collect();
    let answer = import_parts(&gateway, &api, None, false, &import_body(&repeated)).await;
    let ids = imported_ids(&answer, &case);
    assert_eq!(
        ids.len(),
        1,
        "case {case}: expected the one documented entity's identifier, got {} ids",
        ids.len()
    );
    let stored = listed_entities(&gateway, &api, &case).await;
    assert_eq!(
        stored.len(),
        1,
        "case {case}: expected one part for the one documented entity, got {stored:?}"
    );
}

/// One documentation part created through `CreateDocumentationPart`, answered as the
/// `(location, content map)` pair the collection reader reports it as.
async fn created_part(gateway: &Gateway, api: &str) -> (Value, Value) {
    let location = json!({ "type": "RESOURCE", "path": "/pets" });
    let properties = content_map("the pets collection");
    crate::documentation_parts::created_part_id(
        gateway,
        api,
        location.clone(),
        &properties,
        "the part an atomic refusal must not disturb",
    )
    .await;
    let read: Value = serde_json::from_str(&properties).expect("the content map is JSON");
    (location, read)
}

/// The access key ids the audit case below presents.
///
/// The capture is process-wide and every suite stands its emulator up on a fresh engine,
/// so two suites mint the SAME api id; the presented identity is what makes this case's
/// records its own — and asserting the record carries the identity the request presented
/// is the point anyway, so there is more than one.
const AUDIT_AKIDS: &[&str] = &["AKIAIMPORTAUDIT00001", "AKIAIMPORTAUDIT00002"];

/// `ImportDocumentationParts` signed by `akid` rather than the harness's own identity,
/// built through [`Gateway::send`] so the credential scope is this case's.
async fn import_parts_signed_by(
    gateway: &Gateway,
    akid: &str,
    api: &str,
    mode: Option<&str>,
    body: &str,
) -> Answer {
    let uri = match mode {
        Some(mode) => format!("/restapis/{api}/documentation/parts?mode={mode}"),
        None => format!("/restapis/{api}/documentation/parts"),
    };
    let request = http::Request::builder()
        .method("PUT")
        .uri(uri)
        .header("host", "apigateway.us-east-1.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={akid}/20260101/us-east-1/apigateway/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error,
        body,
    }
}

/// One captured `tracing` event: its target and its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a case can read the
/// audit records back. Nothing is written to disk and no global subscriber is installed.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Every audit record captured for `operation` from the caller presenting `akid`,
    /// oldest first. The capture is process-wide, so a case reads back only the records
    /// its own identity signed for.
    fn records_for(&self, operation: &str, akid: &str) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| {
                event.target == AUDIT_TARGET
                    && event.field("operation") == Some(operation)
                    && event
                        .field("caller")
                        .is_some_and(|value| value.contains(akid))
            })
            .cloned()
            .collect()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // This suite asserts on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        // Only the access trail is kept: this capture is installed for the whole test
        // binary, so keeping every trace event would grow without bound.
        let target = event.metadata().target().to_string();
        if target != AUDIT_TARGET {
            return;
        }
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent { target, fields });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// The one capture of this service's audit records, installed ONCE as the PROCESS-WIDE
/// default subscriber.
///
/// It has to be global rather than thread-local (`subscriber::set_default`): `tracing`
/// decides a callsite's interest when the callsite is FIRST reached and caches it
/// GLOBALLY, so with the harness running this crate's suites in parallel, another test
/// reaching the import's `info!` first — with no subscriber installed — caches that
/// callsite as "never" and a thread-local capture sees nothing. One global capture plus
/// an interest-cache rebuild removes that ordering dependency; the case below then reads
/// back only the records naming the API IT created. Nothing is written to disk.
fn audit_capture() -> &'static Capture {
    static CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        // Every suite in this binary that needs the records goes through this one
        // function, so nothing else installs a global subscriber. If one ever does, say so
        // here: the symptom would otherwise be "missing audit records", which diagnoses
        // the wrong file.
        assert!(
            tracing::subscriber::set_global_default(capture.clone()).is_ok(),
            "another global tracing subscriber was installed first, so this suite cannot \
             read the audit records back; a suite that needs one must share this capture \
             instead of installing its own"
        );
        // Recompute interest for callsites a parallel test may already have reached — and
        // cached as "never" — before this subscriber existed.
        tracing::callsite::rebuild_interest_cache();
        capture
    })
}

#[tokio::test]
async fn an_accepted_import_leaves_one_structured_audit_record() {
    let capture = audit_capture();

    // Each outer row is a different presented identity, so the record's attribution is
    // asserted over a set of callers rather than one.
    for akid in AUDIT_AKIDS {
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-import-audit").await;

        // Each inner row is one import of that API, so the records have to name the mode
        // each call ran in and the number of parts it left behind, never a value from the
        // call before.
        let rows: &[(&str, Option<&str>, usize)] = &[
            ("the first import, default mode", None, 3),
            ("a merge of one more entity", Some("merge"), 1),
            ("an overwrite of the whole set", Some("overwrite"), 2),
        ];
        let mut expected_records = 0usize;
        for (row, mode, entities) in rows {
            let case = format!("{row}, signed by {akid}");
            let entries: Vec<(Value, Value)> = distinct_entries(*entities);
            let answer =
                import_parts_signed_by(&gateway, akid, &api, *mode, &import_body(&entries)).await;
            let ids = imported_ids(&answer, &case);
            expected_records += 1;

            // One record per completed state change, and nothing for the reads around it.
            let records = capture.records_for("ImportDocumentationParts", akid);
            assert_eq!(
                records.len(),
                expected_records,
                "case {case}: expected one audit record per import, captured {records:?}"
            );
            let record = records
                .last()
                .expect("the import emitted at least one audit record");
            assert_eq!(
                record.field("outcome"),
                Some("imported"),
                "case {case}: expected the outcome of the completed change, got {record:?}"
            );
            assert_eq!(
                record.field("documentation_parts"),
                Some(entities.to_string().as_str()),
                "case {case}: expected the record to count the {entities} entities imported, got {record:?}"
            );
            assert_eq!(
                ids.len(),
                *entities,
                "case {case}: expected one identifier per documented entity, got {ids:?}"
            );
            assert!(
                record
                    .field("caller")
                    .is_some_and(|caller| caller.contains(akid)),
                "case {case}: expected the record to attribute the call to the presented access key id, got {record:?}"
            );
            assert!(
                record
                    .field("rest_api")
                    .is_some_and(|value| value.contains(&api)),
                "case {case}: expected the record to name the API it changed, got {record:?}"
            );
            assert!(
                record
                    .field("mode")
                    .is_some_and(|value| !value.trim().is_empty()),
                "case {case}: expected the record to name the mode the import ran in, got {record:?}"
            );
            // Metadata only: an imported content map is never in the access trail.
            assert!(
                record
                    .fields
                    .values()
                    .all(|value| !value.contains("description")),
                "case {case}: expected no imported content map in the record, got {record:?}"
            );
        }
    }
}
