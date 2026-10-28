// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bound every refusal of this service keeps: a refusal is a
//! MODELLED response shape, so a request that stays inside its own constraints can never
//! make this service answer outside them.
//!
//! Every error this model declares gives its `Message` member `@length(max: 1000)`
//! (`smithy_query(query="shape",shape="ValidationException,ConflictException,
//! ResourceNotFoundException")`), while several members a refusal quotes back are longer
//! than that on their own — `QualifiedPolicyArn` is `@length(max: 1224)`,
//! `ApprovalTeamArn` `@length(20..2048)`, and `ApprovalTeamRequestApprover$
//! PrimaryIdentitySourceArn` and `Filter$Value` are `@length(max: 1000)`, the WHOLE
//! length a message may have. So every message built by interpolating a value the request
//! carried is held to the member's bound, and cut on a character boundary when it would
//! pass it.
//!
//! The table spans the class the rule generalises over: which operation answers
//! (`CreateApprovalTeam`, `UpdateApprovalTeam`, `ListSessions`), which member is quoted
//! back, and — the thing the bound keys on — how long that value is: an ordinary short
//! value (the message names it in full and is NOT cut), a value at its member's published
//! maximum, and a value just short of and just past the length that makes the message
//! overflow.
//!
//! `GetPolicyVersion` rows (`ResourceNotFoundException` 404, beside the
//! `ValidationException` 400 the other rows carry) are the CONTROL: a refusal this service
//! states with a fixed sentence quotes nothing the request carried, so its message is the
//! same length whatever the value was — the bound is a property of the messages BUILT from
//! a request, not of every refusal.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the error envelope
//! and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, expected_arn, text, Harness, REGION};

/// The policy a fresh store seeds (`state::seeded_policy_catalog`): category and name.
const SEEDED_POLICY: (&str, &str) = ("IdentityCenter", "MPAApprovalTeamPolicy");

/// The team the update rows patch.
const TEAM_NAME: &str = "bounded-refusal-team";

/// The qualified reference to the seeded policy's default version — an attachment every
/// row that is not ABOUT an unattachable policy carries, since a reference the catalog
/// cannot resolve is refused first.
fn attachable_policy() -> String {
    format!(
        "{}/{}",
        state::policy_arn(SEEDED_POLICY.0, SEEDED_POLICY.1),
        state::DEFAULT_POLICY_VERSION
    )
}

/// A reference `total` characters long that satisfies `QualifiedPolicyArn`'s modelled
/// pattern `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`
/// and names no policy the catalog holds.
///
/// The pattern bounds each of the two segments at 1023 characters, so a long reference
/// fills the first one and spends the rest on the second.
fn absent_policy_reference(total: usize) -> String {
    const PREFIX: &str = "arn:aws:mpa:::aws:policy/";
    const SUFFIX: &str = "/1";
    let segments = total - PREFIX.len() - SUFFIX.len() - 1;
    let name = segments.saturating_sub(1023).max(1);
    let category = segments - name;
    assert!(
        (1..=1023).contains(&category) && (1..=1023).contains(&name),
        "a {total}-character reference does not fit the pattern's two {{1,1023}} segments"
    );
    let reference = format!(
        "{PREFIX}{}/{}{SUFFIX}",
        "c".repeat(category),
        "n".repeat(name)
    );
    assert_eq!(
        reference.len(),
        total,
        "the reference built for {total} characters is {} long",
        reference.len()
    );
    reference
}

/// An identity-source ARN `total` characters long. `PrimaryIdentitySourceArn` is a plain
/// string with `@length(max: 1000)` and no pattern, so a conforming value is this ARN
/// padded in its resource segment.
fn identity_source_arn(total: usize) -> String {
    let base = format!(
        "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-",
        engine::emulator_account_id()
    );
    assert!(
        total > base.len(),
        "an identity source ARN cannot be {total} characters long"
    );
    format!("{base}{}", "0".repeat(total - base.len()))
}

/// An identity id `total` characters long. `IdentityId` is `@length(1..100)` with no
/// pattern.
fn identity_id(total: usize) -> String {
    format!("identity-{}", "0".repeat(total - "identity-".len()))
}

/// A `CreateApprovalTeam` body every modelled constraint accepts.
fn create_body(name: &str, approvers: &[(String, String)], policies: &[String]) -> Value {
    json!({
        "Name": name,
        "Description": "refusal message bound",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": approvers
            .iter()
            .map(|(id, source)| json!({
                "PrimaryIdentityId": id,
                "PrimaryIdentitySourceArn": source,
            }))
            .collect::<Vec<_>>(),
        "Policies": policies
            .iter()
            .map(|arn| json!({ "PolicyArn": arn }))
            .collect::<Vec<_>>(),
    })
}

/// The refusal one row drives, and the member it makes the service quote back.
enum Probe {
    /// `CreateApprovalTeam$Policies` naming a policy version the catalog cannot resolve.
    UnattachablePolicy(String),
    /// `CreateApprovalTeam$Approvers` naming one identity twice.
    CreateRepeatedIdentity { id: String, source: String },
    /// `UpdateApprovalTeam$Approvers` naming one identity twice — the same refusal, from
    /// the other write path that takes an approver list.
    UpdateRepeatedIdentity { id: String, source: String },
    /// `GetPolicyVersion` for a reference the catalog holds no version for: the control,
    /// whose message is this service's fixed not-found sentence and therefore quotes none
    /// of the reference.
    AbsentPolicyVersion(String),
    /// `ListSessions` filtering `SessionStatus` on a value the enum does not declare.
    UnknownStatusFilter(String),
}

impl Probe {
    /// The sentence the rule builds for this refusal BEFORE the bound is applied: the
    /// wording is part of the contract, so each row pins it, and the bound is the only
    /// thing that may shorten it.
    fn unbounded_message(&self) -> String {
        match self {
            Self::UnattachablePolicy(reference) => format!(
                "Policies names a policy version that does not exist: {reference}. Attach a \
                 policy version ListPolicies and ListPolicyVersions report."
            ),
            Self::CreateRepeatedIdentity { id, source }
            | Self::UpdateRepeatedIdentity { id, source } => format!(
                "Approvers names the same identity more than once ({id} from {source}); each \
                 approver must be a distinct identity."
            ),
            Self::AbsentPolicyVersion(_) => state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
            Self::UnknownStatusFilter(value) => format!(
                "Filters[0].Value '{value}' is not a status a session can be in; the \
                 SessionStatus filter accepts {}.",
                model::SESSION_STATUS_VALUES.join(" | ")
            ),
        }
    }

    /// The HTTP status and error code this refusal is declared with.
    fn expected_error(&self) -> (u16, &'static str) {
        match self {
            Self::AbsentPolicyVersion(_) => (404, "ResourceNotFoundException"),
            _ => (400, "ValidationException"),
        }
    }

    /// Drive the refusal over the wire against a store of this row's own.
    async fn refuse(&self, harness: &Harness) -> (u16, Value) {
        let ordinary_approver = (identity_id(13), identity_source_arn(60));
        match self {
            Self::UnattachablePolicy(reference) => {
                harness
                    .create(&create_body(
                        TEAM_NAME,
                        &[ordinary_approver],
                        ::std::slice::from_ref(reference),
                    ))
                    .await
            }
            Self::CreateRepeatedIdentity { id, source } => {
                let repeated = (id.clone(), source.clone());
                harness
                    .create(&create_body(
                        TEAM_NAME,
                        &[repeated.clone(), repeated],
                        &[attachable_policy()],
                    ))
                    .await
            }
            Self::UpdateRepeatedIdentity { id, source } => {
                let (status, created) = harness
                    .create(&create_body(
                        TEAM_NAME,
                        &[ordinary_approver],
                        &[attachable_policy()],
                    ))
                    .await;
                assert_eq!(
                    status, 200,
                    "the team an update row patches was not created: {created:?}"
                );
                harness
                    .update(
                        &expected_arn(TEAM_NAME),
                        &json!({
                            "Approvers": [
                                { "PrimaryIdentityId": id, "PrimaryIdentitySourceArn": source },
                                { "PrimaryIdentityId": id, "PrimaryIdentitySourceArn": source },
                            ]
                        }),
                    )
                    .await
            }
            Self::AbsentPolicyVersion(reference) => {
                harness
                    .call(
                        "GET",
                        &format!("/policy-versions/{}", encode_label(reference)),
                        None,
                    )
                    .await
            }
            Self::UnknownStatusFilter(value) => {
                harness
                    .call(
                        "POST",
                        &format!(
                            "/approval-teams/{}/sessions/?List",
                            encode_label(&expected_arn("no-such-team"))
                        ),
                        Some(json!({
                            "Filters": [{
                                "FieldName": "SessionStatus",
                                "Operator": "EQ",
                                "Value": value,
                            }]
                        })),
                    )
                    .await
            }
        }
    }
}

/// The table: the same rule, over four operations, two error shapes, and values from an
/// ordinary length up to each member's published maximum, plus the control rows whose
/// refusal is a fixed sentence that quotes nothing.
fn cases() -> Vec<(String, Probe)> {
    // The length of reference that lands a refusal exactly ON the bound, derived from the
    // sentence the rule words rather than written out, so the rows either side of the
    // bound cannot drift from the wording.
    let around_the_bound = state::MAX_EXCEPTION_MESSAGE
        - Probe::UnattachablePolicy(String::new())
            .unbounded_message()
            .len();

    vec![
        (
            "an unattachable policy reference of ordinary length".to_string(),
            Probe::UnattachablePolicy(absent_policy_reference(70)),
        ),
        (
            "an unattachable policy reference whose message lands on the bound".to_string(),
            Probe::UnattachablePolicy(absent_policy_reference(around_the_bound)),
        ),
        (
            "an unattachable policy reference one character past the bound".to_string(),
            Probe::UnattachablePolicy(absent_policy_reference(around_the_bound + 1)),
        ),
        (
            // `QualifiedPolicyArn` is `@length(max: 1224)` — the longest reference a
            // conforming request can carry at all.
            "an unattachable policy reference at QualifiedPolicyArn's maximum".to_string(),
            Probe::UnattachablePolicy(absent_policy_reference(1224)),
        ),
        (
            "a repeated identity of ordinary length".to_string(),
            Probe::CreateRepeatedIdentity {
                id: identity_id(13),
                source: identity_source_arn(60),
            },
        ),
        (
            // `PrimaryIdentitySourceArn` is `@length(max: 1000)`, the whole bound of the
            // message it goes into.
            "a repeated identity whose source ARN is at its maximum".to_string(),
            Probe::CreateRepeatedIdentity {
                id: identity_id(13),
                source: identity_source_arn(1000),
            },
        ),
        (
            // `IdentityId` is `@length(1..100)` — both quoted members at their maximum.
            "a repeated identity with both members at their maximum".to_string(),
            Probe::CreateRepeatedIdentity {
                id: identity_id(100),
                source: identity_source_arn(1000),
            },
        ),
        (
            "an update repeating an identity of ordinary length".to_string(),
            Probe::UpdateRepeatedIdentity {
                id: identity_id(13),
                source: identity_source_arn(60),
            },
        ),
        (
            "an update repeating an identity with both members at their maximum".to_string(),
            Probe::UpdateRepeatedIdentity {
                id: identity_id(100),
                source: identity_source_arn(1000),
            },
        ),
        (
            "the control: a policy version reference of ordinary length".to_string(),
            Probe::AbsentPolicyVersion(absent_policy_reference(70)),
        ),
        (
            "the control: a policy version reference at QualifiedPolicyArn's maximum"
                .to_string(),
            Probe::AbsentPolicyVersion(absent_policy_reference(1224)),
        ),
        (
            "a session status filter value of ordinary length".to_string(),
            Probe::UnknownStatusFilter("NOT_A_STATUS".to_string()),
        ),
        (
            // `Filter$Value` is `@length(max: 1000)`.
            "a session status filter value at Filter$Value's maximum".to_string(),
            Probe::UnknownStatusFilter("v".repeat(1000)),
        ),
    ]
}

/// The message a refusal carries is the sentence the rule words, cut to the bound its own
/// `Message` member declares and to nothing shorter: a short value is named in full, and a
/// value long enough to overflow is cut at exactly the bound.
#[tokio::test]
async fn every_refusal_that_quotes_a_request_value_keeps_its_modelled_message_bound() {
    let mut cut = 0_usize;
    let mut uncut = 0_usize;

    for (case, probe) in cases() {
        // A store of its own per row: several rows create the same team.
        let harness = Harness::new();
        let (status, body) = probe.refuse(&harness).await;
        let (expected_status, expected_code) = probe.expected_error();
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code}, got {body:?}"
        );

        let message = text(&body, "Message");
        let full = probe.unbounded_message();
        let expected = if full.len() <= state::MAX_EXCEPTION_MESSAGE {
            uncut += 1;
            full.clone()
        } else {
            cut += 1;
            full[..state::MAX_EXCEPTION_MESSAGE].to_string()
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
        assert!(
            message.len() <= state::MAX_EXCEPTION_MESSAGE,
            "case {case:?}: the refusal is {} characters, past the modelled bound of {}: \
             {message:?}",
            message.len(),
            state::MAX_EXCEPTION_MESSAGE
        );
        assert!(
            full.starts_with(&message),
            "case {case:?}: the refusal is not the rule's sentence cut to the bound: \
             {message:?} against {full:?}"
        );
    }

    // The table is only a test of the bound if it has rows on both sides of it.
    assert!(
        cut > 0 && uncut > 0,
        "the table drove {cut} refusal(s) past the bound and {uncut} within it; it must span \
         both"
    );
}

/// The cut itself, over text the wire can carry: the bound holds whatever the value is
/// made of, the result is always a whole-character prefix of the sentence, and a sentence
/// already inside the bound is handed back untouched.
///
/// `Filter$Value` and `PrimaryIdentitySourceArn` are plain strings with no `@pattern`, so
/// a conforming request can carry any UTF-8 in them — including characters whose encoding
/// straddles the byte the bound falls on.
#[test]
fn the_bound_cuts_on_a_character_boundary_whatever_the_value_is_made_of() {
    let fillers = [
        ("ASCII", "a"),
        ("two-byte characters", "é"),
        ("three-byte characters", "✓"),
        ("four-byte characters", "𝄞"),
        ("a mix of widths", "aé✓𝄞"),
    ];
    // Lengths either side of the bound, in CHARACTERS, so the byte the cut falls on lands
    // inside a character for the wider fillers.
    let repeats = [0, 1, 100, 249, 250, 333, 499, 500, 999, 1000, 1001, 2000];

    for (filler_case, filler) in fillers {
        for repeat in repeats {
            let case = format!("{filler_case} repeated {repeat} time(s)");
            let message = filler.repeat(repeat);
            let bounded = state::bounded_message(message.clone());
            assert!(
                bounded.len() <= state::MAX_EXCEPTION_MESSAGE,
                "case {case:?}: the bounded message is {} bytes, past the modelled bound of {}",
                bounded.len(),
                state::MAX_EXCEPTION_MESSAGE
            );
            assert!(
                message.starts_with(&bounded),
                "case {case:?}: the bounded message is not a prefix of the one built: \
                 {bounded:?}"
            );
            if message.len() <= state::MAX_EXCEPTION_MESSAGE {
                assert_eq!(
                    bounded, message,
                    "case {case:?}: a message inside the bound was shortened to {bounded:?}"
                );
            } else {
                // Cut on a character boundary, so at most the width of one character is
                // given up: a cut that fell mid-character would not be valid UTF-8 at all,
                // and one that gave up more would be cutting further than the bound asks.
                let widest = filler.chars().map(char::len_utf8).max().unwrap_or(1);
                assert!(
                    bounded.len() > state::MAX_EXCEPTION_MESSAGE - widest,
                    "case {case:?}: the cut gave up more than one character: {} bytes for a \
                     bound of {}",
                    bounded.len(),
                    state::MAX_EXCEPTION_MESSAGE
                );
            }
        }
    }
}
