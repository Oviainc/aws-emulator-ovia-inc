// SPDX-License-Identifier: CC0-1.0

//! `PublishType` over the awsQuery wire: which refusal an `Arn` reaches, decided from the
//! ARN's own segments.
//!
//! The rule every case here holds the service to: **an `Arn` whose ACCOUNT segment is this
//! account's is read as an ARN before the registry is consulted — its resource portion
//! addresses the extension (`type/<kind>/<Type-Name>`), so a further segment pins a VERSION,
//! and only an extension's default version is published, which is the registry's modelled
//! `CFNRegistryException` (400) carrying the fixed version-in-Arn sentence whatever the
//! extension, its kind, its Region, the version's spelling, the caller's publisher
//! registration or a `PublicVersionNumber` beside it; an `Arn` whose account segment is
//! ANOTHER account's addresses nothing of this account, so it is never read that far and
//! reaches only the account-scoped existence refusal, that same modelled 400 carrying the
//! fixed not-found-in-your-account sentence.**
//!
//! The table spans the class the rule generalises over: both sides of the account segment
//! (this account, and two other twelve-digit accounts), the version segment present and
//! absent, extensions this Region's registry holds and extensions no registry holds, every
//! `RegistryType` (`RESOURCE`, `MODULE`, `HOOK`), both Regions the registry fixture spans,
//! and three spellings of a version id the modelled `TypeVersionId`
//! (`smithy_query(query="shape", shape="TypeVersionId")`: `@length(1..128)`, `@pattern
//! "^[A-Za-z0-9-]+$"`) admits. Each row is run with and without a registered publisher and
//! with and without a `PublicVersionNumber`, because neither is what either refusal is about.
//! The recorded trace this suite was written for is ONE row: a version ARN of THIS account
//! naming an extension no registry holds.
//!
//! `publish_type.rs` is the suite for what a publication WRITES; this one is only about which
//! refusal an ARN reaches and that a refused call writes nothing. The records come from
//! `extension_registry.rs`, this crate's one registry fixture, and every call goes through the
//! real request path (form-encoded body in, XML envelope out).

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    arn_kind_segment, arn_type_name, registry_type, type_arn, type_version_arn,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{router_with_registry, OTHER_REGION};
use crate::stack_lifecycle::{call, member, store, xml_text, REGION};

/// The refusal AWS answers an `Arn` of THIS account that pins a version: only the extension's
/// default version is published. Entirely fixed text, spelled out here rather than read from
/// the crate, because the sentence IS the requirement.
const ONLY_DEFAULT_VERSION: &str =
    "Only default version can be published. Please remove versionId from Arn";

/// The refusal AWS answers an identifier that resolves no extension of this account — which
/// an ARN of another account never can. Entirely fixed text as well.
const NOT_FOUND_IN_YOUR_ACCOUNT: &str = "Type not found in your account.";

/// The refusal AWS answers an account that has not registered as an extension publisher,
/// which is what publishing requires of its caller. Entirely fixed text as well.
const NOT_A_PUBLISHER: &str =
    "You must register as a publisher with CloudFormation to call publish-type.";

/// Two twelve-digit account ids that are NOT this account's: the other side of the ARN's
/// account segment, which the modelled `PrivateTypeArn` pattern
/// (`^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$`) admits
/// exactly as it admits this account's.
const OTHER_ACCOUNTS: [&str; 2] = ["123456789012", "210987654321"];

/// Spellings of a version id a registry ARN can pin, from the modelled `TypeVersionId`
/// (`smithy_query(query="shape", shape="TypeVersionId")`: `@length(1..128)`, `@pattern
/// "^[A-Za-z0-9-]+$"`): the zero-padded form this registry mints, a single digit, and one
/// carrying letters and a dash.
const VERSION_IDS: [&str; 3] = ["00000001", "7", "A1b2-C3"];

async fn publish_type(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "PublishType", &members).await
}

/// Register this account as an extension publisher, so the caller-side gate is behind the
/// cases that run with one.
async fn register_publisher(router: &MultiServiceRouter) {
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

/// A private-type ARN of an account that is NOT this one: the same form [`type_arn`] mints,
/// with `account` in the account segment instead of this account's id.
fn other_account_type_arn(
    account: &str,
    region: &str,
    type_kind: &str,
    type_name: &str,
) -> String {
    format!(
        "arn:aws:cloudformation:{region}:{account}:type/{}/{}",
        arn_kind_segment(type_kind),
        arn_type_name(type_name)
    )
}

/// Every record the registry holds right now: a refused call must publish nothing.
fn registry_snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    format!("{:?}", store(engine).lock().extensions)
}

/// Hold a refused call to the registry's modelled refusal: HTTP 400 `CFNRegistryException`,
/// carrying `message` and no `PublicTypeArn`.
fn assert_registry_refusal(case: &str, status_code: u16, body: &str, message: &str) {
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
    assert_eq!(
        xml_text(body, "PublicTypeArn"),
        None,
        "case {case:?}: expected no PublicTypeArn in the refusal, got {body}"
    );
}

/// One row: the `Arn` the request carries, and the sentence the refusal must word.
struct ArnCase {
    case: String,
    arn: String,
    expected: &'static str,
}

/// The ARNs of the seeded private records, and the names no registry holds, over every
/// extension kind and both Regions the fixture spans — each one as an ARN of THIS account and
/// as the same ARN of another account, with a version segment and without.
fn arn_cases() -> Vec<ArnCase> {
    // (what the name is, the kind it is registered under where it is, the Region it names)
    let extensions: [(&str, &str, &str); 6] = [
        ("a registered private resource type", registry_type::RESOURCE, "My::Sample::Resource"),
        ("a registered private module", registry_type::MODULE, "My::Sample::Thing::MODULE"),
        ("a registered private hook", registry_type::HOOK, "My::Sample::Hook"),
        ("an extension no registry holds", registry_type::RESOURCE, "No::Such::Resource"),
        ("a hook no registry holds", registry_type::HOOK, "No::Such::Hook"),
        ("a record another Region's registry holds", registry_type::RESOURCE, "Other::Region::Resource"),
    ];
    let mut cases = Vec::new();
    for (what, type_kind, type_name) in extensions {
        let region = if type_name == "Other::Region::Resource" {
            OTHER_REGION
        } else {
            REGION
        };
        let own = type_arn(region, type_kind, type_name);
        // An ARN of THIS account that pins a version: the version-in-Arn refusal, read from
        // the ARN before the registry is consulted, so it holds whether or not the extension
        // is registered here.
        for version_id in VERSION_IDS {
            cases.push(ArnCase {
                case: format!("{what}, by an ARN of this account pinning version {version_id:?}"),
                arn: type_version_arn(&own, version_id),
                expected: ONLY_DEFAULT_VERSION,
            });
        }
        for account in OTHER_ACCOUNTS {
            let foreign = other_account_type_arn(account, region, type_kind, type_name);
            // The same ARN FORMS in another account: nothing of this account is addressed, so
            // the version segment is never read and the existence refusal is the only answer.
            cases.push(ArnCase {
                case: format!("{what}, by an ARN of account {account}"),
                arn: foreign.clone(),
                expected: NOT_FOUND_IN_YOUR_ACCOUNT,
            });
            cases.push(ArnCase {
                case: format!("{what}, by an ARN of account {account} pinning a version"),
                arn: type_version_arn(&foreign, VERSION_IDS[0]),
                expected: NOT_FOUND_IN_YOUR_ACCOUNT,
            });
        }
    }
    // An ARN of THIS account carrying NO version segment, for an extension no registry holds:
    // the version rule has nothing to read, so what the call runs into is existence.
    cases.push(ArnCase {
        case: "an extension no registry holds, by an ARN of this account naming no version"
            .to_string(),
        arn: type_arn(REGION, registry_type::RESOURCE, "No::Such::Resource"),
        expected: NOT_FOUND_IN_YOUR_ACCOUNT,
    });
    cases
}

/// Which refusal an `Arn` reaches is decided from the ARN's own account and version segments,
/// over every row of the table and whatever else the request carries.
#[tokio::test]
async fn which_refusal_an_arn_reaches_is_decided_by_its_account_and_version_segments() {
    let (router, engine) = router_with_registry();
    let before = registry_snapshot(&engine);

    // Four passes over the same rows: before this account is a publisher and after it is,
    // with and without a version number beside the ARN. Neither is what either refusal is
    // about, so neither may change a row's answer.
    for registered_publisher in [false, true] {
        if registered_publisher {
            register_publisher(&router).await;
        }
        for row in arn_cases() {
            for extra in [vec![], vec![("PublicVersionNumber", "1.0.0")]] {
                let mut members = vec![("Arn", row.arn.as_str())];
                members.extend_from_slice(&extra);
                let case = format!(
                    "{} (publisher registered: {registered_publisher}, extra members: {extra:?})",
                    row.case
                );
                let (status_code, body) = publish_type(&router, &members).await;
                assert_registry_refusal(&case, status_code, &body, row.expected);
                assert_eq!(
                    registry_snapshot(&engine),
                    before,
                    "case {case:?}: expected the refused call to publish nothing"
                );
            }
        }
    }
}

/// The version-in-Arn refusal is keyed on the ARN's VERSION segment, not on its account: the
/// same in-account ARN without one is read on past it, to the gate the caller is judged by.
#[tokio::test]
async fn an_in_account_arn_naming_no_version_is_read_past_the_version_rule() {
    let (router, engine) = router_with_registry();
    let before = registry_snapshot(&engine);

    for (case, type_kind, type_name) in [
        (
            "a registered private resource type",
            registry_type::RESOURCE,
            "My::Sample::Resource",
        ),
        (
            "a registered private module",
            registry_type::MODULE,
            "My::Sample::Thing::MODULE",
        ),
        (
            "a registered private hook",
            registry_type::HOOK,
            "My::Sample::Hook",
        ),
    ] {
        let arn = type_arn(REGION, type_kind, type_name);
        let (status_code, body) = publish_type(&router, &[("Arn", &arn)]).await;
        assert_registry_refusal(case, status_code, &body, NOT_A_PUBLISHER);
        assert_eq!(
            registry_snapshot(&engine),
            before,
            "case {case:?}: expected the refused call to publish nothing"
        );
    }
}
