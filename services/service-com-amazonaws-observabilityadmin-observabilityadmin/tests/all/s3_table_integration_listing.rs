// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for listing and retiring S3 Table integrations.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes. The create/read side of the slice is
//! covered by [`crate::s3_table_integrations`]; what is asserted here is the pair of
//! operations that walks the collection and the one that retires a member of it.
//!
//! The rules under test:
//!
//! 1. a listing reports a summary of every integration the store holds in the region
//!    the request was signed for, and of no integration of another region: the ARN
//!    that is the only handle on it, and that record's own status;
//! 2. `MaxResults` bounds ONE page and the `NextToken` beside it resumes exactly where
//!    that page stopped, so the pages of one walk cover the region's integrations once
//!    each, in the store's order; a page with nothing left beyond it carries no token;
//! 3. a `NextToken` carrying anything this operation could not have issued — an id that is
//!    not of the form this service mints one in — is refused with the modelled 400 rather
//!    than silently answering a page, while a well-formed id the store no longer holds
//!    stays a POSITION: the walk resumes strictly after it;
//! 4. a listing is a pure read: it leaves the store exactly as it was;
//! 5. `DeleteS3TableIntegration` retires the one integration its `Arn` names in the
//!    caller's region, after which neither a read nor a listing reports it, and the
//!    other integrations are untouched;
//! 6. an `Arn` that names no integration in the caller's region — a retired one, one
//!    of another region, one of another account or partition — is refused with the
//!    modelled 400 and retires nothing.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    s3_table_integration_arn, ComAmazonawsObservabilityadminObservabilityadminState,
    INTEGRATION_STATUS_ACTIVE,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "only the integrations of the caller's own region" can be shown
/// to be per-region rather than store-wide.
const OTHER_REGION: &str = "eu-west-1";

/// A role ARN every create below carries: the member's own `^arn:aws…` pattern is
/// what the create is held to, and this value satisfies it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/telemetry-s3-tables";

/// Create one integration in `region` and answer the ARN it was stored under.
async fn create(emulator: &Emulator, region: &str, case: &str) -> String {
    let (status, response) = call(
        emulator,
        region,
        "/CreateS3TableIntegration",
        &json!({
            "Encryption": { "SseAlgorithm": "AES256" },
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create in {region} to answer 200, got {status} {response:?}"
    );
    response
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a create answers an Arn, got {response:?}"))
        .to_string()
}

/// One listing page: its summaries and the `NextToken` it carried.
async fn list(
    emulator: &Emulator,
    region: &str,
    request: &Value,
    case: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let (status, response) = call(emulator, region, "/ListS3TableIntegrations", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to answer 200, got {status} {response:?}"
    );
    let summaries = response
        .get("IntegrationSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing answers IntegrationSummaries, got {response:?}")
        })
        .iter()
        .map(|summary| {
            (
                summary
                    .get("Arn")
                    .and_then(Value::as_str)
                    .unwrap_or("<no Arn>")
                    .to_string(),
                summary
                    .get("Status")
                    .and_then(Value::as_str)
                    .unwrap_or("<no Status>")
                    .to_string(),
            )
        })
        .collect();
    let next = response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (summaries, next)
}

/// Every integration `region` reports, walked page by page with `page_size` (or in one
/// call when it is `None`), with the number of pages the walk took.
async fn walk(
    emulator: &Emulator,
    region: &str,
    page_size: Option<usize>,
    case: &str,
) -> (Vec<(String, String)>, usize) {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (page, next) = list(emulator, region, &request, case).await;
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: expected at most {size} summaries per page, got {}",
                page.len()
            );
        }
        if next.is_some() {
            assert_eq!(
                page.len(),
                page_size.unwrap_or(page.len()),
                "case {case:?}: a NextToken may only follow a page that filled the limit, got \
                 {page:?}"
            );
        }
        seen.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages < 64,
            "case {case:?}: expected the walk to terminate, got {pages} pages"
        );
    }
    (seen, pages)
}

/// How many integration records the shared store holds.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
        .lock()
        .s3_table_integrations
        .len()
}

async fn get(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetS3TableIntegration",
        &json!({ "Arn": arn }),
    )
    .await
}

async fn delete(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/DeleteS3TableIntegration",
        &json!({ "Arn": arn }),
    )
    .await
}

/// A listing reports every integration of the caller's own region and none of another
/// region's, whatever the number of integrations each region holds — and it changes
/// nothing a later caller sees.
#[tokio::test]
async fn a_listing_reports_this_regions_integrations_and_changes_nothing() {
    // The rows differ along what the rule generalises over: how many integrations the
    // caller's region holds (none, one, several) and how many another region holds
    // beside them.
    for (case, home, other) in [
        ("no integration at all", 0usize, 0usize),
        ("one integration, nothing elsewhere", 1, 0),
        ("one integration beside another region's", 1, 2),
        ("several integrations beside another region's", 4, 3),
        ("nothing here, several elsewhere", 0, 2),
    ] {
        let emulator = emulator();
        let mut home_arns: Vec<String> = Vec::new();
        for _ in 0..home {
            home_arns.push(create(&emulator, HOME_REGION, case).await);
        }
        let mut other_arns: Vec<String> = Vec::new();
        for _ in 0..other {
            other_arns.push(create(&emulator, OTHER_REGION, case).await);
        }

        let (summaries, pages) = walk(&emulator, HOME_REGION, None, case).await;
        assert_eq!(
            pages, 1,
            "case {case:?}: expected one unpaged call to answer every integration, got {pages} \
             pages"
        );
        let mut listed: Vec<String> = summaries.iter().map(|(arn, _)| arn.clone()).collect();
        let mut expected = home_arns.clone();
        listed.sort();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing to report exactly {HOME_REGION}'s integrations, \
             got {summaries:?}"
        );
        for (arn, status) in &summaries {
            assert_eq!(
                status, INTEGRATION_STATUS_ACTIVE,
                "case {case:?}: expected {arn} to be reported {INTEGRATION_STATUS_ACTIVE}, got \
                 {status:?}"
            );
            // The summary and the read of the same integration cannot disagree.
            let (status_code, read) = get(&emulator, HOME_REGION, arn).await;
            assert_eq!(
                (status_code, read.get("Status").and_then(Value::as_str)),
                (200, Some(status.as_str())),
                "case {case:?}: expected the read of {arn} to report the status the listing did, \
                 got {status_code} {read:?}"
            );
        }

        // The other region's listing is its own, and neither listing moved a record.
        let (elsewhere, _) = walk(&emulator, OTHER_REGION, None, case).await;
        let mut elsewhere_arns: Vec<String> = elsewhere.iter().map(|(arn, _)| arn.clone()).collect();
        let mut expected_elsewhere = other_arns.clone();
        elsewhere_arns.sort();
        expected_elsewhere.sort();
        assert_eq!(
            elsewhere_arns, expected_elsewhere,
            "case {case:?}: expected {OTHER_REGION}'s listing to report its own integrations, got \
             {elsewhere:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            home + other,
            "case {case:?}: expected the listings to leave all {} records alone, got {}",
            home + other,
            stored_count(&emulator)
        );
    }
}

/// `MaxResults` bounds one page and the `NextToken` beside it resumes exactly where
/// that page stopped: for every page size, the walk reports each integration once.
#[tokio::test]
async fn max_results_bounds_a_page_and_next_token_resumes_after_it() {
    // The rows span the page size against the number of integrations: a size under,
    // equal to and over the count, including the smallest size the model admits and
    // one case where the count is an exact multiple of the size.
    let count = 6usize;
    let emulator = emulator();
    let mut created: Vec<String> = Vec::new();
    for _ in 0..count {
        created.push(create(&emulator, HOME_REGION, "paged walk").await);
    }
    let (unpaged, _) = walk(&emulator, HOME_REGION, None, "paged walk").await;
    let order: Vec<String> = unpaged.iter().map(|(arn, _)| arn.clone()).collect();

    for size in [1usize, 2, 3, 5, count, count + 1, 100] {
        let case = format!("MaxResults {size} over {count} integrations");
        let (seen, pages) = walk(&emulator, HOME_REGION, Some(size), &case).await;
        let walked: Vec<String> = seen.iter().map(|(arn, _)| arn.clone()).collect();
        assert_eq!(
            walked, order,
            "case {case:?}: expected the paged walk to report the unpaged order once each, got \
             {walked:?}"
        );
        let expected_pages = count / size + usize::from(!count.is_multiple_of(size));
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case:?}: expected {} pages, got {pages}",
            expected_pages.max(1)
        );
    }
}

/// A `NextToken` this operation did not issue is refused with the modelled 400, and a
/// token it did issue is served.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    let emulator = emulator();
    for _ in 0..3 {
        create(&emulator, HOME_REGION, "forged token").await;
    }
    let (_, issued) = list(
        &emulator,
        HOME_REGION,
        &json!({ "MaxResults": 1 }),
        "forged token",
    )
    .await;
    let issued = issued.expect("a page that stopped short of the store issues a NextToken");

    // The rows differ along the ways a token can fail to carry an id this service mints:
    // not hex at all, hex that is not UTF-8, hex of free text, hex of an ARN rather than
    // the id inside it, a UUID with a group of the wrong width, a UUID in upper case, and
    // the issued token with one hex digit dropped.
    let rows: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex that is not UTF-8", "ff".to_string()),
        (
            "hex of a value no minted id can take",
            hex::encode("Not An Id"),
        ),
        (
            "hex of an ARN rather than an id",
            hex::encode(s3_table_integration_arn(HOME_REGION, "abc")),
        ),
        (
            "hex of a UUID with a group of the wrong width",
            hex::encode("ffffffff-ffff-4fff-8fff-fffffffffff"),
        ),
        (
            "hex of a UUID in upper case",
            hex::encode("FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFF"),
        ),
        (
            "the issued token with one hex digit dropped",
            issued[..issued.len() - 1].to_string(),
        ),
    ];
    for (case, token) in rows {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ListS3TableIntegrations",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // The token the operation did issue still resumes the walk.
    let (page, _) = list(
        &emulator,
        HOME_REGION,
        &json!({ "MaxResults": 1, "NextToken": issued }),
        "the issued token",
    )
    .await;
    assert_eq!(
        page.len(),
        1,
        "case {:?}: expected the issued token to resume the walk, got {page:?}",
        "the issued token"
    );

    // And a token of the form this operation mints whose id the store does not hold is a
    // POSITION, not a forgery: the page resumes strictly after it. This id sorts past every
    // stored one, so nothing is left beyond it and the page is empty and final.
    let case = "a well-formed id the store does not hold";
    let (page, next) = list(
        &emulator,
        HOME_REGION,
        &json!({ "NextToken": hex::encode("ffffffff-ffff-4fff-8fff-ffffffffffff") }),
        case,
    )
    .await;
    assert!(
        page.is_empty() && next.is_none(),
        "case {case:?}: expected the page after the last position to be empty and final, got \
         {page:?} with {next:?}"
    );
}

/// A delete retires the one integration its `Arn` names, leaves every other record
/// alone, and is afterwards invisible to both a read and a listing.
#[tokio::test]
async fn a_delete_retires_the_integration_and_leaves_the_rest() {
    // The rows differ along which member of the collection is retired: the only one,
    // the first of several, the last of several, and one in the middle.
    for (case, count, index) in [
        ("the only integration", 1usize, 0usize),
        ("the first of four", 4, 0),
        ("one in the middle of four", 4, 2),
        ("the last of four", 4, 3),
    ] {
        let emulator = emulator();
        for _ in 0..count {
            create(&emulator, HOME_REGION, case).await;
        }
        // A listing is what fixes the store's order, so the row's index names a real
        // position in it rather than a creation order the service never promised.
        let (before, _) = walk(&emulator, HOME_REGION, None, case).await;
        let doomed = before[index].0.clone();

        let (status, response) = delete(&emulator, HOME_REGION, &doomed).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the delete of {doomed} to answer the modelled 204, got \
             {status} {response:?}"
        );

        // Neither a read nor a listing reports the retired integration.
        let (status, read) = get(&emulator, HOME_REGION, &doomed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the read of the retired {doomed} to answer 404, got {status} \
             {read:?}"
        );
        let (after, _) = walk(&emulator, HOME_REGION, None, case).await;
        let expected: Vec<(String, String)> = before
            .iter()
            .filter(|(arn, _)| *arn != doomed)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected the listing to report the surviving integrations in the same \
             order, got {after:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            count - 1,
            "case {case:?}: expected {} stored records after the delete, got {}",
            count - 1,
            stored_count(&emulator)
        );
        // Every survivor is still readable under its own ARN.
        for (arn, _) in &after {
            let (status, read) = get(&emulator, HOME_REGION, arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the surviving {arn} to still read back, got {status} \
                 {read:?}"
            );
        }
    }
}

/// An `Arn` that names no integration in the caller's own region is refused with the
/// modelled 400 and retires nothing.
#[tokio::test]
async fn a_delete_of_an_arn_that_names_nothing_is_refused_and_retires_nothing() {
    let emulator = emulator();
    let home = create(&emulator, HOME_REGION, "unresolvable delete").await;
    let elsewhere = create(&emulator, OTHER_REGION, "unresolvable delete").await;
    let id = home.rsplit('/').next().unwrap_or_default().to_string();

    // The rows differ along which field of the ARN fails to resolve, and along the
    // region the request is signed for: another region's integration, this one's ARN
    // read from another region, another account, another partition, another resource
    // type, and an id nothing was created under.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "another region's integration",
            elsewhere.clone(),
            HOME_REGION,
        ),
        ("this integration from another region", home.clone(), OTHER_REGION),
        (
            "an ARN naming the id in another region",
            s3_table_integration_arn(OTHER_REGION, &id),
            HOME_REGION,
        ),
        (
            "an ARN of another account",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:000000000001:s3-table-integration/{id}"),
            HOME_REGION,
        ),
        (
            "an ARN of another partition",
            format!(
                "arn:aws-us-gov:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3-table-integration/{id}"
            ),
            HOME_REGION,
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/{id}"),
            HOME_REGION,
        ),
        (
            "an id nothing was created under",
            s3_table_integration_arn(HOME_REGION, "ffffffff-ffff-4fff-8fff-ffffffffffff"),
            HOME_REGION,
        ),
    ];
    for (case, arn, region) in rows {
        let (status, response) = delete(&emulator, region, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({arn}): expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            2,
            "case {case:?}: expected the refused delete to retire nothing, got {} records",
            stored_count(&emulator)
        );
    }

    // A second delete of a retired integration is the same refusal: the handle stops
    // resolving when the record stops existing.
    let (status, _) = delete(&emulator, HOME_REGION, &home).await;
    assert_eq!(
        status, 204,
        "case {:?}: expected the first delete to answer 204, got {status}",
        "delete twice"
    );
    let (status, response) = delete(&emulator, HOME_REGION, &home).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected the second delete to answer 400, got {status} {response:?}",
        "delete twice"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {:?}: expected ValidationException, got {response:?}",
        "delete twice"
    );
}
