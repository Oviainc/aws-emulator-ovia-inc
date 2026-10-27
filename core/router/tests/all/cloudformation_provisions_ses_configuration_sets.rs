// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SES::ConfigurationSet` resources its template declares — in Amazon SES.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns the type created it, so the handle the stack publishes for that resource
//! (`Ref`/`Fn::GetAtt`) addresses a resource that service really holds.** For a configuration
//! set the handle is its NAME, so after the create waiter returns, the owning service's own
//! read (`DescribeConfigurationSet`) and the out-of-band write a client changes a set with
//! answer for the stack's own output — rather than Amazon SES refusing
//! `ConfigurationSetDoesNotExist` for a set its own stack just published. Two crates have to
//! agree for that to hold (CloudFormation makes the call, Amazon SES serves it), so the
//! suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the set
//! or leaves the name to CloudFormation, whether the name arrives through a template
//! parameter and an intrinsic rather than as a literal, how many sets one stack declares,
//! and whether the declaration carries the one further property of the type that reaches a
//! call of the owning service (a custom open-and-click tracking domain, which is a second
//! call on the set just created). Every expected value is computed from what the row
//! declares, never read back out of the answer being checked.
//!
//! The deletion half, the refusal half and the not-mapped half are rows of the same rule:
//! deleting the stack deletes the sets it created, a set Amazon SES REFUSES to create leaves
//! the stack `ROLLBACK_COMPLETE` naming the resource rather than publishing a name nothing
//! answers to, and a declared property this emulator sends to no call of the owning service
//! fails the resource rather than reporting a set configured differently from its
//! declaration. Drift detection is the same rule read the other way: it asks Amazon SES what
//! the set IS now, so a set changed out of band is `MODIFIED` and an untouched one `IN_SYNC`.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// The region every request here is signed for, and so the region the stack's configuration
/// sets belong to. Deliberately NOT the emulator's default region: the call that creates the
/// set has to be made in the Region the stack was created in.
const REGION: &str = "us-west-2";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. A configuration set has no transient status of its own: Amazon SES answers
/// `CreateConfigurationSet` with a set that is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The domain this suite verifies as an identity before declaring a tracking domain under
/// it: `CreateConfigurationSetTrackingOptions` refuses a tracking domain that "is not
/// verified in Amazon SES", and a verified domain covers its subdomains.
const VERIFIED_DOMAIN: &str = "example.com";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
/// The form components are encoded by the test crate's one awsQuery encoder
/// ([`crate::query_wire::form_encode`]), so a template body full of `{`, `"` and `&` survives
/// the body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon SES operation (API version 2010-12-01).
async fn ses(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "ses", "2010-12-01", action, members).await
}

/// One Amazon SES call that must be answered.
async fn ses_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = ses(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SES {action}, got {status} with {body}"
    );
    body
}

/// Read the configuration set back through the owning service, asking for the one attribute
/// group a declaration of this type can carry.
async fn describe_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    ses(
        router,
        "DescribeConfigurationSet",
        &[
            ("ConfigurationSetName", name),
            ("ConfigurationSetAttributeNames.member.1", "trackingOptions"),
        ],
    )
    .await
}

/// [`describe_configuration_set`] where the read must be answered.
async fn describe_configuration_set_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, body) = describe_configuration_set(router, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {name:?} back, got {status} with {body}"
    );
    body
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn error_code(xml: &str) -> String {
    opt_element(xml, "Code").unwrap_or_default()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Verify `VERIFIED_DOMAIN` as a domain identity, so a tracking subdomain under it is one
/// Amazon SES accepts.
async fn verify_domain(router: &MultiServiceRouter, case: &str) {
    ses_ok(
        router,
        case,
        "VerifyDomainIdentity",
        &[("Domain", VERIFIED_DOMAIN)],
    )
    .await;
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
///
/// `parameters` are the template `Parameters` the `CreateStack` call supplies.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut owned: Vec<(String, String)> = vec![
        ("StackName".to_string(), stack_name.to_string()),
        ("TemplateBody".to_string(), template.to_string()),
    ];
    for (index, (key, value)) in parameters.iter().enumerate() {
        let position = index + 1;
        owned.push((
            format!("Parameters.member.{position}.ParameterKey"),
            (*key).to_string(),
        ));
        owned.push((
            format!("Parameters.member.{position}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    let members: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for a
/// resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// What the row says about one configuration set the template declares.
struct DeclaredSet {
    /// The output key whose value is the set's name (`{"Ref": <logical id>}`).
    name_output: &'static str,
    /// The resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The name the resource must end up carrying, or `None` where CloudFormation generates
    /// it.
    expected_name: Option<&'static str>,
    /// The custom open-and-click domain `DescribeConfigurationSet` must report for the
    /// created set, computed from what the row's template declares.
    expected_tracking_domain: Option<&'static str>,
}

/// One row: a template and the configuration sets it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The template `Parameters` the `CreateStack` call supplies.
    parameters: &'static [(&'static str, &'static str)],
    sets: &'static [DeclaredSet],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named set with no further properties",
        stack_name: "named-set-stack",
        template: r#"{"Resources":{"Audit":{"Type":"AWS::SES::ConfigurationSet",
               "Properties":{"Name":"stack-audit-set"}}},
             "Outputs":{"AuditName":{"Value":{"Ref":"Audit"}}}}"#,
        parameters: &[],
        sets: &[DeclaredSet {
            name_output: "AuditName",
            logical_id: "Audit",
            expected_name: Some("stack-audit-set"),
            expected_tracking_domain: None,
        }],
    },
    Row {
        case: "an unnamed set, whose name CloudFormation generates",
        stack_name: "unnamed-set-stack",
        template: r#"{"Resources":{"Plain":{"Type":"AWS::SES::ConfigurationSet"}},
             "Outputs":{"PlainName":{"Value":{"Ref":"Plain"}}}}"#,
        parameters: &[],
        sets: &[DeclaredSet {
            name_output: "PlainName",
            logical_id: "Plain",
            expected_name: None,
            expected_tracking_domain: None,
        }],
    },
    Row {
        // The name arrives through a template parameter and an intrinsic rather than as a
        // literal, so the set is created under the RESOLVED name.
        case: "a set whose name comes from a parameter through Ref",
        stack_name: "parameterised-set-stack",
        template: r#"{"Parameters":{"SetName":{"Type":"String"}},
             "Resources":{"Named":{"Type":"AWS::SES::ConfigurationSet",
               "Properties":{"Name":{"Ref":"SetName"}}}},
             "Outputs":{"NamedName":{"Value":{"Ref":"Named"}}}}"#,
        parameters: &[("SetName", "stack_parameterised_set")],
        sets: &[DeclaredSet {
            name_output: "NamedName",
            logical_id: "Named",
            expected_name: Some("stack_parameterised_set"),
            expected_tracking_domain: None,
        }],
    },
    Row {
        case: "two sets, one of them carrying a custom open-and-click tracking domain",
        stack_name: "tracked-set-stack",
        template: r#"{"Resources":{
               "Tracked":{"Type":"AWS::SES::ConfigurationSet",
                 "Properties":{"Name":"stack-tracked-set",
                   "TrackingOptions":{"CustomRedirectDomain":"click.example.com"}}},
               "Untracked":{"Type":"AWS::SES::ConfigurationSet",
                 "Properties":{"Name":"stack-untracked-set"}}},
             "Outputs":{"TrackedName":{"Value":{"Ref":"Tracked"}},
               "UntrackedName":{"Value":{"Ref":"Untracked"}}}}"#,
        parameters: &[],
        sets: &[
            DeclaredSet {
                name_output: "TrackedName",
                logical_id: "Tracked",
                expected_name: Some("stack-tracked-set"),
                expected_tracking_domain: Some("click.example.com"),
            },
            DeclaredSet {
                name_output: "UntrackedName",
                logical_id: "Untracked",
                expected_name: Some("stack-untracked-set"),
                expected_tracking_domain: None,
            },
        ],
    },
];

#[tokio::test]
async fn a_completed_stack_published_configuration_sets_ses_really_holds() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();
        verify_domain(&router, case).await;

        let body =
            create_and_settle(&router, case, row.stack_name, row.template, row.parameters).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = outputs_of(&body);

        for set in row.sets {
            let name = outputs
                .get(set.name_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", set.name_output)
                })
                .clone();
            match set.expected_name {
                Some(expected) => assert_eq!(
                    name, expected,
                    "case {case:?}: expected the declared name {expected:?}, got {name:?}"
                ),
                None => assert!(
                    is_generated_name(&name, row.stack_name, set.logical_id),
                    "case {case:?}: expected a generated <stack>-<logical id>-<suffix> name, \
                     got {name:?}"
                ),
            }

            // The owning service really holds it, under the name the stack published.
            let described = describe_configuration_set_ok(&router, case, &name).await;
            assert_eq!(
                opt_element(&described, "Name").as_deref(),
                Some(name.as_str()),
                "case {case:?}: expected SES to report the set as {name:?}, got {described}"
            );
            assert_eq!(
                opt_element(&described, "CustomRedirectDomain").as_deref(),
                set.expected_tracking_domain,
                "case {case:?}: expected the tracking domain the template declared for \
                 {name:?}, got {described}"
            );

            // And the write a client changes a set with lands on the set the stack
            // published — a set Amazon SES did not hold would refuse this as
            // ConfigurationSetDoesNotExist.
            let update = if set.expected_tracking_domain.is_some() {
                "UpdateConfigurationSetTrackingOptions"
            } else {
                "CreateConfigurationSetTrackingOptions"
            };
            ses_ok(
                &router,
                case,
                update,
                &[
                    ("ConfigurationSetName", &name),
                    ("TrackingOptions.CustomRedirectDomain", "links.example.com"),
                ],
            )
            .await;
            let read = describe_configuration_set_ok(&router, case, &name).await;
            assert_eq!(
                opt_element(&read, "CustomRedirectDomain").as_deref(),
                Some("links.example.com"),
                "case {case:?}: expected the out-of-band tracking domain to be readable for \
                 {name:?}, got {read}"
            );

            // The stack still reports the resource it created, under the same physical id: a
            // write through Amazon SES changes the set, not the stack's record of it.
            let (status, reported) = cloudformation(
                &router,
                "DescribeStackResources",
                &[("StackName", row.stack_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} \
                 with {reported}"
            );
            assert!(
                elements(&reported, "PhysicalResourceId").contains(&name),
                "case {case:?}: expected the stack to still report {name:?}, got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_configuration_sets_it_created() {
    let router = build_router();
    let case = "two sets deleted with their stack";
    let stack_name = "deleted-set-stack";
    let template = r#"{"Resources":{
           "Kept":{"Type":"AWS::SES::ConfigurationSet",
             "Properties":{"Name":"deleted-stack-kept"}},
           "Other":{"Type":"AWS::SES::ConfigurationSet"}},
         "Outputs":{"KeptName":{"Value":{"Ref":"Kept"}},
           "OtherName":{"Value":{"Ref":"Other"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let names: Vec<String> = ["KeptName", "OtherName"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for name in &names {
        describe_configuration_set_ok(&router, case, name).await;
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every set the stack created is gone from Amazon SES, which is what makes the stack's
    // resources the stack's own rather than leftovers.
    for name in &names {
        let (status, answer) = describe_configuration_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the set {name:?} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ConfigurationSetDoesNotExist",
            "case {case:?}: expected ConfigurationSetDoesNotExist for {name:?}, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_configuration_set_ses_refuses_to_create_fails_the_stack() {
    let router = build_router();
    let case = "a name carrying a character the owning service's name rule excludes";
    let stack_name = "refused-set-stack";
    // `ConfigurationSet$Name` must "contain only letters (a-z, A-Z), numbers (0-9),
    // underscores (_), or dashes (-)" — a request Amazon SES's own CreateConfigurationSet
    // refuses, which is what the stack must report rather than completing.
    let template = r#"{"Resources":{"Bad":{"Type":"AWS::SES::ConfigurationSet",
           "Properties":{"Name":"refused.stack.set"}}},
         "Outputs":{"BadName":{"Value":{"Ref":"Bad"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than publish a \
         configuration set name nothing answers to, got {body}"
    );
    // The STACK's reason summarises which resource failed; the owning service's own
    // refusal is reported on that resource's event.
    let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Bad"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported
                .contains("The configuration set name must contain only letters")),
        "case {case:?}: expected the failing resource's event to carry the owning service's \
         own refusal, got {events}"
    );

    let (status, answer) = describe_configuration_set(&router, "refused.stack.set").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected no set of that name, got {status} with {answer}"
    );
}

/// A declared property this emulator sends to NO call of the owning service fails the
/// resource rather than reporting a set configured differently from its declaration — and
/// leaves nothing behind, since the stack records no physical id it could later delete.
#[tokio::test]
async fn a_property_no_ses_call_takes_fails_the_stack_and_leaves_no_set() {
    let router = build_router();
    let case = "a sending-options property, which no operation of this emulator takes";
    let stack_name = "unmapped-set-stack";
    let template = r#"{"Resources":{"Paused":{"Type":"AWS::SES::ConfigurationSet",
           "Properties":{"Name":"unmapped-stack-set",
             "SendingOptions":{"SendingEnabled":false}}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the stack to report the property it could not apply rather \
         than a set created without it, got {body}"
    );
    // The STACK's reason summarises which resource failed; WHICH property could not be
    // applied is that resource's own diagnosis, on its event.
    let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Paused"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("SendingOptions")),
        "case {case:?}: expected the failing resource's event to name the property, got \
         {events}"
    );

    let (status, answer) = describe_configuration_set(&router, "unmapped-stack-set").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected no set to have been created, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ConfigurationSetDoesNotExist",
        "case {case:?}: expected ConfigurationSetDoesNotExist, got {answer}"
    );
}

/// Drift detection reads the configuration set back off Amazon SES, so a set whose declared
/// configuration was changed OUT OF BAND — the ordinary way a client changes one, and the way
/// drift is induced — is reported `MODIFIED`, while an untouched one of the same stack is
/// `IN_SYNC`.
#[tokio::test]
async fn a_configuration_set_changed_out_of_band_is_detected_as_drifted() {
    let router = build_router();
    let case = "a declared tracking domain changed through UpdateConfigurationSetTrackingOptions";
    verify_domain(&router, case).await;
    let stack_name = "drifted-set-stack";
    let template = r#"{"Resources":{
           "Drifted":{"Type":"AWS::SES::ConfigurationSet",
             "Properties":{"Name":"drifted-stack-set",
               "TrackingOptions":{"CustomRedirectDomain":"declared.example.com"}}},
           "Untouched":{"Type":"AWS::SES::ConfigurationSet",
             "Properties":{"Name":"untouched-stack-set",
               "TrackingOptions":{"CustomRedirectDomain":"declared.example.com"}}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    ses_ok(
        &router,
        case,
        "UpdateConfigurationSetTrackingOptions",
        &[
            ("ConfigurationSetName", "drifted-stack-set"),
            (
                "TrackingOptions.CustomRedirectDomain",
                "drifted.example.com",
            ),
        ],
    )
    .await;

    let (status, body) = cloudformation(&router, "DetectStackDrift", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DetectStackDrift, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (status, body) = cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    assert_eq!(
        opt_element(&body, "StackDriftStatus").as_deref(),
        Some("DRIFTED"),
        "case {case:?}: expected the stack to report the drift its set carries, got {body}"
    );

    // Per resource: the one that was written to is MODIFIED, the one nothing touched is
    // IN_SYNC — read off Amazon SES, not off CloudFormation's record of what it applied.
    let (status, body) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with {body}"
    );
    let reported: BTreeMap<String, String> = elements(&body, "LogicalResourceId")
        .into_iter()
        .zip(elements(&body, "StackResourceDriftStatus"))
        .collect();
    for (logical_id, expected) in [("Drifted", "MODIFIED"), ("Untouched", "IN_SYNC")] {
        assert_eq!(
            reported.get(logical_id).map(String::as_str),
            Some(expected),
            "case {case:?}: expected {logical_id} to be {expected}, got {reported:?}"
        );
    }
}
