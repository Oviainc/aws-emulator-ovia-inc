// SPDX-License-Identifier: CC0-1.0

//! The mgn account-initialization gate, end to end over HTTP.
//!
//! The rule under test: an operation whose model declares
//! `UninitializedAccountException` refuses every request with it — HTTP 400, error code
//! `UninitializedAccountException`, message `Account not initialized` — while the
//! account the request acts on has not completed `InitializeService`, and it does so
//! AHEAD of any per-resource check (a name already in use, an id nothing was created
//! under) but BEHIND the modelled field constraints the front end refuses before a
//! handler runs. A client therefore tells a malformed member
//! (`BadRequestException`) from an uninitialized account (`UninitializedAccountException`)
//! from a missing resource (`ResourceNotFoundException`) by the code alone.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    LaunchConfigurationTemplate, Wave,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The modelled account-state error every gated operation answers, and the one fixed
/// sentence Application Migration Service words it with.
const UNINITIALIZED: &str = "UninitializedAccountException";
const UNINITIALIZED_MESSAGE: &str = "Account not initialized";

/// The front end's own code for a member whose VALUE violates a modelled constraint —
/// not a shape in this service's model, which is what makes it discriminating.
const BAD_REQUEST: &str = "BadRequestException";

/// A well-formed `SsmInstanceID` at the modelled minimum length: `i-` + 17 = 19 (the
/// model's `@length` 19..20, `@pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$)`).
const SSM_INSTANCE_MIN_LEN: &str = "i-0123456789abcdefg";

/// A well-formed `SsmInstanceID` at the modelled maximum length: `mi-` + 17 = 20.
const SSM_INSTANCE_MAX_LEN: &str = "mi-00000000000000000";

/// A `launchConfigurationTemplateID` in the modelled shape (`^lct-[0-9a-zA-Z]{17}$`,
/// length 21..21) that no template was created under.
const UNKNOWN_TEMPLATE_ID: &str = "lct-00000000000000000";

/// Every operation this crate implements whose model declares
/// `UninitializedAccountException`, with a well-formed request for it: the creates, the
/// listings, the reads and the deletes. Source:
/// `smithy_query(query="operation",operation="CreateWave,ListWaves,CreateConnector,ListConnectors,DeleteLaunchConfigurationTemplate,CreateApplication,ListApplications,UpdateApplication,DeleteApplication,DescribeSourceServers")`
/// — each lists `UninitializedAccountException` (httpError 400).
///
/// The rows differ along what the rule generalises over: the operation's kind (create,
/// list, update, delete, read), whether the request names an `accountID` at all, which
/// optional members it carries, and whether the resource it names exists. The recorded
/// `CreateWave(name=…)` and `CreateConnector(name=…, ssmInstanceID='mi-…')` calls are
/// two rows.
fn gated_calls(account: Option<&str>) -> Vec<(String, &'static str, Value)> {
    // `accountID` is a member of some of these operations only; a row for an operation
    // that has none acts on the account the request is signed for, which is the
    // emulator's own account.
    let scoped = |mut body: Value, takes_account: bool| -> Value {
        if let (Some(account), true) = (account, takes_account) {
            body["accountID"] = json!(account);
        }
        body
    };
    let label = |case: &str| -> String {
        match account {
            Some(account) => format!("{case} (accountID {account})"),
            None => format!("{case} (no accountID)"),
        }
    };

    vec![
        (
            label("CreateWave, name only"),
            "CreateWave",
            scoped(json!({"name": "cutover-wave"}), true),
        ),
        (
            label("CreateWave, description and tags"),
            "CreateWave",
            scoped(
                json!({"name": "SAP ERP  prod (wave 1)", "description": "line one", "tags": {"env": "dev"}}),
                true,
            ),
        ),
        (
            label("ListWaves, no filters"),
            "ListWaves",
            scoped(json!({}), true),
        ),
        (
            label("ListWaves, filtered and paged"),
            "ListWaves",
            scoped(
                json!({"filters": {"isArchived": false}, "maxResults": 1}),
                true,
            ),
        ),
        (
            label("CreateConnector, shortest ssmInstanceID"),
            "CreateConnector",
            scoped(
                json!({"name": "migration-connector", "ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
                false,
            ),
        ),
        (
            label("CreateConnector, longest ssmInstanceID, tags and command config"),
            "CreateConnector",
            scoped(
                json!({
                    "name": "managed-instance-connector",
                    "ssmInstanceID": SSM_INSTANCE_MAX_LEN,
                    "tags": {"env": "prod", "team": "migration"},
                    "ssmCommandConfig": {
                        "s3OutputEnabled": true,
                        "outputS3BucketName": "migration-logs",
                        "cloudWatchOutputEnabled": false,
                    },
                }),
                false,
            ),
        ),
        (
            label("ListConnectors, no filters"),
            "ListConnectors",
            scoped(json!({}), false),
        ),
        (
            label("ListConnectors, filtered"),
            "ListConnectors",
            scoped(
                json!({"filters": {"connectorIDs": ["connector-00000000000000000"]}}),
                false,
            ),
        ),
        (
            label("DeleteLaunchConfigurationTemplate, unknown id"),
            "DeleteLaunchConfigurationTemplate",
            scoped(
                json!({"launchConfigurationTemplateID": UNKNOWN_TEMPLATE_ID}),
                false,
            ),
        ),
        (
            label("CreateApplication, name only"),
            "CreateApplication",
            scoped(json!({"name": "billing-app"}), true),
        ),
        (
            label("ListApplications, no filters"),
            "ListApplications",
            scoped(json!({}), true),
        ),
        (
            label("UpdateApplication, unknown id"),
            "UpdateApplication",
            scoped(
                json!({"applicationID": "app-00000000000000000", "name": "renamed"}),
                true,
            ),
        ),
        (
            label("DeleteApplication, unknown id"),
            "DeleteApplication",
            scoped(json!({"applicationID": "app-00000000000000000"}), true),
        ),
        (
            label("DescribeSourceServers, no filters"),
            "DescribeSourceServers",
            scoped(json!({}), true),
        ),
    ]
}

/// Every gated operation answers the same modelled refusal, with the same fixed
/// sentence, while the account it acts on is uninitialized — in the body's `__type`, in
/// the `x-amzn-errortype` header restJson1 resolves an error shape from, and in
/// `message`.
#[tokio::test]
async fn every_gated_operation_refuses_an_uninitialized_account() {
    // Nothing has initialized the caller's own account.
    let mgn = Mgn::new();
    for (case, operation, body) in gated_calls(None) {
        let (status, error_type, answer) = mgn.call_reporting_error_type(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, UNINITIALIZED),
            "case {case:?}: expected 400 {UNINITIALIZED}, got {status} with {answer:?}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some(UNINITIALIZED),
            "case {case:?}: expected x-amzn-errortype {UNINITIALIZED}, got {error_type:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(UNINITIALIZED_MESSAGE),
            "case {case:?}: expected message {UNINITIALIZED_MESSAGE:?}, got {:?}",
            answer["message"]
        );
    }

    // The flag is per account: initializing the caller's own account leaves another
    // account's requests refused exactly the same way.
    let mgn = Mgn::new();
    mgn.initialize().await;
    for (case, operation, body) in gated_calls(Some(OTHER_ACCOUNT)) {
        // The operations that carry no `accountID` member act on the initialized
        // account, so only the ones that can name another account are probed here.
        if !body["accountID"].is_string() {
            continue;
        }
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, UNINITIALIZED),
            "case {case:?}: expected 400 {UNINITIALIZED} for an uninitialized account, got \
             {status} with {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(UNINITIALIZED_MESSAGE),
            "case {case:?}: expected message {UNINITIALIZED_MESSAGE:?}, got {:?}",
            answer["message"]
        );
    }
}

/// The gate is evaluated ahead of every per-resource check: a request whose resource
/// exists, whose name is taken, or whose id names nothing all answer the account gate
/// while the account is uninitialized — and the per-resource answers appear only once
/// it is initialized.
#[tokio::test]
async fn the_gate_is_evaluated_before_any_per_resource_check() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // Records of an account that has NOT been initialized, seeded straight into the
    // store: a wave whose name a create would conflict with, and a launch
    // configuration template a delete would remove.
    {
        let store = mgn.store();
        let mut state = store.lock();
        state.insert_wave(Wave {
            wave_id: "wave-0000000000000000a".to_string(),
            arn: format!("arn:aws:mgn:{REGION}:{OTHER_ACCOUNT}:wave/wave-0000000000000000a"),
            name: "taken-name".to_string(),
            account_id: OTHER_ACCOUNT.to_string(),
            ..Wave::default()
        });
        let seeded = state.insert_launch_configuration_template(LaunchConfigurationTemplate {
            launch_configuration_template_id: UNKNOWN_TEMPLATE_ID.to_string(),
            arn: format!(
                "arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:launch-configuration-template/{UNKNOWN_TEMPLATE_ID}"
            ),
            // The template belongs to the account that was never initialized.
            account_id: OTHER_ACCOUNT.to_string(),
        });
        assert!(
            seeded,
            "seeding a launch configuration template must store it (the store bound was not reached)"
        );
    }

    // A name the uninitialized account already uses is still the gate, not the
    // modelled conflict.
    let (status, answer) = mgn
        .call(
            "CreateWave",
            &json!({"name": "taken-name", "accountID": OTHER_ACCOUNT}),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (400, UNINITIALIZED),
        "a taken wave name on an uninitialized account must be {UNINITIALIZED}, got {status} \
         with {answer:?}"
    );

    // On the initialized account the same name is free, so the create succeeds: the
    // gate is the only thing the refusal above was about.
    let (status, answer) = mgn
        .call("CreateWave", &json!({"name": "taken-name"}))
        .await;
    assert_eq!(
        status, 201,
        "CreateWave must succeed once the account is initialized, got {status} with {answer:?}"
    );

    // And a second create under that name, differing in another member, is the modelled
    // conflict — the per-resource check the gate stands in front of.
    let (status, answer) = mgn
        .call(
            "CreateWave",
            &json!({"name": "taken-name", "description": "a different wave"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (409, "ConflictException"),
        "a taken wave name with different members must be 409 ConflictException, got {status} \
         with {answer:?}"
    );

    // A delete the initialized account cannot see the template of is the modelled
    // not-found; the same call for the uninitialized account is the gate.
    let (status, answer) = mgn
        .call(
            "DeleteLaunchConfigurationTemplate",
            &json!({"launchConfigurationTemplateID": UNKNOWN_TEMPLATE_ID}),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (404, "ResourceNotFoundException"),
        "a template of another account must be 404 ResourceNotFoundException, got {status} with \
         {answer:?}"
    );
}

/// The `ssmInstanceID` rows a malformed-value refusal spans, and the well-formed rows
/// that must clear it. Source: the model's `SsmInstanceID` (`@length` 19..20,
/// `@pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$)`), so each malformed row
/// breaks the pattern, a published length bound, or both.
fn ssm_instance_id_cases() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("free text", "not-a-valid-ssm-instance-id", false),
        ("one character short of the minimum", "i-0123456789abcdef", false),
        (
            "one character past the maximum",
            "mi-0123456789abcdefgh",
            false,
        ),
        ("the right length, the wrong prefix", "x-0123456789abcdefg", false),
        (
            "a character outside the body alphabet",
            "i-0123456789abcdef_",
            false,
        ),
        ("no prefix at all", "0123456789abcdefghi", false),
        ("the modelled minimum length", SSM_INSTANCE_MIN_LEN, true),
        ("the modelled maximum length", SSM_INSTANCE_MAX_LEN, true),
        ("mixed-case body", "mi-A1b2C3d4E5f6G7h8I", true),
        ("all-letter body", "i-abcdefghijklmnopq", true),
    ]
}

/// A member whose value violates a modelled constraint is refused by the front end
/// BEFORE the account gate, with a different code: the two inputs the recorded check
/// contrasts surface `BadRequestException` and `UninitializedAccountException`
/// respectively, and neither answer is the other's.
#[tokio::test]
async fn a_malformed_field_is_refused_before_the_account_gate() {
    let mgn = Mgn::new();

    for (case, ssm_instance_id, well_formed) in ssm_instance_id_cases() {
        let body = json!({"name": "migration-connector", "ssmInstanceID": ssm_instance_id});
        let (status, answer) = mgn.call("CreateConnector", &body).await;
        let expected = if well_formed {
            UNINITIALIZED
        } else {
            BAD_REQUEST
        };
        assert_eq!(
            (status, error_code(&answer)),
            (400, expected),
            "case {case:?} ({ssm_instance_id:?}): expected 400 {expected}, got {status} with \
             {answer:?}"
        );
        assert_eq!(
            answer["connectorID"].as_str(),
            None,
            "case {case:?}: a refused CreateConnector mints no connector, got {:?}",
            answer["connectorID"]
        );
    }

    // Nothing above reached the store.
    let store = mgn.store();
    let state = store.lock();
    assert!(
        state.connectors_of(EMULATOR_ACCOUNT).is_empty(),
        "a refused CreateConnector must store no connector, got {} of them",
        state.connectors_of(EMULATOR_ACCOUNT).len()
    );
}
