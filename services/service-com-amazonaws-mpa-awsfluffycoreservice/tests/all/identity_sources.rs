// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity-source operations: `CreateIdentitySource` and
//! `ListIdentitySources`.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! (`POST /identity-sources`, `POST /identity-sources/?List` with `MaxResults` and
//! `NextToken` bound to the query string) dispatched through the `MultiServiceRouter`
//! the harness in `approval_team_lifecycle.rs` mounts, so the route, the URI bindings,
//! the JSON envelope and the HTTP status are all exercised.
//!
//! The rule under test: `ListIdentitySources` reports the identity sources the store
//! holds, in creation order, in pages of at most `MaxResults`, every member computed
//! from the stored record — so the tables below span the class each rule generalises
//! over (the FORM of the IAM Identity Center instance ARN, the instance's region, the
//! number of tags, the number of stored sources, the page size) rather than one recorded
//! request. Where a table names a published bound (`MaxResults` 1..20, `IdentitySources`
//! `length(max: 20)`, `IdcInstanceArn`'s pattern, 50 tags per resource) a comment names
//! the modelled member or the project constant it comes from.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness, REGION};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;

/// Slack over [`state::IDENTITY_SOURCE_ACTIVATION_DELAY`] before the clock-owed
/// activation is expected to have been materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// One row of the create/list table: what a request names, and therefore what the listed
/// entry must be computed from.
struct SourceCase {
    case: String,
    instance_arn: String,
    /// `IamIdentityCenter$Region`: the INSTANCE's region, which need not be the region
    /// the request is signed for.
    instance_region: String,
    tags: Vec<(String, String)>,
}

/// An IAM Identity Center instance ARN that satisfies `IdcInstanceArn`'s modelled
/// pattern `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` and its
/// `length(min: 20, max: 2048)`: the partition is `.+`, the `sso` prefix of the instance
/// id is optional, and the id's last 16 characters are `[a-zA-Z0-9-.]`.
fn instance_arn(partition: &str, prefix: &str, id: &str) -> String {
    assert_eq!(
        id.len(),
        16,
        "IdcInstanceArn's pattern fixes the instance id at 16 characters, {id:?} has {}",
        id.len()
    );
    format!("arn:{partition}:sso:::instance/{prefix}ins-{id}")
}

/// The create/list table: rows differ along the instance-ARN form (partition, the
/// optional `sso` prefix, the character classes the id allows), the instance's region and
/// the number of tags (none, one, several, and the maximum a resource carries).
fn source_cases() -> Vec<SourceCase> {
    vec![
        SourceCase {
            case: "ins- form, instance in the caller's region, no tags".to_string(),
            instance_arn: instance_arn("aws", "", "1234567890abcdef"),
            instance_region: REGION.to_string(),
            tags: Vec::new(),
        },
        SourceCase {
            case: "ssoins- form, instance in another region, one tag".to_string(),
            instance_arn: instance_arn("aws", "sso", "abcdef1234567890"),
            instance_region: "us-east-1".to_string(),
            tags: vec![("env".to_string(), "test".to_string())],
        },
        SourceCase {
            case: "another partition, dots and dashes in the id, three tags".to_string(),
            instance_arn: instance_arn("aws-us-gov", "", "a.b-c.d-12345678"),
            instance_region: "us-gov-west-1".to_string(),
            tags: vec![
                ("owner".to_string(), "platform".to_string()),
                ("stage".to_string(), String::new()),
                ("team".to_string(), "mpa".to_string()),
            ],
        },
        SourceCase {
            case: "mixed-case id, the maximum tags a resource carries".to_string(),
            instance_arn: instance_arn("aws", "sso", "AbCdEf0123456789"),
            instance_region: "eu-west-1".to_string(),
            // `state::MAX_TAGS_PER_RESOURCE` tags: the ceiling this service enforces,
            // corroborated by `UntagResource$TagKeys`' `length(min: 0, max: 50)`.
            tags: (1..=state::MAX_TAGS_PER_RESOURCE)
                .map(|n| (format!("key{n:02}"), format!("value{n:02}")))
                .collect(),
        },
    ]
}

/// The `CreateIdentitySource` body for a row, with an optional `ClientToken`.
fn create_body(case: &SourceCase, client_token: Option<&str>) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": case.instance_arn,
                "Region": case.instance_region,
            }
        }
    });
    if !case.tags.is_empty() {
        body["Tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect(),
        );
    }
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// `POST /identity-sources`.
async fn create(harness: &Harness, body: &Value) -> (u16, Value) {
    harness
        .call("POST", "/identity-sources", Some(body.clone()))
        .await
}

/// `POST /identity-sources/?List`, with `query` appended to the `?List` marker.
async fn list(harness: &Harness, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        "/identity-sources/?List".to_string()
    } else {
        format!("/identity-sources/?List&{query}")
    };
    harness.call("POST", &uri, None).await
}

/// The entries one page carries.
fn entries(body: &Value) -> Vec<Value> {
    body["IdentitySources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARN this service must mint for the `n`-th identity source created in `REGION`.
fn expected_arn(n: u64) -> String {
    format!(
        "arn:aws:mpa:{REGION}:{}:identity-source/ids-{n:012}",
        engine::emulator_account_id()
    )
}

/// One listed entry, against the request it came from and the record the store holds.
fn assert_entry(case: &str, listed: &Value, arn: &str, request: &SourceCase, harness: &Harness) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    let stored = guard
        .identity_source(arn)
        .unwrap_or_else(|| panic!("case {case:?}: nothing is stored under {arn:?}"));

    assert_eq!(
        text(listed, "IdentitySourceArn"),
        stored.arn,
        "case {case:?}: expected IdentitySourceArn {:?}, got {listed:?}",
        stored.arn
    );
    assert_eq!(
        text(listed, "IdentitySourceType"),
        stored.source_type.as_wire(),
        "case {case:?}: expected IdentitySourceType {:?}, got {listed:?}",
        stored.source_type.as_wire()
    );
    assert!(
        model::IDENTITY_SOURCE_TYPE_VALUES.contains(&text(listed, "IdentitySourceType").as_str()),
        "case {case:?}: IdentitySourceType {:?} is not a token the model declares ({:?})",
        text(listed, "IdentitySourceType"),
        model::IDENTITY_SOURCE_TYPE_VALUES
    );

    let status = text(listed, "Status");
    assert!(
        model::IDENTITY_SOURCE_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not a token the model declares ({:?})",
        model::IDENTITY_SOURCE_STATUS_VALUES
    );
    assert!(
        status == state::IdentitySourceStatus::Creating.as_wire()
            || status == state::IdentitySourceStatus::Active.as_wire(),
        "case {case:?}: a source this service created is CREATING then ACTIVE, got {status:?}"
    );
    assert_eq!(
        status,
        stored.status.as_wire(),
        "case {case:?}: the reported Status must be the stored one ({:?}), got {status:?}",
        stored.status.as_wire()
    );

    // The parameters are the IAM Identity Center instance the request named, reported
    // under the union member its type selects.
    let parameters = &listed["IdentitySourceParameters"]["IamIdentityCenter"];
    assert_eq!(
        text(parameters, "InstanceArn"),
        request.instance_arn,
        "case {case:?}: expected InstanceArn {:?}, got {listed:?}",
        request.instance_arn
    );
    assert_eq!(
        text(parameters, "Region"),
        request.instance_region,
        "case {case:?}: expected Region {:?}, got {listed:?}",
        request.instance_region
    );

    // `CreationTime` is the stored instant, which does not move between reads.
    let reported = instant(listed, "CreationTime");
    let recorded = stored
        .creation_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a creation time after the epoch")
        .as_secs_f64();
    assert!(
        (reported - recorded).abs() < 1.0,
        "case {case:?}: expected CreationTime {recorded}, got {reported}"
    );
}

/// Every created identity source is reported by `ListIdentitySources`, in creation order,
/// with every member computed from the record the create wrote.
#[tokio::test]
async fn every_created_identity_source_is_reported_out_of_state() {
    // Creating an identity source is restricted to the organization's management
    // account, so these tables run as it; the gate itself is tested below.
    let harness = Harness::new();

    let cases = source_cases();
    let mut arns: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (status, created) = create(&harness, &create_body(case, None)).await;
        assert_eq!(
            status, 200,
            "case {:?}: create returned {status} with {created:?}",
            case.case
        );
        let arn = text(&created, "IdentitySourceArn");
        assert_eq!(
            arn,
            expected_arn(index as u64 + 1),
            "case {:?}: wrong minted ARN in {created:?}",
            case.case
        );
        assert_eq!(
            text(&created, "IdentitySourceType"),
            state::IdentitySourceType::IamIdentityCenter.as_wire(),
            "case {:?}: wrong IdentitySourceType in {created:?}",
            case.case
        );
        arns.push(arn);
    }

    let (status, body) = list(&harness, "").await;
    assert_eq!(status, 200, "ListIdentitySources returned {body:?}");
    let listed = entries(&body);
    assert_eq!(
        listed.len(),
        cases.len(),
        "every created source must be listed: {body:?}"
    );
    for ((entry, arn), case) in listed.iter().zip(arns.iter()).zip(cases.iter()) {
        assert_entry(&case.case, entry, arn, case, &harness);
    }
    assert!(
        body.get("NextToken").is_none(),
        "a complete page carries no NextToken: {body:?}"
    );

    // Each row's tags were recorded against the source as a taggable resource, so
    // `ListTagsForResource` reads them back.
    for (arn, case) in arns.iter().zip(cases.iter()) {
        let (status, tags) = harness
            .call("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource returned {status} with {tags:?}",
            case.case
        );
        let reported = tags["Tags"].as_object().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            case.tags.len(),
            "case {:?}: expected {} tags, got {tags:?}",
            case.case,
            case.tags.len()
        );
        for (key, value) in &case.tags {
            assert_eq!(
                reported.get(key).and_then(Value::as_str),
                Some(value.as_str()),
                "case {:?}: expected tag {key:?}={value:?}, got {tags:?}",
                case.case
            );
        }
    }

    // A read changes nothing a later caller sees. The clock-owed activation is settled
    // first, so what is compared is two reads of the same state.
    tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
    let (status, first) = list(&harness, "").await;
    assert_eq!(status, 200, "ListIdentitySources returned {first:?}");
    let (status, again) = list(&harness, "").await;
    assert_eq!(status, 200, "ListIdentitySources returned {again:?}");
    assert_eq!(
        first, again,
        "ListIdentitySources is a read: two calls must agree"
    );
}

/// A new identity source is `CREATING` until its activation comes due, and `ACTIVE`
/// afterwards — the clock owes the transition, and no caller asks for it.
#[tokio::test]
async fn a_new_identity_source_becomes_active_once_its_activation_is_due() {
    for case in source_cases() {
        let harness = Harness::new();
        let (status, created) = create(&harness, &create_body(&case, None)).await;
        assert_eq!(
            status, 200,
            "case {:?}: create returned {created:?}",
            case.case
        );
        let arn = text(&created, "IdentitySourceArn");

        let (_, before) = list(&harness, "").await;
        let entry = entries(&before).pop().expect("the created source");
        assert_eq!(
            text(&entry, "Status"),
            state::IdentitySourceStatus::Creating.as_wire(),
            "case {:?}: a caller listing straight back must still see CREATING: {before:?}",
            case.case
        );

        tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
        let (_, after) = list(&harness, "").await;
        let entry = entries(&after).pop().expect("the created source");
        assert_eq!(
            text(&entry, "Status"),
            state::IdentitySourceStatus::Active.as_wire(),
            "case {:?}: the source must be ACTIVE once its activation is due: {after:?}",
            case.case
        );
        assert_eq!(
            text(&entry, "IdentitySourceArn"),
            arn,
            "case {:?}: activation must not change the ARN: {after:?}",
            case.case
        );
        assert_eq!(
            instant(&entry, "CreationTime"),
            instant(
                &entries(&before).pop().expect("the created source"),
                "CreationTime"
            ),
            "case {:?}: CreationTime must not move between reads",
            case.case
        );
    }
}

/// A store with no identity source lists none: an empty page, no `NextToken`, HTTP 200.
/// Reading is not restricted to the organization's management account, so this harness
/// records none.
#[tokio::test]
async fn an_empty_store_lists_no_identity_source() {
    let harness = Harness::without_organization();
    for query in ["", "MaxResults=1", "MaxResults=20"] {
        let (status, body) = list(&harness, query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: ListIdentitySources returned {status} with {body:?}"
        );
        assert!(
            entries(&body).is_empty(),
            "case {query:?}: nothing is stored, so nothing is listed: {body:?}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {query:?}: an exhausted page carries no NextToken: {body:?}"
        );
    }
}

/// Paging walks every stored source exactly once, in creation order, for every store size
/// and every page size `MaxResults` allows.
#[tokio::test]
async fn paging_walks_every_identity_source_exactly_once() {
    // Store sizes: empty, one, a few, and the most this service holds
    // (`state::MAX_IDENTITY_SOURCES`, from `IdentitySources`' `length(max: 20)`).
    for stored in [0_usize, 1, 2, 7, state::MAX_IDENTITY_SOURCES] {
        let harness = Harness::new();
        let cases = source_cases();
        let mut expected: Vec<String> = Vec::new();
        for n in 0..stored {
            // Rows are cycled, so no store under test is one repeated request.
            let case = &cases[n % cases.len()];
            let (status, created) = create(&harness, &create_body(case, None)).await;
            assert_eq!(status, 200, "create {n} returned {created:?}");
            expected.push(text(&created, "IdentitySourceArn"));
        }

        // Each published bound of `MaxResults` (1 and 20) and the sizes between.
        for page_size in [MAX_RESULTS_MIN, 2, 3, 7, MAX_RESULTS_MAX] {
            let case = format!("{stored} sources, MaxResults={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    None => format!("MaxResults={page_size}"),
                    Some(t) => format!("MaxResults={page_size}&NextToken={t}"),
                };
                let (status, body) = list(&harness, &query).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListIdentitySources returned {body:?}"
                );
                let page = entries(&body);
                assert!(
                    page.len() as i64 <= page_size,
                    "case {case:?}: a page of {} exceeds MaxResults",
                    page.len()
                );
                for entry in &page {
                    seen.push(text(entry, "IdentitySourceArn"));
                }
                pages += 1;
                assert!(
                    pages <= stored + 1,
                    "case {case:?}: paging did not terminate"
                );
                match body.get("NextToken").and_then(Value::as_str) {
                    Some(next) => {
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield every source exactly once, in creation order"
            );
            assert_eq!(
                seen.iter().collect::<BTreeSet<_>>().len(),
                seen.len(),
                "case {case:?}: paging returned a source twice: {seen:?}"
            );
        }

        // `MaxResults` omitted takes the modelled default of 20.
        let (status, body) = list(&harness, "").await;
        assert_eq!(status, 200, "the default page returned {body:?}");
        assert_eq!(
            entries(&body).len(),
            stored.min(MAX_RESULTS_MAX as usize),
            "the default MaxResults of {MAX_RESULTS_MAX} must fill the page: {body:?}"
        );
        assert_eq!(
            body.get("NextToken").is_some(),
            stored > MAX_RESULTS_MAX as usize,
            "a NextToken must be present exactly while sources remain: {body:?}"
        );
    }
}

/// A `NextToken` this service never issued is refused with `ValidationException` (400),
/// whatever its form.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let harness = Harness::new();
    let cases = source_cases();
    for case in &cases {
        let (status, created) = create(&harness, &create_body(case, None)).await;
        assert_eq!(status, 200, "create returned {created:?}");
    }

    // (case, the token). `%20` is a percent-encoded space: the query decoder hands the
    // handler " ".
    let rows: [(&str, String); 5] = [
        ("not a token at all", "not-a-token".to_string()),
        ("a space", "%20".to_string()),
        ("a negative position", "-1".to_string()),
        ("an identity-source ARN", expected_arn(1)),
        (
            "an approval-team ARN",
            format!(
                "arn:aws:mpa:{REGION}:{}:approval-team/team",
                engine::emulator_account_id()
            ),
        ),
    ];
    for (case, token) in rows {
        let (status, body) = list(&harness, &format!("NextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign NextToken must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The token the service DID issue is accepted, so the refusal is about a token's
    // provenance and not about tokens.
    let (status, body) = list(&harness, "MaxResults=1").await;
    assert_eq!(status, 200, "the first page returned {body:?}");
    let token = text(&body, "NextToken");
    assert!(
        !token.is_empty(),
        "a partial page must carry a NextToken: {body:?}"
    );
    let (status, body) = list(&harness, &format!("MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        status, 200,
        "the service's own NextToken must be accepted, got {body:?}"
    );
}

/// A create request that does not name a valid IAM Identity Center instance is refused
/// with `ValidationException` (400) and records nothing.
#[tokio::test]
async fn a_create_that_names_no_valid_instance_is_refused() {
    let harness = Harness::new();

    // (case, the body). Each row violates `IdcInstanceArn`'s modelled
    // `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` / `length(20..2048)`, or
    // names no instance at all.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an instance id shorter than the pattern's 16 characters",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ins-123", "Region": REGION}}}),
        ),
        (
            "an instance id longer than the pattern's 16 characters",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ins-1234567890abcdef0", "Region": REGION}}}),
        ),
        (
            "a character the pattern's id class excludes",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ins-1234567890abcde!", "Region": REGION}}}),
        ),
        (
            "no ins- prefix on the instance id",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/1234567890abcdef", "Region": REGION}}}),
        ),
        (
            "another service in the ARN's service field",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "arn:aws:mpa:::instance/ins-1234567890abcdef", "Region": REGION}}}),
        ),
        (
            "not an ARN at all, and shorter than length(min: 20)",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "not-an-arn", "Region": REGION}}}),
        ),
        (
            "an empty instance ARN",
            json!({"IdentitySourceParameters": {"IamIdentityCenter": {
                "InstanceArn": "", "Region": REGION}}}),
        ),
        (
            "parameters that name no identity source type",
            json!({"IdentitySourceParameters": {}}),
        ),
        ("no parameters at all", json!({})),
    ];

    for (case, body) in rows {
        let (status, refused) = create(&harness, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: wrong error code in {refused:?}"
        );
        let (status, listed) = list(&harness, "").await;
        assert_eq!(status, 200, "case {case:?}: list returned {listed:?}");
        assert!(
            entries(&listed).is_empty(),
            "case {case:?}: a refused create must record nothing: {listed:?}"
        );
    }

    // Every row above is refused for the value it carries, not because this service
    // refuses creates: a request naming a valid instance is accepted and listed.
    let case = &source_cases()[0];
    let (status, created) = create(&harness, &create_body(case, None)).await;
    assert_eq!(status, 200, "a valid create returned {created:?}");
    let (_, listed) = list(&harness, "").await;
    assert_eq!(
        entries(&listed).len(),
        1,
        "the accepted create must be the only source listed: {listed:?}"
    );
}

/// `ClientToken` is `@idempotencyToken`: replaying a create with the same token answers
/// with the source the first call minted and records no second one.
#[tokio::test]
async fn a_replayed_client_token_mints_one_identity_source() {
    let harness = Harness::new();

    // Token forms: `Token` is `length(max: 4096)` with no pattern.
    let tokens = [
        "token-1".to_string(),
        "9f1c7b24-0000-4000-8000-000000000001".to_string(),
        "t".repeat(4096),
    ];
    let cases = source_cases();
    let mut minted: Vec<String> = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        let case = &cases[index % cases.len()];
        let (status, first) = create(&harness, &create_body(case, Some(token))).await;
        assert_eq!(
            status, 200,
            "case {:?}: create returned {first:?}",
            case.case
        );
        let (status, replay) = create(&harness, &create_body(case, Some(token))).await;
        assert_eq!(
            status, 200,
            "case {:?}: the replay returned {replay:?}",
            case.case
        );
        assert_eq!(
            text(&replay, "IdentitySourceArn"),
            text(&first, "IdentitySourceArn"),
            "case {:?}: a replay must answer with the first source's ARN",
            case.case
        );
        assert_eq!(
            instant(&replay, "CreationTime"),
            instant(&first, "CreationTime"),
            "case {:?}: a replay must answer with the first source's CreationTime",
            case.case
        );
        minted.push(text(&first, "IdentitySourceArn"));
    }

    let (status, body) = list(&harness, "").await;
    assert_eq!(status, 200, "list returned {body:?}");
    let listed: Vec<String> = entries(&body)
        .iter()
        .map(|entry| text(entry, "IdentitySourceArn"))
        .collect();
    assert_eq!(
        listed, minted,
        "one source per distinct ClientToken must be stored: {body:?}"
    );

    // A create with NO token is never a replay: it mints another source.
    let (status, fresh) = create(&harness, &create_body(&cases[0], None)).await;
    assert_eq!(status, 200, "an untokened create returned {fresh:?}");
    let (_, body) = list(&harness, "").await;
    assert_eq!(
        entries(&body).len(),
        minted.len() + 1,
        "an untokened create must mint a source of its own: {body:?}"
    );
}

/// `CreateIdentitySource` is permitted to the management account of the recorded
/// organization and to nobody else — including when no organization is recorded, which is
/// the state this emulator starts in. A refused create records nothing, so
/// `ListIdentitySources` reports nothing; listing itself is not gated that way.
#[tokio::test]
async fn only_the_management_account_may_create_an_identity_source() {
    // (case, recorded management account, permitted)
    let rows: [(&str, Option<&str>, bool); 4] = [
        ("no organization recorded", None, false),
        (
            "another account is the management account",
            Some("999999999999"),
            false,
        ),
        (
            "an account one digit from the caller's is the management account",
            Some("000000000001"),
            false,
        ),
        (
            "the caller's own account is the management account",
            Some(engine::emulator_account_id()),
            true,
        ),
    ];

    for (case, management, permitted) in rows {
        let harness = Harness::without_organization();
        if let Some(account) = management {
            harness.record_management_account(account);
        }

        // Every row of the create table is gated the same way: the decision is about the
        // caller, not about what the body carries.
        for source_case in source_cases() {
            let label = format!("{case} / {}", source_case.case);
            let (status, body) = create(&harness, &create_body(&source_case, None)).await;
            if permitted {
                assert_eq!(status, 200, "case {label:?}: create returned {body:?}");
                continue;
            }
            assert_eq!(
                status, 403,
                "case {label:?}: create returned {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {label:?}: wrong error code in {body:?}"
            );
            assert_eq!(
                text(&body, "Message"),
                "Action can only be performed by an organization\u{2019}s management account.",
                "case {label:?}: wrong refusal message"
            );
        }

        // A refused create wrote NOTHING; a permitted one wrote one source per row.
        let expected = if permitted { source_cases().len() } else { 0 };
        let stored = {
            let store = state::service_state(&*harness.engine).expect("the service store");
            let guard = store.lock();
            guard.identity_sources.len()
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: the store holds {stored} identity source(s)"
        );
        let (status, listed) = list(&harness, "").await;
        assert_eq!(status, 200, "case {case:?}: list returned {listed:?}");
        assert_eq!(
            entries(&listed).len(),
            expected,
            "case {case:?}: list reported {} identity source(s): {listed:?}",
            entries(&listed).len()
        );
    }
}

/// A `ClientToken` is scoped to the create it was given to: the same token presented to
/// `CreateApprovalTeam` and to `CreateIdentitySource` mints one resource of each kind,
/// and each create replays to its OWN resource.
#[tokio::test]
async fn a_client_token_is_scoped_to_the_create_it_was_given_to() {
    let harness = Harness::new();
    let token = "shared-client-token";

    // A team body the modelled constraints accept: `Name` matches `^[a-zA-Z0-9._-]+$`,
    // `Description` is `length(1..256)`, `Approvers` `length(1..20)` and `Policies`
    // `length(1..10)`.
    let team_body = json!({
        "Name": "token-scope-team",
        "Description": "client token scope",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": expected_arn(1),
        }],
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
        }],
        "ClientToken": token,
    });

    let (status, team) = harness.create(&team_body).await;
    assert_eq!(status, 200, "the team create returned {status} with {team:?}");
    let team_arn = text(&team, "Arn");

    let case = &source_cases()[0];
    let (status, source) = create(&harness, &create_body(case, Some(token))).await;
    assert_eq!(
        status, 200,
        "the identity-source create returned {status} with {source:?}"
    );
    let source_arn = text(&source, "IdentitySourceArn");
    assert_eq!(
        source_arn,
        expected_arn(1),
        "the shared token must still mint an identity source: {source:?}"
    );
    assert_ne!(
        source_arn, team_arn,
        "the two creates must mint resources of their own"
    );

    // Each create replays to the resource IT minted, so neither token record was
    // overwritten by the other create.
    let (status, replayed_team) = harness.create(&team_body).await;
    assert_eq!(
        status, 200,
        "the team replay returned {status} with {replayed_team:?}"
    );
    assert_eq!(
        text(&replayed_team, "Arn"),
        team_arn,
        "the team replay must answer with the first team: {replayed_team:?}"
    );
    let (status, replayed_source) = create(&harness, &create_body(case, Some(token))).await;
    assert_eq!(
        status, 200,
        "the identity-source replay returned {status} with {replayed_source:?}"
    );
    assert_eq!(
        text(&replayed_source, "IdentitySourceArn"),
        source_arn,
        "the identity-source replay must answer with the first source: {replayed_source:?}"
    );

    let (status, body) = list(&harness, "").await;
    assert_eq!(status, 200, "list returned {body:?}");
    assert_eq!(
        entries(&body).len(),
        1,
        "exactly one identity source must be stored: {body:?}"
    );
}

/// The store is bounded: once it holds `state::MAX_IDENTITY_SOURCES` sources — the most
/// `IdentitySources`' `length(max: 20)` lets this service report — a further create is
/// refused with the `ServiceQuotaExceededException` (402) the model declares, and the
/// stored sources are untouched.
#[tokio::test]
async fn a_create_past_the_quota_is_refused() {
    let harness = Harness::new();
    let cases = source_cases();
    for n in 0..state::MAX_IDENTITY_SOURCES {
        let case = &cases[n % cases.len()];
        let (status, created) = create(&harness, &create_body(case, None)).await;
        assert_eq!(status, 200, "create {n} returned {created:?}");
    }

    let (status, refused) = create(&harness, &create_body(&cases[0], None)).await;
    assert_eq!(
        status, 402,
        "a create past the quota must be refused, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "wrong error code in {refused:?}"
    );

    let (status, body) = list(&harness, &format!("MaxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(status, 200, "list returned {body:?}");
    assert_eq!(
        entries(&body).len(),
        state::MAX_IDENTITY_SOURCES,
        "the refused create must leave the stored sources alone: {body:?}"
    );
}
