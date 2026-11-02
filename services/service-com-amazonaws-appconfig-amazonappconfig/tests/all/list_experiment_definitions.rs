// SPDX-License-Identifier: CC0-1.0

//! `ListExperimentDefinitions` as a REPORT over stored state: it answers one summary per
//! experiment definition the account holds, narrowed by the application, configuration
//! profile, environment and status the request filters on, cut into pages a `NextToken`
//! walks exactly once.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire. The rules under test,
//! each asserted over a SET of inputs rather than over one recorded call:
//!
//! 1. a summary reports the stored definition — `ApplicationId`, `Id`, `Name`,
//!    `Hypothesis`, `Status`, `ConfigurationProfileId`, `EnvironmentId`, `FlagKey`,
//!    `CreatedAt` and `UpdatedAt` exactly as `GetExperimentDefinition` reports them — and
//!    carries no member outside that modelled set;
//! 2. an unfiltered listing reports every definition the ACCOUNT holds, across every
//!    application, in one stable order;
//! 3. each filter narrows the listing to the definitions of the resource it addresses:
//!    `ApplicationIdentifier`, `ConfigurationProfileIdentifier` and
//!    `EnvironmentIdentifier` each address their resource by its ID **or** its name — a
//!    name being resolved under each application, so one name addresses a different
//!    resource under another — and `Status` selects the definitions holding that status;
//! 4. `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports every
//!    definition exactly once and in the same order, at any page size;
//! 5. an `ApplicationIdentifier` that addresses nothing is 404 `ResourceNotFoundException`,
//!    a `NextToken` this listing did not issue is 400 `BadRequestException`, and a
//!    `MaxResults` outside the modelled range is refused;
//! 6. listing is a read: it changes nothing a later caller sees.
//!
//! The tables span what those rules generalise over: HOW MANY definitions the account
//! holds (none, one, several across several applications, and more than a page), WHICH
//! status each definition is in, WHICH resource each filter addresses and in WHICH form
//! (id or name, one shared by two applications, one nothing answers to), WHETHER a
//! definition carries a hypothesis, and WHICH page size the request asks for (absent, the
//! published bounds of `MaxResults`, values in between and one past each bound).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::EXPERIMENT_DEFINITION_STATUS_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::MAX_LIST_PAGE_SIZE;

use crate::appconfig_workflow::{
    create_application, create_environment, create_flags_profile, json_call, router, Answer,
};
use crate::experiment_definitions::{create_definition, flag, get_definition, treatment};
use crate::resource_tags::{filler, pct};
use crate::start_experiment_run::start_run;

/// The members `ExperimentDefinitionSummary` carries
/// (`smithy_query(query="shape",shape="ExperimentDefinitionSummary")`). A summary reports
/// each of them off the stored definition, and nothing else.
const SUMMARY_MEMBERS: &[&str] = &[
    "ApplicationId",
    "Id",
    "Name",
    "Hypothesis",
    "Status",
    "ConfigurationProfileId",
    "EnvironmentId",
    "FlagKey",
    "CreatedAt",
    "UpdatedAt",
];

/// One row of the filter table: what the row is called, the `httpQuery` filter members the
/// request carries, and the definition ids the listing must report for it.
type FilterCase = (&'static str, Vec<(&'static str, String)>, Vec<String>);

/// The published bounds of this operation's `MaxResults`
/// (`range 1..50`,
/// `smithy_query(query="operation",operation="ListExperimentDefinitions")`), read off the
/// service's own constant for the upper one so a case cannot drift from it.
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = MAX_LIST_PAGE_SIZE as i64;

/// `GET /experimentdefinitions` with `query` as the request's `httpQuery` members
/// (`application_identifier`, `configuration_profile_identifier`,
/// `environment_identifier`, `status`, `max_results`, `next_token`). `pub(crate)` because
/// more than one suite asserts what a listing reports about a stored definition.
pub(crate) async fn list(router: &MultiServiceRouter, query: &[(&str, String)]) -> Answer {
    let mut uri = "/experimentdefinitions".to_string();
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", pct(value)))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a successful listing.
fn items(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExperimentDefinitions expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// The `Id` of every summary in `page`, in the order they were served.
fn ids(case: &str, page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|summary| {
            summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary carries no Id: {summary}"))
                .to_string()
        })
        .collect()
}

/// One application with the environments and configuration profiles a definition needs.
struct Tenant {
    /// The minted application id.
    application: String,
    /// The application's own name, which addresses it just as its id does.
    name: String,
    /// `(id, name)` per environment, in the order they were created.
    environments: Vec<(String, String)>,
    /// `(id, name)` per configuration profile, in the order they were created.
    profiles: Vec<(String, String)>,
}

/// Create an application named `app-{label}` holding one environment and one configuration
/// profile per name given, so a case can address each by either form.
async fn tenant(
    router: &MultiServiceRouter,
    label: &str,
    environments: &[&str],
    profiles: &[&str],
) -> Tenant {
    let name = format!("app-{label}");
    let application = create_application(router, &name).await;
    let mut created_environments = Vec::new();
    for environment in environments {
        let id = create_environment(router, &application, environment).await;
        created_environments.push((id, (*environment).to_string()));
    }
    let mut created_profiles = Vec::new();
    for profile in profiles {
        let id = create_flags_profile(router, &application, profile).await;
        created_profiles.push((id, (*profile).to_string()));
    }
    Tenant {
        application,
        name,
        environments: created_environments,
        profiles: created_profiles,
    }
}

/// One stored experiment definition and the resources it was created against, so a case can
/// state what each filter must report without reading it back out of the listing.
#[derive(Clone)]
struct Seeded {
    application: String,
    application_name: String,
    id: String,
    name: String,
    profile: String,
    profile_name: String,
    environment: String,
    environment_name: String,
    status: String,
}

/// Create one experiment definition under `tenant`, against the environment and profile at
/// the given positions, and answer the record it was stored as.
async fn define(
    router: &MultiServiceRouter,
    tenant: &Tenant,
    name: &str,
    profile_index: usize,
    environment_index: usize,
    hypothesis: Option<&str>,
    flag_key: &str,
) -> Seeded {
    let (profile, profile_name) = tenant.profiles[profile_index].clone();
    let (environment, environment_name) = tenant.environments[environment_index].clone();
    let mut body = json!({
        "Name": name,
        "ConfigurationProfileIdentifier": profile,
        "EnvironmentIdentifier": environment,
        "FlagKey": flag_key,
        "AudienceRule": "attribute(\"tier\") == \"beta\"",
        "Treatments": [treatment(50.0, Some("the new flow"), flag(true))],
        "Control": treatment(50.0, Some("today's experience"), flag(false)),
    });
    if let Some(hypothesis) = hypothesis {
        body["Hypothesis"] = Value::from(hypothesis);
    }
    let answer = create_definition(router, &tenant.application, &body).await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateExperimentDefinition must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let created = answer.json();
    Seeded {
        application: tenant.application.clone(),
        application_name: tenant.name.clone(),
        id: created
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: the create answered no Id: {created}"))
            .to_string(),
        name: name.to_string(),
        profile,
        profile_name,
        environment,
        environment_name,
        status: created
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: the create answered no Status: {created}"))
            .to_string(),
    }
}

/// Start a run of `seeded`, which is what moves its definition to the status an experiment
/// in flight holds.
async fn make_active(router: &MultiServiceRouter, seeded: &mut Seeded) {
    let started = start_run(router, &seeded.application, &seeded.id, &json!({})).await;
    assert_eq!(
        started.status,
        201,
        "case {:?}: StartExperimentRun must answer 201, got {} / {}",
        seeded.name,
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    seeded.status = read_status(router, &seeded.application, &seeded.id).await;
}

/// Archive `seeded` — the delete that "hide[s] it from the active list while preserving it
/// for future reference" (`smithy_query(query="doc",shape="DeleteExperimentDefinition")`),
/// which is what moves a definition to the archived status.
async fn make_archived(router: &MultiServiceRouter, seeded: &mut Seeded) {
    let archived = json_call(
        router,
        "DELETE",
        &format!(
            "/applications/{}/experimentdefinitions/{}?delete_type=ARCHIVE",
            pct(&seeded.application),
            pct(&seeded.id)
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        archived.status,
        204,
        "case {:?}: an archiving delete must answer 204, got {} / {}",
        seeded.name,
        archived.status,
        String::from_utf8_lossy(&archived.body)
    );
    seeded.status = read_status(router, &seeded.application, &seeded.id).await;
}

/// The `Status` the single-definition read reports, so a case states the status the SERVICE
/// holds rather than one the test assumed.
async fn read_status(router: &MultiServiceRouter, application: &str, definition: &str) -> String {
    let answer = get_definition(router, application, definition).await;
    assert_eq!(
        answer.status,
        200,
        "GetExperimentDefinition must report {definition:?}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Status")
        .unwrap_or_else(|| panic!("GetExperimentDefinition answered no Status for {definition:?}"))
}

/// The ids of `seeded`, in the order a listing must report them: ascending `Id`, the total
/// order the service pages over.
fn expected_ids(seeded: &[&Seeded]) -> Vec<String> {
    let mut ordered: Vec<String> = seeded.iter().map(|one| one.id.clone()).collect();
    ordered.sort();
    ordered
}

/// Two applications, each with two environments and two profiles that SHARE their names
/// across the applications, holding five definitions between them: every status the model
/// lists, with and without a hypothesis, under each profile and environment.
async fn world(router: &MultiServiceRouter) -> Vec<Seeded> {
    let alpha = tenant(router, "alpha", &["env-one", "env-two"], &["profile-one", "profile-two"]).await;
    let beta = tenant(router, "beta", &["env-one", "env-two"], &["profile-one", "profile-two"]).await;

    let mut seeded = vec![
        // A definition nothing has run yet, carrying a hypothesis.
        define(
            router,
            &alpha,
            "alpha-idle",
            0,
            0,
            Some("the new flow converts better"),
            "checkout-flow",
        )
        .await,
        // One with no hypothesis at all, under the SAME profile but another environment.
        define(router, &alpha, "alpha-active", 0, 1, None, "search-ranking").await,
        // One under the other profile, with a hypothesis at the published lower bound of
        // `Description` (len 0..1024).
        define(router, &alpha, "alpha-archived", 1, 0, Some(""), "price-banner").await,
        // The second application's own definitions, against resources whose NAMES its
        // sibling's resources also carry.
        define(
            router,
            &beta,
            "beta-idle",
            0,
            0,
            Some(&filler(1024, 5)),
            "checkout-flow",
        )
        .await,
        define(router, &beta, "beta-active", 1, 1, None, "basket-size").await,
    ];

    make_active(router, &mut seeded[1]).await;
    make_archived(router, &mut seeded[2]).await;
    make_active(router, &mut seeded[4]).await;
    seeded
}

/// RULE 1: each summary reports the stored definition exactly as the single-definition read
/// does, and carries no member outside the modelled summary — over definitions that differ
/// in status, in the hypothesis they carry and in the application that holds them.
#[tokio::test]
async fn a_summary_reports_the_stored_definition_as_the_reader_does() {
    let router = router();
    let seeded = world(&router).await;

    let answer = list(&router, &[]).await;
    let page = items("every definition", &answer);
    assert_eq!(
        page.len(),
        seeded.len(),
        "case \"every definition\": expected {} summaries, got {}: {}",
        seeded.len(),
        page.len(),
        answer.json()
    );

    for one in &seeded {
        let case = format!("definition {}", one.name);
        let summary = page
            .iter()
            .find(|summary| summary.get("Id").and_then(Value::as_str) == Some(one.id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the listing left it out: {}", answer.json()));

        // The reader is the other document describing this same definition: every member
        // the summary carries must be the one it reports, so a summary is a report over
        // state rather than a value made up at listing time.
        let read = get_definition(&router, &one.application, &one.id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExperimentDefinition expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let read = read.json();
        let expected: Value = Value::Object(
            SUMMARY_MEMBERS
                .iter()
                .filter_map(|member| {
                    read.get(*member)
                        .map(|value| ((*member).to_string(), value.clone()))
                })
                .collect(),
        );
        assert_eq!(
            summary, &expected,
            "case {case:?}: expected the reader's members {expected}, got {summary}"
        );
    }
}

/// RULE 2: an unfiltered listing reports every definition the ACCOUNT holds, in ascending
/// `Id` order — and RULE 6: repeating it answers the same thing, and every definition it
/// reported still reads back unchanged.
#[tokio::test]
async fn an_unfiltered_listing_reports_the_whole_account_in_id_order() {
    let router = router();

    // Rows differ along how much the account holds when the listing runs: nothing at all,
    // one definition, and then the whole world.
    let empty = list(&router, &[]).await;
    assert!(
        items("an account holding no definition", &empty).is_empty(),
        "case \"an account holding no definition\": expected an empty Items, got {}",
        empty.json()
    );
    assert_eq!(
        empty.json().get("NextToken"),
        None,
        "case \"an account holding no definition\": expected no NextToken, got {}",
        empty.json()
    );

    let only = tenant(&router, "only", &["env-one"], &["profile-one"]).await;
    let first = define(&router, &only, "only-one", 0, 0, None, "checkout-flow").await;
    let case = "an account holding one definition";
    let page = items(case, &list(&router, &[]).await);
    assert_eq!(
        ids(case, &page),
        vec![first.id.clone()],
        "case {case:?}: expected just {:?}, got {:?}",
        first.id,
        ids(case, &page)
    );

    let seeded = world(&router).await;
    let mut all: Vec<&Seeded> = seeded.iter().collect();
    all.push(&first);
    let expected = expected_ids(&all);

    let case = "the whole account";
    let answer = list(&router, &[]).await;
    let page = items(case, &answer);
    assert_eq!(
        ids(case, &page),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        ids(case, &page)
    );

    // A read changes nothing: the same call answers the same thing, and each definition
    // still reads back.
    let again = list(&router, &[]).await;
    assert_eq!(
        again.json(),
        answer.json(),
        "case {case:?}: listing twice must answer the same, got {}",
        again.json()
    );
    for one in &all {
        let status = read_status(&router, &one.application, &one.id).await;
        assert_eq!(
            status, one.status,
            "case {:?}: the reader must still report status {:?} after listing, got {status:?}",
            one.name, one.status
        );
    }
}

/// RULE 3: each identifier filter narrows the listing to the definitions of the resource it
/// addresses, in either addressing form — including a name two applications share, which
/// addresses one resource under each.
#[tokio::test]
async fn each_identifier_filter_reports_only_that_resources_definitions() {
    let router = router();
    let seeded = world(&router).await;
    let (alpha_idle, alpha_active, alpha_archived, beta_idle, beta_active) = (
        &seeded[0], &seeded[1], &seeded[2], &seeded[3], &seeded[4],
    );

    // Rows differ along WHICH filter is applied, WHICH form addresses the resource, and
    // whether the identifier addresses a resource of one application or of both.
    let cases: Vec<FilterCase> = vec![
        (
            "the first application by id",
            vec![("application_identifier", alpha_idle.application.clone())],
            expected_ids(&[alpha_idle, alpha_active, alpha_archived]),
        ),
        (
            "the first application by name",
            vec![(
                "application_identifier",
                alpha_idle.application_name.clone(),
            )],
            expected_ids(&[alpha_idle, alpha_active, alpha_archived]),
        ),
        (
            "the second application by id",
            vec![("application_identifier", beta_idle.application.clone())],
            expected_ids(&[beta_idle, beta_active]),
        ),
        (
            "a configuration profile by id",
            vec![(
                "configuration_profile_identifier",
                alpha_idle.profile.clone(),
            )],
            expected_ids(&[alpha_idle, alpha_active]),
        ),
        (
            // The profile name both applications' first profile carries: it addresses one
            // profile under each, so the listing reports the definitions of both.
            "a configuration profile name two applications share",
            vec![(
                "configuration_profile_identifier",
                alpha_idle.profile_name.clone(),
            )],
            expected_ids(&[alpha_idle, alpha_active, beta_idle]),
        ),
        (
            "a configuration profile name, narrowed to one application",
            vec![
                ("application_identifier", beta_idle.application.clone()),
                (
                    "configuration_profile_identifier",
                    beta_idle.profile_name.clone(),
                ),
            ],
            expected_ids(&[beta_idle]),
        ),
        (
            "an environment by id",
            vec![("environment_identifier", alpha_idle.environment.clone())],
            expected_ids(&[alpha_idle, alpha_archived]),
        ),
        (
            "an environment name two applications share",
            vec![(
                "environment_identifier",
                alpha_idle.environment_name.clone(),
            )],
            expected_ids(&[alpha_idle, alpha_archived, beta_idle]),
        ),
        (
            "an environment and a profile together",
            vec![
                (
                    "configuration_profile_identifier",
                    alpha_active.profile.clone(),
                ),
                (
                    "environment_identifier",
                    alpha_active.environment.clone(),
                ),
            ],
            expected_ids(&[alpha_active]),
        ),
        (
            "every filter at once",
            vec![
                ("application_identifier", beta_active.application_name.clone()),
                (
                    "configuration_profile_identifier",
                    beta_active.profile_name.clone(),
                ),
                (
                    "environment_identifier",
                    beta_active.environment_name.clone(),
                ),
                ("status", beta_active.status.clone()),
            ],
            expected_ids(&[beta_active]),
        ),
        (
            // A filter that addresses no resource of this account leaves no definition to
            // report, which is an empty listing rather than another resource's.
            "a configuration profile nothing answers to",
            vec![(
                "configuration_profile_identifier",
                "no-such-profile".to_string(),
            )],
            Vec::new(),
        ),
        (
            "an environment nothing answers to",
            vec![("environment_identifier", "no-such-environment".to_string())],
            Vec::new(),
        ),
        (
            "a profile of one application with the environment of another",
            vec![
                (
                    "configuration_profile_identifier",
                    alpha_idle.profile.clone(),
                ),
                ("environment_identifier", beta_active.environment.clone()),
            ],
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let answer = list(&router, &query).await;
        let page = items(case, &answer);
        assert_eq!(
            ids(case, &page),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            ids(case, &page)
        );
    }
}

/// RULE 3: `Status` reports only the definitions holding that status — asserted for every
/// value the modelled enum lists, and for a value outside it.
#[tokio::test]
async fn the_status_filter_reports_only_definitions_in_that_status() {
    let router = router();
    let seeded = world(&router).await;

    let unfiltered = items("every definition", &list(&router, &[]).await);

    // Every value of the modelled `ExperimentDefinitionStatus` enum, read off the service's
    // own value list rather than hand-copied.
    for status in EXPERIMENT_DEFINITION_STATUS_VALUES {
        let expected: Vec<String> = expected_ids(
            &seeded
                .iter()
                .filter(|one| one.status == *status)
                .collect::<Vec<&Seeded>>(),
        );
        // The world holds a definition in each status, so no row is vacuous.
        assert!(
            !expected.is_empty(),
            "case {status:?}: the world must hold a definition in every modelled status"
        );

        let case = format!("Status {status}");
        let answer = list(&router, &[("status", (*status).to_string())]).await;
        let page = items(&case, &answer);
        assert_eq!(
            ids(&case, &page),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            ids(&case, &page)
        );
        for summary in &page {
            assert_eq!(
                summary.get("Status").and_then(Value::as_str),
                Some(*status),
                "case {case:?}: every summary must hold the filtered status, got {summary}"
            );
        }
        // Narrowing by status is the same selection the unfiltered listing already reports
        // for that status, so the filter cannot invent or hide a definition.
        let from_unfiltered: Vec<String> = {
            let mut selected: Vec<String> = unfiltered
                .iter()
                .filter(|summary| summary.get("Status").and_then(Value::as_str) == Some(*status))
                .map(|summary| {
                    summary
                        .get("Id")
                        .and_then(Value::as_str)
                        .expect("a summary carries an Id")
                        .to_string()
                })
                .collect();
            selected.sort();
            selected
        };
        assert_eq!(
            from_unfiltered, expected,
            "case {case:?}: the unfiltered listing must agree, expected {expected:?}, got {from_unfiltered:?}"
        );
    }

    // A status outside the modelled enum is refused by the schema walk before the listing
    // runs, rather than answered as an empty page.
    let refused = list(&router, &[("status", "PAUSED".to_string())]).await;
    assert_eq!(
        refused.status,
        400,
        "case \"Status outside the enum\": expected 400, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
}

/// RULE 4: `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports
/// every definition exactly once and in the same order — at every page size, including the
/// published bounds of `MaxResults`, and with and without a filter.
#[tokio::test]
async fn a_next_token_walk_reports_every_definition_exactly_once() {
    let router = router();
    let seeded = world(&router).await;
    let expected_all = expected_ids(&seeded.iter().collect::<Vec<&Seeded>>());
    let alpha = &seeded[0];
    let expected_alpha = expected_ids(&[&seeded[0], &seeded[1], &seeded[2]]);

    // Rows differ along the filter the walk carries (none, and one application's) and along
    // the page size: the published minimum, each size up past the number of items, the
    // published maximum, and no `MaxResults` at all.
    for (label, filter, expected) in [
        ("unfiltered", Vec::new(), expected_all),
        (
            "narrowed to one application",
            vec![("application_identifier", alpha.application.clone())],
            expected_alpha,
        ),
    ] {
        let mut page_sizes: Vec<Option<i64>> = (MAX_RESULTS_MIN
            ..=(expected.len() as i64 + 1).min(MAX_RESULTS_MAX))
            .map(Some)
            .collect();
        page_sizes.push(Some(MAX_RESULTS_MAX));
        page_sizes.push(None);

        for page_size in page_sizes {
            let case = format!("{label}, MaxResults {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut query = filter.clone();
                if let Some(page_size) = page_size {
                    query.push(("max_results", page_size.to_string()));
                }
                if let Some(token) = &next_token {
                    query.push(("next_token", token.clone()));
                }
                let answer = list(&router, &query).await;
                let page = items(&case, &answer);
                let bound = page_size.unwrap_or(MAX_RESULTS_MAX);
                assert!(
                    page.len() as i64 <= bound,
                    "case {case:?}: a page must carry at most {bound} items, got {}",
                    page.len()
                );
                seen.extend(ids(&case, &page));
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                next_token = answer.string("NextToken");
                if next_token.is_none() {
                    break;
                }
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that resumes must carry items, got {}",
                    answer.json()
                );
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: the walk must report every definition exactly once in id order, expected {expected:?}, got {seen:?}"
            );
        }
    }
}

/// RULE 4: an account holding more definitions than one page carries hands back a
/// `NextToken`, and the default page is the published maximum.
#[tokio::test]
async fn a_listing_longer_than_a_page_is_cut_at_the_published_maximum() {
    let router = router();
    let crowded = tenant(&router, "crowded", &["env-one"], &["profile-one"]).await;

    let mut held: Vec<Seeded> = Vec::new();
    for index in 0..(MAX_LIST_PAGE_SIZE + 2) {
        held.push(
            define(
                &router,
                &crowded,
                &format!("definition-{index:03}"),
                0,
                0,
                None,
                "checkout-flow",
            )
            .await,
        );
    }
    let expected = expected_ids(&held.iter().collect::<Vec<&Seeded>>());

    let case = "a default page over more definitions than it carries";
    let answer = list(&router, &[]).await;
    let page = items(case, &answer);
    assert_eq!(
        ids(case, &page),
        expected[..MAX_LIST_PAGE_SIZE].to_vec(),
        "case {case:?}: expected the first {MAX_LIST_PAGE_SIZE} ids, got {:?}",
        ids(case, &page)
    );
    let token = answer
        .string("NextToken")
        .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken, got {}", answer.json()));

    let case = "the page the token resumes";
    let resumed = list(&router, &[("next_token", token)]).await;
    let page = items(case, &resumed);
    assert_eq!(
        ids(case, &page),
        expected[MAX_LIST_PAGE_SIZE..].to_vec(),
        "case {case:?}: expected the remaining ids, got {:?}",
        ids(case, &page)
    );
    assert_eq!(
        resumed.json().get("NextToken"),
        None,
        "case {case:?}: the last page hands back no NextToken, got {}",
        resumed.json()
    );
}

/// RULE 5: an `ApplicationIdentifier` that addresses nothing is 404, a `NextToken` this
/// listing did not issue is 400, and a `MaxResults` outside the published range is refused —
/// and RULE 6: every refusal leaves the account exactly as it was.
#[tokio::test]
async fn a_listing_that_filters_on_a_missing_application_or_a_foreign_token_is_refused() {
    let router = router();
    let seeded = world(&router).await;
    let expected = expected_ids(&seeded.iter().collect::<Vec<&Seeded>>());

    // An experiment definition exists only under an application, so an
    // `ApplicationIdentifier` addressing none is the not-found this operation models. Rows
    // differ along the form of the identifier that addresses nothing.
    for (case, identifier) in [
        ("an id nothing answers to", "nosuchapp".to_string()),
        ("a name nothing answers to", "app-missing".to_string()),
        (
            "a name an application of another kind of resource carries",
            seeded[0].profile_name.clone(),
        ),
    ] {
        let refused = list(&router, &[("application_identifier", identifier)]).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code()
        );
    }

    // A token this listing did not mint resumes nothing: rows differ along WHERE the token
    // came from — text that is no token at all, and a token another listing of this same
    // service minted.
    let extensions = json_call(&router, "GET", "/extensions?max_results=1", &Value::Null).await;
    let foreign_token = extensions
        .string("NextToken")
        .expect("ListExtensions must hand back a token over the AWS-authored catalogue");
    for (case, token) in [
        ("text that is no token at all", "not-a-token".to_string()),
        ("a token another listing minted", foreign_token),
    ] {
        let refused = list(&router, &[("next_token", token)]).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
    }

    // `MaxResults` is bounded 1..50: each published bound is served and each value one past
    // it is refused before the listing runs.
    for (case, page_size, expected_status) in [
        ("the lower published bound", MAX_RESULTS_MIN, 200_u16),
        ("the upper published bound", MAX_RESULTS_MAX, 200),
        ("one below the lower bound", MAX_RESULTS_MIN - 1, 400),
        ("one past the upper bound", MAX_RESULTS_MAX + 1, 400),
    ] {
        let answer = list(&router, &[("max_results", page_size.to_string())]).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: MaxResults {page_size} expected {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Every refusal above wrote nothing: the account still holds exactly what it did.
    let case = "the account after every refusal";
    let answer = list(&router, &[]).await;
    let page = items(case, &answer);
    assert_eq!(
        ids(case, &page),
        expected,
        "case {case:?}: expected {expected:?}, got {}",
        answer.json()
    );
}
