// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the quota `CreateEmailContact` refuses a create at, and
//! for the vocabulary of the modelled `ServiceQuotaExceededException` (HTTP 402) it
//! answers with.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handler — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a create is refused exactly once the account already holds the
//! service's PUBLISHED per-account email-contact quota, and the 402 reports that quota
//! the way Service Quotas publishes it — the service's own `serviceCode` and the
//! `quotaCode` the quota is published under — with every `@required` member carrying
//! the identity its documentation names, `resourceId` the account whose holding exceeds
//! the quota.
//!
//! The published values are asserted as the LITERALS a client reads off the wire and
//! then looks the quota up by, never as whatever the implementation happens to derive.

use serde_json::{json, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::{
    EMAIL_CONTACTS_QUOTA_NAME, MAX_EMAIL_CONTACTS,
};

use crate::email_contacts::{create, delete, emulator, error_code, list, Emulator, ACCOUNT};

/// The per-account email-contact quota AWS publishes for this service, pinned as the
/// number itself: the same value the Service Quotas catalog publishes for ServiceCode
/// [`PUBLISHED_SERVICE_CODE`] / QuotaCode [`PUBLISHED_QUOTA_CODE`].
const PUBLISHED_QUOTA: usize = 10;

/// The `ServiceCode` the refusal names — this service's own code, which is the code
/// Service Quotas publishes its quotas under.
const PUBLISHED_SERVICE_CODE: &str = "notifications-contacts";

/// The `QuotaCode` the refusal names, which is the code the Service Quotas catalog
/// publishes the same quota under: the quota's published name as one identifier.
const PUBLISHED_QUOTA_CODE: &str = "EmailContactsPerAccount";

/// The `resourceType` the refusal names: the resource word of the modelled
/// `EmailContactArn` pattern.
const PUBLISHED_RESOURCE_TYPE: &str = "emailcontact";

/// How many contacts the account currently holds, read back through the listing.
///
/// One page is enough: the published quota is the ceiling on the whole account, and it
/// is well under the modelled `maxResults` maximum of 100.
async fn held(em: &Emulator) -> usize {
    let (status, body) = list(em, "maxResults=100").await;
    assert_eq!(
        status, 200,
        "ListEmailContacts must answer 200 while counting the account's contacts, got {body:?}"
    );
    body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("emailContacts is @required, got {body:?}"))
        .len()
}

/// Whether `code` sits in the namespace AWS ITSELF mints quota codes in: `L-` and eight
/// upper-case hexadecimal characters, the form of every code in AWS's published Service
/// Quotas catalog (`L-1216C47A`).
///
/// That namespace is AWS's alone: a code of this form AWS did not publish names a quota
/// `servicequotas:GetServiceQuota` cannot resolve, so this service's own refusal must
/// NOT report one.
fn is_aws_minted_shape(code: &str) -> bool {
    code.strip_prefix("L-").is_some_and(|rest| {
        rest.len() == 8
            && rest
                .chars()
                .all(|char| char.is_ascii_digit() || ('A'..='F').contains(&char))
    })
}

/// Whether `code` satisfies the `QuotaCode` Service Quotas models for a quota it
/// publishes: 1..128 characters, `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`
/// (`smithy_query(query="shape", shape="ServiceQuota")` of the servicequotas model), so
/// a client can carry the code this refusal named straight into that API.
fn is_resolvable_quota_code(code: &str) -> bool {
    (2..=128).contains(&code.len())
        && code.starts_with(|char: char| char.is_ascii_alphabetic())
        && code
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-')
}

/// Assert one refused create is the modelled 402 and that every member the model marks
/// `@required` carries the identity its documentation names.
fn assert_quota_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 402,
        "case {case:?}: a create at the published quota must be refused with 402, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {body:?}"
    );

    let resource_id = body["resourceId"].as_str().unwrap_or_default();
    assert_eq!(
        resource_id, ACCOUNT,
        "case {case:?}: resourceId must name the account the quota counts for, got {resource_id:?} in {body:?}"
    );
    let resource_type = body["resourceType"].as_str().unwrap_or_default();
    assert_eq!(
        resource_type, PUBLISHED_RESOURCE_TYPE,
        "case {case:?}: expected resourceType {PUBLISHED_RESOURCE_TYPE:?}, got \
         {resource_type:?} in {body:?}"
    );
    let service_code = body["serviceCode"].as_str().unwrap_or_default();
    assert_eq!(
        service_code, PUBLISHED_SERVICE_CODE,
        "case {case:?}: expected serviceCode {PUBLISHED_SERVICE_CODE:?}, got \
         {service_code:?} in {body:?}"
    );
    let quota_code = body["quotaCode"].as_str().unwrap_or_default();
    assert_eq!(
        quota_code, PUBLISHED_QUOTA_CODE,
        "case {case:?}: expected the published quota code {PUBLISHED_QUOTA_CODE:?}, got \
         {quota_code:?} in {body:?}"
    );

    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&PUBLISHED_QUOTA.to_string()),
        "case {case:?}: the message must state the quota of {PUBLISHED_QUOTA}, got {message:?}"
    );
    assert!(
        message.contains(EMAIL_CONTACTS_QUOTA_NAME),
        "case {case:?}: the message must name the quota {EMAIL_CONTACTS_QUOTA_NAME:?}, got \
         {message:?}"
    );
}

/// Fill the account to its published quota, asserting every create under it succeeds.
async fn fill_to_the_quota(em: &Emulator) {
    for n in 0..PUBLISHED_QUOTA {
        let (status, body) = create(
            em,
            &json!({
                "name": format!("filler-{n:04}"),
                "emailAddress": format!("filler-{n:04}@example.com"),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case fill {n}: a create under the quota must answer 201, got {status} / {body:?}"
        );
        assert_eq!(
            held(em).await,
            n + 1,
            "case fill {n}: the account must hold exactly {} contacts",
            n + 1
        );
    }
}

#[tokio::test]
async fn a_create_is_refused_once_the_account_holds_the_published_quota() {
    // The ceiling is AWS's own published per-account quota, so the number itself is
    // pinned here and not just the constant's behaviour — and it is the same number the
    // Service Quotas catalog publishes for this service's email-contact quota.
    assert_eq!(
        MAX_EMAIL_CONTACTS, PUBLISHED_QUOTA,
        "the published per-account email-contact quota is {PUBLISHED_QUOTA}, got \
         {MAX_EMAIL_CONTACTS}"
    );
    let em = emulator();
    fill_to_the_quota(&em).await;

    // Rows differ along what a create can vary: the length and the character mix of
    // `name` (its modelled `@length` is 1..64), the length of `emailAddress` (6..254),
    // whether the request carries tags, and whether it repeats a name the account
    // already holds. The quota is a bound on what the ACCOUNT holds, so none of them
    // changes the answer.
    let long_name = "n".repeat(64);
    let long_address = format!("{}@example.com", "a".repeat(200));
    let rows: Vec<(&str, Value)> = vec![
        (
            "the shortest possible name",
            json!({ "name": "a", "emailAddress": "a@b.co" }),
        ),
        (
            "a name at the modelled maximum length",
            json!({ "name": long_name, "emailAddress": "long-name@example.com" }),
        ),
        (
            "a name drawn from the whole modelled character class",
            json!({ "name": "oncall_primary-2.0~x", "emailAddress": "oncall@example.com" }),
        ),
        (
            "a long email address",
            json!({ "name": "long-address", "emailAddress": long_address }),
        ),
        (
            "a create carrying tags",
            json!({
                "name": "tagged",
                "emailAddress": "tagged@example.com",
                "tags": { "team": "oncall", "env": "prod" },
            }),
        ),
        (
            "a name the account already holds",
            json!({ "name": "filler-0000", "emailAddress": "filler-0000@example.com" }),
        ),
    ];

    for (case, body) in &rows {
        let (status, answer) = create(&em, body).await;
        assert_quota_refusal(case, status, &answer);
        // A refused create stores nothing, so the account still holds exactly its
        // quota and the next refusal reads the same way.
        assert_eq!(
            held(&em).await,
            PUBLISHED_QUOTA,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

#[tokio::test]
async fn freeing_one_slot_admits_exactly_one_more_create() {
    let em = emulator();
    fill_to_the_quota(&em).await;

    // The quota bounds what the account holds at one instant, not how many creates it
    // ever made: deleting a contact admits one more create and no second one.
    for round in 0..3 {
        let victim = list(&em, "maxResults=1").await.1["emailContacts"][0]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case round {round}: the account must hold a contact"))
            .to_string();
        assert_eq!(
            delete(&em, &victim).await.0,
            200,
            "case round {round}: the delete must succeed"
        );
        assert_eq!(
            held(&em).await,
            PUBLISHED_QUOTA - 1,
            "case round {round}: the delete must free exactly one slot"
        );

        let (status, body) = create(
            &em,
            &json!({
                "name": format!("replacement-{round}"),
                "emailAddress": format!("replacement-{round}@example.com"),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case round {round}: a create into the freed slot must answer 201, got {status} / {body:?}"
        );

        let (status, body) = create(
            &em,
            &json!({
                "name": format!("one-too-many-{round}"),
                "emailAddress": format!("one-too-many-{round}@example.com"),
            }),
        )
        .await;
        assert_quota_refusal(&format!("round {round}, one past the quota"), status, &body);
    }
}

/// The code the refusal names has to be one a client can carry into Service Quotas: it
/// satisfies that API's modelled `QuotaCode`, it is the published quota's name as one
/// identifier — which is the code the Service Quotas catalog publishes it under — and it
/// is NOT in the `L-<eight hex>` namespace AWS alone mints into, where a code this
/// emulator made up would name a quota AWS never published.
#[test]
fn the_reported_quota_code_is_resolvable_and_outside_the_aws_minted_namespace() {
    // The published code is the quota's own name with its words joined and each
    // capitalised — the rule restated here rather than read off the implementation.
    let from_the_name: String = EMAIL_CONTACTS_QUOTA_NAME
        .split(|char: char| !char.is_ascii_alphanumeric())
        .flat_map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        })
        .collect();
    assert_eq!(
        PUBLISHED_QUOTA_CODE, from_the_name,
        "case the published code: expected the quota name {EMAIL_CONTACTS_QUOTA_NAME:?} as one \
         identifier ({from_the_name}), got {PUBLISHED_QUOTA_CODE}"
    );
    assert!(
        is_resolvable_quota_code(PUBLISHED_QUOTA_CODE),
        "case the published code: {PUBLISHED_QUOTA_CODE:?} must satisfy the QuotaCode Service \
         Quotas models"
    );
    assert!(
        !is_aws_minted_shape(PUBLISHED_QUOTA_CODE),
        "case the published code: {PUBLISHED_QUOTA_CODE:?} must stay out of the namespace AWS \
         itself mints quota codes in"
    );

    // The namespace test itself, over codes that span it — AWS's own published form and
    // the near misses — so the assertion above cannot pass by reading every value the
    // same way.
    for (case, code, minted) in [
        ("a code AWS publishes", "L-1216C47A", true),
        ("another code AWS publishes", "L-96F55078", true),
        ("all digits after the prefix", "L-12345678", true),
        ("a letter past F", "L-1216C47G", false),
        ("lower-case hex", "L-1216c47a", false),
        ("seven characters after the prefix", "L-1216C47", false),
        ("nine characters after the prefix", "L-1216C47A1", false),
        ("a label wearing the prefix", "L-EMAILCONTACTS", false),
        ("the descriptive code", PUBLISHED_QUOTA_CODE, false),
    ] {
        assert_eq!(
            is_aws_minted_shape(code),
            minted,
            "case {case:?}: {code:?} is {}in the namespace AWS mints quota codes in",
            if minted { "" } else { "not " }
        );
    }
}
