// SPDX-License-Identifier: CC0-1.0

//! `CreateDelegationRequest`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rules being asserted:
//!
//! * the call FILES a delegation request: it mints the identifier the request is held
//!   under and answers it, and every member the request carried — plus the state the
//!   presence of a target account puts the request in, the instant it was filed and the
//!   instant it expires — is what the read of that identifier reports afterwards;
//! * a member whose value has the wrong FORM for what its documentation says it names
//!   (a notification channel that is not an SNS topic's ARN, a permission naming no
//!   policy template) is refused with HTTP 400 `ValidationError`, worded by whether the
//!   value is no ARN at all or an ARN of another resource, and nothing is filed;
//! * a `RequestorWorkflowId` the account has already filed a request under is refused
//!   with the modelled `EntityAlreadyExistsException` (HTTP 409 `EntityAlreadyExists`),
//!   and the request filed under it first is left as it was;
//! * a filing past the account's delegation-request quota is refused with the modelled
//!   `LimitExceededException` (HTTP 409 `LimitExceeded`), so repeated filings cannot grow
//!   the account's store without end;
//! * a caller whose identity-based policies do not permit the call is refused 403
//!   `AccessDenied` before anything is minted.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: which
//! optional members the filing carries, both published `SessionDuration` bounds, both
//! truth values of `OnlySendByOwner`, the partition, Region, account and name an ARN
//! carries, and the ways a value can fail to be the ARN its member must name.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::DelegationRequest;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router_with_engine,
};

/// The modelled bounds of `delegationRequestIdType`, which the identifier this operation
/// mints has to satisfy: "length 16..128, pattern `^[\w-]+$`"
/// (`smithy_query(query="operation", operation="CreateDelegationRequest")`).
const MIN_ID_CHARACTERS: usize = 16;
const MAX_ID_CHARACTERS: usize = 128;

/// The modelled maximum length of `consoleDeepLinkType`, the type the answered console
/// link is reported through (same `smithy_query` call).
const MAX_CONSOLE_DEEP_LINK_CHARACTERS: usize = 255;

/// The message AWS states a value supplied for a member its documentation says is an ARN
/// with, when that value cannot be read as an ARN at all — recorded wording, which quotes
/// nothing of the value or the member: the sentence states the ARN between `Invalid ARN:`
/// and `Could not be parsed!`, one space on either side, so a value nothing can be read
/// from renders that slot empty and the two spaces meet
/// (`tests/all/unparseable_arn_refusal_wording.rs` is this crate's table for it).
const NOT_AN_ARN: &str = "Invalid ARN:  Could not be parsed!";

/// The message AWS states a well-formed ARN naming a resource the member cannot address
/// with — the wording measured for the sibling read of an `EntityArn` of the wrong
/// resource type (`tests/all/human_readable_summaries.rs`).
const WRONG_RESOURCE_IN_ARN: &str = "Invalid resource type in ARN";

/// One filing, as a row of a table states it: the five members every call must carry,
/// plus the optional ones a row may add.
#[derive(Debug, Clone)]
struct Filing {
    description: String,
    notification_channel: String,
    policy_template_arn: Option<String>,
    requestor_workflow_id: String,
    session_duration: i32,
    only_send_by_owner: bool,
    owner_account_id: Option<String>,
    request_message: Option<String>,
    redirect_url: Option<String>,
    /// One policy parameter, as `(Name, Value, Type)` — "a list of policy parameters that
    /// define the scope and constraints of the delegated permissions"
    /// (`DelegationPermission$Parameters`).
    parameter: Option<(String, String, String)>,
}

/// A filing every documented rule accepts, under the workflow id `workflow`: the base a
/// row varies ONE thing of.
fn filing(workflow: &str) -> Filing {
    Filing {
        description: "temporary access for a partner engagement".to_string(),
        notification_channel: "arn:aws:sns:us-east-1:123456789012:partner-updates".to_string(),
        policy_template_arn: Some(
            "arn:aws:iam::aws:policy-template/PartnerDropReadWrite".to_string(),
        ),
        requestor_workflow_id: workflow.to_string(),
        // The published minimum `SessionDuration` (range 300..43200).
        session_duration: 300,
        only_send_by_owner: false,
        owner_account_id: None,
        request_message: None,
        redirect_url: None,
        parameter: None,
    }
}

impl Filing {
    /// This filing as awsQuery form parameters, in the flattened spelling the protocol
    /// gives a structure member and a list inside one.
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            ("Description".to_string(), self.description.clone()),
            (
                "NotificationChannel".to_string(),
                self.notification_channel.clone(),
            ),
            (
                "RequestorWorkflowId".to_string(),
                self.requestor_workflow_id.clone(),
            ),
            (
                "SessionDuration".to_string(),
                self.session_duration.to_string(),
            ),
            (
                "OnlySendByOwner".to_string(),
                self.only_send_by_owner.to_string(),
            ),
        ];
        if let Some(arn) = &self.policy_template_arn {
            params.push(("Permissions.PolicyTemplateArn".to_string(), arn.clone()));
        }
        if let Some(account) = &self.owner_account_id {
            params.push(("OwnerAccountId".to_string(), account.clone()));
        }
        if let Some(message) = &self.request_message {
            params.push(("RequestMessage".to_string(), message.clone()));
        }
        if let Some(url) = &self.redirect_url {
            params.push(("RedirectUrl".to_string(), url.clone()));
        }
        if let Some((name, value, kind)) = &self.parameter {
            params.push((
                "Permissions.Parameters.member.1.Name".to_string(),
                name.clone(),
            ));
            params.push((
                "Permissions.Parameters.member.1.Values.member.1".to_string(),
                value.clone(),
            ));
            params.push((
                "Permissions.Parameters.member.1.Type".to_string(),
                kind.clone(),
            ));
        }
        params
    }
}

/// One `CreateDelegationRequest` call, presented as signed by `akid` when a row names
/// one.
async fn create(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    filing: &Filing,
) -> (u16, String) {
    let owned = filing.params();
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_signed_by(router, akid, "CreateDelegationRequest", &params).await
}

/// One `GetDelegationRequest` call for `delegation_request_id`.
async fn get(router: &MultiServiceRouter, delegation_request_id: &str) -> (u16, String) {
    call(
        router,
        "GetDelegationRequest",
        &[("DelegationRequestId", delegation_request_id)],
    )
    .await
}

/// The instant the member `tag` reports, in seconds, and `None` when the response carries
/// no such member.
fn reported_instant(case: &str, body: &str, tag: &str) -> Option<i64> {
    let reported = elements(body, tag);
    assert!(
        reported.len() <= 1,
        "case {case:?}: expected at most one <{tag}>, got {reported:?}"
    );
    reported.first().map(|text| {
        DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected an ISO-8601 {tag}, got {text:?}: {error}")
            })
            .secs()
    })
}

/// The identifiers of the delegation requests the account holds, read straight out of the
/// service's own store: what a refused filing must leave untouched.
pub(crate) fn held(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock().delegation_requests.keys().cloned().collect();
    held
}

/// RULE: the call files a delegation request — it mints the identifier it is held under,
/// answers that identifier and the console page the request is managed through, and every
/// member the filing carried is what the read of that identifier reports afterwards, with
/// the state the presence of a target account puts the request in.
#[tokio::test]
async fn a_filed_request_is_what_the_read_reports() {
    // Rows: which optional members the filing carries, both published `SessionDuration`
    // bounds, both truth values of `OnlySendByOwner`, and an ARN from another partition.
    let rows = vec![
        ("the members a filing must carry", filing("workflow-plain")),
        (
            "every optional member, at the published maximum session duration",
            Filing {
                owner_account_id: Some(account_id().to_string()),
                request_message: Some(
                    "the earlier request was rejected for a missing boundary".to_string(),
                ),
                redirect_url: Some("https://partner.example.com/delegation/return".to_string()),
                parameter: Some((
                    "BucketName".to_string(),
                    "partner-drop".to_string(),
                    // One of the values `PolicyParameterTypeEnum` declares
                    // (`smithy_query(query="shape", shape="PolicyParameterTypeEnum")`).
                    "string".to_string(),
                )),
                session_duration: 43_200,
                only_send_by_owner: true,
                ..filing("workflow-full")
            },
        ),
        (
            "a target account, an empty description and a topic in another partition",
            Filing {
                owner_account_id: Some("210987654321".to_string()),
                description: String::new(),
                notification_channel:
                    "arn:aws-cn:sns:cn-north-1:210987654321:delegation_updates.1".to_string(),
                policy_template_arn: Some(
                    "arn:aws-cn:iam::aws:policy-template/PartnerDrop".to_string(),
                ),
                ..filing("workflow-other-partition")
            },
        ),
    ];

    let (router, engine) = router_with_engine();
    let mut minted: Vec<String> = Vec::new();
    for (case, row) in rows {
        let (status, body) = create(&router, None, &row).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the filing to be served, got {status}: {body}"
        );

        let id = element(&body, "DelegationRequestId");
        assert!(
            (MIN_ID_CHARACTERS..=MAX_ID_CHARACTERS).contains(&id.chars().count())
                && id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || character == '_'
                        || character == '-'),
            "case {case:?}: expected a minted identifier inside delegationRequestIdType, \
             got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh identifier, got {id:?} again after {minted:?}"
        );
        minted.push(id.clone());

        // "A deep link URL to the Amazon Web Services Management Console for managing the
        // delegation request": the console's own address, naming THIS request.
        let link = element(&body, "ConsoleDeepLink");
        assert!(
            link.starts_with("https://")
                && link.contains("/iam/home")
                && link.ends_with(&id)
                && link.chars().count() <= MAX_CONSOLE_DEEP_LINK_CHARACTERS,
            "case {case:?}: expected a console deep link naming {id}, got {link:?}"
        );

        // The round trip: the filed request as the read reports it.
        let (status, read) = get(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the filed request to be readable, got {status}: {read}"
        );

        let mut expected: Vec<(&str, Vec<String>)> = vec![
            ("DelegationRequestId", vec![id.clone()]),
            ("Description", vec![row.description.clone()]),
            (
                "PolicyTemplateArn",
                row.policy_template_arn.iter().cloned().collect(),
            ),
            (
                "SessionDuration",
                vec![row.session_duration.to_string()],
            ),
            (
                "OwnerAccountId",
                row.owner_account_id.iter().cloned().collect(),
            ),
            (
                "RequestMessage",
                row.request_message.iter().cloned().collect(),
            ),
            ("RedirectUrl", row.redirect_url.iter().cloned().collect()),
            // "Identity of the requestor of this delegation request. This will be an
            // Amazon Web Services account ID": the account the call was made in.
            ("RequestorId", vec![account_id().to_string()]),
            // A request is filed by the partner and owned by an identity that associates
            // itself with it afterwards, so a request just filed is owned by nobody and
            // nobody has approved, noted or rejected it.
            ("OwnerId", Vec::new()),
            ("ApproverId", Vec::new()),
            ("Notes", Vec::new()),
            ("RejectionReason", Vec::new()),
            // The template's content is registered through partner onboarding and reaches
            // no API, so no permission policy was resolved for the request and no
            // permissions boundary could be derived from one.
            ("PermissionPolicy", Vec::new()),
            (
                "State",
                vec![if row.owner_account_id.is_some() {
                    "ASSIGNED".to_string()
                } else {
                    "UNASSIGNED".to_string()
                }],
            ),
        ];
        if let Some((name, _, kind)) = &row.parameter {
            expected.push(("Name", vec![name.clone()]));
            expected.push(("Type", vec![kind.clone()]));
        }
        for (tag, want) in expected {
            assert_eq!(
                elements(&read, tag),
                want,
                "case {case:?}: expected <{tag}> {want:?}, got {:?}: {read}",
                elements(&read, tag)
            );
        }
        // A parameter's `Values` is a list, so its one value sits in a nested `member`
        // element that the leaf reader above cannot address.
        if let Some((_, value, _)) = &row.parameter {
            assert!(
                read.contains(&format!("<Values><member>{value}</member></Values>")),
                "case {case:?}: expected the parameter value {value:?} to be reported: {read}"
            );
        }
        // `OnlySendByOwner` defaults to false, which the protocol may leave off the wire,
        // so the false rows accept either spelling of "not set".
        let reported_flag = elements(&read, "OnlySendByOwner");
        assert!(
            if row.only_send_by_owner {
                reported_flag == vec!["true".to_string()]
            } else {
                reported_flag.is_empty() || reported_flag == vec!["false".to_string()]
            },
            "case {case:?}: expected OnlySendByOwner {}, got {reported_flag:?}",
            row.only_send_by_owner
        );

        // The instants: the request was filed now, its record's last write is that same
        // filing, and it expires the published-free lifetime this emulator gives a filed
        // request after it.
        let created = reported_instant(case, &read, "CreateDate")
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreateDate: {read}"));
        assert_eq!(
            reported_instant(case, &read, "UpdatedTime"),
            Some(created),
            "case {case:?}: a request just filed was last updated when it was created: {read}"
        );
        assert_eq!(
            reported_instant(case, &read, "ExpirationTime"),
            Some(created + state::DELEGATION_REQUEST_LIFETIME_SECONDS),
            "case {case:?}: expected the expiry one lifetime after the filing: {read}"
        );
    }

    // One record per filing, under the identifiers the calls answered.
    let mut stored = held(&engine);
    stored.sort();
    let mut answered = minted;
    answered.sort();
    assert_eq!(
        stored, answered,
        "the account must hold exactly the requests the calls filed"
    );
}

/// RULE: `NotificationChannel` names the channel IAM publishes a delegation request's
/// updates to, and "at this time, only SNS topic ARNs are accepted for notification", so
/// a value that is not an SNS topic's ARN is one IAM cannot accept — refused with HTTP
/// 400 `ValidationError`, naming the member and quoting the value, and nothing is filed.
#[tokio::test]
async fn a_notification_channel_that_is_not_an_sns_topic_arn_is_refused() {
    let (router, engine) = router_with_engine();

    // Rows: the ways a value inside `notificationChannelType`'s own bounds (length
    // 2..400, pattern `^[a-zA-Z0-9:_.-]+$`) can still fail to be a topic's ARN, each with
    // the refusal its failure is stated by — a value that is no ARN, or an ARN naming
    // another resource.
    for (row, (case, channel, expected)) in [
        ("a bare topic name", "partner-updates", NOT_AN_ARN),
        (
            "an SNS ARN naming no resource",
            "arn:aws:sns:us-east-1:123456789012:",
            NOT_AN_ARN,
        ),
        (
            "an ARN missing its partition field",
            "arn:sns:us-east-1:123456789012:partner-updates",
            NOT_AN_ARN,
        ),
        (
            "a resource of another service",
            "arn:aws:sqs:us-east-1:123456789012:partner-queue",
            WRONG_RESOURCE_IN_ARN,
        ),
        (
            "a subscription of a topic, not the topic",
            "arn:aws:sns:us-east-1:123456789012:partner-updates:3f8a-1d2b",
            WRONG_RESOURCE_IN_ARN,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = create(
            &router,
            None,
            &Filing {
                notification_channel: channel.to_string(),
                ..filing(&format!("workflow-channel-{row}"))
            },
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: unexpected refusal wording: {body}"
        );
    }

    // Rows: topic ARNs the rule accepts, differing in partition, Region, account and
    // topic name — each filing one request and nothing else.
    for (row, (case, channel)) in [
        (
            "a topic in the commercial partition",
            "arn:aws:sns:us-east-1:123456789012:partner-updates",
        ),
        (
            "a topic in another partition and Region",
            "arn:aws-us-gov:sns:us-gov-west-1:210987654321:Partner_Updates.1",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = create(
            &router,
            None,
            &Filing {
                notification_channel: channel.to_string(),
                ..filing(&format!("workflow-good-channel-{row}"))
            },
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a topic ARN to be accepted, got {status}: {body}"
        );
    }
    assert_eq!(
        held(&engine).len(),
        2,
        "only the accepted filings may be held: {:?}",
        held(&engine)
    );
}

/// RULE: `Permissions` carries "the permissions to be delegated in this delegation
/// request" as the ARN of the policy template whose pre-registered content they are, so a
/// permission that names no such ARN delegates nothing — refused with HTTP 400
/// `ValidationError`, and nothing is filed.
#[tokio::test]
async fn a_permission_that_names_no_policy_template_arn_is_refused() {
    let (router, engine) = router_with_engine();

    // Rows: the ways a value inside `arnType`'s own bound (length 20..2048) can still
    // fail to name an IAM policy template, each with the refusal its failure is stated
    // by.
    for (row, (case, template, expected)) in [
        (
            "a value that is not an ARN",
            "not-an-arn-but-long-enough",
            NOT_AN_ARN,
        ),
        (
            "an ARN naming no resource",
            "arn:aws:iam::123456789012",
            NOT_AN_ARN,
        ),
        (
            "a resource of another service",
            "arn:aws:s3:::partner-drop-policy-template",
            WRONG_RESOURCE_IN_ARN,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = create(
            &router,
            None,
            &Filing {
                policy_template_arn: Some(template.to_string()),
                ..filing(&format!("workflow-template-{row}"))
            },
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: unexpected refusal wording: {body}"
        );
    }

    // A permission carrying parameters but no template at all names no content for those
    // parameters to scope, so it is refused the way an absent member is stated.
    let (status, body) = create(
        &router,
        None,
        &Filing {
            policy_template_arn: None,
            parameter: Some((
                "BucketName".to_string(),
                "partner-drop".to_string(),
                "string".to_string(),
            )),
            ..filing("workflow-template-absent")
        },
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected ValidationError, got {:?}: {body}",
        error_code(&body)
    );
    assert_eq!(
        element(&body, "Message"),
        NOT_AN_ARN,
        "unexpected refusal wording: {body}"
    );

    assert!(
        held(&engine).is_empty(),
        "a refused filing stores nothing, got {:?}",
        held(&engine)
    );
}

/// RULE: a policy template is pre-registered content published by a SERVICE rather than a
/// resource an account owns, so the ARN naming one carries no account number — a
/// `PolicyTemplateArn` whose account field spells one is refused HTTP 400 `ValidationError`
/// in AWS's fixed sentence and nothing is filed, while the forms a published template's ARN
/// does take (the publisher, or no account at all) are accepted.
#[tokio::test]
async fn a_policy_template_arn_carrying_an_account_number_is_refused() {
    let (router, engine) = router_with_engine();

    // The whole sentence AWS refuses such an ARN with: fixed text, quoting neither the
    // member nor the value. Spelled out here rather than read from a helper, so this suite
    // notices the wording drifting.
    const ACCOUNT_NUMBERS_REFUSED: &str =
        "Invalid ARN: account numbers are not acceptable for template ARNs!";

    // Rows: the ways an ARN can carry an account NUMBER — this account's own id, another
    // account's, in another partition, in another Region, and under another IAM resource
    // type — so the rule is read over the account field rather than over one ARN.
    let refused = [
        (
            "this account's own id",
            format!(
                "arn:aws:iam::{}:policy-template/PartnerDropReadWrite",
                account_id()
            ),
        ),
        (
            "another account's id",
            "arn:aws:iam::210987654321:policy-template/PartnerDrop".to_string(),
        ),
        (
            "an account number in another partition",
            "arn:aws-cn:iam::210987654321:policy-template/PartnerDrop".to_string(),
        ),
        (
            "an account number beside a Region",
            "arn:aws:iam:us-east-1:210987654321:policy-template/PartnerDrop".to_string(),
        ),
        (
            "an account number under another IAM resource type",
            "arn:aws:iam::210987654321:role-template/PartnerDeployment".to_string(),
        ),
    ];

    for (row, (case, template)) in refused.iter().enumerate() {
        let (status, body) = create(
            &router,
            None,
            &Filing {
                policy_template_arn: Some(template.clone()),
                ..filing(&format!("workflow-account-number-{row}"))
            },
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            ACCOUNT_NUMBERS_REFUSED,
            "case {case:?}: unexpected refusal wording: {body}"
        );
    }

    assert!(
        held(&engine).is_empty(),
        "a refused filing stores nothing, got {:?}",
        held(&engine)
    );

    // The control: the account field a published template's ARN DOES carry — the publisher
    // AWS writes there, and no account at all — names a template, so those filings are
    // served.
    let accepted = [
        (
            "the publisher in the account field",
            "arn:aws:iam::aws:policy-template/PartnerDropReadWrite",
        ),
        (
            "no account at all",
            "arn:aws:iam:::policy-template/PartnerDropReadWrite",
        ),
        (
            // A field that is not empty and is not a number names a publisher too.
            "a publisher in another partition",
            "arn:aws-cn:iam::aws:policy-template/PartnerDrop",
        ),
    ];
    for (row, (case, template)) in accepted.iter().enumerate() {
        let (status, body) = create(
            &router,
            None,
            &Filing {
                policy_template_arn: Some((*template).to_string()),
                ..filing(&format!("workflow-published-template-{row}"))
            },
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a published template's ARN to be accepted, got \
             {status}: {body}"
        );
    }
    assert_eq!(
        held(&engine).len(),
        accepted.len(),
        "only the accepted filings may be held: {:?}",
        held(&engine)
    );
}

/// RULE: "IAM maintains a uniqueness check on this workflow id for each request - if a
/// workflow id for an existing request is passed, this API call will fail"
/// (`CreateDelegationRequest$RequestorWorkflowId`), so the second filing under one
/// partner-side workflow id is refused with the modelled `EntityAlreadyExistsException`
/// (HTTP 409, awsQuery code `EntityAlreadyExists`) and the request filed first stands.
#[tokio::test]
async fn a_requestor_workflow_id_files_one_request_only() {
    let (router, engine) = router_with_engine();

    // Rows: workflow ids spanning what `requestorWorkflowIdType` admits (length 5..400,
    // pattern `^[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]+$`) — the published
    // minimum length, a spelling with spaces and punctuation, and the published maximum
    // length.
    for (row, (case, workflow)) in [
        ("the published minimum length", "wf-01".to_string()),
        ("spaces and punctuation", "partner workflow #7 (retry)".to_string()),
        ("the published maximum length", "w".repeat(400)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut row_filing = filing(&workflow);
        row_filing.description = format!("filing {row}");
        let (status, body) = create(&router, None, &row_filing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first filing to be served, got {status}: {body}"
        );
        let first = element(&body, "DelegationRequestId");

        // The same workflow id again, with everything else about the filing changed: what
        // refuses it is the workflow id alone.
        let repeat = Filing {
            description: "a second filing of the same partner workflow".to_string(),
            owner_account_id: Some(account_id().to_string()),
            session_duration: 43_200,
            ..filing(&workflow)
        };
        let (status, body) = create(&router, None, &repeat).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {case:?}: expected EntityAlreadyExists, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!(
                "Delegation request with requestor workflow id {workflow} already exists."
            ),
            "case {case:?}: unexpected refusal wording: {body}"
        );
        assert!(
            elements(&body, "DelegationRequestId").is_empty(),
            "case {case:?}: a refused filing mints no identifier, got {:?}",
            elements(&body, "DelegationRequestId")
        );

        // The request filed first is untouched: the read still reports the description
        // the first filing carried, not the second's.
        let (status, read) = get(&router, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first request to still be readable, got {status}: \
             {read}"
        );
        assert_eq!(
            element(&read, "Description"),
            format!("filing {row}"),
            "case {case:?}: the refused filing must not have overwritten the first: {read}"
        );
    }

    // One request per distinct workflow id, and nothing more.
    assert_eq!(
        held(&engine).len(),
        3,
        "each distinct workflow id files exactly one request: {:?}",
        held(&engine)
    );
}

/// RULE: an account holds a bounded number of delegation requests, so a filing that
/// would carry it past that quota "attempted to create resources beyond the current
/// Amazon Web Services account limits" and is refused with the modelled
/// `LimitExceededException` (HTTP 409, awsQuery code `LimitExceeded`), whose message
/// describes the limit exceeded — and nothing is minted or stored for it.
#[tokio::test]
async fn a_filing_past_the_account_quota_is_refused() {
    let (router, engine) = router_with_engine();

    // The account one filing short of its quota: the bound is taken against how many
    // requests the account HOLDS, so the records are put straight into the service's own
    // store rather than filed one call at a time.
    {
        let store = state::service_state(&*engine).expect("the IAM store");
        let mut held = store.lock();
        for seeded in 0..state::MAX_DELEGATION_REQUESTS_PER_ACCOUNT - 1 {
            let id = format!("seeded{seeded:0>10}");
            held.delegation_requests.insert(
                id.clone(),
                DelegationRequest {
                    delegation_request_id: Some(id),
                    ..DelegationRequest::default()
                },
            );
        }
    }

    // The last filing the quota admits is served.
    let (status, body) = create(&router, None, &filing("workflow-at-the-quota")).await;
    assert_eq!(
        status, 200,
        "the filing that reaches the quota must still be served, got {status}: {body}"
    );
    assert_eq!(
        held(&engine).len(),
        state::MAX_DELEGATION_REQUESTS_PER_ACCOUNT,
        "the account must now hold exactly its quota"
    );

    // The next one is not, and it leaves the account exactly as it was.
    let (status, body) = create(&router, None, &filing("workflow-past-the-quota")).await;
    assert_eq!(status, 409, "expected 409, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "expected LimitExceeded, got {:?}: {body}",
        error_code(&body)
    );
    assert_eq!(
        element(&body, "Message"),
        format!(
            "Cannot exceed quota for DelegationRequestsPerAccount: {}",
            state::MAX_DELEGATION_REQUESTS_PER_ACCOUNT
        ),
        "the refusal must describe the limit exceeded: {body}"
    );
    assert!(
        elements(&body, "DelegationRequestId").is_empty(),
        "a refused filing mints no identifier, got {:?}",
        elements(&body, "DelegationRequestId")
    );
    assert_eq!(
        held(&engine).len(),
        state::MAX_DELEGATION_REQUESTS_PER_ACCOUNT,
        "a refused filing stores nothing"
    );
}

/// RULE: IAM authorizes its own API, so a caller whose identity-based policies do not
/// permit `iam:CreateDelegationRequest` on the account's delegation requests is refused
/// 403 `AccessDenied` and nothing is filed.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    let (router, engine) = router_with_engine();

    // Rows: the two ways IAM refuses a call — an explicit `Deny` about it, and no `Allow`
    // for it at all.
    for (row, (case, policies, reason)) in [
        (
            "an explicit deny",
            vec![
                (
                    "may-file".to_string(),
                    statement("Allow", "iam:CreateDelegationRequest", "*"),
                ),
                (
                    "deny-filing".to_string(),
                    statement("Deny", "iam:CreateDelegationRequest", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:ListUsers", "*"),
            )],
            "because no identity-based policy allows the iam:CreateDelegationRequest action"
                .to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let user_name = format!("filer{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let (status, body) = create(
            &router,
            Some(&access_key_id),
            &filing(&format!("workflow-refused-{row}")),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
        // The decision is made on the account's delegation-request collection: the
        // request being filed has no identifier yet, because IAM mints it.
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "iam:CreateDelegationRequest",
            &format!("arn:aws:iam::{}:delegation-request/", account_id()),
            &reason,
        );
    }

    assert!(
        held(&engine).is_empty(),
        "a refused call files nothing, got {:?}",
        held(&engine)
    );

    // And a caller the policies DO permit files a request, so the refusals above are the
    // decision and not a blanket failure.
    let access_key_id = principal(
        &router,
        "a permitted caller",
        "permitted-filer",
        &[(
            "may-file".to_string(),
            statement("Allow", "iam:CreateDelegationRequest", "*"),
        )],
    )
    .await;
    let (status, body) = create(
        &router,
        Some(&access_key_id),
        &filing("workflow-permitted"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a permitted caller must be served, got {status}: {body}"
    );
    assert_eq!(
        held(&engine),
        vec![element(&body, "DelegationRequestId")],
        "the permitted call files exactly one request"
    );
}
