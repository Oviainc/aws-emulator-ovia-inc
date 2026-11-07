// SPDX-License-Identifier: CC0-1.0

//! Cover for the three rules a project's own identity and source configuration are held
//! to, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! * A source that HAS input source code must say where that code is, so a project that
//!   could never resolve its source is refused rather than stored.
//! * A project is addressed by NAME or by its own ARN wherever an operation takes a
//!   project identifier, and an identifier that resolves to nothing is reported back
//!   exactly as the request sent it.
//! * `InvalidateProjectCache` resets the cache of a project this account holds, and
//!   refuses a name it holds none under — leaving the project's own settings alone.
//!
//! Each table spans the class its rule is about: the model's whole `SourceType`
//! catalogue, both identifier forms plus the ARNs that name no local project, and a
//! project with each kind of cache the model declares.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SOURCE_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project, create_project_body, emulator, emulator_on, error_type, expected_arn,
    REGION,
};

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The project the account holds under `name`, read back through `BatchGetProjects`.
async fn read_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// RULE: `ProjectSource$location` is where the source code to be built lives, and the
/// model states when it is required: "If you specify CODEPIPELINE for the Type property,
/// don't specify this property. For all of the other types, you must specify Location."
/// (`NO_SOURCE` is the other type with no source code to locate.) A source that states
/// none is refused with 400 `InvalidInputException` worded `Invalid source: location is
/// required` — fixed text that quotes nothing of the request — wherever a project's
/// source is SET, and nothing is stored.
///
/// The table walks `SourceType`'s whole modelled value list
/// (`ops::types::SOURCE_TYPE_VALUES`), so it covers the types the issue never cited as
/// well as the one it did, in both forms of absence the service reads as absent (omitted
/// and all whitespace), on `CreateProject` and on `UpdateProject`.
#[tokio::test]
async fn a_source_that_has_source_code_must_state_its_location() {
    let router = emulator();

    // The two types that carry their source from somewhere other than a location of
    // their own, named by the service rather than spelled here.
    let needs_no_location = [state::NO_SOURCE_TYPE, state::CODEPIPELINE_SOURCE_TYPE];

    for source_type in SOURCE_TYPE_VALUES {
        // Fixed text: CodeBuild names the member of the configuration that is incomplete
        // and the member missing from it, and quotes nothing of the request, so the
        // sentence is the same for every source type the rule covers.
        let expected = state::SOURCE_LOCATION_REQUIRED_MESSAGE;
        // (case, the source the request sets)
        let absent: [(&str, Value); 2] = [
            (
                "location omitted",
                json!({"type": source_type, "buildspec": "version: 0.2"}),
            ),
            (
                "location all whitespace",
                json!({"type": source_type, "buildspec": "version: 0.2", "location": "  \n\t "}),
            ),
        ];

        for (form, source) in absent {
            let case = format!("{source_type}, {form}");
            let name = format!(
                "loc-{}-{}",
                source_type.to_lowercase(),
                form.replace(' ', "-")
            );
            let (status, body) = call(
                &router,
                "CreateProject",
                create_project_body(&name, json!({"source": source.clone()})),
            )
            .await;

            if needs_no_location.contains(source_type) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a source with no source code of its own needs no \
                     location, got {status} with {body}"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from CreateProject, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected CodeBuild's location sentence, got {body}"
            );
            assert!(
                !message(&body).contains(source_type) && !message(&body).contains(&name),
                "case {case:?}: the sentence quotes nothing of the request, got {body}"
            );

            // Nothing was stored for the refused request.
            let (status, read) = call(&router, "BatchGetProjects", json!({"names": [&name]})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchGetProjects: expected 200, got {status} with {read}"
            );
            assert_eq!(
                read["projectsNotFound"],
                json!([name.clone()]),
                "case {case:?}: a refused create must store nothing, got {read}"
            );

            // The same source WITH a location is accepted and stored.
            let located = format!("{name}-ok");
            let mut accepted = source.clone();
            if let Value::Object(members) = &mut accepted {
                members.insert(
                    "location".to_string(),
                    json!("https://example.test/repo/source"),
                );
            }
            let stored = create_project(
                &router,
                &located,
                json!({"source": accepted.clone()}),
            )
            .await;
            assert_eq!(
                stored["source"]["location"],
                json!("https://example.test/repo/source"),
                "case {case:?}: the stored source must carry the location, got {stored}"
            );

            // And the rule holds on an UPDATE that would empty it: the project keeps the
            // source it had.
            let (status, body) = call(
                &router,
                "UpdateProject",
                json!({"name": located, "source": source}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} on UpdateProject: expected 400, got {status} with {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?} on UpdateProject: expected CodeBuild's location sentence, got {body}"
            );
            let kept = read_project(&router, &located).await;
            assert_eq!(
                kept["source"]["location"],
                json!("https://example.test/repo/source"),
                "case {case:?}: a refused update must leave the source as it was, got {kept}"
            );
        }
    }
}

/// One row of the identifier table: the identifiers a request names, the project names
/// that must resolve from them, and the identifiers that must be reported absent.
struct IdentifierRow {
    case: &'static str,
    names: Vec<String>,
    resolved: Vec<&'static str>,
    absent: Vec<String>,
}

/// RULE: `BatchGetProjects$names` takes "The names or ARNs of the build projects", so a
/// project resolves from either form — and an identifier that resolves to no project of
/// this account is reported in `projectsNotFound` exactly as the request sent it.
///
/// The rows span the identifier forms: the name, the project's own ARN, a mix of the two
/// in one request, the same project named twice, an ARN of another region and of another
/// account, an ARN of another resource type, and a name no project is held under.
#[tokio::test]
async fn a_project_resolves_by_name_or_by_its_own_arn() {
    let router = emulator();
    let first = create_project(&router, "ident-first", json!({})).await;
    create_project(&router, "ident-second", json!({})).await;
    let first_arn = first["arn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        first_arn,
        expected_arn("project/ident-first"),
        "the created project must carry the ARN of its own name and region, got {first}"
    );
    let second_arn = expected_arn("project/ident-second");
    // The two ARNs that name THIS project's resource path in a scope this account's
    // records are not in: another region, and another account.
    let another_region_arn = format!(
        "arn:aws:codebuild:eu-west-1:{}:project/ident-first",
        engine::EMULATOR_ACCOUNT_ID
    );
    let another_account_arn = format!("arn:aws:codebuild:{REGION}:210987654321:project/ident-first");

    // (case, the identifiers the request names, the names that must resolve, the
    // identifiers that must be reported absent)
    let rows: Vec<IdentifierRow> = vec![
        IdentifierRow {
            case: "the name",
            names: vec!["ident-first".to_string()],
            resolved: vec!["ident-first"],
            absent: vec![],
        },
        IdentifierRow {
            case: "the project's own ARN",
            names: vec![first_arn.clone()],
            resolved: vec!["ident-first"],
            absent: vec![],
        },
        IdentifierRow {
            case: "a name and an ARN together",
            names: vec!["ident-first".to_string(), second_arn.clone()],
            resolved: vec!["ident-first", "ident-second"],
            absent: vec![],
        },
        IdentifierRow {
            case: "one project named twice, by both forms",
            names: vec![first_arn.clone(), "ident-first".to_string()],
            resolved: vec!["ident-first"],
            absent: vec![],
        },
        IdentifierRow {
            case: "an ARN in another region",
            names: vec![another_region_arn.clone()],
            resolved: vec![],
            absent: vec![another_region_arn],
        },
        IdentifierRow {
            case: "an ARN of another account",
            names: vec![another_account_arn.clone()],
            resolved: vec![],
            absent: vec![another_account_arn],
        },
        IdentifierRow {
            case: "an ARN of another resource type",
            names: vec![expected_arn("report-group/ident-first")],
            resolved: vec![],
            absent: vec![expected_arn("report-group/ident-first")],
        },
        IdentifierRow {
            case: "a name no project is held under",
            names: vec!["ident-never-created".to_string()],
            resolved: vec![],
            absent: vec!["ident-never-created".to_string()],
        },
        IdentifierRow {
            case: "an ARN no project is held under",
            names: vec![expected_arn("project/ident-never-created")],
            resolved: vec![],
            absent: vec![expected_arn("project/ident-never-created")],
        },
    ];

    for IdentifierRow {
        case,
        names,
        resolved,
        absent,
    } in rows
    {
        let (status, body) = call(&router, "BatchGetProjects", json!({"names": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let reported: Vec<String> = body["projects"]
            .as_array()
            .map(|projects| {
                projects
                    .iter()
                    .map(|project| project["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, resolved,
            "case {case:?}: expected {resolved:?} to resolve, got {body}"
        );
        assert_eq!(
            body["projectsNotFound"],
            json!(absent),
            "case {case:?}: expected {absent:?} reported absent, got {body}"
        );
    }
}

/// RULE: `InvalidateProjectCache` "Resets the cache for a project", so it acts on a
/// project this account holds and refuses a name it holds none under with 400
/// `ResourceNotFoundException` — and the reset leaves the project's own configuration,
/// its `cache` included, exactly as it was.
///
/// The rows span the cache kinds the model declares (`ops::types::CACHE_TYPE_VALUES`
/// drives the configurations below) plus a project that names no cache at all, so the
/// rule is covered for a project with nothing cached as well as for one with a cache.
#[tokio::test]
async fn invalidating_a_cache_resets_a_held_project_and_refuses_an_absent_one() {
    // Over an engine this test holds, so the instant the reset writes is read back
    // through the very store the handler writes it to — the one observable effect a cache
    // reset has, since CodeBuild models no member for it.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let reset_instant = |name: &str| {
        engine
            .state::<CodeBuildState>()
            .lock()
            .projects
            .get(name)
            .expect("the project is held")
            .cache_invalidated_millis
    };

    // (case, the `cache` the project is created with)
    let caches: [(&str, Value); 4] = [
        ("no cache member at all", json!({})),
        ("NO_CACHE", json!({"cache": {"type": "NO_CACHE"}})),
        (
            "LOCAL",
            json!({"cache": {"type": "LOCAL", "modes": ["LOCAL_SOURCE_CACHE"]}}),
        ),
        (
            "S3",
            json!({"cache": {"type": "S3", "location": "cache-bucket/prefix"}}),
        ),
    ];

    for (index, (case, configuration)) in caches.into_iter().enumerate() {
        let name = format!("cache-proj-{index}");
        let created = create_project(&router, &name, configuration).await;
        assert_eq!(
            reset_instant(&name),
            None,
            "case {case:?}: a project that has never built has no reset instant"
        );

        // The reset answers 200 and reports nothing: `InvalidateProjectCacheOutput`
        // models no member.
        let (status, body) = call(
            &router,
            "InvalidateProjectCache",
            json!({"projectName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: the answer states no member, got {body}"
        );
        // The reset is in the store: the project's cache now counts as empty from the
        // instant the call landed at.
        let first_reset = reset_instant(&name);
        assert!(
            first_reset.is_some(),
            "case {case:?}: the reset must be recorded against the project, got \
             {first_reset:?}"
        );

        // Idempotent: resetting an already-reset cache is the same call again.
        let (status, body) = call(
            &router,
            "InvalidateProjectCache",
            json!({"projectName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second reset: expected 200, got {status} with {body}"
        );
        let second_reset = reset_instant(&name);
        assert!(
            second_reset >= first_reset,
            "case {case:?}: a later reset cannot predate an earlier one, got \
             {second_reset:?} after {first_reset:?}"
        );

        // The project itself is untouched — the cache was emptied, the project was not
        // changed.
        let read = read_project(&router, &name).await;
        assert_eq!(
            read, created,
            "case {case:?}: a reset must leave the project as it was, got {read}"
        );

        // An UPDATE of the project's settings leaves the reset instant alone: the cache
        // was emptied by the call that emptied it, and nothing else.
        let (status, updated) = call(
            &router,
            "UpdateProject",
            json!({"name": &name, "description": "a described project"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateProject: expected 200, got {status} with {updated}"
        );
        assert_eq!(
            reset_instant(&name),
            second_reset,
            "case {case:?}: an update must not change when the cache was reset"
        );
    }

    // A name this account holds no project under: the modelled missing resource, worded
    // the way the project-management operations word it.
    for absent in ["cache-never-created", "cache-proj-0-not-this-one"] {
        let (status, body) = call(
            &router,
            "InvalidateProjectCache",
            json!({"projectName": absent}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {absent:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {absent:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            message(&body),
            state::PROJECT_NOT_FOUND_MESSAGE,
            "case {absent:?}: expected CodeBuild's absent-project sentence, got {body}"
        );
        assert!(
            !message(&body).contains(absent),
            "case {absent:?}: the sentence must name no identifier, got {body}"
        );
    }
}
