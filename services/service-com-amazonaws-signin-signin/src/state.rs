// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-signin-signin service.
//!
//! Two resources live here, and every implemented handler of this service reads
//! and writes exactly these records:
//!
//! * the **resource-permission statement** — `PutResourcePermissionStatement`
//!   mints one, `ListResourcePermissionStatements` enumerates them,
//!   `GetResourcePolicy` renders them as the account's SignIn resource-based
//!   policy document and `DeleteResourcePermissionStatement` removes one;
//! * the **console-authorization configuration** —
//!   `PutConsoleAuthorizationConfiguration` persists one per target and
//!   `GetConsoleAuthorizationConfiguration` reports it back.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::ops::types::{
    PermissionStatementSummary, PolicyStatement, ResourceNotFoundException,
    SigninResourceBasedPolicy, TooManyRequestsError, ValidationException,
};

/// The `Version` of the rendered `SigninResourceBasedPolicy`.
///
/// `2012-10-17` is the current IAM policy-language version every AWS
/// resource-based policy document declares; the model types `version` as a plain
/// string and no operation of this service accepts one, so the service's own
/// policy language version is the only value it can report.
pub const POLICY_VERSION: &str = "2012-10-17";

/// The largest page `ListResourcePermissionStatements` serves, and the page size
/// it uses when the request names no `maxResults`:
/// `ListResourcePermissionStatementsInput$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const STATEMENT_PAGE_LIMIT: i32 = 100;

/// How many permission statements the store holds before
/// `PutResourcePermissionStatement` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any
/// account's limit: AWS bounds a resource-based policy by its document size and
/// publishes no statement count, and this emulator models neither Service Quotas
/// nor an account's adjustments. It is what stops the state growing without end —
/// every record it holds is released by `DeleteResourcePermissionStatement`,
/// which also forgets that statement's client tokens, so the token maps cannot
/// outgrow this bound either.
pub const PERMISSION_STATEMENT_QUOTA: usize = 100;

/// How many DISTINCT targets the console-authorization store holds before
/// `PutConsoleAuthorizationConfiguration` refuses to take on another one.
///
/// Like [`PERMISSION_STATEMENT_QUOTA`] this is the emulator's OWN bound on the
/// store, not a claim about any account's limit: AWS publishes no maximum number
/// of console-authorization targets and this emulator models neither Service
/// Quotas nor an account's adjustments. It is what stops the map growing without
/// end, since `TargetId`'s key space is effectively unlimited and the release
/// path (`DeleteConsoleAuthorizationConfiguration`) is still a scaffold: a put
/// naming a target the store already holds is an update and always allowed,
/// while a put naming a NEW target past this bound is refused.
pub const CONSOLE_AUTHORIZATION_TARGET_QUOTA: usize = 100;

/// The `error` code the model documents for `ValidationException`: "Will be
/// INVALID_REQUEST for validation errors".
pub const VALIDATION_ERROR_CODE: &str = "INVALID_REQUEST";

/// The `error` code the model documents for `ResourceNotFoundException`: "Will be
/// RESOURCE_NOT_FOUND".
pub const NOT_FOUND_ERROR_CODE: &str = "RESOURCE_NOT_FOUND";

/// The `error` code the model documents for `ServiceQuotaExceededException`:
/// "Will be SERVICE_QUOTA_EXCEEDED".
pub const QUOTA_ERROR_CODE: &str = "SERVICE_QUOTA_EXCEEDED";

/// The `error` code the model documents for `TooManyRequestsError`: "Will be
/// INVALID_REQUEST for rate limiting scenarios".
pub const RATE_LIMIT_ERROR_CODE: &str = "INVALID_REQUEST";

/// The `error` code the model documents for `InternalServerException`: "Will be
/// SERVER_ERROR for internal server errors" — the enum member `SERVER_ERROR`
/// carries the wire value `server_error`.
pub const SERVER_ERROR_CODE: &str = "server_error";

/// `ConditionBlock`: the condition operator to the condition keys it tests, each
/// key holding the values it is tested against.
///
/// The model types it `map<ConditionType, map<String, list<String>>>`, which is
/// the IAM policy-document condition grammar:
/// `"Condition": { "<operator>": { "<condition key>": [values] } }`.
pub type ConditionBlock = HashMap<String, HashMap<String, Vec<String>>>;

/// The IAM condition operator an exact-match condition key is tested under.
const STRING_EQUALS: &str = "StringEquals";

/// The IAM condition operator an address or CIDR condition key is tested under.
const IP_ADDRESS: &str = "IpAddress";

/// The IAM condition operator the principal a statement does NOT apply to is
/// tested under.
const ARN_NOT_EQUALS: &str = "ArnNotEquals";

/// The AWS global condition key a VPC identifier is tested against.
const SOURCE_VPC_KEY: &str = "aws:SourceVpc";

/// The AWS global condition key a VPC endpoint identifier is tested against.
/// Both the SignIn and the console endpoint of one statement are values of this
/// one key.
const SOURCE_VPCE_KEY: &str = "aws:SourceVpce";

/// The condition key an address inside the VPC is tested against.
const VPC_SOURCE_IP_KEY: &str = "aws:VpcSourceIp";

/// The AWS global condition key the requester's address is tested against.
const SOURCE_IP_KEY: &str = "aws:SourceIp";

/// The AWS global condition key the region of the request is tested against.
const REQUESTED_REGION_KEY: &str = "aws:RequestedRegion";

/// The AWS global condition key the calling principal's ARN is tested against.
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";

/// The authorization scope of a 12-digit account id target.
pub const ACCOUNT_SCOPE: &str = "ACCOUNT";

/// The authorization scope of an `o-` AWS Organizations organization target.
pub const ORGANIZATION_SCOPE: &str = "ORGANIZATION";

/// The authorization scope of an `r-` AWS Organizations root target.
pub const ROOT_SCOPE: &str = "ROOT";

/// One permission statement of the account's SignIn resource-based policy, as
/// this emulator persists it: the id it was minted with and the condition-key
/// members the request supplied.
#[derive(Debug, Clone)]
pub struct PermissionStatement {
    /// `StatementId`'s `^[A-Za-z0-9+/]{64}=?$`, minted by
    /// [`ComAmazonawsSigninSigninState::mint_statement_id`] and stored here: it
    /// is what `PutResourcePermissionStatement` returns and what
    /// `DeleteResourcePermissionStatement` names.
    pub statement_id: String,
    pub source_vpc: Option<String>,
    pub signin_source_vpce: Option<String>,
    pub console_source_vpce: Option<String>,
    pub vpc_source_ip: Option<String>,
    pub source_ip: Option<String>,
    pub requested_region: Option<String>,
    pub excluded_principal: Option<String>,
}

impl PermissionStatement {
    /// The populated condition members, projected into the `ConditionBlock` the
    /// list summary and the policy document both carry.
    ///
    /// Each member becomes a value of the AWS global condition key that names
    /// it, under the operator its type is tested with: identifiers and the
    /// region by string equality, addresses by `IpAddress`, and the excluded
    /// principal by `ArnNotEquals` — the statement is the one that does NOT
    /// apply to it. A member the request did not carry contributes nothing, so
    /// a statement created with no condition member at all has no condition
    /// block.
    pub fn conditions(&self) -> Option<ConditionBlock> {
        let mut block = ConditionBlock::new();
        let mut add = |operator: &str, key: &str, value: &Option<String>| {
            if let Some(value) = value {
                block
                    .entry(operator.to_string())
                    .or_default()
                    .entry(key.to_string())
                    .or_default()
                    .push(value.clone());
            }
        };

        add(STRING_EQUALS, SOURCE_VPC_KEY, &self.source_vpc);
        add(STRING_EQUALS, SOURCE_VPCE_KEY, &self.signin_source_vpce);
        add(STRING_EQUALS, SOURCE_VPCE_KEY, &self.console_source_vpce);
        add(STRING_EQUALS, REQUESTED_REGION_KEY, &self.requested_region);
        add(IP_ADDRESS, VPC_SOURCE_IP_KEY, &self.vpc_source_ip);
        add(IP_ADDRESS, SOURCE_IP_KEY, &self.source_ip);
        add(ARN_NOT_EQUALS, PRINCIPAL_ARN_KEY, &self.excluded_principal);

        (!block.is_empty()).then_some(block)
    }

    /// The `PermissionStatementSummary` projection
    /// `ListResourcePermissionStatements` returns.
    pub fn to_summary(&self) -> PermissionStatementSummary {
        PermissionStatementSummary {
            sid: self.statement_id.clone(),
            condition: self.conditions(),
        }
    }

    /// This statement as one `PolicyStatement` of the rendered policy document.
    ///
    /// Only `Condition` is reported: the model's `PolicyStatement` carries
    /// `Effect`, `Principal`, `Action` and `Resource` as OPTIONAL members and
    /// this service's surface has no input for any of them, so the emulator
    /// reports the conditions it stored rather than inventing a frame around
    /// them. `PolicyStatement` models no `Sid` member either — the statement id
    /// is reported by `PutResourcePermissionStatement` and by
    /// `ListResourcePermissionStatements`, which is where the model puts it.
    pub fn to_policy_statement(&self) -> PolicyStatement {
        PolicyStatement {
            effect: None,
            principal: None,
            action: None,
            resource: None,
            condition: self.conditions(),
        }
    }
}

/// The console-authorization configuration of one target, as
/// `PutConsoleAuthorizationConfiguration` persisted it.
#[derive(Debug, Clone)]
pub struct ConsoleAuthorizationConfiguration {
    /// `TargetId`: a 12-digit account id, an `o-` organization id or an `r-`
    /// root id.
    pub target_id: String,
    /// The scope the target's form implies — see [`scope_for_target`].
    pub scope: String,
    /// Whether console authorization is configured for this target.
    pub console_authorization_enabled: bool,
}

impl ConsoleAuthorizationConfiguration {
    /// The three members `Put`/`Get`/`DeleteConsoleAuthorizationConfiguration`
    /// all report, read off the stored record.
    pub fn to_members(&self) -> (String, String, bool) {
        (
            self.target_id.clone(),
            self.scope.clone(),
            self.console_authorization_enabled,
        )
    }
}

/// Per-service state for com-amazonaws-signin-signin. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSigninSigninState {
    /// Every permission statement, keyed by its id. Ids are minted in a
    /// lexicographically increasing sequence, so a `BTreeMap` holds them in
    /// mint order — which is what gives `ListResourcePermissionStatements` the
    /// stable total order its pagination token indexes into, and
    /// `GetResourcePolicy` a deterministic document.
    pub permission_statements: BTreeMap<String, PermissionStatement>,
    /// A `PutResourcePermissionStatement` `clientToken` to the statement that
    /// token created, so a repeated put answers with the first statement
    /// instead of minting a second one. Bounded by the store it points into: an
    /// entry is only written beside a newly minted statement (itself capped by
    /// [`PERMISSION_STATEMENT_QUOTA`]) and is dropped when that statement is
    /// deleted.
    pub statement_idempotency: HashMap<String, String>,
    /// A `DeleteResourcePermissionStatement` `clientToken` to the statement it
    /// removed, so a retried delete answers the same success rather than a 404.
    /// Written only through [`ComAmazonawsSigninSigninState::remember_deletion`],
    /// which keeps it to the [`PERMISSION_STATEMENT_QUOTA`] most recent deletes.
    pub deletion_idempotency: HashMap<String, String>,
    /// The tokens of [`Self::deletion_idempotency`] in the order they were
    /// recorded, so the oldest is the one evicted when the bound is reached.
    pub deletion_token_order: VecDeque<String>,
    /// The console-authorization configuration of each configured target.
    /// Bounded by [`CONSOLE_AUTHORIZATION_TARGET_QUOTA`]: a put naming a new
    /// target past that count is refused rather than stored.
    pub console_auth_configs: HashMap<String, ConsoleAuthorizationConfiguration>,
    /// The counter statement ids are minted from.
    pub statement_id_seq: u64,
}

impl ComAmazonawsSigninSigninState {
    /// Mint an unused statement id matching `StatementId`'s
    /// `^[A-Za-z0-9+/]{64}=?$`.
    ///
    /// The counter is rendered as 64 lowercase hex digits — an alphabet that
    /// pattern admits and whose ASCII order is its numeric order, so ids sort in
    /// mint order and one of them is usable as a `nextToken` (whose own pattern
    /// `^[-a-zA-Z0-9+=/_]*$` admits it too). The loop skips an id already taken,
    /// which is what makes the counter's wrap safe.
    pub fn mint_statement_id(&mut self) -> String {
        loop {
            self.statement_id_seq = self.statement_id_seq.wrapping_add(1);
            let id = format!("{:064x}", self.statement_id_seq);
            if !self.permission_statements.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record that `token` deleted `statement_id`, keeping at most
    /// [`PERMISSION_STATEMENT_QUOTA`] records: once that many deletes are
    /// remembered the OLDEST is forgotten to make room, so the map stays bounded
    /// without the idempotency guarantee decaying as the store ages — a retry of
    /// a recent delete still answers its original success.
    pub fn remember_deletion(&mut self, token: String, statement_id: String) {
        if self
            .deletion_idempotency
            .insert(token.clone(), statement_id)
            .is_none()
        {
            self.deletion_token_order.push_back(token);
        }
        while self.deletion_token_order.len() > PERMISSION_STATEMENT_QUOTA {
            if let Some(evicted) = self.deletion_token_order.pop_front() {
                self.deletion_idempotency.remove(&evicted);
            }
        }
    }

    /// True when `token` is a pagination token this service could have issued:
    /// one of the statement ids [`Self::mint_statement_id`] has already minted.
    ///
    /// `ListResourcePermissionStatements`' `nextToken` is modelled
    /// `^[-a-zA-Z0-9+=/_]*$`, far wider than the ids this service mints, so this
    /// is what separates a token the caller got from a page of this operation
    /// from one it invented. A minted id is 64 lowercase hex digits rendering a
    /// counter value the mint has reached, so a token is issuable exactly when it
    /// reads as such a value; the id of a statement deleted meanwhile stays
    /// issuable, which is what lets a page still be served after the statement
    /// its token named is gone.
    pub fn is_issued_pagination_token(&self, token: &str) -> bool {
        token.len() == 64
            && u128::from_str_radix(token, 16)
                .is_ok_and(|value| (1..=u128::from(self.statement_id_seq)).contains(&value))
    }

    /// Render the account's SignIn resource-based policy from the stored
    /// statements: one policy statement per stored permission statement, in
    /// mint order. With nothing stored the policy carries an empty statement
    /// list — the account has a policy, it just permits nothing extra.
    pub fn to_policy(&self) -> SigninResourceBasedPolicy {
        SigninResourceBasedPolicy {
            version: Some(POLICY_VERSION.to_string()),
            statement: Some(
                self.permission_statements
                    .values()
                    .map(PermissionStatement::to_policy_statement)
                    .collect(),
            ),
        }
    }
}

/// The authorization scope `TargetId`'s form implies.
///
/// `TargetId` is modelled `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$` — an
/// account id, an AWS Organizations organization id or an organizational root
/// id — so the scope is a function of which alternative the value matches.
/// `scope` is modelled as a plain string with no enum, so these three names are
/// this emulator's, named after the Organizations entity each target form
/// identifies. `None` is unreachable through a request the boundary validator
/// admitted, and the handlers refuse it as a validation failure rather than
/// guessing a scope.
pub fn scope_for_target(target_id: &str) -> Option<&'static str> {
    if target_id.len() == 12 && target_id.bytes().all(|b| b.is_ascii_digit()) {
        return Some(ACCOUNT_SCOPE);
    }
    if target_id.starts_with("o-") {
        return Some(ORGANIZATION_SCOPE);
    }
    if target_id.starts_with("r-") {
        return Some(ROOT_SCOPE);
    }
    None
}

/// The `ResourceNotFoundException` payload for a statement id that names no
/// stored statement.
pub fn statement_not_found(statement_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        error: NOT_FOUND_ERROR_CODE.to_string(),
        message: format!("no permission statement with statementId {statement_id}"),
    }
}

/// The `ResourceNotFoundException` payload for a target whose console
/// authorization was never configured.
pub fn configuration_not_found(target_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        error: NOT_FOUND_ERROR_CODE.to_string(),
        message: format!("no console authorization configuration for targetId {target_id}"),
    }
}

/// The `TooManyRequestsError` payload for a put that would take the
/// console-authorization store past [`CONSOLE_AUTHORIZATION_TARGET_QUOTA`].
///
/// `PutConsoleAuthorizationConfiguration` does not declare
/// `ServiceQuotaExceededException`, so the store's bound is reported with the
/// modelled refusal it does declare for exactly this shape of request: the one
/// the model documents for "abuse prevention" and "service protection against
/// excessive" use.
pub fn console_target_quota_exceeded(target_id: &str) -> TooManyRequestsError {
    TooManyRequestsError {
        error: RATE_LIMIT_ERROR_CODE.to_string(),
        message: format!(
            "console authorization is already configured for the maximum of {} targets; \
             configuring targetId {target_id} as well would exceed it",
            CONSOLE_AUTHORIZATION_TARGET_QUOTA
        ),
    }
}

/// The modelled `ValidationException` payload carrying `message`.
pub fn validation_failed(message: impl Into<String>) -> ValidationException {
    ValidationException {
        error: VALIDATION_ERROR_CODE.to_string(),
        message: message.into(),
    }
}

/// The target a console-authorization call addresses: the `targetId` it carries,
/// or the caller's own account when it carries none.
pub fn requested_target(target_id: Option<String>) -> String {
    target_id.unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSigninSigninState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSigninSigninState>)
}
