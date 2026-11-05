// SPDX-License-Identifier: CC0-1.0

//! How a request ADDRESSES a resource: the `TableName` member that takes a name or an
//! ARN, the tag-carrying `ResourceArn`, and the tag set a request states.

use super::*;

/// The requirement phrase for a value that is spelled as an ARN and is not a valid one.
///
/// A member that takes either a name or an ARN (`TableArn`, which the data-plane
/// `TableName` members target) states the ARN FORM rather than the name's pattern when
/// the value it refused was an attempt at an ARN: the pattern a bare table name must
/// satisfy says nothing about which ARN field was wrong.
pub const ARN_FORM_REQUIREMENT: &str = "Valid ARN format is \
     'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'";

/// The form a table name must take: 3..=255 characters of
/// `[a-zA-Z0-9_.-]` (the `CreateTable$TableName` doc's own length and pattern —
/// the data-plane members target `TableArn`, which widens the modelled bound to
/// 1..=1024 so an ARN fits, while the name inside it keeps these).
pub(super) const MIN_TABLE_NAME_LENGTH: usize = 3;
pub(super) const MAX_TABLE_NAME_LENGTH: usize = 255;
pub(super) const TABLE_NAME_PATTERN: &str = "^[a-zA-Z0-9_.-]+$";

/// How AWS's refusals name the table-name member: the `tableName` path its own
/// request validator reports this constraint against, with the rejected name
/// echoed.
///
/// NOT the wire spelling `src/service.rs` restores for a violation of a bound
/// the MODEL declares, and deliberately so: the public model does not carry this
/// bound at all (the data-plane members target `TableArn`, length 1..=1024), so
/// this is a check the service itself makes, and the recorded AWS refusals for
/// it state the validator's own form — `Value 'ab' at 'tableName' failed to
/// satisfy constraint: Member must have length greater than or equal to 3` for
/// a two-character name on `GetItem`. The same clause is stated for every
/// operation that resolves a table name, and the echoed value is the name the
/// caller sent, never a figure of this emulator's.
pub(super) const TABLE_NAME_MEMBER_PATH: &str = "tableName";

/// The `TableName` member accepts a bare table name or the table's ARN ("You can
/// also provide the Amazon Resource Name (ARN) of the table in this parameter" —
/// the `CreateTable$TableName` doc). Resolve either to the bare name, refusing a
/// name outside the documented 3..=255 `[a-zA-Z0-9_.-]+` form.
///
/// A name outside that form is refused the way DynamoDB's request validator words a
/// constraint violation ([`invalid_member`]), because that is what this check is: the
/// member, the value and the constraint it broke.
pub fn resolve_table_name(table_name: &str) -> Result<String, RequestError> {
    // A value spelled as an ARN is refused in one of TWO ways, and which one depends on
    // whether it is an ARN at all:
    //
    //   * a string that is NOT a well-formed ARN — a required field missing or empty —
    //     is refused by the ARN FORM, because that is what is wrong with it;
    //   * a well-formed ARN that simply does not name a table of this service is no way
    //     of naming a table, so it is refused as the table NAME it would have to be,
    //     by the pattern a table name must satisfy.
    let malformed_arn = || {
        invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(table_name),
            ARN_FORM_REQUIREMENT,
        )
    };
    let not_a_table_arn = || {
        invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(table_name),
            &pattern_requirement(TABLE_NAME_PATTERN),
        )
    };
    // A well-formed ARN of THIS service whose resource is not a table names some other
    // kind of resource: the request did address an ARN, and what is wrong with it is the
    // resource TYPE, which is the fact AWS states — over the whole value, since the type
    // is a field of it.
    let invalid_resource_type = || {
        refused(format!("Invalid resource type in '{table_name}'"))
    };
    let name = match table_name.strip_prefix("arn:") {
        Some(rest) => {
            let fields: Vec<&str> = rest.splitn(5, ':').collect();
            let [partition, service, region, account, resource] = fields.as_slice() else {
                return Err(malformed_arn());
            };
            if partition.is_empty()
                || service.is_empty()
                || region.is_empty()
                || account.is_empty()
                || resource.is_empty()
            {
                return Err(malformed_arn());
            }
            if *service != "dynamodb" {
                return Err(not_a_table_arn());
            }
            let path = resource
                .strip_prefix("table/")
                .ok_or_else(invalid_resource_type)?;
            // A sub-resource ARN (`.../index/<i>`, `.../stream/<label>`) names
            // something other than the table, so it is not a way of naming it.
            if path.contains('/') {
                return Err(invalid_resource_type());
            }
            path
        }
        None => table_name,
    };
    let length = name.chars().count();
    if length < MIN_TABLE_NAME_LENGTH {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &min_length_requirement(MIN_TABLE_NAME_LENGTH),
        ));
    }
    if length > MAX_TABLE_NAME_LENGTH {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &max_length_requirement(MAX_TABLE_NAME_LENGTH),
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return Err(invalid_member(
            TABLE_NAME_MEMBER_PATH,
            Some(name),
            &pattern_requirement(TABLE_NAME_PATTERN),
        ));
    }
    Ok(name.to_string())
}

/// The clause AWS states for an ARN that addresses a REGION other than the one the
/// request arrived at: the offending ARN, as the request spelled it.
///
/// An endpoint serves one region, so an ARN naming another names nothing this endpoint
/// can reach. The clause names the VALUE rather than the member it came in: the fact is
/// about the ARN's own region field.
fn invalid_arn_region(table_name: &str) -> RequestError {
    refused(format!("Invalid AWS region in '{table_name}'"))
}

/// A `TableName` resolved against the REGION the request's endpoint names: the same form
/// check [`resolve_table_name`] makes, plus the rule that an ARN spelling of the name
/// must address this endpoint's own region.
///
/// "You can also provide the Amazon Resource Name (ARN) of the table in this parameter"
/// (the `TableName` docs), and an ARN carries a region — while a DynamoDB endpoint
/// serves exactly one, which is the scope a table name is unique in. So an ARN naming
/// another region addresses a table this endpoint has no way of reaching, and the
/// request is refused rather than served from the same-named table of the region it
/// reached. A BARE name carries no region and is the endpoint's own, so it is unaffected.
pub fn resolve_table_name_in(table_name: &str, region: &str) -> Result<String, RequestError> {
    let name = resolve_table_name(table_name)?;
    if table_arn_region(table_name).is_some_and(|addressed| addressed != region) {
        return Err(invalid_arn_region(table_name));
    }
    Ok(name)
}

/// The REGION field of a value spelled as an ARN, or `None` when the value is not
/// spelled as one — a bare table name addresses no region of its own.
fn table_arn_region(table_name: &str) -> Option<&str> {
    table_name
        .strip_prefix("arn:")?
        .split(':')
        .nth(2)
        .filter(|region| !region.is_empty())
}

/// The DynamoDB resource an ARN-addressed operation's `ResourceArn` names: the region
/// it lives in, the table it is, and the ARN as the request spelled it.
///
/// Such a call carries no `TableName`, so the ARN is the caller's whole selector — and
/// it is also the only identifier the request holds, which is why it is kept here: a
/// refusal about this resource echoes `arn`, never the `table` derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub region: String,
    pub table: String,
    pub arn: String,
}

/// Resolve an ARN-addressed operation's `ResourceArn` to the table it names, in the
/// scope the request itself is made in.
///
/// The one place an ARN is classified for the tagging and resource-policy surfaces, so
/// `TagResource`, `UntagResource`, `ListTagsOfResource` and the three `*ResourcePolicy`
/// operations share a single rule, in two parts:
///
/// * SHAPE — the value must be an ARN of THIS service
///   (`arn:<partition>:dynamodb:<region>:<account>:table/<name>`) naming a table and
///   nothing deeper: a sub-resource ARN (`.../index/<i>`, `.../stream/<label>`) is not
///   a resource this emulator keeps a tag set or a policy for, so it is refused rather
///   than silently taken for its table;
/// * SCOPE — the ARN must address a resource of the endpoint the request reached: the
///   same partition, account and REGION this request is signed for, which is the one
///   this service answers for. [`table_arn`] mints exactly that form for a table of
///   this region, so the comparison against it is the whole scope check.
///
/// Either way the value names no resource whose existence could be looked up, so it is
/// an INPUT failure ([`invalid_resource_arn`], 400 `ValidationException`) and not a
/// missing resource — an out-of-scope ARN is not "absent here", it is unaddressable
/// from here. Only a well-formed, in-scope ARN reaches the lookup, where a table that
/// is not there is [`resource_arn_not_found`].
pub fn resolve_resource_arn(
    resource_arn: &str,
    request_region: &str,
) -> Result<TaggedResource, RequestError> {
    let invalid = || invalid_resource_arn(resource_arn);
    // `arn`, partition, service, region, account, resource — the resource field is
    // the last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return Err(invalid()),
    };
    if partition.is_empty() || service != "dynamodb" || region.is_empty() {
        return Err(invalid());
    }
    let path = resource.strip_prefix("table/").ok_or_else(invalid)?;
    if path.contains('/') {
        return Err(invalid());
    }
    let table = resolve_table_name(path).map_err(|_| invalid())?;
    // The scope half of the rule: anything but this endpoint's own ARN form for that
    // table — another partition, another account, another region — is unaddressable
    // here, whatever this store happens to hold.
    if resource_arn != table_arn(request_region, &table) {
        return Err(invalid());
    }
    Ok(TaggedResource {
        region: request_region.to_string(),
        table,
        arn: resource_arn.to_string(),
    })
}

/// Fold a request's `TagList` into the stored [`TagSet`], refusing a list AWS does not
/// accept.
///
/// The one place a request's tag list becomes a set, so every operation that takes one
/// (`CreateTable$Tags`, `TagResource$Tags`) enforces the same two rules:
///
/// * a tag KEY may appear at most once in one request — "Each DynamoDB table can only
///   have up to one tag with the same key" (`Tag$Key`), so a list repeating a key asks
///   for two tags that cannot both exist, and is refused ([`duplicate_tag_key`])
///   instead of collapsing to whichever value happens to win;
/// * a key in the reserved `aws:` namespace is refused — the user "cannot assign" it —
///   so no stored key is ever one AWS alone mints.
///
/// Keys are compared as sent: tag keys are case sensitive, so `Env` and `env` are two
/// keys rather than one repeated.
pub fn tag_set_from_request(tags: &[Tag]) -> Result<TagSet, RequestError> {
    let mut set = TagSet::new();
    for tag in tags {
        if tag
            .key
            .to_ascii_lowercase()
            .starts_with(RESERVED_TAG_PREFIX)
        {
            return Err(RequestError::Validation(format!(
                "Invalid tag key {}: the {RESERVED_TAG_PREFIX} prefix is reserved for Amazon \
                 Web Services-assigned tags and cannot be assigned by a caller",
                tag.key
            )));
        }
        if set.insert(tag.key.clone(), tag.value.clone()).is_some() {
            return Err(duplicate_tag_key(&tag.key));
        }
    }
    Ok(set)
}

/// A stored tag set as the modelled `TagList` carries it, in key order.
pub fn tag_list(tags: &TagSet) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}
