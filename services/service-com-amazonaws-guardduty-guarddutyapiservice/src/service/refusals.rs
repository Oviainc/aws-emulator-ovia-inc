// SPDX-License-Identifier: CC0-1.0

//! The refusal sentences this service answers where the text is the SAME for every
//! reason the request was refused, and names neither the member it arrived on nor
//! the value it carried.
//!
//! A refusal's message is part of the contract a client codes against — it is
//! snapshotted, branched on and scraped — so each class of refusal must carry ONE
//! text, reproduced as GuardDuty words it. Most of this service's refusals are built
//! per resource family beside the records they are about (`crate::state`); the ones
//! here belong to no family, because the service answers them identically across
//! every operation that can meet them:
//!
//! * an unresolvable tagging ARN ([`resource_arn_rejection`]), on the tag read, the
//!   tag write and the tag removal alike;
//! * a page token the service never issued ([`page_token_rejection`]), on every
//!   paginated report;
//! * a `findingIds` entry that could not be a finding id at all
//!   ([`malformed_finding_id_rejection`]), in both directions of the archive write.
//!
//! They live beside this service's other refusal wording — `override_refusal` in
//! the parent module, which words what the request boundary refused — so a reader
//! looking for "how does this service word a refusal" finds all of it in one place,
//! and so no handler can spell one of these sentences a second way. The per-family
//! refusals stay in `crate::state`; these ones could not join them, because that
//! module is past this project's per-file line limit and may not grow until it is
//! split into one file per concern (its `resource_arn_rejection`, which these
//! handlers no longer call, is superseded by [`resource_arn_rejection`] here and
//! goes with that split).

/// The sentence the tagging surface refuses a `ResourceArn` it resolves no taggable
/// resource for with: ONE fixed text, for every reason the ARN did not resolve.
///
/// An ARN naming a resource nothing minted, one claiming another account, partition
/// or Region, one addressed to another Region's endpoint, a sub-resource id the
/// detector does not hold, and a resource type this service does not tag are the
/// same answer: the ARN the request supplied is not an input this service can act
/// on. It is NOT `crate::state::not_owned_message`, which claims the resource
/// belongs to another ACCOUNT and would point a caller with a typo in the ARN at its
/// IAM configuration instead, and it names neither the member nor the ARN.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn";

/// The message a `ResourceArn` this endpoint resolves no taggable resource for is
/// refused with: [`INVALID_RESOURCE_ARN_MESSAGE`], owned by this one reader so
/// `ListTagsForResource`, `TagResource` and `UntagResource` cannot drift apart.
///
/// It takes no ARN: the one sentence answers every class of unresolvable ARN, so
/// nothing of the value the request sent can reach the answer.
pub fn resource_arn_rejection() -> String {
    INVALID_RESOURCE_ARN_MESSAGE.to_string()
}

/// The message EVERY paginated report of this service refuses a page token it cannot
/// resolve to a position in the list it is paging with.
///
/// A `NextToken` is an opaque value the service itself hands out, so a token it did
/// not issue — one that is not a cursor at all, one that is a cursor into another
/// listing, and one that points past the end of this one — is the same fact: a value
/// the service cannot use, which it reports with its generic input-parameter
/// sentence (`crate::state::INVALID_INPUT_VALUE_MESSAGE`) and no mention of the
/// member the token arrived on. One reader for the whole pagination surface, so no
/// listing can drift from the others on how it refuses a token.
pub fn page_token_rejection() -> String {
    crate::state::INVALID_INPUT_VALUE_MESSAGE.to_string()
}

/// The message a `findingIds` entry that could not be a finding id at all is
/// refused with, and the first entry of `finding_ids` that is one.
///
/// A finding `Id` is one of the ids this service mints
/// (`crate::state::is_minted_id`), so a value that is not of that form could not
/// have been meant as a finding id: the service answers that with its generic
/// input-parameter sentence, naming no member and echoing nothing. A value that IS
/// of that form but names no finding of this detector is a DIFFERENT class, which
/// the store answers for itself with the finding-identifier sentence
/// (`crate::state::ComAmazonawsGuarddutyGuarddutyapiserviceState::set_findings_archived_in`
/// through `crate::state::finding_id_rejection`).
///
/// `FindingIds` is modelled as a plain list of strings
/// (`smithy_query(operation="ArchiveFindings")`), so the FORM of an entry is the
/// handler's to check, and it is checked before the store is consulted — a refused
/// batch moves no finding — exactly as the filter-name alphabet is checked before
/// `CreateFilter` touches the store.
pub fn malformed_finding_id_rejection<'a, I>(finding_ids: I) -> Option<String>
where
    I: IntoIterator<Item = &'a String>,
{
    finding_ids
        .into_iter()
        .find(|id| !crate::state::is_minted_id(id))
        .map(|_| crate::state::INVALID_INPUT_VALUE_MESSAGE.to_string())
}
