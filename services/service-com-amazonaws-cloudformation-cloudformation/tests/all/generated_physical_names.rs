// SPDX-License-Identifier: CC0-1.0

//! The physical name CloudFormation GENERATES for a resource a template does not name.
//!
//! RULE: the `<stack name>-<logical id>-<unique id>` physical name CloudFormation
//! generates for a resource whose name the template leaves unspecified is FITTED to the
//! naming constraint the service that owns the resource type publishes — its maximum
//! length and its permitted character set — by shortening the stack-name and logical-id
//! segments while the unique segment is kept whole, so the owning service accepts the name
//! and the stack that declared the resource reaches `CREATE_COMPLETE` instead of failing
//! on that service's refusal and rolling back.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the BOUND — one row per owning service's published limit (64, 63, 128, 255, 256, 512,
//!   and a type with no published bound at all), each limit taken from that service's own
//!   Smithy model where it carries one and from its API reference where it does not (the
//!   source is named per row);
//! * the SIZE of what is fitted — a stack name of ordinary length, the issue's own
//!   37-character one, and one of `STACK_NAME_MAX_LEN`, crossed with a short and a
//!   60-character logical id, so each type is exercised both inside its bound and well past
//!   what an unfitted name would be;
//! * the CHARACTER SET — the types whose owning service admits only lowercase names
//!   (Amazon S3, Amazon ECR, Amazon RDS) beside the ones whose patterns admit a generated
//!   name as it is built.
//!
//! The issue's own case — an unnamed `AWS::IAM::Role` under a 37-character stack name,
//! whose generated 69-character `roleName` IAM refused — is one row of the table, and the
//! role is read back through IAM's own `GetRole` on the same engine, so what the test
//! asserts is that the owning service really holds the resource under the fitted name.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{status, STACK_NAME_MAX_LEN};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, iam, member, router, wait_for_the_transition, xml_text,
    xml_texts,
};

/// How many characters the unique segment of a generated name carries
/// (`<stack name>-<logical id>-<unique id>`, 12 hexadecimal characters).
const UNIQUE_SEGMENT_CHARACTERS: usize = 12;

/// The two hyphens that join the three segments of a generated name.
const SEPARATORS: usize = 2;

/// One row of the naming table: a resource type a template can leave unnamed, with the
/// constraint the service that OWNS it publishes for the name it is created under.
struct Owned {
    /// The CloudFormation resource type.
    resource_type: &'static str,
    /// The longest name the owning service accepts, and where that bound comes from — see
    /// the comment on each row. `None` for a type whose owning service publishes no bound
    /// a generated name could break.
    limit: Option<usize>,
    /// Whether that service's names admit no upper case.
    lowercase: bool,
}

/// The types this table spans, each with its owning service's published name constraint.
///
/// Sources, per row: the `@length` on the member the resource is created under in that
/// service's OWN Smithy model (`smithy_query(query="operation")` on it), or that service's
/// API reference where its model carries none.
const OWNED: &[Owned] = &[
    // IAM `CreateRole.RoleName`: `roleNameType`, `@length(min: 1, max: 64)` — the issue's
    // own row, and the bound its 69-character generated name broke.
    Owned {
        resource_type: "AWS::IAM::Role",
        limit: Some(64),
        lowercase: false,
    },
    // IAM `CreateUser.UserName`: `userNameType`, `@length(min: 1, max: 64)`.
    Owned {
        resource_type: "AWS::IAM::User",
        limit: Some(64),
        lowercase: false,
    },
    // IAM `CreateGroup.GroupName`: `groupNameType`, `@length(min: 1, max: 128)`.
    Owned {
        resource_type: "AWS::IAM::Group",
        limit: Some(128),
        lowercase: false,
    },
    // Amazon S3: "Bucket names must be between 3 (min) and 63 (max) characters long" and
    // "can consist only of lowercase letters, numbers, dots (.), and hyphens (-)" (bucket
    // naming rules).
    Owned {
        resource_type: "AWS::S3::Bucket",
        limit: Some(63),
        lowercase: true,
    },
    // Amazon ECR `CreateRepository.repositoryName`: `RepositoryName`,
    // `@length(min: 2, max: 256)`, and a pattern that admits no upper case.
    Owned {
        resource_type: "AWS::ECR::Repository",
        limit: Some(256),
        lowercase: true,
    },
    // Amazon DynamoDB `CreateTable.TableName`: `TableName`, `@length(min: 3, max: 255)`.
    Owned {
        resource_type: "AWS::DynamoDB::Table",
        limit: Some(255),
        lowercase: false,
    },
    // Amazon CloudWatch Logs `CreateLogGroup.logGroupName`: `LogGroupName`,
    // `@length(min: 1, max: 512)`.
    Owned {
        resource_type: "AWS::Logs::LogGroup",
        limit: Some(512),
        lowercase: false,
    },
    // AWS Lambda `CreateFunction.FunctionName`: the modelled 140 covers the full ARN — "if
    // you specify only the function name, it is limited to 64 characters in length".
    Owned {
        resource_type: "AWS::Lambda::Function",
        limit: Some(64),
        lowercase: false,
    },
    // Amazon CloudWatch `PutMetricAlarm.AlarmName`: `AlarmName`,
    // `@length(min: 1, max: 255)`.
    Owned {
        resource_type: "AWS::CloudWatch::Alarm",
        limit: Some(255),
        lowercase: false,
    },
    // Amazon RDS `DBInstanceIdentifier`: "must contain from 1 to 63 letters, numbers, or
    // hyphens", "can't end with a hyphen or contain two consecutive hyphens", and is
    // "stored as all lowercase" (Amazon RDS API reference).
    Owned {
        resource_type: "AWS::RDS::DBInstance",
        limit: Some(63),
        lowercase: true,
    },
    // A type with NO bound of its own: an `AWS::IAM::Policy` takes its policy name from the
    // required `PolicyName` property, so its physical id is CloudFormation's own generated
    // id and no owning service's name constraint applies to it — nothing is fitted, and the
    // full `<stack name>-<logical id>-<unique id>` is what the stack reports.
    Owned {
        resource_type: "AWS::IAM::Policy",
        limit: None,
        lowercase: false,
    },
];

/// The properties the declaration of `resource_type` carries.
///
/// Only the two types this emulator really CREATES in their owning service need any, and
/// each needs exactly what that service's create requires of a caller: an
/// `AWS::IAM::Role`'s `AssumeRolePolicyDocument` is required on the type and on IAM's own
/// `CreateRole` (the policy here is the issue's own — a role the AWS Config recorder
/// assumes), and an `AWS::RDS::DBInstance` is created through `CreateDBInstance`, whose
/// `Engine` and `DBInstanceClass` are required members. The other types here are recorded
/// by CloudFormation rather than created in their owning service, and the naming rule reads
/// no property of theirs — a declaration that names the resource nowhere is exactly the case
/// under test.
fn properties(resource_type: &str) -> Value {
    if resource_type == "AWS::IAM::Role" {
        return json!({
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "config.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }
        });
    }
    if resource_type == "AWS::RDS::DBInstance" {
        return json!({"Engine": "mysql", "DBInstanceClass": "db.t3.micro"});
    }
    // An `AWS::Lambda::Function` is created through `CreateFunction`, whose `Role` and
    // `Code` are required members (and whose `Runtime` and `Handler` are required of a .zip
    // package): the role is the one the template declares for it ([`EXEC_ROLE_LOGICAL_ID`]),
    // the one role here whose trust policy lets Lambda assume it.
    if resource_type == "AWS::Lambda::Function" {
        return json!({
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Role": {"Fn::GetAtt": [EXEC_ROLE_LOGICAL_ID, "Arn"]},
            "Code": {"ZipFile": "def handler(event, context):\n    return {}\n"}
        });
    }
    json!({})
}

/// The logical id of the execution role the functions below run as. It is NOT one of the
/// table's rows: nothing is asserted about its own generated name, and it is here because
/// the functions cannot be created without a role Lambda can assume.
const EXEC_ROLE_LOGICAL_ID: &str = "FunctionExecRole";

/// One declared resource of the table: its logical id, and the row it came from.
struct Declared<'a> {
    logical_id: String,
    owned: &'a Owned,
}

/// The resources one stack declares: every row of [`OWNED`], once under a SHORT logical id
/// and once under a 60-character one, so the fitting is exercised against both.
fn declarations() -> Vec<Declared<'static>> {
    let mut declared = Vec::new();
    for (index, owned) in OWNED.iter().enumerate() {
        declared.push(Declared {
            logical_id: format!("R{index}"),
            owned,
        });
        // A logical id is alphanumeric; this one is 60 characters, longer than several of
        // the bounds above on its own.
        let padding = "Resource".repeat(8);
        declared.push(Declared {
            logical_id: format!("R{index}Long{}", &padding[..60 - format!("R{index}Long").len()]),
            owned,
        });
    }
    declared
}

/// The template declaring every resource of [`declarations`], none of them named.
fn template(declared: &[Declared<'_>]) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        EXEC_ROLE_LOGICAL_ID.to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "lambda.amazonaws.com"},
                        "Action": "sts:AssumeRole"
                    }]
                }
            },
        }),
    );
    for entry in declared {
        resources.insert(
            entry.logical_id.clone(),
            json!({
                "Type": entry.owned.resource_type,
                "Properties": properties(entry.owned.resource_type),
            }),
        );
    }
    json!({"AWSTemplateFormatVersion": "2010-09-09", "Resources": resources}).to_string()
}

/// The `(logical id, physical id)` pairs the stack reports.
///
/// Read as the two lists `DescribeStackResources` carries, in document order: every
/// resource of every template here has a physical id, so the pairing cannot slip.
async fn reported_names(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackResources({stack:?}) must answer 200, got {code}: {body}"
    );
    let logical = xml_texts(&body, "LogicalResourceId");
    let physical = xml_texts(&body, "PhysicalResourceId");
    assert_eq!(
        logical.len(),
        physical.len(),
        "every reported resource must carry a physical id, got {logical:?} against \
         {physical:?}"
    );
    logical.into_iter().zip(physical).collect()
}

/// `(the stack-name segment, the logical-id segment, the unique segment)` of a generated
/// name.
///
/// The unique segment is the LAST, and a logical id carries no hyphen, so the two hyphens
/// that matter are the last two — whatever hyphens the stack name itself carries.
fn segments(name: &str) -> Option<(&str, &str, &str)> {
    let (head, unique) = name.rsplit_once('-')?;
    let (stack, logical) = head.rsplit_once('-')?;
    Some((stack, logical, unique))
}

/// Whether every character of `text` is a hexadecimal digit of the case `lowercase` asks
/// for.
fn is_hex(text: &str, lowercase: bool) -> bool {
    !text.is_empty()
        && text.chars().all(|character| {
            character.is_ascii_hexdigit()
                && if lowercase {
                    !character.is_ascii_uppercase()
                } else {
                    !character.is_ascii_lowercase()
                }
        })
}

/// The whole rule, per row of the table, for one stack name.
async fn names_are_fitted_to_the_owning_service(case: &str, stack_name: &str) {
    let router = router();
    let declared = declarations();
    let (code, created) = create_stack(
        &router,
        stack_name,
        &template(&declared),
        // A template declaring IAM resources is acknowledged the way a client does it.
        &[member("Capabilities.member.1", "CAPABILITY_IAM")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code}: {created}"
    );
    wait_for_the_transition().await;

    // The issue's observable: the stack COMPLETES. Before the fix the IAM role's generated
    // name was past IAM's 64 characters, IAM refused it, and the stack rolled back.
    let (code, described) = describe_stack(&router, stack_name).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code}: {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {:?} ({:?})",
        xml_text(&described, "StackStatus"),
        xml_text(&described, "StackStatusReason")
    );

    let reported = reported_names(&router, stack_name).await;
    for entry in &declared {
        let row = format!(
            "{case}/{}/{}",
            entry.owned.resource_type, entry.logical_id
        );
        let name = reported
            .iter()
            .find(|(logical, _)| logical == &entry.logical_id)
            .map(|(_, physical)| physical.clone())
            .unwrap_or_else(|| panic!("case {row:?}: the stack must report this resource"));
        let characters = name.chars().count();
        // What the name WOULD be unfitted: the stack name, the logical id, the unique
        // segment and the two hyphens that join them.
        let unfitted = stack_name.chars().count()
            + entry.logical_id.chars().count()
            + UNIQUE_SEGMENT_CHARACTERS
            + SEPARATORS;

        if let Some(limit) = entry.owned.limit {
            assert!(
                characters <= limit,
                "case {row:?}: expected a name within the {limit} characters its owning \
                 service accepts, got {characters} ({name:?})"
            );
        }
        // The room is USED: nothing is given up that the bound did not require, beyond the
        // at most two hyphens the character rules give up — a segment the shortening cut
        // ends on an alphanumeric rather than beside the hyphen that joins the segments.
        let expected = entry
            .owned
            .limit
            .map_or(unfitted, |limit| unfitted.min(limit));
        let floor = expected - SEPARATORS;
        assert!(
            (floor..=expected).contains(&characters),
            "case {row:?}: expected a name of {floor}..={expected} characters (unfitted \
             {unfitted}), got {characters} ({name:?})"
        );

        // The FORM: three segments, the unique one whole, the other two prefixes of the
        // stack name and the logical id they were shortened from.
        let (stack_segment, logical_segment, unique) = segments(&name)
            .unwrap_or_else(|| panic!("case {row:?}: {name:?} must carry three segments"));
        assert!(
            is_hex(unique, entry.owned.lowercase)
                && unique.chars().count() == UNIQUE_SEGMENT_CHARACTERS,
            "case {row:?}: expected a {UNIQUE_SEGMENT_CHARACTERS}-character hexadecimal \
             unique segment, got {unique:?} in {name:?}"
        );
        let expected_stack = if entry.owned.lowercase {
            stack_name.to_ascii_lowercase()
        } else {
            stack_name.to_string()
        };
        let expected_logical = if entry.owned.lowercase {
            entry.logical_id.to_ascii_lowercase()
        } else {
            entry.logical_id.clone()
        };
        assert!(
            expected_stack.starts_with(stack_segment.trim_end_matches('-')),
            "case {row:?}: expected the first segment to be a prefix of the stack name, got \
             {stack_segment:?} in {name:?}"
        );
        assert!(
            !logical_segment.is_empty() && expected_logical.starts_with(logical_segment),
            "case {row:?}: expected the second segment to be a prefix of the logical id, \
             got {logical_segment:?} in {name:?}"
        );
        // Nothing is fitted away where the whole name is within the bound.
        if entry.owned.limit.is_none_or(|limit| unfitted <= limit) {
            assert_eq!(
                name,
                format!("{expected_stack}-{expected_logical}-{unique}"),
                "case {row:?}: a name inside its bound must be the whole \
                 <stack name>-<logical id>-<unique id>"
            );
        }

        // The CHARACTER SET the owning service admits.
        if entry.owned.lowercase {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    && name.starts_with(|c: char| c.is_ascii_alphanumeric())
                    && name.ends_with(|c: char| c.is_ascii_alphanumeric())
                    && !name.contains("--"),
                "case {row:?}: expected a lowercase name beginning and ending with an \
                 alphanumeric and carrying no repeated separator, got {name:?}"
            );
        } else {
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && name.starts_with(|c: char| c.is_ascii_alphanumeric())
                    && name.ends_with(|c: char| c.is_ascii_alphanumeric())
                    && !name.contains("--"),
                "case {row:?}: expected a name of alphanumerics and single hyphens, beginning \
                 and ending with an alphanumeric, got {name:?}"
            );
        }
    }

    // Two resources of one stack never collide, however much the fitting shortened them —
    // which is why the unique segment is what survives it.
    let mut generated: Vec<&String> = reported.iter().map(|(_, physical)| physical).collect();
    let before = generated.len();
    generated.sort();
    generated.dedup();
    assert_eq!(
        generated.len(),
        before,
        "case {case:?}: every generated name in one stack must be distinct, got {generated:?}"
    );

    // A READ does not change them: the names a second DescribeStackResources reports are
    // the ones the first did.
    assert_eq!(
        reported_names(&router, stack_name).await,
        reported,
        "case {case:?}: the physical names must be stable across reads"
    );

    // And the resource the owning service really creates is there UNDER the fitted name:
    // the roles of this stack, read back through IAM's own GetRole.
    for entry in declared.iter().filter(|entry| entry.owned.resource_type == "AWS::IAM::Role") {
        let role_name = reported
            .iter()
            .find(|(logical, _)| logical == &entry.logical_id)
            .map(|(_, physical)| physical.clone())
            .expect("the stack reports every resource");
        let (code, body) = iam(&router, "GetRole", &[("RoleName", &role_name)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}/{}: IAM must hold the role {role_name:?} the stack reports, got \
             {code}: {body}",
            entry.logical_id
        );
        assert_eq!(
            xml_text(&body, "RoleName").as_deref(),
            Some(role_name.as_str()),
            "case {case:?}/{}: GetRole must answer for {role_name:?}, got {body}",
            entry.logical_id
        );
    }
}

#[tokio::test]
async fn a_generated_name_is_fitted_to_the_owning_services_naming_rules() {
    // A stack name of ordinary length: nothing here is past any of the bounds for the short
    // logical ids, and past the smallest of them for the 60-character ones.
    names_are_fitted_to_the_owning_service("an ordinary stack name", "cfg-stack").await;
    // The issue's own stack name, 37 characters: with the issue's logical id this is the
    // 69-character roleName IAM refused.
    names_are_fitted_to_the_owning_service(
        "the issue's stack name",
        "sim2real-behav-cfg-stack-eed8edb87ce1",
    )
    .await;
    // The LONGEST stack name CloudFormation accepts ("A stack name can contain only
    // alphanumeric characters (case-sensitive) and hyphens … and can't be longer than 128
    // characters"), which is past every bound in the table.
    let longest = {
        let head = "platform-production-shared-services-delivery-infrastructure-stack-";
        let mut name = head.to_string();
        name.push_str(&"a".repeat(STACK_NAME_MAX_LEN - head.len()));
        name
    };
    assert_eq!(
        longest.chars().count(),
        STACK_NAME_MAX_LEN,
        "the longest-stack-name row must use a name of exactly {STACK_NAME_MAX_LEN} characters"
    );
    names_are_fitted_to_the_owning_service("the longest stack name", &longest).await;
}

#[tokio::test]
async fn a_name_the_template_declares_is_not_fitted() {
    // The other half of the rule: a name the TEMPLATE supplied is the caller's, so it is
    // used exactly as written even where the owning service would refuse it — the refusal
    // is that service's to answer, and silently rewriting it would create a different
    // resource than the one the template asked for. The rows differ in the declared name's
    // case and length; the type is one this emulator records rather than provisions, so
    // what is read back is the name CloudFormation kept.
    let router = router();
    let past_the_bound = format!("declared-table-{}", "x".repeat(300));
    let cases: &[(&str, &str, &str)] = &[
        ("a short declared table name", "Declared", "declared-config-table"),
        (
            // Upper case, which the generated-name rule lowercases for a type whose owning
            // service admits none: a DECLARED name keeps its case either way.
            "a declared table name carrying upper case",
            "DeclaredMixed",
            "Declared-Config-Table",
        ),
        (
            // Past Amazon DynamoDB's 255 characters (`TableName`, `@length(max: 255)`):
            // still the caller's name.
            "a declared table name past the bound",
            "DeclaredLong",
            &past_the_bound,
        ),
    ];
    let mut resources = serde_json::Map::new();
    for (_, logical_id, declared) in cases {
        resources.insert(
            (*logical_id).to_string(),
            json!({"Type": "AWS::DynamoDB::Table", "Properties": {"TableName": declared}}),
        );
    }
    let template = json!({"Resources": resources}).to_string();
    let (code, created) = create_stack(&router, "declared-names", &template, &[]).await;
    assert_eq!(
        code, 200,
        "CreateStack must answer 200, got {code}: {created}"
    );
    wait_for_the_transition().await;

    let reported = reported_names(&router, "declared-names").await;
    for (case, logical_id, declared) in cases {
        let name = reported
            .iter()
            .find(|(logical, _)| logical == logical_id)
            .map(|(_, physical)| physical.clone())
            .unwrap_or_else(|| panic!("case {case:?}: the stack must report this resource"));
        assert_eq!(
            name, *declared,
            "case {case:?}: expected the declared name {declared:?}, got {name:?}"
        );
    }
}
