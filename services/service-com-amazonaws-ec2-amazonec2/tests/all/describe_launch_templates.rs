// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLaunchTemplates`, driven over the wire the service actually
//! speaks — `ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"), through the crate's shared
//! [`crate::ec2_query`] harness.
//!
//! The rule under test is that the operation REPORTS PERSISTED STATE: every template it
//! answers with is a template the store holds, reported with the members the create stored,
//! and the request only SELECTS which of them are reported — by id, by name, by the
//! documented filters and by page. Nothing it does writes, so a read never changes what a
//! later caller sees.
//!
//! Beside that, a launch template id the account holds no template under is not an id this
//! service minted, so it is refused as malformed — every such id of the request named in one
//! message, `LaunchTemplateIds` being a list — while a name no template carries is refused
//! with the name's own not-found code.
//!
//! Each table row is a distinct input along what those rules generalise over: the name's
//! length and character classes, how many tags a template carries, which launch parameters
//! its data holds, which selector list names it, which filter selects it, how large a page
//! is, and in which way an id fails to name a template.

use ec2::ops::types::OperatorResponse;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::ec2_query::{as_timestamp, element, elements, error_code, error_message, filters};

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The `(key, value)` form fields of one request.
type Params = Vec<(String, String)>;

/// One ec2Query call through the shared harness.
async fn api(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    crate::ec2_query::call(router, REGION, action, params).await
}

/// One `DescribeLaunchTemplates` call.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeLaunchTemplates", params).await
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `LaunchTemplateData.<member>` parameter of a create.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// One template to create: the case it stands for, its name, the launch parameters it holds
/// and the tags it carries.
struct Seed {
    case: &'static str,
    name: &'static str,
    data: Vec<(String, String)>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The templates every suite here is written against. The rows differ along what the rules
/// generalise over: the name's length (the modelled floor of 3 characters up to a long one)
/// and its character classes (letters, digits and each of `()./-_`), how many tags the
/// template carries (none, one, two), and which and how many launch parameters its data
/// holds.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            case: "the shortest name the model admits, one tag",
            name: "web",
            data: vec![data("ImageId", "ami-00000000000000001")],
            tags: vec![("Owner", "TeamA")],
        },
        Seed {
            case: "a dotted and hyphenated name, two tags",
            name: "web.tier-1",
            data: vec![
                data("ImageId", "ami-00000000000000002"),
                data("InstanceType", "t3.micro"),
            ],
            tags: vec![("Owner", "TeamB"), ("Env", "prod")],
        },
        Seed {
            case: "a name carrying parentheses and an underscore, no tags",
            name: "db_tier(3)",
            data: vec![
                data("ImageId", "ami-00000000000000003"),
                data("InstanceType", "m5.large"),
                data("EbsOptimized", "true"),
            ],
            tags: Vec::new(),
        },
        Seed {
            case: "a name carrying a slash, one tag",
            name: "Analytics/Reports",
            data: vec![data("KeyName", "analytics-key")],
            tags: vec![("Env", "prod")],
        },
        Seed {
            case: "a long name of digits and letters, one tag",
            name: "lt0123456789012345678901234567890123456789",
            data: vec![data("UserData", "ZWNob2hlbGxv")],
            tags: vec![("Owner", "TeamA")],
        },
    ]
}

/// Create the template `seed` describes and answer the id the create minted.
async fn create_template(router: &MultiServiceRouter, seed: &Seed) -> String {
    let mut params = vec![p("LaunchTemplateName", seed.name)];
    params.extend(seed.data.iter().cloned());
    if !seed.tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", "launch-template"));
        for (index, (key, value)) in seed.tags.iter().enumerate() {
            params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
    }
    let (status, body) = api(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {:?}: creating {:?} must answer HTTP 200, got {status} with body {body}",
        seed.case, seed.name
    );
    element(&body, "launchTemplateId").unwrap_or_else(|| {
        panic!(
            "case {:?}: the create must report the id it minted: {body}",
            seed.case
        )
    })
}

/// Create every seeded template and answer the `(name, id)` pairs, in seed order.
async fn create_every_template(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for seed in seeds() {
        let launch_template_id = create_template(router, &seed).await;
        created.push((seed.name, launch_template_id));
    }
    created
}

/// The launch template ids a describe body reports, in the order the body carries them.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "launchTemplateId")
}

/// Every launch template id the store holds, in the order the store holds them.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .keys()
        .cloned()
        .collect()
}

/// `Filter.N` form fields for one filter and its values.
fn filter(name: &str, values: &[&str]) -> Params {
    filters(&[(name, values)])
}

/// Every template the store holds is reported with the members the create stored: a describe
/// naming one id answers that one template, and a describe naming nothing answers all of
/// them.
#[tokio::test]
async fn a_described_template_is_the_template_the_store_holds() {
    let (router, engine) = crate::ec2_query::emulator();
    let created = create_every_template(&router).await;
    let expected_created_by = ::engine::emulator_arn("iam", "", "root");

    for (seed, (name, launch_template_id)) in seeds().iter().zip(created.iter()) {
        let (status, body) = describe(&router, &[p("LaunchTemplateId.1", launch_template_id.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            seed.case
        );
        assert_eq!(
            reported_ids(&body),
            vec![launch_template_id.clone()],
            "case {:?}: expected exactly the template named, got body {body}",
            seed.case
        );
        assert_eq!(
            element(&body, "launchTemplateName").as_deref(),
            Some(*name),
            "case {:?}: expected the stored name, got body {body}",
            seed.case
        );
        assert_eq!(
            element(&body, "createdBy").as_deref(),
            Some(expected_created_by.as_str()),
            "case {:?}: expected the creating principal, got body {body}",
            seed.case
        );
        assert_eq!(
            element(&body, "defaultVersionNumber").as_deref(),
            Some("1"),
            "case {:?}: the first version is the default one, got body {body}",
            seed.case
        );
        assert_eq!(
            element(&body, "latestVersionNumber").as_deref(),
            Some("1"),
            "case {:?}: the first version is the latest one, got body {body}",
            seed.case
        );
        let create_time = element(&body, "createTime").unwrap_or_default();
        assert!(
            as_timestamp(&create_time).is_some(),
            "case {:?}: expected an ISO 8601 createTime, got {create_time:?} in {body}",
            seed.case
        );

        // The tags the create gave the template, reported as the template's tag set.
        let reported_tags: Vec<(String, String)> = elements(&body, "key")
            .into_iter()
            .zip(elements(&body, "value"))
            .collect();
        let expected_tags: Vec<(String, String)> = seed
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {:?}: expected the stored tags, got {reported_tags:?} from {body}",
            seed.case
        );
    }

    // A describe that names nothing describes every template of the account, in the id order
    // the store holds them.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "describing every template: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        reported_ids(&body),
        stored_ids(&engine),
        "describing every template: expected every stored template, got body {body}"
    );

    // Reading changes nothing a later caller sees: the same read answers the same templates.
    let (status, again) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the second describe: expected HTTP 200, got {status} with body {again}"
    );
    assert_eq!(
        reported_ids(&again),
        reported_ids(&body),
        "a describe must report the same templates twice, got {again}"
    );
}

/// The selector lists narrow the templates reported: `LaunchTemplateIds` and
/// `LaunchTemplateNames` are a UNION of the templates they name, and a template named twice
/// is still reported once.
#[tokio::test]
async fn the_selector_lists_narrow_the_templates_reported() {
    let (router, engine) = crate::ec2_query::emulator();
    let created = create_every_template(&router).await;
    let id_of = |name: &str| -> String {
        created
            .iter()
            .find(|(seed_name, _)| *seed_name == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("the suite created a template named {name:?}"))
    };
    let every_id = stored_ids(&engine);

    // (case, the selecting parameters, the ids the describe must report)
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no selector at all", Vec::new(), every_id.clone()),
        (
            "one id",
            vec![p("LaunchTemplateId.1", id_of("web"))],
            vec![id_of("web")],
        ),
        (
            "two ids",
            vec![
                p("LaunchTemplateId.1", id_of("web.tier-1")),
                p("LaunchTemplateId.2", id_of("db_tier(3)")),
            ],
            sorted(vec![id_of("web.tier-1"), id_of("db_tier(3)")]),
        ),
        (
            "one name",
            vec![p("LaunchTemplateName.1", "Analytics/Reports")],
            vec![id_of("Analytics/Reports")],
        ),
        (
            "two names",
            vec![
                p("LaunchTemplateName.1", "web"),
                p("LaunchTemplateName.2", "db_tier(3)"),
            ],
            sorted(vec![id_of("web"), id_of("db_tier(3)")]),
        ),
        (
            "an id and the name of another template",
            vec![
                p("LaunchTemplateId.1", id_of("web")),
                p("LaunchTemplateName.1", "web.tier-1"),
            ],
            sorted(vec![id_of("web"), id_of("web.tier-1")]),
        ),
        (
            "an id and the name of the SAME template",
            vec![
                p("LaunchTemplateId.1", id_of("web")),
                p("LaunchTemplateName.1", "web"),
            ],
            vec![id_of("web")],
        ),
        (
            "the same id twice",
            vec![
                p("LaunchTemplateId.1", id_of("web")),
                p("LaunchTemplateId.2", id_of("web")),
            ],
            vec![id_of("web")],
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_ids(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from {body}"
        );
    }
}

/// `values`, in the order a describe reports them: launch template id order.
fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// Every filter `DescribeLaunchTemplatesRequest$Filters` documents selects on the attribute
/// it names, each value matched with EC2's `*`/`?` wildcards, several values joined with OR
/// and several filters with AND.
#[tokio::test]
async fn every_documented_filter_selects_the_templates_it_names() {
    let (router, _engine) = crate::ec2_query::emulator();
    let created = create_every_template(&router).await;
    let id_of = |name: &str| -> String {
        created
            .iter()
            .find(|(seed_name, _)| *seed_name == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("the suite created a template named {name:?}"))
    };

    // The create time the store holds for each template, as the wire reports it: the text a
    // `create-time` filter value is compared against. Read once per template, so the row
    // below selects on the instants the store actually holds rather than on a guess.
    let mut create_times: Vec<(String, String)> = Vec::new();
    for (_, launch_template_id) in &created {
        let (_, body) = describe(&router, &[p("LaunchTemplateId.1", launch_template_id.as_str())]).await;
        create_times.push((
            launch_template_id.clone(),
            element(&body, "createTime").unwrap_or_default(),
        ));
    }
    let create_time = create_times
        .iter()
        .find(|(launch_template_id, _)| *launch_template_id == id_of("web"))
        .map(|(_, at)| at.clone())
        .unwrap_or_default();
    let create_day = create_time
        .split_once('T')
        .map(|(day, _)| format!("{day}*"))
        .unwrap_or_default();
    // Every template created in the same second as that one: which templates share an
    // instant is a fact of this run, so the expectation is read off the store rather than
    // fixed.
    let same_instant: Vec<String> = create_times
        .iter()
        .filter(|(_, at)| *at == create_time)
        .map(|(launch_template_id, _)| launch_template_id.clone())
        .collect();

    // (case, the filters, the ids the describe must report)
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "launch-template-name, one exact value",
            filter("launch-template-name", &["db_tier(3)"]),
            vec![id_of("db_tier(3)")],
        ),
        (
            "launch-template-name, two values joined with OR",
            filter("launch-template-name", &["web", "Analytics/Reports"]),
            sorted(vec![id_of("web"), id_of("Analytics/Reports")]),
        ),
        (
            "launch-template-name, a trailing wildcard",
            filter("launch-template-name", &["web*"]),
            sorted(vec![id_of("web"), id_of("web.tier-1")]),
        ),
        (
            "launch-template-name, a single-character wildcard",
            filter("launch-template-name", &["web.tier-?"]),
            vec![id_of("web.tier-1")],
        ),
        (
            "launch-template-name, a value no template carries",
            filter("launch-template-name", &["no.such.template"]),
            Vec::new(),
        ),
        (
            "tag-key, a key two templates carry",
            filter("tag-key", &["Owner"]),
            sorted(vec![
                id_of("web"),
                id_of("web.tier-1"),
                id_of("lt0123456789012345678901234567890123456789"),
            ]),
        ),
        (
            "tag-key, a key no template carries",
            filter("tag-key", &["Project"]),
            Vec::new(),
        ),
        (
            "tag:<key>, the value two templates hold for it",
            filter("tag:Owner", &["TeamA"]),
            sorted(vec![
                id_of("web"),
                id_of("lt0123456789012345678901234567890123456789"),
            ]),
        ),
        (
            "tag:<key>, a wildcard over the values",
            filter("tag:Owner", &["Team*"]),
            sorted(vec![
                id_of("web"),
                id_of("web.tier-1"),
                id_of("lt0123456789012345678901234567890123456789"),
            ]),
        ),
        (
            "tag:<key>, a key one template holds another value for",
            filter("tag:Env", &["prod"]),
            sorted(vec![id_of("web.tier-1"), id_of("Analytics/Reports")]),
        ),
        (
            "create-time, the instant the store holds",
            filter("create-time", &[create_time.as_str()]),
            sorted(same_instant.clone()),
        ),
        (
            "create-time, a wildcard over the day",
            filter("create-time", &[create_day.as_str()]),
            sorted(created.iter().map(|(_, id)| id.clone()).collect()),
        ),
        (
            "create-time, a day no template was created on",
            filter("create-time", &["1970-01-01*"]),
            Vec::new(),
        ),
        (
            "two filters joined with AND",
            filters(&[("tag-key", &["Owner"]), ("launch-template-name", &["web*"])]),
            sorted(vec![id_of("web"), id_of("web.tier-1")]),
        ),
        (
            "two filters no template satisfies together",
            filters(&[
                ("tag:Owner", &["TeamA"]),
                ("launch-template-name", &["db_tier(3)"]),
            ]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_ids(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from {body}"
        );
    }
}

/// A page is cut at `MaxResults` and walked by the token the previous page answered: every
/// template is reported once, in id order, and the last page carries no token.
#[tokio::test]
async fn a_page_is_cut_at_max_results_and_walked_by_the_token() {
    let (router, engine) = crate::ec2_query::emulator();
    create_every_template(&router).await;
    let every_id = stored_ids(&engine);

    // Page sizes that divide the store, that do not, that take it in one page, and one
    // larger than the store holds.
    for page_size in [1usize, 2, 3, 5, 200] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case MaxResults={page_size}: a page carries at most that many templates, \
                 got {page:?}"
            );
            walked.extend(page);
            next_token = crate::ec2_query::member(&body, "nextToken").filter(|t| !t.is_empty());
            pages += 1;
            assert!(
                pages <= every_id.len() + 1,
                "case MaxResults={page_size}: the walk must terminate, got {walked:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every_id,
            "case MaxResults={page_size}: the pages together must be every template once, \
             got {walked:?}"
        );
    }
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, _engine) = crate::ec2_query::emulator();
    create_every_template(&router).await;

    // The published bounds (`DescribeLaunchTemplatesMaxResults`, `range 1..200`) and one
    // past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (200, 200), (201, 400)] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} \
             with body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a position.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = crate::ec2_query::emulator();
    create_every_template(&router).await;
    let first_id = stored_ids(&engine).first().cloned().unwrap_or_default();

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // launch template id a caller might guess the token is, and that id hex-encoded.
    let rows = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        first_id.clone(),
        ::hex::encode(&first_id),
    ];
    for token in rows {
        let (status, body) = describe(&router, &[p("NextToken", token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// A launch template id the account holds no template under is not an id this service
/// minted, so it is refused as malformed — every such id of the request named in the one
/// message — and a name no template carries is refused with the name's own not-found code.
#[tokio::test]
async fn an_id_or_a_name_that_names_no_template_is_refused() {
    let (router, engine) = crate::ec2_query::emulator();
    let created = create_every_template(&router).await;
    let known = created
        .first()
        .map(|(_, id)| id.clone())
        .unwrap_or_default();
    let every_id = stored_ids(&engine);

    // Rows: an id in each published id form that names no template, an id that is not
    // hexadecimal, one without the resource prefix, one of a length no EC2 id has, one of
    // another resource kind, two unknown ids at once, and a known id beside an unknown one.
    let id_rows: Vec<(&str, Vec<&str>)> = vec![
        ("the long id form, naming no template", vec![
            "lt-0123456789abcdef0",
        ]),
        ("the short id form, naming no template", vec!["lt-0fedcba9"]),
        ("an id that is not hexadecimal", vec!["lt-zzzzzzzzzzzzzzzzz"]),
        ("an id without the resource prefix", vec![
            "00000000000000001",
        ]),
        ("an id of a length no EC2 id has", vec!["lt-0123456"]),
        ("an id of another resource kind", vec![
            "sg-00000000000000001",
        ]),
        ("two ids naming no template", vec![
            "lt-00000000000000099",
            "lt-0fedcba987654321f",
        ]),
    ];

    for (case, ids) in id_rows {
        let params: Params = ids
            .iter()
            .enumerate()
            .map(|(index, id)| p(&format!("LaunchTemplateId.{}", index + 1), *id))
            .collect();
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidLaunchTemplateId.Malformed",
            "case {case:?}: expected the malformed-id code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The following launch template ids are malformed: {}",
                ids.join(", ")
            ),
            "case {case:?}: the refusal must name every id it refused, got {body}"
        );
    }

    // A known id beside an unknown one: only the id the account holds no template under is
    // named in the refusal.
    let (status, body) = describe(
        &router,
        &[
            p("LaunchTemplateId.1", known.clone()),
            p("LaunchTemplateId.2", "lt-00000000000000099"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a known id beside an unknown one\": expected HTTP 400, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_message(&body),
        "The following launch template ids are malformed: lt-00000000000000099",
        "case \"a known id beside an unknown one\": the refusal must name only the id that \
         names no template, got {body}"
    );

    // The member is a bare string the model puts no length on, so the value the refusal
    // repeats back is cut to the length every refusal of this crate echoes
    // (`ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS`), far above any id EC2 issues.
    let oversize = format!("lt-{}", "0".repeat(400));
    let (status, body) = describe(&router, &[p("LaunchTemplateId.1", oversize.clone())]).await;
    assert_eq!(
        status, 400,
        "case \"an id far longer than any EC2 issues\": expected HTTP 400, got {status} with \
         body {body}"
    );
    let echoed: String = oversize
        .chars()
        .take(ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS)
        .collect();
    assert_eq!(
        error_message(&body),
        format!("The following launch template ids are malformed: {echoed}"),
        "case \"an id far longer than any EC2 issues\": the refusal must name the id cut to \
         {} characters, got {body}",
        ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS
    );

    // A name no template carries, including one beside a name that resolves.
    let name_rows: Vec<(&str, Params)> = vec![
        (
            "a name nothing was created under",
            vec![p("LaunchTemplateName.1", "no.such.template")],
        ),
        (
            "an unknown name beside a known one",
            vec![
                p("LaunchTemplateName.1", "web"),
                p("LaunchTemplateName.2", "no.such.template"),
            ],
        ),
    ];
    for (case, params) in name_rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidLaunchTemplateName.NotFoundException",
            "case {case:?}: expected the name's own not-found code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "At least one of the launch templates specified in the request does not exist.",
            "case {case:?}: expected EC2's own sentence for a name it could not resolve, \
             which names none of the request's values, got {body}"
        );
    }

    // Every refusal wrote nothing: the store still holds exactly the templates it held.
    assert_eq!(
        stored_ids(&engine),
        every_id,
        "a refused describe must leave the store alone"
    );
}

/// A filter name this operation does not document is refused, whether or not any template
/// would have matched it.
#[tokio::test]
async fn a_filter_this_operation_does_not_document_is_refused() {
    let (router, _engine) = crate::ec2_query::emulator();
    create_every_template(&router).await;

    // Rows: a name no EC2 operation documents, a `tag:` prefix with no key, the documented
    // names with the wrong case ("filter names are case-sensitive"), and names this
    // operation's `Filters` does not publish although a neighbouring describe does.
    let rows = [
        "no-such-filter",
        "tag:",
        "Tag-Key",
        "Create-Time",
        "launch-template-id",
        "is-default-version",
    ];
    for name in rows {
        let (status, body) = describe(&router, &filter(name, &["x"])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// A template an Amazon Web Services service manages and hides is reported only when the
/// request asks for managed resources: "If this parameter is set to true, the output includes
/// resources that are managed by Amazon Web Services services, even if managed resource
/// visibility is set to hidden" (`DescribeLaunchTemplatesRequest$IncludeManagedResources`),
/// hidden being `OperatorResponse$HiddenByDefault`.
#[tokio::test]
async fn a_managed_and_hidden_template_is_reported_only_when_asked_for() {
    let (router, engine) = crate::ec2_query::emulator();
    let seed = Seed {
        case: "the managed template",
        name: "managed.template",
        data: vec![data("ImageId", "ami-00000000000000001")],
        tags: Vec::new(),
    };
    let launch_template_id = create_template(&router, &seed).await;

    // (case, what the template's operator says about it, whether a request that does NOT ask
    // for managed resources sees it)
    let rows: Vec<(&str, Option<OperatorResponse>, bool)> = vec![
        ("no operator at all", None, true),
        (
            "managed but not hidden",
            Some(OperatorResponse {
                managed: Some(true),
                principal: Some(::engine::emulator_arn("iam", "", "role/aws-service-role")),
                hidden_by_default: Some(false),
            }),
            true,
        ),
        (
            "managed and hidden",
            Some(OperatorResponse {
                managed: Some(true),
                principal: Some(::engine::emulator_arn("iam", "", "role/aws-service-role")),
                hidden_by_default: Some(true),
            }),
            false,
        ),
    ];

    for (case, operator, visible_by_default) in rows {
        {
            let state = engine.state::<Ec2State>();
            let mut store = state.lock();
            let record = store
                .launch_templates
                .get_mut(&launch_template_id)
                .unwrap_or_else(|| panic!("case {case:?}: the template must be stored"));
            record.template.operator = operator.clone();
        }

        for asked in [None, Some("false"), Some("true")] {
            let mut params = vec![p("LaunchTemplateId.1", launch_template_id.clone())];
            if let Some(value) = asked {
                params.push(p("IncludeManagedResources", value));
            }
            let asks_for_managed = asked == Some("true");
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?} with IncludeManagedResources={asked:?}: expected HTTP 200, \
                 got {status} with body {body}"
            );
            let expected: Vec<String> = if visible_by_default || asks_for_managed {
                vec![launch_template_id.clone()]
            } else {
                Vec::new()
            };
            let reported = reported_ids(&body);
            assert_eq!(
                reported, expected,
                "case {case:?} with IncludeManagedResources={asked:?}: expected {expected:?}, \
                 got {reported:?} from {body}"
            );
        }
    }
}

/// A dry run reports the permission check and describes nothing — and a request that is not
/// well formed is answered with its own refusal instead, never getting as far as the check.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_describes_nothing() {
    let (router, engine) = crate::ec2_query::emulator();
    let created = create_every_template(&router).await;
    let known = created
        .first()
        .map(|(_, id)| id.clone())
        .unwrap_or_default();
    let every_id = stored_ids(&engine);

    // (case, the rest of the request, the status and code the dry run must answer)
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        ("a dry run naming no template", Vec::new(), 412, "DryRunOperation"),
        (
            "a dry run naming a stored template",
            vec![p("LaunchTemplateId.1", known.clone())],
            412,
            "DryRunOperation",
        ),
        (
            "a dry run naming an id that names no template",
            vec![p("LaunchTemplateId.1", "lt-0123456789abcdef0")],
            400,
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a dry run naming a name nothing was created under",
            vec![p("LaunchTemplateName.1", "no.such.template")],
            400,
            "InvalidLaunchTemplateName.NotFoundException",
        ),
        (
            "a dry run with an undocumented filter",
            filter("no-such-filter", &["x"]),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, rest, expected_status, expected_code) in rows {
        let mut params = vec![p("DryRun", "true")];
        params.extend(rest);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a refused describe reports no template, got {body}"
        );
    }

    // The dry runs changed nothing: every template is still there, and still describable.
    assert_eq!(
        stored_ids(&engine),
        every_id,
        "a dry run must leave the store alone"
    );
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "after the dry runs: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        reported_ids(&body),
        every_id,
        "after the dry runs: expected every template, got body {body}"
    );
}
