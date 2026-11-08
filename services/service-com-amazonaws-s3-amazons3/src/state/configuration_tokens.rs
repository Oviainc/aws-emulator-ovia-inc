// SPDX-License-Identifier: CC0-1.0

//! The ONE continuation token every PAGED listing of this service hands back, and the one
//! reading of a token a caller hands in.
//!
//! (The file keeps the name it was written under, when the four listings of a bucket's
//! id-addressed configurations were the only pagers that minted one; nothing but the module
//! path depends on it.)
//!
//! Each of those listings pages, and the token that resumes a truncated page is the
//! service's to mint and the caller's to hand back unread: "the continuation token is an
//! opaque value that Amazon S3 understands"
//! (`ListBucketMetricsConfigurationsOutput$NextContinuationToken`), "the token is
//! obfuscated and is not a usable value"
//! (`ListBucketAnalyticsConfigurationsOutput$NextContinuationToken`) and "ContinuationToken
//! is obfuscated and is not a real bucket name"
//! (`ListDirectoryBucketsRequest$ContinuationToken`). Two things follow, and neither is
//! served by rendering the cursor itself:
//!
//! * a token must not be a plain rendering of the position it resumes at — a value whose
//!   text IS a stored id invites a caller to compute the next one rather than round-trip
//!   what it was handed; and
//! * a value the caller did not receive from this service names no page of the listing, so
//!   it is refused ([`super::MALFORMED_CONTINUATION_TOKEN_CODE`]) rather than read as a
//!   cursor into a position the caller chose.
//!
//! So a token carries an AUTHENTICATOR over its cursor, computed under a secret the store
//! draws from the system's randomness and never reveals
//! ([`::service_common::page_tokens::PageTokenKey`], the workspace's one reading of that):
//! no caller can produce the authenticator for a cursor of its own, and a token minted by
//! another store — another process, a restarted one — names no page here either. The
//! cursor is SCOPED to the listing it was issued for, so a token from one listing is not a
//! cursor into another. Opaque, NOT confidential, exactly as that module
//! puts it: what a caller cannot do is make a token of its own.
//!
//! No page is kept on the side: the cursor is the first thing the page did not report — a
//! configuration id, a bucket name — which is why every one of these listings walks what it
//! reports in ascending order.

use ::service_common::jwt::{decode_base64url, encode_base64url};

pub use ::service_common::page_tokens::PageTokenKey;

/// The listing a continuation token was issued for.
///
/// Named inside the token so a `NextContinuationToken` from one listing names no
/// page of another's: the four families a bucket holds per-id configurations of, each bound
/// at its own subresource (`?analytics`, `?metrics`, `?inventory`,
/// `?intelligent-tiering`), and the account's DIRECTORY buckets, which pages by bucket name
/// on its own endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedListing {
    /// `ListBucketAnalyticsConfigurations`.
    Analytics,
    /// `ListDirectoryBuckets`.
    DirectoryBuckets,
    /// `ListBucketIntelligentTieringConfigurations`.
    IntelligentTiering,
    /// `ListBucketInventoryConfigurations`.
    Inventory,
    /// `ListBucketMetricsConfigurations`.
    Metrics,
}

impl PagedListing {
    /// The name this listing is authenticated under, inside the token. One name per listing
    /// and no part of any response — the subresource a configuration listing is bound at,
    /// and the resource the bucket listing reports.
    fn label(self) -> &'static str {
        match self {
            Self::Analytics => "analytics",
            Self::DirectoryBuckets => "directory-buckets",
            Self::IntelligentTiering => "intelligent-tiering",
            Self::Inventory => "inventory",
            Self::Metrics => "metrics",
        }
    }
}

/// The separator between the listing a token was issued for and the position in it: a unit
/// separator, which no listing's label and no cursor carries (no configuration id
/// ([`super::CONFIGURATION_ID_PATTERN`]) and no bucket name admits it), so the two halves of
/// a cursor cannot run together.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The `NextContinuationToken` that resumes a listing at `first_not_returned` (inclusive):
/// the authenticated bytes of that position, rendered in base64url.
///
/// Base64 is the spelling an AWS continuation token has, in the URL-safe alphabet (RFC 4648
/// §5) so the value survives the `continuation-token` query parameter of the next request
/// whatever encodes it — through the workspace's one base64url codec
/// ([`::service_common::jwt::encode_base64url`], declared beside the JOSE reader that needed
/// it first), so no second alphabet swap lives here. The authenticator inside is what makes a
/// token this store did not mint detectable, which is what lets the next page refuse it
/// instead of serving from a position the caller chose.
pub fn encode_continuation_token(
    key: &PageTokenKey,
    listing: PagedListing,
    first_not_returned: &str,
) -> String {
    encode_base64url(&key.mint(&format!(
        "{}{CURSOR_SEPARATOR}{first_not_returned}",
        listing.label()
    )))
}

/// The position `token` resumes `listing` at (inclusive), or `None` for any value
/// this store did not mint for that listing: one that is not base64url, one whose
/// authenticator does not match the cursor it travels with, one issued for another
/// listing, and one carrying no cursor at all.
pub fn decode_continuation_token(
    key: &PageTokenKey,
    listing: PagedListing,
    token: &str,
) -> Option<String> {
    let cursor = key.cursor_of(&decode_base64url(token)?)?;
    let (label, resume_at) = cursor.split_once(CURSOR_SEPARATOR)?;
    (label == listing.label()).then(|| resume_at.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MAX_CONFIGURATION_ID_CHARS;

    /// Every listing's token names its position under the key that minted it, and nothing
    /// else does: another store's key, another family's listing, or a value a caller wrote.
    #[test]
    fn a_token_names_a_position_only_for_the_store_and_listing_that_issued_it() {
        let key = PageTokenKey::new();
        let other_store = PageTokenKey::new();
        let listings = [
            PagedListing::Analytics,
            PagedListing::DirectoryBuckets,
            PagedListing::IntelligentTiering,
            PagedListing::Inventory,
            PagedListing::Metrics,
        ];
        for listing in listings {
            // Cursors spanning what one can be: one character, every character class a
            // configuration id allows, one at the published ceiling, and a directory
            // bucket's name.
            for id in [
                "a".to_string(),
                "Report.one-2_3".to_string(),
                "z".repeat(MAX_CONFIGURATION_ID_CHARS),
                "base--usw2-az1--x-s3".to_string(),
            ] {
                let token = encode_continuation_token(&key, listing, &id);
                assert_eq!(
                    decode_continuation_token(&key, listing, &token).as_deref(),
                    Some(id.as_str()),
                    "case {listing:?}/{id:?}: a minted token must name its position, got \
                     {token:?}"
                );
                assert_eq!(
                    decode_continuation_token(&other_store, listing, &token),
                    None,
                    "case {listing:?}/{id:?}: another store's key must name no position"
                );
                for elsewhere in listings.iter().filter(|other| **other != listing) {
                    assert_eq!(
                        decode_continuation_token(&key, *elsewhere, &token),
                        None,
                        "case {listing:?}/{id:?}: must name no position in {elsewhere:?}"
                    );
                }
                // The token is not a RENDERING of the position it resumes at: neither the
                // id itself nor any value a caller could compute from it without the key.
                for (spelling, computed) in [
                    ("the id itself", id.clone()),
                    ("hex of the id", ::hex::encode(&id)),
                    (
                        "base64 of the id",
                        ::aws_smithy_types::base64::encode(id.as_str()),
                    ),
                ] {
                    assert_ne!(
                        token, computed,
                        "case {listing:?}/{id:?}: the token must not be {spelling}, got \
                         {token:?}"
                    );
                }
                assert!(
                    token
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_')),
                    "case {listing:?}/{id:?}: a token is URL-safe base64, got {token:?}"
                );
            }
        }

        // Values a caller could write rather than round-trip, including one with the SHAPE
        // of a minted token, which only the authenticator can refuse.
        let shaped =
            encode_base64url(&[0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES + 1]);
        for (case, token) in [
            ("an id", "kept".to_string()),
            ("empty", String::new()),
            ("not base64url", "not a token".to_string()),
            ("base64url but too short", "a2VwdA".to_string()),
            ("a minted token's shape", shaped),
        ] {
            assert_eq!(
                decode_continuation_token(&key, PagedListing::Metrics, &token),
                None,
                "case {case}: expected no position for {token:?}"
            );
        }
    }
}
