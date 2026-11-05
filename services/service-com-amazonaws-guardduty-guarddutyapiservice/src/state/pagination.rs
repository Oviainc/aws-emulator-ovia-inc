// SPDX-License-Identifier: CC0-1.0

//! Pagination: the listing a page token names a position in, and the page cut
//! itself.
//!
//! Every paginated report of this service walks a total order through here, so a
//! token handed back by one call means the same position on the next.

/// WHICH ordered list a page token is a position in: the report that minted it,
/// plus every request value that selects or orders the entries it walks.
///
/// A position only means something for the list it was taken in, so the token
/// carries this identity beside the position: a token handed to any other listing
/// resolves against a different identity and is refused, instead of silently
/// resuming in the middle of a list the caller never paged.
///
/// Built by the report that pages: [`Self::of`] names the operation and each
/// [`Self::by`] adds one value the selection or the order depends on.
#[derive(Debug, Clone)]
pub struct Listing {
    /// The identity, rendered: the operation name and the selectors, separated by a
    /// character no rendering of a selector can contain unescaped.
    pub(super) identity: String,
}

/// How many bytes of the identity's digest a token carries. Eight is far more
/// distinct listings than this service can hold, and keeps the token short.
pub(super) const LISTING_TAG_BYTES: usize = 8;

/// The tag's width in a token, which is where the token splits into tag and
/// position: a byte renders as two hex digits.
pub(super) const LISTING_TAG_WIDTH: usize = LISTING_TAG_BYTES * 2;

/// The width the position is rendered to, zero-padded: the minimum width, so every
/// token of a list this service can page comes out the same length.
pub(super) const LISTING_POSITION_WIDTH: usize = 8;

impl Listing {
    /// The list `operation` reports, before any selector narrows or orders it.
    pub fn of(operation: &str) -> Self {
        Self {
            identity: operation.to_string(),
        }
    }

    /// Add one request value the selection or the ordering of the entries depends on
    /// — a resource the list belongs to, a criteria structure, a sort order.
    ///
    /// Rendered with `Debug`, so a modelled structure identifies the selection it
    /// makes without the caller spelling its members out, and two different
    /// selections cannot render alike. The rendering must be the SAME for two
    /// requests carrying the same value, or a caller's own token stops resolving:
    /// a selector holding a `HashMap` is added through [`Self::by_entries`]
    /// instead, because a hash map iterates in an order private to the instance the
    /// request was deserialized into.
    #[must_use]
    pub fn by(mut self, selector: impl std::fmt::Debug) -> Self {
        // A `Debug` rendering escapes control characters, so the unit separator
        // appears in the identity only where this puts it.
        self.identity.push('\u{1f}');
        self.identity.push_str(&format!("{selector:?}"));
        self
    }

    /// Add a MAP-valued selector: its entries, each rendered with its key, in KEY
    /// order.
    ///
    /// What such a selector selects by is the SET of entries the request carried, so
    /// that set is the identity — never the order the map hands them out, which is
    /// the iteration order of one deserialized instance and differs between two
    /// requests carrying the very same criteria.
    #[must_use]
    pub fn by_entries<K: Ord + std::fmt::Debug, V: std::fmt::Debug>(
        self,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut rendered: Vec<String> = entries
            .into_iter()
            .map(|(key, value)| format!("{key:?}={value:?}"))
            .collect();
        rendered.sort();
        self.by(rendered)
    }

    /// The tag a token of this listing carries.
    pub(super) fn tag(&self) -> String {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(self.identity.as_bytes());
        hex::encode(&digest[..LISTING_TAG_BYTES])
    }

    /// The token that asks this listing to resume at `position`.
    pub(super) fn token_at(&self, position: usize) -> String {
        token_spelling(&self.tag(), position)
    }

    /// The position `token` names in THIS listing, or `None` when it is not a token
    /// this listing issued.
    ///
    /// A token is OPAQUE: the one string that resolves to a position is the one
    /// [`Self::token_at`] writes for it, so every other way of spelling that same
    /// position — a position padded to another width, a hexadecimal digit in upper
    /// case, a sign in front of it — resolves to nothing, exactly like a token
    /// belonging to another listing. This is what makes the token reader accept
    /// nothing but what the token minter wrote: the two cannot drift, because the
    /// reader decides by SPELLING the candidate position again and comparing.
    pub(super) fn position_in(&self, token: &str) -> Option<usize> {
        let tag = self.tag();
        let (carried, position) = token.split_at_checked(LISTING_TAG_WIDTH)?;
        if carried != tag {
            return None;
        }
        // `from_str_radix` is the lax step — it reads spellings this listing never
        // writes — so its answer is only a CANDIDATE until the round trip below
        // confirms this listing would have written exactly `token` for it.
        let position = usize::from_str_radix(position, 16).ok()?;
        (token_spelling(&tag, position) == token).then_some(position)
    }
}

/// How a token is SPELLED: the listing's `tag` followed by `position` in hexadecimal,
/// zero-padded to [`LISTING_POSITION_WIDTH`].
///
/// The one place the spelling is written, so the token a report issues and the token
/// it accepts are the same string by construction.
pub(super) fn token_spelling(tag: &str, position: usize) -> String {
    format!("{tag}{position:0width$x}", width = LISTING_POSITION_WIDTH)
}

/// Take one page out of the ordered key list `listing` identifies.
///
/// `token` is one THIS listing issued: it carries the listing's identity beside the
/// number of entries already returned, and the only thing a caller may do with it
/// is hand it back to the same listing. `Err(token)` means the token was not one
/// this listing issued: it does not resolve at all, it is another SPELLING of a
/// position than the one a token carries, it belongs to another listing, or it
/// points past the end of the list, which a listing never hands out (it answers
/// the last page with no `NextToken`).
pub fn paginate(
    listing: &Listing,
    keys: &[String],
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(Vec<String>, Option<String>), String> {
    let (window, next) = paginate_window(listing, keys.len(), max_results, token, page_limit)?;
    Ok((keys[window].to_vec(), next))
}

/// The WINDOW of an ordered list of `len` entries one page covers, and the token
/// the page after it is asked for with.
///
/// [`paginate`] over the keys themselves; this is for a caller that holds the
/// ORDERED RECORDS and wants that same page of them without looking any record up
/// again by key. Both go through this one function, so the token a report issues
/// and the token it accepts cannot drift between them. `Err(token)` is the token
/// rejection [`paginate`] documents.
pub fn paginate_window(
    listing: &Listing,
    len: usize,
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(std::ops::Range<usize>, Option<String>), String> {
    let start = match token.filter(|t| !t.is_empty()) {
        Some(t) => {
            // A token resolves only against the listing that minted it, and names an
            // entry of it that exists: it is emitted only while `end < len`.
            listing
                .position_in(t)
                .filter(|start| *start < len)
                .ok_or_else(|| t.to_string())?
        }
        None => 0,
    };
    let limit = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(page_limit)
        .min(page_limit);
    let end = start.saturating_add(limit).min(len);
    let next = if end < len {
        Some(listing.token_at(end))
    } else {
        None
    };
    Ok((start..end, next))
}
