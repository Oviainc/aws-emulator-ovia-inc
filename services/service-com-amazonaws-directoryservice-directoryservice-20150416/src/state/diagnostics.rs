// SPDX-License-Identifier: CC0-1.0

//! THE SERVICE'S REFUSAL DIAGNOSTICS: the one wording each failure class is stated with,
//! shared so every operation that answers a given failure says the same thing about it.
//!
//! Each function here builds a message from the REQUEST and the store, and nothing else;
//! `crate::state::refusal` is what wraps one into a modelled error shape.

use super::*;

/// The `EntityDoesNotExistException` message for an id that names no stored directory.
/// Shared so every operation that resolves a directory says the same thing about the
/// same failure.
///
/// The wording follows Directory Service's own: the quoted id the request asked for,
/// then "does not exist." — so the message is built from the request and names what the
/// caller has to fix, and nothing else. [`refusal`] appends the request id, once.
pub fn no_such_directory(directory_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist.")
}

/// The message an operation that takes a `DirectoryId` as a FILTER refuses an id no
/// directory is stored under with.
///
/// Directory Service answers this failure in two different families, and which one a
/// caller sees follows from what the id WAS in the request: an operation whose subject is
/// the directory reports that the directory does not exist ([`no_such_directory`]), while
/// one that takes the id as a filter over another listing reports the VALUE as an invalid
/// directory id — unquoted, singular, closed by a period. `DescribeSnapshots`, whose
/// `DirectoryId` narrows the snapshots it lists, is the latter. [`refusal`] appends the
/// request id, once.
pub fn invalid_directory_id(directory_id: &str) -> String {
    format!("Invalid directory ID: {directory_id}.")
}

/// The same report for an operation whose filter is a LIST of directory ids
/// (`DescribeDirectories$DirectoryIds`): every id the store could not resolve, in the
/// order the filter is scanned, separated by a BARE comma, under the plural spelling of
/// the member the ids arrived in. [`refusal`] appends the request id, once.
///
/// The separator carries no space, so a client that splits the message on `,` to recover
/// which ids were rejected gets the ids themselves, and the order is the filter's rather
/// than a sorted one, so the nth name answers the nth id the request sent that the store
/// could not resolve.
pub fn invalid_directory_ids(directory_ids: &[String]) -> String {
    format!(
        "Invalid directory ID(s): {ids}.",
        ids = directory_ids.join(",")
    )
}

/// The `DirectoryUnavailableException` message for a directory that is stored but is not
/// in service. Shared so every operation that needs a directory in service — and whose
/// family states no sentence of its own — says the same thing about the same failure.
///
/// Directory Service words this one as the operation it could not perform, the directory
/// the request named (UNQUOTED) and the fact that refused it: that the directory is
/// unavailable. It reports nothing about WHERE in its build the directory is, so the
/// diagnostic discloses no internal provisioning state and a caller cannot read one out of
/// the message. It is NOT [`no_such_directory`], the sentence for an id that resolves to no
/// directory at all: a caller that branches on the message must be able to tell a directory
/// that is not there from one that is not ready. [`refusal`] appends the request id, once.
///
/// Some families state their own sentence for the same failure, and those say it rather
/// than this one: [`ldaps_directory_unavailable`] for the LDAPS settings,
/// [`computer_directory_unavailable`] for a computer-account write, and
/// [`stage_defect`] for a call whose refusal names the stage it needs.
pub fn directory_unavailable(directory_id: &str) -> String {
    format!("Operation could not be performed because directory {directory_id} is unavailable.")
}

/// The `DirectoryUnavailableException` message a computer-account write on a directory
/// that is not in service is refused with.
///
/// Directory Service words this one in the directory-LOOKUP family's sentence
/// ([`no_such_directory`]) rather than the family-neutral [`directory_unavailable`] one: a
/// computer account is created in the directory's own domain, and a directory that is still
/// being built has no domain to create it in, so the join path reports the directory as one
/// it does not have — under the modelled `DirectoryUnavailableException`, which is what
/// tells the caller to wait for the directory rather than to fix the id it sent.
/// `CreateComputer` states it; the family-neutral sentence is what every operation whose
/// family states none of its own says. [`refusal`] appends the request id, once.
pub fn computer_directory_unavailable(directory_id: &str) -> String {
    no_such_directory(directory_id)
}

/// The `EntityAlreadyExistsException` diagnostic a computer account the directory already
/// holds is refused with, built around `request_id` — the id of the call being answered.
///
/// Directory Service states this one as the account it could not create and nothing about
/// what collided: the FIXED sentence that the computer could not be created, closing with
/// the call's own request id, and neither the directory nor the name the request sent — so a
/// caller learns that the account is there without the service disclosing anything of the
/// one it holds. The id is therefore named TWICE in the answer's `Message`, once by this
/// sentence and once by the suffix [`refusal_naming_its_request_id`] appends, and both are
/// this call's, the one the modelled `RequestId` member reports.
pub fn computer_already_exists(request_id: &str) -> String {
    format!("Could not create computer. RequestId: {request_id}")
}

/// The `DirectoryUnavailableException` message the LDAPS settings of a directory that is
/// not in service are refused with.
///
/// Directory Service words this one in terms of what the directory is not available FOR —
/// the quoted id the request named, then that it is not available for an LDAPS operation —
/// so `EnableLDAPS` and `DisableLDAPS` state it rather than the family-neutral
/// [`directory_unavailable`], and both state it the same way. [`refusal`] appends the
/// request id, once.
pub fn ldaps_directory_unavailable(directory_id: &str) -> String {
    format!("Directory: '{directory_id}' is not available for LDAPS operation.")
}

/// The stages a mutation of a directory's conditional forwarders is served in.
///
/// Directory Service checks the directory's stage as a PARAMETER of such a call — it
/// states which stages the operation needs rather than reporting the directory as
/// unreachable — and the two it serves are [`STAGE_ACTIVE`] and [`STAGE_IMPAIRED`]: a
/// degraded directory still answers for its own DNS configuration.
pub const CONDITIONAL_FORWARDER_SERVING_STAGES: [&str; 2] = [STAGE_ACTIVE, STAGE_IMPAIRED];

/// Why the directory in `stage` cannot serve a mutation of its conditional forwarders, or
/// `None` when it can.
///
/// THE one guard the conditional-forwarder mutations (`CreateConditionalForwarder`,
/// `UpdateConditionalForwarder`, `DeleteConditionalForwarder`) read the directory's stage
/// through, so all three refuse a directory that is not in service the same way: with the
/// modelled `InvalidParameterException` stating the mutation it could not perform, the
/// directory the request named and the stages that mutation needs, which is what tells a
/// caller to wait rather than to retry. It is NOT [`directory_unavailable`], the diagnostic
/// of the family that reports a directory as unreachable. `action` is the operation's own
/// verb ([`FORWARDER_ACTION_CREATE`] and its siblings). [`refusal`] appends the request id,
/// once.
pub fn conditional_forwarder_stage_defect(
    action: &str,
    directory_id: &str,
    stage: &str,
) -> Option<String> {
    if CONDITIONAL_FORWARDER_SERVING_STAGES.contains(&stage) {
        return None;
    }
    Some(stage_defect(
        &format!("{action} conditional forwarder"),
        directory_id,
        &CONDITIONAL_FORWARDER_SERVING_STAGES,
    ))
}

/// The sentence Directory Service states a mutation it cannot perform because the
/// directory's STAGE does not admit it in: the action it could not perform, the directory
/// the request named and the stages that action needs.
///
/// THE one place that sentence is written, so the conditional-forwarder family states one
/// rule one way. The stages come from the operation rather than from the directory, so the
/// message discloses nothing about where in its build the directory is. [`refusal`] appends
/// the request id, once.
pub fn stage_defect(action: &str, directory_id: &str, required: &[&str]) -> String {
    format!(
        "Could not {action} as directory: {directory_id} is not in {required} state.",
        required = required.join(" or ")
    )
}

/// The stages a tag write of a directory is served in.
///
/// `AddTagsToResource` and `RemoveTagsFromResource` change a directory that is already
/// there, so Directory Service admits them once the directory has finished being built:
/// [`STAGE_ACTIVE`], and [`STAGE_IMPAIRED`] with it — a degraded directory still carries
/// its own tags.
pub const TAG_SERVING_STAGES: [&str; 2] = [STAGE_ACTIVE, STAGE_IMPAIRED];

/// Why the directory in `stage` cannot serve a tag write, or `None` when it can.
///
/// THE one guard the tag writes read the directory's stage through, so `AddTagsToResource`
/// and `RemoveTagsFromResource` refuse a directory that is still being built the same way:
/// Directory Service reports such a call as one its subject's state does not support
/// ([`unsupported_in_stage`]), under the modelled `ClientException` both operations
/// declare, rather than as the directory being unreachable ([`directory_unavailable`]) or
/// as a parameter of the call being wrong.
pub fn tag_stage_defect(stage: &str) -> Option<String> {
    if TAG_SERVING_STAGES.contains(&stage) {
        return None;
    }
    Some(unsupported_in_stage(stage))
}

/// The sentence Directory Service states a call its subject's STATE does not support in:
/// that the operation is not supported for a directory in this state, then the status the
/// directory is actually in.
///
/// The fixed text is the rule, and the one variable part is read from the STORE — the
/// stage the directory is in when the call arrives — so a caller polling a create is told
/// which state refused it and can wait for the one that serves it. [`refusal`] appends the
/// request id, once.
pub fn unsupported_in_stage(stage: &str) -> String {
    format!(
        "The operation is not supported for directories in this state. Directory Status: \
         {stage}"
    )
}

/// The stages an alias assignment is served in: [`STAGE_ACTIVE`] alone.
///
/// `CreateAlias` creates "an alias for a directory and assigns the alias to the
/// directory", which is a change to a directory that is already serving: Directory Service
/// admits it only once the directory has reached `Active`, and refuses it while the
/// directory is still being built — with the modelled `InvalidParameterException`, since the
/// stage is a precondition of the call rather than the directory being unreachable.
pub const ALIAS_SERVING_STAGES: [&str; 1] = [STAGE_ACTIVE];

/// Why the directory in `stage` cannot be given an alias, or `None` when it can.
///
/// The stage-gate family [`stage_defect`] words, as the conditional-forwarder mutations
/// state it: the action `CreateAlias` performs, the directory the request named and the
/// stages that action needs ([`ALIAS_SERVING_STAGES`]) — so the sentence states the
/// condition that actually refused the call, and a caller is told to wait for the directory
/// rather than to fix its request. It is NOT the directory-lookup sentence
/// ([`no_such_directory`]): the directory the request named does exist, and this operation
/// answers that one under its own `EntityDoesNotExistException` for an id that resolves to
/// nothing.
pub fn alias_stage_defect(directory_id: &str, stage: &str) -> Option<String> {
    if ALIAS_SERVING_STAGES.contains(&stage) {
        return None;
    }
    Some(stage_defect(
        ALIAS_ACTION_CREATE,
        directory_id,
        &ALIAS_SERVING_STAGES,
    ))
}

/// The action `CreateAlias` states in that refusal, as its own name says it.
pub(super) const ALIAS_ACTION_CREATE: &str = "create alias";

/// The stages a password reset is served in: [`STAGE_ACTIVE`] alone.
///
/// A password is set in the directory's own domain, so `ResetUserPassword` needs a
/// directory whose domain is serving: it is refused for as long as the directory is still
/// being built.
pub const PASSWORD_RESET_SERVING_STAGES: [&str; 1] = [STAGE_ACTIVE];

/// Why the directory in `stage` cannot serve a password reset, or `None` when it can.
///
/// Directory Service states this refusal in the stage-gate family's wording
/// ([`stage_defect`]) rather than the family-neutral [`directory_unavailable`] one: the
/// action it could not perform, the directory the request named and the stage that action
/// needs — so a caller is told to wait for the stage the reset is served in. The error
/// class is still the modelled `DirectoryUnavailableException`: the request is well formed
/// and the directory is simply not ready.
pub fn password_reset_stage_defect(directory_id: &str, stage: &str) -> Option<String> {
    if PASSWORD_RESET_SERVING_STAGES.contains(&stage) {
        return None;
    }
    Some(stage_defect(
        PASSWORD_RESET_ACTION,
        directory_id,
        &PASSWORD_RESET_SERVING_STAGES,
    ))
}

/// The action `ResetUserPassword` states in that refusal, as the operation's own
/// documentation says it ("Resets the password for any user in your Managed Microsoft AD
/// or Simple AD directory").
pub(super) const PASSWORD_RESET_ACTION: &str = "reset user password";

/// The `UnsupportedOperationException` message for a multi-Region call on a directory that
/// does not replicate across Regions.
///
/// FIXED text, the rule the directory broke: multi-Region replication is a feature of the
/// Enterprise edition of AWS Managed Microsoft AD, so a directory of another type or
/// edition supports none of these calls. It quotes neither the directory nor the Region,
/// so `AddRegion` and `RemoveRegion` state the same sentence for the same unsupported
/// directory ([`Directory::replicates_across_regions`] is the predicate both gate on).
/// [`refusal`] appends the request id, once.
pub const MULTI_REGION_ENTERPRISE_ONLY: &str =
    "Multi-region replication is only supported for AWS Managed Microsoft AD (Enterprise \
     edition) directories.";

/// The verb each conditional-forwarder mutation states in that refusal: the action it could
/// not perform, as its own name says it.
pub const FORWARDER_ACTION_CREATE: &str = "create";
pub const FORWARDER_ACTION_UPDATE: &str = "update";
pub const FORWARDER_ACTION_DELETE: &str = "delete";

/// The `EntityAlreadyExistsException` message for a remote domain the directory already
/// forwards.
///
/// Directory Service answers this refusal from the DNS server that was asked to host the
/// zone, so the sentence names the zone the request asked for and the server that already
/// hosts one for it, and then states the reason. The trailing space is part of the
/// recorded diagnostic, which carries it before the suffix [`refusal`] appends.
pub fn conditional_forwarder_zone_exists(remote_domain_name: &str, dns_server: &str) -> String {
    format!(
        "Failed to create zone {remote_domain_name} on server {dns_server}. A conditional \
         forwarding zone already exists for that name. "
    )
}

/// The `ClientException` message for IP address blocks the directory already routes.
///
/// Directory Service states this refusal as the set of routes that are already there,
/// bracketed and comma-separated in the order the request listed them, and then tells the
/// caller how to recreate them. The blocks are the request's own, so a caller can read the
/// offending ones straight out of the message. [`refusal`] appends the request id, once.
///
/// This is the refusal for a block the directory ALREADY routes — one whose own add has
/// settled. A request arriving while a routing change is still being applied is refused
/// before the request is compared to the stored blocks at all, with
/// [`directory_update_in_progress`].
pub fn ip_routes_already_exist(blocks: &[String]) -> String {
    format!(
        "The IP route(s) [{blocks}] already exist. If you want to recreate them, please \
         delete the existing routes first.",
        blocks = blocks.join(", ")
    )
}

/// The `ClientException` message a change to a directory is refused with while a change
/// to that same directory is still being applied.
///
/// Directory Service serializes a directory's updates: it states which directory is
/// already being changed — the id the request named — and tells the caller to wait for
/// that change to finish before making another. [`refusal`] appends the request id, once.
pub fn directory_update_in_progress(directory_id: &str) -> String {
    format!(
        "An update is already in progress for directory {directory_id}. Please wait until \
         this update is complete before making another change."
    )
}

/// The `DirectoryAlreadySharedException` message for a directory already shared with the
/// consumer account the request names.
///
/// Directory Service states the pairing that already exists — the directory the request
/// named and the account it asked to share with, under the capitalised spelling of
/// `Account` — and NOTHING about the shared directory the earlier share minted: the
/// consumer's handle on it is not disclosed by this refusal. [`refusal`] appends the
/// request id, once.
pub fn directory_already_shared(directory_id: &str, shared_account_id: &str) -> String {
    format!("Directory {directory_id} is already shared with Account {shared_account_id}.")
}

/// The `DomainControllerLimitExceededException` message for a requested number of domain
/// controllers past the per-directory ceiling.
///
/// FIXED text with one variable part, the ceiling ([`MAX_DOMAIN_CONTROLLERS_PER_DIRECTORY`]):
/// Directory Service states the quota that was broken and not the number the request
/// asked for. [`refusal`] appends the request id, once.
pub fn domain_controllers_limit_exceeded(limit: usize) -> String {
    format!(
        "Domain controllers limit exceeded. A maximum of {limit} domain controllers may be \
         requested."
    )
}

/// The `NoAvailableCertificateException` message for enabling LDAPS on a directory holding
/// no certificate that can secure its LDAP calls.
///
/// FIXED text: Directory Service states the condition and names neither the directory nor
/// the certificate type, so one sentence serves every such refusal. [`refusal`] appends the
/// request id, once.
pub const NO_VALID_CERTIFICATE_AVAILABLE: &str =
    "Cannot enable LDAPS since there is no valid certificate available.";

/// The `InvalidLDAPSStatusException` message for an LDAPS toggle the status the setting
/// carries does not admit.
///
/// FIXED text with one variable part, the action the call asked for
/// ([`LDAPS_ACTION_ENABLE`] / [`LDAPS_ACTION_DISABLE`]): Directory Service states that the
/// LDAPS status is not one the action can be performed from, and names neither the
/// directory, nor the `LDAPSType`, nor the status itself — so a directory carrying no
/// setting at all and one carrying a status the action cannot start from are refused with
/// one sentence, worded like this family's other refusal
/// ([`NO_VALID_CERTIFICATE_AVAILABLE`]). [`refusal`] appends the request id, once.
pub fn invalid_ldaps_status(action: &str) -> String {
    format!("Cannot {action} LDAPS for a directory in an invalid LDAPS status.")
}

/// The action each LDAPS toggle states in that refusal, as its own name says it.
pub const LDAPS_ACTION_ENABLE: &str = "enable";
pub const LDAPS_ACTION_DISABLE: &str = "disable";

/// The `DirectoryInDesiredStateException` message for a toggle whose directory already has
/// the setting the request asks for.
///
/// Directory Service states that the directory IS in the state that was asked for, naming
/// the directory and nothing else: neither the setting nor the status it carries, so an
/// enable and a disable of the same setting are refused with one sentence — which is what
/// the error shape's own documentation describes ("The directory is already updated to
/// desired update type settings"). [`refusal`] appends the request id, once.
pub fn directory_in_desired_state(directory_id: &str) -> String {
    format!("Directory {directory_id} is already in the desired state.")
}

/// The message an UNSCOPED listing of snapshots refuses a `SnapshotIds` entry no snapshot
/// is stored under with.
///
/// The snapshot half of the filter-value family [`invalid_directory_id`] words for a
/// directory id: an id that arrived as the WHOLE of what a listing asks about is reported
/// as a VALUE the listing cannot be narrowed by — unquoted, singular, closed by a period —
/// rather than as an entity that does not exist, which is what the family that RESOLVES a
/// snapshot ([`no_such_snapshot`], `DeleteSnapshot` and `RestoreFromSnapshot`) reports.
/// A listing narrowed by the directory the snapshots belong to serves a page instead: there
/// the ids SELECT within that directory's snapshots, so one selecting none of them selects
/// nothing. [`refusal`] appends the request id, once.
pub fn invalid_snapshot_id(snapshot_id: &str) -> String {
    format!("Invalid snapshot ID: {snapshot_id}.")
}

/// The `DirectoryNotSharedException` message for a directory that is not shared with the
/// consumer account the request named.
///
/// The error shape's own documentation states the failure ("The specified directory has
/// not been shared with this Amazon Web Services account."); this says it of the pairing
/// the call asked about, worded the way Directory Service words a pairing of a directory
/// and an account ([`directory_already_shared`], the mirror of this refusal): the id the
/// request sent and the account it named, BOTH UNQUOTED, under the capitalised spelling of
/// `Account`, and the clause closed by a full stop. [`refusal`] appends the request id,
/// once.
pub fn directory_not_shared(directory_id: &str, shared_account_id: &str) -> String {
    format!("Directory {directory_id} is not shared with Account: {shared_account_id}.")
}

/// The `EntityDoesNotExistException` message for a remote domain a directory forwards
/// nowhere. Shared so every operation that resolves a stored conditional forwarder
/// (`DeleteConditionalForwarder`, `UpdateConditionalForwarder`) says the same thing about
/// the same failure.
///
/// Worded the way the directory-lookup family is ([`no_such_directory`]): the kind of
/// entity that could not be resolved, the quoted name the request asked for, then "does
/// not exist." — so the message is built from the request and names what the caller has
/// to fix, and nothing else. [`refusal`] appends the request id, once.
pub fn no_such_conditional_forwarder(remote_domain_name: &str) -> String {
    format!("Conditional forwarder: '{remote_domain_name}' does not exist.")
}

/// The `EntityDoesNotExistException` message for a directory that forwards its logs
/// nowhere. Shared so `DeleteLogSubscription` and any later operation that resolves a
/// stored log subscription say the same thing about the same failure.
///
/// A log subscription is identified by the directory it belongs to, so the wording follows
/// Directory Service's own for this failure: the entity that could not be resolved, then
/// the id the request asked for. [`refusal`] appends the request id, once.
pub fn no_such_log_subscription(directory_id: &str) -> String {
    format!("Log subscription does not exist for directory: {directory_id}.")
}

/// The `EntityAlreadyExistsException` message for a directory that already forwards its
/// logs to a log group.
///
/// The mirror of [`no_such_log_subscription`], worded from the request the same way: a
/// directory holds one log subscription, so a second create names the directory whose
/// subscription already exists. [`refusal`] appends the request id, once.
pub fn log_subscription_exists(directory_id: &str) -> String {
    format!("Log subscription already exists for directory: {directory_id}.")
}

/// The `EntityDoesNotExistException` the tag API answers for a `ResourceId` that names
/// no taggable resource.
///
/// `AddTagsToResource` / `ListTagsForResource` / `RemoveTagsFromResource` take a
/// `ResourceId`, not a `DirectoryId`, and the recorded refusal of a well-formed but
/// absent one is the tag family's own diagnostic: it says the id is not a resource this
/// service can tag, and QUOTES NO ID — unlike the directory-lookup family
/// ([`no_such_directory`]), which names the id it could not resolve. The two families
/// are what a caller tells "the id you passed is not a taggable resource id" from "this
/// directory is gone" by, so they must not be collapsed into one template.
///
/// Built through [`refusal_restating_its_request_id`]: the recorded refusal of all three
/// operations carries the call's id in the message twice.
pub fn invalid_resource_id<E: ModeledFailure>() -> E {
    refusal_restating_its_request_id(INVALID_RESOURCE_ID)
}

/// The refusal of a `NextToken` this service did not issue, or issued for another
/// listing.
///
/// The diagnostic is [`INVALID_NEXT_TOKEN`] — the sentence this service words for a
/// rejected page token, which names the MEMBER whose value it rejected — with the
/// family-wide request-id suffix [`refusal`] appends. The error SHAPE is the variant the
/// caller constructs this into, because not every paginated operation of this service
/// models `InvalidNextTokenException` for it.
pub fn invalid_next_token<E: ModeledFailure>() -> E {
    refusal(INVALID_NEXT_TOKEN)
}

/// The diagnostic every paginated listing of this service refuses an unusable
/// `NextToken` with, which [`invalid_next_token`] is what builds the refusal from.
///
/// Directory Service states this refusal as a rejected PARAMETER VALUE — it names the
/// input member whose value it would not accept, and quotes neither the token nor the
/// listing — so one text serves every paginated operation of this service and discloses
/// nothing about the token it was handed. It is NOT the error shape's documentation prose
/// ("The `NextToken` value is not valid."), which describes the class to a reader of the
/// model rather than stating what the service puts on the wire. [`refusal`] appends the
/// request id, once.
pub const INVALID_NEXT_TOKEN: &str = "Invalid value passed for the NextToken parameter.";

/// The tag family's diagnostic, without either request-id suffix: see
/// [`invalid_resource_id`], which is what builds the refusal.
pub(super) const INVALID_RESOURCE_ID: &str = "Invalid resource ID.";

/// The diagnostic of the modelled `AuthenticationFailedException`: the sentence the error
/// shape's own documentation states for this class
/// (`smithy_query(query="doc",shape="AuthenticationFailedException")`), which is what
/// Directory Service answers when it could not authenticate the account a call carried.
///
/// FIXED text, naming nothing of the request: which member of a credential pair was wrong
/// is exactly what an authentication failure does not disclose. Shared so the operations
/// that take an alternate directory account's credentials (`EnableSso`, `DisableSso`) say
/// the same thing about the same failure. [`refusal`] appends the request id, once.
pub const AUTHENTICATION_FAILED: &str = "An authentication error occurred.";

/// The ARN of the Amazon SNS topic named `topic_name` in `region`.
///
/// Built through [`::engine::emulator_arn`], so the account segment is the one
/// canonical account id every generated emulator presents. An SNS topic ARN's resource
/// portion is the bare topic name.
pub fn sns_topic_arn(region: &str, topic_name: &str) -> String {
    ::engine::emulator_arn(SNS_ARN_SERVICE, region, topic_name)
}

/// The Region a reported ARN carries, and the Region an operation that acts per Region
/// (`RemoveRegion`) acts on: the Region of the caller's sigv4 credential scope, or
/// [`DEFAULT_REGION`] when the request carries none.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single source
/// of SigV4-scope parsing, so a header that is not a well-formed sigv4 `Authorization`
/// yields the default rather than a bogus Region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `UserDoesNotExistException` message for a name that names no account of the
/// directory the request asked about.
///
/// Directory Service words this one as the kind of entity, a colon, the name the request
/// sent (unquoted) and "does not exist.", so the message is built from the request and
/// names what the caller has to fix, and nothing else. It is NOT the quoted form the
/// directory-lookup family uses ([`no_such_directory`]): the two families are worded
/// differently, so neither may be built from the other.
/// [`refusal`] appends the request id.
pub fn no_such_user(user_name: &str) -> String {
    format!("User: {user_name} does not exist.")
}

/// The fewest characters a directory accepts in an account password.
///
/// From `CreateDirectoryRequest$Password`'s own documentation, which states the directory
/// password policy this service enforces: "Length `(?=^.{8,64}$)` - Must be between 8 and
/// 64 characters".
pub const MIN_PASSWORD_CHARS: usize = 8;

/// The most characters a directory accepts in an account password, from the other half of
/// that same sentence. The modelled `UserPassword` admits up to 127 characters, so a
/// password between the two bounds is one the boundary validator passes and the
/// directory's own policy refuses.
pub const MAX_PASSWORD_CHARS: usize = 64;

/// How many of the four Active Directory character classes a password must draw on.
///
/// From the same documentation: a password must satisfy the length rule "AND any 3 of the
/// following password complexity rules required by Active Directory": numbers, upper
/// case, lower case and special characters, in the four combinations of three it lists,
/// which is exactly "at least three of the four classes".
pub const PASSWORD_CHARACTER_CLASSES: usize = 3;

/// Why `password` does not meet the password policy of a directory of this service, or
/// `None` when it does.
///
/// The diagnostic is [`PASSWORD_COMPLEXITY_REFUSED`], the sentence Directory Service
/// states for this refusal — FIXED text, which is why both halves of the policy report
/// it unchanged and the password itself, `@sensitive`, appears nowhere in it.
///
/// The rule is the one `CreateDirectoryRequest$Password`'s documentation states for the
/// administrator password, which is the directory's own policy and so the policy every
/// later password of that directory is set under: between [`MIN_PASSWORD_CHARS`] and
/// [`MAX_PASSWORD_CHARS`] characters, drawn from at least [`PASSWORD_CHARACTER_CLASSES`]
/// of Active Directory's four character classes.
pub fn password_policy_defect(password: &str) -> Option<&'static str> {
    let characters = password.chars().count();
    if !(MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&characters) {
        return Some(PASSWORD_COMPLEXITY_REFUSED);
    }
    let classes = [
        password.chars().any(char::is_numeric),
        password.chars().any(char::is_uppercase),
        password.chars().any(char::is_lowercase),
        password
            .chars()
            .any(|character| !character.is_alphanumeric() && !character.is_whitespace()),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if classes < PASSWORD_CHARACTER_CLASSES {
        return Some(PASSWORD_COMPLEXITY_REFUSED);
    }
    None
}

/// The sentence Directory Service refuses a password its directory's policy does not
/// accept with: "Could not reset user password, because the password does not meet the
/// complexity requirements."
///
/// FIXED text with no variable part — the password is `@sensitive` and the policy is the
/// directory's, so neither appears in it. [`refusal`] appends the request id, once.
pub const PASSWORD_COMPLEXITY_REFUSED: &str =
    "Could not reset user password, because the password does not meet the complexity \
     requirements.";

/// The word a directory's administrator password may not contain, whatever its case.
///
/// Part of the policy Directory Service states for the password a create makes the
/// directory's administrator account with ([`INVALID_ADMIN_PASSWORD`]) — the name of that
/// account, which a password may not give away. Not expressible in the modelled `Password`
/// pattern, so it is the half of the policy only the create can enforce.
const ADMIN_WORD: &str = "admin";

/// Why `password` cannot be the password of the administrator account a create makes, or
/// `None` when it can.
///
/// ONE rule for every create that makes a directory's administrator account
/// (`CreateDirectory`, `CreateMicrosoftAD`), so neither can accept a password the other
/// refuses: the policy the `Password` member's own documentation states — between
/// [`MIN_PASSWORD_CHARS`] and [`MAX_PASSWORD_CHARS`] characters, drawn from at least
/// [`PASSWORD_CHARACTER_CLASSES`] of Active Directory's four character classes
/// ([`password_policy_defect`]) — AND the clause that documentation does not express and
/// the modelled pattern therefore cannot refuse: the password may not contain the word
/// [`ADMIN_WORD`], the name of the account it is the password of, in any case.
///
/// The diagnostic is [`INVALID_ADMIN_PASSWORD`], the sentence Directory Service states the
/// WHOLE policy in, so a caller is told the rule it broke whichever half refused it, and
/// the `@sensitive` password appears nowhere in it.
pub fn administrator_password_defect(password: &str) -> Option<&'static str> {
    if password_policy_defect(password).is_some() {
        return Some(INVALID_ADMIN_PASSWORD);
    }
    // Case-insensitively, since the clause is about the WORD rather than its spelling. The
    // length half of the policy ran first, so the lower-casing below is allocated only for
    // a password already bounded by [`MAX_PASSWORD_CHARS`] characters.
    if password.to_lowercase().contains(ADMIN_WORD) {
        return Some(INVALID_ADMIN_PASSWORD);
    }
    None
}

/// The sentence Directory Service refuses a create whose `Password` cannot be the
/// directory administrator's with.
///
/// FIXED text: it states the policy in full and quotes neither the password (it is
/// `@sensitive`) nor which half of the policy the password broke, so every caller that
/// breaks it reads the same sentence. It is NOT [`PASSWORD_COMPLEXITY_REFUSED`], the
/// sentence a RESET of an existing account's password is refused with: the two refusals are
/// worded differently, so neither may be built from the other. [`refusal`] appends the
/// request id, once.
pub const INVALID_ADMIN_PASSWORD: &str =
    "Invalid password. The password must be between 8 and 64 characters, not contain the \
     word \"admin\", and must have at least one character from three of these four \
     categories: lowercase, uppercase, numeric, and special characters.";

/// The sentence the modelled `DirectoryAlreadyInRegionException` states this refusal
/// with: "The Region you specified is the same Region where the Managed Microsoft AD
/// directory was created. Specify a different Region and try again."
///
/// FIXED text: the error shape's own documentation states it with no variable part, so
/// neither the Region nor the directory the request named appears in it. [`refusal`]
/// appends the request id, once.
pub const DIRECTORY_ALREADY_IN_REGION: &str =
    "The Region you specified is the same Region where the Managed Microsoft AD directory \
     was created. Specify a different Region and try again.";

/// The sentence `AddRegion` refuses a request naming the directory's OWN home Region
/// with.
///
/// The primary Region is not a replication at all, so this is refused as an operation
/// that is not supported rather than as a Region the directory is already in — and
/// Directory Service words that refusal with this sentence, FIXED text with no variable
/// part. [`refusal`] appends the request id, once.
pub const REGION_IS_THE_PRIMARY_REGION: &str =
    "The region you specified is the same region where the AWS Managed Microsoft AD \
     directory was created. Specify a different region and try again.";

/// The most Regions this emulator replicates one directory to beside its primary one.
///
/// NOT an AWS quota: a resource guard. `AddRegion` is a write a caller can repeat with a
/// fresh Region name forever, so the per-directory set carries its own bound, enforced at
/// the single insert site in `add_region.rs`, which is what the modelled
/// `RegionLimitExceededException` ("You have reached the limit for maximum number of
/// simultaneous Region replications per directory") then refuses.
pub const MAX_ADDITIONAL_REGIONS: usize = 5;

/// The `EntityDoesNotExistException` message for an id that names no stored snapshot.
/// Shared so every operation that resolves a snapshot says the same thing about the
/// same failure.
///
/// Directory Service words this one as the kind of entity, a colon, the id the request
/// asked for and "does not exist" — so the message is built from the request and names
/// what the caller has to fix, and nothing else. [`refusal`] appends the request id.
pub fn no_such_snapshot(snapshot_id: &str) -> String {
    format!("Snapshot: {snapshot_id} does not exist")
}

/// The `EntityDoesNotExistException` message for an id that names no stored trust
/// relationship. Shared so every operation that resolves a trust says the same thing
/// about the same failure.
///
/// Directory Service words this one as the kind of entity, a colon, the id the request
/// asked for — UNQUOTED — and "does not exist", with no full stop closing the sentence:
/// the recorded bodies for this error class read `Trust: <trust id> does not exist`
/// before [`refusal`] appends the request id. So the message is built from the request
/// and names what the caller has to fix, and nothing else.
pub fn no_such_trust(trust_id: &str) -> String {
    format!("Trust: {trust_id} does not exist")
}

/// The `EntityDoesNotExistException` message for an id that names no domain controller of
/// the directory the request asked about.
///
/// Worded the way this service's lookup family is ([`no_such_directory`],
/// [`no_such_trust`]): the kind of entity, the quoted id the request sent and "does not
/// exist.", so the message is built from the request and names what the caller has to fix.
/// [`refusal`] appends the request id.
pub fn no_such_domain_controller(domain_controller_id: &str) -> String {
    format!("DomainController: '{domain_controller_id}' does not exist.")
}

/// Why a directory that already carries an alias cannot be given another one, as Directory
/// Service states it. Shared so the one operation that refuses it and the suite that pins
/// its wording name the same sentence.
///
/// FIXED text, the rule the request broke and what the caller would have to do about it: it
/// quotes neither the directory nor either alias, because the request is refused for what
/// the directory IS rather than for the value it named — the caller already has both.
/// [`refusal`] appends the call's request id, once, which is the only part of the message
/// that varies.
pub const ALIAS_ALREADY_ASSIGNED: &str = "The directory in the request already has an alias. \
     That alias must be deleted before a new alias can be created.";

/// The `CertificateDoesNotExistException` message for an id that names no certificate
/// registered with the directory the request asked about. Shared so every operation that
/// resolves a registration (`DescribeCertificate`, `DeregisterCertificate`) says the
/// same thing about the same failure.
///
/// Directory Service words this one as the kind of entity, the id the request asked for,
/// "does not exist for" and the member label of the directory it was looked for in, so the
/// message names BOTH ids the lookup was made with and the fixed text names which member
/// each belongs to. The error shape's own documentation sentence is prose describing the
/// error CLASS, not the message the service sends. [`refusal`] appends the request id,
/// once.
pub fn no_such_certificate(certificate_id: &str, directory_id: &str) -> String {
    format!("Certificate {certificate_id} does not exist for directoryId: {directory_id}")
}

/// The `CertificateAlreadyExistsException` message for a certificate a directory already
/// holds: that error shape's own sentence, FIXED text with no interpolated value.
pub const CERTIFICATE_ALREADY_REGISTERED: &str =
    "The certificate has already been registered into the system.";

/// The `CertificateInUseException` message for a registration the directory's live LDAP
/// security rests on: that error shape's own sentence, FIXED text with no interpolated
/// value.
pub const CERTIFICATE_IN_USE: &str =
    "The certificate is being used for the LDAP security connection and cannot be removed \
     without disabling LDAP security.";

/// The `CertificateLimitExceededException` message for a registration past
/// [`MAX_CERTIFICATES_PER_DIRECTORY`]: that error shape's own sentence, FIXED text with no
/// interpolated value.
pub const CERTIFICATE_LIMIT_REACHED: &str =
    "The certificate could not be added because the certificate limit has been reached.";

/// The `InvalidCertificateException` message for `CertificateData` this service cannot
/// read as a certificate.
///
/// Directory Service words this one as the fact — the certificate is not valid — followed
/// by the registration it could not perform and the directory the request named it for, so
/// the message is built from the request and nothing else: the certificate data itself is
/// the caller's and is not echoed, and the sentence carries no closing full stop.
/// [`refusal`] appends the request id, once.
pub fn certificate_not_valid(directory_id: &str) -> String {
    format!(
        "Certificate is not valid. Could not register the certificate with the directory \
         {directory_id}"
    )
}

/// The `EntityDoesNotExistException` message for an id that names no schema extension of
/// the directory the request asked about.
///
/// A schema extension is an extension OF a directory, so the message names both ids the
/// request sent and nothing else, so the caller can tell which id was rejected.
/// [`refusal`] appends the request
/// id.
pub fn no_such_schema_extension(schema_extension_id: &str, directory_id: &str) -> String {
    format!(
        "Schema extension {schema_extension_id} does not exist for directory {directory_id}"
    )
}

/// The `EntityDoesNotExistException` message for an id that names no stored assessment.
///
/// It names the member and the id the request sent, the way Directory Service's own
/// message for this failure does, so the caller can tell which id was rejected.
/// [`refusal`] appends the request id.
pub fn no_such_assessment(assessment_id: &str) -> String {
    format!("Assessment Id {assessment_id} does not exist.")
}
