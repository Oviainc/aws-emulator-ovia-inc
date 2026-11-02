// SPDX-License-Identifier: CC0-1.0

//! `TestType` over the awsQuery wire: the contract tests of one registered extension
//! version.
//!
//! The rule every case here holds the service to: **`TestType` requires the CALLING ACCOUNT
//! to hold a CloudFormation extension-publisher registration and refuses an account that
//! holds none with the registry's modelled `CFNRegistryException` (400) carrying the
//! register-as-a-publisher sentence, before it resolves the extension the request names and
//! without writing anything; for a caller that holds one it resolves that extension against
//! the registry of THIS account and Region — an extension that registry holds no record for
//! is the modelled `TypeNotFoundException` (404) naming what the request named, and a record
//! this account did not register (a public extension) that same modelled 400 — and starts
//! the contract tests of ONE version of a PRIVATE extension this account and Region
//! registered: the version its `VersionId` (or its version ARN) names, or the record's
//! default version where it names none, answered with that version's own ARN while the
//! version's `TypeTestsStatus` is `IN_PROGRESS` and the conclusion the tests reach over the
//! model the registry holds for that version becomes due a moment later; a request that
//! identifies no extension at all is that same modelled 400, and a version the record does
//! not carry the modelled 404.**
//!
//! The table spans the class the rule generalises over: every way a request can identify an
//! extension (`Type` and `TypeName`, the extension's ARN, a version ARN) and select a
//! version (named, pinned by the ARN, named by neither), over records spanning every
//! `RegistryType` (`RESOURCE`, `MODULE`, `HOOK`) and every visibility a record can have
//! (private, public third-party, public and Amazon-Web-Services-published, and one the
//! registry of this Region does not hold at all because it was registered in another), and
//! over both conclusions the tests can reach — the version whose stored schema declares the
//! extension it defines and the version the registry holds no schema for.
//!
//! `RegisterType`, the registry's one WRITER still a scaffold stub, has an issue of its
//! own, so the records tested here are the ones
//! `extension_registry.rs` seeds straight into the store — this crate's one registry
//! fixture, so what `TestType` writes and what `DescribeType` reports back are held to the
//! same records. Every call goes through the real request path (form-encoded body in, XML
//! envelope out), so what is asserted is what a client sees.

use std::sync::Arc;
use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    public_type_arn, registry_type, type_arn, type_tests_status, type_version_arn,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{router_with_registry, OTHER_REGION};
use crate::stack_lifecycle::{
    call, call_in, expected_status, member, store, wait_for_the_transition, xml_text, REGION,
};

/// The refusal AWS answers an account that has not registered as an extension publisher,
/// which is what this operation requires of its caller. Entirely fixed text: no part of it
/// comes from the request or from the store.
pub(crate) const NOT_A_PUBLISHER: &str =
    "You must register as a publisher with CloudFormation to call test-type.";

pub(crate) async fn test_type(
    router: &MultiServiceRouter,
    members: &[(&str, &str)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "TestType", &members).await
}

async fn describe_type(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DescribeType", &members).await
}

/// Register this account as an extension publisher, which is the gate every successful test
/// below is behind.
pub(crate) async fn register_publisher(router: &MultiServiceRouter) {
    let (status_code, body) = call(
        router,
        "RegisterPublisher",
        &[member("AcceptTermsAndConditions", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"registering as a publisher\": expected 200, got {status_code} {body}"
    );
}

/// Hold a refused call to the registry's modelled refusal: HTTP 400 `CFNRegistryException`,
/// carrying `message`.
pub(crate) fn assert_registry_refusal(case: &str, status_code: u16, body: &str, message: &str) {
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the call refused with 400, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("CFNRegistryException"),
        "case {case:?}: expected CFNRegistryException, got {body}"
    );
    let reported = xml_text(body, "Message");
    assert_eq!(
        reported.as_deref(),
        Some(message),
        "case {case:?}: expected {message:?}, got {reported:?}"
    );
}

/// Hold a refused call to the modelled 404: `TypeNotFoundException`, "the specified
/// extension doesn't exist in the CloudFormation registry", naming what the request named.
pub(crate) fn assert_not_found(case: &str, status_code: u16, body: &str, identifier: &str) {
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the call refused with 404, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("TypeNotFoundException"),
        "case {case:?}: expected TypeNotFoundException, got {body}"
    );
    let reported = xml_text(body, "Message");
    let expected = format!("The type '{identifier}' cannot be found.");
    assert_eq!(
        reported.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
}

/// Every record the registry holds right now, as the one snapshot a case compares a refused
/// call against: a refusal must leave every version's test status exactly as it was.
pub(crate) fn registry_snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    format!("{:?}", store(engine).lock().extensions)
}

/// The members of one row, as the wire helpers above take them.
pub(crate) fn borrowed<'a>(members: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    members
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect()
}

/// The ARNs of the seeded records, which every case addresses its extension by.
pub(crate) fn resource_arn() -> String {
    type_arn(REGION, registry_type::RESOURCE, "My::Sample::Resource")
}

#[tokio::test]
async fn test_type_is_refused_for_an_account_that_is_not_a_registered_publisher() {
    let (router, engine) = router_with_registry();
    let before = registry_snapshot(&engine);

    // One row per thing the request can name that DOES resolve to a private registration of
    // this account and Region — a registered private extension, a module, one version of it.
    // The gate is on the CALLER, so each row is refused with the same message; the rows that
    // resolve NOTHING are refused with it too, which `test_type_publisher_gate.rs` is the
    // table for. (A request that identifies no extension at all is a property of the request
    // alone and is refused for that instead, as the row below it holds.)
    for (case, members) in [
        (
            "a registered private resource type, no version named",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
        ),
        (
            "a registered private module",
            vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
        ),
        (
            "a version of a registered private resource type",
            vec![
                ("Arn", resource_arn()),
                ("VersionId", "00000001".to_string()),
            ],
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = test_type(&router, &members).await;
        assert_registry_refusal(case, status_code, &body, NOT_A_PUBLISHER);
        assert_eq!(
            xml_text(&body, "TypeVersionArn"),
            None,
            "case {case:?}: expected no TypeVersionArn in the refusal, got {body}"
        );
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to write nothing"
        );
    }

    // A request that identifies no extension is a property of the request alone, so it is
    // refused for that rather than for the caller — before anything is resolved.
    let (status_code, body) = test_type(&router, &[("LogDeliveryBucket", "contract-test-logs")]).await;
    assert_registry_refusal(
        "a request that identifies no extension at all",
        status_code,
        &body,
        "Either Arn, or both Type and TypeName, must be specified to identify an extension",
    );
    assert_eq!(
        registry_snapshot(&engine),
        before,
        "case {:?}: expected the refused call to write nothing",
        "a request that identifies no extension at all"
    );

    // The very same call, once the account IS a registered publisher, is served: the
    // refusal above is about the caller and nothing else.
    register_publisher(&router).await;
    let (status_code, body) = test_type(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "My::Sample::Resource"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a registered publisher testing a private resource type\": expected 200, got \
         {status_code} {body}"
    );
}

/// The extension a request identifies is resolved against this account and Region's registry
/// once the CALLER gate is open: an identifier the registry holds no record for is refused the
/// modelled 404 naming what the request named, and a record this account did not register the
/// not-a-private-extension 400. What an account that is NOT a publisher is answered for the
/// same identifiers — the caller refusal, before any of this is resolved — is the table in
/// `test_type_publisher_gate.rs`.
#[tokio::test]
async fn an_identifier_the_registry_does_not_hold_is_refused_for_what_it_names() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    let before = registry_snapshot(&engine);

    let unheld_arn = type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource");
    let other_region_arn = type_arn(
        OTHER_REGION,
        registry_type::RESOURCE,
        "Other::Region::Resource",
    );
    // Each row is an identifier this Region's registry holds no record for, over both forms
    // a request identifies an extension by, a version ARN, and a record another Region's
    // registry holds; `identifier` is what the refusal must name.
    let unheld = vec![
        (
            "a name the registry holds no record for",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
            "No::Such::Resource".to_string(),
        ),
        (
            "the ARN of an extension the registry holds no record for",
            vec![("Arn", unheld_arn.clone())],
            "No::Such::Resource".to_string(),
        ),
        (
            // The version segment is split off the ARN before the record is resolved, so
            // what names the extension is the ARN's own extension part — and therefore the
            // name that part carries.
            "a version ARN of an extension the registry holds no record for",
            vec![("Arn", type_version_arn(&unheld_arn, "00000001"))],
            "No::Such::Resource".to_string(),
        ),
        (
            "a record another Region's registry holds, by ARN",
            vec![("Arn", other_region_arn.clone())],
            "Other::Region::Resource".to_string(),
        ),
    ];

    // Before the account is a publisher and after it is: the refusal is about what the
    // request named, so the caller makes no difference to it.
    for (case, members, identifier) in &unheld {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = test_type(&router, &members).await;
        assert_not_found(case, status_code, &body, identifier);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to write nothing"
        );
    }

    // A record the registry DOES hold that this account did not register: it exists, so
    // the refusal is the one about what the record IS.
    let public_arn = type_arn(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let expected = format!(
        "Cannot test {public_arn}: only a private extension registered in this account \
         and Region can be tested"
    );
    let (status_code, body) = test_type(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "MyAlias::Sample::Resource"),
        ],
    )
    .await;
    assert_registry_refusal(
        "a public third-party extension activated in this account",
        status_code,
        &body,
        &expected,
    );
    assert_eq!(
        registry_snapshot(&engine),
        before,
        "case {:?}: expected the refused call to write nothing",
        "a public third-party extension activated in this account"
    );
}

/// One row of the testing table: what the request names, the version it must test, and the
/// conclusion the tests must reach over the model the registry holds for that version.
struct TestCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    /// The `TypeVersionId` of the version the call must test.
    version_id: &'static str,
    /// The extension's own ARN, which the answered `TypeVersionArn` is built from.
    extension_arn: String,
    /// [`type_tests_status`]: `PASSED` or `FAILED`.
    concluded: &'static str,
}

fn testing_cases() -> Vec<TestCase> {
    let resource = resource_arn();
    let module = type_arn(
        REGION,
        registry_type::MODULE,
        "My::Sample::Thing::MODULE",
    );
    let hook = type_arn(REGION, registry_type::HOOK, "My::Sample::Hook");

    vec![
        TestCase {
            // "If you don't specify a version, CloudFormation uses the default version of
            // the extension in your account and Region for testing": the seeded record's
            // default version is its SECOND, so the default is distinguishable from the
            // first.
            case: "private resource type by name, no version named: the DEFAULT version",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            version_id: "00000002",
            extension_arn: resource.clone(),
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            case: "private resource type by name, VersionId naming the first version",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("VersionId", "00000001".to_string()),
            ],
            version_id: "00000001",
            extension_arn: resource.clone(),
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            case: "private resource type by ARN, no version named: the DEFAULT version",
            members: vec![("Arn", resource.clone())],
            version_id: "00000002",
            extension_arn: resource.clone(),
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            case: "private resource type by a version ARN, which pins the version",
            members: vec![("Arn", type_version_arn(&resource, "00000001"))],
            version_id: "00000001",
            extension_arn: resource.clone(),
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            case: "a version ARN beside the VersionId it pins",
            members: vec![
                ("Arn", type_version_arn(&resource, "00000001")),
                ("VersionId", "00000001".to_string()),
            ],
            version_id: "00000001",
            extension_arn: resource.clone(),
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            case: "a log delivery bucket beside the extension",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("VersionId", "00000001".to_string()),
                ("LogDeliveryBucket", "contract-test-logs".to_string()),
            ],
            version_id: "00000001",
            extension_arn: resource,
            concluded: type_tests_status::PASSED,
        },
        TestCase {
            // A MODULE, whose testing is "determining if the module's model meets all
            // necessary requirements" — and the registry holds no schema for this version,
            // so there is no model that could meet them.
            case: "private module by name: the registry holds no schema for its version",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
            version_id: "00000001",
            extension_arn: module,
            concluded: type_tests_status::FAILED,
        },
        TestCase {
            // A HOOK, whose only version is not the record's first: a version is addressed
            // by the id the record carries, never by its position.
            case: "private hook by ARN: the registry holds no schema for its version",
            members: vec![("Arn", hook.clone())],
            version_id: "00000003",
            extension_arn: hook,
            concluded: type_tests_status::FAILED,
        },
    ]
}

#[tokio::test]
async fn testing_a_version_leaves_it_in_progress_until_the_clock_concludes_the_tests() {
    let (router, _engine) = router_with_registry();
    register_publisher(&router).await;

    for TestCase {
        case,
        members,
        version_id,
        extension_arn,
        concluded,
    } in testing_cases()
    {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let started = Instant::now();
        let (status_code, body) = test_type(&router, &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        // "The Amazon Resource Name (ARN) of the extension" being tested, which is the
        // version's own ARN — the one to "pass ... into DescribeType to monitor the current
        // test status".
        let expected_arn = type_version_arn(&extension_arn, version_id);
        let reported = xml_text(&body, "TypeVersionArn");
        assert_eq!(
            reported.as_deref(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected {expected_arn:?}, got {reported:?}"
        );

        // Straight after the call the tests are running: the status is `IN_PROGRESS` until
        // the conclusion is due, and the conclusion has nothing to describe yet.
        let (status_code, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the tested version readable, got {status_code} {body}"
        );
        let reported = xml_text(&body, "TypeTestsStatus");
        let owed = expected_status(started, type_tests_status::IN_PROGRESS, concluded);
        assert_eq!(
            reported.as_deref(),
            Some(owed),
            "case {case:?}: expected {owed:?} straight after the call, got {reported:?}"
        );

        // Past the due instant the conclusion is reported, with a description exactly where
        // there is something to describe: a test that FAILED says why, and one that passed
        // invents no prose.
        wait_for_the_transition().await;
        let (_, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
        let reported = xml_text(&body, "TypeTestsStatus");
        assert_eq!(
            reported.as_deref(),
            Some(concluded),
            "case {case:?}: expected {concluded:?} once the tests concluded, got {reported:?}"
        );
        let description = xml_text(&body, "TypeTestsStatusDescription");
        if concluded == type_tests_status::FAILED {
            assert!(
                description
                    .as_deref()
                    .is_some_and(|text| text.contains(version_id)),
                "case {case:?}: expected a description naming the version, got {description:?}"
            );
        } else {
            assert_eq!(
                description, None,
                "case {case:?}: expected no invented description, got {description:?}"
            );
        }

        // A read reports the conclusion again rather than moving it on: reading the status
        // changes nothing a later caller sees.
        let (_, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
        assert_eq!(
            xml_text(&body, "TypeTestsStatus").as_deref(),
            Some(concluded),
            "case {case:?}: expected a second read to report the same conclusion, got {body}"
        );
    }
}

#[tokio::test]
async fn an_extension_this_account_and_region_did_not_register_is_not_tested() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    let before = registry_snapshot(&engine);

    let resource = resource_arn();
    // A public extension Amazon Web Services publishes, which every account's registry
    // holds without anybody registering it: the first entry of the catalogue this emulator
    // provisions, taken from the catalogue rather than written down here.
    let published = IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the catalogue holds at least one resource type");
    let published_arn = public_type_arn(REGION, registry_type::RESOURCE, published);
    let public_alias_arn = type_arn(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let aws_seeded_arn = type_arn(REGION, registry_type::RESOURCE, "AWS::Sample::Resource");

    // Each record the registry holds that this account did NOT register: a public
    // third-party extension activated here, a public record of this store, and one of the
    // public extensions Amazon Web Services publishes, addressed both by name and by ARN.
    for (case, members, arn) in [
        (
            "a public third-party extension activated in this account, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
            ],
            &public_alias_arn,
        ),
        (
            "a public third-party extension activated in this account, by ARN",
            vec![("Arn", public_alias_arn.clone())],
            &public_alias_arn,
        ),
        (
            "a public extension of this store, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "AWS::Sample::Resource".to_string()),
            ],
            &aws_seeded_arn,
        ),
        (
            "an Amazon-Web-Services-published public extension, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", published.to_string()),
            ],
            &published_arn,
        ),
        (
            "an Amazon-Web-Services-published public extension, by ARN",
            vec![("Arn", published_arn.clone())],
            &published_arn,
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = test_type(&router, &members).await;
        assert_registry_refusal(
            case,
            status_code,
            &body,
            &format!(
                "Cannot test {arn}: only a private extension registered in this account and \
                 Region can be tested"
            ),
        );
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to write nothing"
        );
    }

    // Nothing the registry of this Region does not hold is testable from it, and nothing it
    // does not hold a version of.
    for (case, members, identifier) in [
        (
            "an extension name the registry does not hold",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
            "No::Such::Resource".to_string(),
        ),
        (
            "an extension name registered under another kind",
            vec![
                ("Type", registry_type::HOOK.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            "My::Sample::Resource".to_string(),
        ),
        (
            // An extension's ARN carries the extension's NAME, so a refusal that names what
            // the request identified names that name — the same sentence the name form gets.
            "an extension ARN the registry does not hold",
            vec![(
                "Arn",
                type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
            )],
            "No::Such::Resource".to_string(),
        ),
        (
            "an extension registered in ANOTHER Region, by name",
            vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "Other::Region::Resource".to_string()),
            ],
            "Other::Region::Resource".to_string(),
        ),
        (
            "a VersionId the record does not carry",
            vec![
                ("Arn", resource.clone()),
                ("VersionId", "00000009".to_string()),
            ],
            format!("{resource}/00000009"),
        ),
        (
            "a version ARN the record does not carry",
            vec![("Arn", type_version_arn(&resource, "00000009"))],
            format!("{resource}/00000009"),
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = test_type(&router, &members).await;
        assert_not_found(case, status_code, &body, &identifier);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to write nothing"
        );
    }

    // That same record IS testable from the Region whose registry holds it — the publisher
    // registration "applies to your account in all Amazon Web Services Regions", so only
    // the record's Region is what differs.
    let (status_code, body) = call_in(
        &router,
        OTHER_REGION,
        "TestType",
        &[
            member("Type", registry_type::RESOURCE),
            member("TypeName", "Other::Region::Resource"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"an extension tested in the Region that registered it\": expected 200, got \
         {status_code} {body}"
    );
    let expected_arn = type_version_arn(
        &type_arn(
            OTHER_REGION,
            registry_type::RESOURCE,
            "Other::Region::Resource",
        ),
        "00000001",
    );
    assert_eq!(
        xml_text(&body, "TypeVersionArn").as_deref(),
        Some(expected_arn.as_str()),
        "case \"an extension tested in the Region that registered it\": expected \
         {expected_arn:?}, got {body}"
    );
}

#[tokio::test]
async fn a_request_that_identifies_no_extension_is_refused() {
    let (router, engine) = router_with_registry();
    register_publisher(&router).await;
    let before = registry_snapshot(&engine);

    let resource = resource_arn();
    // Every combination of the identifying members the model's "Conditional: You must
    // specify Arn, or TypeName and Type" rules out, plus the one selector pair that
    // contradicts itself.
    for (case, members, message) in [
        (
            "no identifying member at all",
            Vec::new(),
            "Either Arn, or both Type and TypeName, must be specified to identify an extension"
                .to_string(),
        ),
        (
            "a TypeName with no Type",
            vec![("TypeName", "My::Sample::Resource".to_string())],
            "Either Arn, or both Type and TypeName, must be specified to identify an extension"
                .to_string(),
        ),
        (
            "a Type with no TypeName",
            vec![("Type", registry_type::RESOURCE.to_string())],
            "Either Arn, or both Type and TypeName, must be specified to identify an extension"
                .to_string(),
        ),
        (
            "an Arn beside a Type and TypeName",
            vec![
                ("Arn", resource.clone()),
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            "Specify either Arn, or Type and TypeName, but not both: an extension is \
             identified by its ARN or by its kind and name"
                .to_string(),
        ),
        (
            "a VersionId that contradicts the version the Arn pins",
            vec![
                ("Arn", type_version_arn(&resource, "00000001")),
                ("VersionId", "00000002".to_string()),
            ],
            "VersionId 00000002 does not name the extension version the Arn does (00000001)"
                .to_string(),
        ),
    ] {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status_code, body) = test_type(&router, &members).await;
        assert_registry_refusal(case, status_code, &body, &message);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to write nothing"
        );
    }
}

#[tokio::test]
async fn testing_the_same_version_again_answers_the_same_version_arn() {
    let (router, _engine) = router_with_registry();
    register_publisher(&router).await;

    // `@idempotent`: a second call tests the same version and answers the same ARN, and the
    // version is in progress again rather than carrying the conclusion of the first run.
    let resource = resource_arn();
    let expected_arn = type_version_arn(&resource, "00000002");
    for attempt in ["the first call", "the same call again"] {
        let started = Instant::now();
        let (status_code, body) = test_type(&router, &[("Arn", &resource)]).await;
        assert_eq!(
            status_code, 200,
            "case {attempt:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TypeVersionArn").as_deref(),
            Some(expected_arn.as_str()),
            "case {attempt:?}: expected {expected_arn:?}, got {body}"
        );
        let (_, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
        let owed = expected_status(
            started,
            type_tests_status::IN_PROGRESS,
            type_tests_status::PASSED,
        );
        assert_eq!(
            xml_text(&body, "TypeTestsStatus").as_deref(),
            Some(owed),
            "case {attempt:?}: expected {owed:?}, got {body}"
        );
        wait_for_the_transition().await;
    }

    // And the version the second call left behind concludes the same way.
    let (_, body) = describe_type(&router, &[("Arn", &expected_arn)]).await;
    assert_eq!(
        xml_text(&body, "TypeTestsStatus").as_deref(),
        Some(type_tests_status::PASSED),
        "case \"the version both calls tested\": expected PASSED, got {body}"
    );
}
