// SPDX-License-Identifier: CC0-1.0

//! The ARN form a service reads a CALLER-SUPPLIED resource ARN against.
//!
//! An Amazon Resource Name is six colon-separated fields,
//! `arn:<partition>:<vendor>:<region>:<account>:<resource>`, and the resource field
//! may itself carry colons (`…:type:id`) and slashes (`…:type/id`). The third field
//! is the VENDOR — the service namespace, the same value a service publishes as its
//! `aws.api#service$arnNamespace` — and it is what says which service's API can
//! address the named resource at all.
//!
//! Every service whose model binds a `resourceArn` the caller chooses has to read
//! that field: an ARN of another vendor names nothing in this service's namespace,
//! so it is a bad REQUEST (refused on the input, before any lookup) rather than a
//! resource that happens to be missing. The distinction is client-visible — a
//! validation refusal is permanent and caller-side, a not-found is not — so the one
//! parse lives here rather than once per service.
//!
//! `region` and `account` are deliberately allowed to be EMPTY: AWS writes a
//! partition-global or account-less resource that way (`arn:aws:s3:::bucket`), and
//! those ARNs are well formed. The partition, the vendor and the resource are not:
//! an ARN that names no resource, or no vendor, addresses nothing.
//!
//! `service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs` still
//! carries its own copy of this parse (as `arn_vendor`), inlined before this module
//! existed and pinned by that service's own refusal-wording suite; it is to be
//! migrated onto this one rather than copied again.

/// The vendor (service namespace) field of the ARN `value`, or `None` when `value`
/// is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The resource field
/// keeps every colon it carries, so an ARN of the `…:type:id` form parses exactly
/// like one of the `…:type/id` form.
#[must_use]
pub fn vendor(value: &str) -> Option<&str> {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let vendor = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    (!partition.is_empty() && !vendor.is_empty() && !resource.is_empty()).then_some(vendor)
}

/// Whether `value` is a well-formed resource ARN whose vendor field is `expected`.
///
/// The comparison is exact: an ARN namespace is a fixed lowercase token, so a value
/// differing in case names another vendor.
#[must_use]
pub fn is_resource_arn_of(value: &str, expected: &str) -> bool {
    vendor(value) == Some(expected)
}

#[cfg(test)]
mod tests {
    use super::{is_resource_arn_of, vendor};

    /// The vendor field is read from the third position, whatever the resource
    /// field's own shape, and only from a value that carries all six fields.
    #[test]
    fn the_vendor_is_the_third_field_of_a_six_field_arn() {
        // (case, value, the vendor the parse must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a slash-form resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                Some("aws-marketplace"),
            ),
            (
                "a colon-form resource carrying further colons",
                "arn:aws:states:us-east-1:000000000000:execution:machine:run",
                Some("states"),
            ),
            (
                "an empty region and account",
                "arn:aws:s3:::bucket/key",
                Some("s3"),
            ),
            (
                "another partition",
                "arn:aws-cn:aws-marketplace:cn-north-1:000000000000:deployment-parameter/dp-1",
                Some("aws-marketplace"),
            ),
            ("not an ARN at all", "dp-0000000000000001", None),
            ("a single character", "-", None),
            ("the empty string", "", None),
            (
                "an ARN that stops before its resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000",
                None,
            ),
            (
                "an ARN with an empty resource",
                "arn:aws:aws-marketplace:us-east-1:000000000000:",
                None,
            ),
            (
                "an ARN with an empty vendor",
                "arn:aws::us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
            (
                "an ARN with an empty partition",
                "arn::aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
            (
                "a value whose first field is not arn",
                "urn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1",
                None,
            ),
        ];

        for (case, value, expected) in rows {
            let read = vendor(value);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected vendor {expected:?}, got {read:?}"
            );
        }
    }

    #[test]
    fn a_vendor_match_is_exact() {
        let arn = "arn:aws:aws-marketplace:us-east-1:000000000000:deployment-parameter/dp-1";
        // (case, the vendor asked about, whether the ARN is one of its resources)
        let rows: &[(&str, &str, bool)] = &[
            ("its own vendor", "aws-marketplace", true),
            ("another vendor", "secretsmanager", false),
            ("its own vendor in another case", "AWS-Marketplace", false),
            ("a prefix of its vendor", "aws", false),
            ("the empty vendor", "", false),
        ];
        for (case, expected, holds) in rows {
            let answered = is_resource_arn_of(arn, expected);
            assert_eq!(
                answered, *holds,
                "case {case:?}: expected {holds}, got {answered}"
            );
        }
        assert!(
            !is_resource_arn_of("dp-1", "aws-marketplace"),
            "a value that is not an ARN is nobody's resource ARN"
        );
    }
}
