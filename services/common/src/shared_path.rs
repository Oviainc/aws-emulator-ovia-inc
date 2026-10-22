// SPDX-License-Identifier: CC0-1.0

//! The ARN a request addresses on a URI path space that SEVERAL models sharing one
//! SigV4 signing name all bind.
//!
//! [`crate::claims_request`] separates colliding services by their modelled path
//! spaces, which settles every request except one on a path space two of them publish
//! IDENTICALLY — the `/tags/{resourceArn}`-style tagging URIs are the recurring case
//! (the Lambda control plane and Lambda MicroVMs both bind
//! `/2017-03-31/tags/{Resource}`; the two `ivs`-signing services both bind
//! `/tags/{resourceArn}`). There the ARN in the path is what names the resource, and a
//! resource belongs to exactly ONE service: the one whose own operations mint it. So
//! the resource TYPE decides the claim, and [`shared_path_arn`] is the one reader of it.
//!
//! # Services still carrying their own copy
//!
//! The two `ivs`-signing crates inline an equivalent reader in their `can_handle` —
//! `fn tagging_target` + `enum TaggingTarget`
//! (`grep -rln 'fn tagging_target' services/*/src/service.rs`), which read the same
//! percent-decoded label and classify it the same three ways, additionally requiring a
//! non-empty region and account. They are to be MIGRATED onto
//! `shared_path_arn(path, &["tags"], "ivs")`; they are left in place only because this
//! module's change does not own those services' issues, and each is pinned by
//! `core/router/tests/all/ivs_signing_name_routing.rs`. Nothing new should inline the
//! rule again — call this module.

/// What a request path says about a path space several colliding models bind, and whose
/// resource is named by an ARN in the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedPathArn {
    /// Not on the shared path space at all: the modelled path spaces have already
    /// decided the claim and this discriminator adds nothing.
    NotShared,
    /// On the shared path space, naming a resource of this (lower-cased) type.
    Resource(String),
    /// On the shared path space, naming no resource in the shared ARN namespace — an
    /// absent label, a label that is not an ARN, or another service's ARN.
    Unrecognised,
}

/// Read the ARN a request addresses on a path space several colliding models bind:
/// whether this request is on that space at all, and which resource TYPE its ARN label
/// names.
///
/// * `path` — the request's URI path.
/// * `shared_prefix` — the literal path segments that LEAD to the ARN label, in order
///   (`["2017-03-31", "tags"]` for `/2017-03-31/tags/{Resource}`). A path whose
///   segments do not begin with them is [`SharedPathArn::NotShared`].
/// * `arn_namespace` — the `aws.api#service$arnNamespace` the colliding models share,
///   and so the only service field an ARN either of them addresses can carry.
///
/// The label is percent-decoded (a non-greedy `@httpLabel` has its `:` and `/` encoded
/// by most clients, and sent raw by some), and the resource type is lower-cased, so a
/// non-canonical ARN cannot land a request on the wrong service. The prefix segments
/// are compared RAW, because the caller's path conjunct has already matched the first
/// of them against the model's `ROUTE_PATH_SEGMENTS` by exact equality.
///
/// The label counts as naming a resource only when it is a well-formed resource ARN in
/// that namespace — `arn:<partition>:<namespace>:<region>:<account>:<resource>`, read
/// through [`crate::arn::vendor`], the one parse this crate states for a caller-supplied
/// ARN — so another service's ARN, or a string that merely has a sixth `:`-field, names
/// nothing. The resource TYPE is that resource field's own first token under either ARN
/// convention (`<type>:<id>` or `<type>/<id>`).
///
/// The namespace is compared case-INSENSITIVELY here (rather than exactly, as
/// [`crate::arn::is_resource_arn_of`] does) for the same reason the resource type is
/// lower-cased: a non-canonically spelled ARN must not land a request on a service that
/// does not own the resource it names.
///
/// What decides ownership is the NAMESPACE and the resource TYPE, so the region and
/// account fields are read but not required to be non-empty: an ARN shape may leave
/// either empty (a global service's ARNs carry no region, and an `@pattern` may allow
/// both — `^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$` does), and
/// such an ARN still names the same resource type and therefore the same service.
pub fn shared_path_arn(path: &str, shared_prefix: &[&str], arn_namespace: &str) -> SharedPathArn {
    let mut segments = path.trim_start_matches('/').split('/');
    for expected in shared_prefix {
        if segments.next() != Some(*expected) {
            return SharedPathArn::NotShared;
        }
    }
    let Some(label) = segments.next().filter(|label| !label.is_empty()) else {
        return SharedPathArn::Unrecognised;
    };
    let arn = smithy_server_runtime::percent_decode(label);
    if !crate::arn::vendor(&arn).is_some_and(|vendor| vendor.eq_ignore_ascii_case(arn_namespace)) {
        return SharedPathArn::Unrecognised;
    }
    // `arn::vendor` has already established the six fields and a non-empty resource;
    // `splitn(6, ':')` leaves that whole resource — colons included — as the last one.
    let resource = arn.splitn(6, ':').nth(5).unwrap_or_default();
    let kind = resource
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if kind.is_empty() {
        return SharedPathArn::Unrecognised;
    }
    SharedPathArn::Resource(kind)
}

#[cfg(test)]
mod tests {
    //! [`shared_path_arn`] over the shapes an ARN label arrives in: both ARN
    //! conventions, both spellings of the label's separators, a foreign namespace and a
    //! label that is no ARN at all.

    use super::*;

    #[test]
    fn a_shared_paths_arn_label_names_the_resource_type_it_carries() {
        const PREFIX: &[&str] = &["2017-03-31", "tags"];
        const NAMESPACE: &str = "lambda";

        /// The label as a client that percent-encodes a non-greedy `@httpLabel` sends
        /// it.
        fn encoded(arn: &str) -> String {
            arn.replace(':', "%3A").replace('/', "%2F")
        }

        // (case, path, expected)
        let rows: Vec<(&str, String, SharedPathArn)> = vec![
            (
                "a colon-separated ARN, raw",
                "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:function:probe".to_string(),
                SharedPathArn::Resource("function".to_string()),
            ),
            (
                "a colon-separated ARN, percent-encoded",
                format!(
                    "/2017-03-31/tags/{}",
                    encoded("arn:aws:lambda:eu-west-1:123456789012:microvm-image:img")
                ),
                SharedPathArn::Resource("microvm-image".to_string()),
            ),
            (
                "a slash-separated ARN",
                format!(
                    "/2017-03-31/tags/{}",
                    encoded("arn:aws:lambda:us-east-2:210987654321:layer/shared")
                ),
                SharedPathArn::Resource("layer".to_string()),
            ),
            (
                "a qualified ARN keeps its resource type",
                "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:function:probe:3"
                    .to_string(),
                SharedPathArn::Resource("function".to_string()),
            ),
            (
                "a non-canonically cased resource type",
                "/2017-03-31/tags/arn:AWS:Lambda:us-east-1:000000000000:Function:probe".to_string(),
                SharedPathArn::Resource("function".to_string()),
            ),
            (
                "another namespace's ARN",
                "/2017-03-31/tags/arn:aws:sqs:us-east-1:000000000000:function:probe".to_string(),
                SharedPathArn::Unrecognised,
            ),
            (
                "a label that is not an ARN",
                "/2017-03-31/tags/not-an-arn".to_string(),
                SharedPathArn::Unrecognised,
            ),
            (
                "no label at all",
                "/2017-03-31/tags".to_string(),
                SharedPathArn::Unrecognised,
            ),
            (
                // An ARN shape may leave the region and the account empty (the MicroVM
                // image ARN pattern allows both), and such an ARN names the same
                // resource type, so it must reach the same service.
                "an ARN with no account id",
                "/2017-03-31/tags/arn:aws:lambda:us-east-1::function:probe".to_string(),
                SharedPathArn::Resource("function".to_string()),
            ),
            (
                "an ARN with neither region nor account",
                "/2017-03-31/tags/arn:aws:lambda:::microvm-image:img".to_string(),
                SharedPathArn::Resource("microvm-image".to_string()),
            ),
            (
                "an ARN with no resource type",
                "/2017-03-31/tags/arn:aws:lambda:us-east-1:000000000000:".to_string(),
                SharedPathArn::Unrecognised,
            ),
            (
                "a path outside the shared space",
                "/2015-03-31/functions".to_string(),
                SharedPathArn::NotShared,
            ),
            (
                "the shared space's second segment only",
                "/tags/arn:aws:lambda:us-east-1:000000000000:function:probe".to_string(),
                SharedPathArn::NotShared,
            ),
            ("the root path", "/".to_string(), SharedPathArn::NotShared),
        ];

        for (case, path, expected) in rows {
            let actual = shared_path_arn(&path, PREFIX, NAMESPACE);
            assert_eq!(
                actual, expected,
                "case {case:?} ({path}): expected {expected:?}, got {actual:?}"
            );
        }
    }
}
