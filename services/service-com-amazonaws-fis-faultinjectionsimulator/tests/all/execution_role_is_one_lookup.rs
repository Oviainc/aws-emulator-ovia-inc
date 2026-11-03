// SPDX-License-Identifier: CC0-1.0

//! `StartExperiment`'s execution-role gate asks the account's IAM store two questions
//! about the template's `roleArn` — whether the role's trust policy lets FIS assume it,
//! and which policy documents are in force on it — and this suite pins that both are
//! asked about ONE record.
//!
//! RULE: which stored role an execution-role ARN names is a single reading of the
//! account's IAM records, so the trust answer and the privileges answer a start spends on
//! one `roleArn` always come from the same role: an ARN that names a stored role is
//! answered from that role by both halves, and an ARN that names none is answered by
//! neither, which leaves the start with nothing to show and lets the run go ahead.
//!
//! Both halves are reachable only through `StartExperiment`, so the end-to-end case drives
//! them there, over the router, and tells the three possible answers apart by what the
//! service does:
//!
//! * the trust half resolved the role and the trust policy withholds the grant — the start
//!   is REFUSED (HTTP 400) and no experiment exists;
//! * the privileges half resolved the role and its documents carry no grant for the
//!   action's own API call — the start is ACCEPTED and the experiment ends `failed`,
//!   attributing the failure to insufficient privileges;
//! * neither half resolved anything — the start is accepted and the experiment RUNS,
//!   reporting no error at all.
//!
//! The rows differ along the FORM of the ARN relative to the one the store holds the role
//! under (the stored ARN itself, the role name in another case, an extra path segment,
//! another account, another partition, a longer name), each of them a well-formed IAM role
//! ARN so that this service's own `roleArn` syntax rule admits it, and each run against
//! both stored roles — so a lookup that learned a new way to match an ARN in one half
//! only would make some row answer two of the three ways at once.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::state::action_permissions::role_documents;
use service_com_amazonaws_fis_faultinjectionsimulator::state::{
    role_by_arn, role_trust_refusal, IamState, FIS_SERVICE_PRINCIPAL,
    INSUFFICIENT_PRIVILEGES_CODE,
};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn as iam_role_arn, InlinePolicy, DEFAULT_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, get_experiment, poll_until_final, start_experiment, stop_experiment, token,
};

/// The account id every ARN this emulator mints carries (`::engine::EMULATOR_ACCOUNT_ID`),
/// spelled out so a changed account fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The IAM action the `aws:cloudwatch:assert-alarm-state` action's own API call needs, as
/// the AWS FIS actions reference states it for that action (and as this crate's
/// `ACTION_CATALOG` entry carries it). Spelled out so a changed catalog fails here.
const ALARM_ASSERT_PERMISSION: &str = "cloudwatch:DescribeAlarms";

/// The alarm the action under test asserts on: a value the gate compares against policy
/// statements and never dials.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:one-lookup";

/// The action name every template here gives its single action, which a privileges failure
/// locates.
const ACTION_NAME: &str = "underTest";

/// How long a case waits for a run to reach its terminal status. The emulator's spans are
/// well under this.
const TERMINAL_BUDGET: Duration = Duration::from_secs(8);

/// How long a case waits for the move off `initiating` the clock owes a started
/// experiment.
const TRANSITION_BUDGET: Duration = Duration::from_secs(5);

/// The role whose trust policy WITHHOLDS the grant from FIS, so the trust half refuses
/// every start that resolves it.
const TRUST_WITHHELD_ROLE: &str = "fis-one-lookup-trust";

/// The role FIS may assume that carries no grant for the action's API call, so the
/// privileges half attributes a failure to every run that resolves it.
const PRIVILEGES_WITHHELD_ROLE: &str = "fis-one-lookup-privileges";

/// What a row's `roleArn` names, and so which half of the gate can be shown anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Names {
    /// The stored role whose trust policy withholds the grant.
    TrustWithheld,
    /// The stored role that may be assumed but carries no grant for the action.
    PrivilegesWithheld,
    /// No stored role at all.
    NoRole,
}

/// One row: the `roleArn` a template carries and what the store holds for it.
struct Row {
    case: String,
    role_arn: String,
    names: Names,
}

/// One row of the direct case: the ARN asked about, and the record both halves must answer
/// from — `None` when the store holds none.
struct Lookup {
    case: String,
    role_arn: String,
    record: Option<Record>,
}

/// The stored record a [`Lookup`] resolves to, by the two things that tell it from the
/// other stored role: its name and its own marker document.
struct Record {
    role_name: &'static str,
    document: String,
}

/// An emulator carrying this service over one shared engine, plus that engine: the gate
/// reads the IAM store that lives in the SAME engine, so a case seeds its roles there.
fn emulator_with_iam_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A trust document with one statement: `effect` for the FIS service principal performing
/// the assume-role action.
fn trust(effect: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": FIS_SERVICE_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity document that grants something ELSE: a readable document that carries no
/// grant for the action under test, so the privileges half has evidence and the evidence
/// withholds.
fn unrelated_grant(marker: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": format!("logs:PutLogEvents{marker}"),
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Seed `role_name` into `store` — the record `CreateRole` writes — carrying `trust` as its
/// `AssumeRolePolicyDocument` and `inline` as an embedded role policy, and answer the ARN
/// the store holds it under.
fn insert_role(
    store: &mut IamState,
    role_name: &str,
    trust: &str,
    inline: Option<String>,
) -> String {
    let arn = iam_role_arn(DEFAULT_PATH, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: Some(trust.to_string()),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    if let Some(document) = inline {
        store.role_inline_policies.insert(
            name_key(role_name),
            BTreeMap::from([(
                name_key("one-lookup"),
                InlinePolicy {
                    policy_name: "one-lookup".to_string(),
                    policy_document: document,
                },
            )]),
        );
    }
    arn
}

/// Seed the same two roles into the shared engine's IAM store, and answer the ARNs it
/// holds them under.
fn seed_roles(engine: &Arc<engine::InMemoryEngine>) -> (String, String) {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let trust_withheld = insert_role(&mut store, TRUST_WITHHELD_ROLE, &trust("Deny"), None);
    let privileges_withheld = insert_role(
        &mut store,
        PRIVILEGES_WITHHELD_ROLE,
        &trust("Allow"),
        Some(unrelated_grant("")),
    );
    (trust_withheld, privileges_withheld)
}

/// ARN forms that are NOT the one the store holds `role_name` under, each still a
/// well-formed IAM role ARN (`arn:<partition>:iam::<account>:role/<path>/<name>`, which is
/// the syntax this service requires of a `roleArn`): the rows differ along the segment that
/// differs.
fn near_misses(role_name: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "the role name in another case",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/{}", role_name.to_uppercase()),
        ),
        (
            "a path the stored role has not",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/service-role/{role_name}"),
        ),
        (
            "another account",
            format!("arn:aws:iam::123456789012:role/{role_name}"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:iam::{ACCOUNT_ID}:role/{role_name}"),
        ),
        (
            "a longer role name",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}-standby"),
        ),
    ]
}

/// Every row of the table: each stored role's own ARN, and every near-miss form of it.
fn rows(trust_withheld: &str, privileges_withheld: &str) -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "the stored ARN of the role FIS may not assume".to_string(),
            role_arn: trust_withheld.to_string(),
            names: Names::TrustWithheld,
        },
        Row {
            case: "the stored ARN of the role carrying no grant".to_string(),
            role_arn: privileges_withheld.to_string(),
            names: Names::PrivilegesWithheld,
        },
    ];
    for (role, label) in [
        (TRUST_WITHHELD_ROLE, "beside the untrusted role"),
        (PRIVILEGES_WITHHELD_ROLE, "beside the ungranted role"),
    ] {
        for (form, arn) in near_misses(role) {
            rows.push(Row {
                case: format!("{form}, {label}"),
                role_arn: arn,
                names: Names::NoRole,
            });
        }
    }
    rows
}

/// A create body for a template whose single action is the alarm assert, run under
/// `row`'s `roleArn`.
fn template_body(index: usize, row: &Row) -> Value {
    json!({
        "clientToken": token(&format!("one-lookup-template-{index}")),
        "description": format!("one role lookup: {}", row.case),
        "roleArn": row.role_arn,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": {
            ACTION_NAME: {
                "actionId": "aws:cloudwatch:assert-alarm-state",
                "parameters": { "alarmArns": ALARM_ARN, "alarmStates": "INSUFFICIENT_DATA" }
            }
        }
    })
}

/// The experiment's `state` once the record has moved off `initiating` — the transition the
/// clock owes a started experiment — or as it was last read within `budget`.
async fn state_past_the_transition(
    router: &MultiServiceRouter,
    id: &str,
    budget: Duration,
) -> Value {
    const INTERVAL: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + budget;
    loop {
        let experiment = get_experiment(router, id).await.member("experiment").clone();
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status != "initiating" || Instant::now() >= deadline {
            return experiment["state"].clone();
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

/// RULE: the trust answer and the privileges answer a start spends on one `roleArn` come
/// from ONE stored role, so an ARN the store holds a role for is answered from that role by
/// both halves of the gate and an ARN it holds none for is answered by neither.
#[tokio::test]
async fn both_halves_of_the_execution_role_gate_read_one_role() {
    let (router, engine) = emulator_with_iam_store();
    let (trust_withheld, privileges_withheld) = seed_roles(&engine);

    for (index, row) in rows(&trust_withheld, &privileges_withheld)
        .iter()
        .enumerate()
    {
        let case = &row.case;
        let created = create_template(&router, template_body(index, row)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be created, got {} with {:?}",
            created.status, created.body
        );
        let template_id = created.member("experimentTemplate")["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
            .to_string();

        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("one-lookup-start-{index}")),
                "experimentTemplateId": template_id
            }),
        )
        .await;

        if row.names == Names::TrustWithheld {
            // The trust half resolved the role, and the document withholds the grant.
            assert_eq!(
                started.status, 400,
                "case {case:?}: expected the start of a role FIS may not assume to be \
                 refused, got {} with {:?}",
                started.status, started.body
            );
            assert!(
                started.error_code().contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?}",
                started.body
            );
            let message = started.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&row.role_arn) && message.contains(FIS_SERVICE_PRINCIPAL),
                "case {case:?}: expected the refusal to name the role and the service \
                 principal, got {message:?}"
            );
            continue;
        }

        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the start to be accepted, got {} with {:?}",
            started.status, started.body
        );
        let experiment = started.member("experiment").clone();
        assert_eq!(
            experiment["roleArn"].as_str().unwrap_or_default(),
            row.role_arn,
            "case {case:?}: expected the experiment to carry the template's roleArn, \
             got {experiment:?}"
        );
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {experiment:?}"))
            .to_string();

        match row.names {
            Names::PrivilegesWithheld => {
                // The privileges half resolved the SAME role: its documents are readable
                // and carry no grant, so the run fails and says why.
                let ended = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
                let state = ended["state"].clone();
                assert_eq!(
                    state["status"].as_str().unwrap_or_default(),
                    "failed",
                    "case {case:?}: expected a run whose role carries no grant to fail, \
                     got {state:?}"
                );
                assert_eq!(
                    state["error"]["code"].as_str().unwrap_or_default(),
                    INSUFFICIENT_PRIVILEGES_CODE,
                    "case {case:?}: expected the failure attributed to privileges, \
                     got {state:?}"
                );
                assert_eq!(
                    state["error"]["location"].as_str().unwrap_or_default(),
                    format!("actions.{ACTION_NAME}"),
                    "case {case:?}: expected the error to locate the action that could not \
                     run, got {state:?}"
                );
                let reason = state["reason"].as_str().unwrap_or_default();
                for named in [
                    row.role_arn.as_str(),
                    ACTION_NAME,
                    ALARM_ASSERT_PERMISSION,
                ] {
                    assert!(
                        reason.contains(named),
                        "case {case:?}: expected the reason to name {named:?}, \
                         got {reason:?}"
                    );
                }
                // The same answer is PERSISTED: a later read reports it.
                let read = get_experiment(&router, &id).await;
                assert_eq!(
                    read.member("experiment")["state"]["error"]["code"]
                        .as_str()
                        .unwrap_or_default(),
                    INSUFFICIENT_PRIVILEGES_CODE,
                    "case {case:?}: expected the read-back experiment to carry the same \
                     failure, got {:?}",
                    read.body
                );
            }
            Names::NoRole => {
                // Neither half resolved anything: nothing is shown about the role, so the
                // start was not refused AND the run is not failed for privileges.
                let state = state_past_the_transition(&router, &id, TRANSITION_BUDGET).await;
                assert_eq!(
                    state["status"].as_str().unwrap_or_default(),
                    "running",
                    "case {case:?}: expected a run nothing was shown against to run, \
                     got {state:?}"
                );
                assert!(
                    state["error"].is_null(),
                    "case {case:?}: expected a run that has not failed to report no error, \
                     got {state:?}"
                );
                // This row is done with its experiment: stop it and let the stop settle, so
                // no later row is refused for the account's active-experiment quota.
                let stopped = stop_experiment(&router, &id).await;
                assert_eq!(
                    stopped.status, 200,
                    "case {case:?}: expected the stop to be accepted, got {} with {:?}",
                    stopped.status, stopped.body
                );
                let ended = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
                assert_eq!(
                    ended["state"]["status"].as_str().unwrap_or_default(),
                    "stopped",
                    "case {case:?}: expected the stopped experiment to reach stopped, \
                     got {ended:?}"
                );
            }
            Names::TrustWithheld => unreachable!("the refused rows returned above"),
        }
    }
}

/// RULE (the same reading, asserted on the two helpers directly): one execution-role ARN
/// resolves to one stored record, so the trust half and the privileges half either both
/// answer about that record or both answer about none.
///
/// The ARN forms are the ones the end-to-end case drives, and each stored role carries its
/// OWN marker document — so a row that resolved a different record in one half than in the
/// other would hand back the other role's document.
#[test]
fn one_role_arn_resolves_to_one_stored_record() {
    let mut iam = IamState::default();
    // Both roles withhold the trust grant, so "the trust half resolved a role" and "the
    // trust half refuses" are the same observation.
    let first = insert_role(
        &mut iam,
        TRUST_WITHHELD_ROLE,
        &trust("Deny"),
        Some(unrelated_grant("-first")),
    );
    let second = insert_role(
        &mut iam,
        PRIVILEGES_WITHHELD_ROLE,
        &trust("Deny"),
        Some(unrelated_grant("-second")),
    );

    let mut rows: Vec<Lookup> = vec![
        Lookup {
            case: "the first stored ARN".to_string(),
            role_arn: first,
            record: Some(Record {
                role_name: TRUST_WITHHELD_ROLE,
                document: unrelated_grant("-first"),
            }),
        },
        Lookup {
            case: "the second stored ARN".to_string(),
            role_arn: second,
            record: Some(Record {
                role_name: PRIVILEGES_WITHHELD_ROLE,
                document: unrelated_grant("-second"),
            }),
        },
    ];
    for (role, label) in [
        (TRUST_WITHHELD_ROLE, "beside the first"),
        (PRIVILEGES_WITHHELD_ROLE, "beside the second"),
    ] {
        for (form, arn) in near_misses(role) {
            rows.push(Lookup {
                case: format!("{form}, {label}"),
                role_arn: arn,
                record: None,
            });
        }
    }

    for row in rows {
        let case = &row.case;
        let expected_name = row.record.as_ref().map(|record| record.role_name);
        let resolved = role_by_arn(&iam, &row.role_arn).map(|role| role.role_name.clone());
        let refusal = role_trust_refusal(&row.role_arn, &iam);
        let documents = role_documents(&row.role_arn, &iam);
        assert_eq!(
            resolved.as_deref(),
            expected_name,
            "case {case:?}: expected the shared lookup to resolve {expected_name:?}, \
             got {resolved:?}"
        );
        assert_eq!(
            refusal.is_some(),
            expected_name.is_some(),
            "case {case:?}: expected the trust half to answer about the same record \
             ({expected_name:?}), got a refusal: {}",
            refusal.is_some()
        );
        assert_eq!(
            documents,
            row.record.map(|record| vec![record.document]),
            "case {case:?}: expected the privileges half to read the same record's \
             document, got {documents:?}"
        );
    }
}
