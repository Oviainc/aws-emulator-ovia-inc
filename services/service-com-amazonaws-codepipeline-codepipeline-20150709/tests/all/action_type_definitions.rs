// SPDX-License-Identifier: CC0-1.0

//! `UpdateActionType` and `GetActionType`, over the wire the service actually speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: `UpdateActionType` stores the full `ActionTypeDeclaration` of
//! an action type the account ALREADY holds in the Region the request was signed for
//! — the catalog `ListActionTypes` reports, which is the action types the service
//! provides itself plus this account's registrations — and refuses any other identity
//! with `ActionTypeNotFoundException`; `GetActionType` reports back the definition
//! stored for an identity in that Region, and refuses an identity no definition was
//! stored for.
//!
//! Each row of the round-trip table is a different catalog entry with a definition
//! that differs along what the rule generalises over: which integration model the
//! executor declares, how many properties it carries and with which flags, whether it
//! states a description, permissions, urls, a policy template or a job timeout, and
//! what its artifact-count bounds are. Every read goes through the router, so nothing
//! here can pass by reading a handler's internals.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeDeclaration, ActionTypeIdentifier, ArtifactDetails,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ActionTypeKey, ActionTypeRecord, BuiltInActionType, BUILT_IN_ACTION_TYPES,
    MAX_ACTION_TYPE_DEFINITIONS,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ComAmazonawsCodepipelineCodepipeline20150709State as ServiceState;

use crate::custom_action_types::contains;
use crate::pipeline_lifecycle::Emulator;

/// The Region `Emulator::call` signs for.
const HOME_REGION: &str = "us-east-1";

/// Another Region, for the cases about which Region a definition belongs to.
const OTHER_REGION: &str = "us-west-2";

/// Every `ExecutorType` the model declares
/// (`smithy_query(query="shape",shape="ExecutorType")` → these two integration
/// models).
const EXECUTOR_TYPES: [&str; 2] = ["Lambda", "JobWorker"];

/// `ActionTypeDeclaration$properties` is `@length(min: 0, max: 10)`.
const MAX_PROPERTIES: usize = 10;

/// `MinimumActionTypeArtifactCount` and `MaximumActionTypeArtifactCount` are both
/// `@range(min: 0, max: 10)`.
const MAX_ARTIFACT_COUNT: i64 = 10;

/// `JobTimeout` is `@range(min: 60, max: 43200)`.
const MIN_JOB_TIMEOUT: i64 = 60;

/// A `lambdaFunctionArn` satisfying `LambdaFunctionArn`'s modelled pattern
/// (`^arn:aws(-[\w]+)*:lambda:.+:[0-9]{12}:function:.+$`).
const LAMBDA_ARN: &str = "arn:aws:lambda:us-east-1:123456789012:function:cp-action";

/// An `ActionTypeIdentifier` as a request carries it.
///
/// `pub(crate)` so the other suites of this target drive these two operations through
/// the same helpers; see `tests/all/delete_custom_action_type.rs`.
pub(crate) fn identifier(category: &str, owner: &str, provider: &str, version: &str) -> Value {
    json!({
        "category": category,
        "owner": owner,
        "provider": provider,
        "version": version,
    })
}

/// An `ActionTypeExecutor` for the integration `model`, carrying the
/// `ExecutorConfiguration` member the model documents for exactly that one
/// (`lambdaExecutorConfiguration` is "details about the `Lambda` executor",
/// `jobWorkerExecutorConfiguration` "details about the `JobWorker` executor").
fn executor(model: &str) -> Value {
    let configuration = match model {
        "Lambda" => json!({"lambdaExecutorConfiguration": {"lambdaFunctionArn": LAMBDA_ARN}}),
        "JobWorker" => json!({"jobWorkerExecutorConfiguration": {
            "pollingAccounts": ["123456789012"],
            "pollingServicePrincipals": ["codepipeline.amazonaws.com"],
        }}),
        other => panic!("{other:?} is not one of the modelled ExecutorType values"),
    };
    json!({"type": model, "configuration": configuration})
}

/// One `ActionTypeProperty` with every member stated, so a definition read back can
/// be compared member for member.
fn property(name: &str, optional: bool, no_echo: bool, queryable: bool) -> Value {
    json!({
        "name": name,
        "optional": optional,
        "key": !optional,
        "noEcho": no_echo,
        "queryable": queryable,
        "description": format!("the {name} property"),
    })
}

/// The definition to store for the catalog entry `id`, shaped by `variant` so that no
/// two rows of the table state the same structure.
fn definition(id: &Value, variant: usize) -> Value {
    let mut executor = executor(EXECUTOR_TYPES[variant % EXECUTOR_TYPES.len()]);
    if variant.is_multiple_of(3) {
        executor["jobTimeout"] = json!(MIN_JOB_TIMEOUT + variant as i64);
    }
    if variant % 4 == 1 {
        executor["policyStatementsTemplate"] =
            json!(r#"{"Version":"2012-10-17","Statement":[]}"#);
    }

    let input_minimum = (variant % 3) as i64;
    let output_maximum = if variant % 5 == 2 {
        MAX_ARTIFACT_COUNT
    } else {
        (variant % 2) as i64
    };
    let mut declaration = json!({
        "id": id,
        "executor": executor,
        "inputArtifactDetails": {
            "minimumCount": input_minimum,
            "maximumCount": input_minimum + (variant % 4) as i64,
        },
        "outputArtifactDetails": {
            "minimumCount": 0,
            "maximumCount": output_maximum,
        },
    });

    // How many properties the definition states, and whether one of them is the
    // single queryable property the model allows.
    let count = variant % 4;
    if count > 0 {
        let properties: Vec<Value> = (0..count)
            .map(|index| {
                let queryable = index == 0 && variant % 3 == 2;
                property(
                    &format!("p{index}"),
                    !queryable && index.is_multiple_of(2),
                    false,
                    queryable,
                )
            })
            .collect();
        declaration["properties"] = Value::Array(properties);
    }
    if variant.is_multiple_of(2) {
        declaration["description"] = json!(format!("definition {variant}"));
        // `AllowedAccount` is `@pattern("^[0-9]{12}|\\*$")`, so an account id or `*`.
        declaration["permissions"] = json!({"allowedAccounts": ["123456789012", "*"]});
    }
    if !variant.is_multiple_of(3) {
        declaration["urls"] = json!({
            "configurationUrl": format!("https://example.com/configure/{variant}"),
            "entityUrlTemplate": "https://example.com/entity/{Config:p0}",
        });
    }
    declaration
}

/// `UpdateActionType` for `declaration`, signed for `region`.
async fn update_in(emulator: &Emulator, region: &str, declaration: &Value) -> (u16, Value) {
    emulator
        .call_in(region, "UpdateActionType", &json!({"actionType": declaration}))
        .await
}

/// `GetActionType` for the four members of `id`, signed for `region`.
async fn get_in(emulator: &Emulator, region: &str, id: &Value) -> (u16, Value) {
    emulator.call_in(region, "GetActionType", id).await
}

/// The `(status, __type)` of a refused call.
fn refusal(operation: &str, case: &str, status: u16, response: &Value) -> (u16, String) {
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a refused {operation}, got {response}"))
        .to_string();
    (status, code)
}

#[tokio::test]
async fn a_definition_round_trips_for_every_action_type_the_catalog_holds() {
    let emulator = Emulator::new();
    // The catalog the service provides itself, as `state::BUILT_IN_ACTION_TYPES`
    // records it — every category and both the `AWS` and the `ThirdParty` owner.
    for (variant, entry) in BUILT_IN_ACTION_TYPES.iter().enumerate() {
        let id = identifier(entry.category, entry.owner, entry.provider, entry.version);
        let case = format!("{id} as definition {variant}");
        let declaration = definition(&id, variant);

        let (status, answered) = update_in(&emulator, HOME_REGION, &declaration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateActionType to be accepted, got {status} {answered}"
        );

        let (status, read) = get_in(&emulator, HOME_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetActionType to report the stored definition, \
             got {status} {read}"
        );
        assert!(
            contains(&declaration, &read["actionType"]),
            "case {case:?}: expected the definition {declaration}, got {read}"
        );
    }
}

#[tokio::test]
async fn a_definition_is_replaced_whole_by_the_next_call() {
    let emulator = Emulator::new();
    let id = identifier("Source", "ThirdParty", "GitHub", "1");

    // Two definitions of one identity that differ in every optional member, so a
    // read that reported the first one — or a merge of the two — fails.
    let first = definition(&id, 0);
    let second = definition(&id, 1);
    for (case, declaration) in [("first", &first), ("second", &second)] {
        let (status, answered) = update_in(&emulator, HOME_REGION, declaration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateActionType to be accepted, got {status} {answered}"
        );
    }

    let (status, read) = get_in(&emulator, HOME_REGION, &id).await;
    assert_eq!(status, 200, "expected the definition to be readable, got {read}");
    assert!(
        contains(&second, &read["actionType"]),
        "expected the second definition {second}, got {read}"
    );
    for dropped in ["description", "permissions"] {
        assert_eq!(
            read["actionType"].get(dropped),
            None,
            "expected {dropped:?} — stated only by the first definition — to be gone, got {read}"
        );
    }
}

#[tokio::test]
async fn an_action_type_the_catalog_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();
    // Each row names an identity no entry of the catalog carries, differing from a
    // real one in exactly one member: the provider, the version, the category or the
    // owner.
    let cases = [
        (
            "a provider no action type publishes",
            identifier("Build", "ThirdParty", "NoSuchProvider", "1"),
        ),
        (
            "a real provider at a version it does not publish",
            identifier("Deploy", "AWS", "CodeDeploy", "9"),
        ),
        (
            "a real provider in a category it does not serve",
            identifier("Source", "AWS", "CodeDeploy", "1"),
        ),
        (
            "a real AWS provider claimed for the ThirdParty owner",
            identifier("Deploy", "ThirdParty", "CodeDeploy", "1"),
        ),
        (
            "a real ThirdParty provider claimed for the AWS owner",
            identifier("Source", "AWS", "GitHub", "1"),
        ),
    ];

    for (case, id) in &cases {
        let (status, response) = update_in(&emulator, HOME_REGION, &definition(id, 0)).await;
        assert_eq!(
            refusal("UpdateActionType", case, status, &response),
            (400, "ActionTypeNotFoundException".to_string()),
            "case {case:?}: expected a 400 ActionTypeNotFoundException, got {status} {response}"
        );

        let (status, response) = get_in(&emulator, HOME_REGION, id).await;
        assert_eq!(
            refusal("GetActionType", case, status, &response),
            (400, "ActionTypeNotFoundException".to_string()),
            "case {case:?}: expected a 400 ActionTypeNotFoundException, got {status} {response}"
        );
    }
}

#[tokio::test]
async fn a_catalog_entry_nobody_defined_has_no_definition_to_report() {
    let emulator = Emulator::new();
    // Every action type the catalog holds exists — `ListActionTypes` reports it — but
    // a definition is reported only once `UpdateActionType` has stored one, so this
    // read is refused for exactly the same identity the round-trip case accepts.
    for entry in &BUILT_IN_ACTION_TYPES {
        let id = identifier(entry.category, entry.owner, entry.provider, entry.version);
        let case = format!("{id} before any UpdateActionType call");
        let (status, response) = get_in(&emulator, HOME_REGION, &id).await;
        assert_eq!(
            refusal("GetActionType", &case, status, &response),
            (400, "ActionTypeNotFoundException".to_string()),
            "case {case:?}: expected a 400 ActionTypeNotFoundException, got {status} {response}"
        );
    }
}

#[tokio::test]
async fn a_definition_belongs_to_the_region_its_call_was_signed_for() {
    let emulator = Emulator::new();
    let id = identifier("Invoke", "AWS", "Lambda", "1");
    let declaration = definition(&id, 3);

    let (status, answered) = update_in(&emulator, HOME_REGION, &declaration).await;
    assert_eq!(
        status, 200,
        "expected UpdateActionType in {HOME_REGION} to be accepted, got {status} {answered}"
    );

    let (status, response) = get_in(&emulator, OTHER_REGION, &id).await;
    assert_eq!(
        refusal("GetActionType", OTHER_REGION, status, &response),
        (400, "ActionTypeNotFoundException".to_string()),
        "case {OTHER_REGION:?}: a definition stored in {HOME_REGION} is not readable \
         in another Region, got {status} {response}"
    );

    // The action types the service provides itself are available in every Region, so
    // the same identity takes its own definition there.
    let elsewhere = definition(&id, 4);
    let (status, answered) = update_in(&emulator, OTHER_REGION, &elsewhere).await;
    assert_eq!(
        status, 200,
        "expected UpdateActionType in {OTHER_REGION} to be accepted, got {status} {answered}"
    );
    for (region, expected) in [(HOME_REGION, &declaration), (OTHER_REGION, &elsewhere)] {
        let (status, read) = get_in(&emulator, region, &id).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the definition stored there, got {status} {read}"
        );
        assert!(
            contains(expected, &read["actionType"]),
            "case {region:?}: expected the definition {expected}, got {read}"
        );
    }
}

#[tokio::test]
async fn a_registration_is_as_much_of_the_catalog_as_a_built_in_action_type() {
    let emulator = Emulator::new();
    // A registration the account holds in ONE Region, put in the shared store the
    // handler reads the way `CreateCustomActionType` writes it. Its owner is
    // `ThirdParty` because `ActionTypeIdentifier$owner` is
    // `@pattern("^AWS|ThirdParty$")`, so a definition never names a `Custom` one.
    let key = ActionTypeKey {
        category: "Build".to_string(),
        owner: "ThirdParty".to_string(),
        provider: "Partner-Build_1".to_string(),
        version: "3".to_string(),
    };
    emulator.engine().state::<ServiceState>().lock().register_action_type(
        key.clone(),
        ActionTypeRecord {
            settings: None,
            configuration_properties: Vec::new(),
            input_artifact_details: ArtifactDetails {
                minimum_count: 0,
                maximum_count: 1,
            },
            output_artifact_details: ArtifactDetails {
                minimum_count: 0,
                maximum_count: 1,
            },
            region: Some(HOME_REGION.to_string()),
            tags: Vec::new(),
        },
    );

    let id = identifier(&key.category, &key.owner, &key.provider, &key.version);
    let declaration = definition(&id, 5);

    let (status, answered) = update_in(&emulator, HOME_REGION, &declaration).await;
    assert_eq!(
        status, 200,
        "expected the registration to be definable in its own Region, got {status} {answered}"
    );
    let (status, read) = get_in(&emulator, HOME_REGION, &id).await;
    assert_eq!(status, 200, "expected the stored definition, got {status} {read}");
    assert!(
        contains(&declaration, &read["actionType"]),
        "expected the definition {declaration}, got {read}"
    );

    // A registration belongs to the Region it was made in, so in another Region no
    // action type carries this identity at all.
    let (status, response) = update_in(&emulator, OTHER_REGION, &declaration).await;
    assert_eq!(
        refusal("UpdateActionType", OTHER_REGION, status, &response),
        (400, "ActionTypeNotFoundException".to_string()),
        "case {OTHER_REGION:?}: expected a 400 ActionTypeNotFoundException, got {status} {response}"
    );
}

#[tokio::test]
async fn a_definition_the_documented_rules_rule_out_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let id = identifier("Test", "AWS", "DeviceFarm", "1");
    let with = |member: &str, value: Value| {
        let mut declaration = definition(&id, 0);
        declaration[member] = value;
        declaration
    };
    let with_executor = |model: &str, configuration: Value| {
        let mut declaration = definition(&id, 0);
        declaration["executor"] = json!({"type": model, "configuration": configuration});
        declaration
    };

    let refused = [
        (
            "two queryable properties",
            with(
                "properties",
                json!([
                    property("a", false, false, true),
                    property("b", false, false, true),
                ]),
            ),
        ),
        (
            "a queryable property that is not required",
            with("properties", json!([property("a", true, false, true)])),
        ),
        (
            "a queryable property that is secret",
            with("properties", json!([property("a", false, true, true)])),
        ),
        (
            "one property name declared twice",
            with(
                "properties",
                json!([
                    property("same", false, false, false),
                    property("same", true, false, false),
                ]),
            ),
        ),
        (
            "one property past the 10 a definition may state",
            with(
                "properties",
                Value::Array(
                    (0..=MAX_PROPERTIES)
                        .map(|index| property(&format!("p{index}"), false, false, false))
                        .collect(),
                ),
            ),
        ),
        (
            "an artifact count past the modelled maximum of 10",
            with(
                "inputArtifactDetails",
                json!({"minimumCount": 0, "maximumCount": MAX_ARTIFACT_COUNT + 1}),
            ),
        ),
        (
            "a Lambda executor with no Lambda configuration",
            with_executor("Lambda", json!({})),
        ),
        (
            "a JobWorker executor with no JobWorker configuration",
            with_executor("JobWorker", json!({})),
        ),
        (
            "a Lambda executor configured as a job worker",
            with_executor(
                "Lambda",
                json!({"jobWorkerExecutorConfiguration": {"pollingAccounts": ["123456789012"]}}),
            ),
        ),
        (
            "an executor configured as both integration models",
            with_executor(
                "JobWorker",
                json!({
                    "lambdaExecutorConfiguration": {"lambdaFunctionArn": LAMBDA_ARN},
                    "jobWorkerExecutorConfiguration": {"pollingAccounts": ["123456789012"]},
                }),
            ),
        ),
        (
            "an integration model the service does not publish",
            with_executor(
                "Fargate",
                json!({"lambdaExecutorConfiguration": {"lambdaFunctionArn": LAMBDA_ARN}}),
            ),
        ),
    ];

    for (case, declaration) in &refused {
        let (status, response) = update_in(&emulator, HOME_REGION, declaration).await;
        assert_eq!(
            refusal("UpdateActionType", case, status, &response),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected a 400 ValidationException, got {status} {response}"
        );
    }

    // A refused call persists nothing, so the identity every row above named still
    // carries no definition at all.
    let (status, response) = get_in(&emulator, HOME_REGION, &id).await;
    assert_eq!(
        refusal("GetActionType", "after every refusal", status, &response),
        (400, "ActionTypeNotFoundException".to_string()),
        "expected no definition to have been stored, got {status} {response}"
    );

    // The same definitions at the accepted side of each bound, to show the refusals
    // are the bound and not the rule: exactly one queryable property that is required
    // and not secret, ten properties, an artifact count of exactly ten, and each
    // integration model carrying its own configuration.
    let accepted = [
        (
            "exactly one queryable property, required and not secret",
            with("properties", json!([property("a", false, false, true)])),
        ),
        (
            "properties at the bound of 10",
            with(
                "properties",
                Value::Array(
                    (0..MAX_PROPERTIES)
                        .map(|index| property(&format!("p{index}"), false, false, false))
                        .collect(),
                ),
            ),
        ),
        (
            "an artifact count at the modelled maximum of 10",
            with(
                "inputArtifactDetails",
                json!({"minimumCount": 0, "maximumCount": MAX_ARTIFACT_COUNT}),
            ),
        ),
        (
            "a Lambda executor with its own configuration",
            with_executor(
                "Lambda",
                json!({"lambdaExecutorConfiguration": {"lambdaFunctionArn": LAMBDA_ARN}}),
            ),
        ),
        (
            "a JobWorker executor with its own configuration",
            with_executor(
                "JobWorker",
                json!({"jobWorkerExecutorConfiguration": {
                    "pollingServicePrincipals": ["codepipeline.amazonaws.com"],
                }}),
            ),
        ),
    ];

    for (case, declaration) in &accepted {
        let (status, answered) = update_in(&emulator, HOME_REGION, declaration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateActionType to be accepted, got {status} {answered}"
        );
        let (status, read) = get_in(&emulator, HOME_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stored definition, got {status} {read}"
        );
        assert!(
            contains(declaration, &read["actionType"]),
            "case {case:?}: expected the definition {declaration}, got {read}"
        );
    }
}

#[tokio::test]
async fn the_definition_store_is_bounded_across_every_region() {
    let emulator = Emulator::new();
    // The identity whose definition is already stored in the home Region, and one
    // that is not: both are action types the catalog holds in EVERY Region.
    let stored = BUILT_IN_ACTION_TYPES[0];
    let fresh = BUILT_IN_ACTION_TYPES[1];
    let key_of = |entry: &BuiltInActionType| ActionTypeKey {
        category: entry.category.to_string(),
        owner: entry.owner.to_string(),
        provider: entry.provider.to_string(),
        version: entry.version.to_string(),
    };

    // Fill the store to the bound through the one gate every write goes through,
    // under Region keys of their own: a definition is keyed by the Region its call
    // was signed for, and that Region comes from the caller's own credential scope,
    // so this is the dimension the bound has to hold across.
    {
        let state = emulator.engine().state::<ServiceState>();
        let mut state = state.lock();
        for index in 0..MAX_ACTION_TYPE_DEFINITIONS - 1 {
            let accepted = state.declare_action_type(
                &format!("us-fill-{index}"),
                key_of(&fresh),
                ActionTypeDeclaration::default(),
            );
            assert!(accepted, "case {index}: expected the store to take a definition below its bound");
        }
        let accepted = state.declare_action_type(
            HOME_REGION,
            key_of(&stored),
            ActionTypeDeclaration {
                id: ActionTypeIdentifier {
                    category: stored.category.to_string(),
                    owner: stored.owner.to_string(),
                    provider: stored.provider.to_string(),
                    version: stored.version.to_string(),
                },
                ..ActionTypeDeclaration::default()
            },
        );
        assert!(accepted, "expected the store to take the definition at its bound");
        assert_eq!(
            state.declared_action_type_count(),
            MAX_ACTION_TYPE_DEFINITIONS,
            "expected the store to be at its bound, got {}",
            state.declared_action_type_count()
        );
    }

    // Re-defining an identity this Region already holds a definition for grows
    // nothing, so it is accepted at the bound.
    let stored_id = identifier(
        stored.category,
        stored.owner,
        stored.provider,
        stored.version,
    );
    let declaration = definition(&stored_id, 2);
    let (status, answered) = update_in(&emulator, HOME_REGION, &declaration).await;
    assert_eq!(
        status, 200,
        "expected a definition the store already holds to be replaceable at the bound, \
         got {status} {answered}"
    );
    let (status, read) = get_in(&emulator, HOME_REGION, &stored_id).await;
    assert_eq!(status, 200, "expected the replaced definition, got {status} {read}");
    assert!(
        contains(&declaration, &read["actionType"]),
        "expected the definition {declaration}, got {read}"
    );

    // One definition past the bound is refused with the only error the operation
    // models for a request it cannot carry out.
    let fresh_id = identifier(fresh.category, fresh.owner, fresh.provider, fresh.version);
    let (status, response) = update_in(&emulator, HOME_REGION, &definition(&fresh_id, 1)).await;
    assert_eq!(
        refusal("UpdateActionType", "one past the bound", status, &response),
        (400, "RequestFailedException".to_string()),
        "expected a 400 RequestFailedException past the bound, got {status} {response}"
    );
    assert_eq!(
        emulator
            .engine()
            .state::<ServiceState>()
            .lock()
            .declared_action_type_count(),
        MAX_ACTION_TYPE_DEFINITIONS,
        "expected the refused call to have stored nothing"
    );
}

/// RULE: `GetActionType` REPORTS — it never writes. A read answers the same definition
/// however often it is repeated, and a read of an identity no definition was stored for
/// does not bring one into existence.
#[tokio::test]
async fn reading_a_definition_leaves_the_store_as_it_was() {
    let emulator = Emulator::new();
    let defined = BUILT_IN_ACTION_TYPES[0];
    let defined_id = identifier(
        defined.category,
        defined.owner,
        defined.provider,
        defined.version,
    );
    let declaration = definition(&defined_id, 2);
    let (status, answered) = update_in(&emulator, HOME_REGION, &declaration).await;
    assert_eq!(
        status, 200,
        "expected UpdateActionType to store the definition, got {status} {answered}"
    );

    // The identity nothing defined: a catalog entry in the same Region, so the only
    // thing separating the two reads is whether a definition was stored.
    let undefined = BUILT_IN_ACTION_TYPES[1];
    let undefined_id = identifier(
        undefined.category,
        undefined.owner,
        undefined.provider,
        undefined.version,
    );

    let count = || {
        emulator
            .engine()
            .state::<ServiceState>()
            .lock()
            .declared_action_type_count()
    };
    let stored_before = count();

    for round in 0..3 {
        let (status, read) = get_in(&emulator, HOME_REGION, &defined_id).await;
        assert_eq!(
            status, 200,
            "round {round}: expected the stored definition to stay readable, got {status} {read}"
        );
        assert!(
            contains(&declaration, &read["actionType"]),
            "round {round}: expected {declaration}, got {read}"
        );

        let (status, response) = get_in(&emulator, HOME_REGION, &undefined_id).await;
        assert_eq!(
            refusal("GetActionType", "a repeated miss", status, &response),
            (400, "ActionTypeNotFoundException".to_string()),
            "round {round}: a repeated miss must stay a miss, got {status} {response}"
        );

        assert_eq!(
            count(),
            stored_before,
            "round {round}: reading must not change how many definitions the store holds"
        );
    }
}

/// RULE: only an owner `ActionTypeIdentifier$owner` admits can be named at all, so the
/// `Custom` action types `CreateCustomActionType` mints have no definition to ask for:
/// the request is refused on the modelled pattern rather than answered, and nothing is
/// stored.
#[tokio::test]
async fn a_custom_owned_action_type_cannot_be_named() {
    let emulator = Emulator::new();
    // An action type the account really does hold — `CreateCustomActionType` mints it
    // under the `Custom` owner — so what the refusal turns on is the OWNER and not the
    // action type's existence.
    {
        let state = emulator.engine().state::<ServiceState>();
        state.lock().register_action_type(
            ActionTypeKey {
                category: "Build".to_string(),
                owner: "Custom".to_string(),
                provider: "ExampleCustom".to_string(),
                version: "1".to_string(),
            },
            ActionTypeRecord {
                input_artifact_details: ArtifactDetails {
                    minimum_count: 0,
                    maximum_count: 1,
                },
                output_artifact_details: ArtifactDetails {
                    minimum_count: 0,
                    maximum_count: 1,
                },
                region: Some(HOME_REGION.to_string()),
                ..ActionTypeRecord::default()
            },
        );
    }
    let custom_id = identifier("Build", "Custom", "ExampleCustom", "1");

    let (status, response) = get_in(&emulator, HOME_REGION, &custom_id).await;
    assert_eq!(
        refusal("GetActionType", "a Custom owner", status, &response),
        (400, "ValidationException".to_string()),
        "expected a 400 ValidationException for a Custom owner, got {status} {response}"
    );

    let (status, response) = update_in(&emulator, HOME_REGION, &definition(&custom_id, 0)).await;
    assert_eq!(
        refusal("UpdateActionType", "a Custom owner", status, &response),
        (400, "ValidationException".to_string()),
        "expected a 400 ValidationException for a Custom owner, got {status} {response}"
    );
    assert_eq!(
        emulator
            .engine()
            .state::<ServiceState>()
            .lock()
            .declared_action_type_count(),
        0,
        "expected the refused calls to have stored nothing"
    );
}
