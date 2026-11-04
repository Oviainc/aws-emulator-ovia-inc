// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Rekognition's project lifecycle over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Operation>`),
//! dispatched through the same [`MultiServiceRouter`] the production emulator mounts.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body: nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! THE RULE UNDER TEST: `CreateProject` records a project in this service's store — the
//! name the request supplied, the customization feature it asked for (or the default the
//! member documents), the auto-update setting it supplied, and the instant and ARN the
//! service assigned — answers with that ARN, and refuses a second create under a live
//! project's name without changing it; `DescribeProjects` reports exactly the records the
//! store holds, filtered by the names and features the request asks for, in creation
//! order, one `MaxResults`-sized page at a time.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what it generalises
//! over: the project name's form and length (both published `ProjectName` bounds), the
//! feature (named and defaulted), whether auto-update was asked for, the number of tags,
//! the region the call is signed in — which is where the ARN's region comes from — and
//! the page size (both published `ProjectsPageSize` bounds and one past each).
//!
//! What is NOT asserted here: the tags a create attaches, which the tagging suite asserts
//! through the operations that report and change them; the rows below still create
//! projects with and without tags, so the create path is exercised either way.

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helpers and the reduced reply all come from the
// collection-lifecycle suite rather than being built a second time here.
use crate::face_collections::{call, call_in, harness, Reply};

/// `ProjectName` is `len 1..255, pattern ^[a-zA-Z0-9_.\-]+$`
/// (`smithy_query(query="operation",operation="CreateProject")`).
const PROJECT_NAME_MAX: usize = 255;

/// `MaxResults` is `ProjectsPageSize`, `range 1..100` on `DescribeProjects`
/// (`smithy_query(query="operation",operation="DescribeProjects")`).
const PROJECTS_PAGE_SIZE_MIN: i64 = 1;
const PROJECTS_PAGE_SIZE_MAX: i64 = 100;

/// The two `CustomizationFeature` values a project can be customized for
/// (`smithy_query(query="shape",shape="CustomizationFeature")`: CONTENT_MODERATION |
/// CUSTOM_LABELS).
const CUSTOM_LABELS: &str = "CUSTOM_LABELS";
const CONTENT_MODERATION: &str = "CONTENT_MODERATION";

/// The two `ProjectAutoUpdate` values (`smithy_query(query="shape",
/// shape="ProjectAutoUpdate")`: ENABLED | DISABLED).
const AUTO_UPDATE_ENABLED: &str = "ENABLED";
const AUTO_UPDATE_DISABLED: &str = "DISABLED";

/// One row of the project-lifecycle table: the name, what the project is customized for,
/// whether it was asked to auto-update, the tags created with it, and the region the call
/// is signed in.
struct ProjectCase {
    project_name: String,
    feature: Option<&'static str>,
    auto_update: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    region: &'static str,
}

impl ProjectCase {
    /// The feature this project ends up customized for: the one the request named, or
    /// the default `Feature` documents when it named none.
    fn effective_feature(&self) -> &str {
        self.feature.unwrap_or(CUSTOM_LABELS)
    }

    /// This row as a `CreateProject` request body.
    fn request(&self) -> Value {
        let mut body = json!({ "ProjectName": self.project_name });
        if let Some(feature) = self.feature {
            body["Feature"] = json!(feature);
        }
        if let Some(auto_update) = self.auto_update {
            body["AutoUpdate"] = json!(auto_update);
        }
        if !self.tags.is_empty() {
            body["Tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        body
    }
}

/// One row of the `DescribeProjects` filter table: which names and features the request
/// asks for, and the ARNs of the projects that must come back.
struct FilterCase {
    case: &'static str,
    names: Option<Vec<&'static str>>,
    features: Option<Vec<&'static str>>,
    expected: Vec<String>,
}

/// `CreateProject` for `body`, signed in `region`.
async fn create_project_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "CreateProject", body).await
}

/// Create a project named `project_name` customized for `feature`, and answer the ARN
/// the service minted for it.
async fn create_project(
    router: &MultiServiceRouter,
    project_name: &str,
    feature: &str,
) -> String {
    let mut body = json!({ "ProjectName": project_name, "Feature": feature });
    if feature == CONTENT_MODERATION {
        // Documented on the member: AutoUpdate is a required argument for Content
        // Moderation.
        body["AutoUpdate"] = json!(AUTO_UPDATE_DISABLED);
    }
    let reply = call(router, "CreateProject", body).await;
    assert_eq!(
        reply.status, 200,
        "case {project_name}/{feature}: creating a project must succeed, got {:?}",
        reply.body
    );
    reply
        .string("ProjectArn")
        .unwrap_or_else(|| panic!("case {project_name}/{feature}: a ProjectArn must come back"))
}

/// `DescribeProjects` with the names and features a row asks for, and no page size.
async fn describe_projects(
    router: &MultiServiceRouter,
    names: Option<&[&str]>,
    features: Option<&[&str]>,
) -> Reply {
    let mut body = json!({});
    if let Some(names) = names {
        body["ProjectNames"] = json!(names);
    }
    if let Some(features) = features {
        body["Features"] = json!(features);
    }
    call(router, "DescribeProjects", body).await
}

/// The one description `DescribeProjects` must have reported for `project_name`.
fn only_description(case: &str, reply: &Reply) -> Value {
    let descriptions = reply.list("ProjectDescriptions");
    assert_eq!(
        descriptions.len(),
        1,
        "case {case:?}: expected exactly one project description, got {:?}",
        reply.body
    );
    descriptions[0].clone()
}

fn member(description: &Value, name: &str) -> Option<String> {
    description
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The numeric version segment of a project ARN
/// (`ProjectArn`'s pattern ends `project\/[a-zA-Z0-9_.\-]{1,255}\/[0-9]+$`), as a
/// number — `None` when the ARN does not carry one.
fn arn_version(arn: &str) -> Option<u128> {
    arn.rsplit('/').next()?.parse().ok()
}

/// Milliseconds since the Unix epoch, now.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis()
}

/// Assert that `arn` is the ARN the service must mint for a project of `project_name`
/// created in `region` at about now.
fn assert_project_arn(case: &str, arn: &str, region: &str, project_name: &str) {
    let prefix = format!(
        "arn:aws:rekognition:{region}:{}:project/{project_name}/",
        engine::emulator_account_id()
    );
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
    let version = arn_version(arn)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric version segment, got {arn:?}"));
    // The version is the project's creation instant in milliseconds, so it must sit
    // within a generous window of this test's own clock rather than be a constant.
    let now = now_millis();
    assert!(
        version <= now && now - version < 300_000,
        "case {case:?}: the ARN version must be the creation instant in milliseconds, \
         got {version} against {now}"
    );
    assert!(
        arn.len() >= 20,
        "case {case:?}: ProjectArn is len 20..2048, got {arn:?}"
    );
}

#[tokio::test]
async fn a_project_is_a_stored_resource_describe_projects_reports() {
    let router = harness();

    // Rows differ along what the rule generalises over: the name's form and length
    // (including the modelled 1- and 255-character bounds of `ProjectName`), whether the
    // request names a `Feature` at all, whether it asks for auto-update, the number of
    // tags, and the region the request is signed in — which is where the ARN's region
    // comes from.
    let cases: Vec<ProjectCase> = vec![
        ProjectCase {
            project_name: "my-project".to_string(),
            feature: None,
            auto_update: None,
            tags: vec![],
            region: "us-east-1",
        },
        ProjectCase {
            project_name: "Team.Project_2026-Q1".to_string(),
            feature: Some(CUSTOM_LABELS),
            auto_update: None,
            tags: vec![("env", "test"), ("owner", "qa")],
            region: "eu-west-1",
        },
        ProjectCase {
            project_name: "p".to_string(),
            feature: Some(CONTENT_MODERATION),
            auto_update: Some(AUTO_UPDATE_ENABLED),
            tags: vec![("only", "one")],
            region: "us-west-2",
        },
        ProjectCase {
            project_name: "moderation.adapter_0".to_string(),
            feature: Some(CONTENT_MODERATION),
            auto_update: Some(AUTO_UPDATE_DISABLED),
            tags: vec![],
            region: "ap-southeast-3",
        },
        ProjectCase {
            project_name: "q".repeat(PROJECT_NAME_MAX),
            feature: Some(CUSTOM_LABELS),
            auto_update: None,
            tags: vec![],
            region: "us-east-1",
        },
    ];

    for case in &cases {
        let name = case.project_name.as_str();
        let created = create_project_in(&router, case.region, case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: creating a project must succeed, got {:?}",
            created.body
        );
        let arn = created
            .string("ProjectArn")
            .unwrap_or_else(|| panic!("case {name:?}: a ProjectArn must come back"));
        assert_project_arn(name, &arn, case.region, name);

        // The read back: what the create recorded is what the reader reports.
        let described = describe_projects(
            &router,
            Some(&[name]),
            Some(&[case.effective_feature()]),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: describing projects must succeed, got {:?}",
            described.body
        );
        let description = only_description(name, &described);
        assert_eq!(
            member(&description, "ProjectArn").as_deref(),
            Some(arn.as_str()),
            "case {name:?}: the described project must carry the ARN the create minted, \
             got {description:?}"
        );
        assert_eq!(
            member(&description, "Status").as_deref(),
            Some("CREATED"),
            "case {name:?}: a created project is reported CREATED, got {description:?}"
        );
        assert_eq!(
            member(&description, "Feature").as_deref(),
            Some(case.effective_feature()),
            "case {name:?}: the described feature must be the one in force, \
             got {description:?}"
        );
        assert_eq!(
            member(&description, "AutoUpdate").as_deref(),
            case.auto_update,
            "case {name:?}: AutoUpdate must be exactly what the create carried, \
             got {description:?}"
        );
        assert_eq!(
            description
                .get("Datasets")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {name:?}: a project with no datasets has an empty dataset list, \
             got {description:?}"
        );
        // The creation timestamp and the ARN's version segment are the same recorded
        // instant, reported in seconds and in milliseconds.
        let creation = description
            .get("CreationTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {name:?}: a CreationTimestamp must come back"));
        let version = arn_version(&arn).expect("a numeric version segment");
        assert!(
            (creation - (version as f64) / 1000.0).abs() < 1.0,
            "case {name:?}: CreationTimestamp {creation} and the ARN version {version} must \
             be the same instant"
        );

        // A second create under the same name is a collision, and it changes nothing.
        let again = create_project_in(&router, case.region, case.request()).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, "ResourceInUseException"),
            "case {name:?}: a second create of a live project name must be refused, \
             got {:?}",
            again.body
        );
        let reread = describe_projects(
            &router,
            Some(&[name]),
            Some(&[case.effective_feature()]),
        )
        .await;
        assert_eq!(
            member(&only_description(name, &reread), "ProjectArn").as_deref(),
            Some(arn.as_str()),
            "case {name:?}: a refused create must leave the project it collided with \
             untouched, got {:?}",
            reread.body
        );
    }
}

#[tokio::test]
async fn describe_projects_reports_the_names_and_features_the_request_asks_for() {
    let router = harness();

    let labels_one = create_project(&router, "labels-one", CUSTOM_LABELS).await;
    let labels_two = create_project(&router, "labels-two", CUSTOM_LABELS).await;
    let moderation = create_project(&router, "moderation-one", CONTENT_MODERATION).await;

    let both_features = vec![CUSTOM_LABELS, CONTENT_MODERATION];

    // Rows differ along the filters: no filter at all, each feature alone, both
    // features, one name, several names, and a name no project carries.
    let cases: Vec<FilterCase> = vec![
        // `Features` documents its own default: "If no value is specified, CUSTOM_LABELS
        // is used as a default"
        // (`smithy_query(query="doc",shape="DescribeProjectsRequest",member="Features")`).
        FilterCase {
            case: "no filter",
            names: None,
            features: None,
            expected: vec![labels_one.clone(), labels_two.clone()],
        },
        FilterCase {
            case: "custom labels",
            names: None,
            features: Some(vec![CUSTOM_LABELS]),
            expected: vec![labels_one.clone(), labels_two.clone()],
        },
        FilterCase {
            case: "content moderation",
            names: None,
            features: Some(vec![CONTENT_MODERATION]),
            expected: vec![moderation.clone()],
        },
        FilterCase {
            case: "both features",
            names: None,
            features: Some(both_features.clone()),
            expected: vec![labels_one.clone(), labels_two.clone(), moderation.clone()],
        },
        FilterCase {
            case: "one name",
            names: Some(vec!["labels-two"]),
            features: Some(both_features.clone()),
            expected: vec![labels_two.clone()],
        },
        FilterCase {
            case: "several names",
            names: Some(vec!["labels-one", "moderation-one"]),
            features: Some(both_features.clone()),
            expected: vec![labels_one.clone(), moderation.clone()],
        },
        FilterCase {
            case: "a name and a feature that do not meet",
            names: Some(vec!["moderation-one"]),
            features: Some(vec![CUSTOM_LABELS]),
            expected: vec![],
        },
        FilterCase {
            case: "a name no project carries",
            names: Some(vec!["absent-project"]),
            features: Some(both_features.clone()),
            expected: vec![],
        },
    ];

    for row in &cases {
        let case = row.case;
        let reply = describe_projects(
            &router,
            row.names.as_deref(),
            row.features.as_deref(),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: describing projects must succeed, got {:?}",
            reply.body
        );
        let got: Vec<String> = reply
            .list("ProjectDescriptions")
            .iter()
            .filter_map(|description| member(description, "ProjectArn"))
            .collect();
        assert_eq!(
            got, row.expected,
            "case {case:?}: expected exactly the matching projects, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_projects_page_is_cut_out_of_creation_order() {
    let router = harness();

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let name = format!("page-project-{index}");
        created.push(create_project(&router, &name, CUSTOM_LABELS).await);
    }

    // The unpaged answer is the order every paged walk must reproduce, and it is
    // creation order: `ProjectDescriptions` is "sorted by the date and time the projects
    // are created".
    let whole = describe_projects(&router, None, Some(&[CUSTOM_LABELS])).await;
    let order: Vec<String> = whole
        .list("ProjectDescriptions")
        .iter()
        .filter_map(|description| member(description, "ProjectArn"))
        .collect();
    assert_eq!(
        order, created,
        "the unpaged answer must report every project in creation order, got {:?}",
        whole.body
    );
    assert_eq!(
        whole.string("NextToken"),
        None,
        "a page that holds every project mints no cursor, got {:?}",
        whole.body
    );
    let timestamps: Vec<f64> = whole
        .list("ProjectDescriptions")
        .iter()
        .filter_map(|description| description.get("CreationTimestamp").and_then(Value::as_f64))
        .collect();
    assert!(
        timestamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "the reported order must be by creation time, got {timestamps:?}"
    );

    // Rows differ along the page size, from one project per page to more than the store
    // holds, plus both published bounds of `ProjectsPageSize` and "no MaxResults at all".
    for max_results in [
        Some(PROJECTS_PAGE_SIZE_MIN),
        Some(2),
        Some(3),
        Some(5),
        Some(7),
        Some(PROJECTS_PAGE_SIZE_MAX),
        None,
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "Features": [CUSTOM_LABELS] });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(cursor) = &token {
                body["NextToken"] = json!(cursor);
            }
            let reply = call(&router, "DescribeProjects", body).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?}: a page must succeed, got {:?}",
                reply.body
            );
            let page: Vec<String> = reply
                .list("ProjectDescriptions")
                .iter()
                .filter_map(|description| member(description, "ProjectArn"))
                .collect();
            if let Some(max) = max_results {
                assert!(
                    page.len() as i64 <= max,
                    "case MaxResults={max_results:?}: a page must not exceed the cap, \
                     got {} projects",
                    page.len()
                );
            }
            walked.extend(page);
            token = reply.string("NextToken");
            assert_eq!(
                token.is_some(),
                walked.len() < order.len(),
                "case MaxResults={max_results:?}: a NextToken must come back exactly while \
                 projects remain, walked {} of {}",
                walked.len(),
                order.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, order,
            "case MaxResults={max_results:?}: the pages must walk creation order exactly once"
        );
    }

    // A cursor resumes where its page ended even when the store changed in between: a
    // project created after the token was minted is past the end of the walk, and one
    // created before it is not revisited.
    let first_page = call(
        &router,
        "DescribeProjects",
        json!({ "Features": [CUSTOM_LABELS], "MaxResults": 2 }),
    )
    .await;
    let token = first_page
        .string("NextToken")
        .expect("a token while projects remain");
    let late = create_project(&router, "page-project-late", CUSTOM_LABELS).await;
    let resumed = call(
        &router,
        "DescribeProjects",
        json!({ "Features": [CUSTOM_LABELS], "NextToken": token }),
    )
    .await;
    let got: Vec<String> = resumed
        .list("ProjectDescriptions")
        .iter()
        .filter_map(|description| member(description, "ProjectArn"))
        .collect();
    let mut want: Vec<String> = order[2..].to_vec();
    want.push(late);
    assert_eq!(
        got, want,
        "a cursor must resume exactly where its page ended, got {:?}",
        resumed.body
    );

    // A token this list did not mint names no position in it — including one another
    // list of this service did mint.
    let faces_cursor = state::encode_cursor(state::FACES_CURSOR_SCOPE, "0");
    for token in [
        "",
        "not-a-token",
        "zz",
        // Hex of "ab": it decodes, but this list never minted it.
        "6162",
        faces_cursor.as_str(),
    ] {
        let reply = call(
            &router,
            "DescribeProjects",
            json!({ "Features": [CUSTOM_LABELS], "NextToken": token }),
        )
        .await;
        if token.is_empty() {
            // `ExtendedPaginationToken` is `len 0..1024`, so the empty token is inside
            // the model's range: it names no position and asks for the first page.
            assert_eq!(
                reply.status, 200,
                "case {token:?}: an empty token asks for the first page, got {:?}",
                reply.body
            );
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidPaginationTokenException"),
            "case {token:?}: a token the service did not mint must be refused, got {:?}",
            reply.body
        );
    }

    // One past each published bound of `ProjectsPageSize` is outside the model's range,
    // so it is refused rather than served.
    for max_results in [PROJECTS_PAGE_SIZE_MIN - 1, PROJECTS_PAGE_SIZE_MAX + 1] {
        let reply = call(
            &router,
            "DescribeProjects",
            json!({ "Features": [CUSTOM_LABELS], "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case MaxResults={max_results}: a page size outside the modelled range must be \
             refused, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_content_moderation_project_without_auto_update_is_refused() {
    let router = harness();

    // `AutoUpdate` is documented as a required argument for Content Moderation, and as
    // applicable only to adapters — so the same request is refused for that feature and
    // served for the other.
    let refused = call(
        &router,
        "CreateProject",
        json!({ "ProjectName": "moderation-no-auto-update", "Feature": CONTENT_MODERATION }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "InvalidParameterException"),
        "a Content Moderation project without AutoUpdate must be refused, got {:?}",
        refused.body
    );

    let described = describe_projects(
        &router,
        Some(&["moderation-no-auto-update"]),
        Some(&[CUSTOM_LABELS, CONTENT_MODERATION]),
    )
    .await;
    assert_eq!(
        described.list("ProjectDescriptions").len(),
        0,
        "a refused create must store nothing, got {:?}",
        described.body
    );

    for (case, body) in [
        (
            "custom labels needs no auto-update",
            json!({ "ProjectName": "labels-no-auto-update", "Feature": CUSTOM_LABELS }),
        ),
        (
            "no feature needs no auto-update",
            json!({ "ProjectName": "default-no-auto-update" }),
        ),
        (
            "content moderation with auto-update",
            json!({
                "ProjectName": "moderation-with-auto-update",
                "Feature": CONTENT_MODERATION,
                "AutoUpdate": AUTO_UPDATE_ENABLED,
            }),
        ),
    ] {
        let reply = call(&router, "CreateProject", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the create must succeed, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_project_store_refuses_to_grow_past_its_capacity() {
    let router = harness();

    for index in 0..state::MAX_PROJECTS {
        let name = format!("project-{index:04}");
        let reply = call(&router, "CreateProject", json!({ "ProjectName": name })).await;
        assert_eq!(
            reply.status, 200,
            "case {name}: creating up to the capacity must succeed, got {:?}",
            reply.body
        );
    }
    let over = call(
        &router,
        "CreateProject",
        json!({ "ProjectName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "LimitExceededException"),
        "case projects={}: creating past the capacity must be refused, got {:?}",
        state::MAX_PROJECTS,
        over.body
    );

    // A refused create stores nothing: the store still holds exactly its capacity, and
    // the project the refused request named is not among them.
    let mut walked = 0_usize;
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": PROJECTS_PAGE_SIZE_MAX });
        if let Some(cursor) = &token {
            body["NextToken"] = json!(cursor);
        }
        let reply = call(&router, "DescribeProjects", body).await;
        walked += reply.list("ProjectDescriptions").len();
        token = reply.string("NextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked,
        state::MAX_PROJECTS,
        "a refused create must store nothing"
    );
    let absent = describe_projects(&router, Some(&["one-too-many"]), Some(&[CUSTOM_LABELS])).await;
    assert_eq!(
        absent.list("ProjectDescriptions").len(),
        0,
        "the project a refused create named must not exist, got {:?}",
        absent.body
    );
}
