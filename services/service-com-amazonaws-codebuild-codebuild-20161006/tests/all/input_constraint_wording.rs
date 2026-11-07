// SPDX-License-Identifier: CC0-1.0

//! Cover for how CodeBuild states a request refused by the rules its MODEL declares,
//! and for the one input form it checks itself, driven over the wire the router serves:
//! `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Two rules are under test.
//!
//! * A constraint CodeBuild words ITSELF is answered with CodeBuild's own sentence,
//!   built per RULE from the request — an empty list of build ids, an input resource
//!   list longer than the model allows, a build timeout outside the window the model
//!   pins on it — and every other violation keeps the shared constraint envelope, whose
//!   enumerated value sets are stated in ascending order.
//! * A build id has to BE one (`<projectName>:<uuid>`): a value of another shape is
//!   refused as bad input rather than reported as a build this account does not hold.
//!
//! Each table spans the class the rule is about — several operations, both list members
//! the model bounds, each published bound and one past it, and every shape a malformed
//! id takes — rather than one recorded request.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::PROJECT_VISIBILITY_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project, emulator, error_type, start_build, REGION,
};

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Call `operation`, assert it was refused with HTTP 400 `InvalidInputException`, and
/// hand back the sentence it stated.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {answer}"
    );
    assert_eq!(
        error_type(&answer),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException from {operation}, got {answer}"
    );
    message(&answer)
}

/// A syntactically valid build id (`<projectName>:<uuid>`) that names no build: the uuid
/// is one this service never mints, since every build's uuid is derived from its
/// project, build number and start instant.
fn absent_build_id(project: &str, nth: usize) -> String {
    format!("{project}:00000000-0000-8000-8000-{nth:012}")
}

/// RULE: an input list of BUILD IDS that names no build at all is refused with 400
/// `InvalidInputException` worded `The input build ID list cannot be empty.` — fixed
/// text that names the KIND of list the request left empty and quotes neither the member
/// path nor the size, so the sentence is the same on every operation whose input is a
/// `BuildIds` list.
#[tokio::test]
async fn an_empty_build_id_list_is_refused_in_codebuilds_own_words() {
    let router = emulator();

    // Both operations whose input is a `BuildIds` list — the read and the delete.
    for operation in ["BatchGetBuilds", "BatchDeleteBuilds"] {
        let stated = refusal(&router, operation, operation, json!({"ids": []})).await;
        assert_eq!(
            stated, "The input build ID list cannot be empty.",
            "case {operation:?}: expected CodeBuild's empty-list sentence, got {stated:?}"
        );
        for leaked in ["ids", "items", "constraint"] {
            assert!(
                !stated.contains(leaked),
                "case {operation:?}: the sentence must not name {leaked:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: an input resource list longer than the model allows is refused with 400
/// `InvalidInputException` worded `The maximum length of input resource list is <max>
/// but input list with size <size> is found.` — the limit is the `@length` maximum the
/// model puts on that list and the size is the one the request carried, so the sentence
/// is about any over-long list of identifiers rather than about one member.
///
/// The rows span the two list members the model bounds at 100 (`BuildIds` and
/// `ProjectNames`, `smithy_query(query="shape",shape="BuildIds,ProjectNames")`), the
/// operations that take them, the published bound itself and sizes past it.
#[tokio::test]
async fn an_over_long_input_resource_list_states_the_limit_and_the_size_received() {
    let router = emulator();

    /// The `@length` maximum the model puts on `BuildIds` and `ProjectNames`.
    const LIST_MAXIMUM: usize = 100;

    // (operation, the member that carries the list, whether its entries are build ids)
    let lists: [(&str, &str, bool); 4] = [
        ("BatchGetBuilds", "ids", true),
        ("BatchDeleteBuilds", "ids", true),
        ("BatchGetProjects", "names", false),
        ("BatchGetBuildBatches", "ids", true),
    ];

    for (operation, member, holds_build_ids) in lists {
        let entry = |nth: usize| {
            if holds_build_ids {
                absent_build_id("over", nth)
            } else {
                format!("over-{nth}")
            }
        };
        // At the bound, the list is accepted: the refusal is about being PAST it.
        let at_bound: Vec<String> = (0..LIST_MAXIMUM).map(entry).collect();
        let (status, answer) = call(&router, operation, list_request(member, &at_bound)).await;
        assert_eq!(
            status, 200,
            "{operation} with {LIST_MAXIMUM} entries: the published bound must be \
             accepted, got {status} with {answer}"
        );

        // One past the bound, and well past it.
        for size in [LIST_MAXIMUM + 1, LIST_MAXIMUM + 50] {
            let over: Vec<String> = (0..size).map(entry).collect();
            let case = format!("{operation} with {size} entries at {member}");
            let stated = refusal(&router, &case, operation, list_request(member, &over)).await;
            assert_eq!(
                stated,
                format!(
                    "The maximum length of input resource list is {LIST_MAXIMUM} but \
                     input list with size {size} is found."
                ),
                "case {case:?}: expected CodeBuild's over-long-list sentence, got {stated:?}"
            );
        }
    }
}

/// A request whose `member` carries `entries` — the one member each batch operation
/// above takes.
fn list_request(member: &str, entries: &[String]) -> Value {
    let mut body = json!({});
    if let Value::Object(members) = &mut body {
        members.insert(member.to_string(), json!(entries));
    }
    body
}

/// RULE: a BUILD timeout outside the window the model pins on it is refused with 400
/// `InvalidInputException` worded `Invalid timeout. Should be between 5 minutes and 36
/// hours` — the bounds are `BuildTimeOut`'s own (`@range(min: 5, max: 2160)`), stated in
/// the unit AWS states them in, and the sentence quotes nothing of the request, so it is
/// the same wherever a build timeout is set.
///
/// The rows span every operation that takes one (a project's `timeoutInMinutes` on
/// create and on update, and a build's `timeoutInMinutesOverride`), each published bound
/// and one past it on each side. A violation of the sibling QUEUED timeout, whose window
/// is a different one, still states its member and its constraint — the sentence belongs
/// to the rule, not to every timeout.
#[tokio::test]
async fn a_build_timeout_outside_its_window_is_refused_in_codebuilds_own_words() {
    let router = emulator();
    create_project(&router, "timeout-proj", json!({})).await;

    let expected = "Invalid timeout. Should be between 5 minutes and 36 hours";

    // (operation, the member that carries the timeout) — every operation that sets one.
    let members: [(&str, &str); 3] = [
        ("CreateProject", "timeoutInMinutes"),
        ("UpdateProject", "timeoutInMinutes"),
        ("StartBuild", "timeoutInMinutesOverride"),
    ];

    for (operation, member) in members {
        // Past the maximum, and below the minimum, in several forms.
        for minutes in [2161, 100_000, 4, 0, -1] {
            let case = format!("{operation} with {member} {minutes}");
            let stated = refusal(
                &router,
                &case,
                operation,
                timeout_request(operation, member, minutes),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected CodeBuild's timeout sentence, got {stated:?}"
            );
        }

        // Both published bounds are accepted by the same member.
        for minutes in [5, 2160] {
            let case = format!("{operation} with {member} {minutes}");
            let (status, answer) = call(
                &router,
                operation,
                timeout_request(operation, member, minutes),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a published bound must be accepted, got {status} with {answer}"
            );
        }
    }

    // The queued timeout is a different wait with a window of its own (`TimeOut`,
    // `@range(min: 5, max: 480)`), so a value past ITS maximum keeps the shared
    // envelope.
    let stated = refusal(
        &router,
        "CreateProject with queuedTimeoutInMinutes 481",
        "CreateProject",
        super::project_build_lifecycle::create_project_body(
            "queued-timeout",
            json!({"queuedTimeoutInMinutes": 481}),
        ),
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Value '481' at 'queuedTimeoutInMinutes' failed to \
         satisfy constraint: Member must have value less than or equal to 480",
        "a timeout with a window of its own must still state its member, got {stated:?}"
    );
}

/// A request for `operation` that sets its build-timeout `member` to `minutes`, with
/// whatever else that operation requires — a whole project for the create, the stored
/// project's name for the update, and the project to build for the start.
fn timeout_request(operation: &str, member: &str, minutes: i64) -> Value {
    let mut timeout = json!({});
    if let Value::Object(members) = &mut timeout {
        members.insert(member.to_string(), json!(minutes));
    }
    match operation {
        "CreateProject" => super::project_build_lifecycle::create_project_body(
            &format!("timeout-{}", minutes.unsigned_abs()),
            timeout,
        ),
        "UpdateProject" => {
            super::project_build_lifecycle::merged(json!({"name": "timeout-proj"}), timeout)
        }
        _ => super::project_build_lifecycle::merged(json!({"projectName": "timeout-proj"}), timeout),
    }
}

/// RULE: where the shared constraint envelope states an enumerated type's value set, the
/// values are listed in ascending order — the order the generated value list carries
/// (`ops::types::PROJECT_VISIBILITY_TYPE_VALUES`), not the order the model happens to
/// declare them in (`ProjectVisibilityType` is declared `PUBLIC_READ, PRIVATE`).
///
/// The rows span the forms an unmodelled value takes: a token of the right shape, a
/// modelled value in the wrong case, the empty string and one with a trailing space.
#[tokio::test]
async fn an_enumerated_value_set_is_stated_in_ascending_order() {
    let router = emulator();
    let project = create_project(&router, "visibility-proj", json!({})).await;
    let project_arn = project["arn"].as_str().unwrap_or_default().to_string();

    let set = PROJECT_VISIBILITY_TYPE_VALUES.join(", ");
    assert_eq!(
        set, "PRIVATE, PUBLIC_READ",
        "the generated value list must be the ascending one, got {set:?}"
    );

    for value in ["BOGUS", "private", "", "PRIVATE "] {
        let case = format!("projectVisibility {value:?}");
        let stated = refusal(
            &router,
            &case,
            "UpdateProjectVisibility",
            json!({"projectArn": project_arn, "projectVisibility": value}),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Value '{value}' at 'projectVisibility' \
                 failed to satisfy constraint: Member must satisfy enum value set: [{set}]"
            ),
            "case {case:?}: expected the value set in ascending order, got {stated:?}"
        );
    }

    // The project is untouched by a refused visibility change.
    let (status, read) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["visibility-proj"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetProjects after the refusals: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["projects"][0]["projectVisibility"],
        json!("PRIVATE"),
        "a refused visibility change must leave the project as it was, got {read}"
    );
}

/// RULE: an id has to ADDRESS a build — the `<projectName>:<uuid>` form every build of
/// this service is keyed by, or the ARN whose resource segment is such an id — so a value
/// of neither form is refused with 400 `InvalidInputException` worded `Invalid build ID
/// provided`, where a value that DOES address a build nobody started is reported in the
/// not-found half of the same answer.
///
/// The rows span the shapes a value of neither form takes (no separator at all, an empty
/// half on either side, a second half that is not a uuid, one a digit short of one, a
/// bare project name) and both operations whose input is a `BuildIds` list. The build's
/// own ARN form is NOT among them: nothing records CodeBuild refusing it, so it is
/// admitted and answered on the records the account holds.
#[tokio::test]
async fn a_malformed_build_id_is_refused_rather_than_reported_absent() {
    let router = emulator();
    create_project(&router, "id-form", json!({})).await;
    let build = start_build(&router, json!({"projectName": "id-form"})).await;
    let held = build["id"].as_str().unwrap_or_default().to_string();

    let malformed: [(&str, String); 6] = [
        ("no separator at all", "this-id-has-no-colon-separator".to_string()),
        ("an empty uuid half", "id-form:".to_string()),
        ("an empty project half", ":00000000-0000-8000-8000-000000000000".to_string()),
        ("a second half that is not a uuid", "id-form:not-a-uuid".to_string()),
        (
            "a second half one hex digit short of a uuid",
            "id-form:00000000-0000-8000-8000-00000000000".to_string(),
        ),
        ("a bare project name", "id-form".to_string()),
    ];

    for (case, identifier) in &malformed {
        // (operation, the member the answer reports found builds under)
        for operation in ["BatchGetBuilds", "BatchDeleteBuilds"] {
            let case = format!("{operation} with {case}");
            // Alone, and beside a build the account really holds: one malformed value
            // refuses the whole request.
            for ids in [
                json!([identifier]),
                json!([held.clone(), identifier.clone()]),
            ] {
                let stated = refusal(&router, &case, operation, json!({"ids": ids})).await;
                assert_eq!(
                    stated, "Invalid build ID provided",
                    "case {case:?}: expected CodeBuild's malformed-id sentence, got {stated:?}"
                );
                assert!(
                    !stated.contains(identifier.as_str()),
                    "case {case:?}: the sentence must not quote the value back, got {stated:?}"
                );
            }
        }
    }

    // Nothing was deleted by any refused request: the held build is still there.
    let (status, read) = call(&router, "BatchGetBuilds", json!({"ids": [held.clone()]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds after the refusals: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["builds"][0]["id"],
        json!(held.clone()),
        "a refused request must delete nothing, got {read}"
    );

    // A well-formed id for a build nobody started is the not-found case, not a refusal —
    // on the read and on the delete alike.
    let absent = absent_build_id("id-form", 7);
    let (status, read) = call(
        &router,
        "BatchGetBuilds",
        json!({"ids": [absent.clone(), held.clone()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds for a well-formed absent id: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["buildsNotFound"],
        json!([absent.clone()]),
        "a well-formed absent id belongs in buildsNotFound, got {read}"
    );
    let (status, deleted) = call(
        &router,
        "BatchDeleteBuilds",
        json!({"ids": [absent.clone()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchDeleteBuilds for a well-formed absent id: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["buildsNotDeleted"][0]["id"],
        json!(absent),
        "a well-formed absent id belongs in buildsNotDeleted, got {deleted}"
    );

    // The build's own ARN form is ADMITTED rather than refused — nothing records
    // CodeBuild refusing it — and answered on the records the account holds: a reader
    // keyed by build id holds nothing under an ARN, so it lands in the not-found half.
    let arn_form = state::build_arn(REGION, &held);
    let (status, read) = call(&router, "BatchGetBuilds", json!({"ids": [&arn_form]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds for the ARN form: expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["buildsNotFound"],
        json!([arn_form]),
        "the ARN form must be answered on the records held, got {read}"
    );
}
