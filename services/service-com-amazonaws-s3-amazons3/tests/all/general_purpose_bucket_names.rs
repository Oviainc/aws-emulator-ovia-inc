// SPDX-License-Identifier: CC0-1.0

//! Left to the issue that owns it: the published *General purpose bucket naming rules* —
//! "between 3 (min) and 63 (max) characters long", "only of lowercase letters, numbers, dots
//! (.), and hyphens (-)", "must begin and end with a letter or number", no two adjacent
//! periods, not "formatted as an IP address", no `xn--` prefix, and none of the suffixes
//! reserved for access point aliases, Multi-Region Access Points, directory buckets and
//! table buckets.
//!
//! `CreateBucket` does not enforce them yet, which is a cause of
//! `20261030T053639Z-s3-answers-success-where-aws-refuses-the-request.md` (its cause 6, with
//! the `<BucketName>` member that refusal's envelope carries). This suite held the table for
//! that rule while this fix tried it; enforcing it refused bucket names that unrelated
//! service crates' own fixtures create, so the rule belongs with the issue that owns it,
//! together with those fixtures — not here, where it would land as a side effect of an
//! object-state fix.
//!
//! The file is kept, empty, so the table has a home when that issue is fixed.
