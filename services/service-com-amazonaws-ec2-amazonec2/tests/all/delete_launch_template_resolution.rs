// SPDX-License-Identifier: CC0-1.0

//! Which launch template a `DeleteLaunchTemplate` resolves out of a store that holds
//! SEVERAL of them, driven over the wire the service speaks — `ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"), through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a launch template `CreateLaunchTemplate` accepted resolves — by
//! its `LaunchTemplateId` or by its `LaunchTemplateName`, exactly one of the two — until it
//! is deleted, and the delete answers HTTP 200 with that one template as the readers
//! reported it, removes it with all of its versions, and leaves every other template of the
//! account exactly as resolvable as it was.
//!
//! `tests/all/launch_templates.rs` already drives one template through a create and the
//! delete that follows it. What each row here varies is the store the delete resolves
//! AGAINST: how many other templates it holds, how their names sit beside the target's
//! (one extends it, it extends one, they differ only in ASCII case, they differ only in one
//! character of `^[a-zA-Z0-9\(\)\.\-/_]+$`), which of the two keys the delete names the
//! target by, and whether the target's versions were read back between the create and the
//! delete — a read may not change what the delete then resolves.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code};

/// The Region every call here is signed for. A launch template carries no Region member,
/// so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// Which of the two keys a request names the template by — the API takes "either the launch
/// template ID or the launch template name, but not both"
/// (`DeleteLaunchTemplateRequest$LaunchTemplateId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Id,
    Name,
}

/// One row of the one-key table: what the case is, the parameters the delete carries, and
/// the error code it must be refused with.
type RefusalCase = (&'static str, Vec<(String, String)>, &'static str);

/// One row of the resolution table.
struct Row {
    case: &'static str,
    /// The names to create, in order; every one of them is a template of the account when
    /// the delete runs.
    names: Vec<String>,
    /// Which of `names` the delete names.
    target: usize,
    /// Which key it names it by.
    delete_by: By,
    /// Whether the target's versions are read back before the delete.
    read_versions: bool,
}

/// The members of a reported `launchTemplate`, as the wire carries them: the identity and
/// the service-owned values a delete has to echo unchanged.
fn template_fields(xml: &str) -> Vec<(&'static str, Option<String>)> {
    [
        "launchTemplateId",
        "launchTemplateName",
        "createTime",
        "createdBy",
        "defaultVersionNumber",
        "latestVersionNumber",
    ]
    .into_iter()
    .map(|name| (name, element(xml, name)))
    .collect()
}

/// `"worker-00"`-style names, so one row can hold more siblings than it is worth writing out.
fn numbered_names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}-{index:02}")).collect()
}

/// The rows the table is written against: the sibling count, the shape of the sibling names
/// beside the target's, the key the delete uses, and whether a read came between.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a sibling's name extends the target's, deleted by name",
            names: vec![
                "probe-asg".to_string(),
                "probe-asg-lt".to_string(),
                "probe-asg-lt-2".to_string(),
            ],
            target: 0,
            delete_by: By::Name,
            read_versions: false,
        },
        Row {
            case: "the target's name extends a sibling's, deleted by name",
            names: vec!["probe-asg".to_string(), "probe-asg-lt".to_string()],
            target: 1,
            delete_by: By::Name,
            read_versions: true,
        },
        Row {
            case: "siblings differing from the target only in ASCII case, deleted by name",
            names: vec![
                "Teardown/LT".to_string(),
                "teardown/lt".to_string(),
                "TEARDOWN/LT".to_string(),
            ],
            target: 1,
            delete_by: By::Name,
            read_versions: false,
        },
        Row {
            case: "siblings differing only in one character the name pattern admits, \
                   deleted by name",
            names: vec![
                "cfg.a_b".to_string(),
                "cfg-a_b".to_string(),
                "cfg_a_b".to_string(),
                "cfg(a)b".to_string(),
            ],
            target: 2,
            delete_by: By::Name,
            read_versions: false,
        },
        Row {
            case: "eight siblings, the target deleted by id after its versions were read",
            names: numbered_names("worker", 8),
            target: 5,
            delete_by: By::Id,
            read_versions: true,
        },
        Row {
            case: "the only template of the account, deleted by id",
            names: vec!["lt3".to_string()],
            target: 0,
            delete_by: By::Id,
            read_versions: false,
        },
        Row {
            case: "the only template of the account, deleted by name",
            names: vec!["a-b_c.d".to_string()],
            target: 0,
            delete_by: By::Name,
            read_versions: true,
        },
    ]
}

/// Create one launch template under `name` and answer the id the service minted for it.
async fn create(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let params = vec![
        p("LaunchTemplateName", name.to_string()),
        p("LaunchTemplateData.ImageId", "ami-0abcdef1234567890"),
    ];
    let (status, body) = api(router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate for {name:?} must answer HTTP 200, \
         got {status} with body {body}"
    );
    element(&body, "launchTemplateId").unwrap_or_else(|| {
        panic!("case {case:?}: the create of {name:?} must report an id, got body {body}")
    })
}

/// The ids the store holds, in its own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .keys()
        .cloned()
        .collect()
}

/// `DescribeLaunchTemplates` narrowed to the one template `key` names.
async fn describe_one(
    router: &MultiServiceRouter,
    by: By,
    launch_template_id: &str,
    name: &str,
) -> (u16, String) {
    let params = match by {
        By::Id => vec![p("LaunchTemplateId.1", launch_template_id.to_string())],
        By::Name => vec![p("LaunchTemplateName.1", name.to_string())],
    };
    api(router, "DescribeLaunchTemplates", &params).await
}

/// A delete names exactly the template its one key names, out of a store that holds others
/// whose names sit next to it, and answers with that template as the readers reported it.
#[tokio::test]
async fn a_delete_resolves_exactly_the_template_its_key_names() {
    for row in rows() {
        let case = row.case;
        // A fresh emulator per row, so the ids and the count the row asserts on are the
        // ones its own creates made.
        let (router, engine) = super::ec2_query::emulator();

        let mut created: Vec<(String, String)> = Vec::new();
        for name in &row.names {
            let launch_template_id = create(&router, case, name).await;
            created.push((name.clone(), launch_template_id));
        }
        let (target_name, target_id) = created[row.target].clone();

        // What a reader reports for the target before the delete — the template the delete
        // then has to echo.
        let (status, reported) =
            describe_one(&router, row.delete_by, &target_id, &target_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeLaunchTemplates must report the target, \
             got {status} with body {reported}"
        );
        let before = template_fields(&reported);
        assert_eq!(
            before[0].1.as_deref(),
            Some(target_id.as_str()),
            "case {case:?}: the reader must report the target's own id, got body {reported}"
        );
        assert_eq!(
            before[1].1.as_deref(),
            Some(target_name.as_str()),
            "case {case:?}: the reader must report the target's own name, got body {reported}"
        );

        if row.read_versions {
            let (status, versions) = api(
                &router,
                "DescribeLaunchTemplateVersions",
                &[p("LaunchTemplateId", target_id.clone())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the target's versions must be readable before the delete, \
                 got {status} with body {versions}"
            );
            assert_eq!(
                element(&versions, "versionNumber").unwrap_or_default(),
                FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
                "case {case:?}: a created template holds its first version, got {versions}"
            );
        }

        let delete_params = match row.delete_by {
            By::Id => vec![p("LaunchTemplateId", target_id.clone())],
            By::Name => vec![p("LaunchTemplateName", target_name.clone())],
        };
        let (status, deleted) = api(&router, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting the template {target_name:?} that exists must answer \
             HTTP 200, got {status} with body {deleted}"
        );
        let after = template_fields(&deleted);
        assert_eq!(
            after, before,
            "case {case:?}: the delete must answer with the template as it was, \
             got body {deleted}"
        );

        // Exactly the one template is gone, with its versions; every sibling is untouched.
        let surviving = stored_ids(&engine);
        let expected: Vec<String> = {
            let mut ids: Vec<String> = created
                .iter()
                .filter(|(name, _)| name != &target_name)
                .map(|(_, id)| id.clone())
                .collect();
            ids.sort();
            ids
        };
        let mut surviving_sorted = surviving.clone();
        surviving_sorted.sort();
        assert_eq!(
            surviving_sorted, expected,
            "case {case:?}: the delete must remove exactly the template it named"
        );

        for (name, launch_template_id) in &created {
            if name == &target_name {
                continue;
            }
            for by in [By::Id, By::Name] {
                let (status, body) = describe_one(&router, by, launch_template_id, name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the sibling {name:?} must still resolve by {by:?} after \
                     the delete, got {status} with body {body}"
                );
                assert_eq!(
                    element(&body, "launchTemplateId").as_deref(),
                    Some(launch_template_id.as_str()),
                    "case {case:?}: resolving the sibling {name:?} by {by:?} must report its \
                     own id, got body {body}"
                );
            }
        }

        // Neither of the deleted template's keys resolves any more, and its versions went
        // with it.
        let (status, body) = api(
            &router,
            "DeleteLaunchTemplate",
            &[p("LaunchTemplateId", target_id.clone())],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLaunchTemplateId.NotFound"),
            "case {case:?}: the deleted id names nothing, got {status} with body {body}"
        );
        let (status, body) = api(
            &router,
            "DeleteLaunchTemplate",
            &[p("LaunchTemplateName", target_name.clone())],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLaunchTemplateName.NotFoundException"),
            "case {case:?}: the deleted name names nothing, got {status} with body {body}"
        );
        let (status, body) = api(
            &router,
            "DescribeLaunchTemplateVersions",
            &[p("LaunchTemplateId", target_id.clone())],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLaunchTemplateId.NotFound"),
            "case {case:?}: the deleted template's versions went with it, \
             got {status} with body {body}"
        );
    }
}

/// A delete names ONE key: a template that exists is still refused when the request names
/// both of its keys or neither, and the refusal leaves it stored and resolvable.
#[tokio::test]
async fn a_delete_naming_both_keys_or_neither_leaves_the_template_stored() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "both keys / neither key";
    let name = "probe-asg-lt";
    let launch_template_id = create(&router, case, name).await;

    let refusals: Vec<RefusalCase> = vec![
        (
            "both keys of the same template",
            vec![
                p("LaunchTemplateId", launch_template_id.clone()),
                p("LaunchTemplateName", name.to_string()),
            ],
            "InvalidParameterCombination",
        ),
        ("neither key", Vec::new(), "MissingParameter"),
    ];
    for (which, params, expected_code) in refusals {
        let (status, body) = api(&router, "DeleteLaunchTemplate", &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}/{which}: expected HTTP 400 {expected_code}, \
             got {status} with body {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            vec![launch_template_id.clone()],
            "case {case:?}/{which}: a refused delete removes nothing"
        );
    }

    // The template the refusals named is still there, and one key deletes it.
    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateName", name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the template is still deletable by its name, \
         got {status} with body {body}"
    );
    assert!(
        stored_ids(&engine).is_empty(),
        "case {case:?}: the delete must have removed the one template"
    );
}
