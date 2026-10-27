// SPDX-License-Identifier: CC0-1.0

//! A stack operation that re-declares a resource the stack ALREADY has, with a changed
//! configuration, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **a deployment that re-declares an
//! existing resource with a different configuration applies that change through the owning
//! service's own update API — the resource keeps its identity and its handle, the owning
//! service really holds the new configuration, and the stack operation reaches its COMPLETED
//! status; a change this emulator cannot apply fails the operation naming the resource rather
//! than completing while the caller's change went nowhere.**
//!
//! `AWS::SNS::Topic` is the type the rows are driven through: a topic's configuration is its
//! ATTRIBUTES plus its tags, each with one Amazon SNS call that writes it on a live topic, so
//! the change reaches SNS rather than replacing the topic. The rows differ along what the
//! rule generalises over — a property added, a property re-valued, several at once, a value
//! written as a string and as an object, a tag added and a tag dropped, a property the
//! revision stops declaring (which goes back to that attribute's documented absent value),
//! the change this emulator cannot apply, and both operations that deploy a revision
//! (`UpdateStack` and `ExecuteChangeSet`) — and every expected value is computed from what
//! the row declares, never read back out of the answer being checked.
//!
//! The recorded traces this suite was written for are two rows of it: a change set created
//! over a revised one-topic template, executed, and polled for the stack's completed status;
//! and the no-op revision whose change set settles `FAILED` and is refused execution.

use crate::harness::xml_blocks;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, sns, wait_for_the_transition, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, failed_action, resource_failure_summary, status, NO_CHANGES_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

/// A one-topic template whose topic declares `properties` beside its name.
fn topic_template(topic_name: &str, properties: &str) -> String {
    format!(
        r#"{{"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{topic_name}"{properties}}}}}}}}}"#
    )
}

/// The `StackStatus` a stack reports right now.
async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(code, 200, "DescribeStacks {stack:?}: body {body}");
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` a stack reports right now.
async fn reason_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = describe_stack(router, stack).await;
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// The attributes Amazon SNS holds for the topic `arn` names.
async fn topic_attributes(case: &str, router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (code, body) = sns(router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetTopicAttributes to answer for {arn:?}, got {code} {body}"
    );
    xml_blocks(&body, "entry")
        .iter()
        .filter_map(|entry| {
            Some((
                xml_text(entry, "key")?,
                xml_text(entry, "value").unwrap_or_default(),
            ))
        })
        .collect()
}

/// The tags Amazon SNS holds for the topic `arn` names.
async fn topic_tags(case: &str, router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (code, body) = sns(router, "ListTagsForResource", &[("ResourceArn", arn)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected ListTagsForResource to answer for {arn:?}, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter_map(|entry| {
            Some((
                xml_text(entry, "Key")?,
                xml_text(entry, "Value").unwrap_or_default(),
            ))
        })
        .collect()
}

/// The ARN of the topic the stack published for its `Topic` resource — the handle a client
/// reaches it by, which an in-place update must not change.
async fn topic_arn_of(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[member("StackName", stack), member("LogicalResourceId", "Topic")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResource to answer, got {code} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// One row of the in-place update table.
struct UpdateCase {
    case: &'static str,
    stack: &'static str,
    topic: &'static str,
    /// The properties the CREATE declares, beside the topic's name.
    created: &'static str,
    /// The properties the revision declares, beside the topic's name.
    revised: &'static str,
    /// The topic attributes Amazon SNS must hold once the revision is deployed.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// The tags Amazon SNS must hold once the revision is deployed.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// The class the rule covers: a revision that states a different configuration for a
/// resource the stack already holds.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a property the create did not declare at all",
            stack: "inplace-added",
            topic: "inplace-added-topic",
            created: "",
            revised: r#","DisplayName":"added""#,
            expected_attributes: &[("DisplayName", "added")],
            expected_tags: &[],
        },
        UpdateCase {
            case: "a property the create declared, re-valued",
            stack: "inplace-revalued",
            topic: "inplace-revalued-topic",
            created: r#","DisplayName":"before""#,
            revised: r#","DisplayName":"after""#,
            expected_attributes: &[("DisplayName", "after")],
            expected_tags: &[],
        },
        UpdateCase {
            case: "several attributes at once, one of them a closed value set",
            stack: "inplace-several",
            topic: "inplace-several-topic",
            created: r#","DisplayName":"before","TracingConfig":"PassThrough""#,
            revised: r#","DisplayName":"after","TracingConfig":"Active","SignatureVersion":"2""#,
            expected_attributes: &[
                ("DisplayName", "after"),
                ("TracingConfig", "Active"),
                ("SignatureVersion", "2"),
            ],
            expected_tags: &[],
        },
        UpdateCase {
            case: "an attribute whose declared value is an OBJECT, not a string",
            stack: "inplace-object",
            topic: "inplace-object-topic",
            created: r#","DisplayName":"policy-holder""#,
            revised: r#","DisplayName":"policy-holder","DeliveryPolicy":{"http":{"defaultHealthyRetryPolicy":{"numRetries":3}}}"#,
            expected_attributes: &[
                ("DisplayName", "policy-holder"),
                (
                    "DeliveryPolicy",
                    r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":3}}}"#,
                ),
            ],
            expected_tags: &[],
        },
        UpdateCase {
            case: "a tag added, a tag re-valued and a tag dropped",
            stack: "inplace-tags",
            topic: "inplace-tags-topic",
            created: r#","Tags":[{"Key":"kept","Value":"one"},{"Key":"dropped","Value":"two"}]"#,
            revised: r#","Tags":[{"Key":"kept","Value":"changed"},{"Key":"added","Value":"three"}]"#,
            expected_attributes: &[],
            expected_tags: &[("added", "three"), ("kept", "changed")],
        },
    ]
}

#[tokio::test]
async fn an_update_applies_a_changed_declaration_in_the_owning_service() {
    for case in update_cases() {
        let UpdateCase {
            case,
            stack,
            topic,
            created,
            revised,
            expected_attributes,
            expected_tags,
        } = case;
        let router = router();
        let (code, body) = create_stack(&router, stack, &topic_template(topic, created), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {body}"
        );
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?}"
        );
        let arn = topic_arn_of(case, &router, stack).await;

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &topic_template(topic, revised)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
        );
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete, got {reported:?} (reason {:?})",
            reason_of(&router, stack).await
        );

        // The resource kept its identity: the handle the stack publishes is the one it
        // published before the update, so nothing was replaced.
        let after = topic_arn_of(case, &router, stack).await;
        assert_eq!(
            after, arn,
            "case {case:?}: expected the topic to keep its ARN across an in-place update, \
             got {after:?}"
        );

        // ...and Amazon SNS really holds the declared configuration.
        let held = topic_attributes(case, &router, &arn).await;
        for (name, value) in expected_attributes {
            let found = held
                .iter()
                .find(|(held_name, _)| held_name == name)
                .map(|(_, held_value)| held_value.clone());
            assert_eq!(
                found.as_deref(),
                Some(*value),
                "case {case:?}: expected Amazon SNS to hold {name}={value:?}, got {found:?} \
                 (attributes {held:?})"
            );
        }
        if !expected_tags.is_empty() {
            let tags = topic_tags(case, &router, &arn).await;
            // The tags CloudFormation PROPAGATES to the resource are on the topic too; what
            // this row is about is the declaration's own, so the declared set is checked
            // pair by pair and a key the revision dropped must be gone.
            for (key, value) in expected_tags {
                let found = tags
                    .iter()
                    .find(|(held_key, _)| held_key == key)
                    .map(|(_, held_value)| held_value.clone());
                assert_eq!(
                    found.as_deref(),
                    Some(*value),
                    "case {case:?}: expected Amazon SNS to hold the tag {key}={value:?}, got \
                     {found:?} (tags {tags:?})"
                );
            }
            assert!(
                !tags.iter().any(|(key, _)| key == "dropped"),
                "case {case:?}: expected the tag the revision no longer declares to be \
                 removed, got {tags:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_dropped_property_goes_back_to_its_documented_absent_value() {
    // A property the revision no longer declares: the topic goes back to the value Amazon
    // SNS documents as that attribute's absent state, through the same call that writes it,
    // so the topic is configured as the revision declares it rather than keeping a property
    // the caller removed.
    let router = router();
    let template = topic_template("inplace-dropped-topic", r#","DisplayName":"before""#);
    let (code, body) = create_stack(&router, "inplace-dropped", &template, &[]).await;
    assert_eq!(code, 200, "expected CreateStack to succeed, got {code} {body}");
    wait_for_the_transition().await;
    let arn = topic_arn_of("dropped", &router, "inplace-dropped").await;

    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "inplace-dropped"),
            member("TemplateBody", &topic_template("inplace-dropped-topic", "")),
        ],
    )
    .await;
    assert_eq!(code, 200, "expected UpdateStack to succeed, got {code} {body}");
    wait_for_the_transition().await;
    let reported = status_of(&router, "inplace-dropped").await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "expected the update that drops a property to complete, got {reported:?} (reason {:?})",
        reason_of(&router, "inplace-dropped").await
    );
    let held = topic_attributes("dropped", &router, &arn).await;
    let display_name = held
        .iter()
        .find(|(name, _)| name == "DisplayName")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    assert_eq!(
        display_name, "",
        "expected the dropped display name to go back to its absent value, got \
         {display_name:?} (attributes {held:?})"
    );
}

#[tokio::test]
async fn a_change_this_emulator_cannot_apply_fails_the_update_naming_the_resource() {
    // The inline `Subscription` list is not a property of the topic: each entry is a
    // subscription of Amazon SNS's own, addressed by an ARN this resource's record does not
    // carry, so a revision that changes the list fails the operation naming the resource
    // rather than completing while the caller's change went nowhere.
    let router = router();
    let subscribed = r#","Subscription":[{"Protocol":"email","Endpoint":"one@example.com"}]"#;
    let template = topic_template("inplace-subscribed-topic", subscribed);
    let (code, body) = create_stack(&router, "inplace-subscribed", &template, &[]).await;
    assert_eq!(code, 200, "expected CreateStack to succeed, got {code} {body}");
    wait_for_the_transition().await;

    let revised = r#","Subscription":[{"Protocol":"email","Endpoint":"two@example.com"}]"#;
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "inplace-subscribed"),
            member(
                "TemplateBody",
                &topic_template("inplace-subscribed-topic", revised),
            ),
        ],
    )
    .await;
    assert_eq!(code, 200, "expected UpdateStack to succeed, got {code} {body}");
    wait_for_the_transition().await;
    let reported = status_of(&router, "inplace-subscribed").await;
    assert_eq!(
        reported,
        status::UPDATE_FAILED,
        "expected an update this emulator cannot apply to fail, got {reported:?}"
    );
    let reason = reason_of(&router, "inplace-subscribed").await;
    let expected = resource_failure_summary(failed_action::UPDATE, ["Topic"]);
    assert_eq!(
        reason, expected,
        "expected the failure to name the resource it could not update, got {reason:?}"
    );
}

#[tokio::test]
async fn a_change_set_executed_over_a_revised_topic_completes_and_a_no_op_set_is_refused() {
    let router = router();
    let template = topic_template("changeset-topic", "");
    let (code, body) = create_stack(&router, "changeset-inplace", &template, &[]).await;
    assert_eq!(code, 200, "expected CreateStack to succeed, got {code} {body}");
    wait_for_the_transition().await;

    let revised = topic_template("changeset-topic", r#","DisplayName":"by-change-set""#);
    let (code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "changeset-inplace"),
            member("ChangeSetName", "revision"),
            member("ChangeSetType", "UPDATE"),
            member("TemplateBody", &revised),
        ],
    )
    .await;
    assert_eq!(code, 200, "expected CreateChangeSet to succeed, got {code} {body}");
    let change_set_id = xml_text(&body, "Id").unwrap_or_default();
    wait_for_the_transition().await;
    let (_, described) = call(
        &router,
        "DescribeChangeSet",
        &[member("ChangeSetName", &change_set_id)],
    )
    .await;
    assert_eq!(
        xml_text(&described, "Status").as_deref(),
        Some(change_set_status::CREATE_COMPLETE),
        "expected the change set with a real diff to settle CREATE_COMPLETE, got {described}"
    );

    let (code, body) = call(
        &router,
        "ExecuteChangeSet",
        &[member("ChangeSetName", &change_set_id)],
    )
    .await;
    assert_eq!(code, 200, "expected ExecuteChangeSet to succeed, got {code} {body}");
    wait_for_the_transition().await;
    let reported = status_of(&router, "changeset-inplace").await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "expected the executed change set to complete the stack, got {reported:?} (reason {:?})",
        reason_of(&router, "changeset-inplace").await
    );
    let arn = topic_arn_of("change set", &router, "changeset-inplace").await;
    let held = topic_attributes("change set", &router, &arn).await;
    assert!(
        held.iter()
            .any(|(name, value)| name == "DisplayName" && value == "by-change-set"),
        "expected Amazon SNS to hold the display name the executed change set declared, got \
         {held:?}"
    );

    // The same revision again changes nothing, so its change set settles FAILED with the
    // reason CloudFormation gives — and executing it is refused with the change set's own
    // ARN and current status.
    let (code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "changeset-inplace"),
            member("ChangeSetName", "no-op"),
            member("ChangeSetType", "UPDATE"),
            member("TemplateBody", &revised),
        ],
    )
    .await;
    assert_eq!(code, 200, "expected CreateChangeSet to succeed, got {code} {body}");
    let no_op_id = xml_text(&body, "Id").unwrap_or_default();
    wait_for_the_transition().await;
    let (_, described) = call(
        &router,
        "DescribeChangeSet",
        &[member("ChangeSetName", &no_op_id)],
    )
    .await;
    assert_eq!(
        xml_text(&described, "Status").as_deref(),
        Some(change_set_status::FAILED),
        "expected the no-op change set to settle FAILED, got {described}"
    );
    assert_eq!(
        xml_text(&described, "StatusReason").as_deref(),
        Some(NO_CHANGES_REASON),
        "expected the no-op change set to carry the reason CloudFormation gives, got \
         {described}"
    );
    let (code, body) = call(
        &router,
        "ExecuteChangeSet",
        &[member("ChangeSetName", &no_op_id)],
    )
    .await;
    assert_eq!(
        code, 400,
        "expected executing a FAILED change set to be refused, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidChangeSetStatus"),
        "expected the modelled InvalidChangeSetStatus code, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message"),
        Some(format!(
            "ChangeSet [{no_op_id}] cannot be executed in its current status of [{}]",
            change_set_status::FAILED
        )),
        "expected the refusal to name the change set and its current status, got {body}"
    );
    // The refused execution left the stack exactly where it was.
    let reported = status_of(&router, "changeset-inplace").await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "expected the refused execution to leave the stack alone, got {reported:?}"
    );
}
