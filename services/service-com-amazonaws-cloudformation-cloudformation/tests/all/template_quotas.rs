// SPDX-License-Identifier: CC0-1.0

//! The documented per-template quotas, end to end over the awsQuery wire.
//!
//! The rule under test: a template body declaring more entries in a section than that
//! section's documented per-template quota allows is not a template CloudFormation
//! accepts, so every operation that SUBMITS a body refuses the request with a
//! `ValidationError` naming what one entry of that section IS, the count the body declares
//! and the maximum it is past — rather than accepting it and deploying a stack holding none
//! of what it declared. A body AT a quota is accepted and deployed in full.
//!
//! The table spans the class the rule generalises over: each of the sections the template
//! reader meters (`Parameters`, `Mappings`, `Resources`, `Outputs`), each at its published
//! bound and one entry past it, and each operation that takes a `TemplateBody`
//! (`CreateStack`, `UpdateStack`, `CreateChangeSet`, `ValidateTemplate`,
//! `GetTemplateSummary`). The quotas
//! themselves are the crate's own constants, which are also the ceilings
//! `DescribeAccountLimits` advertises for the account (`account_limits.rs`).

use crate::stack_lifecycle::{call, create_stack, member, router, wait_for_the_transition, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    MAX_MAPPINGS_PER_TEMPLATE, MAX_OUTPUTS_PER_TEMPLATE, MAX_PARAMETERS_PER_TEMPLATE,
    MAX_RESOURCES_PER_TEMPLATE,
};

/// One section of a template, its documented quota, and a declaration to repeat in it.
struct Section {
    /// The section as the template spells it.
    name: &'static str,
    /// The sentence the refusal of a body past this section's quota is worded with, given
    /// the count the body declares. It is the SERVICE's wording for that ONE limit rather
    /// than anything derived from the section's key — the resources are counted as a NUMBER
    /// OF things, the other sections by a COUNT, each with its own noun — so each row spells
    /// the sentence its own limit is refused with.
    refusal: fn(usize, usize) -> String,
    quota: usize,
    /// One entry of this section, given its index.
    entry: fn(usize) -> (String, serde_json::Value),
}

/// The sections the template reader meters, with the documented quota on each.
fn sections() -> Vec<Section> {
    vec![
        Section {
            name: "Parameters",
            refusal: |declared, quota| {
                format!(
                    "Template format error: Parameter count {declared} is greater than max \
                     allowed {quota}"
                )
            },
            quota: MAX_PARAMETERS_PER_TEMPLATE,
            // A parameter carrying a default needs no value from the request, so a body at
            // the quota is submittable as it stands.
            entry: |index| {
                (
                    format!("Param{index:04}"),
                    serde_json::json!({"Type": "String", "Default": "value"}),
                )
            },
        },
        Section {
            name: "Mappings",
            refusal: |declared, quota| {
                format!(
                    "Template format error: Mappings count {declared} is greater than max \
                     allowed {quota}"
                )
            },
            quota: MAX_MAPPINGS_PER_TEMPLATE,
            entry: |index| {
                (
                    format!("Map{index:04}"),
                    serde_json::json!({"us-east-1": {"Value": "value"}}),
                )
            },
        },
        Section {
            name: "Resources",
            refusal: |declared, quota| {
                format!(
                    "Template format error: Number of resources, {declared}, is greater than \
                     maximum allowed, {quota}"
                )
            },
            quota: MAX_RESOURCES_PER_TEMPLATE,
            entry: |index| {
                (
                    format!("Topic{index:04}"),
                    serde_json::json!({"Type": "AWS::SNS::Topic"}),
                )
            },
        },
        Section {
            name: "Outputs",
            refusal: |declared, quota| {
                format!(
                    "Template format error: Outputs count {declared} is greater than max \
                     allowed {quota}"
                )
            },
            quota: MAX_OUTPUTS_PER_TEMPLATE,
            entry: |index| {
                (
                    format!("Out{index:04}"),
                    serde_json::json!({"Value": {"Ref": "AWS::StackName"}}),
                )
            },
        },
    ]
}

/// A template declaring `count` entries in one section.
fn template_with(section: &Section, count: usize) -> String {
    let entries: serde_json::Map<String, serde_json::Value> =
        (0..count).map(section.entry).collect();
    serde_json::json!({ section.name: entries }).to_string()
}

/// The refusal a body past `section`'s quota is answered with.
fn expected_message(section: &Section, declared: usize) -> String {
    (section.refusal)(declared, section.quota)
}

#[tokio::test]
async fn a_create_is_held_to_every_documented_section_quota() {
    let router = router();
    for section in sections() {
        // The published bound: accepted.
        let (status_code, body) = create_stack(
            &router,
            &format!("at-the-{}-quota", section.name.to_lowercase()),
            &template_with(&section, section.quota),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case \"{} at the documented quota\": expected the create to succeed, got \
             {status_code} {body}",
            section.name
        );

        // One entry past it: refused, naming the section and the maximum.
        let declared = section.quota + 1;
        let (status_code, body) = create_stack(
            &router,
            &format!("past-the-{}-quota", section.name.to_lowercase()),
            &template_with(&section, declared),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 400,
            "case \"{} past the documented quota\": expected the create refused, got \
             {status_code} {body}",
            section.name
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case \"{} past the documented quota\": expected ValidationError, got {body}",
            section.name
        );
        assert_eq!(
            xml_text(&body, "Message"),
            Some(expected_message(&section, declared)),
            "case \"{} past the documented quota\": expected the refusal to name the section \
             and the maximum, got {body}",
            section.name
        );
    }
}

#[tokio::test]
async fn every_operation_that_submits_a_template_is_held_to_the_same_quota() {
    let router = router();
    let (status_code, body) = create_stack(&router, "quota-holder", "{}", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"the stack the submitting operations name\": expected the create to succeed, got \
         {status_code} {body}"
    );
    // The stack has to have settled before it can be updated, which is what lets the
    // UPDATE row below reach the template it submits.
    wait_for_the_transition().await;

    let section = sections()
        .into_iter()
        .find(|section| section.name == "Outputs")
        .expect("the Outputs section");
    let declared = section.quota + 1;
    let over_quota = template_with(&section, declared);
    let expected = expected_message(&section, declared);

    for (action, members) in [
        (
            "CreateStack",
            vec![
                member("StackName", "another-stack"),
                member("TemplateBody", &over_quota),
            ],
        ),
        (
            "UpdateStack",
            vec![
                member("StackName", "quota-holder"),
                member("TemplateBody", &over_quota),
            ],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", "quota-holder"),
                member("ChangeSetName", "over-quota"),
                member("TemplateBody", &over_quota),
            ],
        ),
        (
            "ValidateTemplate",
            vec![member("TemplateBody", &over_quota)],
        ),
        // The operations that only READ a body are held to the same quotas: a template
        // CloudFormation will never accept is not summarised as a valid one.
        (
            "GetTemplateSummary",
            vec![member("TemplateBody", &over_quota)],
        ),
    ] {
        let (status_code, body) = call(&router, action, &members).await;
        assert_eq!(
            status_code, 400,
            "case {action:?}: expected the request refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {action:?}: expected ValidationError, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message"),
            Some(expected.clone()),
            "case {action:?}: expected the same refusal every submitting operation gives, got \
             {body}"
        );
    }
}
