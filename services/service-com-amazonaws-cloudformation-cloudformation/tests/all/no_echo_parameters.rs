// SPDX-License-Identifier: CC0-1.0

//! `NoEcho` template parameters, as CloudFormation echoes parameters back.
//!
//! RULE: a template parameter whose declaration carries `NoEcho` is MASKED wherever
//! CloudFormation reports the stack's or the change set's parameters — `NoEcho`: "whether to
//! mask the parameter value to prevent it from being displayed in the console, command line
//! tools, or API" — and the mask AWS documents is the fixed four-asterisk string, whatever
//! the submitted value was and however long it is. A parameter whose declaration does not
//! carry it is reported verbatim, and the masked parameter's real value is still the value in
//! force on the stack: only what is echoed back is masked.
//!
//! The rows differ along what the rule generalises over: the value (its length, from well
//! under to well over the mask's own, and its content), how the declaration spells the flag
//! (`true` as a YAML boolean, as quoted text, and mixed case, which is how a JSON template
//! writes it), whether the value was SUBMITTED or came from the declaration's `Default`,
//! the template spelling (YAML and JSON), and how many parameters one template declares.
//!
//! Every case drives the real awsQuery request path through the router (the harness
//! `stack_lifecycle.rs` owns), so what is asserted is what a client calling `DescribeStacks`
//! reads — which is the whole point: a principal with `cloudformation:DescribeStacks` must
//! not be able to read a database master password out of the stack description.

use service_com_amazonaws_cloudformation_cloudformation::state::NO_ECHO_MASK;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, sns, wait_for_the_transition, xml_text,
    xml_texts,
};

/// The `ParameterValue` a response reports for `key`.
///
/// Read as the two lists the response carries: every parameter of every template here is
/// reported with a value, so the pairing cannot slip — and a parameter reported with none
/// would shift it and fail the assertions loudly.
fn parameter_value(xml: &str, key: &str) -> Option<String> {
    let keys = xml_texts(xml, "ParameterKey");
    let values = xml_texts(xml, "ParameterValue");
    keys.iter()
        .zip(values.iter())
        .find(|(parameter_key, _)| parameter_key.as_str() == key)
        .map(|(_, value)| value.clone())
}

/// The request members that submit one `Parameters` entry per pair.
fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

/// One row: a template, the parameters submitted with it, and what each declared parameter
/// must be reported as.
struct MaskCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    submitted: &'a [(&'a str, &'a str)],
    /// `(parameter key, the value `DescribeStacks` must report)`.
    reported: &'a [(&'a str, &'a str)],
}

/// The 24-character value the issue's own stack was deployed with the shape of: a master
/// password long enough that reporting its length would itself be a disclosure.
const LONG_SECRET: &str = "Pa55word-24-characters!!";

#[tokio::test]
async fn a_no_echo_parameter_is_reported_masked_and_every_other_parameter_verbatim() {
    let cases: &[MaskCase<'_>] = &[
        MaskCase {
            case: "the issue's own shape: a 24-character master password beside an \
                   unmasked parameter, in YAML",
            stack_name: "noecho-password",
            template: concat!(
                "Parameters:\n",
                "  DbPassword:\n",
                "    Type: String\n",
                "    NoEcho: true\n",
                "  DbUser:\n",
                "    Type: String\n",
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
            ),
            submitted: &[("DbPassword", LONG_SECRET), ("DbUser", "probeadmin")],
            reported: &[("DbPassword", NO_ECHO_MASK), ("DbUser", "probeadmin")],
        },
        MaskCase {
            case: "a one-character value: the mask is the same fixed string, so its \
                   length discloses nothing",
            stack_name: "noecho-short",
            template: concat!(
                "Parameters:\n",
                "  Secret:\n",
                "    Type: String\n",
                "    NoEcho: true\n",
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
            ),
            submitted: &[("Secret", "x")],
            reported: &[("Secret", NO_ECHO_MASK)],
        },
        MaskCase {
            case: "a JSON template spelling the flag as text, and one that spells it \
                   false",
            stack_name: "noecho-json",
            template: r#"{"Parameters":{
                   "Masked":{"Type":"String","NoEcho":"true"},
                   "Shown":{"Type":"String","NoEcho":false}},
                 "Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            submitted: &[
                ("Masked", "an-example-token"),
                ("Shown", "not-a-secret"),
            ],
            reported: &[("Masked", NO_ECHO_MASK), ("Shown", "not-a-secret")],
        },
        MaskCase {
            case: "the flag in mixed case, and a masked parameter taking its declared \
                   default",
            stack_name: "noecho-default",
            template: concat!(
                "Parameters:\n",
                "  Supplied:\n",
                "    Type: String\n",
                "    NoEcho: \"True\"\n",
                "  Defaulted:\n",
                "    Type: String\n",
                "    NoEcho: true\n",
                "    Default: the-default-secret\n",
                "  Plain:\n",
                "    Type: Number\n",
                "    Default: 5432\n",
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
            ),
            submitted: &[("Supplied", "a-submitted-secret")],
            reported: &[
                ("Supplied", NO_ECHO_MASK),
                ("Defaulted", NO_ECHO_MASK),
                ("Plain", "5432"),
            ],
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) = create_stack(
            &router,
            case.stack_name,
            case.template,
            &params(case.submitted),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            case.case
        );
        for (key, expected) in case.reported {
            let actual = parameter_value(&described, key);
            assert_eq!(
                actual.as_deref(),
                Some(*expected),
                "case {:?}: DescribeStacks must report {key} as {expected:?}, got \
                 {actual:?} in {described}",
                case.case
            );
        }
        // Nothing anywhere in the description carries a masked value, whatever member it
        // might have been reported in. Only a value longer than the mask is scanned for:
        // a one- or two-character value occurs by chance in the identifiers a stack
        // description carries, and a secret worth masking is not that short.
        for (key, _) in case.submitted {
            if !case
                .reported
                .iter()
                .any(|(masked, value)| masked == key && *value == NO_ECHO_MASK)
            {
                continue;
            }
            let value = case
                .submitted
                .iter()
                .find(|(submitted, _)| submitted == key)
                .map(|(_, value)| *value)
                .unwrap_or_default();
            if value.chars().count() <= NO_ECHO_MASK.chars().count() {
                continue;
            }
            assert!(
                !described.contains(value),
                "case {:?}: the description must not carry the value of {key} anywhere, \
                 got {described}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn an_update_that_changes_a_masked_parameter_still_reports_it_masked() {
    let router = router();
    let template = concat!(
        "Parameters:\n",
        "  DbPassword:\n",
        "    Type: String\n",
        "    NoEcho: true\n",
        "Resources:\n",
        "  Topic:\n",
        "    Type: AWS::SNS::Topic\n",
    );
    create_stack(
        &router,
        "noecho-updated",
        template,
        &params(&[("DbPassword", LONG_SECRET)]),
    )
    .await;
    wait_for_the_transition().await;

    let mut members = vec![
        member("StackName", "noecho-updated"),
        member("TemplateBody", template),
    ];
    members.extend(params(&[("DbPassword", "a-rotated-master-password")]));
    let (status_code, updated) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "UpdateStack must answer 200, got {status_code} {updated}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "noecho-updated").await;
    assert_eq!(
        parameter_value(&described, "DbPassword").as_deref(),
        Some(NO_ECHO_MASK),
        "the rotated value must be masked too, got {described}"
    );
    assert!(
        !described.contains("a-rotated-master-password"),
        "the description must not carry the rotated value, got {described}"
    );
}

#[tokio::test]
async fn a_change_set_reports_a_masked_parameter_masked_too() {
    let router = router();
    let template = concat!(
        "Parameters:\n",
        "  DbPassword:\n",
        "    Type: String\n",
        "    NoEcho: true\n",
        "  DbUser:\n",
        "    Type: String\n",
        "Resources:\n",
        "  Topic:\n",
        "    Type: AWS::SNS::Topic\n",
    );
    let mut members = vec![
        member("StackName", "noecho-change-set"),
        member("ChangeSetName", "first"),
        member("ChangeSetType", "CREATE"),
        member("TemplateBody", template),
    ];
    members.extend(params(&[
        ("DbPassword", LONG_SECRET),
        ("DbUser", "probeadmin"),
    ]));
    let (status_code, created) = call(&router, "CreateChangeSet", &members).await;
    assert_eq!(
        status_code, 200,
        "CreateChangeSet must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;

    let (status_code, described) = call(
        &router,
        "DescribeChangeSet",
        &[
            member("StackName", "noecho-change-set"),
            member("ChangeSetName", "first"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeChangeSet must answer 200, got {status_code} {described}"
    );
    assert_eq!(
        parameter_value(&described, "DbPassword").as_deref(),
        Some(NO_ECHO_MASK),
        "DescribeChangeSet must mask the NoEcho parameter, got {described}"
    );
    assert_eq!(
        parameter_value(&described, "DbUser").as_deref(),
        Some("probeadmin"),
        "DescribeChangeSet must report an unmasked parameter verbatim, got {described}"
    );
    assert!(
        !described.contains(LONG_SECRET),
        "the change set description must not carry the masked value, got {described}"
    );

    // Executing the change set carries the masking onto the stack, which is the same
    // declaration governing the same values.
    let (status_code, executed) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("StackName", "noecho-change-set"),
            member("ChangeSetName", "first"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "ExecuteChangeSet must answer 200, got {status_code} {executed}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "noecho-change-set").await;
    assert_eq!(
        parameter_value(&described, "DbPassword").as_deref(),
        Some(NO_ECHO_MASK),
        "the executed stack must mask the NoEcho parameter, got {described}"
    );
}

#[tokio::test]
async fn the_masked_value_is_still_the_value_in_force_on_the_stack() {
    // The other half of the rule: masking is what CloudFormation ECHOES, not what it
    // stored. The parameter here configures a resource the stack really creates, so the
    // value that reached Amazon SNS is the submitted one — read back through SNS's own
    // wire — while `DescribeStacks` still reports only the mask.
    let router = router();
    let template = concat!(
        "Parameters:\n",
        "  Display:\n",
        "    Type: String\n",
        "    NoEcho: true\n",
        "Resources:\n",
        "  Topic:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      DisplayName: !Ref Display\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref Topic\n",
    );
    create_stack(
        &router,
        "noecho-in-force",
        template,
        &params(&[("Display", "the-display-name")]),
    )
    .await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "noecho-in-force").await;
    assert_eq!(
        parameter_value(&described, "Display").as_deref(),
        Some(NO_ECHO_MASK),
        "DescribeStacks must mask the parameter, got {described}"
    );
    let arn = xml_text(&described, "OutputValue").unwrap_or_default();
    assert!(
        arn.starts_with("arn:aws:sns:"),
        "the stack must publish the topic ARN, got {arn:?} in {described}"
    );
    let (status_code, attributes) =
        sns(&router, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
    assert_eq!(
        status_code, 200,
        "GetTopicAttributes must answer 200, got {status_code} {attributes}"
    );
    assert!(
        attributes.contains("the-display-name"),
        "the value in force must have reached the topic the stack created, got \
         {attributes}"
    );
}
