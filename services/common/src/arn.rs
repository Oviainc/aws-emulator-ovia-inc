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

/// The six fields of a well-formed resource ARN, as a caller submitted them.
///
/// Held as borrows into the value that was parsed, so reading an ARN allocates nothing.
/// `resource` keeps every colon it carries, so an ARN of the `…:type:id` form parses
/// exactly like one of the `…:type/id` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arn<'a> {
    /// The second field (`aws`, `aws-cn`, `aws-us-gov`, …), never empty.
    pub partition: &'a str,
    /// The third field: the service namespace that says whose resource this is, never
    /// empty.
    pub vendor: &'a str,
    /// The fourth field, empty for a partition-global resource.
    pub region: &'a str,
    /// The fifth field, empty for an account-less resource.
    pub account: &'a str,
    /// The sixth field and everything after it, colons included; never empty.
    pub resource: &'a str,
}

impl<'a> Arn<'a> {
    /// The resource TYPE the resource field names: the token before its first `/` or
    /// `:` (`custom-model` of `custom-model/imported/abc123def456`, `execution` of
    /// `execution:machine:run`). The whole resource field when it carries neither.
    #[must_use]
    pub fn resource_type(&self) -> &'a str {
        self.resource
            .split(['/', ':'])
            .next()
            .unwrap_or(self.resource)
    }
}

/// `value`'s six fields, or `None` when `value` is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The ONE parse: a
/// service that needs more than the vendor (the region an ARN names, the resource type
/// it addresses) reads it from here rather than splitting the value again, so no two
/// services can disagree about what an ARN is.
#[must_use]
pub fn parse(value: &str) -> Option<Arn<'_>> {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let vendor = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    (!partition.is_empty() && !vendor.is_empty() && !resource.is_empty()).then_some(Arn {
        partition,
        vendor,
        region,
        account,
        resource,
    })
}

/// The vendor (service namespace) field of the ARN `value`, or `None` when `value`
/// is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The resource field
/// keeps every colon it carries, so an ARN of the `…:type:id` form parses exactly
/// like one of the `…:type/id` form.
#[must_use]
pub fn vendor(value: &str) -> Option<&str> {
    parse(value).map(|arn| arn.vendor)
}

/// Whether `value` is a well-formed resource ARN whose vendor field is `expected`.
///
/// The comparison is exact: an ARN namespace is a fixed lowercase token, so a value
/// differing in case names another vendor.
#[must_use]
pub fn is_resource_arn_of(value: &str, expected: &str) -> bool {
    vendor(value) == Some(expected)
}

/// `value`'s fields when it is a well-formed resource ARN whose vendor is `expected`,
/// the form a service reads its OWN resource ARNs with.
#[must_use]
pub fn parse_of<'a>(value: &'a str, expected: &str) -> Option<Arn<'a>> {
    parse(value).filter(|arn| arn.vendor == expected)
}

#[cfg(test)]
mod tests {
    use super::{is_resource_arn_of, parse, parse_of, vendor};

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

    /// The five fields a row expects, in the order `parse` reads them: partition,
    /// vendor, region, account and the resource TYPE.
    type Fields<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

    /// Every field is read from its own position, and the resource TYPE is the token
    /// before the first `/` or `:` of the resource field whatever follows it.
    #[test]
    fn every_field_is_read_from_its_own_position() {
        // (case, value, (partition, vendor, region, account, resource type))
        let rows: &[(&str, &str, Option<Fields<'_>>)] = &[
            (
                "a slash-form resource",
                "arn:aws:bedrock:us-west-2:000000000000:custom-model/imported/abc123def456",
                Some((
                    "aws",
                    "bedrock",
                    "us-west-2",
                    "000000000000",
                    "custom-model",
                )),
            ),
            (
                "a colon-form resource carrying further colons",
                "arn:aws:states:eu-west-1:000000000000:execution:machine:run",
                Some(("aws", "states", "eu-west-1", "000000000000", "execution")),
            ),
            (
                "an account-less resource in another partition",
                "arn:aws-cn:bedrock:cn-north-1::foundation-model/vendor.model",
                Some(("aws-cn", "bedrock", "cn-north-1", "", "foundation-model")),
            ),
            (
                "a partition-global resource",
                "arn:aws:s3:::bucket",
                Some(("aws", "s3", "", "", "bucket")),
            ),
            ("not an ARN at all", "abc123def456", None),
        ];

        for (case, value, expected) in rows {
            let read = parse(value).map(|arn| {
                (
                    arn.partition,
                    arn.vendor,
                    arn.region,
                    arn.account,
                    arn.resource_type(),
                )
            });
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// `parse_of` is `parse` narrowed to one vendor, with the same exact comparison
    /// `is_resource_arn_of` makes.
    #[test]
    fn parsing_for_one_vendor_answers_only_that_vendors_arns() {
        let arn = "arn:aws:bedrock:us-west-2:000000000000:provisioned-model/abc123def456";
        // (case, the vendor asked about, the region the parse must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("its own vendor", "bedrock", Some("us-west-2")),
            ("another vendor", "kms", None),
            ("its own vendor in another case", "Bedrock", None),
        ];
        for (case, expected, region) in rows {
            let read = parse_of(arn, expected).map(|parsed| parsed.region);
            assert_eq!(
                read, *region,
                "case {case:?}: expected {region:?}, got {read:?}"
            );
        }
        assert_eq!(
            parse_of("provisioned-model/abc123def456", "bedrock"),
            None,
            "a value that is not an ARN is nobody's resource ARN"
        );
    }
}
