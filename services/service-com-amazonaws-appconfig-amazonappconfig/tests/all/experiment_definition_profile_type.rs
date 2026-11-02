// SPDX-License-Identifier: CC0-1.0

//! The configuration profile an experiment definition may be bound to.
//!
//! RULE: an experiment definition runs ONE feature flag of one configuration profile —
//! `ConfigurationProfileIdentifier` is "the configuration profile ID or name that stores
//! the feature flag" and `FlagKey` "the key of the existing feature flag"
//! (`smithy_query(query="operation", operation="CreateExperimentDefinition")`) — so the
//! profile the create resolves has to be one that stores feature flags, a profile of
//! `state::FEATURE_FLAGS_PROFILE_TYPE`. A profile of any other `Type` holds configuration
//! data in which no flag key can be addressed, and the create is refused with
//! `BadRequestException` (HTTP 400) having written nothing: no definition is listed, and
//! none can be read back.
//!
//! The rows span the profile `Type` the create resolves: every entry of the catalogue
//! `CreateConfigurationProfile` accepts (`state::CONFIGURATION_PROFILE_TYPES`, which is
//! the model's `ConfigurationProfileType` catalogue — anything outside it is already
//! refused at create time, as `configuration_profile_type` asserts), plus a profile
//! created with no `Type` at all, which AppConfig stores as the free-form Type. Each row
//! runs twice, with the profile addressed by its ID and by its NAME, because the
//! identifier resolves either form and the precondition is about the RESOLVED profile —
//! which is also why the refusal names the resolved id rather than the submitted
//! identifier.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    configuration_profile_not_for_experiments, CONFIGURATION_PROFILE_TYPES,
    FEATURE_FLAGS_PROFILE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile_from, json_call, router,
};
use crate::experiment_definitions::{create_definition, flag, get_definition, treatment};

/// One profile the rule is asserted over: how a row names it, the `Type` the create
/// declared (`None` for a create that declared none) and the id it was stored under.
struct Profile {
    case: String,
    declared: Option<String>,
    id: String,
    name: String,
}

/// Whether a profile created with `declared` stores feature flags: the Type it was
/// created with, or the free-form Type AppConfig stores when a create declares none.
fn stores_feature_flags(declared: Option<&str>) -> bool {
    declared == Some(FEATURE_FLAGS_PROFILE_TYPE)
}

/// Every profile `Type` the rule is spanned over: the whole catalogue a create accepts,
/// and the absent Type.
fn declared_types() -> Vec<Option<String>> {
    let mut declared: Vec<Option<String>> = CONFIGURATION_PROFILE_TYPES
        .iter()
        .map(|profile_type| Some((*profile_type).to_string()))
        .collect();
    declared.push(None);
    declared
}

/// Create one hosted profile per entry of [`declared_types`] under `application`.
async fn profiles(router: &MultiServiceRouter, application: &str) -> Vec<Profile> {
    let mut created = Vec::new();
    for (index, declared) in declared_types().into_iter().enumerate() {
        let case = match &declared {
            Some(profile_type) => format!("a profile of Type {profile_type}"),
            None => "a profile created with no Type".to_string(),
        };
        let name = format!("profile-{index}");
        let mut body = json!({ "Name": name, "LocationUri": "hosted" });
        if let Some(profile_type) = &declared {
            body["Type"] = Value::from(profile_type.clone());
        }
        let answer = create_profile_from(router, application, &case, &body).await;
        let id = answer
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: CreateConfigurationProfile answered no Id"));
        created.push(Profile {
            case,
            declared,
            id,
            name,
        });
    }
    created
}

/// The `Name` of every definition `ListExperimentDefinitions` reports under
/// `application`.
async fn listed_names(router: &MultiServiceRouter, application: &str) -> BTreeSet<String> {
    let listing = json_call(
        router,
        "GET",
        &format!("/experimentdefinitions?application_identifier={application}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        listing.status,
        200,
        "ListExperimentDefinitions must answer 200, got {} / {}",
        listing.status,
        String::from_utf8_lossy(&listing.body)
    );
    listing
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A create against each profile Type in the catalogue, addressed both ways: only the
/// feature-flags profile is bound to a definition, and every other Type is refused with
/// the modelled `BadRequestException` and leaves no record behind.
#[tokio::test]
async fn only_a_feature_flags_profile_can_be_bound_to_an_experiment_definition() {
    let router = router();
    let application = create_application(&router, "experiment-profile-type").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profiles = profiles(&router, &application).await;

    let mut expected_names: BTreeSet<String> = BTreeSet::new();
    for profile in &profiles {
        // Both forms the identifier resolves: the id the profile was stored under, and
        // the name it was created with.
        for (form, identifier) in [("by id", &profile.id), ("by name", &profile.name)] {
            let case = format!("{} {form}", profile.case);
            let name = format!("definition-{}", case.replace(' ', "-"));
            let created = create_definition(
                &router,
                &application,
                &json!({
                    "Name": name,
                    "ConfigurationProfileIdentifier": identifier,
                    "EnvironmentIdentifier": environment,
                    "FlagKey": "checkout-flow",
                    "AudienceRule": "attribute(\"tier\") == \"beta\"",
                    "Treatments": [treatment(50.0, None, flag(true))],
                    "Control": treatment(50.0, None, flag(false)),
                }),
            )
            .await;

            if stores_feature_flags(profile.declared.as_deref()) {
                assert_eq!(
                    created.status,
                    201,
                    "case {case:?}: a definition on a feature-flags profile must be created, \
                     got {} / {}",
                    created.status,
                    String::from_utf8_lossy(&created.body)
                );
                // The record really persisted, and it holds the RESOLVED profile id
                // whichever form the request named it by.
                let id = created
                    .string("Id")
                    .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id"));
                let read_back = get_definition(&router, &application, &id).await;
                assert_eq!(
                    read_back.status,
                    200,
                    "case {case:?}: GetExperimentDefinition must answer 200, got {} / {}",
                    read_back.status,
                    String::from_utf8_lossy(&read_back.body)
                );
                assert_eq!(
                    read_back.string("ConfigurationProfileId").as_deref(),
                    Some(profile.id.as_str()),
                    "case {case:?}: expected the resolved profile id on the stored record, \
                     got {:?}",
                    read_back.string("ConfigurationProfileId")
                );
                expected_names.insert(name.clone());
            } else {
                assert_eq!(
                    created.status,
                    400,
                    "case {case:?}: a definition on a profile that stores no feature flags \
                     must be refused, got {} / {}",
                    created.status,
                    String::from_utf8_lossy(&created.body)
                );
                assert!(
                    created.error_code().contains("BadRequestException"),
                    "case {case:?}: expected BadRequestException, got {:?}",
                    created.error_code()
                );
                // The refusal states the profile AppConfig resolved, so the id is there
                // even when the request addressed the profile by name.
                assert_eq!(
                    created.string("Message"),
                    Some(configuration_profile_not_for_experiments(&profile.id)),
                    "case {case:?}: expected the profile-type refusal, got {:?}",
                    created.string("Message")
                );
                // Nothing was written: the name the refused create carried addresses no
                // definition.
                let missing = get_definition(&router, &application, &name).await;
                assert_eq!(
                    missing.status,
                    404,
                    "case {case:?}: the refused definition must not be readable, got {} / {}",
                    missing.status,
                    String::from_utf8_lossy(&missing.body)
                );
            }

            // Whatever the row expected, the listing holds exactly the definitions the
            // accepted creates made — so a refusal adds nothing even while other
            // definitions exist under the same application.
            assert_eq!(
                listed_names(&router, &application).await,
                expected_names,
                "case {case:?}: ListExperimentDefinitions must report only the definitions \
                 that were created"
            );
        }
    }

    // The span was really exercised: at least one Type was accepted and at least one
    // refused, so neither branch of the rule went untested.
    assert!(
        !expected_names.is_empty(),
        "no profile Type in the catalogue accepted a definition, so the positive half of \
         the rule was never exercised"
    );
    assert!(
        profiles
            .iter()
            .any(|profile| !stores_feature_flags(profile.declared.as_deref())),
        "no profile Type in the catalogue was refused, so the negative half of the rule \
         was never exercised"
    );
}
