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
//! [`region_of`] is the one reader with a grammar of its own: a service that resolves a
//! caller-supplied ARN against the vendor's endpoint in the ARN's own Region puts that
//! label in a URL, so it is read under the Region grammar the models state rather than
//! as the arbitrary text [`Arn::region`] reports.
//!
//! `service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs` still
//! carries its own copy of this parse (as `arn_vendor`), inlined before this module
//! existed and pinned by that service's own refusal-wording suite; it is to be
//! migrated onto this one rather than copied again.
//!
//! Beside the vendor, a service that reads WHICH resource an ARN names gets the
//! resource field ([`resource`]), the resource TYPE ([`resource_type`]) and the id
//! of a resource of a given type ([`resource_id_of_type`]) from here, so the
//! `<type>/<id>` and `<type>:<id>` forms are read in one place too.

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

/// The whole `resource` field of the ARN `value` (`thing/laser-1`,
/// `execution:machine:run`), or `None` when `value` is not a well-formed resource
/// ARN as [`vendor`] defines it.
#[must_use]
pub fn resource(value: &str) -> Option<&str> {
    parse(value).map(|arn| arn.resource)
}

/// The TYPE of resource the ARN `value` names: the `thing` of `thing/laser-1`, the
/// `execution` of `execution:machine:run`, and the whole resource field when it
/// carries no id (`arn:aws:iot:…:job`).
///
/// AWS writes a resource in either form — `<type>/<id>` and `<type>:<id>` — so both
/// separators end the type. `None` for a value that is not a resource ARN, and for
/// one whose resource field begins with a separator and so names no type.
#[must_use]
pub fn resource_type(value: &str) -> Option<&str> {
    let resource_type = parse(value)?.resource_type();
    (!resource_type.is_empty()).then_some(resource_type)
}

/// The ID of the resource the ARN `value` names, when that resource is of type
/// `resource_type` (`thing/laser-1` with `thing` -> `laser-1`); `None` when `value`
/// names another type, or is not a resource ARN.
///
/// The id may be empty (`thing/`): a caller that requires a non-empty id checks it,
/// because "no id" and "another type" are different refusals.
#[must_use]
pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str> {
    let rest = resource(value)?.strip_prefix(resource_type)?;
    rest.strip_prefix('/').or_else(|| rest.strip_prefix(':'))
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

/// The longest Region label this parse will report.
///
/// Every `*Arn` pattern in the AWS models bounds the Region field at `[a-z0-9-]{1,20}`,
/// so a longer label is not a Region any AWS endpoint has.
const MAX_REGION_LENGTH: usize = 20;

/// The REGION of `value` when it is a well-formed resource ARN of `expected_vendor` AND
/// that field is a Region LABEL rather than arbitrary text.
///
/// For a service that has to ACT on a caller-supplied ARN's Region — reaching the
/// vendor's endpoint in that Region to resolve the resource the ARN names. A Region read
/// for that is not just data: it ends up in an endpoint URL's authority, so this is
/// stricter than [`Arn::region`], which reports the field as submitted. The label must
/// be non-empty, at most [`MAX_REGION_LENGTH`] characters, and composed only of
/// `[a-z0-9-]` — the grammar every model's ARN pattern states for it — so a value
/// carrying `/`, `@`, `:`, `?` or `#` (which would re-point or truncate an authority) is
/// reported as no Region at all and the caller falls back to its own default.
///
/// `None` therefore means "this value names no Region of that vendor", which covers a
/// value that is not an ARN, an ARN of another vendor, a partition-global ARN whose
/// Region field is empty, and a Region field outside the grammar.
#[must_use]
pub fn region_of<'value>(value: &'value str, expected_vendor: &str) -> Option<&'value str> {
    let region = parse_of(value, expected_vendor)?.region;
    let well_formed = !region.is_empty()
        && region.len() <= MAX_REGION_LENGTH
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    well_formed.then_some(region)
}

#[cfg(test)]
mod tests {
    use super::{
        is_resource_arn_of, parse, parse_of, region_of, resource_id_of_type, resource_type, vendor,
    };

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

    /// The resource type is what precedes the first separator of the resource
    /// field, in either form AWS writes a resource in, and the id is read back
    /// only for the type that is asked about.
    #[test]
    fn the_resource_type_and_id_are_read_out_of_the_resource_field() {
        // (case, value, the type, the id of a `thing`)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "a slash-form thing",
                "arn:aws:iot:us-east-1:000000000000:thing/laser-1",
                Some("thing"),
                Some("laser-1"),
            ),
            (
                "a colon-form thing",
                "arn:aws:iot:us-east-1:000000000000:thing:laser-1",
                Some("thing"),
                Some("laser-1"),
            ),
            (
                "an id carrying further separators",
                "arn:aws:states:us-east-1:000000000000:execution:machine:run",
                Some("execution"),
                None,
            ),
            (
                "another type",
                "arn:aws:iot:us-east-1:000000000000:job/upgrade-42",
                Some("job"),
                None,
            ),
            (
                "a type that only prefixes the one asked about",
                "arn:aws:iot:us-east-1:000000000000:thinggroup/plant-floor",
                Some("thinggroup"),
                None,
            ),
            (
                "a resource with no id at all",
                "arn:aws:iot:us-east-1:000000000000:job",
                Some("job"),
                None,
            ),
            (
                "a thing with an empty id",
                "arn:aws:iot:us-east-1:000000000000:thing/",
                Some("thing"),
                Some(""),
            ),
            (
                "a resource field that begins with a separator",
                "arn:aws:iot:us-east-1:000000000000:/laser-1",
                None,
                None,
            ),
            ("not an ARN at all", "laser-1", None, None),
            (
                "an ARN with an empty resource",
                "arn:aws:iot:us-east-1:000000000000:",
                None,
                None,
            ),
        ];

        for (case, value, expected_type, expected_id) in rows {
            let read_type = resource_type(value);
            assert_eq!(
                read_type, *expected_type,
                "case {case:?}: expected resource type {expected_type:?}, got {read_type:?}"
            );
            let read_id = resource_id_of_type(value, "thing");
            assert_eq!(
                read_id, *expected_id,
                "case {case:?}: expected thing id {expected_id:?}, got {read_id:?}"
            );
        }
    }

    /// The Region a service may ACT on: read only from an ARN of the vendor asked
    /// about, and only when the field is a Region label rather than arbitrary text —
    /// because the caller puts it in an endpoint URL's authority.
    #[test]
    fn the_region_is_read_only_from_a_well_formed_label() {
        // (case, value, vendor asked about, the Region the parse must report)
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "its own vendor",
                "arn:aws:lambda:us-east-1:000000000000:function:tools",
                "lambda",
                Some("us-east-1"),
            ),
            (
                "a Region of another partition",
                "arn:aws-cn:lambda:cn-north-1:000000000000:function:tools",
                "lambda",
                Some("cn-north-1"),
            ),
            (
                "a GovCloud Region, at the published length",
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:tools",
                "lambda",
                Some("us-gov-west-1"),
            ),
            (
                "another vendor's ARN",
                "arn:aws:s3:us-east-1:000000000000:bucket/key",
                "lambda",
                None,
            ),
            (
                "a partition-global ARN, whose Region field is empty",
                "arn:aws:lambda::000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying a path separator",
                "arn:aws:lambda:us-east-1/evil.example.com:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying userinfo",
                "arn:aws:lambda:us-east-1@evil.example.com:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field carrying a query",
                "arn:aws:lambda:us-east-1?x=1:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field in upper case",
                "arn:aws:lambda:US-EAST-1:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field one character past the published bound",
                "arn:aws:lambda:us-east-1-us-east-1xy:000000000000:function:tools",
                "lambda",
                None,
            ),
            (
                "a Region field exactly at the published bound",
                "arn:aws:lambda:us-east-1-us-east-1x:000000000000:function:tools",
                "lambda",
                Some("us-east-1-us-east-1x"),
            ),
            ("not an ARN at all", "my-function", "lambda", None),
            ("the empty string", "", "lambda", None),
        ];

        for (case, value, expected_vendor, expected) in rows {
            let read = region_of(value, expected_vendor);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected region {expected:?}, got {read:?}"
            );
        }
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
