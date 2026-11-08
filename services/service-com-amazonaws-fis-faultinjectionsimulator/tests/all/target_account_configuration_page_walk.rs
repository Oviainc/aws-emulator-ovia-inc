// SPDX-License-Identifier: CC0-1.0

//! The ONE page walk both target-account-configuration lists serve, end to end over HTTP.
//!
//! The rule under test: a target account configuration is a sub-resource named by its
//! `accountId`, so a page of them is cut by one and the same rule wherever it is served —
//! the configurations are reported in `accountId` order, a page holds at most the page size
//! the call serves, a `nextToken` arrives exactly when configurations remain and carries
//! the `accountId` the next page resumes at, and a token no page ever issued is refused as
//! a `ValidationException` (HTTP 400) rather than read as a resume point.
//!
//! `ListTargetAccountConfigurations` walks the configurations an experiment template
//! holds and `ListExperimentTargetAccountConfigurations` the snapshot its experiment took
//! when `StartExperiment` ran. The two differ only in which collection they walk, in
//! whether the caller can size the page (the experiment-scoped operation models no
//! `maxResults`) and in the summary shape they report — which
//! `smithy_query(query="shape",shape="TargetAccountConfigurationSummary,ExperimentTargetAccountConfigurationSummary")`
//! shows declaring the same three members — so every case below drives BOTH operations and
//! asserts they page the same collection identically.
//!
//! The rows differ along what the rule generalises over: how many configurations the
//! collection holds (none, one, several), the page size the call serves (each published
//! bound of the caller's `maxResults`, sizes between them, and none at all), the length and
//! form of the `accountId` a configuration is named by (both published bounds of
//! `TargetAccountId` and values between them), the shape of its `roleArn`, whether its
//! `description` is absent, empty, plain, non-ASCII or at the published ceiling, and which
//! kind of unissued token a resume is refused for.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{emulator, send, token, Answer, FALLBACK_REGION, WAIT_DURATION};

/// `TargetAccountId`'s published bounds (`@length(min: 12, max: 48)`, from
/// `smithy_query(query="shape",shape="TargetAccountId")`), spelled out here rather than
/// imported from the crate so a wrong bound in the model or the handler fails this suite.
const ACCOUNT_ID_FLOOR: usize = 12;
/// See [`ACCOUNT_ID_FLOOR`].
const ACCOUNT_ID_CEILING: usize = 48;

/// `TargetAccountConfigurationDescription`'s published ceiling
/// (`@length(min: 0, max: 512)`, from the same query).
const DESCRIPTION_CEILING: usize = 512;

/// `ListTargetAccountConfigurationsMaxResults`'s published ceiling
/// (`@range(min: 1, max: 100)`, from
/// `smithy_query(query="shape",shape="ListTargetAccountConfigurationsMaxResults")`).
const MAX_RESULTS_CEILING: i32 = 100;
/// See [`MAX_RESULTS_CEILING`] — the published floor of the same range.
const MAX_RESULTS_FLOOR: i32 = 1;

/// A bound on how many pages one walk may take: every page but the last carries at least
/// one configuration, so a walk over a collection this suite builds ends far inside it.
/// It keeps a mis-minted token from looping forever instead of failing.
const MAX_PAGES: usize = 64;

/// An `experimentTemplateId` no template in a case holds, within `ExperimentTemplateId`'s
/// modelled `@length(0..64)` and `^[\S]+$`.
const MISSING_TEMPLATE_ID: &str = "EXTZZZZZZZZZZZZ";

/// An `experimentId` no experiment in a case holds, within `ExperimentId`'s modelled
/// `@length(0..64)` and `^[\S]+$`.
const MISSING_EXPERIMENT_ID: &str = "EXPZZZZZZZZZZZZ";

/// One of the two list operations, pointed at the resource it enumerates.
#[derive(Clone, Copy)]
enum Listing<'a> {
    /// `ListTargetAccountConfigurations` over an experiment template, at the page size the
    /// caller asked for (`None` for a call that names no `maxResults`).
    Template {
        id: &'a str,
        max_results: Option<i32>,
    },
    /// `ListExperimentTargetAccountConfigurations` over an experiment: the model gives this
    /// operation no `maxResults`, so the page size is the service's own.
    Experiment { id: &'a str },
}

impl Listing<'_> {
    /// How this listing is named in a failing assertion.
    fn label(&self) -> String {
        match self {
            Listing::Template { id, max_results } => {
                format!("ListTargetAccountConfigurations({id}, maxResults={max_results:?})")
            }
            Listing::Experiment { id } => {
                format!("ListExperimentTargetAccountConfigurations({id})")
            }
        }
    }

    /// The largest page this listing may answer, when the call sizes it.
    fn requested_page_size(&self) -> Option<usize> {
        match self {
            Listing::Template { max_results, .. } => max_results
                .map(|size| usize::try_from(size).expect("a published maxResults is positive")),
            Listing::Experiment { .. } => None,
        }
    }

    /// One call of this listing, resuming at `next_token` when there is one.
    async fn page(&self, router: &MultiServiceRouter, next_token: Option<&str>) -> Answer {
        let mut query: Vec<String> = Vec::new();
        if let Listing::Template {
            max_results: Some(max_results),
            ..
        } = self
        {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let uri = match self {
            Listing::Template { id, .. } => {
                format!("/experimentTemplates/{id}/targetAccountConfigurations{query}")
            }
            Listing::Experiment { id } => {
                format!("/experiments/{id}/targetAccountConfigurations{query}")
            }
        };
        send(router, "GET", &uri, None, FALLBACK_REGION).await
    }
}

/// The `targetAccountConfigurations` list an answer carries.
fn configurations(answer: &Answer, case: &str, label: &str) -> Vec<Value> {
    answer
        .member("targetAccountConfigurations")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} {label}: no targetAccountConfigurations list in {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `nextToken` an answer carries, if any.
fn next_token(answer: &Answer) -> Option<String> {
    answer.member("nextToken").as_str().map(str::to_string)
}

/// The `accountId` of every configuration in `listed`.
fn account_ids(listed: &[Value], case: &str, label: &str) -> Vec<String> {
    listed
        .iter()
        .map(|configuration| {
            configuration["accountId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} {label}: no accountId in {configuration:?}")
                })
                .to_string()
        })
        .collect()
}

/// Walk every page of `listing` and report the concatenated configurations, asserting the
/// paging rule as it goes: a page is never longer than the size the call serves, a
/// `nextToken` only ever arrives with a FULL page, and every full page of one walk is the
/// same size.
async fn walk(router: &MultiServiceRouter, listing: Listing<'_>, case: &str) -> Vec<Value> {
    let label = listing.label();
    let mut collected: Vec<Value> = Vec::new();
    let mut resume: Option<String> = None;
    let mut full_page: Option<usize> = None;
    for page in 0..MAX_PAGES {
        let answer = listing.page(router, resume.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} {label} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = configurations(&answer, case, &label);
        if let Some(bound) = listing.requested_page_size() {
            assert!(
                items.len() <= bound,
                "case {case:?} {label} page {page}: a page holds at most {bound} \
                 configurations, got {}",
                items.len()
            );
        }
        match next_token(&answer) {
            None => {
                collected.extend(items);
                return collected;
            }
            Some(next) => {
                let size = *full_page.get_or_insert(items.len());
                assert_eq!(
                    items.len(),
                    size,
                    "case {case:?} {label} page {page}: a nextToken must only arrive with a \
                     full page of {size}, got {} configurations",
                    items.len()
                );
                assert!(
                    !items.is_empty(),
                    "case {case:?} {label} page {page}: a nextToken must not follow an empty \
                     page, got {:?}",
                    answer.body
                );
                collected.extend(items);
                resume = Some(next);
            }
        }
    }
    panic!("case {case:?} {label}: the walk did not end within {MAX_PAGES} pages");
}

/// An account id of exactly `len` characters, starting at `seed`: `TargetAccountId` is
/// modelled `@length(12..48)`, `pattern ^[\S]+$`, and AWS account ids are digit strings.
fn account_id_of(seed: u32, len: usize) -> String {
    let base = format!("{seed:012}");
    if len <= base.len() {
        base[..len].to_string()
    } else {
        format!("{base}{}", "7".repeat(len - base.len()))
    }
}

/// One row: the target account configuration one create request persists.
struct Row {
    case: &'static str,
    account_id: String,
    role_arn: String,
    description: Option<String>,
}

/// The configurations the page walk is asserted over, in an order that is deliberately not
/// their `accountId` order, so the order a page reports cannot be the insertion order by
/// accident.
///
/// A `roleArn` whose account segment differs from the `accountId` is refused by this
/// service, and so is an `accountId` that is no account number, so every row names an
/// account number and a role in that same account.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "middling-account-id-plain-description",
            account_id: "555555555555".to_string(),
            role_arn: "arn:aws:iam::555555555555:role/FisPageWalk".to_string(),
            description: Some("the staging account".to_string()),
        },
        Row {
            case: "account-id-at-the-floor-no-description",
            account_id: account_id_of(1, ACCOUNT_ID_FLOOR),
            role_arn: "arn:aws:iam::000000000001:role/service-role/fis-page-walk".to_string(),
            description: None,
        },
        Row {
            case: "description-at-the-ceiling",
            account_id: "999999999999".to_string(),
            role_arn: "arn:aws:iam::999999999999:role/fis".to_string(),
            description: Some("d".repeat(DESCRIPTION_CEILING)),
        },
        Row {
            case: "empty-description",
            account_id: "210987654321".to_string(),
            role_arn: "arn:aws-us-gov:iam::210987654321:role/fis-page-walk".to_string(),
            description: Some(String::new()),
        },
        Row {
            case: "non-ascii-description",
            account_id: "444444444444".to_string(),
            role_arn: "arn:aws-cn:iam::444444444444:role/fis-目标账户".to_string(),
            description: Some("説明 — ünïcødé ✅ описание".to_string()),
        },
        Row {
            case: "role-arn-with-a-path",
            account_id: "333333333333".to_string(),
            role_arn: "arn:aws:iam::333333333333:role/path/to/fis-page-walk".to_string(),
            description: Some("an account reached through a role with a path".to_string()),
        },
        Row {
            case: "account-id-with-leading-zeros",
            account_id: account_id_of(2, ACCOUNT_ID_FLOOR),
            role_arn: "arn:aws:iam::000000000002:role/FisPageWalk".to_string(),
            description: Some("the account whose id carries leading zeros".to_string()),
        },
        Row {
            case: "account-id-of-one-repeated-figure",
            account_id: "777777777777".to_string(),
            role_arn: "arn:aws:iam::777777777777:role/fis-page-walk".to_string(),
            description: Some("an account id written as one repeated figure".to_string()),
        },
    ]
}

/// Create the experiment template a case hangs its configurations off, and report its id.
///
/// A target account configuration belongs to a template that targets MULTIPLE accounts,
/// and a template that targets multiple accounts cannot start an experiment while it names
/// none — so a case with no configuration at all builds the single-account template, which
/// is the documented default.
async fn template(router: &MultiServiceRouter, case: &str, configurations: usize) -> String {
    let targeting = if configurations == 0 {
        "single-account"
    } else {
        "multi-account"
    };
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": token(&format!("page-walk-{case}")),
            "description": format!("target account configuration page walk: {case}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-page-walk",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "experimentOptions": { "accountTargeting": targeting },
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } }
            }
        })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

/// Persist `row` on `template_id` through `CreateTargetAccountConfiguration`.
async fn create_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    case: &str,
    row: &Row,
) {
    let mut body = Map::new();
    body.insert("roleArn".to_string(), json!(row.role_arn));
    if let Some(description) = &row.description {
        // An absent `description` has to stay absent on the wire.
        body.insert("description".to_string(), json!(description));
    }
    let answer = send(
        router,
        "POST",
        &format!(
            "/experimentTemplates/{template_id}/targetAccountConfigurations/{}",
            row.account_id
        ),
        Some(Value::Object(body)),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?} row {:?}: expected HTTP 200 from CreateTargetAccountConfiguration, got {} \
         with {:?}",
        row.case,
        answer.status,
        answer.body
    );
}

/// Start an experiment from `template_id` — which snapshots the template's configurations
/// onto the experiment — and report its id.
async fn start_experiment(router: &MultiServiceRouter, template_id: &str, case: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": token(&format!("page-walk-start-{case}")),
            "experimentTemplateId": template_id
        })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment.id in {:?}", answer.body))
        .to_string()
}

/// A template holding the first `configurations` rows, the experiment that snapshotted
/// them, and the `accountId`s in the order every page of either list has to report them.
struct Collection {
    template_id: String,
    experiment_id: String,
    sorted_ids: Vec<String>,
}

async fn collection(router: &MultiServiceRouter, case: &str, configurations: usize) -> Collection {
    let template_id = template(router, case, configurations).await;
    let rows = rows();
    assert!(
        configurations <= rows.len(),
        "case {case:?}: this suite declares only {} rows",
        rows.len()
    );
    for row in rows.iter().take(configurations) {
        create_configuration(router, &template_id, case, row).await;
    }
    let experiment_id = start_experiment(router, &template_id, case).await;
    let mut sorted_ids: Vec<String> = rows
        .iter()
        .take(configurations)
        .map(|row| row.account_id.clone())
        .collect();
    sorted_ids.sort();
    Collection {
        template_id,
        experiment_id,
        sorted_ids,
    }
}

#[tokio::test]
async fn both_lists_page_one_collection_identically() {
    // The rows are collection sizes: none at all, one, and several — each walked at every
    // page size a caller may serve and at the page size the service picks for itself.
    for configurations in [0usize, 1, 2, 5, 8] {
        let case = format!("{configurations}-configurations");
        let router = emulator();
        let held = collection(&router, &case, configurations).await;

        // The experiment-scoped list serves the service's own page size: the whole
        // collection, in `accountId` order.
        let from_experiment = walk(
            &router,
            Listing::Experiment {
                id: &held.experiment_id,
            },
            &case,
        )
        .await;
        assert_eq!(
            account_ids(&from_experiment, &case, "ListExperimentTargetAccountConfigurations"),
            held.sorted_ids,
            "case {case:?}: expected the experiment's configurations in accountId order, got \
             {from_experiment:?}"
        );

        // Every page size the caller may serve enumerates the same collection, in the same
        // order, configuration for configuration — which is the walk both operations share.
        for max_results in [
            None,
            Some(MAX_RESULTS_FLOOR),
            Some(2),
            Some(3),
            Some(MAX_RESULTS_CEILING),
        ] {
            let listing = Listing::Template {
                id: &held.template_id,
                max_results,
            };
            let label = listing.label();
            let from_template = walk(&router, listing, &case).await;
            assert_eq!(
                account_ids(&from_template, &case, &label),
                held.sorted_ids,
                "case {case:?} {label}: expected the template's configurations in accountId \
                 order, got {from_template:?}"
            );
            assert_eq!(
                from_template.len(),
                from_experiment.len(),
                "case {case:?} {label}: expected {} configurations, as the experiment-scoped \
                 list reported, got {}",
                from_experiment.len(),
                from_template.len()
            );
            // `TargetAccountConfigurationSummary` and
            // `ExperimentTargetAccountConfigurationSummary` declare the same three members,
            // so the two lists report each configuration member for member — including an
            // absent `description` staying absent and an explicitly empty one normalized to
            // absent.
            for (index, (template_item, experiment_item)) in
                from_template.iter().zip(&from_experiment).enumerate()
            {
                assert_eq!(
                    template_item, experiment_item,
                    "case {case:?} {label} entry {index}: expected the two lists to report the \
                     same configuration, got {template_item:?} and {experiment_item:?}"
                );
            }
        }

        // The round trip: every row that was created is in the page, with what the create
        // persisted.
        for row in rows().iter().take(configurations) {
            let listed = from_experiment
                .iter()
                .find(|configuration| configuration["accountId"].as_str() == Some(&row.account_id))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?} row {:?}: no configuration for {} in {from_experiment:?}",
                        row.case, row.account_id
                    )
                });
            assert_eq!(
                listed["roleArn"].as_str(),
                Some(row.role_arn.as_str()),
                "case {case:?} row {:?}: expected the stored roleArn, got {listed:?}",
                row.case
            );
            let expected_description = row
                .description
                .as_deref()
                .filter(|description| !description.is_empty());
            assert_eq!(
                listed["description"].as_str(),
                expected_description,
                "case {case:?} row {:?}: expected description {expected_description:?}, got \
                 {listed:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_token_one_list_minted_resumes_either_list_at_the_same_account() {
    let case = "cross-resume";
    let router = emulator();
    let held = collection(&router, case, 5).await;

    // The rows are the cut the token was minted at: after the first configuration, after
    // the second, and after the last one that leaves anything behind.
    for cut in [1usize, 2, 4] {
        let minting = Listing::Template {
            id: &held.template_id,
            max_results: Some(i32::try_from(cut).expect("the cut fits an i32")),
        };
        let first = minting.page(&router, None).await;
        assert_eq!(
            first.status,
            200,
            "case {case:?} cut {cut}: expected HTTP 200, got {} with {:?}",
            first.status,
            first.body
        );
        let issued = next_token(&first).unwrap_or_else(|| {
            panic!(
                "case {case:?} cut {cut}: expected a nextToken while configurations remain, got \
                 {:?}",
                first.body
            )
        });

        // Whichever list the token is offered to, it resumes at the SAME account — the one
        // the minting cut left over — and reports the rest of the collection from there.
        for resuming in [
            Listing::Template {
                id: &held.template_id,
                max_results: None,
            },
            Listing::Experiment {
                id: &held.experiment_id,
            },
        ] {
            let label = resuming.label();
            let answer = resuming.page(&router, Some(&issued)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} cut {cut} {label}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let listed = configurations(&answer, case, &label);
            assert_eq!(
                account_ids(&listed, case, &label),
                held.sorted_ids[cut..],
                "case {case:?} cut {cut} {label}: expected the tail of the collection from \
                 {:?}, got {listed:?}",
                held.sorted_ids[cut]
            );
            assert_eq!(
                next_token(&answer),
                None,
                "case {case:?} cut {cut} {label}: expected no nextToken once the tail fits one \
                 page, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn an_unissued_token_is_refused_by_both_lists_alike() {
    let case = "unissued-token";
    let router = emulator();
    let held = collection(&router, case, 3).await;

    // The marker a page token carries is read off a token the service DID issue, rather
    // than spelled out here, so these rows are refused for being tokens the service never
    // issued and not for a marker this suite guessed.
    let minted = Listing::Template {
        id: &held.template_id,
        max_results: Some(MAX_RESULTS_FLOOR),
    }
    .page(&router, None)
    .await;
    let issued = next_token(&minted).unwrap_or_else(|| {
        panic!("case {case:?}: expected a nextToken out of a one-entry page, got {:?}", minted.body)
    });
    let resumed_at = &held.sorted_ids[1];
    let marker = issued
        .strip_suffix(resumed_at.as_str())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {issued:?} to end with the resumed account {resumed_at:?}")
        })
        .to_string();

    // Each row is a `nextToken` neither list ever issued: a plain word, the account id
    // without the marker in front of it, the marker with nothing behind it, the marker with
    // an account id one character under and one over `TargetAccountId`'s published bounds,
    // the token of another paginated list of this service, and the issued token with a
    // mangled marker. Every one of them is within `NextToken`'s modelled `@length(1..1024)`
    // and `^[\S]+$`, so only the server-side rule can refuse it.
    let unissued = [
        ("a-plain-word", "nonsense".to_string()),
        ("the-account-id-without-the-marker", resumed_at.clone()),
        ("the-marker-with-no-account-behind-it", marker.clone()),
        (
            "an-account-one-under-the-floor",
            format!("{marker}{}", account_id_of(4, ACCOUNT_ID_FLOOR - 1)),
        ),
        (
            "an-account-one-over-the-ceiling",
            format!("{marker}{}", account_id_of(4, ACCOUNT_ID_CEILING + 1)),
        ),
        ("another-lists-token", "resolvedTarget.0".to_string()),
        ("a-mangled-marker", format!("x{issued}")),
    ];

    for (row, bad_token) in &unissued {
        let template_listing = Listing::Template {
            id: &held.template_id,
            max_results: None,
        };
        let experiment_listing = Listing::Experiment {
            id: &held.experiment_id,
        };
        let mut messages = Vec::new();
        for listing in [template_listing, experiment_listing] {
            let label = listing.label();
            let answer = listing.page(&router, Some(bad_token)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} row {row:?} ({bad_token:?}) {label}: expected HTTP 400, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?} row {row:?} ({bad_token:?}) {label}: expected a \
                 ValidationException, got {:?}",
                answer.body
            );
            messages.push(
                answer.body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        assert_eq!(
            messages[0], messages[1],
            "case {case:?} row {row:?} ({bad_token:?}): expected both lists to refuse an \
             unissued token with the same message, got {messages:?}"
        );
        assert!(
            !messages[0].is_empty(),
            "case {case:?} row {row:?} ({bad_token:?}): expected a message with the refusal"
        );

        // The token is judged before the resource is looked up, in both lists: a request
        // carrying a token the service never issued is not a request about a resource.
        for listing in [
            Listing::Template {
                id: MISSING_TEMPLATE_ID,
                max_results: None,
            },
            Listing::Experiment {
                id: MISSING_EXPERIMENT_ID,
            },
        ] {
            let label = listing.label();
            let answer = listing.page(&router, Some(bad_token)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} row {row:?} ({bad_token:?}) {label}: expected the token's HTTP \
                 400 even for a resource the store does not hold, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?} row {row:?} ({bad_token:?}) {label}: expected a \
                 ValidationException, got {:?}",
                answer.body
            );
        }
    }

    // The token the service DID issue is accepted by both lists, so the rows above are
    // refused for what they are and not for being tokens at all; and a list naming a
    // resource the store does not hold, with no token to judge first, is a 404.
    for (listing, expected_code) in [
        (
            Listing::Template {
                id: &held.template_id,
                max_results: None,
            },
            None,
        ),
        (
            Listing::Experiment {
                id: &held.experiment_id,
            },
            None,
        ),
        (
            Listing::Template {
                id: MISSING_TEMPLATE_ID,
                max_results: None,
            },
            Some("ResourceNotFoundException"),
        ),
        (
            Listing::Experiment {
                id: MISSING_EXPERIMENT_ID,
            },
            Some("ResourceNotFoundException"),
        ),
    ] {
        let label = listing.label();
        match expected_code {
            None => {
                let answer = listing.page(&router, Some(&issued)).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} {label}: expected HTTP 200 for a token the service issued, \
                     got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    account_ids(&configurations(&answer, case, &label), case, &label),
                    held.sorted_ids[1..],
                    "case {case:?} {label}: expected the tail from {resumed_at:?}, got {:?}",
                    answer.body
                );
            }
            Some(code) => {
                let answer = listing.page(&router, None).await;
                assert_eq!(
                    answer.status, 404,
                    "case {case:?} {label}: expected HTTP 404 for a resource the store does not \
                     hold, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case:?} {label}: expected {code}, got {:?}",
                    answer.body
                );
            }
        }
    }
}
