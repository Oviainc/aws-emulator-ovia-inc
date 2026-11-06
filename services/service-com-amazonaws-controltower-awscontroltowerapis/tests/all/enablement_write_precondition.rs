// SPDX-License-Identifier: CC0-1.0

//! The precondition the enablement WRITES share, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and message bodies — not the
//! handlers' Rust signatures.
//!
//! The rule under test: AWS Control Tower acts on a control or baseline enablement only for
//! an account it has been SET UP in — setting it up is what creates the landing zone the
//! control enablements are deployed from and the service role the baseline enablements act
//! through — so in an account it has provisioned nothing in every call that enables,
//! updates, re-deploys or disables one is refused with the precondition error of its
//! resource family, before the request's identifiers are resolved and before anything is
//! written. The control-enablement writes refuse with `ResourceNotFoundException` (404)
//! naming the call that creates the landing zone; the baseline-enablement writes refuse with
//! `ValidationException` (400) naming the role. It is the same precondition the enablement
//! READS carry (`tests/all/landing_zone_precondition.rs`), whose wording these cases reuse
//! rather than restate.
//!
//! Three sides of it are asserted, each over a TABLE whose rows differ in what the rule
//! generalises over — which write is called, and what the request carries (no member at all,
//! each way a request may name an enablement, identifiers spanning the forms their member
//! admits including ones at its `@length` bounds, a catalogued control and a catalogued
//! baseline at each version it offers, with and without the optional members):
//!
//! * in an un-set-up account every one of those writes is its family's refusal, in AWS's
//!   wording, minting neither an ARN nor an operation — including the request that names no
//!   enablement at all and the one whose identifier is no reference this service could
//!   resolve, so the refusal is ordered AHEAD of the complaints about the request itself;
//! * what decides it is the ACCOUNT's state, not the request: once the account is set up the
//!   very same requests are answered out of the store again, and none of them answers the
//!   precondition wording;
//! * the refusals write nothing: after a run of them the account is still un-set-up, so the
//!   landing zone created afterwards is its first, and the enablement listings that landing
//!   zone makes answerable are empty.
//!
//! Every status, error code and bound written out by hand names its source:
//! `smithy_query(query="operation", operation="EnableControl,DisableControl,
//! UpdateEnabledControl,ResetEnabledControl,EnableBaseline,DisableBaseline,
//! UpdateEnabledBaseline,ResetEnabledBaseline")` on this crate's `smithy-model/` — all eight
//! model `ResourceNotFoundException` (404) and `ValidationException` (400); no member of
//! `DisableControl` is `@required` while `UpdateEnabledControl` requires `parameters`,
//! `EnableBaseline` requires `baselineIdentifier`, `baselineVersion` and `targetIdentifier`
//! and `UpdateEnabledBaseline` requires `baselineVersion`; `Arn` is `@length(20..2048)` with
//! `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`. The catalogued controls and baselines come from
//! `service_..._awscontroltowerapis::state::{CONTROL_CATALOG, BASELINE_CATALOG}` — the
//! catalogs the two enable handlers resolve their identifiers against — so no row depends on
//! one catalog entry. The emulator, the harness, the `arn_of_length` helper and the two
//! precondition wordings are the sibling suites'.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    BASELINE_CATALOG, CONTROL_CATALOG,
};

use crate::enabled_control_lifecycle::{catalog_arn, control, ou};
use crate::front_door_validation::arn_of_length;
use crate::landing_zone_precondition::{BASELINE_REFUSAL, CONTROL_REFUSAL};
use crate::landing_zone_lifecycle::{call, emulator, set_up_account, Answer};

/// Which resource family a write belongs to, which is what decides the refusal it carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    /// The control enablements: the landing zone is what is reported missing, 404.
    Control,
    /// The baseline enablements: the service role is what could not be assumed, 400.
    Baseline,
}

impl Family {
    fn status(self) -> u16 {
        match self {
            Family::Control => 404,
            Family::Baseline => 400,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Family::Control => "ResourceNotFoundException",
            Family::Baseline => "ValidationException",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Family::Control => CONTROL_REFUSAL,
            Family::Baseline => BASELINE_REFUSAL,
        }
    }
}

/// One write under the precondition.
#[derive(Clone, Copy)]
struct Write {
    path: &'static str,
    family: Family,
}

/// Every call that enables, updates, re-deploys or disables an enablement.
const WRITES: &[Write] = &[
    Write {
        path: "/enable-control",
        family: Family::Control,
    },
    Write {
        path: "/disable-control",
        family: Family::Control,
    },
    Write {
        path: "/update-enabled-control",
        family: Family::Control,
    },
    Write {
        path: "/reset-enabled-control",
        family: Family::Control,
    },
    Write {
        path: "/enable-baseline",
        family: Family::Baseline,
    },
    Write {
        path: "/disable-baseline",
        family: Family::Baseline,
    },
    Write {
        path: "/update-enabled-baseline",
        family: Family::Baseline,
    },
    Write {
        path: "/reset-enabled-baseline",
        family: Family::Baseline,
    },
];

/// The enablable controls of the catalog, which is what `EnableControl` resolves a
/// `controlIdentifier` against.
fn enablable_controls() -> Vec<&'static str> {
    CONTROL_CATALOG
        .iter()
        .filter(|entry| entry.enablable)
        .map(|entry| entry.name)
        .collect()
}

/// An `Arn` of the enablement kind `resource` names, of the form this service mints: well
/// formed, and minted by nothing.
fn unminted(resource: &str, id: &str) -> String {
    format!("arn:aws:controltower:us-west-2:000000000000:{resource}/{id}")
}

/// The enabled-control ARNs a row may name: the form this service mints, another region's,
/// and the two `@length` bounds of `Arn` — none of them minted by any call.
fn enabled_control_identifiers() -> Vec<(String, String)> {
    vec![
        (
            "an enabledcontrol ARN nothing minted".to_string(),
            unminted("enabledcontrol", "AAAAAAAAAAAAAAAA"),
        ),
        (
            "an enabledcontrol ARN of another region".to_string(),
            "arn:aws:controltower:eu-west-1:000000000000:enabledcontrol/BBBBBBBBBBBBBBBB"
                .to_string(),
        ),
        (
            "an ARN exactly at the length min of 20".to_string(),
            arn_of_length(20),
        ),
        (
            "an ARN exactly at the length max of 2048".to_string(),
            arn_of_length(2048),
        ),
    ]
}

/// The enabled-baseline ARNs a row may name, the same four forms one kind down.
fn enabled_baseline_identifiers() -> Vec<(String, String)> {
    vec![
        (
            "an enabledbaseline ARN nothing minted".to_string(),
            unminted("enabledbaseline", "CCCCCCCCCCCCCCCC"),
        ),
        (
            "an enabledbaseline ARN of another region".to_string(),
            "arn:aws:controltower:ap-south-1:000000000000:enabledbaseline/DDDDDDDDDDDDDDDD"
                .to_string(),
        ),
        (
            "an ARN exactly at the length min of 20".to_string(),
            arn_of_length(20),
        ),
        (
            "an ARN exactly at the length max of 2048".to_string(),
            arn_of_length(2048),
        ),
    ]
}

/// The request bodies one write is driven with: what the rule generalises over for THAT
/// write, since the refusal is the account's and not the request's. Each row names its own
/// target, so in a set-up account the same table is a run of independent calls rather than
/// repeats of one.
fn requests(write: Write) -> Vec<(String, serde_json::Value)> {
    match write.path {
        // Every enablable catalog control, under both published ARN forms, with and without
        // the optional members.
        "/enable-control" => enablable_controls()
            .into_iter()
            .enumerate()
            .flat_map(|(index, name)| {
                let catalogued = CONTROL_CATALOG
                    .iter()
                    .find(|entry| entry.name == name)
                    .expect("the name came from the catalog");
                vec![
                    (
                        format!("{name} by its control ARN"),
                        serde_json::json!({
                            "controlIdentifier": control(name),
                            "targetIdentifier": ou(&format!("ec{index}0")),
                        }),
                    ),
                    (
                        format!("{name} by its control catalog ARN, with tags"),
                        serde_json::json!({
                            "controlIdentifier": catalog_arn(catalogued.catalog_id),
                            "targetIdentifier": ou(&format!("ec{index}1")),
                            "tags": { "owner": "platform" },
                        }),
                    ),
                ]
            })
            .collect(),
        // No member of this input is required, so the rows span every way a request may name
        // an enablement — including naming none at all, and naming one with a value that is
        // no reference this service could resolve.
        "/disable-control" => {
            let mut rows = vec![
                ("no member at all".to_string(), serde_json::json!({})),
                (
                    "the control/target pair".to_string(),
                    serde_json::json!({
                        "controlIdentifier": control(enablable_controls()[0]),
                        "targetIdentifier": ou("dc0"),
                    }),
                ),
                (
                    "a controlIdentifier that is no control reference".to_string(),
                    serde_json::json!({
                        "controlIdentifier": arn_of_length(20),
                        "targetIdentifier": ou("dc1"),
                    }),
                ),
                (
                    "a targetIdentifier that is no organizational unit".to_string(),
                    serde_json::json!({
                        "controlIdentifier": control(enablable_controls()[0]),
                        "targetIdentifier": unminted("enabledcontrol", "EEEEEEEEEEEEEEEE"),
                    }),
                ),
            ];
            rows.extend(enabled_control_identifiers().into_iter().map(
                |(case, identifier)| {
                    (
                        format!("enabledControlIdentifier: {case}"),
                        serde_json::json!({ "enabledControlIdentifier": identifier }),
                    )
                },
            ));
            rows.push((
                "all three members".to_string(),
                serde_json::json!({
                    "enabledControlIdentifier": unminted("enabledcontrol", "FFFFFFFFFFFFFFFF"),
                    "controlIdentifier": control(enablable_controls()[0]),
                    "targetIdentifier": ou("dc2"),
                }),
            ));
            rows
        }
        "/update-enabled-control" => enabled_control_identifiers()
            .into_iter()
            .map(|(case, identifier)| {
                (
                    case,
                    serde_json::json!({
                        "enabledControlIdentifier": identifier,
                        "parameters": [{ "key": "ExemptedPrincipalArns", "value": [] }],
                    }),
                )
            })
            .collect(),
        "/reset-enabled-control" => enabled_control_identifiers()
            .into_iter()
            .map(|(case, identifier)| {
                (
                    case,
                    serde_json::json!({ "enabledControlIdentifier": identifier }),
                )
            })
            .collect(),
        // Every catalogued baseline at every version it offers, on a target of its own.
        "/enable-baseline" => BASELINE_CATALOG
            .iter()
            .enumerate()
            .flat_map(|(index, baseline)| {
                baseline
                    .versions
                    .iter()
                    .enumerate()
                    .map(move |(version_index, version)| {
                        (
                            format!("{} at {version}", baseline.name),
                            serde_json::json!({
                                "baselineIdentifier": format!(
                                    "arn:aws:controltower:us-west-2::baseline/{}",
                                    baseline.id
                                ),
                                "baselineVersion": version,
                                "targetIdentifier": ou(&format!("eb{index}{version_index}")),
                            }),
                        )
                    })
            })
            .collect(),
        "/disable-baseline" => enabled_baseline_identifiers()
            .into_iter()
            .map(|(case, identifier)| {
                (
                    case,
                    serde_json::json!({ "enabledBaselineIdentifier": identifier }),
                )
            })
            .collect(),
        "/update-enabled-baseline" => enabled_baseline_identifiers()
            .into_iter()
            .map(|(case, identifier)| {
                (
                    case,
                    serde_json::json!({
                        "enabledBaselineIdentifier": identifier,
                        "baselineVersion": "4.0",
                    }),
                )
            })
            .collect(),
        "/reset-enabled-baseline" => enabled_baseline_identifiers()
            .into_iter()
            .map(|(case, identifier)| {
                (
                    case,
                    serde_json::json!({ "enabledBaselineIdentifier": identifier }),
                )
            })
            .collect(),
        other => panic!("no request table for {other}"),
    }
}

/// Assert `answer` is `write`'s precondition refusal: its family's status and modelled error
/// code, AWS's wording for it, and nothing minted.
fn assert_refused(case: &str, write: Write, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (write.family.status(), Some(write.family.code())),
        "case {case:?} on {}: expected {} {}, got {:?} body {:?}",
        write.path,
        write.family.status(),
        write.family.code(),
        (answer.status, answer.error_code.as_deref()),
        answer.body
    );
    assert_eq!(
        answer.string("message"),
        write.family.message(),
        "case {case:?} on {}: expected the precondition wording {:?}, got {:?}",
        write.path,
        write.family.message(),
        answer.string("message")
    );
    for minted in ["operationIdentifier", "arn"] {
        assert!(
            answer
                .body
                .get(minted)
                .is_none_or(serde_json::Value::is_null),
            "case {case:?} on {}: a refusal must mint no {minted}, got {:?}",
            write.path,
            answer.body
        );
    }
}

/// RULE: in an account AWS Control Tower has provisioned nothing in, every call that would
/// enable, update, re-deploy or disable a control or a baseline is its resource family's
/// precondition refusal — whatever the request carries, and ahead of every complaint about
/// the request itself.
#[tokio::test]
async fn an_unset_up_account_refuses_every_enablement_write() {
    let router = emulator();

    for write in WRITES {
        for (case, body) in requests(*write) {
            let answer = call(&router, write.path, body.clone()).await;
            assert_refused(&format!("{case} ({body})"), *write, &answer);
        }
    }
}

/// RULE (what decides it): the precondition is the ACCOUNT's setup, not the request. Once
/// the account has been set up the very same requests are answered out of the store — some
/// accepted, some refused for what they name — and none of them carries the precondition
/// wording.
#[tokio::test]
async fn a_set_up_account_answers_the_same_requests_out_of_its_store() {
    let router = emulator();
    set_up_account(&router).await;

    for write in WRITES {
        for (case, body) in requests(*write) {
            let answer = call(&router, write.path, body.clone()).await;
            let case = format!("{case} ({body})");
            assert_ne!(
                answer.string("message"),
                write.family.message(),
                "case {case:?} on {}: a set-up account must not answer the precondition \
                 refusal, got {:?}",
                write.path,
                answer.body
            );
            // An answer out of the store is the call's own: a success, or a refusal about
            // what the request names — never the 501 of an unimplemented operation.
            assert!(
                answer.status == 200 || (400..500).contains(&answer.status),
                "case {case:?} on {}: expected a success or a client error, got {} body {:?}",
                write.path,
                answer.status,
                answer.body
            );
        }
    }
}

/// RULE (the writing side of it): a refused write writes nothing, so a run of them leaves
/// the account exactly as un-set-up as it was — the landing zone created afterwards is the
/// account's first, and the enablement listings that landing zone makes answerable are
/// empty.
#[tokio::test]
async fn a_refused_write_writes_nothing() {
    let router = emulator();

    for round in 0..2 {
        for write in WRITES {
            for (case, body) in requests(*write) {
                let answer = call(&router, write.path, body.clone()).await;
                assert_refused(&format!("round {round}: {case} ({body})"), *write, &answer);
            }
        }
    }

    // The account is still un-set-up, so the create is accepted as its FIRST landing zone (a
    // second one would be refused: an account holds one).
    set_up_account(&router).await;

    for (path, member) in [
        ("/list-enabled-controls", "enabledControls"),
        ("/list-enabled-baselines", "enabledBaselines"),
    ] {
        let listed = call(&router, path, serde_json::json!({})).await;
        assert_eq!(
            (listed.status, listed.get(member)),
            (200, &serde_json::json!([])),
            "{path}: expected an empty listing once the account is set up, got {} body {:?}",
            listed.status,
            listed.body
        );
    }

    // And no refused write started an operation: the control operation log is the one of the
    // three logs an un-set-up account's writes could have appended to without a landing
    // zone, and it is empty.
    let operations = call(&router, "/list-control-operations", serde_json::json!({})).await;
    assert_eq!(
        (operations.status, operations.get("controlOperations")),
        (200, &serde_json::json!([])),
        "list-control-operations: a refused write must have started no operation, got {} \
         body {:?}",
        operations.status,
        operations.body
    );
}
