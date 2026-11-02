// SPDX-License-Identifier: CC0-1.0

//! The CALLER gate on `TestType`, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`TestType` is open only to a calling
//! account that holds a CloudFormation extension-publisher registration; an account that
//! holds none is refused with the registry's modelled `CFNRegistryException` (400) carrying
//! the whole fixed sentence `You must register as a publisher with CloudFormation to call
//! test-type.` BEFORE the extension the request names is resolved against any registry — so
//! the refusal is the same whatever the request identifies and whether or not a record
//! answers to it, nothing is written, and nothing about the registry's contents reaches a
//! caller the gate is shut for.**
//!
//! The table spans what the rule generalises over: every way a request identifies an
//! extension (`Type` and `TypeName`, the extension's own ARN, a version ARN, an ARN beside
//! the `VersionId` it pins) over every record such an identifier can resolve to — a private
//! registration of this account and Region of each `RegistryType` (`RESOURCE`, `MODULE`,
//! `HOOK`), a public third-party extension activated here, a public extension of this store,
//! one of the public extensions Amazon Web Services publishes, a record only another
//! Region's registry holds, a version no record carries, and an extension nothing registered
//! anywhere — and, taken from `publisher_gate_scope.rs` so the two suites share one set of
//! names, extension names spanning what the modelled `TypeName` admits (both published length
//! bounds, a name bearing digits, the optional `::MODULE` suffix) under each `RegistryType`.
//! Every row is driven TWICE — once as an account that is not a publisher, once as
//! one that is — so the registration is the only difference between the refusal and the
//! answer the identifier is owed, which is what makes this an ORDERING assertion rather than
//! a wording one.
//!
//! `type_testing.rs` is the suite for what `TestType` does once the gate is open, and
//! `publisher_eligibility.rs` the one for the account enrolment behind the registration;
//! this one is the gate's position in the operation.

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    public_type_arn, registry_type, type_arn, type_version_arn,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;

use crate::extension_registry::{router_with_registry, OTHER_REGION};
use crate::publisher_gate_scope::{unregistered, Unregistered};
use crate::stack_lifecycle::{xml_text, REGION};
use crate::type_testing::{
    assert_not_found, assert_registry_refusal, borrowed, register_publisher, registry_snapshot,
    resource_arn, test_type, NOT_A_PUBLISHER,
};

/// What the SAME request is answered once the calling account IS a registered publisher: the
/// extension-level outcome the caller gate was standing in front of.
enum Served {
    /// 200, naming the ARN of the version the call tests.
    Tested(String),
    /// The modelled 404, naming what the request named.
    NotFound(String),
    /// The registry's modelled 400, carrying that message.
    Refused(String),
}

/// One row: what the request identifies, and what identifying it is owed once the gate is
/// open.
struct Row {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    served: Served,
}

/// The refusal a record the registry holds but this account did not REGISTER is owed: there
/// is no contract-test status on a public extension to test.
fn not_a_private_extension(arn: &str) -> String {
    format!(
        "Cannot test {arn}: only a private extension registered in this account and Region \
         can be tested"
    )
}

fn rows() -> Vec<Row> {
    let mut rows = named_rows();
    // The name shapes `publisher_gate_scope.rs` spans the modelled `TypeName` constraint
    // with, driven through the GATED operation here: that suite holds the UNGATED operations
    // to the modelled 404 on each of these names, so crossing them with the gate keeps both
    // halves of one rule on one set of names rather than on two tables.
    rows.extend(unregistered().into_iter().map(
        |Unregistered {
             case,
             type_kind,
             type_name,
         }| Row {
            case,
            members: vec![
                ("Type", type_kind.to_string()),
                ("TypeName", type_name.clone()),
            ],
            served: Served::NotFound(type_name),
        },
    ));
    rows
}

/// The rows written out here, one per record an identifier can resolve to.
fn named_rows() -> Vec<Row> {
    let resource = resource_arn();
    let module = type_arn(
        REGION,
        registry_type::MODULE,
        "My::Sample::Thing::MODULE",
    );
    let hook = type_arn(REGION, registry_type::HOOK, "My::Sample::Hook");
    let public_alias = type_arn(REGION, registry_type::RESOURCE, "MyAlias::Sample::Resource");
    let public_of_this_store = type_arn(REGION, registry_type::RESOURCE, "AWS::Sample::Resource");
    // One of the public extensions Amazon Web Services publishes, taken from the catalogue
    // this emulator provisions rather than written down here.
    let published = IMPLEMENTED_RESOURCE_TYPES
        .first()
        .expect("the catalogue holds at least one resource type");
    let published_arn = public_type_arn(REGION, registry_type::RESOURCE, published);
    let unheld = type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource");

    vec![
        Row {
            // The seeded record's default version is its SECOND, so "the default version of
            // the extension in your account and Region" is distinguishable from the first.
            case: "a private resource type by name, no version named",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
            ],
            served: Served::Tested(type_version_arn(&resource, "00000002")),
        },
        Row {
            case: "a private resource type by its own ARN",
            members: vec![("Arn", resource.clone())],
            served: Served::Tested(type_version_arn(&resource, "00000002")),
        },
        Row {
            case: "a private resource type by a version ARN",
            members: vec![("Arn", type_version_arn(&resource, "00000001"))],
            served: Served::Tested(type_version_arn(&resource, "00000001")),
        },
        Row {
            case: "a private resource type by ARN beside the VersionId it pins",
            members: vec![
                ("Arn", type_version_arn(&resource, "00000001")),
                ("VersionId", "00000001".to_string()),
            ],
            served: Served::Tested(type_version_arn(&resource, "00000001")),
        },
        Row {
            case: "a private resource type by name and VersionId, with a log bucket",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "My::Sample::Resource".to_string()),
                ("VersionId", "00000001".to_string()),
                ("LogDeliveryBucket", "contract-test-logs".to_string()),
            ],
            served: Served::Tested(type_version_arn(&resource, "00000001")),
        },
        Row {
            case: "a private module by name",
            members: vec![
                ("Type", registry_type::MODULE.to_string()),
                ("TypeName", "My::Sample::Thing::MODULE".to_string()),
            ],
            served: Served::Tested(type_version_arn(&module, "00000001")),
        },
        Row {
            case: "a private hook by ARN",
            members: vec![("Arn", hook.clone())],
            served: Served::Tested(type_version_arn(&hook, "00000003")),
        },
        Row {
            case: "a public third-party extension activated in this account, by name",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "MyAlias::Sample::Resource".to_string()),
            ],
            served: Served::Refused(not_a_private_extension(&public_alias)),
        },
        Row {
            case: "a public extension of this store, by ARN",
            members: vec![("Arn", public_of_this_store.clone())],
            served: Served::Refused(not_a_private_extension(&public_of_this_store)),
        },
        Row {
            case: "an Amazon-Web-Services-published public extension, by name",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", published.to_string()),
            ],
            served: Served::Refused(not_a_private_extension(&published_arn)),
        },
        Row {
            case: "an extension name nothing registered in this Region",
            members: vec![
                ("Type", registry_type::RESOURCE.to_string()),
                ("TypeName", "No::Such::Resource".to_string()),
            ],
            served: Served::NotFound("No::Such::Resource".to_string()),
        },
        Row {
            // The shape of the request the behavioural recording sends: an extension ARN of
            // this account carrying a version segment, naming a record nothing registered.
            case: "a version ARN of an extension nothing registered in this Region",
            members: vec![("Arn", type_version_arn(&unheld, "00000001"))],
            served: Served::NotFound("No::Such::Resource".to_string()),
        },
        Row {
            case: "a record only another Region's registry holds, by ARN",
            members: vec![(
                "Arn",
                type_arn(
                    OTHER_REGION,
                    registry_type::RESOURCE,
                    "Other::Region::Resource",
                ),
            )],
            served: Served::NotFound("Other::Region::Resource".to_string()),
        },
        Row {
            case: "a VersionId a registered private record does not carry",
            members: vec![
                ("Arn", resource.clone()),
                ("VersionId", "00000009".to_string()),
            ],
            served: Served::NotFound(format!("{resource}/00000009")),
        },
    ]
}

#[tokio::test]
async fn an_account_that_is_not_a_publisher_is_refused_before_the_extension_is_resolved() {
    let (router, engine) = router_with_registry();
    let before = registry_snapshot(&engine);

    // The account holds no publisher registration, so every row — whatever it identifies,
    // and whether or not the registry holds anything for it — is answered that one fixed
    // sentence, hands back no version to poll, and leaves every record exactly as it was.
    for Row { case, members, .. } in rows() {
        let (status_code, body) = test_type(&router, &borrowed(&members)).await;
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
}

#[tokio::test]
async fn the_registration_is_the_only_difference_between_the_refusal_and_the_answer() {
    let (router, engine) = router_with_registry();

    // The same rows, as an account that IS a registered publisher: each one is answered what
    // its identifier is owed — the version it names tested, or the refusal that belongs to
    // the record it named — which is what the caller refusal above was standing in front of.
    register_publisher(&router).await;
    for Row {
        case,
        members,
        served,
    } in rows()
    {
        let before = registry_snapshot(&engine);
        let (status_code, body) = test_type(&router, &borrowed(&members)).await;
        match served {
            Served::Tested(version_arn) => {
                assert_eq!(
                    status_code, 200,
                    "case {case:?}: expected 200, got {status_code} {body}"
                );
                let reported = xml_text(&body, "TypeVersionArn");
                assert_eq!(
                    reported.as_deref(),
                    Some(version_arn.as_str()),
                    "case {case:?}: expected {version_arn:?}, got {reported:?}"
                );
            }
            Served::NotFound(identifier) => {
                assert_not_found(case, status_code, &body, &identifier);
                assert_eq!(
                    registry_snapshot(&engine),
                    before,
                    "case {case:?}: expected the refused call to write nothing"
                );
            }
            Served::Refused(message) => {
                assert_registry_refusal(case, status_code, &body, &message);
                assert_eq!(
                    registry_snapshot(&engine),
                    before,
                    "case {case:?}: expected the refused call to write nothing"
                );
            }
        }
    }
}

/// A caller the gate is shut for learns nothing about the registry from this operation: the
/// refusal a record it is not allowed to test would get is the SAME sentence as the one an
/// extension nothing registered gets, so the answer cannot be read as evidence that a record
/// exists.
#[tokio::test]
async fn the_refusal_tells_a_non_publisher_nothing_about_what_the_registry_holds() {
    let (router, _engine) = router_with_registry();

    let mut answers = Vec::new();
    for Row { case, members, .. } in rows() {
        let (status_code, body) = test_type(&router, &borrowed(&members)).await;
        answers.push((
            case,
            status_code,
            xml_text(&body, "Code"),
            xml_text(&body, "Message"),
        ));
    }
    let (first_case, first_status, first_code, first_message) = answers
        .first()
        .cloned()
        .expect("the table holds at least one row");
    for (case, status_code, code, message) in &answers {
        assert_eq!(
            (*status_code, code.clone(), message.clone()),
            (first_status, first_code.clone(), first_message.clone()),
            "case {case:?}: expected the same refusal as case {first_case:?}, got \
             {status_code} {code:?} {message:?}"
        );
    }
}
