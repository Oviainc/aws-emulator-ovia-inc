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
/// in service yet. Shared so every operation that needs a directory in service says the
/// same thing about the same failure.
///
/// Directory Service states this refusal with the SAME sentence it states for an id that
/// resolves to no directory at all ([`no_such_directory`]): the quoted id the request
/// asked for, then "does not exist." It reports nothing about where in its build the
/// directory is, so the diagnostic discloses no internal provisioning state and a caller
/// cannot read one out of the message. [`refusal`] appends the request id, once.
pub fn directory_unavailable(directory_id: &str) -> String {
    no_such_directory(directory_id)
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
/// read as a certificate: that error shape's own sentence, FIXED text with no interpolated
/// value — the data itself is the caller's and is not echoed.
pub const CERTIFICATE_ENCODING_INVALID: &str =
    "The certificate PEM that was provided has incorrect encoding.";

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
