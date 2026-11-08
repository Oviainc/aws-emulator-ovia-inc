// SPDX-License-Identifier: CC0-1.0

//! Queue permissions: the bounds a queue policy is held to and the wording of every
//! refusal a grant or a revoke answers with.
//!
//! A permission is not a record of its own: `AddPermission` "generates a policy for you",
//! so a granted permission IS a statement of the queue's `Policy` attribute — the same
//! attribute `SetQueueAttributes` uploads a policy into and `GetQueueAttributes` reports.
//! Keeping it there is what makes a grant observable by every later reader, and lets
//! `RemovePermission` "revoke any permissions in the queue policy that matches the
//! specified Label parameter" whether the statement came from a grant or from an upload.
//! The document and the statement are assembled and taken apart by the
//! [`super::Queue`] methods; the bounds and the wording of every refusal live here. The
//! statement SHAPE is the one every service whose `AddPermission` generates a policy
//! writes, and Amazon SNS's own state assembles its own copy of it; the two are kept apart
//! deliberately, because each carries the statement in its own representation — this one as
//! the JSON a `Policy` attribute string holds, that one as a typed record with the
//! condition and passthrough members a topic policy keeps — so a change to the shape has to
//! be made in both.

use super::*;

/// The policy language version a generated queue policy declares: Amazon SQS synthesises
/// its default queue policy in its own long-standing policy language version, which is not
/// the one a caller writing a policy today would use.
pub const POLICY_VERSION: &str = "2008-10-17";

/// The `Version` member of a policy document.
pub const POLICY_VERSION_MEMBER: &str = "Version";

/// The `Id` member of a policy document.
pub const POLICY_ID_MEMBER: &str = "Id";

/// The `Statement` member of a policy document: "an array of individual statements".
pub const POLICY_STATEMENT_MEMBER: &str = "Statement";

/// The `Sid` member of a policy statement, which carries the `Label` the permission was
/// granted under.
pub const POLICY_SID_MEMBER: &str = "Sid";

/// What the `Id` of a generated queue policy is: the queue's ARN followed by this.
pub const DEFAULT_POLICY_ID_SUFFIX: &str = "/SQSDefaultPolicy";

/// The prefix a granted action carries in the generated policy, qualifying the action
/// name with the service it belongs to.
pub const POLICY_ACTION_PREFIX: &str = "SQS:";

/// The action name that stands for every action: "Valid values: the name of any action
/// or *" (the `Actions` member's own documentation).
pub const ANY_ACTION: &str = "*";

/// The longest `Label` a permission may carry: "Maximum 80 characters"
/// (`AddPermissionRequest$Label`).
pub const MAX_PERMISSION_LABEL_LENGTH: usize = 80;

/// How many digits an Amazon Web Services account number has, which is the only
/// principal form this call takes: "Amazon SQS AddPermission does not support adding a
/// non-account principal."
pub const ACCOUNT_NUMBER_DIGITS: usize = 12;

/// How many statements one queue policy may carry — therefore how many permissions a
/// queue can have, since each grant adds one statement. The ceiling AWS publishes for a
/// queue policy ("The maximum limit is 8,192 bytes, 20 statements, 50 principals, or 10
/// conditions"), and what `OverLimit` means for this call: "AddPermission returns this
/// error if the maximum number of permissions for the queue is reached".
pub const MAX_POLICY_STATEMENTS: usize = 20;

/// How many principals one statement may name, from the same published ceiling.
pub const MAX_PRINCIPALS_PER_STATEMENT: usize = 50;

/// How many actions one statement may allow: "An Amazon SQS policy can have a maximum of
/// seven actions per statement" (`AddPermission`'s own documentation).
pub const MAX_ACTIONS_PER_STATEMENT: usize = 7;

/// The parameter a label is supplied for, as the request carries it.
pub const PERMISSION_LABEL_PARAMETER: &str = "Label";

/// The parameter a principal is supplied for, as the request carries it: the
/// `AWSAccountIds` list is flattened to repeated `AWSAccountId` entries on the wire
/// (`xmlName: AWSAccountId`, `xmlFlattened`).
pub const PRINCIPAL_PARAMETER: &str = "AWSAccountId";

/// The parameter an action is supplied for, as the request carries it: the `Actions`
/// list is flattened to repeated `ActionName` entries on the wire (`xmlName: ActionName`,
/// `xmlFlattened`).
pub const ACTION_PARAMETER: &str = "ActionName";

/// The reason a grant is refused for a label the queue's policy already grants: a label
/// is "the unique identification of the permission you're setting", so the policy cannot
/// carry two.
pub const LABEL_ALREADY_EXISTS: &str = "Already exists.";

/// The reason a revoke is refused for a label the queue's policy does not grant: there
/// is no statement for it to revoke.
pub const LABEL_NOT_FOUND: &str = "can't find label on existing policy.";

/// The action names a permission may allow, which is "the name of any action" of this
/// service (`AddPermissionRequest$Actions`), beside [`ANY_ACTION`].
///
/// Source: `smithy_query(query="list_operations",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`
/// — the operations bound to this service, in the order it reports them.
pub const ACTION_NAMES: &[&str] = &[
    "AddPermission",
    "CancelMessageMoveTask",
    "ChangeMessageVisibility",
    "ChangeMessageVisibilityBatch",
    "CreateQueue",
    "DeleteMessage",
    "DeleteMessageBatch",
    "DeleteQueue",
    "GetQueueAttributes",
    "GetQueueUrl",
    "ListDeadLetterSourceQueues",
    "ListMessageMoveTasks",
    "ListQueueTags",
    "ListQueues",
    "PurgeQueue",
    "ReceiveMessage",
    "RemovePermission",
    "SendMessage",
    "SendMessageBatch",
    "SetQueueAttributes",
    "StartMessageMoveTask",
    "TagQueue",
    "UntagQueue",
];

/// How a refusal states that a grant would take the queue's policy past the statements one
/// policy may carry.
///
/// `would_hold` is the number of statements the REQUEST would leave the policy with — the
/// count it asked for, not the count the policy holds today — and the ceiling is the
/// published one ([`MAX_POLICY_STATEMENTS`]). The fixed text is everything outside those two
/// numbers, down to the space the sentence ends with; the queue is not named, because the
/// pair of numbers is the whole of what the caller has to fix the request from.
pub fn policy_statement_limit_message(would_hold: usize) -> String {
    format!("{would_hold} Statements were found, maximum allowed is {MAX_POLICY_STATEMENTS}. ")
}

/// Why a permission cannot be granted, as the text of the refusal a caller has to fix
/// the request from. The two kinds answer under different error codes, which is why they
/// are told apart here rather than in a handler.
#[derive(Debug)]
pub enum PermissionRefusal {
    /// A parameter's value is not one the call accepts.
    Parameter(String),
    /// A published limit on what a queue policy may hold.
    Limit(String),
}

/// Why `label` is not a `Label` this service accepts, or `None` when it is.
///
/// The rule is the member's own: "Maximum 80 characters. Allowed characters include
/// alphanumeric characters, hyphens (-), and underscores (_)."
pub fn permission_label_refusal(label: &str) -> Option<String> {
    let length = label.chars().count();
    let well_formed = (1..=MAX_PERMISSION_LABEL_LENGTH).contains(&length)
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if well_formed {
        return None;
    }
    // The rule, not which half of it this label broke: a caller fixes the label against
    // the whole rule either way. The value is echoed only where it is already known to
    // be inside its length bound, so a refusal never hands an unbounded string back.
    let reason = format!(
        "Label can only include alphanumeric characters, hyphens, or underscores. 1 to \
         {MAX_PERMISSION_LABEL_LENGTH} in length"
    );
    Some(if length > MAX_PERMISSION_LABEL_LENGTH {
        invalid_parameter_message_without_value(PERMISSION_LABEL_PARAMETER, &reason)
    } else {
        invalid_parameter_message(label, PERMISSION_LABEL_PARAMETER, &reason)
    })
}

/// Why the principals a grant names cannot receive it, or `None` when they can.
///
/// Each one is "the Amazon Web Services account number of a principal", and nothing
/// else: "Amazon SQS AddPermission does not support adding a non-account principal."
pub(super) fn principals_refusal(account_numbers: &[String]) -> Option<PermissionRefusal> {
    for account_number in account_numbers {
        let account = account_number.chars().count() == ACCOUNT_NUMBER_DIGITS
            && account_number.chars().all(|c| c.is_ascii_digit());
        if !account {
            let reason = "Amazon SQS AddPermission does not support adding a non-account \
                          principal.";
            return Some(PermissionRefusal::Parameter(
                if account_number.chars().count() > ACCOUNT_NUMBER_DIGITS {
                    invalid_parameter_message_without_value(PRINCIPAL_PARAMETER, reason)
                } else {
                    invalid_parameter_message(account_number, PRINCIPAL_PARAMETER, reason)
                },
            ));
        }
    }
    (account_numbers.len() > MAX_PRINCIPALS_PER_STATEMENT).then(|| {
        PermissionRefusal::Limit(format!(
            "An Amazon SQS policy statement can name a maximum of \
             {MAX_PRINCIPALS_PER_STATEMENT} principals, and this one names {}.",
            account_numbers.len()
        ))
    })
}

/// Why the actions a grant allows cannot be allowed, or `None` when they can.
///
/// The per-statement ceiling is refused as a PARAMETER fault rather than as a limit: the
/// condition `OverLimit` is documented for on this call is the other one — "AddPermission
/// returns this error if the maximum number of permissions for the queue is reached" —
/// and a request naming more actions than one statement may allow is a request this call
/// could never serve, whatever the queue already carries.
pub(super) fn actions_refusal(actions: &[String]) -> Option<PermissionRefusal> {
    for action in actions {
        if action != ANY_ACTION && !authorization::policy_action_names().contains(&action.as_str()) {
            // The action is named back QUALIFIED with the service it was looked up in,
            // the way the statement a grant would have added names it, because that is
            // the name no action of this service answers to.
            let qualified = format!("{POLICY_ACTION_PREFIX}{action}");
            return Some(PermissionRefusal::Parameter(unknown_action_message(
                &qualified,
            )));
        }
    }
    (actions.len() > MAX_ACTIONS_PER_STATEMENT).then(|| {
        PermissionRefusal::Parameter(invalid_parameter_message_without_value(
            ACTION_PARAMETER,
            &format!(
                "An Amazon SQS policy can have a maximum of {MAX_ACTIONS_PER_STATEMENT} actions \
                 per statement, and this one has {}.",
                actions.len()
            ),
        ))
    })
}

/// The reason a grant is refused for an action name this service has no action for: the
/// caller is pointed at the service's own interface description, which is where the names
/// a grant may allow are published.
pub(super) const UNKNOWN_ACTION_REASON: &str =
    "Please refer to the appropriate WSDL for a list of valid actions.";

/// How a refusal states that `named` is no action name this service accepts where an
/// action name was supplied — in a grant's `ActionName`, or as the `Action` of a statement
/// of a queue policy, which is the same parameter and the same published set
/// ([`authorization::policy_action_names`]).
///
/// The value is echoed only while it is no longer than a qualified name of this service
/// can be, so a refusal never hands an unbounded string back.
pub fn unknown_action_message(named: &str) -> String {
    if named.chars().count() > POLICY_ACTION_PREFIX.len() + MAX_ACTION_NAME_LENGTH {
        return invalid_parameter_message_without_value(ACTION_PARAMETER, UNKNOWN_ACTION_REASON);
    }
    invalid_parameter_message(named, ACTION_PARAMETER, UNKNOWN_ACTION_REASON)
}

/// The longest action name a refusal echoes back: the longest name this service has, so
/// a value no longer than any action name is reported and anything longer is not.
pub(super) const MAX_ACTION_NAME_LENGTH: usize = longest_action_name();

/// The length of the longest entry of [`ACTION_NAMES`].
pub(super) const fn longest_action_name() -> usize {
    let mut longest = 0;
    let mut index = 0;
    while index < ACTION_NAMES.len() {
        let length = ACTION_NAMES[index].len();
        if length > longest {
            longest = length;
        }
        index += 1;
    }
    longest
}

/// The statement that grants `actions` on the queue ARN `resource` to each account in
/// `account_numbers`, labelled `label`.
///
/// A member naming one value carries it alone and several as a list, which is how a
/// policy document states the two cases.
pub(super) fn permission_statement(
    label: &str,
    account_numbers: &[String],
    actions: &[String],
    resource: &str,
) -> ::serde_json::Value {
    let principals: Vec<String> = account_numbers
        .iter()
        .map(|account_number| account_principal(account_number))
        .collect();
    let allowed: Vec<String> = actions
        .iter()
        .map(|action| format!("{POLICY_ACTION_PREFIX}{action}"))
        .collect();
    ::serde_json::json!({
        POLICY_SID_MEMBER: label,
        "Effect": "Allow",
        "Principal": { "AWS": one_or_many(principals) },
        "Action": one_or_many(allowed),
        "Resource": resource,
    })
}

/// The principal a policy names for the account numbered `account_number`: an account
/// number identifies that account's root user, which is the principal the grant is
/// recorded for.
pub(super) fn account_principal(account_number: &str) -> String {
    format!("arn:aws:iam::{account_number}:root")
}

/// `values` as a policy member states them: the value alone when there is exactly one,
/// the list otherwise.
pub(super) fn one_or_many(mut values: Vec<String>) -> ::serde_json::Value {
    if values.len() == 1 {
        return ::serde_json::Value::String(values.remove(0));
    }
    ::serde_json::Value::Array(
        values
            .into_iter()
            .map(::serde_json::Value::String)
            .collect(),
    )
}
