// SPDX-License-Identifier: CC0-1.0

//! `ImportStacksToStackSet` over the awsQuery wire: stacks that ALREADY EXIST brought under a
//! StackSet.
//!
//! RULE: an import maps each stack it names onto ONE stack instance of the StackSet the
//! request names — "a reference to a stack in a target account within a Region", in the
//! account and Region the stack's own id names, pointing at that very stack — and performs ONE
//! StackSet operation, whose id the call answers with and which concludes on the clock. It
//! deploys nothing: the stacks already exist, so each one is left exactly as it was.
//!
//! The rows span the class the rule generalises over rather than one request: how many stacks
//! one import names (one, two, three), which Regions they live in (the administrator's own and
//! two others, which is what a stack id carrying its own Region makes reachable), how the
//! request names them (inline `StackIds`, an `s3://` object listing them one per line, an https
//! path-style object listing them comma-separated), whether the caller supplies the
//! `OperationId` or the service mints one, and whether the StackSet is addressed by name or by
//! id. The refusals are a table of their own: each way the pair of id members can be wrong,
//! each identifier that resolves to nothing, the documented ceiling on one import, a pair the
//! StackSet already holds an instance for, an operation already in flight and an operation id
//! already used.
//!
//! Every call goes through the real request path — form-encoded body in, XML envelope out, and
//! the Amazon S3 calls as a client makes them — so what is asserted is what a client sees.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, instance_status, operation_action, operation_status, permission_model,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    status, DRIFT_STATUS_NOT_CHECKED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, member, router_with_engine, wait_for_the_transition, xml_blocks, xml_text,
    REGION,
};
use crate::stack_lifecycle::{expected_status, ACCOUNT_ID};
use crate::stack_resource_provisioning::s3;

/// The StackSet every row imports into, held by the administrator account in [`REGION`].
const STACK_SET: &str = "imported-fleet";

/// A StackSet of the same account and Region whose permission model is service-managed: the
/// rows about organizational units are about a StackSet that HAS organization targets.
const SERVICE_MANAGED_STACK_SET: &str = "imported-fleet-service-managed";

/// A template declaring one resource: what each stack being imported was created with.
const PLAIN_TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// The bucket a `StackIdsUrl` row stages its list of ids in, as a client's own `PutObject`
/// stages it.
const STAGING_BUCKET: &str = "cfn-staging-stack-ids";

/// How many stacks ONE import may name.
///
/// Source: the operation's own documentation — "Use the stack import operation to import up to
/// 10 stacks into a new StackSet", and `StackIds`: "You import up to 10 stacks per StackSet at
/// a time" (`smithy_query(query="operation", operation="ImportStacksToStackSet")` carries no
/// `@length` for the list, so the service is what holds a request to it).
const MAX_IMPORTED_STACKS: usize = 10;

/// An `OrganizationalUnitId`, `@pattern("^(ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}|r-[a-z0-9]{4,32})$")`:
/// the OU a row maps its imported stacks to.
const ORGANIZATIONAL_UNIT: &str = "ou-abcd-12345678";

/// `CreateStackSet` with a template, answering the id the store minted for it.
async fn create_stack_set(
    router: &MultiServiceRouter,
    name: &str,
    extra: &[(String, String)],
) -> String {
    let mut members = vec![
        member("StackSetName", name),
        member("TemplateBody", PLAIN_TEMPLATE),
    ];
    members.extend_from_slice(extra);
    let (status_code, body) = call(router, "CreateStackSet", &members).await;
    assert_eq!(
        status_code, 200,
        "the StackSet must be created first, got {status_code} {body}"
    );
    xml_text(&body, "StackSetId")
        .unwrap_or_else(|| panic!("CreateStackSet must answer a StackSetId, got {body}"))
}

/// `CreateStack` in `region`, answering the stack id — the value a request names the stack to
/// import by.
async fn create_stack_in(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status_code, body) = call_in(
        router,
        region,
        "CreateStack",
        &[
            member("StackName", name),
            member("TemplateBody", PLAIN_TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "a stack to import must be created first, got {status_code} {body}"
    );
    xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("CreateStack must answer a StackId, got {body}"))
}

/// Create the staging bucket, once per router.
async fn create_staging_bucket(router: &MultiServiceRouter) {
    let created = s3(router, "PUT", &format!("/{STAGING_BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the staging bucket must be created first, got {} {}",
        created.status, created.body
    );
}

/// Stage one object listing stack ids, as a client's own `PutObject` stores it.
async fn stage(router: &MultiServiceRouter, key: &str, body: &str) {
    let stored = s3(
        router,
        "PUT",
        &format!("/{STAGING_BUCKET}/{key}"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the stack id list {key} must be staged first, got {} {}",
        stored.status, stored.body
    );
}

/// `ListStackInstances` for one StackSet.
async fn list_instances(router: &MultiServiceRouter, stack_set: &str) -> (u16, String) {
    call(
        router,
        "ListStackInstances",
        &[member("StackSetName", stack_set)],
    )
    .await
}

/// `DescribeStackInstance` for the instance of one account and Region pair.
async fn describe_instance(
    router: &MultiServiceRouter,
    stack_set: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackInstance",
        &[
            member("StackSetName", stack_set),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await
}

/// `DescribeStackSetOperation` for one operation of one StackSet.
async fn describe_operation(
    router: &MultiServiceRouter,
    stack_set: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", stack_set),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// `ImportStacksToStackSet` with whatever the row states.
async fn import(router: &MultiServiceRouter, members: &[(String, String)]) -> (u16, String) {
    call(router, "ImportStacksToStackSet", members).await
}

/// One stack instance as a response reports it.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    account: Option<String>,
    region: Option<String>,
    stack_id: Option<String>,
    status: Option<String>,
    detailed_status: Option<String>,
    drift_status: Option<String>,
    organizational_unit_id: Option<String>,
}

/// The instances one `ListStackInstances` response reports, in the order it reports them.
///
/// Each `<member>` of `<Summaries>` is read as a block and every member taken from INSIDE it,
/// so two summaries cannot be mixed up with each other.
fn reported_instances(body: &str) -> Vec<Reported> {
    xml_blocks(body, "member")
        .iter()
        .map(|block| Reported {
            account: xml_text(block, "Account"),
            region: xml_text(block, "Region"),
            stack_id: xml_text(block, "StackId"),
            status: xml_text(block, "Status"),
            detailed_status: xml_text(block, "DetailedStatus"),
            drift_status: xml_text(block, "DriftStatus"),
            organizational_unit_id: xml_text(block, "OrganizationalUnitId"),
        })
        .collect()
}

/// How a row names the stacks it imports.
#[derive(Clone, Copy, Debug)]
enum Naming {
    /// `StackIds.member.<n>`, as an SDK form-encodes the list.
    Inline,
    /// A `StackIdsUrl` object listing one id per line, addressed `s3://<bucket>/<key>`.
    ObjectPerLine,
    /// A `StackIdsUrl` object listing the ids comma-separated, addressed with the https
    /// path-style URL.
    ObjectCommaSeparated,
}

/// One row of the import table.
struct Row {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// The Regions the stacks this row imports live in — one stack per Region, since an account
    /// and Region pair carries at most one instance of a StackSet.
    stack_regions: &'static [&'static str],
    /// How the request names those stacks.
    naming: Naming,
    /// The `OperationId` the row supplies, where it supplies one.
    operation_id: Option<&'static str>,
    /// Whether the request addresses the StackSet by its unique id rather than by its name.
    by_id: bool,
    /// The OU the row maps the imported stacks to, where it maps them to one — which only a
    /// service-managed StackSet has.
    organizational_unit: Option<&'static str>,
}

/// The table: each row names its stacks differently and expects the same rule to hold.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one stack of the administrator's own Region, inline, no OperationId",
            stack_regions: &[REGION],
            naming: Naming::Inline,
            operation_id: None,
            by_id: false,
            organizational_unit: None,
        },
        Row {
            case: "three stacks, each in a different Region, with the caller's OperationId",
            stack_regions: &[REGION, "us-west-2", "eu-west-1"],
            naming: Naming::Inline,
            operation_id: Some("import-of-three"),
            by_id: false,
            organizational_unit: None,
        },
        Row {
            case: "two stacks addressed by the StackSet's unique id rather than its name",
            stack_regions: &[REGION, "ap-southeast-2"],
            naming: Naming::Inline,
            operation_id: None,
            by_id: true,
            organizational_unit: None,
        },
        Row {
            case: "two stacks listed one per line in an s3:// object",
            stack_regions: &[REGION, "us-west-2"],
            naming: Naming::ObjectPerLine,
            operation_id: Some("import-from-an-object"),
            by_id: false,
            organizational_unit: None,
        },
        Row {
            case: "three stacks listed comma-separated in an https path-style object",
            stack_regions: &[REGION, "eu-central-1", "us-west-1"],
            naming: Naming::ObjectCommaSeparated,
            operation_id: None,
            by_id: false,
            organizational_unit: None,
        },
        Row {
            case: "one stack mapped to an organizational unit of a service-managed StackSet",
            stack_regions: &["eu-west-2"],
            naming: Naming::Inline,
            operation_id: None,
            by_id: false,
            organizational_unit: Some(ORGANIZATIONAL_UNIT),
        },
    ]
}

/// Whether `operation_id` satisfies the modelled `ClientRequestToken`
/// (`@length(1..128)`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`) the output reports.
fn is_a_client_request_token(operation_id: &str) -> bool {
    (1..=128).contains(&operation_id.len())
        && operation_id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && operation_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

#[tokio::test]
async fn each_import_brings_its_stacks_under_the_stack_set() {
    for row in rows() {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let configuration = match row.organizational_unit {
            None => Vec::new(),
            // "Required if the permissions model is SERVICE_MANAGED": an OU is a target of the
            // organization a service-managed StackSet deploys through.
            Some(_) => vec![
                member("PermissionModel", permission_model::SERVICE_MANAGED),
                member("AutoDeployment.Enabled", "false"),
            ],
        };
        let stack_set_id = create_stack_set(&router, STACK_SET, &configuration).await;

        // The stacks the row imports, each already deployed in its own Region.
        let mut stacks: Vec<(&str, String)> = Vec::new();
        for (index, region) in row.stack_regions.iter().enumerate() {
            let name = format!("existing-{index}");
            stacks.push((region, create_stack_in(&router, region, &name).await));
        }
        // Every imported stack is a stack that finished being created, which is what a client
        // imports: the clock has to make those creates due before the import.
        wait_for_the_transition().await;

        let mut members = vec![member(
            "StackSetName",
            if row.by_id { &stack_set_id } else { STACK_SET },
        )];
        match row.naming {
            Naming::Inline => {
                for (index, (_, stack_id)) in stacks.iter().enumerate() {
                    members.push(member(&format!("StackIds.member.{}", index + 1), stack_id));
                }
            }
            Naming::ObjectPerLine => {
                create_staging_bucket(&router).await;
                let key = "lists/stacks.txt";
                let listed: Vec<&str> = stacks.iter().map(|(_, id)| id.as_str()).collect();
                stage(&router, key, &format!("{}\n", listed.join("\n"))).await;
                members.push(member("StackIdsUrl", &format!("s3://{STAGING_BUCKET}/{key}")));
            }
            Naming::ObjectCommaSeparated => {
                create_staging_bucket(&router).await;
                let key = "lists/stacks.csv";
                let listed: Vec<&str> = stacks.iter().map(|(_, id)| id.as_str()).collect();
                stage(&router, key, &listed.join(",")).await;
                members.push(member(
                    "StackIdsUrl",
                    &format!("https://s3.{REGION}.amazonaws.com/{STAGING_BUCKET}/{key}"),
                ));
            }
        }
        if let Some(operation_id) = row.operation_id {
            members.push(member("OperationId", operation_id));
        }
        if let Some(organizational_unit) = row.organizational_unit {
            members.push(member("OrganizationalUnitIds.member.1", organizational_unit));
        }

        let started = Instant::now();
        let (status_code, body) = import(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the import to succeed, got {status_code} {body}"
        );
        let operation_id = xml_text(&body, "OperationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {body}"));
        match row.operation_id {
            // "A unique, user defined, identifier for the StackSet operation": the operation is
            // the caller's own id where the caller named one.
            Some(supplied) => assert_eq!(
                operation_id, supplied,
                "case {case:?}: expected the supplied OperationId, got {body}"
            ),
            None => assert!(
                is_a_client_request_token(&operation_id),
                "case {case:?}: expected a minted OperationId of the modelled form, got \
                 {operation_id:?}"
            ),
        }

        // The operation this import performed, in flight when the call answered.
        let in_flight = expected_status(started, operation_status::RUNNING, operation_status::SUCCEEDED);
        let (status_code, body) = describe_operation(&router, STACK_SET, &operation_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the operation to be readable, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(in_flight),
            "case {case:?}: expected the operation {in_flight}, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::CREATE),
            "case {case:?}: the import creates the StackSet's instances, got {body}"
        );
        assert_eq!(
            xml_text(&body, "StackSetId").as_deref(),
            Some(stack_set_id.as_str()),
            "case {case:?}: the operation belongs to the StackSet it was asked of, got {body}"
        );

        // One instance per imported stack, in the account and Region the stack's own id names,
        // pointing at that stack.
        let (status_code, body) = list_instances(&router, STACK_SET).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the instances to be readable, got {status_code} {body}"
        );
        let expected: Vec<Reported> = stacks
            .iter()
            .map(|(region, stack_id)| Reported {
                account: Some(ACCOUNT_ID.to_string()),
                region: Some((*region).to_string()),
                stack_id: Some(stack_id.clone()),
                status: Some(
                    expected_status(started, instance_status::OUTDATED, instance_status::CURRENT)
                        .to_string(),
                ),
                detailed_status: Some(
                    expected_status(started, detailed_status::PENDING, detailed_status::SUCCEEDED)
                        .to_string(),
                ),
                // Nothing has been drift checked by an import.
                drift_status: Some(DRIFT_STATUS_NOT_CHECKED.to_string()),
                organizational_unit_id: row.organizational_unit.map(str::to_string),
            })
            .collect();
        assert_eq!(
            reported_instances(&body),
            expected,
            "case {case:?}: expected one instance per imported stack, got {body}"
        );

        // Each instance is addressable by its own account and Region pair, and reports the same
        // stack there.
        for (region, stack_id) in &stacks {
            let (status_code, body) =
                describe_instance(&router, STACK_SET, ACCOUNT_ID, region).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the instance in {region} to be readable, got \
                 {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {case:?}: the instance in {region} points at the imported stack, got {body}"
            );
        }

        // The import deployed nothing: every imported stack is the stack it was, in the Region
        // it was deployed in.
        for (region, stack_id) in &stacks {
            let (status_code, body) = call_in(
                &router,
                region,
                "DescribeStacks",
                &[member("StackName", stack_id)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: the imported stack must still exist, got {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {case:?}: the import leaves the stack as it was, got {body}"
            );
        }

        // The conclusion the clock owes: the operation succeeded and its instances are up to
        // date with the StackSet.
        wait_for_the_transition().await;
        let (status_code, body) = describe_operation(&router, STACK_SET, &operation_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the concluded operation to be readable, got {status_code} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(operation_status::SUCCEEDED),
            "case {case:?}: expected the operation to conclude, got {body}"
        );
        let (status_code, body) = list_instances(&router, STACK_SET).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the instances to be readable, got {status_code} {body}"
        );
        let settled: Vec<(Option<String>, Option<String>)> = reported_instances(&body)
            .into_iter()
            .map(|instance| (instance.status, instance.detailed_status))
            .collect();
        let expected_settled: Vec<(Option<String>, Option<String>)> = stacks
            .iter()
            .map(|_| {
                (
                    Some(instance_status::CURRENT.to_string()),
                    Some(detailed_status::SUCCEEDED.to_string()),
                )
            })
            .collect();
        assert_eq!(
            settled, expected_settled,
            "case {case:?}: a concluded import leaves its instances current, got {body}"
        );
    }
}

/// One row of the refusal table: what the request carries beyond nothing, and the answer it is
/// due.
struct Refusal {
    case: &'static str,
    members: Vec<(String, String)>,
    status_code: u16,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn each_refused_import_names_its_modelled_taxonomy_and_writes_nothing() {
    let (router, _engine) = router_with_engine();
    create_stack_set(&router, STACK_SET, &[]).await;
    // A second StackSet, service-managed, for the rows about organizational units: an OU is a
    // target of the organization a service-managed StackSet deploys through.
    create_stack_set(
        &router,
        SERVICE_MANAGED_STACK_SET,
        &[
            member("PermissionModel", permission_model::SERVICE_MANAGED),
            member("AutoDeployment.Enabled", "false"),
        ],
    )
    .await;
    let here = create_stack_in(&router, REGION, "existing-here").await;
    let also_here = create_stack_in(&router, REGION, "existing-beside-it").await;
    let elsewhere = create_stack_in(&router, "us-west-2", "existing-elsewhere").await;
    let doomed = create_stack_in(&router, REGION, "existing-no-more").await;
    wait_for_the_transition().await;
    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", "existing-no-more")]).await;
    assert_eq!(
        status_code, 200,
        "the stack whose deletion completed must be deleted first, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    create_staging_bucket(&router).await;
    stage(&router, "lists/empty.txt", "\n  \n\n").await;

    // A stack id of the form this store mints that names no stack at all.
    let ghost = format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/\
         00000000-0000-0000-0000-000000000000"
    );
    let too_many: Vec<(String, String)> = (1..=MAX_IMPORTED_STACKS + 1)
        .map(|index| {
            member(
                &format!("StackIds.member.{index}"),
                &format!(
                    "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/over-{index}/\
                     00000000-0000-0000-0000-00000000000{index}"
                ),
            )
        })
        .collect();

    let refusals: Vec<Refusal> = vec![
        Refusal {
            case: "a StackSetName this Region holds no StackSet for",
            members: vec![
                member("StackSetName", "ghost-set"),
                member("StackIds.member.1", &here),
            ],
            status_code: 404,
            code: "StackSetNotFoundException",
            message: "StackSet ghost-set not found",
        },
        Refusal {
            case: "a stack id no Region holds a stack for",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", &ghost),
            ],
            status_code: 404,
            code: "StackNotFoundException",
            message: "",
        },
        Refusal {
            case: "a stack whose deletion has completed",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", &doomed),
            ],
            status_code: 404,
            code: "StackNotFoundException",
            message: "",
        },
        Refusal {
            case: "both ways of naming the stacks",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", &here),
                member(
                    "StackIdsUrl",
                    &format!("s3://{STAGING_BUCKET}/lists/empty.txt"),
                ),
            ],
            status_code: 400,
            code: "ValidationError",
            message: "You cannot specify both StackIds and StackIdsUrl",
        },
        Refusal {
            case: "neither way of naming the stacks",
            members: vec![member("StackSetName", STACK_SET)],
            status_code: 400,
            code: "ValidationError",
            message: "Either StackIds or StackIdsUrl must be specified",
        },
        Refusal {
            case: "an object that lists no stack at all",
            members: vec![
                member("StackSetName", STACK_SET),
                member(
                    "StackIdsUrl",
                    &format!("s3://{STAGING_BUCKET}/lists/empty.txt"),
                ),
            ],
            status_code: 400,
            code: "ValidationError",
            message: "You must specify at least one value for StackIds",
        },
        Refusal {
            case: "a StackIdsUrl that addresses no object at all",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIdsUrl", &format!("s3://{STAGING_BUCKET}")),
            ],
            status_code: 400,
            code: "ValidationError",
            message: "StackIdsUrl must reference a valid S3 object to which you have access.",
        },
        Refusal {
            case: "a StackIdsUrl addressing an object Amazon S3 does not hold",
            members: vec![
                member("StackSetName", STACK_SET),
                member(
                    "StackIdsUrl",
                    &format!("s3://{STAGING_BUCKET}/lists/absent.txt"),
                ),
            ],
            status_code: 400,
            code: "ValidationError",
            message: "",
        },
        Refusal {
            case: "more stacks than one import may name",
            members: [vec![member("StackSetName", STACK_SET)], too_many].concat(),
            status_code: 400,
            code: "LimitExceededException",
            message: "Limit for imported stacks exceeded: no more than 10 stacks can be \
                      imported into a StackSet at a time",
        },
        Refusal {
            case: "two stacks of one account and Region, which is one instance's pair",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", &here),
                member("StackIds.member.2", &also_here),
            ],
            status_code: 400,
            code: "InvalidOperationException",
            message: "Stack instance in account 000000000000 and Region us-east-1 already \
                      exists",
        },
        Refusal {
            case: "organizational units against a self-managed StackSet",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", &here),
                member("OrganizationalUnitIds.member.1", ORGANIZATIONAL_UNIT),
            ],
            status_code: 400,
            code: "ValidationError",
            message: "StackSets with SELF_MANAGED permission model can only have accounts as \
                      target",
        },
        Refusal {
            // Which of several organizational units an imported account sits under is
            // Organizations membership this emulator holds none of, so the request is answered
            // as work nobody has written rather than over an OU nothing places it in.
            case: "more than one organizational unit to map the imported stacks to",
            members: vec![
                member("StackSetName", SERVICE_MANAGED_STACK_SET),
                member("StackIds.member.1", &here),
                member("OrganizationalUnitIds.member.1", ORGANIZATIONAL_UNIT),
                member("OrganizationalUnitIds.member.2", "ou-abcd-87654321"),
            ],
            status_code: 501,
            code: "NotImplemented",
            message: "",
        },
        Refusal {
            case: "a stack NAME where the member carries stack ids",
            members: vec![
                member("StackSetName", STACK_SET),
                member("StackIds.member.1", "existing-here"),
            ],
            status_code: 404,
            code: "StackNotFoundException",
            message: "Stack with id existing-here does not exist",
        },
    ];

    for refusal in &refusals {
        let case = refusal.case;
        let (status_code, body) = import(&router, &refusal.members).await;
        assert_eq!(
            status_code, refusal.status_code,
            "case {case:?}: expected {}, got {status_code} {body}",
            refusal.status_code
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(refusal.code),
            "case {case:?}: expected the {} refusal, got {body}",
            refusal.code
        );
        if !refusal.message.is_empty() {
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(refusal.message),
                "case {case:?}: expected the documented wording, got {body}"
            );
        }
    }

    // A refused import imports NOTHING: the StackSet holds no instance and performed no
    // operation.
    let (status_code, body) = list_instances(&router, STACK_SET).await;
    assert_eq!(
        status_code, 200,
        "expected the instances to be readable, got {status_code} {body}"
    );
    assert_eq!(
        reported_instances(&body),
        Vec::new(),
        "a refused import leaves the StackSet empty, got {body}"
    );

    // The one stack id that resolves really is importable, so every refusal above was about
    // what the row states and not about the fixture.
    let (status_code, body) = import(
        &router,
        &[
            member("StackSetName", STACK_SET),
            member("StackIds.member.1", &here),
            member("StackIds.member.2", &elsewhere),
            member("OperationId", "the-first-import"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "expected the import to succeed, got {status_code} {body}"
    );

    // "Only one operation can be performed for a stack set at a given time": a second import
    // while the first is in flight is refused, and imports nothing.
    let (status_code, body) = import(
        &router,
        &[
            member("StackSetName", STACK_SET),
            member("StackIds.member.1", &also_here),
        ],
    )
    .await;
    assert_eq!(
        status_code, 409,
        "expected an operation in progress, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("OperationInProgressException"),
        "expected the OperationInProgressException refusal, got {body}"
    );

    wait_for_the_transition().await;

    // "The specified operation ID already exists": the id of an operation this StackSet already
    // performed names that operation, so the request is not performed a second time.
    let (status_code, body) = import(
        &router,
        &[
            member("StackSetName", STACK_SET),
            member("StackIds.member.1", &also_here),
            member("OperationId", "the-first-import"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 409,
        "expected the operation id to be refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("OperationIdAlreadyExistsException"),
        "expected the OperationIdAlreadyExistsException refusal, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some("Operation with Id the-first-import already exists"),
        "expected the documented wording, got {body}"
    );

    // The pair the first import took is the pair a second stack of that account and Region would
    // land on, so importing it now is refused too.
    let (status_code, body) = import(
        &router,
        &[
            member("StackSetName", STACK_SET),
            member("StackIds.member.1", &also_here),
        ],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "expected the held pair to be refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidOperationException"),
        "expected the InvalidOperationException refusal, got {body}"
    );

    // What the first import wrote is still exactly what it wrote: two instances, one per stack.
    let (status_code, body) = list_instances(&router, STACK_SET).await;
    assert_eq!(
        status_code, 200,
        "expected the instances to be readable, got {status_code} {body}"
    );
    let imported: Vec<(Option<String>, Option<String>)> = reported_instances(&body)
        .into_iter()
        .map(|instance| (instance.region, instance.stack_id))
        .collect();
    assert_eq!(
        imported,
        vec![
            (Some(REGION.to_string()), Some(here.clone())),
            (Some("us-west-2".to_string()), Some(elsewhere.clone())),
        ],
        "expected the two imported stacks, got {body}"
    );
}
