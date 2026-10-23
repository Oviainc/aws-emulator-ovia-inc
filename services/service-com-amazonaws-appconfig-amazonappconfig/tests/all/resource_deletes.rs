// SPDX-License-Identifier: CC0-1.0

//! `DeleteDeploymentStrategy`, `DeleteEnvironment` and `DeleteExtension`.
//!
//! The rule under test, in one sentence: a delete that answers its modelled 204 has taken
//! the addressed record out of this service's state, so every later call naming it — a
//! reader, a writer, the tagging reader, a second delete — is refused with
//! `ResourceNotFoundException` (HTTP 404, `x-amzn-errortype` set), while nothing else the
//! store holds is disturbed.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire.
//!
//! The tables span what the rule generalises over rather than one recorded resource:
//!
//! * for a strategy, the criteria it carries (each `GrowthType`, the published bounds of
//!   `MinutesBetween0And24Hours` 0..1440 and `Percentage` 1..100), whether it carries
//!   tags, and the length of its `Name`;
//! * for an environment, which optional members it carries, how many monitors, whether
//!   anything was deployed into it, and each value of the `DeletionProtectionCheck` header
//!   the model's enum declares;
//! * for an extension, each identifier form the member documents (id, name, version ARN)
//!   and each way a version is selected (omitted — the documented highest — an explicit
//!   number, and the one an ARN pins).
//!
//! AppConfig's own predefined `AppConfig.*` strategies are account-visible but not
//! account-owned, so the suite also pins that an attempted delete of one removes nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::PREDEFINED_DEPLOYMENT_STRATEGIES;

use crate::appconfig_workflow::{
    create_application, create_profile, json_call, router, send, Answer,
};
use crate::resource_tags::{arn, filler, list_tags, pct};

/// Where a deployment strategy is created and read (`POST`/`GET`
/// `/deploymentstrategies`).
const STRATEGIES: &str = "/deploymentstrategies";

/// Where a deployment strategy is DELETED. The model spells the delete route
/// `/deployementstrategies/{DeploymentStrategyId}` while the reader's is
/// `/deploymentstrategies/{DeploymentStrategyId}`
/// (`smithy_query(query="operation",operation="DeleteDeploymentStrategy")`), so the two
/// are named separately here rather than derived from one string.
const STRATEGY_DELETES: &str = "/deployementstrategies";

/// The documented upper bound of `MinutesBetween0And24Hours` (range 0..1440).
const MAX_MINUTES: i64 = 1440;

/// The documented upper bound of `Name` (length 1..64).
const MAX_NAME: usize = 64;

/// One `DELETE` with no body, optionally carrying bound headers.
async fn delete(router: &MultiServiceRouter, uri: &str, headers: &[(&str, String)]) -> Answer {
    send(router, "DELETE", uri, "application/json", Vec::new(), headers).await
}

/// A delete that must have succeeded: the modelled 204, with no body.
fn assert_deleted(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: {operation} must answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: {operation} answers no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// A call that must miss: the modelled `ResourceNotFoundException`, HTTP 404, with the
/// error type on the wire where an SDK classifies it from.
fn assert_not_found(case: &str, operation: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: {operation} must answer 404 for a resource that is gone, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
        answer.error_code()
    );
    assert!(
        answer
            .header("x-amzn-errortype")
            .is_some_and(|value| value.contains("ResourceNotFoundException")),
        "case {case:?}: {operation} must set x-amzn-errortype, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert!(
        answer
            .string("Message")
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: {operation} must carry a Message, got {}",
        answer.json()
    );
}

/// `GET` one deployment strategy.
async fn get_strategy(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(router, "GET", &format!("{STRATEGIES}/{id}"), &Value::Null).await
}

/// `GET` one environment.
async fn get_environment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await
}

/// `GET /extensions/{ExtensionIdentifier}[?version_number=..]`. The identifier is bound
/// to a NON-greedy `@httpLabel`, so an ARN travels percent-encoded.
async fn get_extension(
    router: &MultiServiceRouter,
    identifier: &str,
    version_number: Option<i32>,
) -> Answer {
    let uri = match version_number {
        Some(number) => format!("/extensions/{}?version_number={number}", pct(identifier)),
        None => format!("/extensions/{}", pct(identifier)),
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `DELETE /extensions/{ExtensionIdentifier}[?version=..]` — `DeleteExtension` binds its
/// version to the `version` query parameter, not to `GetExtension`'s `version_number`
/// (`smithy_query(query="operation",operation="DeleteExtension")`).
async fn delete_extension(
    router: &MultiServiceRouter,
    identifier: &str,
    version_number: Option<i32>,
) -> Answer {
    let uri = match version_number {
        Some(number) => format!("/extensions/{}?version={number}", pct(identifier)),
        None => format!("/extensions/{}", pct(identifier)),
    };
    delete(router, &uri, &[]).await
}

/// Host one configuration version under `profile`, so a deployment has something to roll
/// out.
async fn host_configuration(router: &MultiServiceRouter, application: &str, profile: &str) {
    let hosted = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );
}

/// `POST` a deployment of `profile` version 1 into `environment`.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await
}

/// The `Id` of a created resource, asserted present.
fn created_id(case: &str, operation: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: {operation} must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: {operation} answered no Id: {}", answer.json()))
}

/// A deleted deployment strategy is gone: the reader, the tag reader, `StartDeployment`
/// and a second delete all miss on its id.
///
/// Rows differ along what the rule generalises over — each growth type, both published
/// bounds of the duration and the growth factor, the name's upper bound, and whether the
/// strategy carries tags — because the rule is about the record's removal and not about
/// any one strategy's criteria.
#[tokio::test]
async fn a_deleted_deployment_strategy_is_gone_from_every_caller() {
    let router = router();

    // One tree to deploy into, so the "a deleted strategy is no longer deployable" half of
    // the rule is asserted through `StartDeployment` as well as through the reader.
    let application = create_application(&router, "strategy-delete-app").await;
    let profile = create_profile(&router, &application, "strategy-delete-profile").await;
    host_configuration(&router, &application, &profile).await;
    let environment = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments"),
        &json!({ "Name": "strategy-delete-env" }),
    )
    .await;
    let environment = created_id("fixture", "CreateEnvironment", &environment);

    let rows: Vec<(&str, Value)> = vec![
        (
            "linear, every member stated, tagged",
            json!({
                "Name": "linear-tagged",
                "Description": "a flat rollout",
                "DeploymentDurationInMinutes": 5,
                "FinalBakeTimeInMinutes": 1,
                "GrowthFactor": 20,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
                "Tags": { "team": "platform" },
            }),
        ),
        (
            "exponential growth at the lower minute bound",
            json!({
                "Name": "exponential-instant",
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 10,
                "GrowthType": "EXPONENTIAL",
            }),
        ),
        (
            "every criterion at its published upper bound",
            json!({
                "Name": "at-the-bounds",
                "DeploymentDurationInMinutes": MAX_MINUTES,
                "FinalBakeTimeInMinutes": MAX_MINUTES,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
            }),
        ),
        (
            "name at the 64-character bound, every optional member absent",
            json!({
                "Name": filler(MAX_NAME, 1),
                "DeploymentDurationInMinutes": 15,
                "GrowthFactor": 1,
            }),
        ),
    ];

    for (case, request) in rows {
        let created = json_call(&router, "POST", STRATEGIES, &request).await;
        let id = created_id(case, "CreateDeploymentStrategy", &created);
        let strategy_arn = arn(&format!("deploymentstrategy/{id}"));

        // It is really there first, so what the delete changes is not an absence that was
        // there all along.
        let before = get_strategy(&router, &id).await;
        assert_eq!(
            before.status,
            200,
            "case {case:?}: GetDeploymentStrategy must report the created strategy, got {} / {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );
        let tagged_before = list_tags(&router, &strategy_arn).await;
        assert_eq!(
            tagged_before.status,
            200,
            "case {case:?}: ListTagsForResource must report the created strategy, got {} / {}",
            tagged_before.status,
            String::from_utf8_lossy(&tagged_before.body)
        );

        let deleted = delete(&router, &format!("{STRATEGY_DELETES}/{id}"), &[]).await;
        assert_deleted(case, "DeleteDeploymentStrategy", &deleted);

        assert_not_found(case, "GetDeploymentStrategy", &get_strategy(&router, &id).await);
        assert_not_found(
            case,
            "ListTagsForResource",
            &list_tags(&router, &strategy_arn).await,
        );
        assert_not_found(
            case,
            "StartDeployment naming the deleted strategy",
            &start_deployment(&router, &application, &environment, &profile, &id).await,
        );
        assert_not_found(
            case,
            "a second DeleteDeploymentStrategy",
            &delete(&router, &format!("{STRATEGY_DELETES}/{id}"), &[]).await,
        );
    }
}

/// AppConfig's predefined `AppConfig.*` strategies are deployable without a create call
/// and belong to the account of nobody: an attempted delete removes nothing, so it is
/// refused with the modelled `BadRequestException` (400) and the strategy still reads
/// back exactly as it did.
///
/// The catalog is `state::PREDEFINED_DEPLOYMENT_STRATEGIES`, from the AppConfig User
/// Guide's "Predefined deployment strategies" table, and every row is walked — including
/// the ones no recorded request names.
#[tokio::test]
async fn a_predefined_deployment_strategy_cannot_be_deleted() {
    let router = router();

    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let before = get_strategy(&router, predefined.id).await;
        assert_eq!(
            before.status,
            200,
            "case {case:?}: GetDeploymentStrategy must report the predefined strategy, got {} / {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let refused = delete(
            &router,
            &format!("{STRATEGY_DELETES}/{}", predefined.id),
            &[],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: deleting a predefined strategy must be refused 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );

        let after = get_strategy(&router, predefined.id).await;
        assert_eq!(
            after.status,
            200,
            "case {case:?}: a refused delete leaves the strategy readable, got {} / {}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
        assert_eq!(
            after.json(),
            before.json(),
            "case {case:?}: a refused delete must change nothing, got {}",
            after.json()
        );
    }
}

/// One row of the environment table.
struct EnvironmentRow {
    case: &'static str,
    /// The `CreateEnvironment` body.
    body: Value,
    /// Whether a deployment is started into it before it is deleted.
    deploy: bool,
    /// The `DeletionProtectionCheck` the delete sends, bound to
    /// `x-amzn-deletion-protection-check`.
    check: Option<&'static str>,
}

/// One CloudWatch alarm monitor. The member names are the model's own
/// (`smithy_query(query="shape",shape="Monitor")`: `AlarmArn` required, `AlarmRoleArn`).
fn monitor(index: usize) -> Value {
    json!({
        "AlarmArn": format!("arn:aws:cloudwatch:us-east-1:123456789012:alarm:appconfig-{index}"),
        "AlarmRoleArn": "arn:aws:iam::123456789012:role/appconfig-monitor",
    })
}

/// A deleted environment is gone, with the deployments made into it: the reader misses,
/// the WRITER is refused rather than resurrecting it, the tag reader misses, a second
/// delete misses — and every sibling environment is untouched.
///
/// Rows differ along which optional members the environment carries, how many monitors,
/// whether anything was deployed into it, and which `DeletionProtectionCheck` the delete
/// asks for (every value the model's enum declares, plus the header omitted).
#[tokio::test]
async fn a_deleted_environment_is_gone_with_its_deployments() {
    let router = router();
    let application = create_application(&router, "env-delete-app").await;
    let profile = create_profile(&router, &application, "env-delete-profile").await;
    host_configuration(&router, &application, &profile).await;

    let rows = vec![
        EnvironmentRow {
            case: "bare environment, no deletion-protection header",
            body: json!({ "Name": "bare" }),
            deploy: false,
            check: None,
        },
        EnvironmentRow {
            case: "description and the five monitors the MonitorList bound allows",
            body: json!({
                "Name": "monitored",
                "Description": "watched by every alarm",
                "Monitors": (0..5).map(monitor).collect::<Vec<_>>(),
            }),
            deploy: false,
            check: Some("ACCOUNT_DEFAULT"),
        },
        EnvironmentRow {
            case: "an environment with a deployment history",
            body: json!({ "Name": "deployed", "Description": "" }),
            deploy: true,
            check: Some("APPLY"),
        },
        EnvironmentRow {
            case: "name at the 64-character bound",
            body: json!({ "Name": filler(MAX_NAME, 7) }),
            deploy: false,
            check: Some("BYPASS"),
        },
    ];

    // Every row is created first, so deleting one can be shown to leave the others alone.
    let mut created = Vec::new();
    for row in &rows {
        let answer = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments"),
            &row.body,
        )
        .await;
        let id = created_id(row.case, "CreateEnvironment", &answer);
        if row.deploy {
            let started =
                start_deployment(&router, &application, &id, &profile, "AppConfig.AllAtOnce").await;
            assert_eq!(
                started.status,
                201,
                "case {:?}: StartDeployment must answer 201, got {} / {}",
                row.case,
                started.status,
                String::from_utf8_lossy(&started.body)
            );
        }
        created.push((row, id));
    }

    for (index, (row, environment)) in created.iter().enumerate() {
        let case = row.case;
        let uri = format!("/applications/{application}/environments/{environment}");
        let environment_arn = arn(&format!(
            "application/{application}/environment/{environment}"
        ));

        let before = get_environment(&router, &application, environment).await;
        assert_eq!(
            before.status,
            200,
            "case {case:?}: GetEnvironment must report the created environment, got {} / {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );

        let headers: Vec<(&str, String)> = row
            .check
            .map(|check| vec![("x-amzn-deletion-protection-check", check.to_string())])
            .unwrap_or_default();
        let deleted = delete(&router, &uri, &headers).await;
        assert_deleted(case, "DeleteEnvironment", &deleted);

        assert_not_found(
            case,
            "GetEnvironment",
            &get_environment(&router, &application, environment).await,
        );

        // A WRITE against a deleted environment is refused, and applies nothing: the read
        // after it still misses, so the update did not resurrect the record.
        let updated = json_call(
            &router,
            "PATCH",
            &uri,
            &json!({ "Description": "written to a deleted environment" }),
        )
        .await;
        assert_not_found(case, "UpdateEnvironment", &updated);
        assert_not_found(
            case,
            "GetEnvironment after the refused UpdateEnvironment",
            &get_environment(&router, &application, environment).await,
        );

        assert_not_found(
            case,
            "ListTagsForResource",
            &list_tags(&router, &environment_arn).await,
        );
        assert_not_found(case, "a second DeleteEnvironment", &delete(&router, &uri, &[]).await);

        // The deployment history went with the environment that owned it.
        if row.deploy {
            assert_not_found(
                case,
                "GetDeployment",
                &json_call(&router, "GET", &format!("{uri}/deployments/1"), &Value::Null).await,
            );
        }

        // Deleting one environment removes exactly one: every row not yet deleted is
        // still readable.
        for (other, other_id) in created.iter().skip(index + 1) {
            let sibling = get_environment(&router, &application, other_id).await;
            assert_eq!(
                sibling.status,
                200,
                "case {case:?}: deleting it must leave {:?} readable, got {} / {}",
                other.case,
                sibling.status,
                String::from_utf8_lossy(&sibling.body)
            );
        }
    }

    // An environment under an application that does not exist, and one under a real
    // application that never had it: both name no environment, so both miss.
    assert_not_found(
        "unknown application",
        "DeleteEnvironment",
        &delete(&router, "/applications/zzzz999/environments/zzzz998", &[]).await,
    );
    assert_not_found(
        "unknown environment under a real application",
        "DeleteEnvironment",
        &delete(
            &router,
            &format!("/applications/{application}/environments/zzzz998"),
            &[],
        )
        .await,
    );
}

/// The three versions of one extension a row operates on, with the ARN each version was
/// minted with (index 0 is version 1).
struct Versions {
    id: String,
    name: String,
    arns: Vec<String>,
}

/// `POST /extensions`, appending a version when `latest_version_number` names the name's
/// current latest (the member is an optimistic lock bound to the `Latest-Version-Number`
/// header).
async fn create_extension(
    router: &MultiServiceRouter,
    name: &str,
    latest_version_number: Option<i32>,
) -> Answer {
    let body = json!({
        "Name": name,
        "Actions": {
            "PRE_START_DEPLOYMENT": [{
                "Name": "notify",
                "Description": "the action bound to the point",
                "Uri": arn("function:notify"),
            }],
        },
        "Tags": { "owner": "platform" },
    });
    let headers: Vec<(&str, String)> = latest_version_number
        .map(|number| vec![("Latest-Version-Number", number.to_string())])
        .unwrap_or_default();
    send(
        router,
        "POST",
        "/extensions",
        "application/json",
        serde_json::to_vec(&body).expect("the test request body must serialize"),
        &headers,
    )
    .await
}

/// Three versions of one extension, created through `CreateExtension`'s optimistic lock.
async fn three_versions(router: &MultiServiceRouter, case: &str, name: &str) -> Versions {
    let mut arns = Vec::new();
    let mut id = String::new();
    for version in 1..=3 {
        let previous = (version > 1).then_some(version - 1);
        let answer = create_extension(router, name, previous).await;
        id = created_id(case, "CreateExtension", &answer);
        assert_eq!(
            answer.json().get("VersionNumber").and_then(Value::as_i64),
            Some(i64::from(version)),
            "case {case:?}: CreateExtension must answer version {version}, got {}",
            answer.json()
        );
        arns.push(
            answer
                .string("Arn")
                .unwrap_or_else(|| panic!("case {case:?}: CreateExtension answered no Arn")),
        );
    }
    Versions {
        id,
        name: name.to_string(),
        arns,
    }
}

/// The version `GetExtension` reports for an identifier naming no version.
async fn reported_version(router: &MultiServiceRouter, case: &str, identifier: &str) -> i64 {
    let answer = get_extension(router, identifier, None).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetExtension({identifier:?}) expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("VersionNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: GetExtension answered no VersionNumber"))
}

/// `DeleteExtension` removes ONE version — the highest when the request names none, the
/// one the request names otherwise — and the extension itself once no version is left.
///
/// Rows differ along the identifier form the delete uses (the id, the name and a version
/// ARN: the three forms `ExtensionIdentifier` documents) and along how the version is
/// selected (omitted, pinned by the ARN, or stated explicitly — which overrides the ARN).
#[tokio::test]
async fn deleting_an_extension_version_removes_that_version_and_the_last_removes_the_extension() {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, fn(&Versions) -> (String, Option<i32>), i64)> = vec![
        (
            "no version named, addressed by id: the highest goes",
            |versions| (versions.id.clone(), None),
            3,
        ),
        (
            "no version named, addressed by name: the highest goes",
            |versions| (versions.name.clone(), None),
            3,
        ),
        (
            "the version an ARN pins",
            |versions| (versions.arns[1].clone(), None),
            2,
        ),
        (
            "an explicit version number",
            |versions| (versions.id.clone(), Some(1)),
            1,
        ),
        (
            "an explicit version number overrides the one the ARN pins",
            |versions| (versions.arns[2].clone(), Some(1)),
            1,
        ),
    ];

    for (case, address, removed) in rows {
        // A fresh store per row: the rule is about one extension's versions, and a row
        // must not read what another row left behind.
        let router = router();
        let versions = three_versions(&router, case, "versioned-extension").await;
        let (identifier, version_number) = address(&versions);

        let deleted = delete_extension(&router, &identifier, version_number).await;
        assert_deleted(case, "DeleteExtension", &deleted);

        // The named version is gone, and its tags with it.
        assert_not_found(
            case,
            "GetExtension naming the deleted version",
            &get_extension(&router, &versions.id, Some(removed as i32)).await,
        );
        assert_not_found(
            case,
            "ListTagsForResource on the deleted version's ARN",
            &list_tags(&router, &versions.arns[(removed - 1) as usize]).await,
        );

        // Every other version is untouched, and a reader naming none gets the highest one
        // still held.
        let remaining: Vec<i64> = (1..=3).filter(|number| *number != removed).collect();
        for number in &remaining {
            let held = get_extension(&router, &versions.id, Some(*number as i32)).await;
            assert_eq!(
                held.status,
                200,
                "case {case:?}: version {number} must survive, got {} / {}",
                held.status,
                String::from_utf8_lossy(&held.body)
            );
        }
        let highest = remaining.iter().copied().max().expect("two versions remain");
        assert_eq!(
            reported_version(&router, case, &versions.id).await,
            highest,
            "case {case:?}: a reader naming no version must get the highest version left"
        );

        // Emptying the extension one version at a time removes the extension itself: the
        // id, the name and every ARN it ever minted then all miss.
        for number in &remaining {
            let further = delete_extension(&router, &versions.id, Some(*number as i32)).await;
            assert_deleted(case, "DeleteExtension of a remaining version", &further);
        }
        assert_not_found(
            case,
            "GetExtension by id",
            &get_extension(&router, &versions.id, None).await,
        );
        assert_not_found(
            case,
            "GetExtension by name",
            &get_extension(&router, &versions.name, None).await,
        );
        for arn in &versions.arns {
            assert_not_found(
                case,
                "GetExtension by version ARN",
                &get_extension(&router, arn, None).await,
            );
        }
        assert_not_found(
            case,
            "a further DeleteExtension",
            &delete_extension(&router, &versions.id, None).await,
        );
    }
}

/// A delete that names no version this service holds, or no extension at all, removes
/// nothing and says so.
#[tokio::test]
async fn deleting_an_extension_version_that_does_not_exist_removes_nothing() {
    let router = router();
    let case = "a version the extension never held";
    let versions = three_versions(&router, case, "intact-extension").await;

    // Rows differ along how the absent version is named: one past the highest, one below
    // the first, and an identifier nothing answers to at all.
    assert_not_found(
        case,
        "DeleteExtension naming version 4",
        &delete_extension(&router, &versions.id, Some(4)).await,
    );
    assert_not_found(
        "a version number below the first",
        "DeleteExtension naming version 0",
        &delete_extension(&router, &versions.name, Some(0)).await,
    );
    assert_not_found(
        "an identifier nothing answers to",
        "DeleteExtension",
        &delete_extension(&router, "no-such-extension", None).await,
    );

    // Nothing was removed: all three versions still read back.
    for number in 1..=3 {
        let held = get_extension(&router, &versions.id, Some(number)).await;
        assert_eq!(
            held.status,
            200,
            "case {case:?}: a refused delete must leave version {number} readable, got {} / {}",
            held.status,
            String::from_utf8_lossy(&held.body)
        );
    }
}
