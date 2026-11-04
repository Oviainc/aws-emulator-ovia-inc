// SPDX-License-Identifier: CC0-1.0

//! The policy documents this service reports and persists, driven over the wire.
//!
//! The rules under test:
//!
//! 1. `ListPermissions` reports every modelled `Permission` member for a grant, including
//!    `Policy` — the single-statement IAM policy the grant itself expresses, allowing the
//!    granted actions (in this service's action namespace) on the CA the grant was made
//!    on, to the principal that holds it, conditioned on the account that assigned it —
//!    and reports `Actions` in the members' own order rather than the order the grant
//!    request listed them in.
//! 2. `PutPolicy` attaches the submitted resource policy to the private CA its
//!    `ResourceArn` names and `GetPolicy` displays it back: as a document, with the
//!    whitespace between its tokens dropped and everything else — member order, and every
//!    string with every character `AWSPolicy` admits — exactly as submitted.
//! 3. `PutPolicy` refuses, with `InvalidPolicyException`, a document that is not a policy
//!    of the CA it is being attached to: one that is not a JSON policy, one whose
//!    statement is about no identity, one granting an action outside this service, and one
//!    whose `Resource` is anything other than that CA. An ARN that names no private CA,
//!    a CA that is not there and a CA in `DELETED` are refused with their own modelled
//!    errors, and every refusal attaches nothing.
//!
//! The harness — the router, the signed request and the success/refusal helpers — is the
//! one `private_ca_lifecycle` drives the rest of this service with; only the cases here
//! are new.

use serde_json::{json, Value};

use crate::private_ca_lifecycle::{
    root_ca_request, string, wait_for_the_clock, Emulator, ACCOUNT, REGION,
};

/// Every `ActionType` member the model declares, from
/// `smithy_query(query="shape",shape="ActionType")`: `IssueCertificate`,
/// `GetCertificate`, `ListPermissions`. A grant names all of them, in any order
/// (`ActionList`, len 1..3).
const ACTIONS: [&str; 3] = ["IssueCertificate", "GetCertificate", "ListPermissions"];

/// This service's action namespace, from `smithy_query(query="service_info")`:
/// `awsService.arnNamespace` is `acm-pca`, which is the prefix an IAM action of this
/// service carries.
const ACTION_NAMESPACE: &str = "acm-pca";

/// `AWSPolicy`'s modelled length bounds, from
/// `smithy_query(query="operation",operation="PutPolicy")`: `PutPolicyRequest$Policy` is
/// `AWSPolicy`, len 1..81920.
const POLICY_MAX_LENGTH: usize = 81_920;
/// The lower bound of that same constraint. No document that short is a policy, so the
/// attach rows below start at the shortest document a statement can be framed in; what
/// this bound is used for is the reported `Permission$Policy`, which must sit inside it.
const POLICY_MIN_LENGTH: usize = 1;

/// Create a private CA and return its ARN.
async fn certificate_authority(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(case))
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// A `CreatePermission` body: the CA, the principal, the actions and, when the case names
/// one, the source account.
fn grant(arn: &str, principal: &str, source_account: Option<&str>, actions: &[&str]) -> Value {
    let mut body = json!({
        "CertificateAuthorityArn": arn,
        "Principal": principal,
        "Actions": actions,
    });
    if let Some(account) = source_account {
        body["SourceAccount"] = json!(account);
    }
    body
}

/// Every permission one CA holds, in a single page.
async fn all_permissions(emulator: &Emulator, case: &str, arn: &str) -> Vec<Value> {
    let page = emulator
        .ok(
            case,
            "ListPermissions",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    page.get("Permissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Permissions list in {page:?}"))
        .clone()
}

/// The permission a list reports for `principal` and `source_account`.
fn reported<'a>(
    permissions: &'a [Value],
    principal: &str,
    source_account: &str,
) -> Option<&'a Value> {
    permissions.iter().find(|permission| {
        permission["Principal"] == json!(principal)
            && permission["SourceAccount"] == json!(source_account)
    })
}

/// `actions` in the order a `Permission` reports them: the members' own order, which is
/// neither the request's nor the model's declaration order (`ActionType` declares
/// `IssueCertificate` before `GetCertificate`).
pub(crate) fn in_reported_order(actions: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = actions.iter().map(|action| action.to_string()).collect();
    ordered.sort();
    ordered
}

/// `AWSPolicy`'s modelled pattern is `^[\u0009\u000A\u000D\u0020-\u00FF]+$`: a document it
/// admits is non-empty and carries only tab, LF, CR and the Latin-1 range above them.
fn satisfies_the_policy_pattern(policy: &str) -> bool {
    !policy.is_empty()
        && policy
            .chars()
            .all(|c| matches!(c, '\u{9}' | '\u{a}' | '\u{d}' | '\u{20}'..='\u{ff}'))
}

/// `PutPolicy` for `resource_arn`, requiring success.
async fn put_policy(emulator: &Emulator, case: &str, resource_arn: &str, policy: &str) -> Value {
    emulator
        .ok(
            case,
            "PutPolicy",
            &json!({"ResourceArn": resource_arn, "Policy": policy}),
        )
        .await
}

/// `GetPolicy` for `resource_arn`, requiring 200, and answer its `Policy` member.
async fn get_policy(emulator: &Emulator, case: &str, resource_arn: &str) -> String {
    let answer = emulator
        .ok(case, "GetPolicy", &json!({"ResourceArn": resource_arn}))
        .await;
    string(&answer, "Policy", case)
}

/// The policy the store holds for `ca_arn`, read beside the wire.
fn stored_policy(emulator: &Emulator, ca_arn: &str) -> Option<String> {
    let state = emulator.state();
    let state = state.lock();
    state
        .certificate_authorities
        .get(ca_arn)
        .and_then(|record| record.resource_policy.clone())
}

// ---------------------------------------------------------------------------
// Permission$Policy: the policy a grant expresses
// ---------------------------------------------------------------------------

/// One row of the grant table: the case, the CA the grant is made on, the principal that
/// holds it, the account that assigned it (when the request names one) and the actions in
/// the order the request lists them.
type GrantRow<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, &'a [&'a str]);

/// `ListPermissions` reports, for every grant, the policy that grant expresses and the
/// granted actions in the order a `Permission` reports them.
#[tokio::test]
async fn a_listed_permission_reports_the_policy_its_grant_expresses() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "policy-grants").await;
    // A second CA, so the rule's `Resource` is pinned to the CA the grant was made on
    // rather than to whichever CA exists.
    let other_arn = certificate_authority(&emulator, "policy-grants-other").await;

    // The rows differ along what the rule generalises over: the order a grant lists the
    // actions in, which principal holds it, which account assigned it and which CA it was
    // made on. Every grant names every action, which is what `CreatePermission` records
    // ([`permission_action_set`] covers the refusal of a grant that does not).
    let rows: [GrantRow; 7] = [
        (
            "every action, in the order the model declares them",
            &arn,
            "acm.amazonaws.com",
            None,
            &ACTIONS,
        ),
        (
            "every action, reversed",
            &arn,
            "ram.amazonaws.com",
            Some("123456789012"),
            &[ACTIONS[2], ACTIONS[1], ACTIONS[0]],
        ),
        (
            "every action, already in reported order",
            &arn,
            "acm.amazonaws.com",
            Some("210987654321"),
            &[ACTIONS[1], ACTIONS[0], ACTIONS[2]],
        ),
        (
            "every action, another holder of the same CA",
            &arn,
            "acm-pca.amazonaws.com",
            Some("123456789012"),
            &[ACTIONS[2], ACTIONS[0], ACTIONS[1]],
        ),
        (
            "every action, a holder with no source account",
            &arn,
            "s3.amazonaws.com",
            None,
            &[ACTIONS[1], ACTIONS[2], ACTIONS[0]],
        ),
        (
            "every action, the same holder under a third account",
            &arn,
            "acm.amazonaws.com",
            Some("999999999999"),
            &[ACTIONS[2], ACTIONS[1], ACTIONS[0]],
        ),
        (
            "a grant on another CA",
            &other_arn,
            "acm.amazonaws.com",
            None,
            &ACTIONS,
        ),
    ];

    for (case, ca, principal, source_account, actions) in rows {
        emulator
            .ok(
                case,
                "CreatePermission",
                &grant(ca, principal, source_account, actions),
            )
            .await;
    }

    for (case, ca, principal, source_account, actions) in rows {
        // A request that names no source account is the calling account's grant, and the
        // calling account is the one every ARN of this emulator carries.
        let account = source_account.unwrap_or(ACCOUNT);
        let permissions = all_permissions(&emulator, case, ca).await;
        let permission = reported(&permissions, principal, account).unwrap_or_else(|| {
            panic!("case {case:?}: no grant for {principal} / {account} in {permissions:?}")
        });

        let expected_actions = in_reported_order(actions);
        assert_eq!(
            permission["Actions"],
            json!(expected_actions),
            "case {case:?}: expected the actions in reported order {expected_actions:?}, \
             got {permission:?}"
        );

        let policy = permission
            .get("Policy")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no Policy member in {permission:?}"))
            .to_string();
        assert!(
            satisfies_the_policy_pattern(&policy)
                && (POLICY_MIN_LENGTH..=POLICY_MAX_LENGTH).contains(&policy.chars().count()),
            "case {case:?}: expected a reported policy inside AWSPolicy's modelled \
             constraints, got {} characters: {policy:?}",
            policy.chars().count()
        );

        let document: Value = serde_json::from_str(&policy)
            .unwrap_or_else(|_| panic!("case {case:?}: Policy must be a JSON document: {policy:?}"));
        assert_eq!(
            document["Version"],
            json!("2012-10-17"),
            "case {case:?}: expected the current IAM policy language version, got {document:?}"
        );
        let statements = document["Statement"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Statement array in {document:?}"));
        assert_eq!(
            statements.len(),
            1,
            "case {case:?}: a grant expresses one statement, got {statements:?}"
        );
        let statement = &statements[0];
        assert_eq!(
            statement["Effect"],
            json!("Allow"),
            "case {case:?}: a grant allows, got {statement:?}"
        );
        assert_eq!(
            statement["Principal"],
            json!({"Service": principal}),
            "case {case:?}: expected the holder as the statement's principal, got {statement:?}"
        );
        let expected_namespaced: Vec<String> = expected_actions
            .iter()
            .map(|action| format!("{ACTION_NAMESPACE}:{action}"))
            .collect();
        assert_eq!(
            statement["Action"],
            json!(expected_namespaced),
            "case {case:?}: expected the granted actions as {expected_namespaced:?}, \
             got {statement:?}"
        );
        assert_eq!(
            statement["Resource"],
            json!(ca),
            "case {case:?}: expected the CA the grant was made on as the resource, \
             got {statement:?}"
        );
        assert_eq!(
            statement["Condition"],
            json!({"StringEquals": {"aws:SourceAccount": account}}),
            "case {case:?}: expected the assigning account as the condition, got {statement:?}"
        );

        // Reporting a grant is a read: the same grant reports the same document again.
        let again = all_permissions(&emulator, case, ca).await;
        let repeated = reported(&again, principal, account)
            .unwrap_or_else(|| panic!("case {case:?}: the grant must still be listed: {again:?}"));
        assert_eq!(
            repeated, permission,
            "case {case:?}: expected a repeated listing to report the same permission, \
             got {repeated:?}"
        );
    }
}

/// The policy a grant expresses follows the grant: revoking one grant and granting
/// another changes what is reported, and no two grants of one CA report each other's.
#[tokio::test]
async fn the_reported_policy_follows_the_grant_it_belongs_to() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "policy-follows").await;

    // Each grant names every action — what differs is the holder and the account that
    // assigned it, which is what the document a grant expresses is read off.
    let holders: [(&str, &str, &[&str]); 3] = [
        ("acm.amazonaws.com", "123456789012", &ACTIONS),
        ("ram.amazonaws.com", "210987654321", &ACTIONS),
        (
            "acm.amazonaws.com",
            "210987654321",
            &[ACTIONS[1], ACTIONS[2], ACTIONS[0]],
        ),
    ];
    for (principal, account, actions) in holders {
        emulator
            .ok(
                "granted",
                "CreatePermission",
                &grant(&arn, principal, Some(account), actions),
            )
            .await;
    }

    let permissions = all_permissions(&emulator, "granted", &arn).await;
    let documents: Vec<String> = holders
        .iter()
        .map(|(principal, account, _)| {
            let case = format!("the grant of {principal} from {account}");
            reported(&permissions, principal, account)
                .and_then(|permission| permission.get("Policy"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no Policy in {permissions:?}"))
                .to_string()
        })
        .collect();
    for (index, document) in documents.iter().enumerate() {
        for (other, sibling) in documents.iter().enumerate() {
            assert!(
                index == other || document != sibling,
                "case {:?}: two grants of one CA must not express the same policy, got \
                 {document:?}",
                holders[index].0
            );
        }
    }

    // Revoked, and the account's grant given to another holder: the reported document is
    // the one the CURRENT grant expresses, and the revoked grant's is reported by nobody.
    let (principal, account, _) = holders[1];
    emulator
        .ok(
            "revoked",
            "DeletePermission",
            &json!({
                "CertificateAuthorityArn": arn,
                "Principal": principal,
                "SourceAccount": account,
            }),
        )
        .await;
    let new_holder = "sns.amazonaws.com";
    emulator
        .ok(
            "granted again",
            "CreatePermission",
            &grant(&arn, new_holder, Some(account), &ACTIONS),
        )
        .await;
    let permissions = all_permissions(&emulator, "granted again", &arn).await;
    assert!(
        reported(&permissions, principal, account).is_none(),
        "case \"granted again\": the revoked grant must be gone, got {permissions:?}"
    );
    let policy = reported(&permissions, new_holder, account)
        .and_then(|permission| permission.get("Policy"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"granted again\": no Policy in {permissions:?}"))
        .to_string();
    let document: Value = serde_json::from_str(&policy).expect("a JSON policy");
    let namespaced: Vec<String> = in_reported_order(&ACTIONS)
        .iter()
        .map(|action| format!("{ACTION_NAMESPACE}:{action}"))
        .collect();
    assert_eq!(
        document["Statement"][0]["Action"],
        json!(namespaced),
        "case \"granted again\": expected the new grant's actions, got {document:?}"
    );
    assert_eq!(
        document["Statement"][0]["Principal"],
        json!({"Service": new_holder}),
        "case \"granted again\": expected the new holder as the principal, got {document:?}"
    );
    assert_ne!(
        policy, documents[1],
        "case \"granted again\": expected a document other than the revoked grant's, \
         got {policy:?}"
    );
}

// ---------------------------------------------------------------------------
// PutPolicy / GetPolicy: attach then display
// ---------------------------------------------------------------------------

/// One attach row: the case, the document as it is SUBMITTED and the document as it must
/// be DISPLAYED back.
type AttachRow = (String, String, String);

/// A policy statement about `ca_arn`, as a `Value`, with `sid` as its statement id.
fn statement_for(ca_arn: &str, sid: &str) -> Value {
    json!({
        "Sid": sid,
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
        "Action": ["acm-pca:GetCertificate", "acm-pca:DescribeCertificateAuthority"],
        "Resource": ca_arn
    })
}

/// A policy about `ca_arn`, as a `Value`, with `sid` as its single statement's id.
fn policy_value(ca_arn: &str, sid: &str) -> Value {
    json!({"Version": "2012-10-17", "Statement": [statement_for(ca_arn, sid)]})
}

/// A policy about `ca_arn` of exactly `length` characters, padded out in its statement id.
///
/// `length` has to leave room for the framing; every row below is far above it.
fn policy_of_length(ca_arn: &str, length: usize) -> String {
    let framing = policy_value(ca_arn, "").to_string().chars().count();
    let padding = length
        .checked_sub(framing)
        .unwrap_or_else(|| panic!("a policy about {ca_arn} cannot be framed in {length} characters"));
    policy_value(ca_arn, &"A".repeat(padding)).to_string()
}

/// The attach rows for the CA `ca_arn`: the rows differ in whitespace (none, pretty
/// printed, tabs and CRLF), in statement count and framing, in character class and in
/// length, the modelled maximum included.
///
/// The DISPLAYED column of every row built from a `Value` is `serde_json`'s compact
/// rendering of that same `Value`, and the submitted column its pretty-printed rendering:
/// one serializer writes both, so what the row pins is the rule — the whitespace between
/// tokens is dropped and nothing else changes — rather than this emulator's own output.
fn attach_rows(ca_arn: &str) -> Vec<AttachRow> {
    let pretty = |case: &str, document: &Value| {
        (
            case.to_string(),
            serde_json::to_string_pretty(document).expect("a JSON document"),
            document.to_string(),
        )
    };
    let compact = |case: &str, document: &Value| {
        (case.to_string(), document.to_string(), document.to_string())
    };

    let mut rows: Vec<AttachRow> = vec![
        compact("a compact single-statement policy", &policy_value(ca_arn, "share")),
        pretty(
            "the same policy, pretty printed",
            &policy_value(ca_arn, "share"),
        ),
        pretty(
            // The same pattern admits every character up to U+00FF, so Latin-1 text in a
            // statement id is a policy whose text must survive unchanged.
            "a Latin-1 statement id above ASCII",
            &policy_value(ca_arn, "Café-Zürich-Straße-Ñoño-Allow"),
        ),
        pretty(
            "every Latin-1 character the pattern admits in a statement id",
            &policy_value(
                ca_arn,
                // A JSON string escapes `"` and `\`, so the document stays a document;
                // every other admitted character goes in as itself.
                &('\u{20}'..='\u{ff}')
                    .filter(|c| !matches!(c, '"' | '\\'))
                    .collect::<String>(),
            ),
        ),
        pretty(
            "two statements, one of them a Deny",
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    statement_for(ca_arn, "cross-account-read"),
                    {
                        "Sid": "no-issuance",
                        "Effect": "Deny",
                        "Principal": {"AWS": ["arn:aws:iam::111122223333:root",
                                              "444455556666"]},
                        "Action": "acm-pca:IssueCertificate",
                        "Resource": ca_arn
                    }
                ]
            }),
        ),
        pretty(
            // `Statement` holds one statement or a list of them: both are policies.
            "a single statement given as an object rather than a list",
            &json!({"Version": "2012-10-17", "Statement": statement_for(ca_arn, "object")}),
        ),
        (
            // `AWSPolicy`'s pattern admits tab (U+0009), LF (U+000A) and CR (U+000D): all
            // three are whitespace between tokens. Inside a string, the spaces of the
            // statement id are text and have to survive (a raw tab inside a JSON string
            // is not JSON, so the id carries spaces).
            "tabs and CRLF between the tokens, and spaces inside a string".to_string(),
            format!(
                "{{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{{\r\n\
                 \t\t\t\"Sid\": \"a sid with spacing\",\r\n\t\t\t\"Effect\": \"Allow\",\r\n\
                 \t\t\t\"Principal\": \"arn:aws:iam::{ACCOUNT}:root\",\r\n\
                 \t\t\t\"Action\": \"acm-pca:GetCertificate\",\r\n\
                 \t\t\t\"Resource\": \"{ca_arn}\"\r\n\t\t}}\r\n\t]\r\n}}"
            ),
            format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"a sid with spacing\",\
                 \"Effect\":\"Allow\",\"Principal\":\"arn:aws:iam::{ACCOUNT}:root\",\
                 \"Action\":\"acm-pca:GetCertificate\",\"Resource\":\"{ca_arn}\"}}]}}"
            ),
        ),
    ];
    // Lengths across the modelled range, the maximum and one below it included.
    for length in [1_024, POLICY_MAX_LENGTH - 1, POLICY_MAX_LENGTH] {
        let policy = policy_of_length(ca_arn, length);
        assert_eq!(
            policy.chars().count(),
            length,
            "a row of {length} characters must be exactly that long"
        );
        rows.push((
            format!("a policy of {length} characters"),
            policy.clone(),
            policy,
        ));
    }
    // Every row IS a policy document inside `AWSPolicy`'s modelled constraints, so a
    // refusal below is this emulator's answer and never the row's fault.
    for (case, submitted, displayed) in &rows {
        for (which, document) in [("submitted", submitted), ("displayed", displayed)] {
            assert!(
                serde_json::from_str::<Value>(document).is_ok(),
                "case {case:?}: the {which} document must be JSON, got {document:?}"
            );
            assert!(
                satisfies_the_policy_pattern(document)
                    && (POLICY_MIN_LENGTH..=POLICY_MAX_LENGTH)
                        .contains(&document.chars().count()),
                "case {case:?}: the {which} document must satisfy AWSPolicy's constraints, \
                 got {} characters",
                document.chars().count()
            );
        }
    }
    rows
}

/// What `PutPolicy` attached is what `GetPolicy` displays — as a document, with only the
/// whitespace between its tokens dropped — and a second attach replaces it.
#[tokio::test]
async fn put_policy_attaches_a_document_get_policy_displays_as_a_document() {
    let emulator = Emulator::new();

    for index in 0..attach_rows("arn:aws:acm-pca:x:y:certificate-authority/z").len() {
        let arn = certificate_authority(&emulator, &format!("round-trip-{index}")).await;
        let rows = attach_rows(&arn);
        let (case, submitted, displayed) = rows[index].clone();

        // Before the attach there is no policy to display.
        emulator
            .refused(
                &case,
                "GetPolicy",
                &json!({"ResourceArn": arn}),
                "ResourceNotFoundException",
            )
            .await;

        let answer = put_policy(&emulator, &case, &arn, &submitted).await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: PutPolicy models no output members, got {answer:?}"
        );

        let answered = get_policy(&emulator, &case, &arn).await;
        assert_eq!(
            answered, displayed,
            "case {case:?}: expected the attached document of {} characters displayed as a \
             document, got {} characters: {answered:?}",
            submitted.chars().count(),
            answered.chars().count()
        );
        assert_eq!(
            stored_policy(&emulator, &arn).as_deref(),
            Some(displayed.as_str()),
            "case {case:?}: expected the document the CA holds to be the one displayed"
        );
        // A read is a read: the second call answers the same document.
        let again = get_policy(&emulator, &case, &arn).await;
        assert_eq!(
            again, answered,
            "case {case:?}: expected a repeated read to answer the same document, got {again:?}"
        );

        // "The policy can be attached or updated with PutPolicy" (`GetPolicy` docs): a
        // second attach replaces the first document rather than adding to it.
        let (next_case, next_submitted, next_displayed) = rows[(index + 1) % rows.len()].clone();
        put_policy(&emulator, &next_case, &arn, &next_submitted).await;
        let answered = get_policy(&emulator, &next_case, &arn).await;
        assert_eq!(
            answered, next_displayed,
            "case {next_case:?}: expected the replacing document back, got {answered:?}"
        );
    }
}

/// A client is free to spell a non-ASCII character as a `\uXXXX` escape — Python's
/// `json.dumps` does by default, so botocore sends policy documents that way — and the
/// escape denotes the character it escapes: the document attached, and displayed back, is
/// the same one either spelling was sent with.
#[tokio::test]
async fn an_escaped_request_body_attaches_the_characters_it_escapes() {
    let emulator = Emulator::new();

    // Each row is the statement id of a policy whose text is non-ASCII Latin-1, which
    // `AWSPolicy`'s pattern admits in full.
    let rows: [(&str, &str); 3] = [
        ("Latin-1 letters", "Café-Zürich-Straße-Ñoño-Allow"),
        (
            "every admitted letter above ASCII",
            "ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞßÿ",
        ),
        (
            "punctuation and symbols above ASCII",
            "¡¢£¤¥¦§¨©ª«¬®¯°±²³´µ¶·¸¹º»¼½¾¿×÷",
        ),
    ];

    for (case, sid) in rows {
        let arn = certificate_authority(&emulator, &format!("escaped-{case}")).await;
        let policy = policy_value(&arn, sid).to_string();

        // The request body spells every non-ASCII character as a `\uXXXX` escape, the way
        // a client whose JSON writer escapes them does.
        let body = json!({"ResourceArn": arn, "Policy": policy}).to_string();
        let escaped: String = body
            .chars()
            .map(|c| {
                if c.is_ascii() {
                    c.to_string()
                } else {
                    c.encode_utf16(&mut [0u16; 2])
                        .iter()
                        .map(|unit| format!("\\u{unit:04x}"))
                        .collect()
                }
            })
            .collect();
        assert!(
            escaped.is_ascii() && escaped != body,
            "case {case:?}: the escaped body must be ASCII and differ from the raw one"
        );

        let (status, parsed) = emulator.call_raw("PutPolicy", escaped.into_bytes()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the escaped attach to be accepted, got {status} {parsed:?}"
        );

        let answered = get_policy(&emulator, case, &arn).await;
        assert_eq!(
            answered, policy,
            "case {case:?}: expected the characters the escapes denote, got {answered:?}"
        );
    }
}

/// A policy belongs to the one CA it was attached to, and attaching one touches no other.
#[tokio::test]
async fn each_private_ca_carries_only_the_policy_attached_to_it() {
    let emulator = Emulator::new();

    let mut attached: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let arn = certificate_authority(&emulator, &format!("own-policy-{index}")).await;
        let (case, submitted, displayed) = attach_rows(&arn)[index].clone();
        put_policy(&emulator, &case, &arn, &submitted).await;
        attached.push((arn, displayed));
    }
    let bare = certificate_authority(&emulator, "own-policy-none").await;

    for (arn, displayed) in &attached {
        let case = format!("the policy of {arn}");
        let answered = get_policy(&emulator, &case, arn).await;
        assert_eq!(
            answered, *displayed,
            "case {case:?}: expected each CA to display its own policy, got {answered:?}"
        );
    }
    emulator
        .refused(
            "a CA beside CAs that do carry a policy",
            "GetPolicy",
            &json!({"ResourceArn": bare}),
            "ResourceNotFoundException",
        )
        .await;
}

// ---------------------------------------------------------------------------
// PutPolicy: the documents it refuses
// ---------------------------------------------------------------------------

/// The message a refused `Resource` carries, naming the CA the policy is attached to:
/// the refusal is built from the request, which is what tells the caller what the
/// statements have to be about.
fn different_resource_message(ca_arn: &str) -> String {
    format!(
        "InvalidPolicy: The supplied policy contains different resource or incorrect ARN \
         format: {ca_arn}"
    )
}

/// A document that is not a policy of the CA it is submitted for is refused with
/// `InvalidPolicyException`, and the refusal attaches nothing.
#[tokio::test]
async fn put_policy_refuses_a_document_that_is_not_a_policy_of_the_ca() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "invalid-policy").await;
    let other = certificate_authority(&emulator, "invalid-policy-other").await;
    let (_, standing, standing_displayed) = attach_rows(&arn)[0].clone();
    put_policy(&emulator, "a policy that stands", &arn, &standing).await;

    // Every row satisfies `AWSPolicy`'s modelled length and pattern (so it reaches the
    // handler) and is not a policy this CA can carry: "If this policy does not contain
    // all required statements or if it includes any statement that is not allowed, the
    // PutPolicy action returns an InvalidPolicyException" (`PutPolicyRequest$Policy`
    // docs). The third column is the message the rule builds, where the rule builds one.
    let statement = |members: Value| json!({"Version": "2012-10-17", "Statement": [members]});
    let rows: Vec<(&str, String, Option<String>)> = vec![
        ("not JSON at all", "not a policy".to_string(), None),
        ("a truncated JSON object", "{\"Statement\":".to_string(), None),
        (
            "a JSON array rather than an object",
            "[{\"Effect\":\"Allow\"}]".to_string(),
            None,
        ),
        ("a JSON string", "\"Allow\"".to_string(), None),
        ("a JSON number", "17".to_string(), None),
        (
            "an object with no Statement",
            json!({"Version": "2012-10-17"}).to_string(),
            None,
        ),
        (
            "an empty Statement list",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
            None,
        ),
        (
            "a statement that is not an object",
            json!({"Version": "2012-10-17", "Statement": ["Allow"]}).to_string(),
            None,
        ),
        (
            "a statement whose Effect is neither Allow nor Deny",
            statement(json!({"Effect": "Permit", "Principal": "*",
                             "Action": "acm-pca:GetCertificate", "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a statement with no Effect",
            statement(json!({"Principal": "*", "Action": "acm-pca:GetCertificate",
                             "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a statement with no Principal",
            statement(json!({"Effect": "Allow", "Action": "acm-pca:GetCertificate",
                             "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a principal that names no identity",
            statement(json!({"Effect": "Allow", "Principal": {"AWS": "nobody"},
                             "Action": "acm-pca:GetCertificate", "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a principal of an unknown kind",
            statement(json!({"Effect": "Allow", "Principal": {"Carrier": "pigeon"},
                             "Action": "acm-pca:GetCertificate", "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a statement with no Action",
            statement(json!({"Effect": "Allow", "Principal": "*", "Resource": arn})).to_string(),
            None,
        ),
        (
            "an action of another service",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": ["acm-pca:GetCertificate", "s3:GetObject"],
                             "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "an action with no service namespace",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "GetCertificate", "Resource": arn}))
            .to_string(),
            None,
        ),
        (
            "a statement with no Resource",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "acm-pca:GetCertificate"}))
            .to_string(),
            None,
        ),
        (
            "a wildcard resource",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "acm-pca:GetCertificate", "Resource": "*"}))
            .to_string(),
            Some(different_resource_message(&arn)),
        ),
        (
            "another CA of this account as the resource",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "acm-pca:GetCertificate", "Resource": other}))
            .to_string(),
            Some(different_resource_message(&arn)),
        ),
        (
            "the CA's own ARN with a resource segment appended",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "acm-pca:GetCertificate",
                             "Resource": format!("{arn}/certificate/*")}))
            .to_string(),
            Some(different_resource_message(&arn)),
        ),
        (
            "one statement about this CA beside one about another",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    statement_for(&arn, "mine"),
                    statement_for(&other, "not mine")
                ]
            })
            .to_string(),
            Some(different_resource_message(&arn)),
        ),
        (
            "a resource list carrying one resource that is not this CA",
            statement(json!({"Effect": "Allow", "Principal": "*",
                             "Action": "acm-pca:GetCertificate",
                             "Resource": [arn.clone(), other.clone()]}))
            .to_string(),
            Some(different_resource_message(&arn)),
        ),
    ];

    for (case, policy, message) in rows {
        let refusal = emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": policy}),
                "InvalidPolicyException",
            )
            .await;
        if let Some(message) = message {
            let answered = refusal
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                answered, message,
                "case {case:?}: expected the refusal to name the CA the policy must be \
                 about, got {answered:?}"
            );
        } else {
            assert!(
                refusal
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the refusal carries a message, got {refusal:?}"
            );
        }
        assert_eq!(
            stored_policy(&emulator, &arn).as_deref(),
            Some(standing_displayed.as_str()),
            "case {case:?}: a refused attach must leave the standing policy as it was"
        );
        assert_eq!(
            stored_policy(&emulator, &other),
            None,
            "case {case:?}: a refused attach must touch no other CA"
        );
    }
}

/// An ARN of this service that names no private CA, and a CA ARN that names none of this
/// emulator's CAs, are each refused with their modelled error and attach nothing.
#[tokio::test]
async fn put_policy_refuses_an_arn_it_cannot_attach_a_policy_to() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "arn-guards").await;
    let (_, submitted, displayed) = attach_rows(&arn)[0].clone();

    // Each row satisfies `Arn`'s modelled pattern and length (so it reaches the handler)
    // and names something other than `certificate-authority/<id>` in this service.
    let not_a_ca: Vec<(&str, String)> = vec![
        (
            "no CA id",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
        ),
        (
            "an extra resource segment",
            format!("{arn}/certificate/0123456789abcdef"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abcdef"),
        ),
        (
            "an empty account",
            format!("arn:aws:acm-pca:{REGION}::certificate-authority/abcdef"),
        ),
        (
            "an eleven-digit account",
            format!("arn:aws:acm-pca:{REGION}:11112222333:certificate-authority/abcdef"),
        ),
    ];
    for (case, resource_arn) in not_a_ca {
        emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": resource_arn, "Policy": submitted}),
                "InvalidArnException",
            )
            .await;
        assert_eq!(
            stored_policy(&emulator, &resource_arn),
            None,
            "case {case:?}: a refused attach must persist nothing"
        );
    }

    // Well-formed CA ARNs that name no CA this emulator holds.
    let unknown: Vec<(&str, String)> = vec![
        (
            "a CA that was never created",
            format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a CA of another account",
            format!("arn:aws:acm-pca:{REGION}:111122223333:certificate-authority/abcdef"),
        ),
        (
            "a CA of another region",
            format!("arn:aws:acm-pca:us-gov-west-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
        (
            "a CA of another partition",
            format!("arn:aws-cn:acm-pca:cn-north-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
    ];
    for (case, resource_arn) in unknown {
        // The policy is about THAT CA, so the refusal is about the CA and not about the
        // document.
        let policy = policy_value(&resource_arn, "unknown-ca").to_string();
        emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": resource_arn, "Policy": policy}),
                "ResourceNotFoundException",
            )
            .await;
        assert_eq!(
            stored_policy(&emulator, &resource_arn),
            None,
            "case {case:?}: a refused attach must persist nothing"
        );
    }

    // And the CA those rows were built around still takes a policy.
    put_policy(&emulator, "the CA itself", &arn, &submitted).await;
    let answered = get_policy(&emulator, "the CA itself", &arn).await;
    assert_eq!(
        answered, displayed,
        "case \"the CA itself\": expected the attached policy back, got {answered:?}"
    );
}

/// A CA in `DELETED` is waiting to be restored or permanently deleted: what it holds is
/// not writable, which is what `InvalidStateException` answers. A restore reopens it.
#[tokio::test]
async fn put_policy_refuses_a_deleted_certificate_authority() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "deleted-ca").await;
    let rows = attach_rows(&arn);
    let (_, first, first_displayed) = rows[0].clone();
    let (_, second, second_displayed) = rows[1].clone();
    put_policy(&emulator, "before the deletion", &arn, &first).await;

    wait_for_the_clock();
    emulator
        .ok(
            "deleted",
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn, "PermanentDeletionTimeInDays": 7}),
        )
        .await;

    emulator
        .refused(
            "a deleted CA",
            "PutPolicy",
            &json!({"ResourceArn": arn, "Policy": second}),
            "InvalidStateException",
        )
        .await;
    assert_eq!(
        stored_policy(&emulator, &arn).as_deref(),
        Some(first_displayed.as_str()),
        "case \"a deleted CA\": a refused attach must leave the standing policy as it was"
    );

    emulator
        .ok(
            "restored",
            "RestoreCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    put_policy(&emulator, "after the restore", &arn, &second).await;
    let answered = get_policy(&emulator, "after the restore", &arn).await;
    assert_eq!(
        answered, second_displayed,
        "case \"after the restore\": expected the new document back, got {answered:?}"
    );
}
