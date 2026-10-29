// SPDX-License-Identifier: CC0-1.0

//! The Region segment of the licence ARN an acceptance mints, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: an ARN this service mints takes its Region segment from
//! the caller's SigV4 credential scope only when that field is a well-formed AWS
//! Region label, and otherwise from the emulator's default Region — so every
//! `licenseArn` an entitlement reports satisfies the modelled `AwsArn` pattern
//! whatever text the `Authorization` header carried, and can always be handed
//! back to `SearchAgreements`' `LicenseArn` filter, whose own `FilterValue`
//! pattern that Region grammar is inside.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_type_filter, call, call_as, create, emulator, new_request, term, Answer,
};

/// The Region an ARN this service mints is in when the caller signed for text
/// that is no Region at all — `::engine::emulator_arn`'s documented default,
/// spelled out here rather than imported so a handler minting in some other
/// Region fails this test.
const DEFAULT_REGION: &str = "us-east-1";

/// The account id every generated emulator presents.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// The longest Region label every AWS model's ARN pattern admits in its Region
/// position (`[a-z0-9-]{1,20}`), as `service_common::arn` states it — the
/// published bound this suite tests on, one inside and one past.
const LONGEST_REGION_LABEL: &str = "us-east-1-us-east-1x";

/// A credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request to it, with `region` in the Region field
/// exactly as a client wrote it.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{region}/aws-marketplace/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Whether `value` satisfies `AwsArn`'s modelled pattern, which
/// `AgreementEntitlement$licenseArn` targets — from
/// `smithy_query(query="shape", shape="AwsArn")`:
/// `^arn:aws[a-zA-Z-]*:[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,1023}$`,
/// with `@length(1, 2048)`.
///
/// Spelled out from the model here rather than built with the crate's own
/// minter, so a handler that stored a value the member cannot hold cannot make
/// this test pass.
fn satisfies_aws_arn(value: &str) -> bool {
    if value.is_empty() || value.len() > 2048 {
        return false;
    }
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    let partition_well_formed = partition.strip_prefix("aws").is_some_and(|rest| {
        rest.chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-')
    });
    partition_well_formed
        && segment_is_well_formed(service, 1, 63)
        && segment_is_well_formed(region, 0, 63)
        && segment_is_well_formed(account, 0, 63)
        && !resource.is_empty()
        && resource.len() <= 1024
        && resource.starts_with(|c: char| c.is_ascii_alphanumeric())
        && resource
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '/' | '+' | '=' | ',' | '@' | '.' | '-'))
}

/// Whether one `[A-Za-z0-9_/.-]`-classed ARN segment is well formed: within the
/// published length, in that class, and — when the pattern requires a first
/// character at all (`minimum` of 1) — starting alphanumeric.
fn segment_is_well_formed(segment: &str, minimum: usize, maximum: usize) -> bool {
    segment.len() >= minimum
        && segment.len() <= maximum
        && (minimum == 0 || segment.starts_with(|c: char| c.is_ascii_alphanumeric()))
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-'))
}

/// Whether `value` can be carried as a search filter's value at all — from
/// `smithy_query(query="shape", shape="FilterValue")`: `@length(1, 256)` and
/// `^[A-Za-z0-9.,+:/_-]+$`. A licence this service minted that fails this could
/// never be passed to the `LicenseArn` filter the model publishes for finding
/// the agreement that granted it.
fn satisfies_filter_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | '+' | ':' | '/' | '_' | '-'))
}

/// The Region field of an ARN, or the empty string when `value` carries no
/// fourth field.
fn arn_region(value: &str) -> &str {
    value.split(':').nth(3).unwrap_or_default()
}

/// Create one agreement request and accept it under `authorization`, answering
/// what the acceptance answered.
async fn accept_signed_as(
    router: &MultiServiceRouter,
    proposal: &str,
    authorization: &str,
) -> Answer {
    let created = create(router, new_request(proposal, vec![term("eula.v1")])).await;
    assert_eq!(
        created.status, 200,
        "creating the agreement request for {proposal} must succeed, got {} with {:?}",
        created.status, created.body
    );
    let request_id = created.str_at("/agreementRequestId");
    call_as(
        router,
        "AcceptAgreementRequest",
        json!({"agreementRequestId": request_id}),
        authorization,
    )
    .await
}

/// The licences the entitlements of one agreement carry.
async fn licenses_of(router: &MultiServiceRouter, agreement_id: &str) -> Vec<String> {
    let answer = call(
        router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            entitlement["licenseArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The agreements `SearchAgreements` selects for one licence ARN.
async fn searched_for_license(router: &MultiServiceRouter, license: &str) -> Answer {
    call(
        router,
        "SearchAgreements",
        json!({
            "catalog": "AWSMarketplace",
            "filters": [
                agreement_type_filter(),
                {"name": "LicenseArn", "values": [license]},
            ],
        }),
    )
    .await
}

/// One row: the Region field a caller's credential scope carried, and the Region
/// the licence ARN the acceptance mints must therefore name.
struct RegionCase {
    case: String,
    signed_region: String,
    expected_region: String,
}

impl RegionCase {
    /// A Region field the ARN patterns admit, so the licence is minted in it.
    fn kept(case: &str, signed_region: &str) -> Self {
        Self {
            case: case.to_string(),
            signed_region: signed_region.to_string(),
            expected_region: signed_region.to_string(),
        }
    }

    /// A Region field that is no Region label, so the licence is minted in the
    /// default Region instead.
    fn defaulted(case: &str, signed_region: &str) -> Self {
        Self {
            case: case.to_string(),
            signed_region: signed_region.to_string(),
            expected_region: DEFAULT_REGION.to_string(),
        }
    }
}

/// The rows this rule generalises over: a credential scope's Region field
/// differing in its characters and in its length, from Regions of three
/// partitions through the published length bound to fields no Region label can
/// be.
fn region_cases() -> Vec<RegionCase> {
    let mut cases = vec![
        RegionCase::kept("the Region the endpoint is addressed in", DEFAULT_REGION),
        RegionCase::kept("another commercial Region", "us-west-2"),
        RegionCase::kept("a Region of another partition", "cn-north-1"),
        RegionCase::kept("a GovCloud Region", "us-gov-west-1"),
        RegionCase::kept("a single-character label", "a"),
        RegionCase::kept(
            "a label exactly at the published bound",
            LONGEST_REGION_LABEL,
        ),
        RegionCase::defaulted(
            "a label one character past the published bound",
            &format!("{LONGEST_REGION_LABEL}y"),
        ),
        RegionCase::defaulted("a hundred characters", &"a".repeat(100)),
        RegionCase::defaulted("a field in upper case", "US-EAST-1"),
        RegionCase::defaulted("a field carrying a space", "us east 1"),
        RegionCase::defaulted("a field carrying an underscore", "us_east_1"),
        RegionCase::defaulted("a host label appended after a dot", "us-east-1.example.com"),
    ];
    // One row per character outside the Region grammar `[a-z0-9-]` that the
    // `Authorization` header can carry in that field — `/` cannot, because the
    // credential scope itself is `/`-separated, and neither can `,`, which ends
    // the credential the scope is read from. Each would otherwise be
    // interpolated into the minted ARN verbatim.
    for forbidden in [' ', '+', '@', '=', '%', '*', '~', '#', '?', '!', '$', ':'] {
        cases.push(RegionCase::defaulted(
            &format!("a field carrying {forbidden:?}"),
            &format!("us-east-1{forbidden}x"),
        ));
    }
    cases
}

/// Accepting an agreement request mints each entitlement's licence in the Region
/// the caller signed for when that field is a Region label, and in the default
/// Region otherwise — so the stored `licenseArn` satisfies the modelled `AwsArn`
/// pattern for every row, and the `LicenseArn` filter can carry it back and
/// select the agreement that granted it.
#[tokio::test]
async fn a_minted_licence_names_a_region_the_modelled_arn_pattern_admits() {
    let router = emulator();

    for (index, row) in region_cases().into_iter().enumerate() {
        let case = &row.case;
        let proposal = format!("ap-region{index}");
        let accepted =
            accept_signed_as(&router, &proposal, &signed_for(&row.signed_region)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: accepting under this credential scope must succeed, got {} \
             with {:?}",
            accepted.status, accepted.body
        );
        let agreement = accepted.str_at("/agreementId");
        assert!(
            !agreement.is_empty(),
            "case {case:?}: the acceptance must answer an agreement id, got {:?}",
            accepted.body
        );

        let licenses = licenses_of(&router, &agreement).await;
        assert!(
            !licenses.is_empty(),
            "case {case:?}: the agreement must grant at least one entitlement, got \
             {licenses:?}"
        );

        for license in &licenses {
            assert!(
                satisfies_aws_arn(license),
                "case {case:?}: expected a licence ARN satisfying AwsArn's pattern, got \
                 {license:?}"
            );
            assert_eq!(
                arn_region(license),
                row.expected_region,
                "case {case:?}: expected the licence minted in Region {:?}, got {license:?}",
                row.expected_region
            );
            let expected_prefix = format!(
                "arn:aws:license-manager:{}:{EMULATOR_ACCOUNT}:license:l-",
                row.expected_region
            );
            assert!(
                license
                    .strip_prefix(&expected_prefix)
                    .is_some_and(|id| !id.is_empty()
                        && id.chars().all(|c| c.is_ascii_hexdigit())),
                "case {case:?}: the licence must be an AWS License Manager licence of this \
                 emulator's account in {expected_prefix:?}, got {license:?}"
            );
            assert!(
                satisfies_filter_value(license),
                "case {case:?}: a minted licence must be passable as a FilterValue, got \
                 {license:?}"
            );

            let found = searched_for_license(&router, license).await;
            assert_eq!(
                found.status, 200,
                "case {case:?}: searching for {license:?} must succeed, got {} with {:?}",
                found.status, found.body
            );
            let selected: Vec<String> = found.body["agreementViewSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| {
                    summary["agreementId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                selected,
                vec![agreement.clone()],
                "case {case:?}: the LicenseArn filter must select the agreement that granted \
                 {license:?}, got {:?}",
                found.body
            );
        }
    }
}

/// The licence an entitlement carries is minted ONCE: a caller signing for a
/// field that is no Region label gets the default Region, and the same stored
/// value is what every later report of that entitlement carries — a read never
/// re-mints it, whatever scope the reader signs with.
#[tokio::test]
async fn the_region_a_licence_names_is_fixed_by_the_acceptance() {
    let router = emulator();

    // A scope whose Region field is text the ARN patterns forbid, so the
    // acceptance falls back to the default Region.
    let accepted = accept_signed_as(&router, "ap-regionfixed", &signed_for("us east 1")).await;
    assert_eq!(
        accepted.status, 200,
        "accepting under a malformed credential scope must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement = accepted.str_at("/agreementId");
    let minted = licenses_of(&router, &agreement).await;
    assert!(
        minted.iter().all(|license| satisfies_aws_arn(license)),
        "every minted licence must satisfy AwsArn's pattern, got {minted:?}"
    );

    // The same entitlements read under a scope signing for a different, perfectly
    // well-formed Region report the licences the ACCEPTANCE minted.
    let reread = call_as(
        &router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement}),
        &signed_for("eu-central-1"),
    )
    .await;
    assert_eq!(
        reread.status, 200,
        "re-reading the entitlements must succeed, got {} with {:?}",
        reread.status, reread.body
    );
    let reported: Vec<String> = reread.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            entitlement["licenseArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        reported, minted,
        "a read reports the licences the acceptance minted and mints none of its own, got \
         {reported:?}"
    );
    for license in &reported {
        assert_eq!(
            arn_region(license),
            DEFAULT_REGION,
            "the Region a licence names is the one its acceptance minted it in, got \
             {license:?}"
        );
    }
}
