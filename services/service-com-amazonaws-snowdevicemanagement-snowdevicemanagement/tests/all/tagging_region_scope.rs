// SPDX-License-Identifier: CC0-1.0

//! Which records a tagging call can address, end to end over HTTP.
//!
//! The rule under test: a request reaches ONE regional endpoint, and a record of this
//! service belongs to one Region and is invisible in every other, so a tagging
//! operation resolves its `resourceArn` only inside the Region the request is signed
//! for — an ARN whose Region field is another Region names no record this endpoint
//! serves, whatever id it carries, and the call is refused (HTTP 404
//! `NotFoundException`, the tagging family's not-found class) with nothing written and
//! nothing removed. All three tagging operations resolve through the one rule, so the
//! read and both writes are asserted together.
//!
//! Rows differ along what the rule generalises over: the Region the request is signed
//! for, the Region the ARN names (the same one, another Region that holds its own
//! inventory, and a Region that holds no device at all), the resource KIND (managed
//! device, task), and the Region the addressed record actually lives in. The served
//! rows are the mirror of the rule and are part of it: exactly the combinations where
//! the signed Region, the ARN's Region and the record's own Region agree are served.
//!
//! Every case goes through the router and every expectation is read back through
//! `DescribeDevice` / `DescribeTask` in the record's own Region, so an answer that is
//! not computed from persisted state cannot pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// This service's ARN namespace / sigv4 signing name, from
/// `smithy_query(query="service_info")`.
const ARN_SERVICE: &str = "snow-device-management";

/// The canonical account id every emulator ARN carries.
const ACCOUNT: &str = "000000000000";

/// One Region this account has an activated inventory in.
const REGION_A: &str = "us-west-2";

/// Another Region with an inventory of its own, so a cross-Region ARN is refused
/// because of the Region it names and not because the other Region is empty.
const REGION_B: &str = "eu-central-1";

/// A Region this account has activated NO device in. The tagging operations are not
/// gated by the device precondition (`device_region_gate.rs`), so an ARN of this Region
/// still reaches the existence check — and finds nothing.
const DEVICELESS_REGION: &str = "ap-northeast-1";

/// Every Region a row signs for, and every Region a row's ARN may name.
const REGIONS: &[&str] = &[REGION_A, REGION_B, DEVICELESS_REGION];

/// How this service words a `resourceArn` that parses but addresses no device and no
/// task it serves. Spelled out here rather than imported, so wording that drifts cannot
/// make this suite pass.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The tags every record carries before a row runs, so a refused write has something to
/// leave alone and a refused removal something to leave behind.
const BASELINE_KEY: &str = "env";
const BASELINE_VALUE: &str = "prod";

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`) with any `namespace#` prefix dropped, or the empty
    /// string on a success. Compared WHOLE by the assertions, because a client branches
    /// on the code and `ResourceNotFoundException` ends in `NotFoundException`.
    fn error_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    /// The `message` an error body carries, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("snow-device-management.{region}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/\
                 snow-device-management/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Percent-encode one URI path segment the way an SDK does: an ARN carries `:` and `/`,
/// and `/tags/{resourceArn}` is a single (non-greedy) label.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

/// An ARN of this service, built field by field so a row can vary exactly the Region.
fn arn(region: &str, resource: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{region}:{ACCOUNT}:{resource}")
}

/// A JSON `TagMap` from pairs.
fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

async fn list_tags(router: &MultiServiceRouter, signed: &str, resource_arn: &str) -> Answer {
    let uri = format!("/tags/{}", encode_label(resource_arn));
    call(router, signed, "GET", &uri, None).await
}

async fn tag_resource(
    router: &MultiServiceRouter,
    signed: &str,
    resource_arn: &str,
    tags: Value,
) -> Answer {
    let uri = format!("/tags/{}", encode_label(resource_arn));
    call(router, signed, "POST", &uri, Some(json!({ "tags": tags }))).await
}

async fn untag_resource(
    router: &MultiServiceRouter,
    signed: &str,
    resource_arn: &str,
    keys: &[&str],
) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("/tags/{}?{query}", encode_label(resource_arn));
    call(router, signed, "DELETE", &uri, None).await
}

/// Assert a call succeeded and hand back its body.
fn ok(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body.clone()
}

/// A required string member of a JSON object.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The `tags` member of a report, as a comparable map.
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let Some(tags) = body.get("tags") else {
        return BTreeMap::new();
    };
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a string tag value, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// Which kind of record a row addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Device,
    Task,
}

impl Kind {
    /// The `resourceType` segment this kind's ARN carries.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Device => "managed-device",
            Self::Task => "task",
        }
    }
}

/// One record this account really holds: its kind, its id and the Region it lives in.
struct Record {
    kind: Kind,
    id: String,
    home: &'static str,
}

impl Record {
    /// This record addressed through an ARN naming `region`.
    fn arn_in(&self, region: &str) -> String {
        arn(region, &format!("{}/{}", self.kind.resource_type(), self.id))
    }
}

/// The record's own report of its tags, read in the Region it lives in: the independent
/// witness that a write landed on it, or that a refusal left it alone.
async fn stored_tags(
    router: &MultiServiceRouter,
    record: &Record,
    case: &str,
) -> BTreeMap<String, String> {
    let answer = match record.kind {
        Kind::Device => {
            call(
                router,
                record.home,
                "POST",
                &format!("/managed-device/{}/describe", record.id),
                None,
            )
            .await
        }
        Kind::Task => {
            call(
                router,
                record.home,
                "POST",
                &format!("/task/{}", record.id),
                None,
            )
            .await
        }
    };
    tags_of(&ok(&answer, case), case)
}

/// One emulator with an activated inventory in [`REGION_A`] and [`REGION_B`], one task
/// created over the wire in each, and every one of those four records carrying the
/// baseline tag.
async fn emulator() -> (MultiServiceRouter, Vec<Record>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
    let mut devices: Vec<(&'static str, String)> = Vec::new();
    for region in [REGION_A, REGION_B] {
        let activated = state.lock().activate_devices(region, 2, SystemTime::now());
        let device = activated
            .first()
            .expect("the activation seam puts devices in the Region")
            .clone();
        devices.push((region, device));
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let mut records = Vec::new();
    for (region, device) in devices {
        let case = format!("the fixture of {region}");
        let created = ok(
            &call(
                &router,
                region,
                "POST",
                "/task",
                Some(json!({ "targets": [device.clone()], "command": { "reboot": {} } })),
            )
            .await,
            &case,
        );
        records.push(Record {
            kind: Kind::Device,
            id: device,
            home: region,
        });
        records.push(Record {
            kind: Kind::Task,
            id: string(&created, "taskId", &case),
            home: region,
        });
    }

    // Each record starts from the same baseline, written through its OWN Region, so
    // every row below has something to leave intact.
    for record in &records {
        let case = format!("seeding {:?} {}", record.kind, record.id);
        ok(
            &tag_resource(
                &router,
                record.home,
                &record.arn_in(record.home),
                tag_map(&[(BASELINE_KEY, BASELINE_VALUE)]),
            )
            .await,
            &case,
        );
    }
    (router, records)
}

fn baseline() -> BTreeMap<String, String> {
    BTreeMap::from([(BASELINE_KEY.to_string(), BASELINE_VALUE.to_string())])
}

/// Assert a tagging call was refused as the family's not-found class, with the one class
/// message and nothing of the ARN echoed back.
fn assert_not_found(answer: &Answer, case: &str, resource_arn: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404 for an ARN naming no record of the signed \
         Region, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {case:?}: expected the tagging family's NotFoundException, got {}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        NOT_FOUND_MESSAGE,
        "case {case:?}: expected the one class message for this refusal, got {}",
        answer.body
    );
    assert!(
        !answer.body.to_string().contains(resource_arn),
        "case {case:?}: the refusal must not echo {resource_arn:?}, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_tagging_call_addresses_only_the_records_of_the_region_it_is_signed_for() {
    let (router, records) = emulator().await;

    // Every (record, signed Region, ARN Region) combination: served only where all
    // three Regions agree, refused everywhere else.
    let mut refusal_messages: BTreeSet<String> = BTreeSet::new();
    for (index, record) in records.iter().enumerate() {
        for signed in REGIONS {
            for arn_region in REGIONS {
                let case = format!(
                    "{:?} {} of {} addressed through an ARN naming {arn_region}, signed for \
                     {signed}",
                    record.kind, record.id, record.home
                );
                let resource_arn = record.arn_in(arn_region);
                let served = *signed == *arn_region && *arn_region == record.home;
                // A value unique to the row, so a tag left behind by another row cannot
                // satisfy these assertions.
                let probe_key = format!("probe-{index}-{signed}-{arn_region}");

                let read = list_tags(&router, signed, &resource_arn).await;
                let write = tag_resource(
                    &router,
                    signed,
                    &resource_arn,
                    tag_map(&[(probe_key.as_str(), "written")]),
                )
                .await;

                if served {
                    assert_eq!(
                        tags_of(&ok(&read, &case), &case),
                        baseline(),
                        "case {case:?}: the tag read must report the record's stored tags"
                    );
                    ok(&write, &case);
                    let mut expected = baseline();
                    expected.insert(probe_key.clone(), "written".to_string());
                    let after_write = stored_tags(&router, record, &case).await;
                    assert_eq!(
                        after_write, expected,
                        "case {case:?}: the record's own report must carry the written tag, \
                         got {after_write:?}"
                    );
                    // Removing the probe again restores the baseline, so the next row
                    // starts where this one did — and the removal is shown to act on the
                    // same record the write did.
                    ok(
                        &untag_resource(&router, signed, &resource_arn, &[probe_key.as_str()]).await,
                        &case,
                    );
                    let after_removal = stored_tags(&router, record, &case).await;
                    assert_eq!(
                        after_removal,
                        baseline(),
                        "case {case:?}: the removal must have taken only the probe off, got \
                         {after_removal:?}"
                    );
                    continue;
                }

                // Refused: the read discloses nothing, the write persists nothing, and a
                // removal naming a key the record really carries removes nothing.
                assert_not_found(&read, &case, &resource_arn);
                assert_not_found(&write, &case, &resource_arn);
                let removal = untag_resource(&router, signed, &resource_arn, &[BASELINE_KEY]).await;
                assert_not_found(&removal, &case, &resource_arn);
                for answer in [&read, &write, &removal] {
                    refusal_messages.insert(answer.message().to_string());
                }

                let kept = stored_tags(&router, record, &case).await;
                assert_eq!(
                    kept,
                    baseline(),
                    "case {case:?}: a refused tagging call must leave the record exactly as \
                     it was, got {kept:?}"
                );
            }
        }
    }

    // One sentence for the whole class: the rows carry distinct ARNs, Regions and
    // resource kinds, so a single message across all of them is also proof that it is
    // fitted to none of them.
    assert_eq!(
        refusal_messages.len(),
        1,
        "case \"every refused combination\": one condition must get one sentence, got \
         {refusal_messages:?}"
    );
}

#[tokio::test]
async fn a_cross_region_arn_cannot_write_a_tag_another_region_then_reports() {
    // The consequence the rule exists to prevent, stated as its own invariant: a tag
    // written through an ARN naming a Region the record does not live in must not show
    // up in that record's own Region. Rows differ in the resource kind and in which of
    // the two Regions is the foreign one, so neither Region is special.
    let (router, records) = emulator().await;

    for record in &records {
        let foreign = if record.home == REGION_A {
            REGION_B
        } else {
            REGION_A
        };
        for signed in [record.home, foreign, DEVICELESS_REGION] {
            let case = format!(
                "{:?} {} of {} written through an ARN naming {foreign}, signed for {signed}",
                record.kind, record.id, record.home
            );
            let resource_arn = record.arn_in(foreign);
            let write = tag_resource(
                &router,
                signed,
                &resource_arn,
                tag_map(&[("cross-region", "written")]),
            )
            .await;
            assert_not_found(&write, &case, &resource_arn);

            let reported = stored_tags(&router, record, &case).await;
            assert_eq!(
                reported,
                baseline(),
                "case {case:?}: a write through a foreign-Region ARN must not be \
                 observable in the record's own Region, got {reported:?}"
            );
            let read_back = list_tags(&router, record.home, &record.arn_in(record.home)).await;
            assert_eq!(
                tags_of(&ok(&read_back, &case), &case),
                baseline(),
                "case {case:?}: the record's own tag read must agree with its report"
            );
        }
    }
}
