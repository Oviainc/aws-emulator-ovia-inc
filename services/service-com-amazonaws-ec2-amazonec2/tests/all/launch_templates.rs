// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EC2 launch template slice: `CreateLaunchTemplate` and
//! `DeleteLaunchTemplate`, driven over the wire the service actually speaks — `ec2Query`,
//! an `application/x-www-form-urlencoded` POST answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"), through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test is that a launch template is PERSISTED STATE: a create stores one
//! and answers with what it stored, and a delete removes exactly the template the request
//! names — with every version of it — and answers with that template as it was, so the id
//! and the name it carried resolve to nothing afterwards. Each table row is a distinct
//! input along what that generalises over: the name's length and characters, how many tags
//! the create carries, whether it carries a version description or an operator, which
//! launch parameters its data holds, and whether the delete names the template by id or by
//! name.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_LAUNCH_TEMPLATE_VERSION,
    LaunchTemplateRecord, MAX_LAUNCH_TEMPLATE_NAME_CHARS, MIN_LAUNCH_TEMPLATE_NAME_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};

/// The Region every call here is signed for. A launch template carries no Region member,
/// so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One row of the delete-resolution table: what the case is, the parameters the delete
/// carries, and the error code it must be refused with.
type DeleteCase = (&'static str, Vec<(String, String)>, &'static str);

/// The stored record for `launch_template_id`, as the store holds it.
fn stored(engine: &engine::InMemoryEngine, launch_template_id: &str) -> Option<LaunchTemplateRecord> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .get(launch_template_id)
        .cloned()
}

/// How many launch templates the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().launch_templates.len()
}

/// Whether `id` is the id form this service mints: `lt-` and 17 lowercase hex digits.
fn is_minted_id(id: &str) -> bool {
    id.strip_prefix("lt-").is_some_and(|suffix| {
        suffix.len() == 17 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Which of the two keys a delete names the template by — the API takes "either the launch
/// template ID or the launch template name, but not both".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteBy {
    Id,
    Name,
}

/// One row of the create/delete table: a template to create, and how to delete it.
struct Row {
    case: &'static str,
    name: String,
    version_description: Option<String>,
    /// `LaunchTemplateData.*` parameters: the launch parameters the version holds.
    data: Vec<(String, String)>,
    tags: Vec<(String, String)>,
    operator_principal: Option<String>,
    delete_by: DeleteBy,
}

/// The create parameters for one row.
fn create_params(row: &Row) -> Vec<(String, String)> {
    let mut params = vec![p("LaunchTemplateName", row.name.clone())];
    params.extend(row.data.iter().cloned());
    if let Some(description) = &row.version_description {
        params.push(p("VersionDescription", description.clone()));
    }
    if !row.tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", "launch-template"));
        for (index, (key, value)) in row.tags.iter().enumerate() {
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key.clone(),
            ));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value.clone(),
            ));
        }
    }
    if let Some(principal) = &row.operator_principal {
        params.push(p("Operator.Principal", principal.clone()));
    }
    params
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// The rows the create/delete table is written against. They differ along every attribute
/// the rule generalises over: the name's length and character classes, the number of tags,
/// whether a version description or an operator is given, how many and which launch
/// parameters the data carries, and which key the delete names the template by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "shortest name, one launch parameter, no tags, deleted by id",
            name: "abc".to_string(),
            version_description: None,
            data: vec![data("ImageId", "ami-0abcdef1234567890")],
            tags: Vec::new(),
            operator_principal: None,
            delete_by: DeleteBy::Id,
        },
        Row {
            case: "longest name, deleted by name",
            name: "n".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS),
            version_description: Some("the first version".to_string()),
            data: vec![data("InstanceType", "t3.micro")],
            tags: Vec::new(),
            operator_principal: None,
            delete_by: DeleteBy::Name,
        },
        Row {
            case: "every character class the name pattern admits, three tags",
            name: "Web(1).tier-a/b_C2".to_string(),
            version_description: None,
            data: vec![
                data("ImageId", "ami-00000000000000001"),
                data("InstanceType", "t3.small"),
                data("UserData", "IyEvYmluL2Jhc2gKZWNobyBoaQo="),
            ],
            tags: vec![
                ("Name".to_string(), "web".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
                ("Stage".to_string(), String::new()),
            ],
            operator_principal: None,
            delete_by: DeleteBy::Name,
        },
        Row {
            case: "lists and nested members in the data, one tag, deleted by id",
            name: "batch.workers".to_string(),
            version_description: Some(String::new()),
            data: vec![
                data("ImageId", "ami-00000000000000002"),
                data("SecurityGroupId.1", "sg-00000000000000001"),
                data("SecurityGroupId.2", "sg-00000000000000002"),
                data("Monitoring.Enabled", "true"),
                data("EbsOptimized", "true"),
            ],
            tags: vec![("Name".to_string(), "batch".to_string())],
            operator_principal: None,
            delete_by: DeleteBy::Id,
        },
        Row {
            case: "a managing service provider, longest version description",
            name: "managed_template".to_string(),
            version_description: Some("d".repeat(255)),
            data: vec![data("ImageId", "ami-00000000000000003")],
            tags: Vec::new(),
            operator_principal: Some("autoscaling.amazonaws.com".to_string()),
            delete_by: DeleteBy::Id,
        },
    ]
}

/// A create stores the template and reports what it stored; the delete that follows
/// removes it with its versions and reports the same template, after which neither its id
/// nor its name resolves.
#[tokio::test]
async fn a_created_launch_template_is_stored_reported_and_then_deleted() {
    let (router, engine) = super::ec2_query::emulator();
    let created_by = engine::emulator_arn("iam", "", "root");

    for row in rows() {
        let case = row.case;
        let (status, body) = api(&router, "CreateLaunchTemplate", &create_params(&row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateLaunchTemplate, got {status} with body {body}"
        );

        let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();
        assert!(
            is_minted_id(&launch_template_id),
            "case {case:?}: expected an 'lt-' id of 17 hex digits, got {launch_template_id:?}"
        );
        let reported_name = element(&body, "launchTemplateName").unwrap_or_default();
        assert_eq!(
            reported_name, row.name,
            "case {case:?}: expected the name the request gave, got {reported_name:?}"
        );
        assert_eq!(
            element(&body, "createdBy").unwrap_or_default(),
            created_by,
            "case {case:?}: expected the creating principal {created_by:?}, got body {body}"
        );
        assert_eq!(
            element(&body, "defaultVersionNumber").unwrap_or_default(),
            FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
            "case {case:?}: the only version is the default one, got body {body}"
        );
        assert_eq!(
            element(&body, "latestVersionNumber").unwrap_or_default(),
            FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
            "case {case:?}: the only version is the latest one, got body {body}"
        );
        let create_time = element(&body, "createTime").unwrap_or_default();
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                &create_time,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected an ISO 8601 createTime, got {create_time:?}"
        );
        let reported_tags: Vec<(String, String)> = elements(&body, "key")
            .into_iter()
            .zip(elements(&body, "value"))
            .collect();
        let expected_tags: Vec<(String, String)> = row.tags.clone();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the tags the request gave, got {reported_tags:?}"
        );
        if let Some(principal) = &row.operator_principal {
            assert_eq!(
                element(&body, "principal").as_deref(),
                Some(principal.as_str()),
                "case {case:?}: expected the managing principal in the operator, got body {body}"
            );
            assert_eq!(
                element(&body, "managed").unwrap_or_default(),
                "true",
                "case {case:?}: a template a provider manages is reported managed, got body {body}"
            );
        } else {
            assert!(
                element(&body, "principal").is_none(),
                "case {case:?}: no operator was asked for, so none may be reported: {body}"
            );
        }

        // The STORE, not the response, is what the create has to have changed: one record
        // holding exactly the one version, with the launch parameters the request carried.
        let record = stored(&engine, &launch_template_id)
            .unwrap_or_else(|| panic!("case {case:?}: the created template must be stored"));
        assert_eq!(
            record.versions.len(),
            1,
            "case {case:?}: a create stores exactly one version, got {:?}",
            record.versions.keys().collect::<Vec<_>>()
        );
        let version = record
            .versions
            .get(&FIRST_LAUNCH_TEMPLATE_VERSION)
            .unwrap_or_else(|| panic!("case {case:?}: version 1 must be stored"));
        assert_eq!(
            version.version_description, row.version_description,
            "case {case:?}: expected the version description the request gave, got {:?}",
            version.version_description
        );
        for (member, value) in &row.data {
            let expected = match member.as_str() {
                "LaunchTemplateData.ImageId" => version.data.image_id.clone(),
                "LaunchTemplateData.InstanceType" => version.data.instance_type.clone(),
                "LaunchTemplateData.UserData" => version.data.user_data.clone(),
                "LaunchTemplateData.EbsOptimized" => {
                    version.data.ebs_optimized.map(|set| set.to_string())
                }
                "LaunchTemplateData.Monitoring.Enabled" => version
                    .data
                    .monitoring
                    .as_ref()
                    .and_then(|monitoring| monitoring.enabled)
                    .map(|set| set.to_string()),
                "LaunchTemplateData.SecurityGroupId.1" => version
                    .data
                    .security_group_ids
                    .as_ref()
                    .and_then(|ids| ids.first().cloned()),
                "LaunchTemplateData.SecurityGroupId.2" => version
                    .data
                    .security_group_ids
                    .as_ref()
                    .and_then(|ids| ids.get(1).cloned()),
                other => panic!("case {case:?}: the table has no reader for {other}"),
            };
            assert_eq!(
                expected.as_deref(),
                Some(value.as_str()),
                "case {case:?}: expected the stored version to carry {member}={value}, \
                 got {expected:?}"
            );
        }

        // Deleting by either key removes that one template and answers with it as it was.
        let delete_params = match row.delete_by {
            DeleteBy::Id => vec![p("LaunchTemplateId", launch_template_id.clone())],
            DeleteBy::Name => vec![p("LaunchTemplateName", row.name.clone())],
        };
        let (status, deleted) = api(&router, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteLaunchTemplate, got {status} with body {deleted}"
        );
        assert_eq!(
            element(&deleted, "launchTemplateId").unwrap_or_default(),
            launch_template_id,
            "case {case:?}: the delete must report the template it removed, got {deleted}"
        );
        assert_eq!(
            element(&deleted, "launchTemplateName").unwrap_or_default(),
            row.name,
            "case {case:?}: the delete must report the template it removed, got {deleted}"
        );
        assert_eq!(
            element(&deleted, "createTime").unwrap_or_default(),
            create_time,
            "case {case:?}: the create time of a template never changes, got {deleted}"
        );

        assert!(
            stored(&engine, &launch_template_id).is_none(),
            "case {case:?}: the deleted template, and its versions, must be gone from the store"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: the delete must remove exactly the one template"
        );

        // Neither key resolves any more, and each is refused with the code for the key the
        // request used.
        let (status, again) = api(&router, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete names nothing, got {status} with body {again}"
        );
        let expected_code = match row.delete_by {
            DeleteBy::Id => "InvalidLaunchTemplateId.NotFound",
            DeleteBy::Name => "InvalidLaunchTemplateName.NotFoundException",
        };
        assert_eq!(
            error_code(&again),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {again}"
        );
    }
}

/// A name is accepted exactly when the modelled `LaunchTemplateName` can carry it: 3 to
/// 128 characters of `^[a-zA-Z0-9\(\)\.\-/_]+$`. The rows are both published length bounds,
/// one past each, every character class the pattern admits, and characters it does not.
#[tokio::test]
async fn a_name_is_accepted_exactly_when_the_modelled_name_can_carry_it() {
    let (router, engine) = super::ec2_query::emulator();

    let cases: Vec<(&str, String, bool)> = vec![
        (
            "the published minimum length",
            "a".repeat(MIN_LAUNCH_TEMPLATE_NAME_CHARS),
            true,
        ),
        (
            "one character below the minimum",
            "a".repeat(MIN_LAUNCH_TEMPLATE_NAME_CHARS - 1),
            false,
        ),
        (
            "the published maximum length",
            "b".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS),
            true,
        ),
        (
            "one character above the maximum",
            "c".repeat(MAX_LAUNCH_TEMPLATE_NAME_CHARS + 1),
            false,
        ),
        ("digits only", "123".to_string(), true),
        ("mixed case letters", "AbCdEf".to_string(), true),
        (
            "every punctuation the pattern admits",
            "x()./-_y".to_string(),
            true,
        ),
        ("a space", "a b".to_string(), false),
        ("an asterisk", "a*b".to_string(), false),
        ("a plus sign", "a+b".to_string(), false),
        ("an at sign", "a@b".to_string(), false),
        ("a colon", "a:b".to_string(), false),
        ("a backslash", "a\\b".to_string(), false),
        ("a non-ASCII letter", "naïve".to_string(), false),
        ("empty", String::new(), false),
    ];

    let mut accepted = 0;
    for (case, name, is_accepted) in cases {
        let params = vec![
            p("LaunchTemplateName", name.clone()),
            data("ImageId", "ami-0abcdef1234567890"),
        ];
        let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
        if is_accepted {
            accepted += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for name {name:?}, got {status} with body {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for name {name:?}, got {status} with body {body}"
            );
        }
        assert_eq!(
            stored_count(&engine),
            accepted,
            "case {case:?}: only an accepted name may store a template"
        );
    }
}

/// A create that carries no launch parameter at all is refused: "you must specify at least
/// one parameter for the launch template data".
#[tokio::test]
async fn a_create_with_no_launch_parameter_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[p("LaunchTemplateName", "empty-data")],
    )
    .await;
    assert_eq!(
        status, 400,
        "empty launch template data: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "empty launch template data: nothing may be stored, got body {body}"
    );
}

/// A name names one template: a second create under a name the account already holds is
/// refused, and the name becomes free again once the template is deleted.
#[tokio::test]
async fn a_name_names_one_template_until_it_is_deleted() {
    let (router, engine) = super::ec2_query::emulator();

    let first = vec![
        p("LaunchTemplateName", "shared-name"),
        data("ImageId", "ami-00000000000000001"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &first).await;
    assert_eq!(status, 200, "first create: got {status} with body {body}");
    let first_id = element(&body, "launchTemplateId").unwrap_or_default();

    // A different request under the same name — different data, so this is not an
    // idempotent repeat.
    let second = vec![
        p("LaunchTemplateName", "shared-name"),
        data("InstanceType", "t3.large"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &second).await;
    assert_eq!(
        status, 400,
        "second create under one name: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateName.AlreadyExistsException",
        "second create under one name: got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a refused create stores nothing: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateName", "shared-name")],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");

    let (status, body) = api(&router, "CreateLaunchTemplate", &second).await;
    assert_eq!(
        status, 200,
        "create after the delete: got {status} with body {body}"
    );
    let third_id = element(&body, "launchTemplateId").unwrap_or_default();
    assert_ne!(
        third_id, first_id,
        "a reused name gets a new template, not the deleted one's id"
    );
}

/// A delete names the template by id or by name, never both and never neither, and an id
/// is checked for shape before it is looked up.
#[tokio::test]
async fn a_delete_resolves_one_key_and_refuses_the_rest() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = api(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "resolver"),
            data("ImageId", "ami-00000000000000001"),
        ],
    )
    .await;
    assert_eq!(status, 200, "setup create: got {status} with body {body}");
    let known_id = element(&body, "launchTemplateId").unwrap_or_default();

    // Rows: neither key, both keys, a malformed id (not hex, no prefix, the wrong length),
    // a well-formed id of each published id length that names nothing, and an unknown name.
    let cases: Vec<DeleteCase> = vec![
        (
            "neither key",
            Vec::new(),
            "MissingParameter",
        ),
        (
            "both keys",
            vec![
                p("LaunchTemplateId", known_id.clone()),
                p("LaunchTemplateName", "resolver"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "an id that is not hexadecimal",
            vec![p("LaunchTemplateId", "lt-zzzzzzzzzzzzzzzzz")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id without the resource prefix",
            vec![p("LaunchTemplateId", "00000000000000001")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id of a length no EC2 id has",
            vec![p("LaunchTemplateId", "lt-0123456")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "an id of another resource kind",
            vec![p("LaunchTemplateId", "sg-00000000000000001")],
            "InvalidLaunchTemplateId.Malformed",
        ),
        (
            "a well-formed long id nothing was created for",
            vec![p("LaunchTemplateId", "lt-0fedcba987654321f")],
            "InvalidLaunchTemplateId.NotFound",
        ),
        (
            "a well-formed short id nothing was created for",
            vec![p("LaunchTemplateId", "lt-0fedcba9")],
            "InvalidLaunchTemplateId.NotFound",
        ),
        (
            "a name nothing was created under",
            vec![p("LaunchTemplateName", "no-such-template")],
            "InvalidLaunchTemplateName.NotFoundException",
        ),
    ];

    for (case, params, expected_code) in cases {
        let (status, body) = api(&router, "DeleteLaunchTemplate", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        assert!(
            stored(&engine, &known_id).is_some(),
            "case {case:?}: a refused delete must leave the stored template alone"
        );
    }
}

/// `DryRun` reports the outcome a permitted caller would have got — `DryRunOperation`,
/// HTTP 412 — and changes nothing, on both the create and the delete.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_writing() {
    let (router, engine) = super::ec2_query::emulator();

    let create = vec![
        p("LaunchTemplateName", "dry-run"),
        data("ImageId", "ami-00000000000000001"),
    ];
    let mut dry_create = create.clone();
    dry_create.push(p("DryRun", "true"));
    let (status, body) = api(&router, "CreateLaunchTemplate", &dry_create).await;
    assert_eq!(
        status, 412,
        "dry-run create: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry-run create: got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "dry-run create: nothing may be stored"
    );

    let (status, body) = api(&router, "CreateLaunchTemplate", &create).await;
    assert_eq!(status, 200, "create: got {status} with body {body}");
    let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[
            p("LaunchTemplateId", launch_template_id.clone()),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "dry-run delete: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry-run delete: got body {body}"
    );
    assert!(
        stored(&engine, &launch_template_id).is_some(),
        "dry-run delete: the template must still be stored"
    );

    // A dry run that would have been REFUSED is refused with that reason instead.
    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[
            p("LaunchTemplateId", "lt-0fedcba987654321f"),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "dry-run delete of an unknown template: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateId.NotFound",
        "dry-run delete of an unknown template: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", launch_template_id.clone())],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");
    assert_eq!(
        stored_count(&engine),
        0,
        "delete: the template must be gone"
    );
}

/// A `ClientToken` makes a repeated create answer with the one template it created, and a
/// token whose template was deleted creates a new one rather than naming the deleted id.
#[tokio::test]
async fn a_client_token_makes_a_repeated_create_answer_with_the_one_template() {
    let (router, engine) = super::ec2_query::emulator();

    let params = vec![
        p("LaunchTemplateName", "idempotent"),
        data("ImageId", "ami-00000000000000001"),
        p("ClientToken", "token-0001"),
    ];
    let (status, first) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(status, 200, "first create: got {status} with body {first}");
    let first_id = element(&first, "launchTemplateId").unwrap_or_default();

    let (status, repeat) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(status, 200, "repeat: got {status} with body {repeat}");
    assert_eq!(
        element(&repeat, "launchTemplateId").unwrap_or_default(),
        first_id,
        "a repeat under one token answers with the template it created, got {repeat}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a repeat under one token creates no second template"
    );

    // A different token under the same name is a different request, and the name is taken.
    let mut other_token = params.clone();
    other_token.retain(|(key, _)| key != "ClientToken");
    other_token.push(p("ClientToken", "token-0002"));
    let (status, body) = api(&router, "CreateLaunchTemplate", &other_token).await;
    assert_eq!(
        status, 400,
        "another token, same name: got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateName.AlreadyExistsException",
        "another token, same name: got body {body}"
    );

    let (status, body) = api(
        &router,
        "DeleteLaunchTemplate",
        &[p("LaunchTemplateId", first_id.clone())],
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");

    // The token no longer names anything, so the same request creates a new template.
    let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "create after the delete: got {status} with body {body}"
    );
    let new_id = element(&body, "launchTemplateId").unwrap_or_default();
    assert_ne!(
        new_id, first_id,
        "a token whose template was deleted must not answer with the deleted id"
    );
    assert!(
        stored(&engine, &new_id).is_some(),
        "create after the delete: the new template must be stored"
    );
}

/// A tag specification must name the one resource type this create tags —
/// `launch-template` — and the tags it carries land on the template.
#[tokio::test]
async fn a_tag_specification_must_name_the_launch_template_resource_type() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the resource types `RunInstances` tags, the type of another resource of this
    // service, a type that is not a resource type at all, and no type at all.
    let refused = ["instance", "volume", "network-interface", "vpc", "", "fleet"];
    for (index, resource_type) in refused.iter().enumerate() {
        let mut params = vec![
            p("LaunchTemplateName", format!("tagged-{index}")),
            data("ImageId", "ami-00000000000000001"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "web"),
        ];
        if !resource_type.is_empty() {
            params.push(p("TagSpecification.1.ResourceType", *resource_type));
        }
        let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {resource_type:?}: got body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {resource_type:?}: a refused create stores nothing"
        );
    }

    let params = vec![
        p("LaunchTemplateName", "tagged"),
        data("ImageId", "ami-00000000000000001"),
        p("TagSpecification.1.ResourceType", "launch-template"),
        p("TagSpecification.1.Tag.1.Key", "Name"),
        p("TagSpecification.1.Tag.1.Value", "web"),
    ];
    let (status, body) = api(&router, "CreateLaunchTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case \"launch-template\": expected HTTP 200, got {status} with body {body}"
    );
    let launch_template_id = element(&body, "launchTemplateId").unwrap_or_default();
    let record = stored(&engine, &launch_template_id)
        .expect("the tagged template must be stored");
    let tags = record.template.tags.clone().unwrap_or_default();
    assert_eq!(
        tags.len(),
        1,
        "case \"launch-template\": expected the one tag the request gave, got {tags:?}"
    );
    assert_eq!(
        (tags[0].key.as_deref(), tags[0].value.as_deref()),
        (Some("Name"), Some("web")),
        "case \"launch-template\": expected Name=web on the stored template, got {tags:?}"
    );
}
