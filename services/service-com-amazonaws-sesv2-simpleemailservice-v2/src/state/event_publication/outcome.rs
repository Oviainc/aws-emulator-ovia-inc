// SPDX-License-Identifier: CC0-1.0

//! What the recipients of an accepted message do with it, and what the account's
//! suppression list gains because of it.
//!
//! Two rules decide both. They are stated here once, so the send path and the events it
//! publishes cannot disagree about one message:
//!
//! * The suppression list is CONSULTED before a message is handed on. An address on the
//!   suppression list that applies to the send is not attempted at all: the message is
//!   still accepted and still gets an id of its own, and that address produces a bounce
//!   whose `bounceSubType` names the list as the reason.
//! * The suppression list is WRITTEN by what a recipient does with a message. A hard
//!   bounce and a complaint add the recipient to the account-level list, carrying the
//!   reason and the message that caused it — for exactly the reasons the account says it
//!   suppresses addresses for (`SuppressionAttributes$SuppressedReasons`), or the ones
//!   the configuration set the send names overrides them with
//!   (`SuppressionOptions$SuppressedReasons`, "overrides the tenant or account
//!   suppression" settings for emails sent using that set).
//!
//! The recipients that react DETERMINISTICALLY are the mailbox simulator's: those
//! mailboxes belong to SES itself, so a message addressed to one is answered by SES rather
//! than handed to another mail provider, and the answer is the documented one for that
//! mailbox. A mailbox of that domain the simulator does not define is no mailbox at all,
//! so it answers as any unknown user does: a permanent failure for that one address.
//!
//! Every OTHER address is one no mail provider here answers for, so what it would really do
//! with the message is not decidable in this emulator: it is reported as having taken the
//! message, which is the outcome of an accepted send that nothing refused, and the reports
//! carry nothing a delivery attempt would have produced
//! ([`super::notification`] invents no processing time, reporting MTA or SMTP reply). An
//! address a client wants a bounce or a complaint for is the simulator's own, which is what
//! the simulator exists for.
//!
//! Nothing here reports a per-run value: a reaction is decided by the address and by the
//! account's own stored preferences, and the identifier an entry carries is derived from
//! the message and the address it is about.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::SystemTime;

/// The domain the SES mailbox simulator answers for. A message to one of its mailboxes is
/// answered by SES itself, which is what makes the answer deterministic.
pub const MAILBOX_SIMULATOR_DOMAIN: &str = "simulator.amazonses.com";

/// `SuppressionListReason`'s value for an address on the list because "a message sent to
/// that address results in a hard bounce" (see `crate::ops::types::SUPPRESSION_LIST_REASON_VALUES`).
pub const SUPPRESSION_REASON_BOUNCE: &str = "BOUNCE";

/// `SuppressionListReason`'s value for an address on the list because "a message sent to
/// that address results in a complaint".
pub const SUPPRESSION_REASON_COMPLAINT: &str = "COMPLAINT";

/// `bounceType`'s value for a failure the receiving side will never accept, so the sender
/// is not to retry it.
const BOUNCE_TYPE_PERMANENT: &str = "Permanent";

/// `bounceSubType`'s value for a permanent failure with no more specific cause than the
/// mailbox refusing the message.
const BOUNCE_SUBTYPE_GENERAL: &str = "General";

/// `bounceSubType`'s value for a message SES did not attempt at all because the address is
/// on the account-level suppression list.
const BOUNCE_SUBTYPE_ON_ACCOUNT_SUPPRESSION_LIST: &str = "OnAccountSuppressionList";

/// `bouncedRecipients[].action`: what was done with the message for that recipient.
const BOUNCE_ACTION_FAILED: &str = "failed";

/// The enhanced status code of a permanent failure for one mailbox: RFC 3463's 5.1.1, "bad
/// destination mailbox address".
const STATUS_BAD_MAILBOX: &str = "5.1.1";

/// The diagnostic an unknown mailbox answers with: the SMTP reply the receiving server
/// gave, as a bounce reports it.
const UNKNOWN_USER_DIAGNOSTIC_CODE: &str = "smtp; 550 5.1.1 user unknown";

/// The diagnostic a bounce carries when the suppression list, rather than a mail server,
/// is why the message was not sent — the sentence SES words it with, which names what the
/// sender has to do about it and carries no part of the message.
const ON_ACCOUNT_SUPPRESSION_LIST_DIAGNOSTIC_CODE: &str =
    "Amazon SES did not send the message to this address because it is on the suppression \
     list for your account. For more information about removing addresses from the \
     suppression list, see the Amazon SES Developer Guide at \
     https://docs.aws.amazon.com/ses/latest/dg/sending-email-suppression-list.html";

/// `complaintFeedbackType`: the kind of complaint the recipient's provider reported, under
/// the names RFC 5965 registers for a feedback report.
const COMPLAINT_FEEDBACK_TYPE_ABUSE: &str = "abuse";

/// How a message failed for one recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounce {
    /// `bounceType`: whether the failure is final.
    pub bounce_type: &'static str,
    /// `bounceSubType`: what kind of failure it is.
    pub bounce_sub_type: &'static str,
    /// `bouncedRecipients[].action`.
    pub action: &'static str,
    /// `bouncedRecipients[].status`: the enhanced status code.
    pub status: &'static str,
    /// `bouncedRecipients[].diagnosticCode`: what the failing side said.
    pub diagnostic_code: &'static str,
    /// Whether this bounce is one the account's suppression list is written from.
    ///
    /// A HARD bounce is: SES learned from the receiving side that the mailbox will never
    /// take mail, which is what the list exists to remember. A bounce SES produced from
    /// the list itself teaches nothing new, so it adds nothing.
    pub suppresses: bool,
}

impl Bounce {
    /// The permanent failure an unknown mailbox produces.
    pub fn unknown_user() -> Self {
        Self {
            bounce_type: BOUNCE_TYPE_PERMANENT,
            bounce_sub_type: BOUNCE_SUBTYPE_GENERAL,
            action: BOUNCE_ACTION_FAILED,
            status: STATUS_BAD_MAILBOX,
            diagnostic_code: UNKNOWN_USER_DIAGNOSTIC_CODE,
            suppresses: true,
        }
    }

    /// The failure an address on the account-level suppression list produces: SES made no
    /// attempt, so nothing on the receiving side was asked and nothing is learned.
    pub fn on_account_suppression_list() -> Self {
        Self {
            bounce_type: BOUNCE_TYPE_PERMANENT,
            bounce_sub_type: BOUNCE_SUBTYPE_ON_ACCOUNT_SUPPRESSION_LIST,
            action: BOUNCE_ACTION_FAILED,
            status: STATUS_BAD_MAILBOX,
            diagnostic_code: ON_ACCOUNT_SUPPRESSION_LIST_DIAGNOSTIC_CODE,
            suppresses: false,
        }
    }
}

/// What one recipient of a message did with it.
#[derive(Debug, Clone)]
pub struct RecipientOutcome {
    /// The recipient, exactly as the request spelled it.
    pub address: String,
    /// Whether the receiving side took the message for the mailbox.
    pub delivered: bool,
    /// The failure the recipient produced, when it produced one.
    pub bounce: Option<Bounce>,
    /// Whether the recipient reported the message as unwanted.
    pub complained: bool,
    /// `complaintFeedbackType`, for a recipient that complained.
    pub complaint_feedback_type: Option<&'static str>,
}

impl RecipientOutcome {
    /// A recipient the message reached.
    fn delivered(address: &str) -> Self {
        Self {
            address: address.to_string(),
            delivered: true,
            bounce: None,
            complained: false,
            complaint_feedback_type: None,
        }
    }

    /// A recipient the message failed for, and the failure.
    fn bounced(address: &str, bounce: Bounce) -> Self {
        Self {
            address: address.to_string(),
            delivered: false,
            bounce: Some(bounce),
            complained: false,
            complaint_feedback_type: None,
        }
    }

    /// A recipient the message reached and that then reported it as unwanted: a complaint
    /// is made ABOUT a message that arrived, so the delivery happened too.
    fn complained(address: &str) -> Self {
        Self {
            address: address.to_string(),
            delivered: true,
            bounce: None,
            complained: true,
            complaint_feedback_type: Some(COMPLAINT_FEEDBACK_TYPE_ABUSE),
        }
    }
}

/// What the mailbox simulator answers a message addressed to one of its mailboxes with.
///
/// The catalog is the documented one: `success` (the message is accepted), `bounce` (the
/// provider rejects it as an unknown user), `ooto` (it is accepted and an out-of-the-office
/// reply comes back), `complaint` (it is accepted and reported as spam) and
/// `suppressionlist` (SES answers as it does for an address on the account's suppression
/// list). Any other mailbox of that domain is not one the simulator defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulatedAnswer {
    /// The message is taken for the mailbox.
    Accepted,
    /// The message is rejected for a mailbox that does not exist.
    UnknownUser,
    /// The message is taken, and reported as unwanted.
    Complaint,
    /// SES answers as it does for an address on the account's suppression list.
    SuppressionList,
}

/// The mailboxes the simulator defines, with the answer each gives. The out-of-the-office
/// mailbox takes the message like the success mailbox does — the reply it then sends goes
/// back to the sender rather than being a failure of the send.
const SIMULATOR_MAILBOXES: [(&str, SimulatedAnswer); 5] = [
    ("success", SimulatedAnswer::Accepted),
    ("bounce", SimulatedAnswer::UnknownUser),
    ("ooto", SimulatedAnswer::Accepted),
    ("complaint", SimulatedAnswer::Complaint),
    ("suppressionlist", SimulatedAnswer::SuppressionList),
];

/// The answer the mailbox simulator gives `address`, or `None` when the address is not one
/// of its mailboxes at all.
///
/// The domain is compared the way DNS compares one ([`crate::state::domain_key`]) and the
/// local part the way a mailbox name is: the simulator's mailbox names are ASCII, and a
/// local part belongs to the receiving side, which here is SES.
fn simulated_answer(address: &str) -> Option<SimulatedAnswer> {
    let (local, domain) = crate::state::addr_spec(address).rsplit_once('@')?;
    if crate::state::domain_key(domain) != MAILBOX_SIMULATOR_DOMAIN {
        return None;
    }
    let mailbox = local.to_ascii_lowercase();
    // A mailbox the simulator does not define is not a mailbox: it answers as any unknown
    // user does.
    Some(
        SIMULATOR_MAILBOXES
            .iter()
            .find(|(name, _)| *name == mailbox)
            .map_or(SimulatedAnswer::UnknownUser, |(_, answer)| *answer),
    )
}

/// What each recipient of `message` did with it, writing to the account-level suppression
/// list whatever the reactions put there.
///
/// Called once per accepted message, under the store lock: the list decides the outcome of
/// a recipient and a recipient's outcome writes the list, so both happen in one pass over
/// the message.
pub fn outcomes(
    store: &mut crate::state::ComAmazonawsSesv2SimpleemailserviceV2State,
    message: &crate::state::SentMessage,
    now: SystemTime,
) -> Vec<RecipientOutcome> {
    let reasons = suppressed_reasons(store, message.configuration_set_name.as_deref());
    let mut outcomes = Vec::new();
    for address in message.recipients() {
        let outcome = outcome_of(store, address);
        // What the recipient did is what the list is written from, and only for the
        // reasons the account (or the set that overrode it) suppresses addresses for.
        let reason = if outcome.bounce.as_ref().is_some_and(|bounce| bounce.suppresses) {
            Some(SUPPRESSION_REASON_BOUNCE)
        } else if outcome.complained {
            Some(SUPPRESSION_REASON_COMPLAINT)
        } else {
            None
        };
        if let Some(reason) = reason.filter(|reason| {
            reasons
                .iter()
                .any(|suppressed| suppressed.eq_ignore_ascii_case(reason))
        }) {
            suppress(store, &outcome.address, reason, message, now);
        }
        outcomes.push(outcome);
    }
    outcomes
}

/// What the recipient `address` does with a message, before anything is written down.
fn outcome_of(
    store: &crate::state::ComAmazonawsSesv2SimpleemailserviceV2State,
    address: &str,
) -> RecipientOutcome {
    // The list is consulted first: an address on it is not handed on at all, whatever the
    // receiving side would have done with the message.
    if store
        .suppressed_destinations
        .contains_key(&crate::state::suppression_key(None, address))
    {
        return RecipientOutcome::bounced(address, Bounce::on_account_suppression_list());
    }
    match simulated_answer(address) {
        Some(SimulatedAnswer::UnknownUser) => {
            RecipientOutcome::bounced(address, Bounce::unknown_user())
        }
        Some(SimulatedAnswer::Complaint) => RecipientOutcome::complained(address),
        Some(SimulatedAnswer::SuppressionList) => {
            RecipientOutcome::bounced(address, Bounce::on_account_suppression_list())
        }
        // The simulator's accepting mailbox, and every address outside the simulator,
        // which is handed on and taken for the mailbox.
        Some(SimulatedAnswer::Accepted) | None => RecipientOutcome::delivered(address),
    }
}

/// The reasons an address is added to the account-level suppression list automatically for
/// a send through `configuration_set_name`.
///
/// A configuration set's `SuppressionOptions$SuppressedReasons` is an OVERRIDE of the
/// account's, so a set that states one is followed and a set that states none leaves the
/// account's preferences in force.
fn suppressed_reasons(
    store: &crate::state::ComAmazonawsSesv2SimpleemailserviceV2State,
    configuration_set_name: Option<&str>,
) -> Vec<String> {
    configuration_set_name
        .and_then(|name| store.configuration_sets.get(name))
        .and_then(|set| set.suppression_options.as_ref())
        .and_then(|options| options.suppressed_reasons.clone())
        .unwrap_or_else(|| store.account.suppressed_reasons.clone())
}

/// Put `address` on the account-level suppression list for `reason`, caused by `message`.
///
/// The entry carries what `SuppressedDestinationAttributes` documents: the message that
/// caused it and the identifier generated for the addition. A list already holding as many
/// addresses as it may takes nothing, which is the store's own ceiling and not something a
/// send reports: the message was accepted either way.
fn suppress(
    store: &mut crate::state::ComAmazonawsSesv2SimpleemailserviceV2State,
    address: &str,
    reason: &str,
    message: &crate::state::SentMessage,
    now: SystemTime,
) {
    let record = crate::state::SuppressedDestinationRecord {
        email_address: address.to_string(),
        reason: reason.to_string(),
        last_update_time: now,
        message_id: Some(message.message_id.clone()),
        feedback_id: Some(feedback_id(&message.message_id, address)),
        tenant_name: None,
    };
    store.suppress_destination(crate::state::suppression_key(None, address), record);
}

/// The identifier generated for the feedback one recipient of one message produced.
///
/// `FeedbackId` is documented only as "a unique identifier that's generated when an email
/// address is added to the suppression list", and the `feedbackId` a published bounce or
/// complaint carries identifies that same report, so both read it here. The value is
/// derived from the message and the address it is about: unique per report, and stable for
/// as long as the report is readable, which is what a client correlating the two depends
/// on.
///
/// The digest is `std`'s, whose output is explicitly NOT guaranteed to be the same across
/// Rust releases — nothing relies on that: the value is computed from the stored report, so
/// no caller and no test may assert a literal one.
pub fn feedback_id(message_id: &str, address: &str) -> String {
    let mut digest = String::with_capacity(32);
    for salt in [0_u64, u64::from_be_bytes(*b"Feedback")] {
        let mut hasher = DefaultHasher::new();
        salt.hash(&mut hasher);
        message_id.hash(&mut hasher);
        address.hash(&mut hasher);
        digest.push_str(&format!("{:016x}", hasher.finish()));
    }
    digest
}
