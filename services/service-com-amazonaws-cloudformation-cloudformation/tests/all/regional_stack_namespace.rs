// SPDX-License-Identifier: CC0-1.0

//! One stack namespace PER REGION, over the awsQuery wire.
//!
//! RULE: a stack name identifies a stack only within the account AND the Region the
//! request is signed for — `CreateStackInput$StackName`: "the name must be unique in the
//! Region in which you are creating the stack". So the same name in a second Region is a
//! second stack, both coexist, the modelled `AlreadyExistsException` is only for a name
//! already live in the SAME Region, and every read — `DescribeStacks` by name, by stack
//! id, and with no name at all, `ListStacks`, `GetTemplate` — answers from the Region its
//! request was signed for and from no other.
//!
//! The rows differ along what the rule generalises over: which PAIR of Regions is used
//! (including a pair in which neither is the emulator's default, so nothing about the rule
//! hangs on `us-east-1`), the spelling of the template (JSON and YAML), and the shape of
//! the stack name. The recorded case — a redeploy of one template under one name into the
//! Region it should have gone to, while the misdeployed copy still exists — is one row.
//!
//! Each row also pins that the two stacks are really Regional and not two copies of one
//! record: the stack ids carry their own Regions, and the `AWS::Region` pseudo parameter
//! each one's output resolves is its own Region.
//!
//! One case stands apart: the stack QUOTA is metered in the same per-Region namespace, and
//! the Region a request is metered in is the caller's own credential scope — so the store
//! also carries a ceiling across every Region, which is what bounds it however many Region
//! strings a caller signs for.

use crate::stack_lifecycle::{
    call_in, member, router_with_engine, store, wait_for_the_transition, xml_text, xml_texts,
    ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    status, StoredStack, MAX_LIVE_STACKS, MAX_STACKS,
};
use smithy_server_runtime::MultiServiceRouter;

/// One row: a pair of Regions, the stack name both deployments use, and the template.
struct RegionPair<'a> {
    case: &'a str,
    first: &'a str,
    second: &'a str,
    stack_name: &'a str,
    template: &'a str,
}

/// The `OutputValue` reported for `key`, read as the two document-ordered lists a
/// `DescribeStacks` answer carries. Sound here because every output of every template
/// below resolves.
fn output_value(xml: &str, key: &str) -> String {
    let keys = xml_texts(xml, "OutputKey");
    let values = xml_texts(xml, "OutputValue");
    keys.iter()
        .zip(values.iter())
        .find(|(output_key, _)| output_key.as_str() == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

async fn create_stack_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    template: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateStack",
        &[member("StackName", name), member("TemplateBody", template)],
    )
    .await
}

async fn describe_stack_in(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeStacks",
        &[member("StackName", name_or_id)],
    )
    .await
}

#[tokio::test]
async fn the_same_stack_name_in_a_second_region_is_a_second_stack() {
    let rows: &[RegionPair<'_>] = &[
        RegionPair {
            // The recorded shape: the stack went to the wrong Region, so the same
            // template under the same name is deployed to the one it belongs in while the
            // misdeployed copy is still there.
            case: "the default Region and another, in YAML",
            first: REGION,
            second: "us-west-2",
            stack_name: "redeployed-stack",
            template: concat!(
                "Resources:\n",
                "  Table:\n",
                "    Type: AWS::DynamoDB::Table\n",
                "    Properties:\n",
                "      TableName: redeployed-table\n",
                "      BillingMode: PAY_PER_REQUEST\n",
                "      KeySchema:\n",
                "        - AttributeName: pk\n",
                "          KeyType: HASH\n",
                "      AttributeDefinitions:\n",
                "        - AttributeName: pk\n",
                "          AttributeType: S\n",
                "Outputs:\n",
                "  DeployedIn:\n",
                "    Value: !Ref AWS::Region\n",
            ),
        },
        RegionPair {
            // Neither Region is the emulator's default: the rule is about the Region the
            // request is signed for, not about one privileged Region.
            case: "two Regions, neither the default, in JSON",
            first: "eu-central-1",
            second: "ap-southeast-2",
            stack_name: "a-very-long-stack-name-for-one-template-everywhere",
            // The log group's name carries the Region it is deployed in: the group is really
            // created in Amazon CloudWatch Logs now
            // (`state::provisioning::logs_resources`), and this row's subject is the stack
            // NAME being per-Region rather than the resource name being reusable.
            template: r#"{"Resources":{"Group":{"Type":"AWS::Logs::LogGroup",
                 "Properties":{"LogGroupName":{"Fn::Sub":"everywhere-group-${AWS::Region}"}}}},
                 "Outputs":{"DeployedIn":{"Value":{"Ref":"AWS::Region"}}}}"#,
        },
        RegionPair {
            case: "a one-character stack name across two Regions",
            first: "us-west-1",
            second: "eu-west-3",
            stack_name: "a",
            template: concat!(
                "Resources:\n",
                "  Table:\n",
                "    Type: AWS::DynamoDB::Table\n",
                "    Properties:\n",
                "      BillingMode: PAY_PER_REQUEST\n",
                "      KeySchema:\n",
                "        - AttributeName: pk\n",
                "          KeyType: HASH\n",
                "      AttributeDefinitions:\n",
                "        - AttributeName: pk\n",
                "          AttributeType: S\n",
                "Outputs:\n",
                "  DeployedIn:\n",
                "    Value: !Ref AWS::Region\n",
            ),
        },
    ];

    for row in rows {
        let router = crate::stack_lifecycle::router();

        let (status, first_body) =
            create_stack_in(&router, row.first, row.stack_name, row.template).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first CreateStack must answer 200, got {status} {first_body}",
            row.case
        );
        let first_id = xml_text(&first_body, "StackId").unwrap_or_default();
        wait_for_the_transition().await;

        // The whole of the rule: the SAME name and template in a second Region is a second
        // stack, not an AlreadyExists.
        let (status, second_body) =
            create_stack_in(&router, row.second, row.stack_name, row.template).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack in {} must answer 200 while the stack of that name \
             exists only in {}, got {status} {second_body}",
            row.case, row.second, row.first
        );
        let second_id = xml_text(&second_body, "StackId").unwrap_or_default();
        assert_ne!(
            first_id, second_id,
            "case {:?}: the two Regions' stacks must be two stacks, got one id {first_id:?}",
            row.case
        );
        wait_for_the_transition().await;

        // Each Region's stack is the one its own reads resolve, with its own id, and both
        // are complete.
        for (region, expected_id) in [(row.first, &first_id), (row.second, &second_id)] {
            let (status, described) = describe_stack_in(&router, region, row.stack_name).await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeStacks in {region} must answer 200, got {status} \
                 {described}",
                row.case
            );
            assert_eq!(
                xml_text(&described, "StackId").as_deref(),
                Some(expected_id.as_str()),
                "case {:?}: the stack {region} reports must be the one created there, got \
                 {described}",
                row.case
            );
            assert_eq!(
                xml_text(&described, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {:?}: the stack in {region} must reach CREATE_COMPLETE, got \
                 {described}",
                row.case
            );
            // The stack really is that Region's: its id and its AWS::Region output say so.
            let prefix =
                format!("arn:aws:cloudformation:{region}:{ACCOUNT_ID}:stack/{}/", row.stack_name);
            assert!(
                expected_id.starts_with(&prefix),
                "case {:?}: expected a stack id under {prefix:?}, got {expected_id:?}",
                row.case
            );
            assert_eq!(
                output_value(&described, "DeployedIn"),
                region,
                "case {:?}: AWS::Region must resolve to {region}, got {described}",
                row.case
            );
            // And it is the only stack that Region holds.
            let (_, listed) = call_in(&router, region, "DescribeStacks", &[]).await;
            assert_eq!(
                xml_texts(&listed, "StackId"),
                vec![expected_id.clone()],
                "case {:?}: DescribeStacks in {region} must report only that Region's \
                 stack, got {listed}",
                row.case
            );
            let (_, summaries) = call_in(&router, region, "ListStacks", &[]).await;
            assert_eq!(
                xml_texts(&summaries, "StackId"),
                vec![expected_id.clone()],
                "case {:?}: ListStacks in {region} must report only that Region's stack, \
                 got {summaries}",
                row.case
            );
        }

        // A stack id is an ARN carrying its own Region, so it names nothing in the other
        // one: "If the stack doesn't exist, a ValidationError is returned."
        let (status, refused) = describe_stack_in(&router, row.first, &second_id).await;
        assert_eq!(
            status, 400,
            "case {:?}: the other Region's stack id must name nothing in {}, got {status} \
             {refused}",
            row.case, row.first
        );
        let (status, refused) = call_in(
            &router,
            row.first,
            "GetTemplate",
            &[member("StackName", &second_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: GetTemplate in {} must not read the other Region's stack, got \
             {status} {refused}",
            row.case, row.first
        );

        // The per-Region namespace is still a namespace: the name IS taken in the Region
        // that holds it.
        let (status, clash) =
            create_stack_in(&router, row.second, row.stack_name, row.template).await;
        assert_eq!(
            status, 400,
            "case {:?}: a second create in {} must be refused, got {status} {clash}",
            row.case, row.second
        );
        assert_eq!(
            xml_text(&clash, "Code").as_deref(),
            Some("AlreadyExistsException"),
            "case {:?}: expected AlreadyExistsException for the name already live in {}, \
             got {clash}",
            row.case, row.second
        );

        // Retiring the misdeployed copy leaves the other Region's stack exactly as it was.
        let (status, deleted) = call_in(
            &router,
            row.first,
            "DeleteStack",
            &[member("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteStack in {} must answer 200, got {status} {deleted}",
            row.case, row.first
        );
        wait_for_the_transition().await;
        let (status, gone) = describe_stack_in(&router, row.first, row.stack_name).await;
        assert_eq!(
            status, 400,
            "case {:?}: the deleted stack must be unaddressable by name in {}, got \
             {status} {gone}",
            row.case, row.first
        );
        let (status, survivor) = describe_stack_in(&router, row.second, row.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack in {} must survive the other Region's deletion, got \
             {status} {survivor}",
            row.case, row.second
        );
        assert_eq!(
            xml_text(&survivor, "StackId").as_deref(),
            Some(second_id.as_str()),
            "case {:?}: the surviving stack must still be the one created in {}, got \
             {survivor}",
            row.case, row.second
        );
    }
}

/// Seed `count` live stacks of `region` straight into the store, which is how a case
/// reaches a quota no reasonable number of wire calls would.
fn given_stacks(engine: &std::sync::Arc<engine::InMemoryEngine>, region: &str, count: usize) {
    let now = std::time::SystemTime::now();
    let state = store(engine);
    let mut held = state.lock();
    let first = held.stacks.len();
    for index in first..first + count {
        let name = format!("filler-{index:06}");
        let mut stack = StoredStack::new(
            format!("arn:aws:cloudformation:{region}:{ACCOUNT_ID}:stack/{name}/{index:06}"),
            name,
            Some("{}".to_string()),
            now,
        );
        stack.region = region.to_string();
        held.stacks.push(stack);
    }
}

/// The stack quota is metered PER REGION — a Region at its quota refuses while another
/// still accepts — and the store as a whole is capped across every Region, because the
/// Region a request is metered in is the caller's own credential scope.
#[tokio::test]
async fn the_stack_quota_is_metered_per_region_and_capped_across_regions() {
    let (router, engine) = router_with_engine();
    let template = r#"{"Resources":{"Table":{"Type":"AWS::DynamoDB::Table"}}}"#;

    // One Region at its documented quota.
    given_stacks(&engine, REGION, MAX_STACKS);
    let (status, refused) = call_in(
        &router,
        REGION,
        "CreateStack",
        &[
            member("StackName", "one-too-many"),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"the Region is at its quota\": the create is refused, got {status} {refused}"
    );
    assert_eq!(
        xml_text(&refused, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"the Region is at its quota\": expected LimitExceededException, got {refused}"
    );

    // Another Region is not: the quota is per Region.
    let (status, accepted) = call_in(
        &router,
        "eu-west-1",
        "CreateStack",
        &[
            member("StackName", "one-too-many"),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another Region is below its own quota\": the create is accepted, got {status} \
         {accepted}"
    );

    // ...up to the ceiling on the store as a whole, which is what bounds the product of the
    // per-Region quota and the Regions a caller can sign for. Seeded one per distinct Region
    // string, as a caller choosing its own credential scope would.
    let held = store(&engine).lock().live_stack_count();
    for index in 0..MAX_LIVE_STACKS.saturating_sub(held) {
        given_stacks(&engine, &format!("ap-southeast-{index}"), 1);
    }
    let (status, refused) = call_in(
        &router,
        "us-east-2",
        "CreateStack",
        &[
            member("StackName", "past-the-ceiling"),
            member("TemplateBody", template),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"the store is at its ceiling\": a create in a Region of its own is still \
         refused, got {status} {refused}"
    );
    assert_eq!(
        xml_text(&refused, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"the store is at its ceiling\": expected LimitExceededException, got {refused}"
    );
    assert_eq!(
        store(&engine).lock().live_stack_count(),
        MAX_LIVE_STACKS,
        "case \"the store is at its ceiling\": the refused create stored nothing"
    );
}
