// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **data access policy** create/read path, end to end over the
//! wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client. The harness
//! is the one `collection_lifecycle` uses; nothing is rebuilt here.
//!
//! The rule under test: `CreateAccessPolicy` persists the access policy under its
//! `(type, name)` pair with a server-minted `policyVersion` and the policy document stored
//! PARSED, and answers from that stored record — so `GetAccessPolicy` reports exactly what
//! the create reported, a second create for a pair already stored is the conflict, and a
//! replayed `clientToken` answers the record the first call created. The tables span what
//! the rule generalises over — the document's JSON form (object, array, nested, scalars of
//! every type, non-ASCII text, whitespace in the request text), the `PolicyName` length
//! bounds, a described and an undescribed policy, and the whole `AccessPolicyType` value
//! list — not the one request the issue quoted.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text};

/// The values `AccessPolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="AccessPolicyType")` -> `values`): the single value
/// `data`. Every case below is driven over this whole list.
const ACCESS_POLICY_TYPES: [&str; 1] = ["data"];

/// The account's data-access-policy quota, as `crate::state::MAX_ACCESS_POLICIES` records
/// it from AWS's "Quotas for Amazon OpenSearch Serverless": 100 per account.
const MAX_ACCESS_POLICIES: usize = 100;

/// The deepest JSON nesting the emulator converts into a policy document, as
/// `crate::state::MAX_DOCUMENT_DEPTH` records it: an emulator resource guard, so the
/// conversion's recursion is bounded by a constant rather than by the request.
const MAX_DOCUMENT_DEPTH: usize = 64;

/// Now, in the epoch milliseconds `createdDate` / `lastModifiedDate` are modelled with.
///
/// The test keeps its own clock on purpose: the window a stamp is asserted inside has to
/// come from outside the implementation that produced the stamp.
fn now_epoch_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_millis(),
    )
    .expect("epoch milliseconds fit in an i64")
}

/// One create case: the policy name, the `policy` TEXT the request carries, and the
/// description it carries (or none).
struct Case {
    what: &'static str,
    name: String,
    policy: String,
    description: Option<String>,
}

/// The create table: rows differ in the JSON form of the document, the member types inside
/// it, the whitespace of the request text, the `PolicyName` length and whether a
/// description is carried.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "an object document with a nested rule list",
            name: "data-read-only".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}"#.to_string(),
            description: Some("read-only access to the demo indexes".to_string()),
        },
        Case {
            what: "a bare array document, and no description",
            // `PolicyName` is @length(min: 3, max: 32): this is the minimum.
            name: "abc".to_string(),
            policy: r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/demo"],"Permission":["aoss:*"]}],"Principal":["arn:aws:iam::000000000000:role/Admin"]}]"#.to_string(),
            description: None,
        },
        Case {
            what: "a name at PolicyName's 32-character maximum",
            name: format!("a{}", "b".repeat(31)),
            policy: r#"{"Rules":[]}"#.to_string(),
            description: Some(String::new()),
        },
        Case {
            what: "scalars of every JSON type, including non-ASCII text",
            name: "mixed-scalars".to_string(),
            // `é` is inside `PolicyDocument`'s own character class
            // (`[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]+`).
            policy: r#"{"enabled":true,"count":3,"ratio":1.5,"negative":-7,"nothing":null,"note":"café"}"#.to_string(),
            description: Some("café au lait, and a number: 3".to_string()),
        },
        Case {
            what: "a request text carrying tabs, newlines and spaces",
            name: "whitespace-text".to_string(),
            policy: "{\n\t\"Rules\" : [\n\t\t{ \"ResourceType\" : \"index\" }\n\t]\n}".to_string(),
            description: Some("the document is reported without the whitespace".to_string()),
        },
        Case {
            what: "a deeply nested document and a description at PolicyDescription's 1000-character maximum",
            name: "deeply-nested".to_string(),
            policy: r#"{"a":{"b":{"c":{"d":[{"e":[[1,2],[3,4]]}]}}}}"#.to_string(),
            description: Some("d".repeat(1000)),
        },
        Case {
            what: "a one-character document, PolicyDocument's minimum length",
            name: "tiny-document".to_string(),
            policy: "7".to_string(),
            description: None,
        },
    ]
}

#[tokio::test]
async fn the_access_policy_create_and_read_are_served_from_state() {
    let router = emulator();
    let mut versions: BTreeSet<String> = BTreeSet::new();

    for kind in ACCESS_POLICY_TYPES {
        for case in cases() {
            let label = format!("CreateAccessPolicy({kind}/{}, {})", case.name, case.what);
            let mut request = json!({ "type": kind, "name": case.name, "policy": case.policy });
            if let Some(description) = &case.description {
                request["description"] = json!(description);
            }

            let before = now_epoch_millis();
            let (status, code, answer) = call(&router, "CreateAccessPolicy", request).await;
            let after = now_epoch_millis();
            assert_eq!(
                status, 200,
                "case {label}: expected 200, got {status} {code} with {answer}"
            );

            let detail = at(&answer, "accessPolicyDetail").clone();
            assert_eq!(
                (text(&detail, "type"), text(&detail, "name")),
                (kind.to_string(), case.name.clone()),
                "case {label}: the detail names the stored policy, got {detail}"
            );

            let version = text(&detail, "policyVersion");
            assert!(
                is_policy_version(&version),
                "case {label}: policyVersion must match PolicyVersion's length and pattern, got {version:?}"
            );
            assert!(
                versions.insert(version.clone()),
                "case {label}: every policy write mints a fresh policyVersion, got a repeat of {version:?}"
            );

            // The document is reported PARSED — `AccessPolicyDetail$policy` is "the JSON
            // policy document without any whitespaces" — so it equals the JSON the request
            // text carried, whatever whitespace that text had.
            let expected: Value =
                serde_json::from_str(&case.policy).expect("the case policy is JSON");
            assert_eq!(
                at(&detail, "policy"),
                &expected,
                "case {label}: the policy document is the parsed request document, got {detail}"
            );

            assert_eq!(
                text(&detail, "description"),
                case.description.clone().unwrap_or_default(),
                "case {label}: the stored description is reported, got {detail}"
            );

            // A just-created policy was last modified when it was created, and both stamps
            // are the epoch milliseconds of the call that stored it.
            let created = at(&detail, "createdDate").as_i64().unwrap_or_default();
            let modified = at(&detail, "lastModifiedDate").as_i64().unwrap_or_default();
            assert_eq!(
                created, modified,
                "case {label}: a just-created policy's lastModifiedDate is its createdDate, got {detail}"
            );
            assert!(
                (before..=after).contains(&created),
                "case {label}: createdDate must be the epoch millis of the create, expected {before}..={after}, got {created}"
            );

            // The read-back reports exactly what the create reported, down to the version
            // and the timestamps: both answer the one stored record.
            let (status, code, read) = call(
                &router,
                "GetAccessPolicy",
                json!({ "type": kind, "name": case.name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case GetAccessPolicy({kind}/{}): expected 200, got {status} {code} with {read}",
                case.name
            );
            assert_eq!(
                at(&read, "accessPolicyDetail"),
                &detail,
                "case GetAccessPolicy({kind}/{}): the read must answer the created record, got {read}",
                case.name
            );
        }

        // Every row is still readable after the whole table ran, so a create never
        // overwrote a record it did not name.
        for case in cases() {
            let (status, code, read) = call(
                &router,
                "GetAccessPolicy",
                json!({ "type": kind, "name": case.name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case GetAccessPolicy({kind}/{}, after the table): expected 200, got {status} {code} with {read}",
                case.name
            );
        }

        // And a pair that was never created is the 404 the model declares for the read.
        let (status, code, answer) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": kind, "name": "never-created" }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case GetAccessPolicy({kind}/never-created): expected 404 ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }
}

#[tokio::test]
async fn create_access_policy_refuses_a_duplicate_pair_and_an_invalid_input() {
    let router = emulator();
    let valid = r#"{"Rules":[]}"#;

    let (status, code, first) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "demo-access", "policy": r#"{"Rules":[{"ResourceType":"index"}]}"# }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(demo-access): expected 200, got {status} {code} with {first}"
    );
    let stored = at(&first, "accessPolicyDetail").clone();

    // A second create for a pair already stored is the conflict the model declares...
    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "demo-access", "policy": valid }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateAccessPolicy(duplicate pair): expected 409 ConflictException, got {status} {code} with {answer}"
    );

    // ... and it leaves the stored policy exactly as it was.
    let (status, code, read) = call(
        &router,
        "GetAccessPolicy",
        json!({ "type": "data", "name": "demo-access" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetAccessPolicy(demo-access, after the conflict): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "accessPolicyDetail"),
        &stored,
        "case GetAccessPolicy(demo-access, after the conflict): a refused create must not rewrite the record, got {read}"
    );

    // The refusals a create owes: `type` outside `AccessPolicyType`'s value list, a
    // `policy` that is not a JSON document, and each modelled bound, on the bound itself
    // and one past it.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "type outside AccessPolicyType",
            json!({ "type": "encryption", "name": "bad-policy", "policy": valid }),
        ),
        (
            "type in the wrong case",
            json!({ "type": "Data", "name": "bad-policy", "policy": valid }),
        ),
        (
            "policy that is not JSON",
            json!({ "type": "data", "name": "bad-policy", "policy": "not json at all" }),
        ),
        (
            "name one short of the 3-character minimum",
            json!({ "type": "data", "name": "ab", "policy": valid }),
        ),
        (
            "name one past the 32-character maximum",
            json!({ "type": "data", "name": format!("a{}", "b".repeat(32)), "policy": valid }),
        ),
        (
            "name outside PolicyName's ^[a-z][a-z0-9-]+$",
            json!({ "type": "data", "name": "Bad_Name", "policy": valid }),
        ),
        (
            "policy one past the 20480-character maximum",
            json!({ "type": "data", "name": "bad-policy", "policy": format!(r#"{{"a":"{}"}}"#, "x".repeat(20473)) }),
        ),
        (
            "description one past the 1000-character maximum",
            json!({ "type": "data", "name": "bad-policy", "policy": valid, "description": "d".repeat(1001) }),
        ),
        (
            "clientToken below the 1-character minimum",
            json!({ "type": "data", "name": "bad-policy", "policy": valid, "clientToken": "" }),
        ),
        (
            "policy absent",
            json!({ "type": "data", "name": "bad-policy" }),
        ),
        (
            "name absent",
            json!({ "type": "data", "policy": valid }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "CreateAccessPolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case CreateAccessPolicy({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // A refused create persists nothing: the name every refusal above used is still absent.
    let (status, code, answer) = call(
        &router,
        "GetAccessPolicy",
        json!({ "type": "data", "name": "bad-policy" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case GetAccessPolicy(bad-policy): a refused create must persist nothing, got {status} {code} with {answer}"
    );

    // The bound itself is accepted, on both sides of each range the refusals probed: a
    // 20480-character document and a 1000-character description.
    let at_bounds = json!({
        "type": "data",
        "name": "bound-policy",
        "policy": format!(r#"{{"a":"{}"}}"#, "x".repeat(20472)),
        "description": "d".repeat(1000),
    });
    let policy_len = at_bounds["policy"].as_str().unwrap_or_default().len();
    assert_eq!(
        policy_len, 20480,
        "case CreateAccessPolicy(policy at the 20480-character maximum): the case itself must sit on the bound, got {policy_len}"
    );
    let (status, code, answer) = call(&router, "CreateAccessPolicy", at_bounds).await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(policy and description on their maxima): expected 200, got {status} {code} with {answer}"
    );
}

#[tokio::test]
async fn a_replayed_client_token_answers_the_access_policy_it_first_created() {
    let router = emulator();
    let request = json!({
        "type": "data",
        "name": "token-policy",
        "policy": r#"{"Rules":[{"ResourceType":"index"}]}"#,
        "clientToken": "a-caller-supplied-token",
    });

    let (status, code, first) = call(&router, "CreateAccessPolicy", request.clone()).await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(token-policy): expected 200, got {status} {code} with {first}"
    );
    let stored = at(&first, "accessPolicyDetail").clone();

    // The same token again is a RETRY of that call: the same record, version and stamps —
    // not a second policy and not a conflict.
    for (case, body) in [
        ("the identical retry", request.clone()),
        (
            "a retry whose body differs",
            json!({
                "type": "data",
                "name": "token-policy",
                "policy": r#"{"Rules":[]}"#,
                "description": "a different body under the same token",
                "clientToken": "a-caller-supplied-token",
            }),
        ),
    ] {
        let (status, code, answer) = call(&router, "CreateAccessPolicy", body).await;
        assert_eq!(
            status, 200,
            "case CreateAccessPolicy({case}): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "accessPolicyDetail"),
            &stored,
            "case CreateAccessPolicy({case}): a replayed clientToken answers the first record, got {answer}"
        );
    }

    // A DIFFERENT token for the same pair is a new request for a name already taken.
    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({
            "type": "data",
            "name": "token-policy",
            "policy": r#"{"Rules":[]}"#,
            "clientToken": "another-token",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateAccessPolicy(a fresh token for a stored pair): expected 409 ConflictException, got {status} {code} with {answer}"
    );

    // The token index is keyed by operation as well, so the same token spent on another
    // create is an independent resource rather than a replay of this one.
    let (status, code, answer) = call(
        &router,
        "CreateSecurityPolicy",
        json!({
            "type": "network",
            "name": "token-policy",
            "policy": r#"{"Rules":[],"AllowFromPublic":true}"#,
            "clientToken": "a-caller-supplied-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy(the access policy's token): a token is per-operation, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(at(&answer, "securityPolicyDetail"), "type"),
        "network",
        "case CreateSecurityPolicy(the access policy's token): the answer must be the security policy it created, got {answer}"
    );
}

#[tokio::test]
async fn create_access_policy_refuses_the_request_past_the_account_quota() {
    let router = emulator();

    for index in 0..MAX_ACCESS_POLICIES {
        let name = format!("quota-policy-{index}");
        let (status, code, answer) = call(
            &router,
            "CreateAccessPolicy",
            json!({ "type": "data", "name": name, "policy": r#"{"Rules":[]}"# }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateAccessPolicy({name}): the first {MAX_ACCESS_POLICIES} fit the quota, got {status} {code} with {answer}"
        );
    }

    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "one-policy-too-many", "policy": r#"{"Rules":[]}"# }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case CreateAccessPolicy(one past the {MAX_ACCESS_POLICIES}-policy quota): expected 402 ServiceQuotaExceededException, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(&answer, "resourceType"),
        "access-policy",
        "case CreateAccessPolicy(one past the quota): the quota error names the resource type, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The nesting bound on the policy document: the conversion into the modelled
// `Document` recurses once per level over caller-supplied JSON, so the depth it
// accepts is a constant of the emulator, not a function of the request.
// ---------------------------------------------------------------------------

/// A JSON array nested `depth` levels deep: `[[[ … ]]]`.
fn nested_array(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

#[tokio::test]
async fn create_access_policy_bounds_the_nesting_of_the_policy_document() {
    let router = emulator();

    // The bound itself is accepted and round-trips through the reader.
    let name = format!("nested-{MAX_DOCUMENT_DEPTH}");
    let policy = nested_array(MAX_DOCUMENT_DEPTH);
    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": name, "policy": policy }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(a document nested {MAX_DOCUMENT_DEPTH} deep): expected 200, got {status} {code} with {answer}"
    );
    let detail = at(&answer, "accessPolicyDetail").clone();
    let expected: Value = serde_json::from_str(&policy).expect("the nested case is JSON");
    assert_eq!(
        at(&detail, "policy"),
        &expected,
        "case CreateAccessPolicy(a document nested {MAX_DOCUMENT_DEPTH} deep): the document is converted whole, got {detail}"
    );
    let (status, code, read) = call(
        &router,
        "GetAccessPolicy",
        json!({ "type": "data", "name": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetAccessPolicy({name}): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "accessPolicyDetail"),
        &detail,
        "case GetAccessPolicy({name}): the read answers the created record, got {read}"
    );

    // One past the bound, far past it, and past the JSON parser's own nesting limit are
    // all refused with the modelled 400 — and none of them aborts the emulator, which is
    // what the bound exists for: the next call still answers.
    for depth in [MAX_DOCUMENT_DEPTH + 1, 127, 1_000, 5_000] {
        let (status, code, answer) = call(
            &router,
            "CreateAccessPolicy",
            json!({ "type": "data", "name": format!("nested-{depth}"), "policy": nested_array(depth) }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case CreateAccessPolicy(a document nested {depth} deep): expected 400 ValidationException, got {status} {code} with {answer}"
        );
        let (status, code, answer) = call(
            &router,
            "GetAccessPolicy",
            json!({ "type": "data", "name": format!("nested-{depth}") }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case GetAccessPolicy(nested-{depth}): a refused document persists nothing, got {status} {code} with {answer}"
        );
    }

    // The emulator is still serving after every one of those: the record created at the
    // bound reads back unchanged.
    let (status, code, read) = call(
        &router,
        "GetAccessPolicy",
        json!({ "type": "data", "name": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetAccessPolicy({name}, after the refusals): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "accessPolicyDetail"),
        &detail,
        "case GetAccessPolicy({name}, after the refusals): the store is untouched, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The audit record every access-policy call emits
// ---------------------------------------------------------------------------

/// The name prefix this test's own policies share, so its audit records are told apart
/// from those the sibling tests in this binary write in parallel.
const AUDITED: &str = "audited-";

/// The access key id every call in this binary is signed with (`collection_lifecycle`'s
/// `authorization` builds `Credential=AKIDTEST/...`), which is what
/// `emulator_server::extract_akid` reports in an audit record.
const AUDIT_ACCESS_KEY_ID: &str = "AKIDTEST";

/// Records captured from `tracing`, oldest first.
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
/// re-evaluates a handler's callsite with no subscriber of its own. The records of a
/// sibling are told apart by the resource each test addresses.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The captured records on the `emulator::audit` target that THIS test's calls wrote.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| record.starts_with("emulator::audit ") && record.contains(AUDITED))
        .cloned()
        .collect()
}

/// RULE: every access-policy call — state-changing or reporting, accepted or refused —
/// leaves exactly ONE structured record on the `emulator::audit` target naming the service,
/// the operation, the identity the request presented, the request target, the resource it
/// was about and the outcome, and carries none of the caller's own data.
#[tokio::test]
async fn every_access_policy_call_emits_one_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", resource = "access-policy/data/audited-probe", "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    let secret_document = r#"{"Rules":[{"ResourceType":"index","Note":"do-not-log-this"}]}"#;

    // (case, operation, request, expected status, expected outcome token)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a policy was created",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "audited-policy", "policy": secret_document, "description": "do-not-log-this-either" }),
            200,
            "policy-created",
        ),
        (
            "the pair was already taken",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "audited-policy", "policy": r#"{"Rules":[]}"# }),
            409,
            "already-exists",
        ),
        (
            "the document is not JSON",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "audited-refused", "policy": "not json at all" }),
            400,
            "validation-refused",
        ),
        (
            "the stored policy was read",
            "GetAccessPolicy",
            json!({ "type": "data", "name": "audited-policy" }),
            200,
            "policy-read",
        ),
        (
            "the pair names no policy",
            "GetAccessPolicy",
            json!({ "type": "data", "name": "audited-missing" }),
            404,
            "not-found",
        ),
        (
            "a clientToken was replayed",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "audited-token", "policy": r#"{"Rules":[]}"#, "clientToken": "audited-token-value" }),
            200,
            "policy-created",
        ),
        (
            "the same clientToken again",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "audited-token", "policy": r#"{"Rules":[]}"#, "clientToken": "audited-token-value" }),
            200,
            "idempotent-replay",
        ),
    ];

    for (case, operation, request, expected_status, outcome) in rows {
        let kind = text(&request, "type");
        let name = text(&request, "name");
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected {expected_status}, got {status} {code} with {answer}"
        );

        let records = audit_records(&captured.drain());
        assert_eq!(
            records.len(),
            1,
            "case {case}: exactly one audit record per call, got {records:?}"
        );
        let record = &records[0];
        for expected in [
            "service=aoss".to_string(),
            format!("operation={operation}"),
            format!("resource=access-policy/{kind}/{name}"),
            format!("outcome={outcome}"),
            "request_target=POST /".to_string(),
            // The identity the request presented: the harness signs with this access key
            // id, so the field's CONTENT is checked, not merely its presence.
            format!("access_key_id={AUDIT_ACCESS_KEY_ID}"),
        ] {
            assert!(
                record.contains(&expected),
                "case {case}: the audit record must carry {expected:?}, got {record:?}"
            );
        }
        // Metadata only: neither the caller's policy document nor its description nor the
        // request signature reaches the log.
        for forbidden in ["do-not-log-this", "deadbeef"] {
            assert!(
                !record.contains(forbidden),
                "case {case}: the audit record must not carry {forbidden:?}, got {record:?}"
            );
        }
    }
}
