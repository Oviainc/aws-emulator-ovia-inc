// SPDX-License-Identifier: CC0-1.0

//! Which resource a configuration-profile lookup that resolves nothing is refused as.
//!
//! The rule under test, in one sentence: an operation whose addressed resource is a
//! configuration profile refuses a path that resolves no such profile with
//! `ResourceNotFoundException` (HTTP 404) naming the CONFIGURATION PROFILE — both
//! identifiers the request supplied, echoed as the path carried them, plus the account the
//! lookup was scoped to — whichever of the two identifiers is the unresolvable one, so the
//! refusal does not disclose which of them failed to resolve.
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is the refusal an AppConfig client reads on the wire.
//!
//! The table spans what the rule generalises over rather than the one recorded lookup:
//! WHICH identifier of the path is unresolvable (the profile id, the application id, or
//! both at once), the FORM the unresolvable identifier takes (one of the minted id shape, a
//! single character, each member's own modelled `@length` maximum — `ApplicationId` targets
//! `Name` at 1..64 and `ConfigurationProfileId` targets `LongName` at 1..128,
//! `smithy_query(query="operation",operation="GetConfigurationProfile")` — a case variant
//! of a real id, and an id that resolves a resource of ANOTHER kind), whether a resource
//! carrying that identifier exists under a DIFFERENT parent, and WHICH operation does the
//! lookup: `GetConfigurationProfile`, `UpdateConfigurationProfile` and
//! `DeleteConfigurationProfile` all address a profile by the same two labels, so all three
//! are asserted to report a miss the same way.
//!
//! Each row also ends by asserting that the refusal changed nothing: the application still
//! lists exactly the profile it held and that profile still reads back.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, create_profile, json_call, router, send, Answer,
};

/// The `@length` maximum of `ApplicationId`, which the model targets at `Name`
/// (`smithy_query(query="operation",operation="GetConfigurationProfile")`).
const APPLICATION_ID_MAX_LENGTH: usize = 64;

/// The `@length` maximum of `ConfigurationProfileId`, which the model targets at `LongName`
/// (same query as [`APPLICATION_ID_MAX_LENGTH`]).
const CONFIGURATION_PROFILE_ID_MAX_LENGTH: usize = 128;

/// One operation that addresses a configuration profile by `(ApplicationId,
/// ConfigurationProfileId)`, and how to call it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reader {
    /// `GET /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}`.
    Get,
    /// `PATCH` on the same URI.
    Update,
    /// `DELETE` on the same URI.
    Delete,
}

impl Reader {
    /// The operation's name, for the assertion messages.
    fn operation(self) -> &'static str {
        match self {
            Reader::Get => "GetConfigurationProfile",
            Reader::Update => "UpdateConfigurationProfile",
            Reader::Delete => "DeleteConfigurationProfile",
        }
    }

    /// Address `(application, profile)` through this operation.
    async fn call(
        self,
        router: &MultiServiceRouter,
        application: &str,
        profile: &str,
    ) -> Answer {
        let uri = format!("/applications/{application}/configurationprofiles/{profile}");
        match self {
            Reader::Get => json_call(router, "GET", &uri, &Value::Null).await,
            Reader::Update => {
                json_call(router, "PATCH", &uri, &json!({ "Description": "patched" })).await
            }
            Reader::Delete => {
                send(router, "DELETE", &uri, "application/json", Vec::new(), &[]).await
            }
        }
    }
}

/// Every operation whose addressed resource is a configuration profile.
const READERS: [Reader; 3] = [Reader::Get, Reader::Update, Reader::Delete];

/// `GET …/configurationprofiles`, so a row can assert that a refused lookup left the
/// application's profiles exactly as they were.
async fn list_profile_ids(router: &MultiServiceRouter, application: &str) -> Vec<String> {
    let answer = json_call(
        router,
        "GET",
        &format!("/applications/{application}/configurationprofiles"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListConfigurationProfiles must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A path that resolves no configuration profile is refused as that profile, by every
/// operation addressed through one and whichever identifier of the path is unresolvable.
#[tokio::test]
async fn a_profile_lookup_that_misses_names_the_profile_whichever_identifier_is_missing() {
    let router = router();
    let account = account();
    let application = create_application(&router, "profile-lookup-subject").await;
    let profile = create_profile(&router, &application, "addressed").await;
    // A second application holding a profile of its own, so a row can name an identifier
    // that resolves — just not under the parent the request scoped the lookup to.
    let other_application = create_application(&router, "profile-lookup-elsewhere").await;
    let other_profile = create_profile(&router, &other_application, "elsewhere").await;

    let long_application_id = "a".repeat(APPLICATION_ID_MAX_LENGTH);
    let long_profile_id = "p".repeat(CONFIGURATION_PROFILE_ID_MAX_LENGTH);

    // (case, the ApplicationId the request carries, the ConfigurationProfileId it carries).
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a profile id of the minted shape that was never minted",
            application.clone(),
            "nope123".to_string(),
        ),
        (
            "a profile id of one character",
            application.clone(),
            "p".to_string(),
        ),
        (
            "a profile id at the member's length maximum",
            application.clone(),
            long_profile_id.clone(),
        ),
        (
            "a profile id differing from the real one only in case",
            application.clone(),
            profile.to_uppercase(),
        ),
        (
            "a profile id that belongs to another application",
            application.clone(),
            other_profile.clone(),
        ),
        (
            "an application id that was never minted",
            "zzzzzzz".to_string(),
            profile.clone(),
        ),
        (
            "an application id of one character",
            "a".to_string(),
            profile.clone(),
        ),
        (
            "an application id at the member's length maximum",
            long_application_id.clone(),
            profile.clone(),
        ),
        (
            "an application id differing from the real one only in case",
            application.to_uppercase(),
            profile.clone(),
        ),
        (
            "an application id that is really the profile's own id",
            profile.clone(),
            profile.clone(),
        ),
        (
            "a real profile read through another application",
            other_application.clone(),
            profile.clone(),
        ),
        (
            "neither identifier resolves",
            "zzzzzzz".to_string(),
            "nope123".to_string(),
        ),
    ];

    for (case, application_id, configuration_profile_id) in &rows {
        let expected = format!(
            "Configuration Profile with Application Id {application_id} and Configuration \
             Profile Id {configuration_profile_id} could not be found for account {account}."
        );
        for reader in READERS {
            let answer = reader
                .call(&router, application_id, configuration_profile_id)
                .await;
            let operation = reader.operation();
            assert_eq!(
                answer.status,
                404,
                "case {case:?}: {operation} must answer 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.string("Message"),
                Some(expected.clone()),
                "case {case:?}: {operation} must answer the configuration profile template, \
                 got {:?}",
                answer.string("Message")
            );
        }

        // None of the three refusals created or removed anything: the application still
        // holds exactly the profile it was given, and that profile still reads back.
        assert_eq!(
            list_profile_ids(&router, &application).await,
            vec![profile.clone()],
            "case {case:?}: the refusals must leave the application's profiles alone"
        );
        let held = Reader::Get.call(&router, &application, &profile).await;
        assert_eq!(
            held.status,
            200,
            "case {case:?}: the profile the application holds must still be reported, got {} / {}",
            held.status,
            String::from_utf8_lossy(&held.body)
        );
    }
}

/// One past a path member's modelled `@length` maximum is a CONSTRAINT violation, not a
/// lookup: the request never addresses a resource, so it is refused as a bad request
/// rather than with the not-found template asserted above.
#[tokio::test]
async fn an_identifier_past_its_length_maximum_is_a_bad_request() {
    let router = router();
    let application = create_application(&router, "profile-lookup-bounds").await;
    let profile = create_profile(&router, &application, "bounded").await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "an application id one character past its maximum",
            "a".repeat(APPLICATION_ID_MAX_LENGTH + 1),
            profile.clone(),
        ),
        (
            "a profile id one character past its maximum",
            application.clone(),
            "p".repeat(CONFIGURATION_PROFILE_ID_MAX_LENGTH + 1),
        ),
    ];

    for (case, application_id, configuration_profile_id) in &rows {
        for reader in READERS {
            let answer = reader
                .call(&router, application_id, configuration_profile_id)
                .await;
            let operation = reader.operation();
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: {operation} must answer 400, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("BadRequestException"),
                "case {case:?}: {operation} must answer BadRequestException, got {:?}",
                answer.error_code()
            );
        }
    }
}
