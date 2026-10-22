// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the CloudWatch Synthetics group slice: `CreateGroup` and the
//! read that proves what it persisted, `GetGroup`.
//!
//! The rule asserted here, one row per input class rather than one recorded request:
//! `CreateGroup` persists a group under the requested name with the tags supplied, mints
//! its ID (lowercase alphanumeric, as `Group$Arn`'s pattern requires of the ARN's
//! resource segment) independently of the request, builds its ARN from that ID and the
//! Region the call was signed for, stamps both timestamps with the create instant, and
//! answers the stored record; the same record comes back from `GetGroup` addressed by
//! name, by ID or by ARN; a second create of a live name is refused with the modelled
//! `ConflictException`; a create past the documented account quota is refused with the
//! modelled `ServiceQuotaExceededException`.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite already defines.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::canary_lifecycle::{Answer, Synthetics};

/// `CreateGroup`: "You can have as many as 20 groups in your account."
/// (`smithy_query(query="doc",shape="CreateGroup")`).
const MAX_GROUPS: usize = 20;

/// `TagMap` is modelled `length 1..50` — "You can associate as many as 50 tags with a
/// group" (`smithy_query(query="shape",shape="TagMap")`).
const MAX_TAGS: usize = 50;

/// `GroupName` is modelled `length 1..64`
/// (`smithy_query(query="shape",shape="GroupName")`).
const MAX_GROUP_NAME_LEN: usize = 64;

/// The fixed sentence a group lookup that misses answers with, by this service's
/// missing-resource rule (see `tests/all/missing_resource_errors.rs`).
const GROUP_NOT_FOUND: &str = "Group not found.";

/// Whether `value` is a legal `Group$Id`.
///
/// `Group$Arn` is modelled
/// `^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:group:[0-9a-z]+$`
/// (`smithy_query(query="shape",shape="Group")`), and the ID is that resource segment, so
/// an ID is a non-empty run of lowercase letters and digits.
fn is_group_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// One `CreateGroup` input class.
struct Row {
    case: &'static str,
    name: String,
    /// The `Tags` map sent, or `None` when the member is omitted.
    tags: Option<Value>,
    /// Whether this name can be addressed directly in `GET /group/{GroupIdentifier}`:
    /// a group name "can include any Unicode characters", which a raw URI path cannot.
    /// Such a row is read back by ID and by ARN instead.
    name_in_uri: bool,
}

/// `MAX_TAGS` tags, the most a group may carry.
///
/// `TagKey` is `length 1..128, pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` and `TagValue` is
/// `length 0..256` (`smithy_query(query="shape",shape="TagMap")`), so these keys are legal
/// and the generated values stay inside the bound.
fn max_tags() -> Value {
    let mut map = Map::new();
    for index in 0..MAX_TAGS {
        map.insert(format!("key{index}"), json!(format!("value{index}")));
    }
    Value::Object(map)
}

/// The input classes the rule generalises over: name charset and length, and tag count,
/// key form and value form.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ascii-hyphenated-untagged",
            name: "prod-canaries".to_string(),
            tags: None,
            name_in_uri: true,
        },
        Row {
            case: "single-character-one-tag",
            name: "g".to_string(),
            tags: Some(json!({ "env": "prod" })),
            name_in_uri: true,
        },
        Row {
            case: "digits-only-three-tags",
            name: "2026".to_string(),
            tags: Some(json!({
                "env": "staging",
                "cost-center": "4711",
                "owner/service": "synthetics",
            })),
            name_in_uri: true,
        },
        Row {
            // The modelled maximum name length.
            case: "max-length-name",
            name: "g".repeat(MAX_GROUP_NAME_LEN),
            tags: None,
            name_in_uri: true,
        },
        Row {
            // `TagValue` is `length 0..256`: an empty value is legal and must round-trip.
            case: "empty-tag-value",
            name: "empty_value-tag".to_string(),
            tags: Some(json!({ "a.b_c:d": "" })),
            name_in_uri: true,
        },
        Row {
            // The modelled maximum tag count.
            case: "max-tags",
            name: "fifty-tagged".to_string(),
            tags: Some(max_tags()),
            name_in_uri: true,
        },
        Row {
            // "The name for the group. It can include any Unicode characters."
            // (`smithy_query(query="doc",shape="CreateGroupRequest",member="Name")`).
            case: "unicode-name",
            name: "grupo-café-群組".to_string(),
            tags: Some(json!({ "team": "observabilité" })),
            name_in_uri: false,
        },
        Row {
            case: "spaces-and-parentheses-name",
            name: "My Group (east)".to_string(),
            tags: None,
            name_in_uri: false,
        },
    ]
}

/// A `CreateGroup` body for `row`.
fn create_group_body(row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("Name".to_string(), json!(row.name));
    if let Some(tags) = row.tags.clone() {
        body.insert("Tags".to_string(), tags);
    }
    Value::Object(body)
}

/// Assert a 200, naming the case on failure.
fn expect_ok(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
}

/// Every input class is persisted and reported from the store, and comes back unchanged
/// from `GetGroup` addressed by name, by ID and by ARN.
#[tokio::test]
async fn create_group_persists_every_input_class_and_get_group_reads_it_back() {
    let region = "eu-west-2";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for row in rows() {
        let case = row.case;
        let created = aws.call("POST", "/group", Some(create_group_body(&row))).await;
        expect_ok(case, &created);

        let group = created.at("Group").clone();
        assert_eq!(
            group["Name"].as_str(),
            Some(row.name.as_str()),
            "case {case}: expected the requested name, got {:?}",
            group["Name"]
        );

        let id = created.string_at("Group.Id");
        assert!(
            is_group_id(&id),
            "case {case}: expected a lowercase-alphanumeric group id, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected a fresh group id, got the already-minted {id:?}"
        );

        let arn = created.string_at("Group.Arn");
        assert_eq!(
            arn,
            format!("arn:aws:synthetics:{region}:{account}:group:{id}"),
            "case {case}: expected an ARN built from the signed Region and the group id, \
             got {arn:?}"
        );

        // Both timestamps are the create instant: nothing has modified the group yet.
        let created_time = group["CreatedTime"].as_f64();
        let last_modified = group["LastModifiedTime"].as_f64();
        assert!(
            created_time.is_some_and(|seconds| seconds > 0.0),
            "case {case}: expected a CreatedTime past the epoch, got {:?}",
            group["CreatedTime"]
        );
        assert_eq!(
            created_time, last_modified,
            "case {case}: expected LastModifiedTime to equal CreatedTime on a fresh group, \
             got {created_time:?} and {last_modified:?}"
        );

        // Tags round-trip exactly, and an untagged group reports no map at all
        // (`TagMap` is modelled `length 1..50`, so an empty map is not a legal value).
        match row.tags.as_ref() {
            Some(tags) => assert_eq!(
                &group["Tags"], tags,
                "case {case}: expected the supplied tags, got {:?}",
                group["Tags"]
            ),
            None => assert!(
                group["Tags"].is_null(),
                "case {case}: expected no Tags on an untagged group, got {:?}",
                group["Tags"]
            ),
        }

        // "You can specify the group name, the ARN, or the group ID as the
        // GroupIdentifier": every form resolves to the same stored record, and the
        // record the create reported is the record that was persisted.
        let mut identifiers = vec![("by-id", id.clone()), ("by-arn", arn.clone())];
        if row.name_in_uri {
            identifiers.push(("by-name", row.name.clone()));
        }
        for (form, identifier) in identifiers {
            let read = aws
                .call("GET", &format!("/group/{identifier}"), None)
                .await;
            expect_ok(&format!("{case}/{form}"), &read);
            assert_eq!(
                read.at("Group"),
                &group,
                "case {case}/{form}: expected GetGroup to report the created group, got {:?}",
                read.at("Group")
            );
        }

        // A read changes nothing a later caller sees: the second read is byte-identical.
        let reread = aws.call("GET", &format!("/group/{id}"), None).await;
        assert_eq!(
            reread.at("Group"),
            &group,
            "case {case}/reread: expected an unchanged group, got {:?}",
            reread.at("Group")
        );
    }
}

/// The ID is minted, not derived from the request: the same name created in two separate
/// stores gets two different IDs, and so do two different names in one store.
#[tokio::test]
async fn group_ids_are_minted_not_derived_from_the_request() {
    let name = "same-name-everywhere";
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, region) in [("first-store", "us-east-1"), ("second-store", "us-east-1")] {
        let aws = Synthetics::new(region);
        let created = aws
            .call("POST", "/group", Some(json!({ "Name": name })))
            .await;
        expect_ok(case, &created);
        let id = created.string_at("Group.Id");
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected an id that does not repeat across stores, got {id:?}"
        );
    }
    assert_eq!(
        ids.len(),
        2,
        "case identical-names: expected two distinct ids for the same name, got {ids:?}"
    );
}

/// A second create of a live name is refused, and the group that holds the name is
/// untouched by the refusal.
#[tokio::test]
async fn a_duplicate_group_name_is_refused_and_leaves_the_live_group_alone() {
    let aws = Synthetics::new("us-west-2");
    for row in rows() {
        let case = row.case;
        let first = aws.call("POST", "/group", Some(create_group_body(&row))).await;
        expect_ok(case, &first);
        let group = first.at("Group").clone();

        // The retry differs only in its tags, so what is refused is the NAME and not the
        // rest of the request.
        let retry_body = json!({ "Name": row.name, "Tags": { "retry": "yes" } });
        let duplicate = aws.call("POST", "/group", Some(retry_body)).await;
        assert_eq!(
            duplicate.status, 409,
            "case {case}/duplicate: expected 409, got {} ({:?})",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {case}/duplicate: expected ConflictException, got {:?}",
            duplicate.error_code
        );

        let id = first.string_at("Group.Id");
        let read = aws.call("GET", &format!("/group/{id}"), None).await;
        expect_ok(&format!("{case}/survives"), &read);
        assert_eq!(
            read.at("Group"),
            &group,
            "case {case}/survives: expected the first group unchanged by the refused \
             duplicate, got {:?}",
            read.at("Group")
        );
    }
}

/// A `GroupIdentifier` the store does not hold is answered with the fixed, identifier-free
/// missing-group sentence — whatever form the identifier takes.
#[tokio::test]
async fn a_missing_group_answers_one_fixed_identifier_free_message() {
    let aws = Synthetics::new("ap-southeast-2");
    let live = aws
        .call("POST", "/group", Some(json!({ "Name": "live-group" })))
        .await;
    expect_ok("create-live", &live);
    let live_id = live.string_at("Group.Id");

    // Every row is a legal `GroupIdentifier` (`length 1..128`) no store holds: a name
    // form, an id form and an ARN form, including near misses of the live group.
    let identifiers = [
        ("short-name", "x".to_string()),
        ("name-form", "no-such-group".to_string()),
        ("suffix-of-live-name", "live-group-2".to_string()),
        ("id-form", "0123456789ab".to_string()),
        (
            "arn-form",
            format!(
                "arn:aws:synthetics:ap-southeast-2:{}:group:0123456789ab",
                emulator_server::emulator_account_id()
            ),
        ),
        (
            // An ARN carrying the LIVE group's id but another service: the partition,
            // service, Region and account are part of a resource's identity, so this is
            // not that group's ARN and must not resolve to it.
            "arn-of-another-service",
            format!(
                "arn:aws:iam:ap-southeast-2:{}:group:{live_id}",
                emulator_server::emulator_account_id()
            ),
        ),
        (
            // The same, in another account.
            "arn-of-another-account",
            format!("arn:aws:synthetics:ap-southeast-2:123456789012:group:{live_id}"),
        ),
        (
            // The same, in another Region: a group's ARN carries the Region it was
            // created in.
            "arn-of-another-region",
            format!(
                "arn:aws:synthetics:eu-west-1:{}:group:{live_id}",
                emulator_server::emulator_account_id()
            ),
        ),
        ("max-length-identifier", "z".repeat(128)),
    ];
    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (case, identifier) in identifiers {
        let answer = aws
            .call("GET", &format!("/group/{identifier}"), None)
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        let message = answer
            .at("Message")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body))
            .to_string();
        assert!(
            !message.to_lowercase().contains(&identifier.to_lowercase()),
            "case {case}: expected a Message that does not name the requested group \
             {identifier:?}, got {message:?}"
        );
        assert_eq!(
            message, GROUP_NOT_FOUND,
            "case {case}: expected the fixed missing-group sentence, got {message:?}"
        );
        messages.insert(message);
    }
    assert_eq!(
        messages.len(),
        1,
        "case request-independence: expected one fixed message across every identifier \
         form, got {messages:?}"
    );

    // The misses resolved to nothing: the live group is still there.
    let read = aws.call("GET", &format!("/group/{live_id}"), None).await;
    expect_ok("live-group-survives", &read);
}

/// The account quota bounds the store: the groups up to it are accepted, the next one is
/// refused with the modelled quota error, and the refusal does not disturb what is stored.
#[tokio::test]
async fn a_create_past_the_account_quota_is_refused() {
    let aws = Synthetics::new("us-east-1");
    let mut ids = Vec::new();
    for index in 0..MAX_GROUPS {
        let case = format!("group-{index}");
        let created = aws
            .call(
                "POST",
                "/group",
                Some(json!({ "Name": format!("quota-group-{index}") })),
            )
            .await;
        expect_ok(&case, &created);
        ids.push(created.string_at("Group.Id"));
    }

    for (case, name) in [
        ("past-quota", "quota-group-overflow".to_string()),
        ("past-quota-again", "quota-group-overflow-2".to_string()),
    ] {
        let refused = aws
            .call("POST", "/group", Some(json!({ "Name": name })))
            .await;
        assert_eq!(
            refused.status, 402,
            "case {case}: expected 402, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: expected ServiceQuotaExceededException, got {:?}",
            refused.error_code
        );
        let read = aws
            .call("GET", &format!("/group/{name}"), None)
            .await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the refused group not to be stored, got {} ({:?})",
            read.status, read.body
        );
    }

    // Everything accepted before the quota is still readable.
    for (index, id) in ids.iter().enumerate() {
        let read = aws.call("GET", &format!("/group/{id}"), None).await;
        expect_ok(&format!("group-{index}-survives"), &read);
        assert_eq!(
            read.string_at("Group.Name"),
            format!("quota-group-{index}"),
            "case group-{index}-survives: expected the stored group, got {:?}",
            read.at("Group")
        );
    }
}

// ---------------------------------------------------------------------------
// CreateGroup leaves one structured, attributable record per call
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than configuring
/// `tracing-subscriber`: what this asserts is one line per event carrying its target and
/// fields. The same shape of harness is used by
/// `services/service-com-amazonaws-accessanalyzer-accessanalyzer/tests/all/modeled_rejections.rs`;
/// it stays per-crate because an integration test target cannot import another crate's
/// test module.
#[derive(Clone, Default)]
struct CapturedRecords(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in parallel
/// on other threads — so a thread-local subscriber is raced away as soon as another thread
/// re-evaluates a handler's callsite with no subscriber of its own.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The target `state::AUDIT_TARGET` names, written here as a literal so the assertions
/// pin the wire-visible target rather than whatever the constant happens to hold.
const AUDIT_TARGET: &str = "emulator::audit";

/// The captured `emulator::audit` records this suite's `CreateGroup` calls wrote.
///
/// Sibling tests in this binary create groups in parallel, so the filter is the Region
/// this test alone signs for ([`AUDIT_REGION`]) on top of the operation: that is what lets
/// each assertion demand EXACTLY one record per call.
fn create_group_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with(&format!("{AUDIT_TARGET} "))
                && record.contains("operation=CreateGroup")
                && record.contains(&format!("region=\"{AUDIT_REGION}\""))
        })
        .cloned()
        .collect()
}

/// The Region this test signs for: its own, so its records are identifiable among those a
/// sibling test writes in parallel.
const AUDIT_REGION: &str = "ca-central-1";

/// RULE: every `CreateGroup` call leaves exactly ONE structured record on the audit
/// target, naming the operation, the identity the request presented, the Region it
/// resolved to and the outcome — and nothing the caller supplied.
///
/// A table over the outcomes the operation can report.
#[tokio::test]
async fn every_create_group_call_emits_one_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(
        target: AUDIT_TARGET,
        operation = "CreateGroup",
        region = ?AUDIT_REGION,
        "probe",
    );
    let probe = create_group_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case subscriber-self-test: the capture must receive events, got {probe:?}"
    );

    let region = AUDIT_REGION;
    let aws = Synthetics::new(region);
    // The group name and the tag value below must appear in NO record: an audit record
    // carries metadata only.
    let name = "audited-group";
    let tag_value = "audited-tag-value";
    let create = json!({ "Name": name, "Tags": { "env": tag_value } });

    for (case, body, outcome, status) in [
        ("created", create.clone(), "created", 200),
        (
            "duplicate-name",
            create.clone(),
            "rejected_name_conflict",
            409,
        ),
    ] {
        let answer = aws.call("POST", "/group", Some(body)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} ({:?})",
            answer.status, answer.body
        );
        let records = create_group_records(&captured.drain());
        assert_eq!(
            records.len(),
            1,
            "case {case}: expected exactly one audit record for the call, got {records:?}"
        );
        let record = &records[0];
        for expected in [
            "operation=CreateGroup".to_string(),
            // The identity the harness signs with, as the credential scope presents it.
            "caller=\"AKIDEXAMPLE\"".to_string(),
            format!("region=\"{region}\""),
            format!("outcome={outcome}"),
        ] {
            assert!(
                record.contains(&expected),
                "case {case}: expected the audit record to carry {expected:?}, got {record:?}"
            );
        }
        for supplied in [name, tag_value] {
            assert!(
                !record.contains(supplied),
                "case {case}: expected no caller-supplied value in the audit record, \
                 found {supplied:?} in {record:?}"
            );
        }
    }

    // The third outcome: fill the account quota, then the refused create.
    for index in 1..MAX_GROUPS {
        let filler = aws
            .call(
                "POST",
                "/group",
                Some(json!({ "Name": format!("audited-filler-{index}") })),
            )
            .await;
        expect_ok(&format!("quota-filler-{index}"), &filler);
        captured.drain();
    }
    let refused = aws
        .call("POST", "/group", Some(json!({ "Name": "audited-overflow" })))
        .await;
    assert_eq!(
        refused.status, 402,
        "case quota-exceeded: expected 402, got {} ({:?})",
        refused.status, refused.body
    );
    let records = create_group_records(&captured.drain());
    assert_eq!(
        records.len(),
        1,
        "case quota-exceeded: expected exactly one audit record for the call, got {records:?}"
    );
    assert!(
        records[0].contains("outcome=rejected_quota_exceeded"),
        "case quota-exceeded: expected the refusal's outcome in the audit record, got {:?}",
        records[0]
    );
}
