// SPDX-License-Identifier: CC0-1.0

//! The precondition the enablement reads share, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and message bodies — not the
//! handlers' Rust signatures.
//!
//! The rule under test: AWS Control Tower serves its enabled-resource reads for an account
//! it has been SET UP in — setting it up is what creates the landing zone and the service
//! role those APIs act through — so in an account it has provisioned nothing in each such
//! read is refused with the precondition error of its resource family, whatever the request
//! carries, instead of reporting out of the empty store. The enabled-BASELINE reads refuse
//! with `ValidationException` (400) naming the role; the enabled-CONTROL reads and the
//! control-operation read refuse with `ResourceNotFoundException` (404) naming the call that
//! creates the landing zone.
//!
//! Three sides of it are asserted, each over a TABLE whose rows differ in what the rule
//! generalises over — which read is called, and what the request carries (no member at all,
//! each published `maxResults` bound, a filter, a page token, and identifiers spanning the
//! forms their member admits, including ones sitting exactly on its `@length` bounds):
//!
//! * in an un-set-up account every one of those reads is its family's refusal, in AWS's
//!   wording, reporting no records and no details;
//! * what decides it is the ACCOUNT's state, not the request: once the account is set up —
//!   and still once the landing zone it was set up with has been decommissioned, which
//!   leaves the operation log behind — the very same requests are answered out of the store
//!   again (an empty listing, and a by-identifier miss naming the identifier);
//! * the refusals are reporting-only, so a run of them provisions nothing: the account is
//!   still un-set-up afterwards, and a landing zone created after them is the account's
//!   first.
//!
//! The reads that are NOT under the precondition are pinned here too, so the rule cannot
//! widen past the operations whose refusal AWS attests: `ListControlOperations`,
//! `ListLandingZones` and the `ListBaselines` catalog all answer an un-set-up account.
//!
//! Every status, error code and bound written out by hand names its source:
//! `smithy_query(query="operation", operation="ListEnabledBaselines,ListEnabledControls,
//! GetEnabledBaseline,GetEnabledControl,GetControlOperation,ListControlOperations")` on this
//! crate's `smithy-model/` — `ListEnabledBaselines` models `ValidationException` (400) and
//! no `ResourceNotFoundException` at all, the other four model both, `maxResults` ranges
//! 5..100 (enabled baselines) and 1..200 (enabled controls), `Arn` is `@length(20..2048)`
//! with `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$` and `OperationIdentifier` is `@length(36)`
//! with `@pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`. The
//! emulator, the harness and the `arn_of_length` helper are the sibling suites'.

use crate::front_door_validation::arn_of_length;
use crate::landing_zone_lifecycle::{
    call, emulator, past_the_settle_span, set_up_account, Answer,
};

/// What AWS Control Tower answers a baseline-enablement call with when it cannot assume the
/// service role setting it up creates. Fixed text: the role is the fixed service role of
/// that name. Shared with `tests/all/enablement_write_precondition.rs`, which asserts the
/// same wording on the WRITES of that family.
pub(crate) const BASELINE_REFUSAL: &str =
    "AWS Control Tower could not complete the operation because it could not assume the \
     'AWSControlTowerAdmin' role. Check the configuration for this role and try again.";

/// What it answers a control-enablement or control-operation call with when the account has
/// no landing zone. Fixed text, naming the call that creates one. Shared with the write
/// suite for the same reason.
pub(crate) const CONTROL_REFUSAL: &str =
    "AWS Control Tower cannot complete the operation, because you must create a landing \
     zone first. To continue, create your landing zone from the console, or call the \
     CreateLandingZone API.";

/// One read under the precondition: its path, the output member that would carry what it
/// reports, and the refusal its resource family answers with.
#[derive(Clone, Copy)]
struct Read {
    path: &'static str,
    /// The member a served answer reports, so a case can assert a refusal reports none.
    reports: &'static str,
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// The enabled-baseline reads: `ValidationException` (400) naming the role.
const LIST_ENABLED_BASELINES: Read = Read {
    path: "/list-enabled-baselines",
    reports: "enabledBaselines",
    status: 400,
    code: "ValidationException",
    message: BASELINE_REFUSAL,
};
const GET_ENABLED_BASELINE: Read = Read {
    path: "/get-enabled-baseline",
    reports: "enabledBaselineDetails",
    status: 400,
    code: "ValidationException",
    message: BASELINE_REFUSAL,
};

/// The enabled-control and control-operation reads: `ResourceNotFoundException` (404)
/// naming the landing zone.
const LIST_ENABLED_CONTROLS: Read = Read {
    path: "/list-enabled-controls",
    reports: "enabledControls",
    status: 404,
    code: "ResourceNotFoundException",
    message: CONTROL_REFUSAL,
};
const GET_ENABLED_CONTROL: Read = Read {
    path: "/get-enabled-control",
    reports: "enabledControlDetails",
    status: 404,
    code: "ResourceNotFoundException",
    message: CONTROL_REFUSAL,
};
const GET_CONTROL_OPERATION: Read = Read {
    path: "/get-control-operation",
    reports: "controlOperation",
    status: 404,
    code: "ResourceNotFoundException",
    message: CONTROL_REFUSAL,
};

/// Well-formed `Arn` values spanning the forms the member admits, none of which this
/// service minted anything for: the resource kind the read is about, another kind, another
/// service's resource, and a value sitting exactly on each published `@length` bound.
fn arns(resource: &str) -> Vec<(String, serde_json::Value)> {
    vec![
        (
            format!("an {resource} ARN nothing minted"),
            serde_json::json!(format!(
                "arn:aws:controltower:us-west-2:000000000000:{resource}/AAAAAAAAAAAAAAAA"
            )),
        ),
        (
            "a landing zone ARN, another resource kind of this service".to_string(),
            serde_json::json!(
                "arn:aws:controltower:eu-west-1:000000000000:landingzone/BBBBBBBBBBBBBBBB"
            ),
        ),
        (
            "an ARN of another AWS service".to_string(),
            serde_json::json!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"),
        ),
        (
            "an ARN exactly at the length min of 20".to_string(),
            serde_json::json!(arn_of_length(20)),
        ),
        (
            "an ARN exactly at the length max of 2048".to_string(),
            serde_json::json!(arn_of_length(2048)),
        ),
    ]
}

/// The request bodies one read is driven with: what the rule generalises over for THAT
/// read, since the refusal is the account's and not the request's.
fn requests(read: Read) -> Vec<(String, serde_json::Value)> {
    match read.path {
        "/list-enabled-baselines" => vec![
            ("no member at all".to_string(), serde_json::json!({})),
            (
                "maxResults exactly its range min of 5".to_string(),
                serde_json::json!({ "maxResults": 5 }),
            ),
            (
                "maxResults exactly its range max of 100".to_string(),
                serde_json::json!({ "maxResults": 100 }),
            ),
            (
                "a filter on a target nothing is enabled on".to_string(),
                serde_json::json!({
                    "filter": {
                        "targetIdentifiers":
                            ["arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"]
                    }
                }),
            ),
            (
                "includeChildren and a page token at once".to_string(),
                serde_json::json!({ "includeChildren": true, "nextToken": "0123456789abcdef.0" }),
            ),
        ],
        "/list-enabled-controls" => vec![
            ("no member at all".to_string(), serde_json::json!({})),
            (
                "maxResults exactly its range min of 1".to_string(),
                serde_json::json!({ "maxResults": 1 }),
            ),
            (
                "maxResults exactly its range max of 200".to_string(),
                serde_json::json!({ "maxResults": 200 }),
            ),
            (
                "a targetIdentifier nothing is enabled on".to_string(),
                serde_json::json!({
                    "targetIdentifier":
                        "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"
                }),
            ),
            (
                "a filter and a page token at once".to_string(),
                serde_json::json!({
                    "filter": { "statuses": ["SUCCEEDED"] },
                    "nextToken": "0123456789abcdef.0"
                }),
            ),
        ],
        "/get-enabled-baseline" => arns("enabledbaseline")
            .into_iter()
            .map(|(case, value)| {
                (
                    case,
                    serde_json::json!({ "enabledBaselineIdentifier": value }),
                )
            })
            .collect(),
        "/get-enabled-control" => arns("enabledcontrol")
            .into_iter()
            .map(|(case, value)| {
                (
                    case,
                    serde_json::json!({ "enabledControlIdentifier": value }),
                )
            })
            .collect(),
        // `OperationIdentifier` is fixed at 36 characters of lowercase hex and dashes, so
        // the rows are the values of that one form: the all-zero identifier, an all-`f` one,
        // and one of mixed digits — none of them minted by any call.
        "/get-control-operation" => [
            ("the all-zero identifier", "00000000-0000-0000-0000-000000000000"),
            ("an all-f identifier", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
            ("an identifier no call minted", "deadbeef-dead-beef-dead-beefdeadbeef"),
        ]
        .into_iter()
        .map(|(case, identifier)| {
            (
                case.to_string(),
                serde_json::json!({ "operationIdentifier": identifier }),
            )
        })
        .collect(),
        other => panic!("no request table for {other}"),
    }
}

/// Every read the precondition covers.
fn reads() -> Vec<Read> {
    vec![
        LIST_ENABLED_BASELINES,
        LIST_ENABLED_CONTROLS,
        GET_ENABLED_BASELINE,
        GET_ENABLED_CONTROL,
        GET_CONTROL_OPERATION,
    ]
}

/// Assert `answer` is `read`'s precondition refusal: its family's status and modelled error
/// code, AWS's wording for it, and nothing reported.
fn assert_refused(case: &str, read: Read, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (read.status, Some(read.code)),
        "case {case:?} on {}: expected {} {}, got {:?} body {:?}",
        read.path,
        read.status,
        read.code,
        (answer.status, answer.error_code.as_deref()),
        answer.body
    );
    assert_eq!(
        answer.string("message"),
        read.message,
        "case {case:?} on {}: expected the precondition wording {:?}, got {:?}",
        read.path,
        read.message,
        answer.string("message")
    );
    assert!(
        answer
            .body
            .get(read.reports)
            .is_none_or(serde_json::Value::is_null),
        "case {case:?} on {}: a refusal must report nothing, got {:?}",
        read.path,
        answer.body
    );
}

/// RULE: in an account AWS Control Tower has provisioned nothing in, every enabled-resource
/// read is its resource family's precondition refusal — whatever the request carries —
/// rather than a report out of the empty store.
#[tokio::test]
async fn an_unset_up_account_refuses_every_enablement_read() {
    let router = emulator();

    for read in reads() {
        for (case, body) in requests(read) {
            let answer = call(&router, read.path, body.clone()).await;
            assert_refused(&format!("{case} ({body})"), read, &answer);
        }
    }
}

/// RULE (what decides it): the precondition is the ACCOUNT's setup, not the request. Once
/// the account has been set up the very same requests are answered out of the store again —
/// and still are once that landing zone has been decommissioned, because the account has
/// been set up either way.
#[tokio::test]
async fn a_set_up_account_answers_the_same_requests_out_of_its_store() {
    for (state, decommission) in [
        ("the account's landing zone deployed", false),
        ("that landing zone since decommissioned", true),
    ] {
        let router = emulator();
        let arn = set_up_account(&router).await;
        if decommission {
            // Nothing sleeps server-side: the wait is the test's, so the teardown the clock
            // owes has been applied before the reads below.
            tokio::time::sleep(past_the_settle_span()).await;
            let deleted = call(
                &router,
                "/delete-landingzone",
                serde_json::json!({ "landingZoneIdentifier": arn }),
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "state {state:?}: delete-landingzone expected 200, got {} body {:?}",
                deleted.status, deleted.body
            );
            tokio::time::sleep(past_the_settle_span()).await;
        }

        for read in reads() {
            for (case, body) in requests(read) {
                let answer = call(&router, read.path, body.clone()).await;
                let case = format!("{state}: {case} ({body})");

                // A page token this listing never handed out is still refused by the
                // listing's own rule — which is a refusal about the REQUEST, so it carries
                // neither the precondition's status nor its wording.
                let token_sent = body.get("nextToken").is_some();
                if read.path.starts_with("/list-") {
                    let expected = if token_sent { 400 } else { 200 };
                    assert_eq!(
                        answer.status, expected,
                        "case {case:?} on {}: expected {expected}, got {} body {:?}",
                        read.path, answer.status, answer.body
                    );
                    if !token_sent {
                        assert_eq!(
                            answer.get(read.reports),
                            &serde_json::json!([]),
                            "case {case:?} on {}: a set-up account that has enabled nothing \
                             must report an empty listing, got {:?}",
                            read.path,
                            answer.body
                        );
                    }
                } else {
                    // A by-identifier read of a set-up account reports the IDENTIFIER as
                    // what it could not find, so it is the modelled 404 for every resource
                    // kind — and its message is about that identifier, not the landing zone.
                    assert_eq!(
                        (answer.status, answer.error_code.as_deref()),
                        (404, Some("ResourceNotFoundException")),
                        "case {case:?} on {}: expected 404 ResourceNotFoundException, got \
                         {:?} body {:?}",
                        read.path,
                        (answer.status, answer.error_code.as_deref()),
                        answer.body
                    );
                }

                assert_ne!(
                    answer.string("message"),
                    read.message,
                    "case {case:?} on {}: a set-up account must not answer the precondition \
                     refusal, got {:?}",
                    read.path,
                    answer.body
                );
            }
        }
    }
}

/// RULE (the reporting side of it): a refused read provisions nothing, so a run of them
/// leaves the account exactly as un-set-up as it was — every read still refuses, and the
/// landing zone created afterwards is the account's first.
#[tokio::test]
async fn a_refused_read_provisions_nothing() {
    let router = emulator();

    for round in 0..2 {
        for read in reads() {
            for (case, body) in requests(read) {
                let answer = call(&router, read.path, body.clone()).await;
                assert_refused(&format!("round {round}: {case} ({body})"), read, &answer);
            }
        }
    }

    // The account is still un-set-up, so the create is accepted as its FIRST landing zone
    // (a second one would be refused: an account holds one), and the reads answer from then
    // on.
    set_up_account(&router).await;
    for read in reads().into_iter().filter(|read| read.path.starts_with("/list-")) {
        let listed = call(&router, read.path, serde_json::json!({})).await;
        assert_eq!(
            (listed.status, listed.get(read.reports)),
            (200, &serde_json::json!([])),
            "{}: expected an empty listing once the account is set up, got {} body {:?}",
            read.path,
            listed.status,
            listed.body
        );
    }
}

/// RULE (its boundary): the precondition covers the enabled-resource reads, not every read
/// of this service — the control-operation LISTING, the landing zone listing and the
/// AWS-managed baseline catalog all answer an account nothing has been provisioned in.
#[tokio::test]
async fn the_reads_outside_the_precondition_answer_an_unset_up_account() {
    let router = emulator();

    for (path, member, empty) in [
        ("/list-control-operations", "controlOperations", true),
        ("/list-landingzones", "landingZones", true),
        ("/list-baselines", "baselines", false),
    ] {
        let answer = call(&router, path, serde_json::json!({})).await;
        assert_eq!(
            answer.status, 200,
            "{path}: expected 200 before the account is set up, got {} body {:?}",
            answer.status, answer.body
        );
        let reported = answer
            .get(member)
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            reported == 0,
            empty,
            "{path}: expected {} records, got {reported} in {:?}",
            if empty { "no" } else { "some" },
            answer.body
        );
    }
}
