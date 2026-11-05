// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a table carries, and the decision it drives.
//!
//! What makes a document one this emulator can attach, the statements it is read as,
//! and whether the policies in force let one caller take one action on one table. They
//! live beside [`super`] rather than inside it because they are about a DOCUMENT and
//! the decision it drives, while the parent module is the table and item store itself;
//! `super` re-exports them, so every caller still names them `crate::state::…`.

use super::*;

/// Why a resource-based policy document is not one this emulator can attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyFault {
    /// Past the documented size limit, carrying the size the request sent — the
    /// modelled `LimitExceededException`.
    TooLarge(usize),
    /// Not a policy document at all, carrying what is wrong with it — a
    /// `ValidationException`.
    Invalid(String),
}

/// A resource-based policy attached to a table.
///
/// The document is kept verbatim, because `GetResourcePolicy` "returns the
/// resource-based policy document attached to the resource ... in JSON format", and
/// its statements are parsed ONCE here so evaluating a data-plane request never
/// re-parses it. The document is capped at [`MAX_RESOURCE_POLICY_BYTES`] before it is
/// parsed, which is what bounds the statements, values and patterns one request can
/// ask the matcher below for.
#[derive(Debug, Clone)]
pub struct AttachedPolicy {
    /// The document exactly as it was attached.
    pub document: String,
    /// The revision id, DERIVED from the document: "PutResourcePolicy is an idempotent
    /// operation; running it multiple times on the same resource using the same policy
    /// document will return the same revision id" (the `PutResourcePolicy` doc). A
    /// different document therefore gets a different id, and the same one re-attached
    /// gets the one it had.
    pub revision_id: String,
    /// When the clock owes this policy its application to the data plane.
    ///
    /// "PutResourcePolicy is an asynchronous operation" and "the policy application is
    /// eventually consistent" (the `PutResourcePolicy` doc), so a request made straight
    /// after the attach is still authorized as it was — while a policy that arrives
    /// WITH the table is applied at once, because "when you attach a resource-based
    /// policy while creating a table, the policy application is strongly consistent"
    /// (`CreateTable$ResourcePolicy`). The attaching operation chooses which of the two
    /// it is; nothing sleeps for the span.
    pub effective_at: SystemTime,
    /// The parsed statements, in document order.
    pub(super) statements: Vec<PolicyStatement>,
}

impl AttachedPolicy {
    /// Parse `document` into the policy to attach, or say why it cannot be attached.
    ///
    /// `effective_at` is when the data plane starts honouring it (see the field).
    pub fn attach(document: &str, effective_at: SystemTime) -> Result<Self, PolicyFault> {
        if document.len() > MAX_RESOURCE_POLICY_BYTES {
            return Err(PolicyFault::TooLarge(document.len()));
        }
        let parsed: serde_json::Value = serde_json::from_str(document).map_err(|error| {
            PolicyFault::Invalid(format!("Resource-based policy is not valid JSON: {error}"))
        })?;
        let object = parsed.as_object().ok_or_else(|| {
            PolicyFault::Invalid("Resource-based policy must be a JSON object".to_string())
        })?;
        let listed: Vec<&serde_json::Value> = match object.get("Statement") {
            Some(serde_json::Value::Array(list)) => list.iter().collect(),
            Some(single @ serde_json::Value::Object(_)) => vec![single],
            _ => {
                return Err(PolicyFault::Invalid(
                    "Resource-based policy must carry a Statement".to_string(),
                ))
            }
        };
        let mut statements = Vec::with_capacity(listed.len());
        for statement in listed {
            statements.push(PolicyStatement::parse(statement)?);
        }
        Ok(Self {
            document: document.to_string(),
            revision_id: policy_revision_id(document),
            effective_at,
            statements,
        })
    }

    /// Whether this policy EXPLICITLY DENIES `action` on `resource_arn` to a caller any
    /// of `principals` names, and whether the statement that denied it carried a
    /// `Condition` the emulator did not evaluate.
    ///
    /// Only a Deny decides anything here: "an explicit deny in any policy overrides any
    /// allow" (the IAM policy evaluation logic), so a Deny that matches the request is the
    /// one outcome this half of the decision reaches. `principals` is every spelling the
    /// caller may be named by — its own ARN, the ARN of the role an assumed-role session
    /// is a session of, and the spellings that name its whole account, since a statement
    /// denying the account denies each of its principals. What GRANTS is
    /// [`Self::grants_any`], which is decided against the caller's own spellings alone.
    pub fn denies_any(
        &self,
        principals: &[String],
        action: &str,
        resource_arn: &str,
    ) -> Option<bool> {
        self.statements
            .iter()
            .find(|statement| statement.denies(principals, action, resource_arn))
            .map(PolicyStatement::conditioned)
    }

    /// Whether this policy GRANTS `action` on `resource_arn` to a caller any of
    /// `principals` names.
    ///
    /// A resource-based policy's `Allow` is a permission of its own where it names the
    /// CALLER: "the resource-based policy and the identity-based policy are evaluated
    /// together, and access is granted if either allows it". Only the caller's OWN
    /// spellings may be passed — a statement naming the account delegates to that
    /// account's identity-based policies rather than granting (see [`authorization`]).
    pub fn grants_any(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.statements
            .iter()
            .any(|statement| statement.grants(principals, action, resource_arn))
    }
}

/// The fixed opening of every `PolicyNotFoundException` message, whichever
/// resource-policy operation raises it and whichever of the two situations the
/// modelled error covers ("tried to access a nonexistent resource-based policy" and
/// "a policy is present for the resource but its revision ID didn't match the
/// expected value"). What follows it is the detail built from the request and from
/// this store: see [`policy_not_found_message`] and
/// [`policy_revision_mismatch_message`].
pub const POLICY_NOT_FOUND_PREFIX: &str =
    "Resource-based policy not found for the provided ResourceArn: ";

/// The refusal of an UNCONDITIONED request for a policy that is not there: the fixed
/// prefix and the `ResourceArn` the request sent, spelled as the request spelled it.
pub fn policy_not_found_message(resource_arn: &str) -> String {
    format!("{POLICY_NOT_FOUND_PREFIX}{resource_arn}")
}

/// The refusal of a request whose `ExpectedRevisionId` the target does not satisfy:
/// the fixed prefix, the revision id the request asked for, and what the target
/// actually carries — the revision id of the policy attached to it, or that it
/// carries no policy at all. The two tails are what lets a client doing optimistic
/// concurrency tell a stale revision apart from a policy that is gone.
pub fn policy_revision_mismatch_message(
    expected_revision_id: &str,
    table: &str,
    attached_revision_id: Option<&str>,
) -> String {
    match attached_revision_id {
        Some(current) => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but the policy associated to target table {table} has \
             revision id {current}."
        ),
        None => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but table {table} has no associated policy."
        ),
    }
}

/// The revision id of a policy document: a digest of the document, so the same
/// document always gets the same id and a different one never does.
pub(super) fn policy_revision_id(document: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(document.as_bytes()))
}

/// One statement of a resource-based policy, reduced to what decides a request.
#[derive(Debug, Clone)]
pub(super) struct PolicyStatement {
    /// `Effect: Deny`. An Allow statement is parsed and kept so the document round
    /// trips, but it changes no outcome (see [`AttachedPolicy::denies`]).
    pub(super) deny: bool,
    pub(super) principal: Selector,
    pub(super) action: Selector,
    pub(super) resource: Selector,
    /// Whether the statement carries a `Condition`.
    ///
    /// A condition is matched against the REQUEST CONTEXT (the source VPC, the time of
    /// day, a tag on the caller), and this emulator has no identity or network context
    /// to supply, so it cannot evaluate one. A Deny that carries one is therefore
    /// applied anyway — FAIL CLOSED: the emulator never serves a request the attached
    /// policy might refuse, which is the error that matters for a policy whose whole
    /// job is to refuse. The flag is kept so the refusal can say the condition was not
    /// evaluated.
    pub(super) conditioned: bool,
}

impl PolicyStatement {
    /// Parse one statement, refusing a document this emulator would otherwise store
    /// and then not honour.
    pub(super) fn parse(value: &serde_json::Value) -> Result<Self, PolicyFault> {
        let object = value.as_object().ok_or_else(|| {
            PolicyFault::Invalid("Each Statement must be a JSON object".to_string())
        })?;
        let conditioned = object.contains_key("Condition");
        let effect = object
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement must carry an Effect of Allow or Deny".to_string(),
                )
            })?;
        let deny = if effect.eq_ignore_ascii_case("Deny") {
            true
        } else if effect.eq_ignore_ascii_case("Allow") {
            false
        } else {
            return Err(PolicyFault::Invalid(format!(
                "Statement Effect must be Allow or Deny, got {}",
                clipped(effect)
            )));
        };
        // "Resource-based policies ... require a Principal" (the IAM user guide's
        // policy grammar), so a statement without one is not a resource policy.
        let principal = Selector::parse(object, "Principal", "NotPrincipal", principal_values)?
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement of a resource-based policy must carry a Principal".to_string(),
                )
            })?;
        let action = Selector::parse(object, "Action", "NotAction", string_values)?.ok_or_else(
            || PolicyFault::Invalid("Each Statement must carry an Action".to_string()),
        )?;
        // A resource-based policy is attached to ONE resource, so a statement that
        // names no Resource applies to the resource it is attached to.
        let resource = Selector::parse(object, "Resource", "NotResource", string_values)?
            .unwrap_or_else(Selector::everything);
        Ok(Self {
            deny,
            principal,
            action,
            resource,
            conditioned,
        })
    }

    /// Whether this statement denies the request.
    ///
    /// A `Condition` is not evaluated (see the field's doc) and does not stop a Deny
    /// from applying. `principals` is every spelling the caller may be named by; a
    /// statement naming any of them is about this request.
    pub(super) fn denies(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.deny && self.about(principals, action, resource_arn)
    }

    /// Whether this statement GRANTS the request.
    ///
    /// A conditioned `Allow` grants nothing: the condition is not evaluated (see the
    /// field's doc), so the same fail-closed reading that applies a conditioned Deny
    /// withholds a conditioned grant — the emulator never serves on a condition it did
    /// not check.
    pub(super) fn grants(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        !self.deny && !self.conditioned && self.about(principals, action, resource_arn)
    }

    /// Whether this statement is ABOUT the request at all: its principal, action and
    /// resource elements each select it.
    pub(super) fn about(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.principal
            .matches(|value| principal_names(value, principals))
            && self.action.matches(|pattern| action_matches(pattern, action))
            && self
                .resource
                .matches(|pattern| resource_matches(pattern, resource_arn))
    }

    /// Whether this statement's decision rests on a `Condition` the emulator did not
    /// evaluate, so a refusal can say so.
    pub(super) fn conditioned(&self) -> bool {
        self.conditioned
    }
}

/// One statement element: the values it lists, and whether the statement applies to
/// those values (`Action`) or to everything BUT them (`NotAction`).
#[derive(Debug, Clone)]
pub(super) struct Selector {
    pub(super) values: Vec<String>,
    pub(super) negated: bool,
}

impl Selector {
    /// Read `positive` or its `Not…` counterpart out of a statement. A statement that
    /// carries both forms of one element is not a policy document.
    pub(super) fn parse(
        object: &serde_json::Map<String, serde_json::Value>,
        positive: &'static str,
        negative: &'static str,
        extract: fn(&serde_json::Value) -> Option<Vec<String>>,
    ) -> Result<Option<Self>, PolicyFault> {
        let (raw, negated) = match (object.get(positive), object.get(negative)) {
            (Some(_), Some(_)) => {
                return Err(PolicyFault::Invalid(format!(
                    "A Statement may carry {positive} or {negative}, not both"
                )))
            }
            (Some(value), None) => (value, false),
            (None, Some(value)) => (value, true),
            (None, None) => return Ok(None),
        };
        let values = extract(raw).ok_or_else(|| {
            PolicyFault::Invalid(format!(
                "{positive} must be a string, a list of strings, or a map of principal kinds"
            ))
        })?;
        Ok(Some(Self { values, negated }))
    }

    /// The element a statement that names none of its own carries: everything.
    pub(super) fn everything() -> Self {
        Self {
            values: vec!["*".to_string()],
            negated: false,
        }
    }

    /// Whether the request's value satisfies this element, honouring the negation.
    pub(super) fn matches(&self, matched: impl Fn(&str) -> bool) -> bool {
        self.values.iter().any(|value| matched(value)) != self.negated
    }
}

/// A statement element given as a string or a list of strings.
pub(super) fn string_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(text) => Some(vec![text.clone()]),
        serde_json::Value::Array(list) => list
            .iter()
            .map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// A `Principal` element: `"*"`, or a map from principal kind to one or more values.
///
/// A kind other than `AWS` — `Service`, `Federated`, `CanonicalUser` — names no IAM
/// identity, so its values are kept WITH their kind and can match no caller rather
/// than being silently dropped.
pub(super) fn principal_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(_) | serde_json::Value::Array(_) => string_values(value),
        serde_json::Value::Object(map) => {
            let mut values = Vec::new();
            for (kind, listed) in map {
                let listed = string_values(listed)?;
                if kind.eq_ignore_ascii_case("AWS") {
                    values.extend(listed);
                } else {
                    values.extend(listed.into_iter().map(|value| format!("{kind}:{value}")));
                }
            }
            Some(values)
        }
        _ => None,
    }
}

/// The identity every request to this emulator carries when nothing more is known about
/// it: the root of the single account it serves.
///
/// What the POLICY-MANAGEMENT lock-out is decided against ([`locks_the_caller_out`]),
/// which asks whether a document would take the ACCOUNT's own ability to change that
/// policy away. Who a DATA-PLANE request speaks for is resolved from the credential it
/// signed with instead ([`authorization::request_caller`]).
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Whether a policy `Principal` value names the identity this emulator serves.
///
/// `principals` is every spelling the caller may be named by — its own ARN, the role
/// behind an assumed-role session, and (where a DENY is being decided) the ARN and bare
/// id of its account, because a statement about the account is about each of its
/// principals. A value matches when it names any of them, with IAM's wildcards.
///
/// A principal of a kind that names no IAM identity — a `Service`, a `Federated` or a
/// `CanonicalUser` — is kept WITH its kind by [`principal_values`], so it matches no
/// spelling here rather than being read as one.
pub(super) fn principal_names(value: &str, principals: &[String]) -> bool {
    principals
        .iter()
        .any(|named| wildcard_match(value, named))
}

/// Whether a policy `Action` value names the action being performed. IAM action names
/// are case-insensitive.
pub(super) fn action_matches(pattern: &str, action: &str) -> bool {
    wildcard_match(
        &pattern.to_ascii_lowercase(),
        &action.to_ascii_lowercase(),
    )
}

/// Whether a policy `Resource` value names the resource the request names.
///
/// The partition, region and account segments of an ARN are not compared: this
/// emulator serves one partition and one account, and a policy is attached to a table
/// in a known region, so what distinguishes one resource from another here is the
/// SERVICE and the resource path (`table/<name>`, `table/<name>/index/<index>`).
pub(super) fn resource_matches(pattern: &str, resource_arn: &str) -> bool {
    if wildcard_match(pattern, resource_arn) {
        return true;
    }
    match (arn_tail(pattern), arn_tail(resource_arn)) {
        (Some(pattern), Some(actual)) => wildcard_match(&pattern, &actual),
        _ => false,
    }
}

/// The service and resource path of an ARN — `dynamodb:table/x` — or `None` when the
/// value is not an ARN at all.
pub(super) fn arn_tail(arn: &str) -> Option<String> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, service) = (fields.next()?, fields.next()?);
    let (_region, _account) = (fields.next()?, fields.next()?);
    let resource = fields.next()?;
    Some(format!("{service}:{resource}"))
}

/// IAM's glob matching: `*` stands for any run of characters and `?` for exactly one.
///
/// Iterative with a single backtrack point, so the work is bounded by the lengths of
/// the two strings rather than by their structure; both come from a document already
/// capped at [`MAX_RESOURCE_POLICY_BYTES`].
pub(super) fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut at_pattern, mut at_value) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while at_value < value.len() {
        if at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == value[at_value])
        {
            at_pattern += 1;
            at_value += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            resume = at_value;
            at_pattern += 1;
        } else if let Some(at_star) = star {
            at_pattern = at_star + 1;
            resume += 1;
            at_value = resume;
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|glob| *glob == '*')
}

/// The actions that let a caller CHANGE a resource's policy again.
///
/// A policy that denies the caller any of them removes "your permissions to change the
/// policy of this resource in the future"
/// (`PutResourcePolicy$ConfirmRemoveSelfResourceAccess`), which is what that member has
/// to be set to `true` to accept.
pub(super) const POLICY_MANAGEMENT_ACTIONS: [&str; 3] = [
    "dynamodb:PutResourcePolicy",
    "dynamodb:DeleteResourcePolicy",
    "dynamodb:GetResourcePolicy",
];

/// Whether `policy`, attached to the table at `resource_arn`, would deny THIS CALLER the
/// ability to change that policy again.
///
/// Checked against the policy the request is attaching rather than the one the table
/// carries: the lock-out is what the NEW document would do, and it is decided before
/// anything is stored so a refused request leaves the table exactly as it was.
///
/// Every spelling a statement may name this account by is compared — the root user's ARN
/// and the bare account id that stands for it ([`::service_common::iam_authorization::account_principals`]) —
/// because a statement that denies the ACCOUNT denies each of its principals, so a
/// document that names the account by its id takes the same access away as one that names
/// its root.
pub fn locks_the_caller_out(policy: &AttachedPolicy, resource_arn: &str) -> bool {
    let caller = caller_principal();
    let mut named = vec![caller.clone()];
    named.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    POLICY_MANAGEMENT_ACTIONS
        .iter()
        .any(|action| policy.denies_any(&named, action, resource_arn).is_some())
}

/// The `AccessDeniedException` message a data-plane request earns when the policies in
/// force do not permit it, or `None` when they do.
///
/// `caller` is who the request speaks for ([`authorization::request_caller`], resolved
/// before this store was locked) and `action` is the IAM action the operation performs
/// (`dynamodb:GetItem`). The decision itself is [`authorization::denial`]'s, which is the
/// one place this service makes it.
pub fn policy_denial(
    caller: &authorization::Caller,
    table: &Table,
    action: &str,
    now: SystemTime,
) -> Option<String> {
    authorization::denial(caller, table, action, now)
}
